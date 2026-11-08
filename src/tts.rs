use std::collections::VecDeque;
use std::path::{Path, PathBuf};
use std::sync::mpsc::{channel, Sender as StdSender};
use std::time::{Duration, Instant};

use crate::audio_player::AudioPlayer;
use crate::fs_util::read_env_var_from_file;
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
        // Traitement de la macro [link="url"]label[/link] -> on conserve le label et on saute les balises
        if c == '[' && i + 2 < chars.len() && chars[i + 1] == '/' {
            let rem: String = chars[i..std::cmp::min(i + 10, chars.len())].iter().collect();
            if rem.to_ascii_lowercase().starts_with("[/link") {
                while i < chars.len() && chars[i] != ']' {
                    i += 1;
                }
                if i < chars.len() && chars[i] == ']' {
                    i += 1;
                }
                result.push(' ');
                continue;
            }
        }
        if c == '[' && i + 5 < chars.len() {
            let prefix: String = chars[i..i + 5].iter().collect();
            if prefix.eq_ignore_ascii_case("[link") && (chars[i + 5] == '=' || chars[i + 5] == ' ' || chars[i + 5] == ']') {
                while i < chars.len() && chars[i] != ']' {
                    i += 1;
                }
                if i < chars.len() && chars[i] == ']' {
                    i += 1;
                }
                result.push(' ');
                continue;
            }
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
        VoiceGender::Clone => (&["denise", "henri"], true),
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

/// Décodeur WAV natif léger (PCM 16 bits ou Float 32 bits)
pub fn decode_wav_to_samples(bytes: &[u8]) -> Option<(Vec<f32>, u32)> {
    if bytes.len() < 44 || &bytes[0..4] != b"RIFF" || &bytes[8..12] != b"WAVE" {
        return None;
    }
    let mut cursor = 12;
    let mut audio_format: u16 = 1;
    let mut channels: u16 = 1;
    let mut sample_rate: u32 = 24000;
    let mut bits_per_sample: u16 = 16;
    let mut data_offset = 0;
    let mut data_len = 0;

    while cursor + 8 <= bytes.len() {
        let chunk_id = &bytes[cursor..cursor + 4];
        let chunk_size = u32::from_le_bytes(bytes[cursor + 4..cursor + 8].try_into().ok()?) as usize;
        cursor += 8;

        if chunk_id == b"fmt " && chunk_size >= 16 && cursor + chunk_size <= bytes.len() {
            audio_format = u16::from_le_bytes(bytes[cursor..cursor + 2].try_into().ok()?);
            channels = u16::from_le_bytes(bytes[cursor + 2..cursor + 4].try_into().ok()?);
            sample_rate = u32::from_le_bytes(bytes[cursor + 4..cursor + 8].try_into().ok()?);
            bits_per_sample = u16::from_le_bytes(bytes[cursor + 14..cursor + 16].try_into().ok()?);
        } else if chunk_id == b"data" {
            data_offset = cursor;
            data_len = chunk_size.min(bytes.len().saturating_sub(cursor));
            break;
        }
        cursor += chunk_size;
    }

    if data_offset == 0 || data_len == 0 {
        return None;
    }

    let payload = &bytes[data_offset..data_offset + data_len];
    let ch = channels.max(1) as usize;
    let mut samples = Vec::new();

    if audio_format == 1 && bits_per_sample == 16 {
        for frame in payload.chunks_exact(2 * ch) {
            let mut sum = 0.0f32;
            for c in 0..ch {
                let sample_i16 = i16::from_le_bytes([frame[c * 2], frame[c * 2 + 1]]);
                sum += sample_i16 as f32 / 32768.0;
            }
            samples.push(sum / ch as f32);
        }
    } else if audio_format == 3 && bits_per_sample == 32 {
        for frame in payload.chunks_exact(4 * ch) {
            let mut sum = 0.0f32;
            for c in 0..ch {
                let sample_f32 = f32::from_le_bytes([frame[c * 4], frame[c * 4 + 1], frame[c * 4 + 2], frame[c * 4 + 3]]);
                sum += sample_f32;
            }
            samples.push(sum / ch as f32);
        }
    } else {
        return None;
    }

    Some((samples, sample_rate))
}

/// Encode une séquence d'échantillons audio normalisés [-1.0, 1.0] en flux binaire WAV PCM 16-bit mono
pub fn encode_samples_to_wav(samples: &[f32], sample_rate: u32) -> Vec<u8> {
    let num_channels: u16 = 1;
    let bits_per_sample: u16 = 16;
    let byte_rate = sample_rate * (num_channels as u32) * (bits_per_sample as u32 / 8);
    let block_align = num_channels * (bits_per_sample / 8);
    let data_size = (samples.len() * 2) as u32;
    let chunk_size = 36 + data_size;

    let mut buf = Vec::with_capacity(44 + data_size as usize);
    buf.extend_from_slice(b"RIFF");
    buf.extend_from_slice(&chunk_size.to_le_bytes());
    buf.extend_from_slice(b"WAVE");

    buf.extend_from_slice(b"fmt ");
    buf.extend_from_slice(&16u32.to_le_bytes());
    buf.extend_from_slice(&1u16.to_le_bytes());
    buf.extend_from_slice(&num_channels.to_le_bytes());
    buf.extend_from_slice(&sample_rate.to_le_bytes());
    buf.extend_from_slice(&byte_rate.to_le_bytes());
    buf.extend_from_slice(&block_align.to_le_bytes());
    buf.extend_from_slice(&bits_per_sample.to_le_bytes());

    buf.extend_from_slice(b"data");
    buf.extend_from_slice(&data_size.to_le_bytes());

    for &sample in samples {
        let clamped = sample.clamp(-1.0, 1.0);
        let s_i16 = (clamped * 32767.0).round() as i16;
        buf.extend_from_slice(&s_i16.to_le_bytes());
    }
    buf
}

/// Sauvegarde les échantillons audio dans un fichier WAV PCM 16-bit mono
pub fn save_wav_file(path: &std::path::Path, samples: &[f32], sample_rate: u32) -> std::io::Result<()> {
    if let Some(parent) = path.parent() {
        let _ = std::fs::create_dir_all(parent);
    }
    let bytes = encode_samples_to_wav(samples, sample_rate);
    std::fs::write(path, bytes)
}

/// Ajoute des échantillons audio à la fin d'un fichier WAV existant (avec 350 ms de silence de séparation).
/// Crée le fichier s'il n'existe pas encore.
pub fn append_wav_file(path: &std::path::Path, new_samples: &[f32], sample_rate: u32) -> std::io::Result<(usize, f32)> {
    let mut all_samples = Vec::new();
    if path.is_file() {
        if let Ok(bytes) = std::fs::read(path) {
            if let Some((existing_samples, existing_rate)) = decode_wav_to_samples(&bytes) {
                if existing_rate == sample_rate {
                    all_samples = existing_samples;
                    let silence_len = (sample_rate as f32 * 0.35) as usize;
                    all_samples.extend(std::iter::repeat(0.0f32).take(silence_len));
                }
            }
        }
    }
    all_samples.extend_from_slice(new_samples);
    let total_count = all_samples.len();
    let total_dur = total_count as f32 / sample_rate as f32;
    save_wav_file(path, &all_samples, sample_rate)?;
    Ok((total_count, total_dur))
}

/// Retourne la durée en secondes d'un fichier WAV s'il existe
pub fn get_wav_duration_secs(path: &std::path::Path) -> Option<f32> {
    if !path.is_file() {
        return None;
    }
    let bytes = std::fs::read(path).ok()?;
    let (samples, rate) = decode_wav_to_samples(&bytes)?;
    if rate > 0 { Some(samples.len() as f32 / rate as f32) } else { None }
}

/// Vérifie qu'un dossier espeak-ng-data contient des tables phonétiques complètes et non corrompues
fn is_valid_espeak_dir(p: &Path) -> bool {
    if !p.is_dir() {
        return false;
    }
    let phontab = p.join("phontab");
    let phonindex = p.join("phonindex");
    let phondata = p.join("phondata");
    let intonations = p.join("intonations");
    let fr_dict = p.join("fr_dict");
    let voices = p.join("voices");

    let basic_tables = phontab.is_file() && phontab.metadata().map_or(false, |m| m.len() > 10_000)
        && phonindex.is_file() && phonindex.metadata().map_or(false, |m| m.len() > 200)
        && phondata.is_file() && phondata.metadata().map_or(false, |m| m.len() > 100_000)
        && intonations.is_file() && intonations.metadata().map_or(false, |m| m.len() > 500);

    if !basic_tables {
        return false;
    }

    let has_voices = voices.is_dir() && std::fs::read_dir(&voices).map_or(false, |mut it| it.next().is_some());
    let has_dict = fr_dict.is_file() || p.join("en_dict").is_file() || std::fs::read_dir(p).map_or(false, |entries| {
        entries.flatten().any(|e| e.file_name().to_string_lossy().ends_with("_dict"))
    });

    has_voices && has_dict
}

/// Recherche récursivement le dossier valide 'espeak-ng-data' (avec tables phonétiques valides)
pub fn find_espeak_ng_data(base_dir: &Path) -> Option<PathBuf> {
    // 1. Vérification directe
    if is_valid_espeak_dir(base_dir) {
        return Some(base_dir.to_path_buf());
    }
    let direct = base_dir.join("espeak-ng-data");
    if is_valid_espeak_dir(&direct) {
        return Some(direct);
    }

    // 2. Recherche récursive dans les sous-répertoires (jusqu'à 3 niveaux)
    fn scan_recursive(dir: &Path, depth: usize) -> Option<PathBuf> {
        if depth > 3 {
            return None;
        }
        if let Ok(entries) = std::fs::read_dir(dir) {
            for entry in entries.flatten() {
                let p = entry.path();
                if p.is_dir() {
                    let fname = p.file_name().and_then(|s| s.to_str()).unwrap_or("");
                    if fname == "espeak-ng-data" && is_valid_espeak_dir(&p) {
                        return Some(p);
                    }
                    if is_valid_espeak_dir(&p) {
                        return Some(p);
                    }
                    if let Some(found) = scan_recursive(&p, depth + 1) {
                        return Some(found);
                    }
                }
            }
        }
        None
    }
    scan_recursive(base_dir, 0)
}

/// Recherche récursivement les fichiers du modèle de synthèse vocale (ONNX, tokens, espeak data)
pub fn find_voice_clone_files(base_dir: &Path) -> (Option<PathBuf>, Option<PathBuf>, Option<PathBuf>) {
    let mut model = None;
    let mut tokens = None;
    let scan_dir = |dir: &Path, m: &mut Option<PathBuf>, t: &mut Option<PathBuf>| {
        if let Ok(entries) = std::fs::read_dir(dir) {
            for entry in entries.flatten() {
                let p = entry.path();
                if p.is_file() {
                    let ext = p.extension().and_then(|s| s.to_str()).unwrap_or("");
                    let fname = p.file_name().and_then(|s| s.to_str()).unwrap_or("").to_lowercase();
                    if ext.eq_ignore_ascii_case("onnx") && m.is_none() {
                        *m = Some(p.clone());
                    } else if (fname == "tokens.txt" || (fname.ends_with(".txt") && fname.contains("token"))) && t.is_none() {
                        *t = Some(p.clone());
                    }
                }
            }
        }
    };

    scan_dir(base_dir, &mut model, &mut tokens);

    // Inspection des sous-répertoires si un fichier modèle manque
    if model.is_none() || tokens.is_none() {
        if let Ok(entries) = std::fs::read_dir(base_dir) {
            for entry in entries.flatten() {
                let p = entry.path();
                if p.is_dir() {
                    scan_dir(&p, &mut model, &mut tokens);
                    if model.is_some() && tokens.is_some() {
                        break;
                    }
                }
            }
        }
    }

    let data = find_espeak_ng_data(base_dir);
    (model, tokens, data)
}

/// Télécharge et extrait automatiquement le modèle de voix clonée local s'il n'est pas déjà présent
pub fn ensure_voice_clone_model(model_dir: &Path) -> Result<(), String> {
    let (model, tokens, data) = find_voice_clone_files(model_dir);
    if model.is_some() && tokens.is_some() && data.is_some() {
        return Ok(());
    }

    let _ = std::fs::create_dir_all(model_dir);
    println!(
        "[TTS Clone] Modèle local manquant dans '{}' : initialisation du téléchargement automatique...",
        model_dir.display()
    );

    let rt = tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()
        .map_err(|e| format!("Runtime Tokio: {e}"))?;

    let client = reqwest::Client::builder()
        .timeout(Duration::from_secs(180))
        .build()
        .map_err(|e| format!("Client HTTP: {e}"))?;

    let custom_url = std::env::var("VOICE_CLONE_DOWNLOAD_URL")
        .ok()
        .or_else(|| read_env_var_from_file(".env", "VOICE_CLONE_DOWNLOAD_URL"));

    let candidate_urls: Vec<String> = if let Some(u) = custom_url {
        vec![u]
    } else {
        vec![
            "https://github.com/k2-fsa/sherpa-onnx/releases/download/tts-models/vits-piper-fr_FR-siwis-medium.tar.bz2".to_string(),
            "https://huggingface.co/csukuangfj/vits-piper-fr_FR-siwis-medium/resolve/main/vits-piper-fr_FR-siwis-medium.tar.bz2".to_string(),
        ]
    };

    let download_target = model_dir.join("model_archive.tar.bz2");
    let mut downloaded = false;

    for url in &candidate_urls {
        println!("[TTS Clone] Téléchargement de l'archive modèle depuis : {url} ...");
        let res = rt.block_on(async {
            let resp = client.get(url).send().await.map_err(|e| e.to_string())?;
            if !resp.status().is_success() {
                return Err(format!("Statut HTTP {}", resp.status()));
            }
            let bytes = resp.bytes().await.map_err(|e| e.to_string())?;
            Ok::<_, String>(bytes)
        });

        match res {
            Ok(bytes) => {
                if bytes.is_empty() {
                    eprintln!("[TTS Clone] Réponse vide reçue ({url})");
                    continue;
                }
                if let Err(e) = std::fs::write(&download_target, &bytes) {
                    eprintln!("[TTS Clone] Erreur d'écriture de l'archive : {e}");
                    continue;
                }
                println!(
                    "[TTS Clone] Archive reçue ({:.1} Mo). Extraction en cours...",
                    bytes.len() as f32 / (1024.0 * 1024.0)
                );

                let ok = std::process::Command::new("tar")
                    .args([
                        "-xf",
                        download_target.to_str().unwrap_or_default(),
                        "-C",
                        model_dir.to_str().unwrap_or_default(),
                    ])
                    .status()
                    .map_or(false, |s| s.success());

                let _ = std::fs::remove_file(&download_target);

                if ok {
                    println!("[TTS Clone] Modèle extrait avec succès.");
                    downloaded = true;
                    break;
                } else {
                    eprintln!("[TTS Clone] Commande 'tar' non disponible ou extraction échouée. Repli sur téléchargement unitaire.");
                }
            }
            Err(e) => {
                eprintln!("[TTS Clone] Échec du téléchargement ({url}) : {e}");
            }
        }
    }

    // Repli : téléchargement direct des fichiers individuels ONNX et tokens si tar a échoué
    let (model, tokens, _) = find_voice_clone_files(model_dir);
    if model.is_some() && tokens.is_some() {
        return Ok(());
    }

    if !downloaded {
        println!("[TTS Clone] Téléchargement direct des fichiers séparés (model.onnx + tokens.txt)...");
        let onnx_url = "https://huggingface.co/csukuangfj/vits-piper-fr_FR-siwis-medium/resolve/main/fr_FR-siwis-medium.onnx";
        let tokens_url = "https://huggingface.co/csukuangfj/vits-piper-fr_FR-siwis-medium/resolve/main/tokens.txt";

        let dest_onnx = model_dir.join("model.onnx");
        let dest_tokens = model_dir.join("tokens.txt");

        let dl = |u: &str, d: &Path| -> Result<(), String> {
            rt.block_on(async {
                let resp = client.get(u).send().await.map_err(|e| e.to_string())?;
                if !resp.status().is_success() {
                    return Err(format!("Statut HTTP {}", resp.status()));
                }
                let b = resp.bytes().await.map_err(|e| e.to_string())?;
                std::fs::write(d, b).map_err(|e| e.to_string())?;
                Ok(())
            })
        };

        dl(onnx_url, &dest_onnx)?;
        dl(tokens_url, &dest_tokens)?;
        println!("[TTS Clone] Fichiers model.onnx et tokens.txt téléchargés avec succès.");
    }

    // Nettoie un éventuel dossier espeak-ng-data incomplet (sans dossier voices)
    let direct_espeak = model_dir.join("espeak-ng-data");
    if direct_espeak.exists() && !is_valid_espeak_dir(&direct_espeak) {
        eprintln!("[TTS Clone] Suppression de l'ancien dossier espeak-ng-data incomplet...");
        let _ = std::fs::remove_dir_all(&direct_espeak);
    }

    // Téléchargement et extraction de l'archive officielle complète espeak-ng-data.zip
    if find_espeak_ng_data(model_dir).is_none() {
        println!("[TTS Clone] Données espeak-ng-data complètes manquantes : téléchargement de espeak-ng-data.zip...");
        let zip_path = model_dir.join("espeak-ng-data.zip");
        let zip_urls = [
            "https://github.com/k2-fsa/sherpa-onnx/releases/download/tts-models/espeak-ng-data.zip",
            "https://huggingface.co/csukuangfj/espeak-ng-data/resolve/main/espeak-ng-data.zip",
        ];

        let mut zip_ok = false;
        for z_url in &zip_urls {
            let dl = rt.block_on(async {
                let resp = client.get(*z_url).send().await.map_err(|e| e.to_string())?;
                if !resp.status().is_success() {
                    return Err(format!("Statut HTTP {}", resp.status()));
                }
                let bytes = resp.bytes().await.map_err(|e| e.to_string())?;
                if bytes.len() < 1_000_000 {
                    return Err("Fichier zip trop petit".to_string());
                }
                std::fs::write(&zip_path, bytes).map_err(|e| e.to_string())?;
                Ok::<(), String>(())
            });
            if dl.is_ok() {
                zip_ok = true;
                break;
            }
        }

        if zip_ok && zip_path.is_file() {
            println!("[TTS Clone] Extraction complète de espeak-ng-data.zip...");
            #[cfg(windows)]
            let unzipped = {
                let ps_cmd = format!(
                    "Expand-Archive -LiteralPath '{}' -DestinationPath '{}' -Force",
                    zip_path.display(),
                    model_dir.display()
                );
                std::process::Command::new("powershell")
                    .args(["-NoProfile", "-NonInteractive", "-Command", &ps_cmd])
                    .status()
                    .map_or(false, |s| s.success())
                    || std::process::Command::new("tar")
                        .args(["-xf", zip_path.to_str().unwrap_or_default(), "-C", model_dir.to_str().unwrap_or_default()])
                        .status()
                        .map_or(false, |s| s.success())
            };
            #[cfg(not(windows))]
            let unzipped = std::process::Command::new("unzip")
                .args(["-q", "-o", zip_path.to_str().unwrap_or_default(), "-d", model_dir.to_str().unwrap_or_default()])
                .status()
                .map_or(false, |s| s.success());

            let _ = std::fs::remove_file(&zip_path);
            if unzipped {
                println!("[TTS Clone] Tables espeak-ng-data et voix extraites avec succès.");
            } else {
                eprintln!("[TTS Clone] Échec extraction espeak-ng-data.zip.");
            }
        }
    }

    Ok(())
}

/// Backend de clonage vocal 100% Rust / Natif (sans Python)
pub struct NativeVoiceCloneBackend {
    player: AudioPlayer,
    reference_wav_path: Option<String>,
    model_dir: PathBuf,
    #[cfg(feature = "sherpa")]
    tts_engine: Option<sherpa_rs::tts::VitsTts>,
}

fn to_clean_path_for_c(path: &Path) -> String {
    let p = if path.is_absolute() {
        path.to_path_buf()
    } else if let Ok(cwd) = std::env::current_dir() {
        cwd.join(path)
    } else {
        path.to_path_buf()
    };
    let s = p.to_string_lossy().replace(r"\\?\", "").to_string();
    #[cfg(windows)]
    {
        s.replace('/', "\\")
    }
    #[cfg(not(windows))]
    {
        s
    }
}

impl NativeVoiceCloneBackend {
    #[cfg(feature = "sherpa")]
    fn create_engine(model_dir: &Path) -> Option<sherpa_rs::tts::VitsTts> {
        use sherpa_rs::tts::{VitsTts, VitsTtsConfig};

        println!("[TTS Clone] Inspection des fichiers du modèle Sherpa dans {:?}...", model_dir);
        let (resolved_model, resolved_tokens, resolved_data) = find_voice_clone_files(model_dir);
        if let (Some(model_path), Some(tokens_path)) = (resolved_model, resolved_tokens) {
            let model_str = to_clean_path_for_c(&model_path);
            let tokens_str = to_clean_path_for_c(&tokens_path);

            let lexicon_cand = model_path.parent().map(|p| p.join("lexicon.txt")).filter(|p| p.is_file());
            let lexicon_str = lexicon_cand.as_ref().map(|p| to_clean_path_for_c(p)).unwrap_or_default();

            let model_name = model_path.to_string_lossy().to_lowercase();
            let is_piper = model_name.contains("piper") || model_name.contains("siwis") || model_name.ends_with(".onnx");

            // Pour un modèle Piper, le dossier espeak-ng-data valide est strictement requis
            if is_piper {
                match resolved_data.as_ref() {
                    Some(d) if is_valid_espeak_dir(d) => {},
                    _ => {
                        eprintln!("[TTS Clone] ATTENTION : Dossier 'espeak-ng-data' manquant ou incomplet dans {:?}.", model_dir);
                        eprintln!("[TTS Clone] Initialisation Sherpa annulée pour empêcher l'assertion CRT _osfile(fh) & FOPEN.");
                        return None;
                    }
                }
            }

            if let Some(ref data_p) = resolved_data {
                let clean_data = to_clean_path_for_c(data_p);
                std::env::set_var("ESPEAK_DATA_PATH", &clean_data);
            }

            let data_dir_str = resolved_data
                .map(|d| to_clean_path_for_c(&d))
                .unwrap_or_default();

            println!("[TTS Clone] Initialisation de Sherpa VitsTts avec model={}, data_dir={}...", model_str, data_dir_str);
            let config = VitsTtsConfig {
                model: model_str,
                lexicon: lexicon_str,
                tokens: tokens_str,
                data_dir: data_dir_str,
                length_scale: 1.0,
                ..Default::default()
            };

            let engine_res = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
                VitsTts::new(config)
            }));
            match engine_res {
                Ok(engine) => {
                    println!("[TTS Clone] Moteur vocal natif Sherpa initialisé avec succès !");
                    Some(engine)
                }
                Err(err) => {
                    eprintln!("[TTS Clone] Échec lors de l'initialisation de VitsTts : {:?}", err);
                    None
                }
            }
        } else {
            eprintln!("[TTS Clone] Fichiers modèles (model.onnx ou tokens.txt) introuvables.");
            None
        }
    }

    pub fn new() -> Option<Self> {
        let player = AudioPlayer::new().ok()?;
        let model_dir = PathBuf::from("models").join("voice-clone");

        // Téléchargement automatique si aucun modèle ONNX n'est présent
        let _ = ensure_voice_clone_model(&model_dir);

        let reference_wav_path = std::env::var("VOICE_REFERENCE_WAV")
            .ok()
            .or_else(|| read_env_var_from_file(".env", "VOICE_REFERENCE_WAV"))
            .map(|s| s.trim().to_string())
            .filter(|s| !s.is_empty())
            .or_else(|| {
                for cand in &["models/voice_reference.wav", "speaker.wav", "models/speaker.wav", "voice.wav"] {
                    if std::path::Path::new(cand).exists() {
                        return Some(cand.to_string());
                    }
                }
                None
            });

        #[cfg(feature = "sherpa")]
        let tts_engine = Self::create_engine(&model_dir);
        #[cfg(not(feature = "sherpa"))]
        let tts_engine = None;

        Some(Self {
            player,
            reference_wav_path,
            model_dir,
            #[cfg(feature = "sherpa")]
            tts_engine,
        })
    }

    pub fn synthesize(&mut self, text: &str) -> Result<(Vec<f32>, u32), String> {
        #[cfg(feature = "sherpa")]
        {
            if self.tts_engine.is_none() {
                let _ = ensure_voice_clone_model(&self.model_dir);
                self.tts_engine = Self::create_engine(&self.model_dir);
            }

            if let Some(ref mut engine) = self.tts_engine {
                let sid = std::env::var("VOICE_SPEAKER_ID")
                    .ok()
                    .and_then(|s| s.trim().parse::<i32>().ok())
                    .unwrap_or(0);
                let speed = std::env::var("VOICE_SPEED")
                    .ok()
                    .and_then(|s| s.trim().parse::<f32>().ok())
                    .unwrap_or(1.0);

                println!("[TTS Clone] Synthèse Sherpa Vits pour : \"{}\"", text);
                let audio = engine
                    .create(text, sid, speed)
                    .map_err(|e| format!("Erreur synthèse Sherpa Vits: {e}"))?;
                println!("[TTS Clone] Synthèse terminée ({} échantillons).", audio.samples.len());
                return Ok((audio.samples, audio.sample_rate as u32));
            }
        }

        // Repli si le modèle ONNX n'est pas encore téléchargé dans models/voice-clone/
        Err("Modèle de clonage vocal non installé dans models/voice-clone/.".to_string())
    }

    pub fn stop(&self) {
        self.player.stop();
    }

    pub fn speak_sentence(
        &mut self,
        sentence: &str,
        rx: &std::sync::mpsc::Receiver<TtsCommand>,
        pending_cmd: &mut Option<TtsCommand>,
        sentence_queue: &mut VecDeque<String>,
    ) -> Result<bool, String> {
        let (samples, rate) = self.synthesize(sentence)?;
        self.player.play_samples(samples, rate);

        while self.player.is_playing() {
            let t = std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap_or_default()
                .as_millis() as f32 * 0.015;
            let sim_level = (0.35 + 0.35 * (t * 1.8).sin().abs()).clamp(0.15, 0.85);
            set_tts_audio_level(sim_level);

            std::thread::sleep(Duration::from_millis(15));

            if let Ok(next_cmd) = rx.try_recv() {
                match next_cmd {
                    TtsCommand::SetGender(_) => {}
                    TtsCommand::Stop => {
                        self.player.stop();
                        set_tts_audio_level(0.0);
                        return Ok(true);
                    }
                    TtsCommand::Speak(next_text) => {
                        self.player.stop();
                        set_tts_audio_level(0.0);
                        *pending_cmd = Some(TtsCommand::Speak(next_text));
                        return Ok(true);
                    }
                    TtsCommand::Enqueue(next_text) => {
                        let clean = sanitize_for_tts(&next_text);
                        for s in split_into_sentences(&clean) {
                            sentence_queue.push_back(s);
                        }
                    }
                }
            }
        }

        set_tts_audio_level(0.0);
        Ok(false)
    }
}

