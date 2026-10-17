use std::sync::mpsc::{channel, Sender as StdSender};
use std::time::{Duration, Instant};

use crate::set_tts_audio_level;
use crate::types::{AgentEvent, AgentStatus, TtsCommand, VoiceGender};

/// Nettoie le texte Markdown/HTML et convertit les symboles en texte parlé naturel
pub fn sanitize_for_tts(text: &str) -> String {
    let mut cleaned = String::new();
    let mut in_code_block = false;

    for line in text.lines() {
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

/// Lance le worker de synthèse vocale réactif
pub fn spawn_tts_worker(event_tx: StdSender<AgentEvent>) -> StdSender<TtsCommand> {
    let (tx, rx) = channel::<TtsCommand>();
    std::thread::spawn(move || {
        let mut current_gender = VoiceGender::Female; // Denise par défaut

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

                        for sentence in sentences {
                            let active_wide_buf: Vec<u16> =
                                sentence.encode_utf16().chain(std::iter::once(0)).collect();

                            if voice
                                .Speak(PCWSTR(active_wide_buf.as_ptr()), base_flags, None)
                                .is_ok()
                            {
                                let start_wait = Instant::now();
                                let mut has_started = false;
                                const STARTUP_TIMEOUT: Duration = Duration::from_millis(1200);

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
                                        } else if has_started || start_wait.elapsed() > STARTUP_TIMEOUT {
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

                            if was_interrupted {
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
                            if was_interrupted { break; }
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
                        };

                        let mut was_interrupted = false;
                        for sentence in sentences {
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
                            if was_interrupted {
                                break;
                            }
                            set_tts_audio_level(0.0);
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