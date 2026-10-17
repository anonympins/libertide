use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
use std::sync::mpsc::{channel, Sender as StdSender};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use any_tts::{load_model, ModelType, SynthesisRequest, TtsConfig, TtsModel};
use cpal::traits::{DeviceTrait, HostTrait, StreamTrait};
use crate::set_tts_audio_level;
use crate::types::{AgentEvent, AgentStatus, TtsCommand, VoiceGender};

/// Nettoie le texte Markdown/HTML et convertit les symboles en texte parlé naturel
pub fn sanitize_for_tts(text: &str) -> String {
    // Normalisation préalable des caractères typographiques français et Unicode
    let normalized = text
        .replace(['’', '‘', 'ʼ', 'ʻ'], "'")
        .replace(['«', '»', '“', '”', '"'], " ")
        .replace('…', "...")
        .replace(['—', '–'], ", ")
        .replace('œ', "oe")
        .replace('Œ', "Oe")
        .replace('æ', "ae")
        .replace('Æ', "Ae");

    let mut cleaned = String::new();
    let mut in_code_block = false;

    for line in normalized.lines() {
        let trimmed = line.trim();
        if trimmed.starts_with("```") {
            in_code_block = !in_code_block;
            continue;
        }
        if in_code_block {
            continue;
        }
        cleaned.push_str(line);
        cleaned.push(' ');
    }

    let chars: Vec<char> = cleaned.chars().collect();
    let mut result = String::with_capacity(cleaned.len());
    let mut i = 0;
    let mut in_xml_tag = false;

    while i < chars.len() {
        let c = chars[i];
        if c == '<' {
            in_xml_tag = true;
            i += 1;
            continue;
        }
        if in_xml_tag {
            if c == '>' {
                in_xml_tag = false;
            }
            i += 1;
            continue;
        }
        // Traitement des liens Markdown [texte](url) -> on conserve le texte et on saute l'url
        if c == ']' && i + 1 < chars.len() && chars[i + 1] == '(' {
            i += 2;
            while i < chars.len() && chars[i] != ')' {
                i += 1;
            }
            result.push(' ');
            i += 1;
            continue;
        }
        // Détection et suppression des URLs brutes
        if (c == 'h' || c == 'H') && i + 6 < chars.len() {
            let end = (i + 8).min(chars.len());
            let slice: String = chars[i..end].iter().collect();
            if slice.eq_ignore_ascii_case("https://") || slice.to_lowercase().starts_with("http://") {
                while i < chars.len() && !chars[i].is_whitespace() && chars[i] != ')' && chars[i] != ']' && chars[i] != '>' {
                    i += 1;
                }
                continue;
            }
        }
        match c {
            '*' | '#' | '`' | '_' | '~' | '{' | '}' | '[' | ']' | '|' | '\\' | '^' | '@' => result.push(' '),
            '&' => result.push_str(" et "),
            '%' => result.push_str(" pour cent "),
            '€' => result.push_str(" euros "),
            '$' => result.push_str(" dollars "),
            '+' => result.push_str(" plus "),
            '=' => result.push_str(" égal "),
            '°' => result.push_str(" degrés "),
            '>' | '<' | '/' => result.push(' '),
            _ if !c.is_control() => result.push(c),
            _ => {}
        }
        i += 1;
    }
    result.split_whitespace().collect::<Vec<&str>>().join(" ")
}

/// Supprime les accents typiques du français pour permettre la prononciation
/// par un phonétiseur neural Kokoro anglophone en cas d'absence de modèle phonétique FR dédié
fn strip_accents(text: &str) -> String {
    text.chars().map(|c| match c {
        'é' | 'è' | 'ê' | 'ë' => 'e',
        'É' | 'È' | 'Ê' | 'Ë' => 'E',
        'à' | 'â' | 'ä' => 'a',
        'À' | 'Â' | 'Ä' => 'A',
        'î' | 'ï' => 'i',
        'Î' | 'Ï' => 'I',
        'ô' | 'ö' => 'o',
        'Ô' | 'Ö' => 'O',
        'ù' | 'û' | 'ü' => 'u',
        'Ù' | 'Û' | 'Ü' => 'U',
        'ç' => 'c',
        'Ç' => 'C',
        _ => c,
    }).collect()
}

/// Découpe un texte nettoyé en phrases ou segments naturels pour une synthèse fluide et préemptible
pub fn split_into_sentences(text: &str) -> Vec<String> {
    let mut sentences = Vec::new();
    let mut current = String::new();
    let chars: Vec<char> = text.chars().collect();
    let len = chars.len();
    let mut i = 0;

    const ABBREVIATIONS: &[&str] = &[
        "m", "mr", "mme", "mlle", "dr", "pr", "prof", "ex", "etc", "vs", "av", "apr", "st", "ste",
    ];

    while i < len {
        let c = chars[i];
        current.push(c);

        let is_sentence_ender = c == '.' || c == '!' || c == '?' || c == ';' || c == ':';
        if is_sentence_ender {
            // Vérification si c'est un point décimal (ex: 3.5 ou 1.0)
            let is_decimal = c == '.'
                && i > 0
                && chars[i - 1].is_ascii_digit()
                && i + 1 < len
                && chars[i + 1].is_ascii_digit();

            // Vérification si c'est une abréviation
            let is_abbreviation = if c == '.' {
                let last_word = current
                    .trim_end_matches('.')
                    .split_whitespace()
                    .last()
                    .unwrap_or("")
                    .to_lowercase();
                ABBREVIATIONS.contains(&last_word.as_str())
            } else {
                false
            };

            let next_is_space_or_end = i + 1 >= len || chars[i + 1].is_whitespace();

            if !is_decimal && !is_abbreviation && next_is_space_or_end {
                while i + 1 < len && (chars[i + 1] == '.' || chars[i + 1] == '!' || chars[i + 1] == '?') {
                    i += 1;
                    current.push(chars[i]);
                }
                let trimmed = current.trim();
                if !trimmed.is_empty() {
                    sentences.push(trimmed.to_string());
                }
                current.clear();
            }
        } else if c == '\n' {
            let trimmed = current.trim();
            if !trimmed.is_empty() {
                sentences.push(trimmed.to_string());
            }
            current.clear();
        } else if current.len() >= 180 && (c == ',' || c == '—' || c == '-') && i + 1 < len && chars[i + 1].is_whitespace() {
            let trimmed = current.trim();
            if !trimmed.is_empty() {
                sentences.push(trimmed.to_string());
            }
            current.clear();
        }
        i += 1;
    }

    let remainder = current.trim();
    if !remainder.is_empty() {
        sentences.push(remainder.to_string());
    }

    sentences
        .into_iter()
        .filter(|s| s.chars().any(|c| c.is_alphanumeric()))
        .collect()
}

#[cfg(windows)]
unsafe fn token_matches_attributes(
    token: &windows::Win32::Media::Speech::ISpObjectToken,
    attrs: windows::core::PCWSTR,
) -> bool {
    let mut matches = windows::Win32::Foundation::BOOL(0);
    token.MatchesAttributes(attrs, &mut matches).is_ok() && matches.as_bool()
}