/// Backend TTS pour modèle de clonage vocal (XTTS-v2 / F5-TTS / Kokoro / Piper)
pub struct HttpVoiceCloneBackend {
    endpoint: String,
    speaker_wav: Option<String>,
    player: AudioPlayer,
    client: reqwest::Client,
    rt: tokio::runtime::Runtime,
}

impl HttpVoiceCloneBackend {
    pub fn new() -> Option<Self> {
        let player = AudioPlayer::new().ok()?;
        let endpoint = std::env::var("VOICE_CLONE_URL")
            .ok()
            .or_else(|| read_env_var_from_file(".env", "VOICE_CLONE_URL"))
            .or_else(|| std::env::var("TTS_CLONE_URL").ok())
            .or_else(|| read_env_var_from_file(".env", "TTS_CLONE_URL"))
            .map(|s| s.trim().to_string())
            .filter(|s| !s.is_empty())?;

        let speaker_wav = std::env::var("VOICE_REFERENCE_WAV")
            .ok()
            .or_else(|| read_env_var_from_file(".env", "VOICE_REFERENCE_WAV"))
            .map(|s| s.trim().to_string())
            .filter(|s| !s.is_empty())
            .or_else(|| {
                for cand in &["models/voice_reference.wav", "speaker.wav", "models/speaker.wav", "voice.wav"] {
                    if std::path::Path::new(cand).exists() {
                        return Some(cand.to_string());
                    }
                }
                None
            })
            .map(|p| {
                let path = std::path::Path::new(&p);
                if path.is_relative() {
                    if let Ok(cwd) = std::env::current_dir() {
                        return cwd.join(path).to_string_lossy().to_string();
                    }
                }
                p
            });

        let rt = tokio::runtime::Builder::new_current_thread()
            .enable_all()
            .build()
            .ok()?;

        let client = reqwest::Client::builder()
            .timeout(Duration::from_secs(12))
            .build()
            .ok()?;

        println!("[TTS Clone] Moteur vocal cloné configuré sur '{}' (échantillon: {:?})", endpoint, speaker_wav);

        Some(Self {
            endpoint,
            speaker_wav,
            player,
            client,
            rt,
        })
    }