#[cfg(windows)]
unsafe fn find_and_apply_voice(
    voice: &windows::Win32::Media::Speech::ISpVoice,
    profile: VoiceGender,
) {
    use windows::core::{w, PCWSTR};
    use windows::Win32::Foundation::BOOL;
    use windows::Win32::Media::Speech::{
        ISpObjectToken, ISpObjectTokenCategory, SpObjectTokenCategory,
    };
    use windows::Win32::System::Com::{CoCreateInstance, CLSCTX_ALL};

    extern "system" {
        fn CoTaskMemFree(pv: *const std::ffi::c_void);
    }

    let (preferred_names, is_female): (&[&str], bool) = match profile {
        VoiceGender::Denise => (&["denise", "hortense", "julie"], true),
        VoiceGender::Henri => (&["henri", "paul", "claude"], false),
        VoiceGender::Hortense => (&["hortense", "denise", "julie"], true),
        VoiceGender::Paul => (&["paul", "henri", "claude"], false),
        VoiceGender::Sylvie => (&["sylvie", "harmonie", "denise"], true),
        VoiceGender::Jean => (&["jean", "mathieu", "henri", "paul"], false),
        VoiceGender::Kokoro => (&["denise", "hortense", "julie"], true),
    };

    // Recherche dans les voix modernes OneCore (où se trouvent Denise & Henri)
    // puis dans les voix Desktop SAPI classiques (Hortense & Paul)
    let category_paths = [
        w!("HKEY_LOCAL_MACHINE\\SOFTWARE\\Microsoft\\Speech_OneCore\\Voices"),
        w!("HKEY_LOCAL_MACHINE\\SOFTWARE\\Microsoft\\Speech\\Voices"),
    ];

    let mut fallback_gender_token: Option<ISpObjectToken> = None;
    let mut fallback_french_token: Option<ISpObjectToken> = None;

    for &cat_path in &category_paths {
        let category: Result<ISpObjectTokenCategory, _> =
            CoCreateInstance(&SpObjectTokenCategory, None, CLSCTX_ALL);
        let Ok(category) = category else { continue };

        if category.SetId(cat_path, BOOL(0)).is_err() {
            continue;
        }

        let Ok(enum_tokens) = category.EnumTokens(PCWSTR::null(), PCWSTR::null()) else {
            continue;
        };

        let mut count = 0u32;
        if enum_tokens.GetCount(&mut count).is_err() {
            continue;
        };

        for i in 0..count {
            let Ok(token) = enum_tokens.Item(i) else { continue };

            let id_str = if let Ok(id) = token.GetId() {
                if !id.0.is_null() {
                    let mut len = 0;
                    while *id.0.add(len) != 0 {
                        len += 1;
                    }
                    let slice = std::slice::from_raw_parts(id.0, len);
                    let s = String::from_utf16_lossy(slice);
                    CoTaskMemFree(id.0 as _);
                    s
                } else {
                    String::new()
                }
            } else {
                String::new()
            };

            let id_lower = id_str.to_lowercase();

            let is_french = token_matches_attributes(&token, w!("Language=40c"))
                || token_matches_attributes(&token, w!("Language=c0c"))
                || id_lower.contains("fr-fr")
                || id_lower.contains("frfr")
                || id_lower.contains("fr-ca")
                || id_lower.contains("frca")
                || id_lower.contains("_40c")
                || id_lower.contains("-40c");

            if !is_french {
                continue;
            }

            // 1. Recherche par nom prioritaire (Denise / Henri, etc.)
            for &pref in preferred_names {
                if id_lower.contains(pref) {
                    if voice.SetVoice(&token).is_ok() {
                        let display = id_str.rsplit('\\').next().unwrap_or(&id_str);
                        println!("[TTS] Voix '{}' (correspondance: {pref}) sélectionnée.", display);
                        return;
                    }
                }
            }

            // Enregistrement des tokens de repli
            let matches_gender = if is_female {
                token_matches_attributes(&token, w!("Gender=Female"))
                    || id_lower.contains("female")
            } else {
                token_matches_attributes(&token, w!("Gender=Male"))
                    || id_lower.contains("male")
            };

            if matches_gender && fallback_gender_token.is_none() {
                fallback_gender_token = Some(token.clone());
            } else if fallback_french_token.is_none() {
                fallback_french_token = Some(token.clone());
            }
        }
    }

    // 2. Repli sur une voix française du genre demandé
    if let Some(token) = fallback_gender_token {
        if voice.SetVoice(&token).is_ok() {
            println!("[TTS] Voix de repli appliquée par genre ({:?}).", profile);
            return;
        }
    }

    // 3. Repli sur n'importe quelle voix française disponible
    if let Some(token) = fallback_french_token {
        if voice.SetVoice(&token).is_ok() {
            println!("[TTS] Voix de repli française appliquée.");
            return;
        }
    }

    println!("[TTS] Aucune voix française trouvée dans les catégories SAPI / OneCore.");
}

/// Calcule le niveau d'ouverture labiale et de dynamique audio à partir des visèmes/phonèmes SAPI
fn calculate_viseme_audio_level(viseme_id: i32, phoneme_id: u32, elapsed_secs: f32, current_level: f32) -> f32 {
    // Les visèmes SAPI vont de 0 (silence) à 21 (formes de bouche distinctes)
    let target = match viseme_id {
        0 => {
            if phoneme_id > 0 {
                0.15
            } else {
                0.0
            }
        }
        // Voyelles très ouvertes (ah, aa, ae)
        1 | 2 | 11 | 12 => 0.85 + 0.15 * (elapsed_secs * 28.0).sin().abs(),
        // Voyelles intermédiaires (eh, ao, ih)
        3 | 4 | 5 | 6 => 0.65 + 0.18 * (elapsed_secs * 24.0).sin().abs(),
        // Voyelles arrondies ou fermées (uw, er, y)
        7 | 8 | 9 | 10 => 0.45 + 0.15 * (elapsed_secs * 22.0).cos().abs(),
        // Consonnes labiales (m, b, p) -> bouche fermée
        15 | 21 => 0.22,
        // Consonnes dentales et fricatives (f, v, th, s, z, ch, sh)
        13 | 14 | 16 | 17 | 18 => 0.40 + 0.12 * (elapsed_secs * 30.0).sin().abs(),
        // Consonnes palatales et gutturales (k, g, n, l, r)
        19 | 20 => 0.50 + 0.15 * (elapsed_secs * 25.0).cos().abs(),
        _ => {
            let p = phoneme_id as f32;
            (0.35 + 0.35 * (p * 0.19 + elapsed_secs * 20.0).sin().abs()).clamp(0.15, 0.90)
        }
    };

    // Filtrage récursif exponentiel : attaque rapide (0.45) et relâchement doux (0.25)
    if target > current_level {
        current_level + (target - current_level) * 0.45
    } else {
        current_level + (target - current_level) * 0.25
    }
}

/// Calcule la durée de pause prosodique optimale à la fin d'une phrase
fn punctuation_pause_duration(sentence: &str) -> Duration {
    let s = sentence.trim_end();
    if s.ends_with('.') || s.ends_with('!') || s.ends_with('?') {
        Duration::from_millis(80)
    } else if s.ends_with(';') || s.ends_with(':') {
        Duration::from_millis(50)
    } else {
        Duration::from_millis(35)
    }
}