    pub fn synthesize(&self, text: &str) -> Result<(Vec<f32>, u32), String> {
        let ref_text = std::env::var("VOICE_REFERENCE_TEXT")
            .ok()
            .or_else(|| read_env_var_from_file(".env", "VOICE_REFERENCE_TEXT"))
            .map(|s| s.trim().to_string())
            .filter(|s| !s.is_empty());

        let mut body = serde_json::json!({
            "text": text,
            "language": "fr",
            "speaker_wav": self.speaker_wav,
        });

        if let Some(ref rt) = ref_text {
            body["speaker_text"] = serde_json::json!(rt);
            body["reference_text"] = serde_json::json!(rt);
            body["prompt_text"] = serde_json::json!(rt);
        }

        let endpoint = self.endpoint.clone();
        let client = self.client.clone();

        let bytes = self.rt.block_on(async {
            let res = client.post(&endpoint)
                .json(&body)
                .send()
                .await
                .map_err(|e| format!("Connexion serveur TTS impossible: {e}"))?;

            if !res.status().is_success() {
                return Err(format!("Erreur HTTP TTS: {}", res.status()));
            }

            res.bytes().await.map_err(|e| format!("Lecture réponse audio: {e}"))
        })?;

        decode_wav_to_samples(&bytes).ok_or_else(|| "Échec décodage WAV de la voix clonée".to_string())
    }