fn get_kokoro_voice(profile: VoiceGender) -> &'static str {
    match profile {
        VoiceGender::Denise => "ff_siwis",
        VoiceGender::Henri => "am_adam",
        VoiceGender::Hortense => "af_bella",
        VoiceGender::Paul => "bm_george",
        VoiceGender::Sylvie => "bf_emma",
        VoiceGender::Jean => "am_michael",
        VoiceGender::Kokoro => "ff_siwis",
    }
}

fn resample_to_device(
    samples: &[f32],
    in_rate: u32,
    out_rate: u32,
    channels: u16,
) -> Vec<f32> {
    if samples.is_empty() || in_rate == 0 || out_rate == 0 || channels == 0 {
        return Vec::new();
    }
    let channels = channels as usize;
    if in_rate == out_rate {
        if channels == 1 {
            return samples.to_vec();
        }
        let mut out = Vec::with_capacity(samples.len() * channels);
        for &s in samples {
            for _ in 0..channels {
                out.push(s);
            }
        }
        return out;
    }
    let ratio = in_rate as f64 / out_rate as f64;
    let target_len = (samples.len() as f64 / ratio).round() as usize;
    let mut out = Vec::with_capacity(target_len * channels);

    for i in 0..target_len {
        let src_idx = i as f64 * ratio;
        let idx0 = (src_idx.floor() as usize).min(samples.len() - 1);
        let idx1 = (idx0 + 1).min(samples.len() - 1);
        let frac = (src_idx - idx0 as f64) as f32;
        let s = samples[idx0] + frac * (samples[idx1] - samples[idx0]);

        for _ in 0..channels {
            out.push(s);
        }
    }
    out
}

#[derive(Debug, PartialEq, Eq)]
enum PlaybackOutcome {
    Completed,
    Interrupted,
    Failed,
}

fn play_kokoro_pcm(
    samples: &[f32],
    sample_rate: u32,
    rx: &std::sync::mpsc::Receiver<TtsCommand>,
    pending_cmd: &mut Option<TtsCommand>,
    current_gender: &mut VoiceGender,
    #[cfg(windows)] voice: &windows::Win32::Media::Speech::ISpVoice,
) -> PlaybackOutcome {
    let host = cpal::default_host();
    let Some(device) = host.default_output_device() else {
        return PlaybackOutcome::Failed;
    };
    let Ok(supported_config) = device.default_output_config() else {
        return PlaybackOutcome::Failed;
    };

    let out_rate = supported_config.sample_rate();
    let channels = supported_config.channels();
    let stream_config: cpal::StreamConfig = supported_config.clone().into();

    let interleaved_data = Arc::new(resample_to_device(samples, sample_rate, out_rate, channels));
    if interleaved_data.is_empty() {
        return PlaybackOutcome::Completed;
    }

    let cursor = Arc::new(AtomicUsize::new(0));
    let interrupted = Arc::new(AtomicBool::new(false));

    let cur = Arc::clone(&cursor);
    let intr = Arc::clone(&interrupted);
    let data = Arc::clone(&interleaved_data);

    let stream_res = match supported_config.sample_format() {
        cpal::SampleFormat::F32 => device.build_output_stream(
            stream_config.clone(),
            move |output: &mut [f32], _| {
                if intr.load(Ordering::Relaxed) {
                    output.fill(0.0);
                    return;
                }
                let pos = cur.load(Ordering::Relaxed);
                if pos >= data.len() {
                    output.fill(0.0);
                    return;
                }
                let available = &data[pos..];
                let to_copy = output.len().min(available.len());
                output[..to_copy].copy_from_slice(&available[..to_copy]);
                output[to_copy..].fill(0.0);
                cur.fetch_add(to_copy, Ordering::Relaxed);
            },
            |err| eprintln!("[TTS] Erreur flux audio cpal: {err}"),
            None,
        ),
        cpal::SampleFormat::I16 => device.build_output_stream(
            stream_config,
            move |output: &mut [i16], _| {
                if intr.load(Ordering::Relaxed) {
                    output.fill(0);
                    return;
                }
                let pos = cur.load(Ordering::Relaxed);
                if pos >= data.len() {
                    output.fill(0);
                    return;
                }
                let available = &data[pos..];
                let to_copy = output.len().min(available.len());
                for (dst, &src) in output[..to_copy].iter_mut().zip(&available[..to_copy]) {
                    *dst = (src * 32767.0).clamp(-32768.0, 32767.0) as i16;
                }
                output[to_copy..].fill(0);
                cur.fetch_add(to_copy, Ordering::Relaxed);
            },
            |err| eprintln!("[TTS] Erreur flux audio cpal: {err}"),
            None,
        ),
        _ => return PlaybackOutcome::Failed,
    };

    let Ok(stream) = stream_res else {
        return PlaybackOutcome::Failed;
    };

    if stream.play().is_err() {
        return PlaybackOutcome::Failed;
    }

    let total_samples = interleaved_data.len();
    let ch = channels as usize;
    let window_size = (out_rate as usize * ch / 50).max(64);

    let stream_start = Instant::now();
    let expected_duration = Duration::from_secs_f64(
        (total_samples as f64 / (out_rate as f64 * ch as f64)).max(0.5) + 3.0,
    );
    let mut last_progress_pos = 0;
    let mut last_progress_time = Instant::now();
    let stall_timeout = Duration::from_millis(4000);
    let mut smoothed_level = 0.0f32;

    while cursor.load(Ordering::Relaxed) < total_samples {
        std::thread::sleep(Duration::from_millis(20));

        if let Ok(next_cmd) = rx.try_recv() {
            match next_cmd {
                TtsCommand::SetGender(new_gender) => {
                    *current_gender = new_gender;
                    #[cfg(windows)]
                    unsafe {
                        find_and_apply_voice(voice, *current_gender);
                    }
                }
                TtsCommand::Stop => {
                    interrupted.store(true, Ordering::SeqCst);
                    drop(stream);
                    set_tts_audio_level(0.0);
                    return PlaybackOutcome::Interrupted;
                }
                TtsCommand::Speak(next_text) => {
                    interrupted.store(true, Ordering::SeqCst);
                    drop(stream);
                    set_tts_audio_level(0.0);
                    *pending_cmd = Some(TtsCommand::Speak(next_text));
                    return PlaybackOutcome::Interrupted;
                }
            }
        }

        let cur_pos = cursor.load(Ordering::Relaxed);
        if cur_pos > last_progress_pos {
            last_progress_pos = cur_pos;
            last_progress_time = Instant::now();
        } else if last_progress_time.elapsed() > stall_timeout || stream_start.elapsed() > expected_duration {
            eprintln!("[TTS] Watchdog audio déclenché : flux CPAL bloqué. Arrêt du flux.");
            interrupted.store(true, Ordering::SeqCst);
            drop(stream);
            set_tts_audio_level(0.0);
            return PlaybackOutcome::Failed;
        }

        let pos = cursor.load(Ordering::Relaxed);
        let start = pos.saturating_sub(window_size);
        let slice = &interleaved_data[start..pos.min(total_samples)];
        let target_level = if !slice.is_empty() {
            let sum_sq: f32 = slice.iter().map(|&s| s * s).sum();
            let rms = (sum_sq / slice.len() as f32).sqrt();
            (rms * 5.0).clamp(0.05, 1.0)
        } else {
            0.0
        };

        if target_level > smoothed_level {
            smoothed_level += (target_level - smoothed_level) * 0.45;
        } else {
            smoothed_level += (target_level - smoothed_level) * 0.25;
        }
        set_tts_audio_level(smoothed_level);
    }

    set_tts_audio_level(0.0);
    drop(stream);
    PlaybackOutcome::Completed
}