    pub fn stop(&self) {
        self.player.stop();
    }

    pub fn speak_sentence(
        &mut self,
        sentence: &str,
        rx: &std::sync::mpsc::Receiver<TtsCommand>,
        pending_cmd: &mut Option<TtsCommand>,
        sentence_queue: &mut VecDeque<String>,
    ) -> Result<bool, String> {
        let (samples, rate) = self.synthesize(sentence)?;
        self.player.play_samples(samples, rate);

        while self.player.is_playing() {
            std::thread::sleep(Duration::from_millis(20));

            if let Ok(next_cmd) = rx.try_recv() {
                match next_cmd {
                    TtsCommand::SetGender(_) => {}
                    TtsCommand::Stop => {
                        self.player.stop();
                        set_tts_audio_level(0.0);
                        return Ok(true);
                    }
                    TtsCommand::Speak(next_text) => {
                        self.player.stop();
                        set_tts_audio_level(0.0);
                        *pending_cmd = Some(TtsCommand::Speak(next_text));
                        return Ok(true);
                    }
                    TtsCommand::Enqueue(next_text) => {
                        let clean = sanitize_for_tts(&next_text);
                        for s in split_into_sentences(&clean) {
                            sentence_queue.push_back(s);
                        }
                    }
                }
            }
        }
        set_tts_audio_level(0.0);
        Ok(false)
    }
}

pub struct CompositeTtsBackend {
    current_gender: VoiceGender,
    clone_backend: Option<HttpVoiceCloneBackend>,
    native_clone_backend: Option<NativeVoiceCloneBackend>,
    #[cfg(windows)]
    system_backend: Option<SapiBackend>,
    #[cfg(not(windows))]
    system_backend: Option<UnixTtsBackend>,
}

trait SystemTtsBackend {
    fn set_gender(&mut self, gender: VoiceGender);
    fn stop(&mut self);
    fn speak_sentence(
        &mut self,
        sentence: &str,
        rx: &std::sync::mpsc::Receiver<TtsCommand>,
        pending_cmd: &mut Option<TtsCommand>,
        sentence_queue: &mut VecDeque<String>,
    ) -> bool; // renvoie true si interrompu
}

#[cfg(windows)]
struct SapiBackend {
    voice: windows::Win32::Media::Speech::ISpVoice,
    current_gender: VoiceGender,
    base_flags: u32,
}

#[cfg(windows)]
impl SapiBackend {
    fn new(initial_voice: VoiceGender) -> Option<Self> {
        use windows::Win32::Media::Speech::{SpVoice, SPF_ASYNC, SPF_IS_NOT_XML, SPF_PURGEBEFORESPEAK};
        use windows::Win32::System::Com::{CoCreateInstance, CoInitializeEx, CLSCTX_ALL, COINIT_MULTITHREADED};

        unsafe {
            let _ = CoInitializeEx(None, COINIT_MULTITHREADED);
            let voice: Result<windows::Win32::Media::Speech::ISpVoice, _> = CoCreateInstance(&SpVoice, None, CLSCTX_ALL);
            let Ok(voice) = voice else {
                eprintln!("[TTS] Impossible d'initialiser le synthétiseur vocal Windows.");
                return None;
            };

            find_and_apply_voice(&voice, initial_voice);
            let _ = voice.SetVolume(100);
            let _ = voice.SetRate(1);
            let base_flags = (SPF_ASYNC.0 | SPF_PURGEBEFORESPEAK.0 | SPF_IS_NOT_XML.0) as u32;

            Some(Self {
                voice,
                current_gender: initial_voice,
                base_flags,
            })
        }
    }
}