/// Recherche le fichier de voix Kokoro (.pt) sur le disque dans le dépôt, le dossier courant ou l'arborescence de l'exécutable.
fn find_existing_voice_file(voice_filename: &str) -> Option<PathBuf> {
    // 1. Racine du dépôt via CARGO_MANIFEST_DIR (infaillible lors du dev / cargo run)
    let manifest_dir = Path::new(env!("CARGO_MANIFEST_DIR"));
    let manifest_candidates = [
        manifest_dir.join("voices").join(voice_filename),
        manifest_dir.join(voice_filename),
    ];
    for p in &manifest_candidates {
        if p.is_file() {
            return Some(p.clone());
        }
    }

    // 2. Répertoire de travail courant et ses parents
    if let Ok(cwd) = std::env::current_dir() {
        let mut curr = Some(cwd.as_path());
        for _ in 0..5 {
            if let Some(dir) = curr {
                let p1 = dir.join("voices").join(voice_filename);
                if p1.is_file() {
                    return Some(p1);
                }
                let p2 = dir.join(voice_filename);
                if p2.is_file() {
                    return Some(p2);
                }
                curr = dir.parent();
            } else {
                break;
            }
        }
    }

    // 3. Répertoire de l'exécutable courant et ses parents (remonte depuis target/debug)
    if let Ok(exe) = std::env::current_exe() {
        let mut curr = exe.parent();
        for _ in 0..6 {
            if let Some(dir) = curr {
                let p1 = dir.join("voices").join(voice_filename);
                if p1.is_file() {
                    return Some(p1);
                }
                let p2 = dir.join(voice_filename);
                if p2.is_file() {
                    return Some(p2);
                }
                curr = dir.parent();
            } else {
                break;
            }
        }
    }

    None
}