#[cfg(windows)]
impl SystemTtsBackend for SapiBackend {
    fn set_gender(&mut self, gender: VoiceGender) {
        self.current_gender = gender;
        unsafe {
            find_and_apply_voice(&self.voice, self.current_gender);
        }
    }

    fn stop(&mut self) {
        use windows::core::PCWSTR;
        unsafe {
            let _ = self.voice.Speak(PCWSTR::null(), self.base_flags, None);
        }
        set_tts_audio_level(0.0);
    }

    fn speak_sentence(
        &mut self,
        sentence: &str,
        rx: &std::sync::mpsc::Receiver<TtsCommand>,
        pending_cmd: &mut Option<TtsCommand>,
        sentence_queue: &mut VecDeque<String>,
    ) -> bool {
        use windows::core::PCWSTR;
        use windows::Win32::Media::Speech::SPVOICESTATUS;

        let active_wide_buf: Vec<u16> = sentence.encode_utf16().chain(std::iter::once(0)).collect();

        unsafe {
            if self.voice.Speak(PCWSTR(active_wide_buf.as_ptr()), self.base_flags, None).is_err() {
                return false;
            }

            let start_wait = Instant::now();
            let mut has_started = false;
            let mut smoothed_level = 0.0f32;
            const STARTUP_TIMEOUT: Duration = Duration::from_millis(3000);

            loop {
                std::thread::sleep(Duration::from_millis(20));

                if let Ok(next_cmd) = rx.try_recv() {
                    match next_cmd {
                        TtsCommand::SetGender(new_gender) => {
                            self.set_gender(new_gender);
                        }
                        TtsCommand::Stop => {
                            let _ = self.voice.Speak(PCWSTR::null(), self.base_flags, None);
                            set_tts_audio_level(0.0);
                            return true;
                        }
                        TtsCommand::Speak(next_text) => {
                            let _ = self.voice.Speak(PCWSTR::null(), self.base_flags, None);
                            set_tts_audio_level(0.0);
                            *pending_cmd = Some(TtsCommand::Speak(next_text));
                            return true;
                        }
                        TtsCommand::Enqueue(next_text) => {
                            let clean = sanitize_for_tts(&next_text);
                            for s in split_into_sentences(&clean) {
                                sentence_queue.push_back(s);
                            }
                        }
                    }
                }

                let mut status = SPVOICESTATUS::default();
                if self.voice.GetStatus(&mut status, std::ptr::null_mut()).is_ok() {
                    if status.dwRunningState == 2 {
                        has_started = true;
                        let elapsed = start_wait.elapsed().as_secs_f32();
                        smoothed_level = calculate_viseme_audio_level(
                            status.VisemeId.0,
                            status.PhonemeId as u32,
                            elapsed,
                            smoothed_level,
                        );
                        set_tts_audio_level(smoothed_level);
                    } else if has_started {
                        set_tts_audio_level(0.0);
                        break;
                    } else if start_wait.elapsed() > STARTUP_TIMEOUT {
                        set_tts_audio_level(0.0);
                        break;
                    } else {
                        set_tts_audio_level(0.0);
                    }
                } else {
                    set_tts_audio_level(0.0);
                    break;
                }
            }
        }
        false
    }
}