/// S'assure que le fichier de voix Kokoro (.pt) est accessible et dupliqué dans les chemins du runtime.
/// Retourne le chemin absolu prêt pour any_tts (sans extension .pt).
pub fn ensure_kokoro_voice_file(voice_name: &str) -> Result<String, String> {
    let clean_voice = voice_name
        .trim_end_matches(".pt")
        .trim_start_matches("voices/")
        .trim_start_matches("voices\\");
    let voice_filename = format!("{clean_voice}.pt");

    // 1. Vérification dans le cache mémoire des chemins résolus
    if let Ok(guard) = VOICE_PATH_CACHE.lock() {
        if let Some(ref cache) = *guard {
            if let Some(cached_path) = cache.get(clean_voice) {
                return Ok(cached_path.clone());
            }
        }
    }

    // Répertoire cible 'voices/' (racine du projet en dev, ou à côté de l'exécutable)
    let manifest_voices = Path::new(env!("CARGO_MANIFEST_DIR")).join("voices");
    let primary_voices_dir = if manifest_voices.parent().map_or(false, |p| p.exists()) {
        manifest_voices
    } else if let Ok(exe) = std::env::current_exe() {
        exe.parent().map(|p| p.join("voices")).unwrap_or_else(|| PathBuf::from("voices"))
    } else {
        PathBuf::from("voices")
    };
    let _ = std::fs::create_dir_all(&primary_voices_dir);

    if let Some(found_path) = find_existing_voice_file(&voice_filename) {
        let canonical = std::fs::canonicalize(&found_path).unwrap_or(found_path.clone());

        // Assurer la présence dans le dossier voices/
        let target_in_voices = primary_voices_dir.join(&voice_filename);
        if !target_in_voices.exists() {
            let _ = std::fs::copy(&canonical, &target_in_voices);
        }

        // Nettoyage de sécurité si le fichier traînait à la racine
        let local_root = Path::new(&voice_filename);
        if local_root.is_file() && target_in_voices.is_file() {
            let _ = std::fs::remove_file(local_root);
        }

        // Duplication dans le dossier voices/ de l'exécutable si distinct
        if let Ok(exe) = std::env::current_exe() {
            if let Some(exe_dir) = exe.parent() {
                let exe_voices = exe_dir.join("voices");
                if exe_voices != primary_voices_dir {
                    let _ = std::fs::create_dir_all(&exe_voices);
                    let _ = std::fs::copy(&canonical, exe_voices.join(&voice_filename));
                }
                let exe_root_file = exe_dir.join(&voice_filename);
                if exe_root_file.is_file() && exe_voices.join(&voice_filename).is_file() {
                    let _ = std::fs::remove_file(exe_root_file);
                }
            }
        }

        let final_path = if target_in_voices.exists() {
            std::fs::canonicalize(&target_in_voices).unwrap_or(target_in_voices)
        } else {
            canonical
        };
        let path_str = final_path.to_string_lossy();
        let abs_str = path_str
            .strip_prefix(r"\\?\")
            .unwrap_or(&path_str)
            .replace('\\', "/");

        let without_ext = abs_str.strip_suffix(".pt").unwrap_or(&abs_str).to_string();
        if let Ok(mut guard) = VOICE_PATH_CACHE.lock() {
            guard
                .get_or_insert_with(HashMap::new)
                .insert(clean_voice.to_string(), without_ext.clone());
        }
        return Ok(without_ext);
    }

    let target_path = primary_voices_dir.join(&voice_filename);

    let urls = [
        format!("https://huggingface.co/hexgrad/Kokoro-82M/resolve/main/voices/{voice_filename}"),
        format!("https://huggingface.co/spaces/hexgrad/Kokoro-TTS/resolve/main/voices/{voice_filename}"),
    ];
    let rt = tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()
        .map_err(|e| format!("Runtime tokio: {e}"))?;

    let client = reqwest::Client::builder()
        .timeout(Duration::from_secs(30))
        .build()
        .map_err(|e| format!("Client HTTP: {e}"))?;

    let mut downloaded_bytes = None;
    let mut last_error = String::new();

    for url in &urls {
        println!("[Kokoro TTS] Voix '{voice_filename}' introuvable localement. Téléchargement depuis {url}...");
        let res = rt.block_on(async {
            let resp = client
                .get(url)
                .send()
                .await
                .map_err(|e| format!("Erreur réseau téléchargement voix Kokoro: {e}"))?;

            if !resp.status().is_success() {
                return Err(format!("Statut {}", resp.status()));
            }

            resp.bytes().await.map_err(|e| format!("Lecture octets voix: {e}"))
        });

        match res {
            Ok(b) => { downloaded_bytes = Some(b); break; }
            Err(e) => { last_error = e; }
        }
    }

    let bytes = downloaded_bytes.ok_or_else(|| {
        format!("Échec du téléchargement de '{voice_filename}': {last_error}")
    })?;

    std::fs::write(&target_path, &bytes).map_err(|e| format!("Écriture fichier voix: {e}"))?;
    println!("[Kokoro TTS] Voix '{voice_filename}' téléchargée avec succès dans {}", target_path.display());

    // Duplication dans exe_dir/voices si distinct
    if let Ok(exe) = std::env::current_exe() {
        if let Some(exe_dir) = exe.parent() {
            let exe_voices = exe_dir.join("voices");
            if exe_voices != primary_voices_dir {
                let _ = std::fs::create_dir_all(&exe_voices);
                let _ = std::fs::write(exe_voices.join(&voice_filename), &bytes);
            }
        }
    }

    // Nettoyage si un fichier résiduel du même nom existait à la racine
    let local_root = Path::new(&voice_filename);
    if local_root.is_file() {
        let _ = std::fs::remove_file(local_root);
    }

    let canonical = std::fs::canonicalize(&target_path).unwrap_or(target_path);
    let path_str = canonical.to_string_lossy();
    let abs_str = path_str
        .strip_prefix(r"\\?\")
        .unwrap_or(&path_str)
        .replace('\\', "/");
    let without_ext = abs_str.strip_suffix(".pt").unwrap_or(&abs_str).to_string();
    if let Ok(mut guard) = VOICE_PATH_CACHE.lock() {
        guard
            .get_or_insert_with(HashMap::new)
            .insert(clean_voice.to_string(), without_ext.clone());
    }
    Ok(without_ext)
}

/// Cache thread-safe pour les chemins absolus des fichiers de voix Kokoro résolus sur le disque.
static VOICE_PATH_CACHE: Mutex<Option<HashMap<String, String>>> = Mutex::new(None);

/// Cache global multi-entrées (clé: nom_de_voix -> (voix_cible, langue)) pour éviter d'explorer à chaque phrase
static KOKORO_CONFIG_CACHE: Mutex<Option<HashMap<String, (String, String)>>> = Mutex::new(None);

/// Synthétise une phrase avec Kokoro de manière sécurisée et rapide
fn synthesize_kokoro_safe(
    tts: &mut Box<dyn TtsModel>,
    sentence: &str,
    voice_name: &str,
) -> Option<(Vec<f32>, u32)> {
    let synth_start = Instant::now();
    let trimmed = sentence.trim();
    if trimmed.is_empty() {
        return None;
    }

    let sample_rate = tts.sample_rate();
    let clean_v = voice_name.trim()
        .trim_end_matches(".pt")
        .trim_start_matches("voices/")
        .trim_start_matches("voices\\");

    // Seuil temps réel strict : au-delà de 7s, on bascule immédiatement sur SAPI
    const MAX_SYNTH_LATENCY: Duration = Duration::from_millis(12000);

    // 1. Fast-path multi-cache : si cette voix a déjà une configuration validée, l'utiliser directement
    if let Ok(guard) = KOKORO_CONFIG_CACHE.lock() {
        if let Some(ref cache) = *guard {
            if let Some((ref cached_voice, ref cached_lang)) = cache.get(clean_v) {
                let req = SynthesisRequest::new(trimmed)
                    .with_voice(cached_voice)
                    .with_language(cached_lang);
                if let Ok(Ok(audio)) = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| tts.synthesize(&req))) {
                    if !audio.samples.is_empty() {
                        let elapsed = synth_start.elapsed();
                        println!("[Kokoro TTS] Synthèse réussie en {:.2}s (fast-path voix: '{cached_voice}', lang: '{cached_lang}').", elapsed.as_secs_f32());
                        if elapsed <= MAX_SYNTH_LATENCY {
                            return Some((audio.samples, sample_rate));
                        } else {
                            eprintln!("[Kokoro TTS] Inférence trop lente ({:.2}s > 7.0s). Déclenchement du repli SAPI instantané.", elapsed.as_secs_f32());
                            return None;
                        }
                    }
                }
            }
        }
    }

    let fallback_voice = if clean_v.starts_with("am_") || clean_v.starts_with("bm_") {
        "am_adam"
    } else {
        "af_bella"
    };

    let voices_to_try: Vec<&str> = if clean_v == fallback_voice {
        vec![clean_v]
    } else {
        vec![clean_v, fallback_voice]
    };

    for v_name in voices_to_try {
        let abs_path = ensure_kokoro_voice_file(v_name).unwrap_or_else(|_| {
            find_existing_voice_file(&format!("{v_name}.pt"))
                .map(|p| {
                    let s = p.to_string_lossy().replace('\\', "/");
                    s.strip_suffix(".pt").unwrap_or(&s).to_string()
                })
                .unwrap_or_else(|| format!("voices/{v_name}"))
        });
        let stem_path = abs_path.strip_suffix(".pt").unwrap_or(&abs_path).to_string();
        let voice_candidates = [
            stem_path.clone(),
            format!("voices/{v_name}"),
        ];

        let languages: &[&str] = if v_name.starts_with("ff_") {
            &["fr-fr", "fr", "en-us"]
        } else if v_name.starts_with("bf_") || v_name.starts_with("bm_") {
            &["en-gb", "en-us"]
        } else {
            &["fr-fr", "en-us"]
        };

        for voice_target in &voice_candidates {
            for &lang in languages {
                let req = SynthesisRequest::new(trimmed)
                    .with_voice(voice_target)
                    .with_language(lang);
                let synth_result = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
                    tts.synthesize(&req)
                }));

                match synth_result {
                    Ok(Ok(audio)) if !audio.samples.is_empty() => {
                        let elapsed = synth_start.elapsed();
                        println!("[Kokoro TTS] Synthèse réussie en {:.2}s (voix: '{voice_target}', lang: '{lang}').", elapsed.as_secs_f32());
                        if let Ok(mut guard) = KOKORO_CONFIG_CACHE.lock() {
                            guard
                                .get_or_insert_with(HashMap::new)
                                .insert(clean_v.to_string(), (voice_target.clone(), lang.to_string()));
                        }
                        if elapsed > MAX_SYNTH_LATENCY {
                            eprintln!("[Kokoro TTS] Inférence trop lente ({:.2}s > 7.0s). Repli SAPI.", elapsed.as_secs_f32());
                            return None;
                        }
                        return Some((audio.samples, sample_rate));
                    }
                    Ok(Ok(_)) => {}
                    Ok(Err(err)) => {
                        println!("[Kokoro TTS] Essai (voix: '{voice_target}', lang: '{lang}') non concluant: {err:?}");
                    }
                    Err(_) => {
                        eprintln!("[Kokoro TTS] Panique interceptée pour (voix: '{voice_target}', lang: '{lang}')");
                    }
                }
            }
        }
    }

    // 2. Repli rapide : translitération des accents si le tokenizer Kokoro est anglophone
    let deaccented = strip_accents(trimmed);
    if deaccented != trimmed {
        let fb_abs = ensure_kokoro_voice_file(fallback_voice).unwrap_or_else(|_| fallback_voice.to_string());
        let fb_stem = fb_abs.strip_suffix(".pt").unwrap_or(&fb_abs).to_string();
        let fb_candidates = [fb_stem, format!("voices/{fallback_voice}"), fallback_voice.to_string()];
        for fb_cand in &fb_candidates {
            let req = SynthesisRequest::new(&deaccented)
                .with_voice(fb_cand)
                .with_language("en-us");
            if let Ok(Ok(audio)) = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| tts.synthesize(&req))) {
                if !audio.samples.is_empty() {
                    if let Ok(mut guard) = KOKORO_CONFIG_CACHE.lock() {
                        guard
                            .get_or_insert_with(HashMap::new)
                            .insert(clean_v.to_string(), (fb_cand.to_string(), "en-us".to_string()));
                    }
                    return Some((audio.samples, sample_rate));
                }
            }
        }
    }
    None
}