#[cfg(not(windows))]
struct UnixTtsBackend {
    current_gender: VoiceGender,
}

#[cfg(not(windows))]
impl UnixTtsBackend {
    fn new(initial_voice: VoiceGender) -> Option<Self> {
        Some(Self {
            current_gender: initial_voice,
        })
    }
}

#[cfg(not(windows))]
impl SystemTtsBackend for UnixTtsBackend {
    fn set_gender(&mut self, gender: VoiceGender) {
        self.current_gender = gender;
    }

    fn stop(&mut self) {
        set_tts_audio_level(0.0);
    }

    fn speak_sentence(
        &mut self,
        sentence: &str,
        rx: &std::sync::mpsc::Receiver<TtsCommand>,
        pending_cmd: &mut Option<TtsCommand>,
        sentence_queue: &mut VecDeque<String>,
    ) -> bool {
        #[cfg(target_os = "macos")]
        let mut child_proc = {
            let macos_voice = match self.current_gender {
                VoiceGender::Denise | VoiceGender::Hortense | VoiceGender::Sylvie | VoiceGender::Clone => "Amelie",
                VoiceGender::Henri | VoiceGender::Paul | VoiceGender::Jean => "Thomas",
            };
            std::process::Command::new("say")
                .args(["-v", macos_voice, sentence])
                .spawn()
                .or_else(|_| std::process::Command::new("say").arg(sentence).spawn())
                .ok()
        };

        #[cfg(not(target_os = "macos"))]
        let mut child_proc = {
            let (spd_voice, espeak_voice) = match self.current_gender {
                VoiceGender::Denise | VoiceGender::Clone => ("female1", "fr+f2"),
                VoiceGender::Henri => ("male1", "fr+m2"),
                VoiceGender::Hortense => ("female2", "fr+f4"),
                VoiceGender::Paul => ("male2", "fr+m1"),
                VoiceGender::Sylvie => ("female3", "fr-ca+f2"),
                VoiceGender::Jean => ("male3", "fr+m3"),
            };

            std::process::Command::new("spd-say")
                .args(["-l", "fr", "-t", spd_voice, "-w", sentence])
                .spawn()
                .or_else(|_| std::process::Command::new("espeak-ng").args(["-v", espeak_voice, sentence]).spawn())
                .or_else(|_| std::process::Command::new("espeak").args(["-v", espeak_voice, sentence]).spawn())
                .ok()
        };

        if let Some(ref mut proc) = child_proc {
            while let Ok(None) = proc.try_wait() {
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
                            self.set_gender(new_gender);
                        }
                        TtsCommand::Stop => {
                            let _ = proc.kill();
                            set_tts_audio_level(0.0);
                            return true;
                        }
                        TtsCommand::Speak(next_text) => {
                            let _ = proc.kill();
                            set_tts_audio_level(0.0);
                            *pending_cmd = Some(TtsCommand::Speak(next_text));
                            return true;
                        }
                        TtsCommand::Enqueue(next_text) => {
                            let clean = sanitize_for_tts(&next_text);
                            for s in split_into_sentences(&clean) {
                                sentence_queue.push_back(s);
                            }
                        }
                    }
                }
            }
        }

        set_tts_audio_level(0.0);
        false
    }
}