/// Lance le worker de synthèse vocale réactif
pub fn spawn_tts_worker(event_tx: StdSender<AgentEvent>, initial_voice: VoiceGender) -> StdSender<TtsCommand> {
    let (tx, rx) = channel::<TtsCommand>();
    std::thread::spawn(move || {
        let mut current_gender = initial_voice;
        let kokoro_engine: Arc<Mutex<Option<Box<dyn TtsModel>>>> = Arc::new(Mutex::new(None));
        let kokoro_ready = Arc::new(AtomicBool::new(false));

        // Chargement asynchrone en tâche de fond du modèle Kokoro
        {
            let engine = Arc::clone(&kokoro_engine);
            let ready = Arc::clone(&kokoro_ready);
            std::thread::spawn(move || {
                println!("[Kokoro TTS] Chargement asynchrone du modèle Kokoro...");
                let config = TtsConfig::new(ModelType::Kokoro);

                // Pré-téléchargement de la voix française par défaut
                let default_voice_res = ensure_kokoro_voice_file("ff_siwis");
                match default_voice_res {
                    Ok(ref path) => {
                        // Pré-remplissage immédiat du Fast-Path dès le chargement
                        if let Ok(mut guard) = KOKORO_CONFIG_CACHE.lock() {
                            guard
                                .get_or_insert_with(HashMap::new)
                                .insert("ff_siwis".to_string(), (path.clone(), "fr-fr".to_string()));
                        }
                    }
                    Err(e) => {
                        eprintln!("[Kokoro TTS] Avertissement téléchargement voix par défaut 'ff_siwis': {e}");
                        let _ = ensure_kokoro_voice_file("af_bella");
                    }
                }

                let load_result = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
                    load_model(config)
                }));

                match load_result {
                    Ok(Ok(tts)) => {
                        println!("[Kokoro TTS] Modèle Kokoro prêt. Synthèse neuronale active.");
                        *engine.lock().unwrap_or_else(|p| p.into_inner()) = Some(tts);
                        ready.store(true, Ordering::SeqCst);
                    }
                    Ok(Err(err)) => {
                        eprintln!("[Kokoro TTS] Modèle Kokoro indisponible ({err:?}). Repli automatique sur SAPI.");
                    }
                    Err(_) => {
                        eprintln!("[Kokoro TTS] Panique interceptée lors du chargement de Kokoro. Repli automatique sur SAPI.");
                    }
                }
            });
        }

        #[cfg(windows)]
        unsafe {
            use windows::core::PCWSTR;
            use windows::Win32::Media::Speech::{
                ISpVoice, SpVoice, SPF_ASYNC, SPF_IS_NOT_XML, SPF_PURGEBEFORESPEAK, SPVOICESTATUS,
            };
            use windows::Win32::System::Com::{
                CoCreateInstance, CoInitializeEx, CLSCTX_ALL, COINIT_MULTITHREADED,
            };

            let _ = CoInitializeEx(None, COINIT_MULTITHREADED);
            let voice: Result<ISpVoice, _> = CoCreateInstance(&SpVoice, None, CLSCTX_ALL);
            let Ok(voice) = voice else {
                eprintln!("[TTS] Impossible d'initialiser le synthétiseur vocal Windows.");
                return;
            };

            // Initialisation de la voix selon le genre configuré (Denise / Henri)
            find_and_apply_voice(&voice, current_gender);

            let _ = voice.SetVolume(100);
            let _ = voice.SetRate(1);
            let base_flags = (SPF_ASYNC.0 | SPF_PURGEBEFORESPEAK.0 | SPF_IS_NOT_XML.0) as u32;

            let mut pending_cmd: Option<TtsCommand> = None;

            loop {
                let cmd = if let Some(c) = pending_cmd.take() {
                    set_tts_audio_level(0.0);
                    c
                } else {
                    match rx.recv() {
                        Ok(c) => c,
                        Err(_) => break,
                    }
                };

                match cmd {
                    TtsCommand::SetGender(new_gender) => {
                        current_gender = new_gender;
                        find_and_apply_voice(&voice, current_gender);
                    }
                    TtsCommand::Stop => {
                        let _ = voice.Speak(PCWSTR::null(), base_flags, None);
                        set_tts_audio_level(0.0);
                    }
                    TtsCommand::Speak(raw_text) => {
                        let clean_text = sanitize_for_tts(&raw_text);
                        let sentences = split_into_sentences(&clean_text);
                        if sentences.is_empty() {
                            let _ = event_tx.send(AgentEvent::StatusChanged(AgentStatus::Idle));
                            let _ = event_tx.send(AgentEvent::TtsFinished);
                            continue;
                        }

                        let _ = event_tx.send(AgentEvent::StatusChanged(AgentStatus::Speaking));
                        let mut was_interrupted = false;
                        let mut smoothed_level = 0.0f32;
                        let mut kokoro_failed_in_speech = false;
                        let mut prefetch: Option<(
                            VoiceGender,
                            Arc<AtomicBool>,
                            std::thread::JoinHandle<Option<(Vec<f32>, u32)>>,
                        )> = None;

                        for (idx, sentence) in sentences.iter().enumerate() {
                            let mut played_with_kokoro = false;

                            // Exécution réservée exclusivement au profil neuronal Kokoro
                            if current_gender == VoiceGender::Kokoro && !kokoro_failed_in_speech {
                                if kokoro_ready.load(Ordering::SeqCst) {
                                    let synth_res = if let Some((p_gender, cancel_flag, handle)) = prefetch.take() {
                                        if p_gender == current_gender && !cancel_flag.load(Ordering::Relaxed) {
                                            handle.join().unwrap_or(None)
                                        } else {
                                            cancel_flag.store(true, Ordering::SeqCst);
                                            let mut guard = match kokoro_engine.lock() {
                                                Ok(g) => g,
                                                Err(p) => {
                                                    eprintln!("[Kokoro TTS] Mutex empoisonné suite à une panique, récupération.");
                                                    p.into_inner()
                                                }
                                            };
                                            if let Some(ref mut tts) = *guard {
                                                let voice_name = get_kokoro_voice(current_gender);
                                                synthesize_kokoro_safe(tts, sentence, voice_name)
                                            } else {
                                                None
                                            }
                                        }
                                    } else {
                                        let mut guard = match kokoro_engine.lock() {
                                            Ok(g) => g,
                                            Err(p) => {
                                                eprintln!("[Kokoro TTS] Mutex empoisonné suite à une panique, récupération.");
                                                p.into_inner()
                                            }
                                        };
                                        if let Some(ref mut tts) = *guard {
                                            let voice_name = get_kokoro_voice(current_gender);
                                            synthesize_kokoro_safe(tts, sentence, voice_name)
                                        } else {
                                            None
                                        }
                                    };
                                if let Some((samples, sample_rate)) = synth_res {
                                    if !samples.is_empty() {
                                        // Pipelining : pré-synthèse en tâche de fond de la phrase suivante
                                        if idx + 1 < sentences.len() {
                                            let next_sentence = sentences[idx + 1].clone();
                                            let next_voice = get_kokoro_voice(current_gender);
                                            let engine_clone = Arc::clone(&kokoro_engine);
                                            let cancel_flag = Arc::new(AtomicBool::new(false));
                                            let cancel_clone = Arc::clone(&cancel_flag);

                                            let handle = std::thread::spawn(move || {
                                                if cancel_clone.load(Ordering::Relaxed) {
                                                    return None;
                                                }
                                                let mut guard = match engine_clone.lock() {
                                                    Ok(g) => g,
                                                    Err(p) => p.into_inner(),
                                                };
                                                if cancel_clone.load(Ordering::Relaxed) {
                                                    return None;
                                                }
                                                if let Some(ref mut tts) = *guard {
                                                    synthesize_kokoro_safe(tts, &next_sentence, next_voice)
                                                } else {
                                                    None
                                                }
                                            });

                                            prefetch = Some((current_gender, cancel_flag, handle));
                                        }

                                        match play_kokoro_pcm(
                                            &samples,
                                            sample_rate,
                                            &rx,
                                            &mut pending_cmd,
                                            &mut current_gender,
                                            &voice,
                                        ) {
                                            PlaybackOutcome::Completed => {
                                                played_with_kokoro = true;
                                            }
                                            PlaybackOutcome::Interrupted => {
                                                if let Some((_, cancel, _)) = prefetch.take() {
                                                    cancel.store(true, Ordering::SeqCst);
                                                }
                                                was_interrupted = true;
                                                break;
                                            }
                                            PlaybackOutcome::Failed => {
                                                if let Some((_, cancel, _)) = prefetch.take() {
                                                    cancel.store(true, Ordering::SeqCst);
                                                }
                                                eprintln!("[Kokoro TTS] Échec lecture audio cpal. Bascule sur SAPI.");
                                            }
                                        }
                                    }
                                } else {
                                    // Disjoncteur : si Kokoro échoue sur cette phrase, on évite de faire
                                    // mouliner le CPU sur les phrases suivantes et on bascule directement sur SAPI.
                                    if let Some((_, cancel, _)) = prefetch.take() {
                                        cancel.store(true, Ordering::SeqCst);
                                    }
                                    kokoro_failed_in_speech = true;
                                }
                                } else {
                                    println!("[Kokoro TTS] Modèle Kokoro en cours de chargement. Repli temporaire sur SAPI.");
                                }
                            }

                            // Repli immédiat sur SAPI (ISpVoice) si Kokoro échoue ou n'est pas encore prêt
                            if !played_with_kokoro {
                            let active_wide_buf: Vec<u16> =
                                sentence.encode_utf16().chain(std::iter::once(0)).collect();

                            if voice
                                .Speak(PCWSTR(active_wide_buf.as_ptr()), base_flags, None)
                                .is_ok()
                            {
                                let start_wait = Instant::now();
                                let mut has_started = false;
                                const STARTUP_TIMEOUT: Duration = Duration::from_millis(3000);

                                loop {
                                    std::thread::sleep(Duration::from_millis(20));

                                    if let Ok(next_cmd) = rx.try_recv() {
                                        match next_cmd {
                                            TtsCommand::SetGender(new_gender) => {
                                                current_gender = new_gender;
                                                find_and_apply_voice(&voice, current_gender);
                                            }
                                            TtsCommand::Stop => {
                                                let _ = voice.Speak(PCWSTR::null(), base_flags, None);
                                                set_tts_audio_level(0.0);
                                                was_interrupted = true;
                                                break;
                                            }
                                            TtsCommand::Speak(next_text) => {
                                                let _ = voice.Speak(PCWSTR::null(), base_flags, None);
                                                set_tts_audio_level(0.0);
                                                pending_cmd = Some(TtsCommand::Speak(next_text));
                                                was_interrupted = true;
                                                break;
                                            }
                                        }
                                    }

                                    let mut status = SPVOICESTATUS::default();
                                    if voice.GetStatus(&mut status, std::ptr::null_mut()).is_ok()
                                    {
                                        // dwRunningState == 2 correspond à SPRS_IS_SPEAKING
                                        if status.dwRunningState == 2 {
                                            has_started = true;
                                            let elapsed = start_wait.elapsed().as_secs_f32();
                                            smoothed_level = calculate_viseme_audio_level(status.VisemeId.0, status.PhonemeId as u32, elapsed, smoothed_level);
                                            set_tts_audio_level(smoothed_level);
                                        } else if has_started {
                                            set_tts_audio_level(0.0);
                                            break;
                                        } else if start_wait.elapsed() > STARTUP_TIMEOUT {
                                            // La phrase est complètement restituée
                                            set_tts_audio_level(0.0);
                                            smoothed_level = 0.0;
                                            break;
                                        } else {
                                            set_tts_audio_level(0.0);
                                        }
                                    } else {
                                        set_tts_audio_level(0.0);
                                        break;
                                    }
                                }
                            } else {
                                break;
                            }
                            }

                            if was_interrupted {
                                if let Some((_, cancel, _)) = prefetch.take() {
                                    cancel.store(true, Ordering::SeqCst);
                                }
                                break;
                            }

                            // Micro-pause prosodique inter-phrases avec surveillance de préemption
                            let pause_dur = punctuation_pause_duration(&sentence);
                            let pause_start = Instant::now();
                            while pause_start.elapsed() < pause_dur {
                                std::thread::sleep(Duration::from_millis(10));
                                if let Ok(next_cmd) = rx.try_recv() {
                                    match next_cmd {
                                        TtsCommand::SetGender(new_gender) => {
                                            current_gender = new_gender;
                                            find_and_apply_voice(&voice, current_gender);
                                        }
                                        TtsCommand::Stop => {
                                            set_tts_audio_level(0.0);
                                            was_interrupted = true;
                                            break;
                                        }
                                        TtsCommand::Speak(next_text) => {
                                            set_tts_audio_level(0.0);
                                            pending_cmd = Some(TtsCommand::Speak(next_text));
                                            was_interrupted = true;
                                            break;
                                        }
                                    }
                                }
                            }
                            if was_interrupted {
                                if let Some((_, cancel, _)) = prefetch.take() {
                                    cancel.store(true, Ordering::SeqCst);
                                }
                                break;
                            }
                        }
                        if let Some((_, cancel, _)) = prefetch.take() {
                            cancel.store(true, Ordering::SeqCst);
                        }

                        if !was_interrupted {
                            set_tts_audio_level(0.0);
                            let _ = event_tx.send(AgentEvent::StatusChanged(AgentStatus::Idle));
                            let _ = event_tx.send(AgentEvent::TtsFinished);
                        }
                    }
                }
            }
        }

        #[cfg(not(windows))]
        {
            let mut current_process: Option<std::process::Child> = None;
            let mut pending_cmd: Option<TtsCommand> = None;

            loop {
                let cmd = if let Some(c) = pending_cmd.take() {
                    c
                } else {
                    match rx.recv() {
                        Ok(c) => c,
                        Err(_) => break,
                    }
                };

                match cmd {
                    TtsCommand::SetGender(new_gender) => {
                        current_gender = new_gender;
                    }
                    TtsCommand::Stop => {
                        if let Some(mut child) = current_process.take() {
                            let _ = child.kill();
                        }
                        set_tts_audio_level(0.0);
                    }
                    TtsCommand::Speak(text) => {
                        if let Some(mut child) = current_process.take() {
                            let _ = child.kill();
                        }
                        let clean_text = sanitize_for_tts(&text);
                        let sentences = split_into_sentences(&clean_text);
                        if sentences.is_empty() {
                            let _ = event_tx.send(AgentEvent::TtsFinished);
                            continue;
                        }
                        let _ = event_tx.send(AgentEvent::StatusChanged(AgentStatus::Speaking));
                        let (spd_voice, espeak_voice) = match current_gender {
                            VoiceGender::Denise => ("female1", "fr+f2"),
                            VoiceGender::Henri => ("male1", "fr+m2"),
                            VoiceGender::Hortense => ("female2", "fr+f4"),
                            VoiceGender::Paul => ("male2", "fr+m1"),
                            VoiceGender::Sylvie => ("female3", "fr-ca+f2"),
                            VoiceGender::Jean => ("male3", "fr+m3"),
                            VoiceGender::Kokoro => ("female1", "fr+f2"),
                        };

                        let mut was_interrupted = false;
                        let mut kokoro_failed_in_speech = false;
                        let mut prefetch: Option<(
                            VoiceGender,
                            Arc<AtomicBool>,
                            std::thread::JoinHandle<Option<(Vec<f32>, u32)>>,
                        )> = None;

                        for (idx, sentence) in sentences.iter().enumerate() {
                            let mut played_with_kokoro = false;

                            if current_gender == VoiceGender::Kokoro && !kokoro_failed_in_speech {
                                if kokoro_ready.load(Ordering::SeqCst) {
                                    let synth_res = if let Some((p_gender, cancel_flag, handle)) = prefetch.take() {
                                        if p_gender == current_gender && !cancel_flag.load(Ordering::Relaxed) {
                                            handle.join().unwrap_or(None)
                                        } else {
                                            cancel_flag.store(true, Ordering::SeqCst);
                                            let mut guard = match kokoro_engine.lock() {
                                                Ok(g) => g,
                                                Err(p) => p.into_inner(),
                                            };
                                            if let Some(ref mut tts) = *guard {
                                                let voice_name = get_kokoro_voice(current_gender);
                                                synthesize_kokoro_safe(tts, sentence, voice_name)
                                            } else {
                                                None
                                            }
                                        }
                                    } else {
                                        let mut guard = match kokoro_engine.lock() {
                                            Ok(g) => g,
                                            Err(p) => p.into_inner(),
                                        };
                                        if let Some(ref mut tts) = *guard {
                                            let voice_name = get_kokoro_voice(current_gender);
                                            synthesize_kokoro_safe(tts, sentence, voice_name)
                                        } else {
                                            None
                                        }
                                    };
                                if let Some((samples, sample_rate)) = synth_res {
                                    if !samples.is_empty() {
                                        if idx + 1 < sentences.len() {
                                            let next_sentence = sentences[idx + 1].clone();
                                            let next_voice = get_kokoro_voice(current_gender);
                                            let engine_clone = Arc::clone(&kokoro_engine);
                                            let cancel_flag = Arc::new(AtomicBool::new(false));
                                            let cancel_clone = Arc::clone(&cancel_flag);

                                            let handle = std::thread::spawn(move || {
                                                if cancel_clone.load(Ordering::Relaxed) {
                                                    return None;
                                                }
                                                let mut guard = match engine_clone.lock() {
                                                    Ok(g) => g,
                                                    Err(p) => p.into_inner(),
                                                };
                                                if cancel_clone.load(Ordering::Relaxed) {
                                                    return None;
                                                }
                                                if let Some(ref mut tts) = *guard {
                                                    synthesize_kokoro_safe(tts, &next_sentence, next_voice)
                                                } else {
                                                    None
                                                }
                                            });

                                            prefetch = Some((current_gender, cancel_flag, handle));
                                        }

                                        match play_kokoro_pcm(
                                            &samples,
                                            sample_rate,
                                            &rx,
                                            &mut pending_cmd,
                                            &mut current_gender,
                                        ) {
                                            PlaybackOutcome::Completed => {
                                                played_with_kokoro = true;
                                            }
                                            PlaybackOutcome::Interrupted => {
                                                if let Some((_, cancel, _)) = prefetch.take() {
                                                    cancel.store(true, Ordering::SeqCst);
                                                }
                                                was_interrupted = true;
                                                break;
                                            }
                                            PlaybackOutcome::Failed => {
                                                if let Some((_, cancel, _)) = prefetch.take() {
                                                    cancel.store(true, Ordering::SeqCst);
                                                }
                                            }
                                        }
                                    }
                                } else {
                                    if let Some((_, cancel, _)) = prefetch.take() {
                                        cancel.store(true, Ordering::SeqCst);
                                    }
                                    kokoro_failed_in_speech = true;
                                }
                                } else {
                                    println!("[Kokoro TTS] Modèle Kokoro en cours de chargement. Repli temporaire sur le moteur système.");
                                }
                            }

                            if !played_with_kokoro {
                            let child = std::process::Command::new("spd-say")
                                .args(["-l", "fr", "-t", spd_voice, "-w", &sentence])
                                .spawn()
                                .or_else(|_| std::process::Command::new("espeak-ng").args(["-v", espeak_voice, &sentence]).spawn())
                                .or_else(|_| std::process::Command::new("espeak").args(["-v", espeak_voice, &sentence]).spawn())
                                .ok();

                            current_process = child;
                            if let Some(ref mut proc) = current_process {
                                while let Ok(None) = proc.try_wait() {
                                    // Animation dynamique de l'avatar pendant la lecture Linux
                                    let t = std::time::SystemTime::now()
                                        .duration_since(std::time::UNIX_EPOCH)
                                        .unwrap_or_default()
                                        .as_millis() as f32 * 0.015;
                                    let sim_level = (0.35 + 0.35 * (t * 1.8).sin().abs()).clamp(0.15, 0.85);
                                    set_tts_audio_level(sim_level);
                                    std::thread::sleep(Duration::from_millis(25));
                                    if let Ok(next_cmd) = rx.try_recv() {
                                        match next_cmd {
                                            TtsCommand::SetGender(new_gender) => {
                                                current_gender = new_gender;
                                            }
                                            TtsCommand::Stop => {
                                                let _ = proc.kill();
                                                set_tts_audio_level(0.0);
                                                was_interrupted = true;
                                                break;
                                            }
                                            TtsCommand::Speak(next_text) => {
                                                let _ = proc.kill();
                                                set_tts_audio_level(0.0);
                                                pending_cmd = Some(TtsCommand::Speak(next_text));
                                                was_interrupted = true;
                                                break;
                                            }
                                        }
                                    }
                                }
                            }
                            }
                            if was_interrupted {
                                if let Some((_, cancel, _)) = prefetch.take() {
                                    cancel.store(true, Ordering::SeqCst);
                                }
                                break;
                            }
                            set_tts_audio_level(0.0);
                        }
                        if let Some((_, cancel, _)) = prefetch.take() {
                            cancel.store(true, Ordering::SeqCst);
                        }

                        current_process = None;
                        if !was_interrupted {
                            let _ = event_tx.send(AgentEvent::StatusChanged(AgentStatus::Idle));
                            let _ = event_tx.send(AgentEvent::TtsFinished);
                        }
                    }
                }
            }
        }
    });

    tx
}