impl SystemTtsBackend for CompositeTtsBackend {
    fn set_gender(&mut self, gender: VoiceGender) {
        self.current_gender = gender;
        if gender == VoiceGender::Clone {
            if self.native_clone_backend.is_none() {
                self.native_clone_backend = NativeVoiceCloneBackend::new();
            }
            if self.clone_backend.is_none() {
                self.clone_backend = HttpVoiceCloneBackend::new();
            }
        }
        if let Some(ref mut sys) = self.system_backend {
            sys.set_gender(gender);
        }
    }

    fn stop(&mut self) {
        if let Some(ref mut clone) = self.clone_backend {
            clone.stop();
        }
        if let Some(ref mut nat) = self.native_clone_backend {
            nat.stop();
        }
        if let Some(ref mut sys) = self.system_backend {
            sys.stop();
        }
        set_tts_audio_level(0.0);
    }

    fn speak_sentence(
        &mut self,
        sentence: &str,
        rx: &std::sync::mpsc::Receiver<TtsCommand>,
        pending_cmd: &mut Option<TtsCommand>,
        sentence_queue: &mut VecDeque<String>,
    ) -> bool {
        let force_clone = std::env::var("TTS_BACKEND")
            .ok()
            .or_else(|| read_env_var_from_file(".env", "TTS_BACKEND"))
            .map_or(false, |b| b.eq_ignore_ascii_case("clone") || b.eq_ignore_ascii_case("sherpa") || b.eq_ignore_ascii_case("http"));

        if self.current_gender == VoiceGender::Clone || force_clone {
            // Initialisation paresseuse si nécessaire
            if self.native_clone_backend.is_none() {
                self.native_clone_backend = NativeVoiceCloneBackend::new();
            }
            if self.clone_backend.is_none() {
                self.clone_backend = HttpVoiceCloneBackend::new();
            }

            // 1. Essai du moteur natif Sherpa en Rust en priorité absolue
            if let Some(ref mut nat) = self.native_clone_backend {
                if let Ok(interrupted) = nat.speak_sentence(sentence, rx, pending_cmd, sentence_queue) {
                    println!("[TTS] Voix synthétisée via le moteur local Sherpa VITS.");
                    return interrupted;
                }
            }

            // 2. Repli sur le service HTTP local si disponible
            if let Some(ref mut clone) = self.clone_backend {
                match clone.speak_sentence(sentence, rx, pending_cmd, sentence_queue) {
                    Ok(interrupted) => return interrupted,
                    Err(err) => {
                        eprintln!("[TTS Clone] Serveur distant injoignable ({err}). Repli sur la voix système (Denise).");
                    }
                }
            }
        }

        if let Some(ref mut sys) = self.system_backend {
            sys.speak_sentence(sentence, rx, pending_cmd, sentence_queue)
        } else {
            false
        }
    }
}

/// Lance le worker de synthèse vocale réactif
pub fn spawn_tts_worker(event_tx: StdSender<AgentEvent>, initial_voice: VoiceGender) -> StdSender<TtsCommand> {
    let (tx, rx) = channel::<TtsCommand>();
    std::thread::spawn(move || {
        let mut current_gender = initial_voice;

        #[cfg(windows)]
        let system_backend = SapiBackend::new(current_gender);

        #[cfg(not(windows))]
        let system_backend = UnixTtsBackend::new(current_gender);

        let is_clone_active = current_gender == VoiceGender::Clone
            || std::env::var("TTS_BACKEND").map_or(false, |b| b.eq_ignore_ascii_case("clone") || b.eq_ignore_ascii_case("sherpa"));

        let clone_backend = if is_clone_active { HttpVoiceCloneBackend::new() } else { None };
        let native_clone_backend = if is_clone_active { NativeVoiceCloneBackend::new() } else { None };

        let mut composite_backend = CompositeTtsBackend {
            current_gender,
            clone_backend,
            native_clone_backend,
            system_backend,
        };

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
                    composite_backend.set_gender(new_gender);
                }
                TtsCommand::Stop => {
                    composite_backend.stop();
                    set_tts_audio_level(0.0);
                }
                TtsCommand::Speak(raw_text) | TtsCommand::Enqueue(raw_text) => {
                    let clean_text = sanitize_for_tts(&raw_text);
                    let mut sentence_queue: VecDeque<String> = split_into_sentences(&clean_text).into();
                    if sentence_queue.is_empty() {
                        let _ = event_tx.send(AgentEvent::StatusChanged(AgentStatus::Idle));
                        let _ = event_tx.send(AgentEvent::TtsFinished);
                        continue;
                    }

                    let _ = event_tx.send(AgentEvent::StatusChanged(AgentStatus::Speaking));
                    let mut was_interrupted = false;

                    while let Some(sentence) = sentence_queue.pop_front() {
                        if composite_backend.speak_sentence(&sentence, &rx, &mut pending_cmd, &mut sentence_queue) {
                            was_interrupted = true;
                            break;
                        }

                        // Pause prosodique entre les phrases
                        let pause_dur = punctuation_pause_duration(&sentence);
                        let pause_start = Instant::now();
                        while pause_start.elapsed() < pause_dur {
                            std::thread::sleep(Duration::from_millis(10));
                            if let Ok(next_cmd) = rx.try_recv() {
                                match next_cmd {
                                    TtsCommand::SetGender(new_gender) => {
                                        current_gender = new_gender;
                                        composite_backend.set_gender(new_gender);
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
                                    TtsCommand::Enqueue(next_text) => {
                                        let clean = sanitize_for_tts(&next_text);
                                        for s in split_into_sentences(&clean) {
                                            sentence_queue.push_back(s);
                                        }
                                    }
                                }
                            }
                        }
                        if was_interrupted {
                            break;
                        }
                    }

                    if !was_interrupted {
                        set_tts_audio_level(0.0);
                        let _ = event_tx.send(AgentEvent::StatusChanged(AgentStatus::Idle));
                        let _ = event_tx.send(AgentEvent::TtsFinished);
                    }
                }
            }
        }
    });

    tx
}