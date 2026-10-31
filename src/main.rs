#[macro_use]
extern crate rust_i18n;
i18n!("locales", fallback = "en");

mod autostart;
mod audio;
mod automation;
mod floating_window;
mod fs_util;
mod html_util;
mod pattern;
mod scheduler;
mod theme;
mod tts;
mod twitch;
mod types;
mod window_util;
mod commands;
mod overlay;
mod ui;
mod ocr;
mod local_llm;

use audio::spawn_audio_worker;
use floating_window::{update_open_floating_cards, FloatingCard, FloatingCardOutput, FloatingContentType, OPEN_FLOATING_CARDS};
use automation::*;
use fs_util::*;
use theme::{AppTheme, SettingsTab};
use tts::{sanitize_for_tts, spawn_tts_worker};
use twitch::{search_twitch_channels, spawn_twitch_worker};
use window_util::*;
use eframe::egui;
pub use overlay::*;
use types::*;
use std::sync::Arc;
use std::sync::atomic::{AtomicU32, Ordering};
use std::sync::mpsc::{channel, Receiver, Sender as StdSender};
use tokio::sync::mpsc::{unbounded_channel, UnboundedReceiver, UnboundedSender};
use std::time::{Duration, Instant};

#[cfg(windows)]
use windows::core::Interface;
#[cfg(windows)]
use windows::Win32::UI::Input::KeyboardAndMouse::GetAsyncKeyState;

pub struct OverlayApp {
    status: AgentStatus,
    chat_history: Vec<ChatEntry>,
    active_tab: ActiveTab,
    twitch_messages: Vec<TwitchMessage>,
    live_transcript: String,
    input_text: String,
    is_recording: bool,
    whisper_ready: bool,
    smoothed_audio_level: f32,
    continuous_mode: bool,
    dictation_mode: bool,
    zoom_factor: f32,
    app_language: String,
    voice_gender: VoiceGender,
    llm_config: LlmConfig,
    theme: AppTheme,
    settings_tab: SettingsTab,
    dwell_state: DwellState,
    last_trackball_input: Instant,

    // Accessibilité : Ciblage par numéros & Mode contacteur unique (Switch Access)
    numbered_targets: Vec<NumberedElementTarget>,
    numbered_last_refresh: Instant,
    numbered_last_hwnd: Option<isize>,
    numbered_last_win_rect: Option<[i32; 4]>,
    numbered_input_buffer: String,
    numbered_input_last_char_time: Instant,
    targets_request_tx: StdSender<TargetsScanRequest>,
    targets_result_rx: Receiver<TargetsScanResult>,
    scan_sequence_counter: u64,
    last_applied_scan_sequence: u64,
    is_scan_in_progress: bool,
    switch_access_mode: bool,
    switch_step: SwitchAccessStep,
    switch_windows: Vec<SwitchWindowTarget>,
    switch_zones: Vec<NavigationZone>,
    switch_active_window_id: Option<isize>,
    switch_active_zone_index: Option<usize>,
    window_analysis_cache: std::collections::HashMap<isize, CachedWindowAnalysis>,
    last_switch_input: Instant,
    switch_auto_scroll: bool,
    switch_focus_index: usize,
    switch_last_tick: Instant,

    // Mode survol : Détection du pointeur, anti-rebond & hiérarchie vocale
    hover_last_cursor_pos: Option<(i32, i32)>,
    hover_cursor_still_since: Instant,
    hover_pending_speech: Option<String>,
    hover_speech_deadline: Option<Instant>,
    hover_last_announced_target: Option<usize>,
    hover_last_announced_zone: Option<String>,
    hover_controls_speech_enabled: bool,
    hover_physical_mouse_moved: bool,

    // Canaux de communication asynchrones
    event_sender: StdSender<AgentEvent>,
    event_receiver: Receiver<AgentEvent>,
    command_sender: UnboundedSender<AgentCommand>,
    tts_sender: StdSender<TtsCommand>,
    audio_sender: StdSender<AudioCommand>,
    twitch_channel_sender: StdSender<String>,
    current_twitch_channel: String,
    twitch_search_query: String,
    twitch_search_results: Vec<TwitchChannelItem>,
    position_initialized: bool,
    floating_cards: Vec<FloatingCard>,
    window_pos: Option<egui::Pos2>,
    window_bottom_y: Option<f32>,
    drag_offset: Option<egui::Vec2>,
    chat_height: f32,
    current_win_w: f32,
    current_win_h: f32,
    is_hidden: bool,
    mouse_passthrough: bool,
    is_streaming_narration: bool,

    // Boîte modale de configuration de la clé API
    show_api_key_modal: bool,
    api_key_input: String,
    model_input: String,
    endpoint_input: String,
    show_api_key_plain: bool,
    selected_provider_modal: LlmProvider,
    api_key_error: Option<String>,

    local_download_progress: Option<(f32, f32, f32, f32)>,
    local_llm_options: local_llm::LocalLlmOptions,

    // Suggestions d'applications en direct (PATH & Levenshtein)
    input_suggestions: Vec<String>,
    last_suggestion_query: String,

    // Macros personnalisées ('raccourci' => texte complet)
    macros: Vec<(String, String)>,
    macro_shortcut_input: String,
    macro_text_input: String,
    macro_error: Option<String>,
}

/// Formate les éléments interactifs d'une fenêtre de manière ultra-compacte pour les petits LLMs locaux
pub fn format_compact_window_elements(win_title: &str, targets: &[NumberedElementTarget]) -> String {
    let mut out = String::new();
    let title_clean = win_title.trim();
    if !title_clean.is_empty() {
        out.push_str(&format!("[Fenêtre active au premier plan : \"{}\"]\n", title_clean));
    } else {
        out.push_str("[Fenêtre active au premier plan]\n");
    }
    if targets.is_empty() {
        out.push_str("(Aucun contrôle interactif identifié)\n");
    } else {
        out.push_str("Contrôles interactifs détectés :\n");
        for t in targets.iter().take(110) {
            let role = match t.role.as_str() {
                "bouton" | "button" => "bouton",
                "champ de saisie" | "input" => "saisie",
                "zone de texte" | "textarea" => "texte",
                "lien" | "link" => "lien",
                "onglet" | "tab" => "onglet",
                "case à cocher" | "checkbox" => "case",
                "liste déroulante" | "select" | "combobox" => "menu",
                other => other,
            };
            let name = t.name.trim();
            let val = t.value.trim();
            if !name.is_empty() && !val.is_empty() && val != name {
                out.push_str(&format!("- #{} [{}] \"{}\" (val: \"{}\")\n", t.index, role, name, val));
            } else if !name.is_empty() {
                out.push_str(&format!("- #{} [{}] \"{}\"\n", t.index, role, name));
            } else if !val.is_empty() {
                out.push_str(&format!("- #{} [{}] val: \"{}\"\n", t.index, role, val));
            } else {
                out.push_str(&format!("- #{} [{}]\n", t.index, role));
            }
        }
        if targets.len() > 110 {
            out.push_str(&format!("... et {} autres éléments non listés.\n", targets.len() - 110));
        }
    }
    out
}

/// Extrait les derniers messages de l'historique en conservant au maximum `max_user_msgs` messages utilisateur
fn get_recent_history_capped_at_user_messages(history: &[ChatMessage], max_user_msgs: usize) -> Vec<ChatMessage> {
    let mut user_count = 0;
    let mut start_idx = 0;
    for (i, msg) in history.iter().enumerate().rev() {
        if msg.role == "user" {
            user_count += 1;
            if user_count == max_user_msgs {
                start_idx = i;
                break;
            }
        }
    }
    let mut recent = history[start_idx..].to_vec();
    let user_indices: Vec<usize> = recent
        .iter()
        .enumerate()
        .filter(|(_, m)| m.role == "user")
        .map(|(i, _)| i)
        .collect();

    if let Some(&last_user_idx) = user_indices.last() {
        for &idx in &user_indices {
            if idx != last_user_idx {
                let content = &recent[idx].content;
                if let Some(pos) = content.rfind("[Demande utilisateur]\n") {
                    recent[idx].content = content[pos + "[Demande utilisateur]\n".len()..].trim().to_string();
                } else if let Some(pos) = content.find("[État actuel de l'écran]") {
                    if let Some(end_pos) = content[pos..].find("\n\n") {
                        recent[idx].content = content[pos + end_pos + 2..].trim().to_string();
                    }
                }
            }
        }
    }
    recent
}

fn extract_badge_number(text: &str) -> Option<usize> {
    let lower = text.trim().to_lowercase();
    let clean = lower
        .trim_start_matches('#')
        .trim_start_matches("n°")
        .trim_start_matches("no.")
        .trim_start_matches("no ")
        .trim();

    if let Ok(n) = clean.parse::<usize>() {
        return Some(n);
    }

    match clean {
        "un" => return Some(1),
        "deux" => return Some(2),
        "trois" => return Some(3),
        "quatre" => return Some(4),
        "cinq" => return Some(5),
        "six" => return Some(6),
        "sept" => return Some(7),
        "huit" => return Some(8),
        "neuf" => return Some(9),
        "dix" => return Some(10),
        "onze" => return Some(11),
        "douze" => return Some(12),
        "treize" => return Some(13),
        "quatorze" => return Some(14),
        "quinze" => return Some(15),
        "seize" => return Some(16),
        "dix-sept" => return Some(17),
        "dix-huit" => return Some(18),
        "dix-neuf" => return Some(19),
        "vingt" => return Some(20),
        _ => {}
    }

    let words: Vec<&str> = lower.split_whitespace().collect();
    let is_badge_command = words.len() <= 6
        && (words.contains(&"badge")
            || words.contains(&"badges")
            || words.contains(&"numero")
            || words.contains(&"numéro")
            || words.contains(&"cible")
            || words.contains(&"element")
            || words.contains(&"élément")
            || words.contains(&"clic")
            || words.contains(&"clique")
            || words.contains(&"cliquer")
            || words.contains(&"click"));

    if is_badge_command || words.len() <= 2 {
        for word in words {
            let w_clean = word
                .trim_start_matches('#')
                .trim_start_matches("n°")
                .trim_start_matches("no.")
                .trim_end_matches('.')
                .trim_end_matches(',');
            if let Ok(n) = w_clean.parse::<usize>() {
                return Some(n);
            }
            match w_clean {
                "un" => return Some(1),
                "deux" => return Some(2),
                "trois" => return Some(3),
                "quatre" => return Some(4),
                "cinq" => return Some(5),
                "six" => return Some(6),
                "sept" => return Some(7),
                "huit" => return Some(8),
                "neuf" => return Some(9),
                "dix" => return Some(10),
                "onze" => return Some(11),
                "douze" => return Some(12),
                "treize" => return Some(13),
                "quatorze" => return Some(14),
                "quinze" => return Some(15),
                "seize" => return Some(16),
                "dix-sept" => return Some(17),
                "dix-huit" => return Some(18),
                "dix-neuf" => return Some(19),
                "vingt" => return Some(20),
                _ => {}
            }
        }
    }
    None
}

fn is_start_subtitles_command(norm: &str) -> bool {
    let localized_cmd = normalize_for_command_match(&t!("commands.start_subtitles").to_string());
    let localized_title = normalize_for_command_match(&t!("commands.subtitles_title").to_string());
    if !localized_cmd.is_empty() && (norm == localized_cmd || norm.contains(&localized_cmd)) {
        return true;
    }
    if !localized_title.is_empty() && (norm == localized_title || norm.contains(&localized_title)) {
        return true;
    }

    let kw_str = t!("commands.start_subtitles_keywords").to_string();
    for kw in kw_str.split(',') {
        let norm_kw = normalize_for_command_match(kw.trim());
        if !norm_kw.is_empty() && (norm == norm_kw || norm.contains(&norm_kw)) {
            return true;
        }
    }

    (norm.contains("active") || norm.contains("activer") || norm.contains("lance") || norm.contains("lancer") || norm.contains("met") || norm.contains("mettre") || norm.contains("affiche") || norm.contains("afficher") || norm.contains("ouvre") || norm.contains("ouvrir") || norm.contains("enable") || norm.contains("start") || norm.contains("turn on") || norm.contains("show"))
        && (norm.contains("sous titre") || norm.contains("sous-titre") || norm.contains("soustitre") || norm.contains("subtitles") || norm.contains("subtitle") || norm.contains("transcription en direct") || norm.contains("transcription live") || norm.contains("transcription audio"))
        || norm == "sous titres en direct" || norm == "sous-titres en direct" || norm == "sous titres" || norm == "sous-titres" || norm == "live subtitles"
        || norm.starts_with("sous titre") || norm.starts_with("sous-titre")
}

fn is_stop_subtitles_command(norm: &str) -> bool {
    let localized_cmd = normalize_for_command_match(&t!("commands.stop_subtitles").to_string());
    if !localized_cmd.is_empty() && (norm == localized_cmd || norm.contains(&localized_cmd)) {
        return true;
    }

    let kw_str = t!("commands.stop_subtitles_keywords").to_string();
    for kw in kw_str.split(',') {
        let norm_kw = normalize_for_command_match(kw.trim());
        if !norm_kw.is_empty() && (norm == norm_kw || norm.contains(&norm_kw)) {
            return true;
        }
    }

    (norm.contains("arrete") || norm.contains("arrête") || norm.contains("arreter") || norm.contains("arrêter")
        || norm.contains("stop") || norm.contains("stopper") || norm.contains("coupe") || norm.contains("couper")
        || norm.contains("ferme") || norm.contains("fermer") || norm.contains("desactive") || norm.contains("désactive")
        || norm.contains("desactiver") || norm.contains("désactiver") || norm.contains("quitte") || norm.contains("quitter")
        || norm.contains("disable") || norm.contains("turn off") || norm.contains("hide") || norm.contains("close"))
        && (norm.contains("sous titre") || norm.contains("sous-titre") || norm.contains("soustitre") || norm.contains("subtitles") || norm.contains("subtitle") || norm.contains("transcription"))
        || norm == "stop sous titres" || norm == "stop sous-titres" || norm == "stop subtitles"
}

fn is_read_card_intent(prompt: &str, floating_cards: &[FloatingCard]) -> Option<FloatingCard> {
    if floating_cards.is_empty() {
        return None;
    }
    let norm = prompt.trim();
    let lower = norm.to_lowercase();

    if lower == "lis la fenetre"
        || lower == "lis la fenêtre"
        || lower == "lis le resume"
        || lower == "lis le résumé"
        || lower == "lis la carte"
        || lower.starts_with("lis la fen")
        || lower.starts_with("lire la fen")
        || lower == "read window"
        || lower == "read card"
        || lower == "read the card"
        || lower == "read the window"
        || lower == "karte vorlesen"
        || lower == "fenster vorlesen"
        || lower == "leer ventana"
        || lower == "leer tarjeta"
    {
        return floating_cards.last().cloned();
    }

    if norm.starts_with('🔊') || norm.starts_with('🔉') || lower.starts_with("lire :") || lower.starts_with("read:") || lower.starts_with("leer:") || lower.starts_with("vorlesen:") || lower.starts_with("leggi:") || lower.starts_with("ler:") {
        let clean = norm.trim_start_matches(['🔊', '🔉']).trim();
        let target_title = if let Some((_, after)) = clean.split_once(':') {
            after.trim()
        } else if let Some((_, after)) = clean.split_once('：') {
            after.trim()
        } else {
            clean
        };
        if !target_title.is_empty() {
            if let Some(card) = floating_cards.iter().find(|c| c.title.eq_ignore_ascii_case(target_title) || c.id.eq_ignore_ascii_case(target_title)) {
                return Some(card.clone());
            }
        }
        return floating_cards.last().cloned();
    }
    None
}

/// Détecte la langue principale du système d'exploitation de l'utilisateur
pub fn get_system_locale_language() -> (String, String) {
    if let Ok(l) = std::env::var("APP_LANG").or_else(|_| read_env_var_from_file(".env", "APP_LANG").ok_or(())) {
        let l_clean = l.trim().to_lowercase();
        if l_clean != "auto" && !l_clean.is_empty() {
            let name = match l_clean.as_str() {
                "fr" => "français".to_string(),
                "en" => "English".to_string(),
                "es" => "español".to_string(),
                "de" => "Deutsch".to_string(),
                "it" => "italiano".to_string(),
                "pt" => "português".to_string(),
                "ja" => "日本語 (Japanese)".to_string(),
                "zh" => "中文 (Chinese)".to_string(),
                "ru" => "русский".to_string(),
                _ => l_clean.clone(),
            };
            return (l_clean, name);
        }
    }
    #[cfg(windows)]
    {
        extern "system" {
            fn GetUserDefaultLocaleName(lpLocaleName: *mut u16, cchLocaleName: i32) -> i32;
        }
        let mut buf = [0u16; 85];
        let len = unsafe { GetUserDefaultLocaleName(buf.as_mut_ptr(), buf.len() as i32) };
        if len > 0 {
            let locale = String::from_utf16_lossy(&buf[..(len - 1) as usize]).to_lowercase();
            let lang_prefix = locale.split('-').next().unwrap_or(&locale);
            let name: String = match lang_prefix {
                "fr" => "français".to_string(),
                "en" => "English".to_string(),
                "es" => "español".to_string(),
                "de" => "Deutsch".to_string(),
                "it" => "italiano".to_string(),
                "pt" => "português".to_string(),
                "ja" => "日本語 (Japanese)".to_string(),
                "zh" => "中文 (Chinese)".to_string(),
                "ru" => "русский".to_string(),
                other => other.to_string(),
            };
            return (locale, name);
        }
    }
    #[cfg(not(windows))]
    {
        let env_lang = std::env::var("LC_ALL").or_else(|_| std::env::var("LC_MESSAGES")).or_else(|_| std::env::var("LANG")).unwrap_or_else(|_| "fr_FR".to_string()).to_lowercase();
        let lang_prefix = env_lang.split(['_', '.']).next().unwrap_or("fr");
        let name: String = match lang_prefix {
            "fr" => "français".to_string(),
            "en" => "English".to_string(),
            "es" => "español".to_string(),
            "de" => "Deutsch".to_string(),
            "it" => "italiano".to_string(),
            "pt" => "português".to_string(),
            "ja" => "日本語 (Japanese)".to_string(),
            "zh" => "中文 (Chinese)".to_string(),
            "ru" => "русский".to_string(),
            other => other.to_string(),
        };
        return (env_lang, name);
    }
    ("fr".to_string(), "français (French)".to_string())
}

pub fn localized_theme_name(theme: AppTheme) -> String {
    let key = theme.key().to_lowercase();
    let name_low = theme.display_name().to_lowercase();
    if key.contains("cyan") || name_low.contains("cyan") {
        t!("themes.cyber_cyan.name").to_string()
    } else if key.contains("emerald") || key.contains("emeraude") || name_low.contains("menthe") || name_low.contains("emerald") || name_low.contains("émeraude") {
        t!("themes.mint_emerald.name").to_string()
    } else if key.contains("purple") || key.contains("amethyst") || name_low.contains("améthyste") || name_low.contains("purple") {
        t!("themes.neon_purple.name").to_string()
    } else if (key.contains("yellow") || name_low.contains("jaune") || name_low.contains("yellow")) && (key.contains("contrast") || name_low.contains("contraste")) {
        t!("themes.high_contrast_yellow.name").to_string()
    } else if (key.contains("white") || name_low.contains("blanc") || name_low.contains("white")) && (key.contains("contrast") || name_low.contains("contraste")) {
        t!("themes.high_contrast_white.name").to_string()
    } else if key.contains("amber") || key.contains("ambre") || name_low.contains("ambre") || name_low.contains("amber") {
        t!("themes.soft_amber.name").to_string()
    } else if key.contains("colorblind") || key.contains("dalton") || name_low.contains("dalton") || name_low.contains("colorblind") {
        t!("themes.colorblind_friendly.name").to_string()
    } else {
        theme.display_name().to_string()
    }
}

pub fn localized_theme_desc(theme: AppTheme) -> String {
    let key = theme.key().to_lowercase();
    let name_low = theme.display_name().to_lowercase();
    if key.contains("cyan") || name_low.contains("cyan") {
        t!("themes.cyber_cyan.desc").to_string()
    } else if key.contains("emerald") || key.contains("emeraude") || name_low.contains("menthe") || name_low.contains("emerald") || name_low.contains("émeraude") {
        t!("themes.mint_emerald.desc").to_string()
    } else if key.contains("purple") || key.contains("amethyst") || name_low.contains("améthyste") || name_low.contains("purple") {
        t!("themes.neon_purple.desc").to_string()
    } else if (key.contains("yellow") || name_low.contains("jaune") || name_low.contains("yellow")) && (key.contains("contrast") || name_low.contains("contraste")) {
        t!("themes.high_contrast_yellow.desc").to_string()
    } else if (key.contains("white") || name_low.contains("blanc") || name_low.contains("white")) && (key.contains("contrast") || name_low.contains("contraste")) {
        t!("themes.high_contrast_white.desc").to_string()
    } else if key.contains("amber") || key.contains("ambre") || name_low.contains("ambre") || name_low.contains("amber") {
        t!("themes.soft_amber.desc").to_string()
    } else if key.contains("colorblind") || key.contains("dalton") || name_low.contains("dalton") || name_low.contains("colorblind") {
        t!("themes.colorblind_friendly.desc").to_string()
    } else {
        theme.description().to_string()
    }
}

impl OverlayApp {
    pub fn new(
        cc: &eframe::CreationContext<'_>,
        event_sender: StdSender<AgentEvent>,
        event_receiver: Receiver<AgentEvent>,
        command_sender: UnboundedSender<AgentCommand>,
        tts_sender: StdSender<TtsCommand>,
        audio_sender: StdSender<AudioCommand>,
        twitch_channel_sender: StdSender<String>,
        initial_twitch_channel: String,
        initial_voice: VoiceGender,
    ) -> Self {
        setup_custom_fonts(&cc.egui_ctx);
        let initial_zoom = std::env::var("FONT_SIZE")
            .ok()
            .or_else(|| read_env_var_from_file(".env", "FONT_SIZE"))
            .or_else(|| std::env::var("ZOOM_FACTOR").ok())
            .or_else(|| read_env_var_from_file(".env", "ZOOM_FACTOR"))
            .and_then(|s| s.trim().parse::<f32>().ok())
            .map(|z| z.clamp(0.7, 2.5))
            .unwrap_or(1.0);
        cc.egui_ctx.set_zoom_factor(initial_zoom);

        let llm_config = resolve_llm_config();
        let theme = AppTheme::resolve_theme();
        let is_local_ep = llm_config.provider.is_local_or_custom_endpoint(&llm_config.endpoint);
        let has_no_key = llm_config.provider != LlmProvider::Local && llm_config.api_key.is_empty() && !is_local_ep;
        let hover_asked = is_hover_nav_asked();
        let app_language = std::env::var("APP_LANG")
            .ok()
            .or_else(|| read_env_var_from_file(".env", "APP_LANG"))
            .map(|s| s.trim().to_lowercase())
            .unwrap_or_else(|| "auto".to_string());
        let active_locale = if app_language == "en" {
            "en".to_string()
        } else if app_language == "fr" {
            "fr".to_string()
        } else if app_language != "auto" {
            app_language.clone()
        } else {
            let (locale, _) = get_system_locale_language();
            locale.split(['-', '_']).next().unwrap_or("fr").to_lowercase()
        };
        rust_i18n::set_locale(&active_locale);

        let (initial_subtitle, initial_suggestions) = if !hover_asked {
            let question = t!("speech.ask_hover_nav").to_string();
            let btn_hover = t!("speech.hover_mode_btn").to_string();
            let btn_no = t!("speech.no_thanks_btn").to_string();
            (
                question,
                vec![
                    QuickSuggestionItem::Text(btn_hover),
                    QuickSuggestionItem::Text(btn_no),
                ],
            )
        } else if has_no_key {
            (t!("speech.please_set_api_key").to_string(), Vec::new())
        } else {
            (t!("speech.waiting_instructions").to_string(), Vec::new())
        };
        let _ = tts_sender.send(TtsCommand::Speak(initial_subtitle.clone()));

        let dwell_millis = std::env::var("DWELL_TIME_MS")
            .ok()
            .or_else(|| read_env_var_from_file(".env", "DWELL_TIME_MS"))
            .and_then(|s| s.trim().parse::<u64>().ok())
            .unwrap_or(0);

        let dwell_activation_ms = std::env::var("DWELL_ACTIVATION_DELAY_MS")
            .ok()
            .or_else(|| read_env_var_from_file(".env", "DWELL_ACTIVATION_DELAY_MS"))
            .and_then(|s| s.trim().parse::<u64>().ok())
            .unwrap_or(1200);

        let (targets_request_tx, targets_request_rx) = channel::<TargetsScanRequest>();
        let (targets_result_tx, targets_result_rx) = channel::<TargetsScanResult>();
        spawn_targets_worker(targets_request_rx, targets_result_tx);
        let macros = load_macros();

        let mut app = Self {
            status: AgentStatus::Idle,
            chat_history: vec![ChatEntry {
                role: ChatRole::Agent,
                text: initial_subtitle,
                timestamp: current_time_str(),
                quick_suggestions: initial_suggestions,
                screen_size_kb: None,
            }],
            active_tab: ActiveTab::Assistance,
            twitch_messages: Vec::new(),
            live_transcript: String::new(),
            input_text: String::new(),
            smoothed_audio_level: 0.0,
            is_recording: false,
            whisper_ready: false,
            continuous_mode: true,
            dictation_mode: false,
            zoom_factor: initial_zoom,
            app_language,
            voice_gender: initial_voice,
            llm_config: llm_config.clone(),
            theme,
            settings_tab: SettingsTab::LlmProvider,
            dwell_state: DwellState::new_with_delay(dwell_millis, dwell_activation_ms),
            last_trackball_input: Instant::now() - Duration::from_secs(1),

            numbered_targets: Vec::new(),
            numbered_last_refresh: Instant::now(),
            numbered_last_hwnd: None,
            numbered_last_win_rect: None,
            numbered_input_buffer: String::new(),
            numbered_input_last_char_time: Instant::now(),
            targets_request_tx,
            targets_result_rx,
            scan_sequence_counter: 0,
            last_applied_scan_sequence: 0,
            is_scan_in_progress: false,
            switch_access_mode: false,
            switch_step: SwitchAccessStep::Windows,
            switch_windows: Vec::new(),
            switch_zones: Vec::new(),
            switch_active_window_id: None,
            switch_active_zone_index: None,
            window_analysis_cache: std::collections::HashMap::new(),
            last_switch_input: Instant::now() - Duration::from_secs(1),
            switch_auto_scroll: false,
            switch_focus_index: 0,
            switch_last_tick: Instant::now(),

            hover_last_cursor_pos: None,
            hover_cursor_still_since: Instant::now(),
            hover_pending_speech: None,
            hover_speech_deadline: None,
            hover_last_announced_target: None,
            hover_last_announced_zone: None,
            hover_controls_speech_enabled: false,
            hover_physical_mouse_moved: false,

            event_sender,
            event_receiver,
            command_sender,
            tts_sender,
            audio_sender,
            twitch_channel_sender,
            current_twitch_channel: initial_twitch_channel,
            twitch_search_query: String::new(),
            twitch_search_results: Vec::new(),
            position_initialized: false,
            floating_cards: Vec::new(),
            window_pos: None,
            window_bottom_y: None,
            drag_offset: None,
            current_win_w: 560.0,
            chat_height: 70.0,
            current_win_h: 196.0,
            is_hidden: false,
            mouse_passthrough: false,
            is_streaming_narration: false,
            show_api_key_modal: has_no_key,
            api_key_input: llm_config.api_key.clone(),
            model_input: llm_config.model.clone(),
            endpoint_input: llm_config.endpoint.clone(),
            selected_provider_modal: llm_config.provider,
            show_api_key_plain: false,
            api_key_error: None,
            local_download_progress: None,
            local_llm_options: local_llm::get_local_llm_options(),
            input_suggestions: Vec::new(),
            last_suggestion_query: String::new(),
            macros,
            macro_shortcut_input: String::new(),
            macro_text_input: String::new(),
            macro_error: None,
        };
        app.sync_modal_inputs_for_provider(llm_config.provider);
        update_open_floating_cards(&app.floating_cards);
        app
    }

    pub fn sync_modal_inputs_for_provider(&mut self, provider: LlmProvider) {
        self.selected_provider_modal = provider;
        let key_name = provider.env_key_name();
        self.api_key_input = std::env::var(key_name)
            .or_else(|_| read_env_var_from_file(".env", key_name).ok_or(()))
            .unwrap_or_default();

        let model_var = provider.env_model_name();
        self.model_input = std::env::var(model_var)
            .ok()
            .or_else(|| read_env_var_from_file(".env", model_var))
            .or_else(|| std::env::var("LLM_MODEL").ok())
            .or_else(|| read_env_var_from_file(".env", "LLM_MODEL"))
            .map(|s| s.trim().to_string())
            .filter(|s| !s.is_empty())
            .unwrap_or_else(|| provider.default_model().to_string());

        let endpoint_var = provider.env_endpoint_name();
        self.endpoint_input = std::env::var(endpoint_var)
            .ok()
            .or_else(|| read_env_var_from_file(".env", endpoint_var))
            .map(|s| s.trim().to_string())
            .filter(|s| !s.is_empty())
            .unwrap_or_else(|| provider.default_endpoint().to_string());

        if provider == LlmProvider::Local {
            self.model_input = local_llm::get_active_preset().name.to_string();
        }

        self.api_key_error = None;
    }

    pub fn set_theme(&mut self, theme: AppTheme) {
        self.theme = theme;
        let _ = write_env_var_to_file(".env", "THEME", theme.key());
        std::env::set_var("THEME", theme.key());
    }

    pub fn current_agent_model_display_name(&self) -> String {
        if self.llm_config.provider == LlmProvider::Local {
            local_llm::get_active_preset().name.to_string()
        } else if !self.llm_config.model.is_empty() {
            format!("{} ({})", self.llm_config.provider.display_name(), self.llm_config.model)
        } else {
            self.llm_config.provider.display_name().to_string()
        }
    }

    pub fn set_zoom_factor(&mut self, factor: f32) {
        let clamped = factor.clamp(0.7, 2.5);
        if (self.zoom_factor - clamped).abs() > 0.001 {
            self.zoom_factor = clamped;
            let val_str = format!("{:.2}", clamped);
            let _ = write_env_var_to_file(".env", "FONT_SIZE", &val_str);
            std::env::set_var("FONT_SIZE", &val_str);
        }
    }

    pub fn set_app_language(&mut self, lang: &str) {
        self.app_language = lang.to_string();
        std::env::set_var("APP_LANG", lang);
        let _ = write_env_var_to_file(".env", "APP_LANG", lang);
        let active_locale = if lang != "auto" {
            lang.to_string()
        } else {
            let (locale, _) = get_system_locale_language();
            locale.split(['-', '_']).next().unwrap_or("fr").to_lowercase()
        };
        rust_i18n::set_locale(&active_locale);
    }

    fn draw_avatar(&self, ui: &mut egui::Ui, center: egui::Pos2, base_radius: f32, time: f64) {
        draw_avatar(
            ui,
            center,
            base_radius,
            time,
            self.status,
            self.smoothed_audio_level,
            &self.theme.colors(),
        );
    }

    fn draw_mini_equalizer(&self, ui: &mut egui::Ui, time: f64) {
        draw_mini_equalizer(
            ui,
            time,
            self.status,
            self.smoothed_audio_level,
            &self.theme.colors(),
        );
    }

    pub fn set_voice_gender(&mut self, gender: VoiceGender) {
        self.voice_gender = gender;
        let _ = self.tts_sender.send(TtsCommand::SetGender(gender));
        std::env::set_var("VOICE_PROFILE", gender.to_key_str());
        let _ = write_env_var_to_file(".env", "VOICE_PROFILE", gender.to_key_str());
        let confirmation = match gender {
            VoiceProfile::Denise => t!("speech.voice_denise").to_string(),
            VoiceProfile::Henri => t!("speech.voice_henri").to_string(),
            VoiceProfile::Hortense => t!("speech.voice_hortense").to_string(),
            VoiceProfile::Paul => t!("speech.voice_paul").to_string(),
            VoiceProfile::Sylvie => t!("speech.voice_sylvie").to_string(),
            VoiceProfile::Jean => t!("speech.voice_jean").to_string(),
        };
        self.chat_history.push(ChatEntry {
            role: ChatRole::Agent,
            text: confirmation.to_string(),
            timestamp: current_time_str(),
            quick_suggestions: Vec::new(),
            screen_size_kb: None,
        });
        let _ = self.tts_sender.send(TtsCommand::Speak(confirmation.to_string()));
    }

    pub fn stop_speaking(&mut self) {
        self.is_streaming_narration = false;
        let _ = self.tts_sender.send(TtsCommand::Stop);
        set_tts_audio_level(0.0);
        self.smoothed_audio_level = 0.0;
        if self.status == AgentStatus::Speaking {
            self.status = AgentStatus::Idle;
        }
    }

    pub fn expand_macro(&self, text: &str) -> String {
        expand_macro_text(text, &self.macros)
    }

    fn trigger_emergency_stop(&mut self, _ctx: &egui::Context) {
        self.continuous_mode = false;
        self.dictation_mode = false;
        self.is_streaming_narration = false;
        IS_DICTATION_ACTIVE.store(false, Ordering::SeqCst);
        cancel_all_scheduled_tasks();
        floating_window::stop_live_subtitles();
        crate::audio::stop_live_subtitles();
        self.toggle_switch_access_mode(Some(false));
        clear_screen_numbered_badges();
        self.switch_access_mode = false;
        SWITCH_HOOK_ENABLED.store(false, Ordering::SeqCst);
        automation::reset_switch_keys();
        IS_EMERGENCY_STOPPED.store(true, Ordering::SeqCst);
        TARGET_REASONING_PASSES.store(5, Ordering::SeqCst);
        let _ = CURRENT_REQUEST_ID.fetch_add(1, Ordering::SeqCst);
        if self.is_recording {
            self.stop_recording();
        }
        self.live_transcript.clear();
        self.is_hidden = false;
        self.status = AgentStatus::EmergencyStopped;
        play_earcon(Earcon::Emergency);
        let stop_msg = t!("speech.emergency_stop").to_string();
        self.chat_history.push(ChatEntry {
            role: ChatRole::Agent,
            text: stop_msg.clone(),
            timestamp: current_time_str(),
            quick_suggestions: Vec::new(),
            screen_size_kb: None,
        });
        let _ = self.tts_sender.send(TtsCommand::Stop);
        let _ = self.tts_sender.send(TtsCommand::Speak(stop_msg));
    }

    pub fn start_live_subtitles(&mut self, target_window: Option<String>) {
        crate::audio::start_live_subtitles(target_window.clone());
        if self.is_recording {
            self.stop_recording();
        }
        self.continuous_mode = false;
        let default_title = t!("commands.subtitles_title").to_string();
        let title_name = target_window.clone().unwrap_or(default_title);
        let card = FloatingCard::new(
            "live_subtitles",
            title_name.clone(),
            FloatingContentType::Subtitles {
                text: String::new(),
                target_window: target_window.clone(),
                history: Vec::new(),
            },
        );
        if let Some(existing) = self.floating_cards.iter_mut().find(|c| c.id == "live_subtitles") {
            existing.open = true;
            existing.title = title_name;
        } else {
            self.floating_cards.push(card);
        }
        update_open_floating_cards(&self.floating_cards);
        play_earcon(Earcon::Success);
        let default_source = t!("commands.subtitles_system_audio").to_string();
        let source_desc = target_window.as_deref().unwrap_or(&default_source);
        let msg = t!("speech.subtitles_enabled", source = source_desc).to_string();
        self.chat_history.push(ChatEntry {
            role: ChatRole::Agent,
            text: msg.clone(),
            timestamp: current_time_str(),
            quick_suggestions: vec![QuickSuggestionItem::Text(t!("commands.stop_subtitles").to_string())],
            screen_size_kb: None,
        });
        let _ = self.tts_sender.send(TtsCommand::Stop);
        let _ = self.tts_sender.send(TtsCommand::Speak(msg));
    }

    pub fn stop_live_subtitles(&mut self) {
        floating_window::stop_live_subtitles();
        crate::audio::stop_live_subtitles();
        self.floating_cards.retain(|c| !matches!(c.content, FloatingContentType::Subtitles { .. }));
        update_open_floating_cards(&self.floating_cards);
        play_earcon(Earcon::MicClose);
        let msg = t!("speech.subtitles_disabled").to_string();
        self.chat_history.push(ChatEntry {
            role: ChatRole::Agent,
            text: msg,
            timestamp: current_time_str(),
            quick_suggestions: Vec::new(),
            screen_size_kb: None,
        });
    }

    fn enter_dictation_mode(&mut self) {
        self.dictation_mode = true;
        self.continuous_mode = true;
        IS_DICTATION_ACTIVE.store(true, Ordering::SeqCst);
        play_earcon(Earcon::Success);
        let _ = CURRENT_REQUEST_ID.fetch_add(1, Ordering::SeqCst);
        if self.is_recording {
            self.stop_recording();
        }
        self.live_transcript.clear();
        self.status = AgentStatus::Speaking;
        let _ = self.tts_sender.send(TtsCommand::Stop);
        restore_external_focus();
        let msg = t!("speech.dictation_enabled").to_string();
        self.chat_history.push(ChatEntry {
            role: ChatRole::Agent,
            text: msg.clone(),
            timestamp: current_time_str(),
            quick_suggestions: vec![
                QuickSuggestionItem::Text(t!("commands.disable_dictation").to_string()),
                QuickSuggestionItem::Text(t!("commands.clear_text").to_string()),
            ],
            screen_size_kb: None,
        });
        let _ = self.tts_sender.send(TtsCommand::Speak(msg));
    }

    fn exit_dictation_mode(&mut self) {
        self.dictation_mode = false;
        self.continuous_mode = false;
        IS_DICTATION_ACTIVE.store(false, Ordering::SeqCst);
        play_earcon(Earcon::Success);
        let _ = CURRENT_REQUEST_ID.fetch_add(1, Ordering::SeqCst);
        if self.is_recording {
            self.stop_recording();
        }
        self.live_transcript.clear();
        self.status = AgentStatus::Idle;
        let _ = self.tts_sender.send(TtsCommand::Stop);
        let msg = t!("speech.dictation_disabled").to_string();
        self.chat_history.push(ChatEntry {
            role: ChatRole::Agent,
            text: msg.clone(),
            timestamp: current_time_str(),
            quick_suggestions: Vec::new(),
            screen_size_kb: None,
        });
        let _ = self.tts_sender.send(TtsCommand::Speak(msg));
    }

    pub fn toggle_switch_access_mode(&mut self, force_state: Option<bool>) {
        mark_hover_nav_asked();
        self.switch_access_mode = force_state.unwrap_or(!self.switch_access_mode);
        SWITCH_HOOK_ENABLED.store(self.switch_access_mode, Ordering::SeqCst);
        automation::reset_switch_keys();
        clear_screen_numbered_badges();

        if self.switch_access_mode {
            self.switch_step = SwitchAccessStep::Windows;
            self.switch_windows = collect_switch_candidate_windows();
            self.switch_auto_scroll = false;
            self.switch_active_zone_index = None;
            self.numbered_targets.clear();
            self.switch_focus_index = 0;
            self.switch_last_tick = Instant::now() - Duration::from_millis(1500);
            self.hover_last_cursor_pos = None;
            self.hover_cursor_still_since = Instant::now();
            self.hover_pending_speech = None;
            self.hover_speech_deadline = None;
            self.hover_last_announced_target = None;
            self.hover_last_announced_zone = None;
            self.hover_controls_speech_enabled = false;
            self.hover_physical_mouse_moved = false;
            play_earcon(Earcon::Success);

            if !self.switch_windows.is_empty() {
                let physical_cursor = get_global_cursor_pos();
                let hovered_idx = physical_cursor.and_then(|(cx, cy)| {
                    find_switch_window_at_point(&self.switch_windows, cx, cy)
                });

                #[cfg(windows)]
                let active_idx = hovered_idx.or_else(|| {
                    let fg = unsafe { windows::Win32::UI::WindowsAndMessaging::GetForegroundWindow() };
                    self.switch_windows.iter().position(|w| w.id == fg.0 as isize)
                });
                #[cfg(not(windows))]
                let active_idx = hovered_idx;

                self.switch_focus_index = active_idx.unwrap_or(0);
                let cur_win = self.switch_windows[self.switch_focus_index].clone();
                self.switch_active_window_id = Some(cur_win.id);
                self.switch_step = SwitchAccessStep::Zones;
                self.switch_focus_index = 0;

                self.activate_window_and_display_first_zone(cur_win.id, &cur_win.title, cur_win.rect, true);
            } else {
                let msg = t!("speech.switch_no_windows").to_string();
                self.chat_history.push(ChatEntry {
                    role: ChatRole::Agent,
                    text: msg.clone(),
                    timestamp: current_time_str(),
                    quick_suggestions: Vec::new(),
                    screen_size_kb: None,
                });
                let _ = self.tts_sender.send(TtsCommand::Speak(msg));
            };
        } else {
            self.numbered_targets.clear();
            self.switch_windows.clear();
            self.switch_zones.clear();
            self.switch_active_zone_index = None;
            self.switch_step = SwitchAccessStep::Windows;
            self.switch_auto_scroll = false;
            self.hover_last_cursor_pos = None;
            self.hover_cursor_still_since = Instant::now();
            self.hover_pending_speech = None;
            self.hover_speech_deadline = None;
            self.hover_last_announced_target = None;
            self.hover_last_announced_zone = None;
            self.hover_controls_speech_enabled = false;
            self.hover_physical_mouse_moved = false;
            self.numbered_input_buffer.clear();
            play_earcon(Earcon::MicClose);
            let msg = t!("speech.switch_access_disabled").to_string();
            self.chat_history.push(ChatEntry {
                role: ChatRole::Agent,
                text: msg.clone(),
                timestamp: current_time_str(),
                quick_suggestions: Vec::new(),
                screen_size_kb: None,
            });
            let _ = self.tts_sender.send(TtsCommand::Speak(msg));
        }
    }

    fn start_recording(&mut self, _ctx: &egui::Context, play_sound: bool) {
        if self.status == AgentStatus::EmergencyStopped {
            return;
        }
        self.stop_speaking();
        IS_EMERGENCY_STOPPED.store(false, Ordering::SeqCst);
        self.continuous_mode = true;
        self.is_recording = true;
        self.status = AgentStatus::Listening;
        if play_sound {
            play_earcon(Earcon::MicOpen);
        }
        self.live_transcript.clear();
        if self.dictation_mode {
            restore_external_focus();
        }
        let _ = self.audio_sender.send(AudioCommand::Start);
    }

    fn stop_recording(&mut self) {
        self.is_recording = false;
        self.continuous_mode = false;
        let _ = self.audio_sender.send(AudioCommand::Stop);
        play_earcon(Earcon::MicClose);
        set_mic_audio_level(0.0);
        self.live_transcript.clear();
        self.status = AgentStatus::Idle;
    }

    fn toggle_recording(&mut self, ctx: &egui::Context) {
        if self.is_recording {
            self.stop_recording(); // This will trigger prompt processing if text is present
        } else {
            self.start_recording(ctx, true);
        }
    }

    pub const BADGES_CHUNK_SIZE: usize = 150;

    pub fn narrate_numbered_target(&mut self, target: &NumberedElementTarget) {
        self.narrate_numbered_target_with_prefix(None, target);
    }

    pub fn narrate_numbered_target_with_prefix(&mut self, prefix: Option<&str>, target: &NumberedElementTarget) {
        #[cfg(windows)]
        {
            if let Some(wid) = target.window_id {
                let hwnd = windows::Win32::Foundation::HWND(wid as _);
                if !hwnd.0.is_null() && !is_window_hung(hwnd) {
                    force_foreground_window(hwnd);
                    restore_if_iconic(hwnd);
                }
            }
            focus_element_target(target);
            set_global_cursor_pos(target.click_x, target.click_y);
        }
        #[cfg(not(windows))]
        {
            focus_element_target(target);
            set_global_cursor_pos(target.click_x, target.click_y);
        }
        self.hover_last_cursor_pos = Some((target.click_x, target.click_y));
        self.hover_cursor_still_since = Instant::now();

        let role_label = match target.role.as_str() {
            "bouton" | "button" => t!("roles.button").to_string(),
            "lien" | "link" => t!("roles.link").to_string(),
            "champ de saisie" | "input" => t!("roles.input").to_string(),
            "zone de texte" | "textarea" => t!("roles.textarea").to_string(),
            "onglet" | "tab" => t!("roles.tab").to_string(),
            "lecture textuelle" => t!("roles.text_reader").to_string(),
            "étiquette" | "label" => t!("roles.label").to_string(),
            "visuel (ocr)" | "ocr" => t!("roles.ocr").to_string(),
            "case à cocher" | "checkbox" => t!("roles.checkbox").to_string(),
            "liste déroulante" | "select" | "combobox" => t!("roles.combobox").to_string(),
            "curseur" | "slider" => t!("roles.slider").to_string(),
            "lecteur multimédia" | "media" => t!("roles.media").to_string(),
            other => other.to_string(),
        };

        let target_desc = if target.name.trim().is_empty() {
            t!("commands.target_desc_role", index = target.index, role = role_label).to_string()
        } else if target.value.trim().is_empty() {
            t!("commands.target_desc_name_role", index = target.index, name = target.name.trim(), role = role_label).to_string()
        } else {
            t!("commands.target_desc_full", index = target.index, name = target.name.trim(), role = role_label, value = target.value.trim()).to_string()
        };

        let speech = if let Some(p) = prefix {
            format!("{p} {target_desc}")
        } else {
            target_desc
        };

        let _ = self.tts_sender.send(TtsCommand::Stop);
        let _ = self.tts_sender.send(TtsCommand::Speak(speech));
    }

    pub fn click_numbered_target_by_index(&mut self, idx: usize) {
        self.stop_speaking();

        // 1. Chercher en priorité absolue la cible directement parmi les cibles affichées
        let mut found_target = self.numbered_targets.iter().find(|t| t.index == idx).cloned();

        // 2. Si non trouvée dans la zone active courante, chercher dans l'ensemble des zones de la fenêtre
        if found_target.is_none() {
            for (z_idx, zone) in self.switch_zones.iter().enumerate() {
                if let Some(t) = zone.targets.iter().find(|t| t.index == idx) {
                    found_target = Some(t.clone());
                    self.switch_active_zone_index = Some(z_idx);
                    break;
                }
            }
        }

        // 3. Repli : si AUCUNE cible ne porte ce numéro et qu'on est en sélection de zone,
        // alors seulement le numéro sert à basculer vers la zone correspondante
        if found_target.is_none() && self.switch_access_mode && self.switch_step == SwitchAccessStep::Zones {
            if idx >= 1 && idx <= self.switch_zones.len() {
                self.select_switch_zone(idx - 1);
                return;
            }
        }

        if let Some(target) = found_target {
            if self.switch_step == SwitchAccessStep::Zones {
                self.switch_step = SwitchAccessStep::Elements;
            }
            if target.role == "fenêtre" {
                if let Some(wid) = target.window_id {
                    self.switch_active_window_id = Some(wid);
                    self.transition_to_zones_for_window(wid, &target.name);
                    return;
                }
            }

            #[cfg(windows)]
            {
                clear_screen_numbered_badges();
                if let Some(wid) = target.window_id {
                    let hwnd = windows::Win32::Foundation::HWND(wid as _);
                    if !hwnd.0.is_null() && !is_window_hung(hwnd) {
                        force_foreground_window(hwnd);
                        restore_if_iconic(hwnd);
                    }
                }
                let pt = windows::Win32::Foundation::POINT { x: target.click_x, y: target.click_y };
                unsafe {
                    let target_child = windows::Win32::UI::WindowsAndMessaging::WindowFromPoint(pt);
                    if !target_child.is_invalid() {
                        let root = windows::Win32::UI::WindowsAndMessaging::GetAncestor(
                            target_child,
                            windows::Win32::UI::WindowsAndMessaging::GA_ROOT,
                        );
                        let top_hwnd = if !root.is_invalid() { root } else { target_child };
                        force_foreground_window(top_hwnd);
                        restore_if_iconic(top_hwnd);
                    }
                }
                let mut uia_elem = None;
                let _ = unsafe { windows::Win32::System::Com::CoInitializeEx(None, windows::Win32::System::Com::COINIT_MULTITHREADED) };
                if let Ok(automation) = unsafe {
                    windows::Win32::System::Com::CoCreateInstance::<_, windows::Win32::UI::Accessibility::IUIAutomation>(
                        &windows::Win32::UI::Accessibility::CUIAutomation,
                        None,
                        windows::Win32::System::Com::CLSCTX_INPROC_SERVER,
                    )
                } {
                    if let Ok(elem) = unsafe { automation.ElementFromPoint(pt) } {
                        uia_elem = Some(elem);
                    }
                }
                ui::click_element(target.click_x, target.click_y, uia_elem.as_ref(), None);
            }
            #[cfg(not(windows))]
            {
                clear_screen_numbered_badges();
                ui::click_element(target.click_x, target.click_y, None, None);
            }
            if target.role == "champ de saisie" || target.role == "zone de texte" {
                self.dictation_mode = true;
                IS_DICTATION_ACTIVE.store(true, Ordering::SeqCst);
                self.continuous_mode = true;
                if let Ok(mut guard) = LAST_FOCUSED_RECT.lock() {
                    *guard = Some(target.rect);
                }
            }
            play_earcon(Earcon::Success);
            let target_desc = if !target.name.is_empty() {
                t!("commands.click_target_named", index = idx, name = target.name).to_string()
            } else {
                t!("commands.click_target_num", index = idx).to_string()
            };
            self.chat_history.push(ChatEntry {
                role: ChatRole::Agent,
                text: target_desc.clone(),
                timestamp: current_time_str(),
                quick_suggestions: vec![QuickSuggestionItem::Text(t!("commands.close_badges").to_string())],
                screen_size_kb: None,
            });
            let _ = self.tts_sender.send(TtsCommand::Stop);
            let _ = self.tts_sender.send(TtsCommand::Speak(target_desc));

            // Laisser le temps à l'application cible de réagir au clic (navigation, menu, etc.)
            std::thread::sleep(Duration::from_millis(250));
            self.switch_focus_index = 0;

            // Régénération automatique des numéros/badges après le clic pour enchaîner
            if self.switch_access_mode {
                #[cfg(windows)]
                {
                    let fg = unsafe { windows::Win32::UI::WindowsAndMessaging::GetForegroundWindow() };
                    let target_hwnd = if !fg.0.is_null() && !is_window_hung(fg) {
                        fg.0 as isize
                    } else {
                        target.window_id.unwrap_or(0)
                    };
                    if target_hwnd != 0 {
                        self.refresh_numbered_targets_for_window(target_hwnd);
                    }
                }
                #[cfg(not(windows))]
                {
                    self.refresh_numbered_targets_for_window(0);
                }
            }
        } else {
            let max_idx = self.numbered_targets.iter().map(|t| t.index).max()
                .unwrap_or(0);
            play_earcon(Earcon::Error);
            let msg = if max_idx > 0 {
                t!("commands.target_not_found_range", index = idx, max = max_idx).to_string()
            } else {
                t!("commands.target_not_found_none", index = idx).to_string()
            };
            self.chat_history.push(ChatEntry {
                role: ChatRole::Agent,
                text: msg.clone(),
                timestamp: current_time_str(),
                quick_suggestions: vec![QuickSuggestionItem::Text(t!("commands.close_badges").to_string())],
                screen_size_kb: None,
            });
            let _ = self.tts_sender.send(TtsCommand::Stop);
            let _ = self.tts_sender.send(TtsCommand::Speak(msg));
        }
    }

    /// Transitionne de l'étape fenêtre vers l'étape des Chunks de badges / Zones
    pub fn transition_to_zones_for_window(&mut self, window_id: isize, win_title: &str) {
        self.switch_active_window_id = Some(window_id);
        self.switch_step = SwitchAccessStep::Zones;
        self.switch_focus_index = 0;
        self.switch_auto_scroll = false;
        self.switch_last_tick = Instant::now() + Duration::from_millis(1500);
        clear_screen_numbered_badges();

        #[cfg(windows)]
        let target_hwnd = windows::Win32::Foundation::HWND(window_id as _);
        #[cfg(windows)]
        {
            if !target_hwnd.0.is_null() && !is_window_hung(target_hwnd) {
                force_foreground_window(target_hwnd);
                restore_if_iconic(target_hwnd);
            }
        }

        if let Some(cached) = self.window_analysis_cache.get(&window_id).cloned() {
            self.switch_zones = cached.zones;
            self.display_zones_overlay(&cached.summary);
        } else {
            #[cfg(windows)]
            let raw_targets = collect_numbered_targets_for_window(target_hwnd);
            #[cfg(not(windows))]
            let raw_targets = collect_numbered_targets_for_window(win_title);

            let (summary, fallback_zones) = build_fallback_navigation_zones(win_title, &raw_targets);
            self.window_analysis_cache.insert(window_id, CachedWindowAnalysis {
                summary: summary.clone(),
                zones: fallback_zones.clone(),
            });
            self.switch_zones = fallback_zones;
            self.display_zones_overlay(&summary);

            let _ = self.command_sender.send(AgentCommand::AnalyzeNavigationZones {
                window_id,
                window_title: win_title.to_string(),
                targets: raw_targets,
                context_meta: None,
            });
        }
    }

    pub fn display_zones_overlay(&mut self, speech_text: &str) {
        let zone_infos: Vec<ZoneOverlayInfo> = self.switch_zones.iter().enumerate().map(|(i, z)| {
            ZoneOverlayInfo {
                index: i + 1,
                title: z.title.clone(),
                rect: z.rect,
                is_focused: (self.switch_focus_index % self.switch_zones.len().max(1)) == i,
            }
        }).collect();

        // N'injecter que les badges jaunes de la zone active/focalisée
        if let Some(cur_zone) = self.switch_zones.get(self.switch_focus_index % self.switch_zones.len().max(1)) {
            let mut active_targets = cur_zone.targets.clone();
            for (i, t) in active_targets.iter_mut().enumerate() {
                t.index = i + 1;
            }
            self.numbered_targets = active_targets;
        } else {
            self.numbered_targets.clear();
        }

        let focused_target_idx = self.numbered_targets.first().map(|t| t.index);
        draw_screen_zones_and_targets(&zone_infos, &self.numbered_targets, self.zoom_factor, focused_target_idx);

        self.chat_history.push(ChatEntry {
            role: ChatRole::Agent,
            text: speech_text.to_string(),
            timestamp: current_time_str(),
            quick_suggestions: Vec::new(),
            screen_size_kb: None,
        });
        let _ = self.tts_sender.send(TtsCommand::Stop);
        let _ = self.tts_sender.send(TtsCommand::Speak(speech_text.to_string()));
    }

    /// Active une fenêtre et affiche immédiatement la première zone bleue identifiée ainsi que ses points jaunes
    pub fn activate_window_and_display_first_zone(
        &mut self,
        window_id: isize,
        win_title: &str,
        win_rect: [i32; 4],
        speak_summary: bool,
    ) {
        self.switch_active_window_id = Some(window_id);

        #[cfg(windows)]
        {
            let target_hwnd = windows::Win32::Foundation::HWND(window_id as _);
            force_foreground_window(target_hwnd);
            restore_if_iconic(target_hwnd);
            draw_screen_orange_border(win_rect);
        }
        #[cfg(not(windows))]
        {
            force_foreground_window(win_title);
            restore_if_iconic(win_title);
        }

        let (summary, zones) = if let Some(cached) = self.window_analysis_cache.get(&window_id).cloned() {
            (cached.summary, cached.zones)
        } else {
            #[cfg(windows)]
            let raw_targets = collect_numbered_targets_for_window(windows::Win32::Foundation::HWND(window_id as _));
            #[cfg(not(windows))]
            let raw_targets = collect_numbered_targets_for_window(win_title);

            let (sum, fb_zones) = build_fallback_navigation_zones(win_title, &raw_targets);
            self.window_analysis_cache.insert(window_id, CachedWindowAnalysis {
                summary: sum.clone(),
                zones: fb_zones.clone(),
            });

            if !raw_targets.is_empty() {
                #[cfg(windows)]
                let context_meta = extract_window_context_meta(windows::Win32::Foundation::HWND(window_id as _), win_title);
                #[cfg(not(windows))]
                let context_meta = None;

                let _ = self.command_sender.send(AgentCommand::AnalyzeNavigationZones {
                    window_id,
                    window_title: win_title.to_string(),
                    targets: raw_targets,
                    context_meta,
                });
            }
            (sum, fb_zones)
        };

        self.switch_zones = zones;
        if let Some(first_zone) = self.switch_zones.first() {
            let mut active_targets = first_zone.targets.clone();
            for (i, t) in active_targets.iter_mut().enumerate() {
                t.index = i + 1;
            }
            self.numbered_targets = active_targets;
        } else {
            self.numbered_targets.clear();
        }

        let zone_infos: Vec<ZoneOverlayInfo> = self.switch_zones.iter().enumerate().map(|(i, z)| {
            ZoneOverlayInfo {
                index: i + 1,
                title: z.title.clone(),
                rect: z.rect,
                is_focused: i == 0,
            }
        }).collect();

        let focused_target_idx = self.numbered_targets.first().map(|t| t.index);
        draw_screen_zones_and_targets(&zone_infos, &self.numbered_targets, self.zoom_factor, focused_target_idx);

        if speak_summary {
            let speech = if !summary.is_empty() { summary } else { win_title.to_string() };
            let _ = self.tts_sender.send(TtsCommand::Stop);
            let _ = self.tts_sender.send(TtsCommand::Speak(speech));
        }
    }

    pub fn refresh_numbered_targets_for_window(&mut self, hwnd_id: isize) {
        #[cfg(windows)]
        {
            let hwnd = windows::Win32::Foundation::HWND(hwnd_id as _);
            if hwnd.0.is_null() || is_window_hung(hwnd) {
                return;
            }

            let mut r = windows::Win32::Foundation::RECT::default();
            let has_rect = unsafe { windows::Win32::UI::WindowsAndMessaging::GetWindowRect(hwnd, &mut r).is_ok() };
            let cur_rect = if has_rect { Some([r.left, r.top, r.right, r.bottom]) } else { None };

            self.numbered_last_refresh = Instant::now();
            self.numbered_last_hwnd = Some(hwnd_id);
            self.numbered_last_win_rect = cur_rect;
            self.switch_active_window_id = Some(hwnd_id);
            self.numbered_input_buffer.clear();

            self.scan_sequence_counter += 1;
            let req = TargetsScanRequest {
                hwnd: hwnd_id,
                win_rect: cur_rect,
                previous: self.numbered_targets.clone(),
                sequence: self.scan_sequence_counter,
            };
            self.is_scan_in_progress = true;
            let _ = self.targets_request_tx.send(req);
        }
        #[cfg(not(windows))]
        {
            self.numbered_last_refresh = Instant::now();
            self.switch_active_window_id = Some(hwnd_id);
            self.numbered_input_buffer.clear();
            self.scan_sequence_counter += 1;
            let req = TargetsScanRequest {
                hwnd: hwnd_id,
                win_rect: None,
                previous: self.numbered_targets.clone(),
                sequence: self.scan_sequence_counter,
            };
            self.is_scan_in_progress = true;
            let _ = self.targets_request_tx.send(req);
        }
    }

    /// Sélectionne un Chunk / Zone et bascule vers ses contrôles et éléments OCR individuels
    pub fn select_switch_zone(&mut self, zone_idx: usize) {
        if zone_idx >= self.switch_zones.len() {
            return;
        }
        self.last_switch_input = Instant::now();
        self.switch_auto_scroll = false;
        self.hover_physical_mouse_moved = false;
        play_earcon(Earcon::Success);

        let cur_zone = self.switch_zones[zone_idx].clone();
        self.numbered_targets = cur_zone.targets.clone();
        self.switch_active_zone_index = Some(zone_idx);
        self.switch_step = SwitchAccessStep::Elements;
        self.switch_focus_index = 0;
        self.hover_controls_speech_enabled = true;

        // Réaffecter des indices séquentiels pour ce chunk
        for (i, t) in self.numbered_targets.iter_mut().enumerate() {
            t.index = i + 1;
        }

        if let Some(first_target) = self.numbered_targets.first() {
            focus_element_target(first_target);
            self.hover_last_cursor_pos = Some((first_target.click_x, first_target.click_y));
            self.hover_cursor_still_since = Instant::now();
            self.hover_last_announced_target = Some(first_target.index);
        }

        self.switch_last_tick = Instant::now() + Duration::from_millis(1500);
        let vocal_msg = format!("Zone {} sélectionnée. {}", cur_zone.title, format_plural_control_type(&self.numbered_targets));
        let _ = self.tts_sender.send(TtsCommand::Stop);
        let _ = self.tts_sender.send(TtsCommand::Speak(vocal_msg));

        // Maintenir l'encadré bleu et son badge de titre autour de la zone active
        let zone_info = vec![ZoneOverlayInfo {
            index: zone_idx + 1,
            title: cur_zone.title.clone(),
            rect: cur_zone.rect,
            is_focused: true,
        }];
        draw_screen_zones_and_targets(&zone_info, &self.numbered_targets, self.zoom_factor, Some(1));
        #[cfg(windows)]
        draw_screen_control_border(cur_zone.rect, true);
    }
}

impl eframe::App for OverlayApp {
    fn clear_color(&self, _visuals: &egui::Visuals) -> [f32; 4] {
        if self.is_hidden {
            [0.0, 0.0, 0.0, 0.0]
        } else {
            self.theme.colors().clear_color
        }
    }

    fn ui(&mut self, ui: &mut egui::Ui, _frame: &mut eframe::Frame) {
        let ctx = ui.ctx().clone();

        ctx.set_zoom_factor(self.zoom_factor);

        update_last_external_focus();

        self.dictation_mode = IS_DICTATION_ACTIVE.load(Ordering::SeqCst);

        let target_level = match self.status {
            AgentStatus::Speaking => get_tts_audio_level(),
            AgentStatus::Listening => get_mic_audio_level(),
            _ => 0.0,
        };

        if self.status == AgentStatus::Speaking || self.status == AgentStatus::Listening {
            if target_level > self.smoothed_audio_level {
                self.smoothed_audio_level += (target_level - self.smoothed_audio_level) * 0.65;
            } else {
                self.smoothed_audio_level += (target_level - self.smoothed_audio_level) * 0.22;
            }
        } else {
            self.smoothed_audio_level += (0.0 - self.smoothed_audio_level) * 0.25;
        }

        let theme = self.theme.colors();

        let mut visuals = egui::Visuals::dark();
        visuals.panel_fill = egui::Color32::TRANSPARENT;
        visuals.window_fill = egui::Color32::TRANSPARENT;
        visuals.extreme_bg_color = theme.control_bg;
        visuals.selection.bg_fill = theme.tab_active_bg;
        visuals.selection.stroke = egui::Stroke::new(1.0, theme.accent);
        visuals.widgets.active.bg_fill = theme.tab_active_bg;
        visuals.widgets.active.weak_bg_fill = theme.tab_active_bg;
        visuals.widgets.hovered.weak_bg_fill = theme.tab_inactive_bg;
        visuals.widgets.inactive.weak_bg_fill = theme.tab_inactive_bg;
        ctx.set_visuals(visuals);

        let ppp = ctx.pixels_per_point().max(0.5);
        #[cfg(windows)]
        {
            // Gestion ergonomique des boutons auxiliaires trackball (VK_XBUTTON1, VK_XBUTTON2, VK_MBUTTON)
            if self.last_trackball_input.elapsed() >= Duration::from_millis(250) {
                let xbutton1 = unsafe { (GetAsyncKeyState(0x05) as u16 & 0x8000) != 0 }; // Trackball Bouton 4
                let xbutton2 = unsafe { (GetAsyncKeyState(0x06) as u16 & 0x8000) != 0 }; // Trackball Bouton 5
                let mbutton  = unsafe { (GetAsyncKeyState(0x04) as u16 & 0x8000) != 0 }; // Molette / Clic central

                if mbutton && self.status == AgentStatus::Speaking {
                    self.last_trackball_input = Instant::now();
                    self.stop_speaking();
                    play_earcon(Earcon::Success);
                } else if xbutton1 {
                    self.last_trackball_input = Instant::now();
                    if self.dictation_mode {
                        self.exit_dictation_mode();
                    } else {
                        self.enter_dictation_mode();
                    }
                    ctx.request_repaint();
                } else if xbutton2 {
                    self.last_trackball_input = Instant::now();
                    self.toggle_switch_access_mode(None);
                    ctx.request_repaint();
                }
            }
        }

        let trimmed_input = self.input_text.trim();
        if trimmed_input != self.last_suggestion_query {
            self.last_suggestion_query = trimmed_input.to_string();
            self.input_suggestions = if trimmed_input.len() >= 2 {
                find_matching_applications(trimmed_input, 5)
            } else {
                Vec::new()
            };
        }

        let has_suggestions = !self.input_suggestions.is_empty();
        let is_maximized = ctx.input(|i| i.viewport().maximized.unwrap_or(false));
        let work_area = get_desktop_work_area_scaled(&ctx);
        let overlay_layout = compute_and_sync_overlay_geometry(
            &ctx,
            &work_area,
            &mut self.position_initialized,
            &mut self.window_pos,
            &mut self.window_bottom_y,
            &mut self.current_win_w,
            &mut self.current_win_h,
            self.chat_height,
            has_suggestions,
            self.show_api_key_modal,
            is_maximized,
        );
        let win_w = overlay_layout.win_w;
        let active_win_h = overlay_layout.active_win_h;
        let control_rect = overlay_layout.control_rect;
        let response_rect = overlay_layout.response_rect;
        let min_chat_h = overlay_layout.min_chat_h;
        let max_chat_h = overlay_layout.max_chat_h;
        let padding = overlay_layout.padding;
        let current_chat_h = self.chat_height.clamp(min_chat_h, max_chat_h);

        let should_passthrough = self.is_hidden;
        if should_passthrough != self.mouse_passthrough {
            self.mouse_passthrough = should_passthrough;
            ctx.send_viewport_cmd(egui::ViewportCommand::MousePassthrough(should_passthrough));
        }

        // Dépouillement des événements asynchrones
        while let Ok(event) = self.event_receiver.try_recv() {
            match event {
                AgentEvent::DownloadProgress { percent, downloaded_mb, total_mb, speed_mb_s } => {
                    if percent >= 100.0 {
                        self.local_download_progress = None;
                    } else {
                        self.local_download_progress = Some((percent, downloaded_mb, total_mb, speed_mb_s));
                    }
                    ctx.request_repaint();
                }
                AgentEvent::StatusChanged(new_status) => {
                    if self.status != AgentStatus::EmergencyStopped || new_status == AgentStatus::EmergencyStopped {
                        let prev_status = self.status;
                        self.status = new_status;
                        if new_status == AgentStatus::Listening {
                            self.is_recording = true;
                        } else if new_status == AgentStatus::Idle {
                            self.is_recording = false;
                            if (self.dictation_mode
                                || (prev_status == AgentStatus::Thinking && self.continuous_mode)
                                || (prev_status == AgentStatus::Speaking && self.continuous_mode))
                                && !crate::audio::is_live_subtitles_active()
                                && self.whisper_ready
                                && !self.show_api_key_modal
                                && self.status != AgentStatus::EmergencyStopped
                                && !self.is_hidden
                                && !AGENT_BUSY.load(Ordering::SeqCst)
                            {
                                self.start_recording(&ctx, false);
                            }
                        }
                    }
                }
                AgentEvent::ScreenPayloadSize(kb) => {
                    if let Some(entry) = self.chat_history.iter_mut().rev().find(|e| e.role == ChatRole::User) {
                        entry.screen_size_kb = Some(kb);
                    }
                }
                AgentEvent::NarrationStart => {
                    self.is_streaming_narration = true;
                    self.chat_history.push(ChatEntry {
                        role: ChatRole::Agent,
                        text: String::new(),
                        timestamp: current_time_str(),
                        quick_suggestions: Vec::new(),
                        screen_size_kb: None,
                    });
                }
                AgentEvent::NarrationChunk(chunk) => {
                    if self.is_streaming_narration {
                        if let Some(last) = self.chat_history.last_mut() {
                            if matches!(last.role, ChatRole::Agent) {
                                last.text.push_str(&chunk);
                            }
                        }
                    } else if let Some(last) = self.chat_history.last_mut() {
                        if matches!(last.role, ChatRole::Agent) {
                            last.text.push_str(&chunk);
                        } else {
                            self.chat_history.push(ChatEntry {
                                role: ChatRole::Agent,
                                text: chunk,
                                timestamp: current_time_str(),
                                quick_suggestions: Vec::new(),
                                screen_size_kb: None,
                            });
                        }
                    } else {
                        self.chat_history.push(ChatEntry {
                            role: ChatRole::Agent,
                            text: chunk,
                            timestamp: current_time_str(),
                            quick_suggestions: Vec::new(),
                            screen_size_kb: None,
                        });
                    }
                }
                AgentEvent::ReplaceNarration { text, quick_suggestions } => {
                    let was_streaming = self.is_streaming_narration;
                    self.is_streaming_narration = false;

                    if text.trim().is_empty() && quick_suggestions.is_empty() {
                        continue;
                    }
                    if text.trim().is_empty() {
                        if let Some(last) = self.chat_history.last_mut().filter(|e| e.role == ChatRole::Agent) {
                            for sug in quick_suggestions {
                                if !last.quick_suggestions.iter().any(|s| s.as_str() == sug.as_str()) {
                                    last.quick_suggestions.push(sug);
                                }
                            }
                        }
                    } else if was_streaming || self.llm_config.provider == LlmProvider::Local {
                        if let Some(last) = self.chat_history.last_mut().filter(|e| e.role == ChatRole::Agent) {
                            last.text = text;
                            last.quick_suggestions = quick_suggestions;
                        } else {
                            self.chat_history.push(ChatEntry {
                                role: ChatRole::Agent,
                                text,
                                timestamp: current_time_str(),
                                quick_suggestions,
                                screen_size_kb: None,
                            });
                        }
                    } else {
                        if !sanitize_for_tts(&text).trim().is_empty() {
                            self.status = AgentStatus::Speaking;
                            let _ = self.tts_sender.send(TtsCommand::Speak(text.clone()));
                        }
                        self.chat_history.push(ChatEntry {
                            role: ChatRole::Agent,
                            text,
                            timestamp: current_time_str(),
                            quick_suggestions,
                            screen_size_kb: None,
                        });
                    }
                }
                AgentEvent::SilentNarration(full_text) => {
                    self.chat_history.push(ChatEntry {
                        role: ChatRole::Agent,
                        text: full_text,
                        timestamp: current_time_str(),
                        quick_suggestions: Vec::new(),
                        screen_size_kb: None,
                    });
                }
                AgentEvent::TranscriptionPartial(text) => {
                    let is_status_msg = text == "Écoute de votre voix..." || text == "Transcription locale en cours...";
                    if !is_status_msg && !text.trim().is_empty() && !self.switch_access_mode {
                        self.stop_speaking();
                    }
                    if self.is_recording && !self.is_hidden {
                        self.live_transcript = text;
                    }
                }
                AgentEvent::VoicePromptReady(prompt) => {
                    self.is_recording = false;
                    self.live_transcript.clear();
                    let is_user_typing = ctx.memory(|m| m.has_focus(egui::Id::new("prompt_input_text")))
                        && !self.input_text.trim().is_empty();
                    if is_user_typing {
                        // L'utilisateur est activement en train de taper au clavier : on préserve son texte
                        if self.continuous_mode && self.status != AgentStatus::EmergencyStopped {
                            self.start_recording(&ctx, false);
                        }
                        return;
                    }
                    self.stop_speaking();

                    let prompt = self.expand_macro(&prompt);
                    if prompt != prompt {
                        println!("[Macro] Déploiement vocal : \"{}\"", prompt);
                    }

                    let norm_prompt = normalize_for_command_match(&prompt);
                    if norm_prompt.trim().is_empty() {
                        println!("[Voice] Bruit parasite ignoré (aucun mot détecté) : aucune requête LLM.");
                        if self.continuous_mode && self.status != AgentStatus::EmergencyStopped {
                            self.start_recording(&ctx, false);
                        }
                        return;
                    }

                    // Changement de langue à la voix
                    let target_lang = if norm_prompt.contains("passer en anglais") || norm_prompt.contains("switch to english") || norm_prompt.contains("langue anglaise") || norm_prompt.contains("in english") {
                        Some("en")
                    } else if norm_prompt.contains("passer en francais") || norm_prompt.contains("passer en français")
                        || norm_prompt.contains("switch to french") || norm_prompt.contains("langue francaise") || norm_prompt.contains("langue française") || norm_prompt.contains("en francais") || norm_prompt.contains("en français") {
                        Some("fr")
                    } else if norm_prompt.contains("passer en allemand") || norm_prompt.contains("switch to german") || norm_prompt.contains("auf deutsch") || norm_prompt.contains("langue allemande") || norm_prompt.contains("in german") {
                        Some("de")
                    } else if norm_prompt.contains("passer en espagnol") || norm_prompt.contains("switch to spanish") || norm_prompt.contains("en español") || norm_prompt.contains("en espanol") || norm_prompt.contains("langue espagnole") || norm_prompt.contains("in spanish") {
                        Some("es")
                    } else if norm_prompt.contains("passer en italien") || norm_prompt.contains("switch to italian") || norm_prompt.contains("in italiano") || norm_prompt.contains("langue italienne") {
                        Some("it")
                    } else if norm_prompt.contains("passer en portugais") || norm_prompt.contains("switch to portuguese") || norm_prompt.contains("em português") || norm_prompt.contains("em portugues") || norm_prompt.contains("langue portugaise") {
                        Some("pt")
                    } else if norm_prompt.contains("passer en russe") || norm_prompt.contains("switch to russian") || norm_prompt.contains("по-русски") || norm_prompt.contains("langue russe") {
                        Some("ru")
                    } else if norm_prompt.contains("passer en japonais") || norm_prompt.contains("switch to japanese") || norm_prompt.contains("日本語") || norm_prompt.contains("langue japonaise") {
                        Some("ja")
                    } else if norm_prompt.contains("passer en chinois") || norm_prompt.contains("switch to chinese") || norm_prompt.contains("中文") || norm_prompt.contains("langue chinoise") {
                        Some("zh")
                    } else {
                        None
                    };

                    if let Some(target_code) = target_lang {
                        play_earcon(Earcon::Success);
                        self.set_app_language(target_code);
                        if self.continuous_mode && self.status != AgentStatus::EmergencyStopped {
                            self.start_recording(&ctx, false);
                        }
                        return;
                    }

                    // Changement de thème d'accessibilité à la voix
                    if norm_prompt.contains("theme suivant") || norm_prompt.contains("thème suivant") || norm_prompt.contains("change de theme") || norm_prompt.contains("change de thème") || norm_prompt.contains("changer de theme") || norm_prompt.contains("changer de thème") {
                        play_earcon(Earcon::Success);
                        self.set_theme(self.theme.next());
                        if self.continuous_mode && self.status != AgentStatus::EmergencyStopped {
                            self.start_recording(&ctx, false);
                        }
                        return;
                    }
                    if norm_prompt.contains("contraste eleve") || norm_prompt.contains("contraste élevé") || norm_prompt.contains("high contrast") {
                        play_earcon(Earcon::Success);
                        self.set_theme(AppTheme::HighContrastYellow);
                        if self.continuous_mode && self.status != AgentStatus::EmergencyStopped {
                            self.start_recording(&ctx, false);
                        }
                        return;
                    }
                    if norm_prompt.contains("theme sombre") || norm_prompt.contains("thème sombre") || norm_prompt.contains("theme par defaut") || norm_prompt.contains("thème par défaut") {
                        play_earcon(Earcon::Success);
                        self.set_theme(AppTheme::CyberCyan);
                        if self.continuous_mode && self.status != AgentStatus::EmergencyStopped {
                            self.start_recording(&ctx, false);
                        }
                        return;
                    }

                    // Changement de voix à la voix
                    if norm_prompt.contains("voix suivante") || norm_prompt.contains("autre voix") || norm_prompt.contains("change de voix") || norm_prompt.contains("changer voix") || norm_prompt.contains("changer de voix") {
                        play_earcon(Earcon::Success);
                        self.set_voice_gender(self.voice_gender.next());
                        if self.continuous_mode && self.status != AgentStatus::EmergencyStopped {
                            self.start_recording(&ctx, false);
                        }
                        return;
                    }
                    if norm_prompt.contains("voix de denise") || norm_prompt == "denise" || norm_prompt.contains("voix feminine") || norm_prompt.contains("voix féminine") {
                        play_earcon(Earcon::Success);
                        self.set_voice_gender(VoiceProfile::Denise);
                        if self.continuous_mode && self.status != AgentStatus::EmergencyStopped {
                            self.start_recording(&ctx, false);
                        }
                        return;
                    }
                    if norm_prompt.contains("voix d henri") || norm_prompt.contains("voix d'henri") || norm_prompt == "henri" || norm_prompt.contains("voix masculine") {
                        play_earcon(Earcon::Success);
                        self.set_voice_gender(VoiceProfile::Henri);
                        if self.continuous_mode && self.status != AgentStatus::EmergencyStopped {
                            self.start_recording(&ctx, false);
                        }
                        return;
                    }
                    if norm_prompt.contains("voix d hortense") || norm_prompt.contains("voix d'hortense") || norm_prompt.contains("voix de hortense") || norm_prompt == "hortense" {
                        play_earcon(Earcon::Success);
                        self.set_voice_gender(VoiceProfile::Hortense);
                        if self.continuous_mode && self.status != AgentStatus::EmergencyStopped {
                            self.start_recording(&ctx, false);
                        }
                        return;
                    }
                    if norm_prompt.contains("voix de paul") || norm_prompt == "paul" {
                        play_earcon(Earcon::Success);
                        self.set_voice_gender(VoiceProfile::Paul);
                        if self.continuous_mode && self.status != AgentStatus::EmergencyStopped {
                            self.start_recording(&ctx, false);
                        }
                        return;
                    }
                    if norm_prompt.contains("voix de sylvie") || norm_prompt == "sylvie" || norm_prompt.contains("voix quebecoise") || norm_prompt.contains("voix québécoise") || norm_prompt.contains("voix canadienne") {
                        play_earcon(Earcon::Success);
                        self.set_voice_gender(VoiceProfile::Sylvie);
                        if self.continuous_mode && self.status != AgentStatus::EmergencyStopped {
                            self.start_recording(&ctx, false);
                        }
                        return;
                    }
                    if norm_prompt.contains("voix de jean") || norm_prompt == "jean" {
                        play_earcon(Earcon::Success);
                        self.set_voice_gender(VoiceProfile::Jean);
                        if self.continuous_mode && self.status != AgentStatus::EmergencyStopped {
                            self.start_recording(&ctx, false);
                        }
                        return;
                    }

                    // Traitement du zoom adaptatif vocal
                    if norm_prompt.contains("grossis") || norm_prompt.contains("zoom plus") || norm_prompt.contains("agrandis") || norm_prompt.contains("zoom in")
                        || norm_prompt.contains("police plus grande") || norm_prompt.contains("texte plus grand") || norm_prompt.contains("agrandir la police") || norm_prompt.contains("agrandir le texte") {
                        self.set_zoom_factor(self.zoom_factor + 0.15);
                        play_earcon(Earcon::Success);
                        if self.continuous_mode && self.status != AgentStatus::EmergencyStopped {
                            self.start_recording(&ctx, false);
                        }
                        return;
                    }
                    if norm_prompt.contains("zoom moins") || norm_prompt.contains("retrecis") || norm_prompt.contains("rétrécis") || norm_prompt.contains("zoom out")
                        || norm_prompt.contains("police plus petite") || norm_prompt.contains("texte plus petit") || norm_prompt.contains("diminuer la police") || norm_prompt.contains("diminuer le texte") {
                        self.set_zoom_factor(self.zoom_factor - 0.15);
                        play_earcon(Earcon::Success);
                        if self.continuous_mode && self.status != AgentStatus::EmergencyStopped {
                            self.start_recording(&ctx, false);
                        }
                        return;
                    }
                    if norm_prompt.contains("zoom normal") || norm_prompt.contains("taille normale") || norm_prompt.contains("reset zoom")
                        || norm_prompt.contains("police normale") || norm_prompt.contains("texte normal") || norm_prompt.contains("reinitialiser police") || norm_prompt.contains("réinitialiser police") {
                        self.set_zoom_factor(1.0);
                        play_earcon(Earcon::Success);
                        if self.continuous_mode && self.status != AgentStatus::EmergencyStopped {
                            self.start_recording(&ctx, false);
                        }
                        return;
                    }

                    // Bascule du mode survol/numéros est gérée par is_switch_access_command

                    // Sélection directe d'un badge numéroté
                    if self.switch_access_mode {
                        let parsed_idx = extract_badge_number(&norm_prompt);
                        if let Some(idx) = parsed_idx {
                            self.chat_history.push(ChatEntry {
                                role: ChatRole::User,
                                text: prompt,
                                timestamp: current_time_str(),
                                quick_suggestions: Vec::new(),
                                screen_size_kb: None,
                            });
                            self.click_numbered_target_by_index(idx);
                            if self.continuous_mode && self.status != AgentStatus::EmergencyStopped {
                                self.start_recording(&ctx, false);
                            }
                            return;
                        }
                    }

                    // Raccourci vocal pour faire lire la fenêtre volante active
                    if let Some(card) = is_read_card_intent(&norm_prompt, &self.floating_cards) {
                            let speech = card.build_narrator_speech();
                            self.stop_speaking();
                            play_earcon(Earcon::Success);
                            self.status = AgentStatus::Speaking;
                            self.chat_history.push(ChatEntry {
                                role: ChatRole::User,
                                text: prompt,
                                timestamp: current_time_str(),
                                quick_suggestions: Vec::new(),
                                screen_size_kb: None,
                            });
                            self.chat_history.push(ChatEntry {
                                role: ChatRole::Agent,
                                text: speech.clone(),
                                timestamp: current_time_str(),
                                quick_suggestions: Vec::new(),
                                screen_size_kb: None,
                            });
                            let _ = self.tts_sender.send(TtsCommand::Speak(speech));
                            if self.continuous_mode && self.status != AgentStatus::EmergencyStopped {
                                self.start_recording(&ctx, false);
                            }
                            return;
                    }

                    let (locale, _) = get_system_locale_language();
                    let lang_prefix = locale.split(['-', '_']).next().unwrap_or("fr");

                    if is_switch_access_command(&prompt) {
                        mark_hover_nav_asked();
                        self.toggle_switch_access_mode(None);
                        if self.continuous_mode && self.status != AgentStatus::EmergencyStopped {
                            self.start_recording(&ctx, false);
                        }
                        return;
                    }

                    if is_start_subtitles_command(&norm_prompt) {
                        self.start_live_subtitles(None);
                        if self.continuous_mode && self.status != AgentStatus::EmergencyStopped {
                            self.start_recording(&ctx, false);
                        }
                        return;
                    }
                    if is_stop_subtitles_command(&norm_prompt) {
                        self.stop_live_subtitles();
                        return;
                    }

                    if is_dismiss_hover_command(&prompt) {
                        mark_hover_nav_asked();
                        play_earcon(Earcon::Success);
                        self.status = AgentStatus::Idle;
                        let reply = t!("speech.dismiss_hover").to_string();
                        self.chat_history.push(ChatEntry {
                            role: ChatRole::User,
                            text: prompt,
                            timestamp: current_time_str(),
                            quick_suggestions: Vec::new(),
                            screen_size_kb: None,
                        });
                        self.chat_history.push(ChatEntry {
                            role: ChatRole::Agent,
                            text: reply.clone(),
                            timestamp: current_time_str(),
                            quick_suggestions: Vec::new(),
                            screen_size_kb: None,
                        });
                        let _ = self.tts_sender.send(TtsCommand::Speak(reply));
                        if self.continuous_mode && self.status != AgentStatus::EmergencyStopped {
                            self.start_recording(&ctx, false);
                        }
                        return;
                    }

                    // 1. Arrêt d'urgence local universel (<50ms, sans requête LLM)
                    if is_emergency_stop_command(&prompt) {
                        self.trigger_emergency_stop(&ctx);
                        return;
                    }

                    // Annulation instantanée des commandes différées
                    if is_cancel_scheduled_command(&prompt) {
                        let cancelled = cancel_all_scheduled_tasks();
                        play_earcon(Earcon::Success);
                        let reply = if cancelled > 0 {
                            t!("speech.scheduled_tasks_cancelled_count", count = cancelled).to_string()
                        } else {
                            t!("speech.scheduled_tasks_cancelled").to_string()
                        };
                        self.chat_history.push(ChatEntry {
                            role: ChatRole::User,
                            text: prompt,
                            timestamp: current_time_str(),
                            quick_suggestions: Vec::new(),
                            screen_size_kb: None,
                        });
                        self.chat_history.push(ChatEntry {
                            role: ChatRole::Agent,
                            text: reply.clone(),
                            timestamp: current_time_str(),
                            quick_suggestions: Vec::new(),
                            screen_size_kb: None,
                        });
                        let _ = self.tts_sender.send(TtsCommand::Speak(reply));
                        if self.continuous_mode && self.status != AgentStatus::EmergencyStopped {
                            self.start_recording(&ctx, false);
                        }
                        return;
                    }

                // Détection vocale d'annulation prioritaire (Arrêt du job en cours + Rollback)
                if norm_prompt == "annule" || norm_prompt == "annuler" || norm_prompt == "undo" || norm_prompt == "reviens en arriere" || norm_prompt == "reviens en arrière" {
                    let _ = CURRENT_REQUEST_ID.fetch_add(1, Ordering::SeqCst);
                    let undo_feedback = if let Some(fb) = undo_last_system_action() {
                        fb
                    } else {
                        #[cfg(windows)]
                        send_hotkey(&[windows::Win32::UI::Input::KeyboardAndMouse::VIRTUAL_KEY(0x11)], windows::Win32::UI::Input::KeyboardAndMouse::VIRTUAL_KEY(0x5A));
                        #[cfg(not(windows))]
                        let _ = std::process::Command::new("xdotool").args(["key", "ctrl+z"]).status();
                        t!("commands.last_action_undone").to_string()
                    };
                    play_earcon(Earcon::Success);
                    self.chat_history.push(ChatEntry {
                        role: ChatRole::User,
                        text: prompt,
                        timestamp: current_time_str(),
                        quick_suggestions: Vec::new(),
                        screen_size_kb: None,
                    });
                    self.chat_history.push(ChatEntry {
                        role: ChatRole::Agent,
                        text: undo_feedback.clone(),
                        timestamp: current_time_str(),
                        quick_suggestions: Vec::new(),
                        screen_size_kb: None,
                    });
                    let _ = self.tts_sender.send(TtsCommand::Speak(undo_feedback));
                    self.status = AgentStatus::Idle;
                    return;
                }

                    // 2. Sortie déterministe immédiate du mode dictée
                    if self.dictation_mode && is_stop_dictation_command(&prompt) {
                        self.exit_dictation_mode();
                        return;
                    }

                    // Activation déterministe immédiate du mode dictée
                    if !self.dictation_mode && is_start_dictation_command(&prompt) {
                        self.enter_dictation_mode();
                        return;
                    }

                    // 2 bis. Exécution déterministe locale instantanée des commandes de base (< 20 ms, 0 token LLM)
                    if let Some(feedback) = try_execute_local_navigation_shortcut(&prompt) {
                        play_earcon(Earcon::Success);
                        self.chat_history.push(ChatEntry {
                            role: ChatRole::User,
                            text: prompt,
                            timestamp: current_time_str(),
                            quick_suggestions: Vec::new(),
                            screen_size_kb: None,
                        });
                        if self.continuous_mode && self.status != AgentStatus::EmergencyStopped {
                            self.start_recording(&ctx, false);
                        }
                        return;
                    }

                    // 3. Configuration de la clé API
                    if is_api_key_command(&prompt) {
                        self.sync_modal_inputs_for_provider(self.llm_config.provider);
                        self.show_api_key_modal = true;
                        self.is_hidden = false;
                        let reply = t!("speech.api_key_modal_opened").to_string();
                        self.chat_history.push(ChatEntry {
                            role: ChatRole::Agent,
                            text: reply.clone(),
                            timestamp: current_time_str(),
                            quick_suggestions: Vec::new(),
                            screen_size_kb: None,
                        });
                        let _ = self.tts_sender.send(TtsCommand::Speak(reply));
                        self.continuous_mode = false;
                        continue;
                    }

                    // 4. Masquage / Réaffichage de l'overlay
                    if self.is_hidden {
                        if is_show_command(&prompt) {
                            self.is_hidden = false;
                            self.status = AgentStatus::Idle;
                            self.chat_history.push(ChatEntry {
                                role: ChatRole::User,
                                text: prompt,
                                timestamp: current_time_str(),
                                quick_suggestions: Vec::new(),
                                screen_size_kb: None,
                            });
                            let reply = t!("speech.overlay_restored").to_string();
                            self.chat_history.push(ChatEntry {
                                role: ChatRole::Agent,
                                text: reply.clone(),
                                timestamp: current_time_str(),
                                quick_suggestions: Vec::new(),
                                screen_size_kb: None,
                            });
                            let _ = self.tts_sender.send(TtsCommand::Speak(reply));
                            self.continuous_mode = true;
                        } else if self.continuous_mode && self.status != AgentStatus::EmergencyStopped {
                            // Tout autre message est ignoré lorsque l'overlay est masqué
                            self.start_recording(&ctx, false);
                        }
                    } else {
                        if is_hide_command(&prompt) {
                            self.is_hidden = true;
                            self.status = AgentStatus::Idle;
                            self.chat_history.push(ChatEntry {
                                role: ChatRole::User,
                                text: prompt,
                                timestamp: current_time_str(),
                                quick_suggestions: Vec::new(),
                                screen_size_kb: None,
                            });
                            let reply = t!("speech.overlay_hidden").to_string();
                            self.chat_history.push(ChatEntry {
                                role: ChatRole::Agent,
                                text: reply.clone(),
                                timestamp: current_time_str(),
                                quick_suggestions: Vec::new(),
                                screen_size_kb: None,
                            });
                            let _ = self.tts_sender.send(TtsCommand::Speak(reply));
                            self.continuous_mode = true;
                        } else if is_show_command(&prompt) {
                            let reply = t!("speech.overlay_already_visible").to_string();
                            self.chat_history.push(ChatEntry {
                                role: ChatRole::Agent,
                                text: reply.clone(),
                                timestamp: current_time_str(),
                                quick_suggestions: Vec::new(),
                                screen_size_kb: None,
                            });
                            let _ = self.tts_sender.send(TtsCommand::Speak(reply));
                            self.continuous_mode = true;
                        } else if !prompt.trim().is_empty() {
                            self.status = AgentStatus::Thinking;
                            self.continuous_mode = true;
                            self.input_text.clear();
                            self.chat_history.push(ChatEntry {
                                role: ChatRole::User,
                                text: prompt.clone(),
                                timestamp: current_time_str(),
                                quick_suggestions: Vec::new(),
                                screen_size_kb: None,
                            });
                            let _ = self.command_sender.send(AgentCommand::Prompt(prompt));
                        } else if self.continuous_mode && self.status != AgentStatus::EmergencyStopped {
                            self.start_recording(&ctx, false);
                        }
                    }
                }
                AgentEvent::TtsFinished => {
                    if self.continuous_mode
                        && !crate::audio::is_live_subtitles_active()
                        && self.status != AgentStatus::EmergencyStopped
                        && !self.show_api_key_modal
                        && !AGENT_BUSY.load(Ordering::SeqCst)
                        && self.whisper_ready
                    {
                        self.start_recording(&ctx, true);
                    }
                }
                AgentEvent::WhisperReady => {
                    println!("[Whisper] Notification WhisperReady reçue.");
                    self.whisper_ready = true;
                    let is_user_typing = ctx.memory(|m| {
                        m.has_focus(egui::Id::new("prompt_input_text"))
                            || m.has_focus(egui::Id::new("api_key_input_field"))
                            || m.has_focus(egui::Id::new("model_input_field"))
                    });
                    if self.continuous_mode
                        && self.status != AgentStatus::EmergencyStopped
                        && self.status != AgentStatus::Speaking
                        && !self.show_api_key_modal
                        && !AGENT_BUSY.load(Ordering::SeqCst)
                        && !is_user_typing
                    {
                        self.start_recording(&ctx, true);
                    }
                    ctx.request_repaint();
                }
                AgentEvent::TwitchChatReceived(twitch_msg) => {
                    self.twitch_messages.push(twitch_msg);
                    if self.twitch_messages.len() > 300 {
                        self.twitch_messages.drain(0..self.twitch_messages.len() - 300);
                    }
                }
                AgentEvent::TwitchSearchResults(results) => {
                    self.twitch_search_results = results;
                }
                AgentEvent::RequestIgnored => {
                    if let Some(last) = self.chat_history.last() {
                        if last.role == ChatRole::User {
                            self.chat_history.pop();
                        }
                    }
                    self.live_transcript.clear();
                    if self.continuous_mode && self.status != AgentStatus::EmergencyStopped {
                        self.start_recording(&ctx, false);
                    }
                }
                AgentEvent::RequestApiKey => {
                    self.sync_modal_inputs_for_provider(self.llm_config.provider);
                    self.show_api_key_modal = true;
                    self.is_hidden = false;
                }
                AgentEvent::ShowFloatingCard(card) => {
                    let vp_id = egui::ViewportId::from_hash_of(&card.id);
                    let card_title = card.title.clone();
                    let card_speech = card.build_narrator_speech();
                    if let Some(existing) = self.floating_cards.iter_mut().find(|c| c.id == card.id) {
                        *existing = card;
                    } else {
                        self.floating_cards.push(card);
                    }
                    update_open_floating_cards(&self.floating_cards);
                    let read_suggestion = t!("ui.read_card_prefix", title = card_title).to_string();
                    if let Some(last_entry) = self.chat_history.last_mut() {
                        if !last_entry.quick_suggestions.iter().any(|s| s.as_str() == Some(&read_suggestion)) {
                            last_entry.quick_suggestions.insert(0, QuickSuggestionItem::Text(read_suggestion));
                        }
                    } else {
                        self.chat_history.push(ChatEntry {
                            role: ChatRole::Agent,
                            text: t!("ui.card_displayed", title = card_title).to_string(),
                            timestamp: current_time_str(),
                            quick_suggestions: vec![QuickSuggestionItem::Text(read_suggestion)],
                            screen_size_kb: None,
                        });
                    }
                    if self.status != AgentStatus::Speaking && !card_speech.trim().is_empty() {
                        self.status = AgentStatus::Speaking;
                        let _ = self.tts_sender.send(TtsCommand::Speak(card_speech));
                    }
                    ctx.request_repaint();
                    ctx.request_repaint_of(vp_id);
                }
                AgentEvent::CloseFloatingCard(id) => {
                    let vp_id = egui::ViewportId::from_hash_of(&id);
                    if let Some(card) = self.floating_cards.iter().find(|c| c.id == id) {
                        if matches!(card.content, FloatingContentType::Subtitles { .. }) {
                            floating_window::stop_live_subtitles();
                            crate::audio::stop_live_subtitles();
                        }
                    }
                    self.floating_cards.retain(|c| c.id != id);
                    update_open_floating_cards(&self.floating_cards);
                    ctx.request_repaint();
                    ctx.request_repaint_of(vp_id);
                }
                AgentEvent::ClickNumberedTarget(idx) => {
                    self.click_numbered_target_by_index(idx);
                }
                AgentEvent::ToggleNumberedTargets(enabled) => {
                    self.toggle_switch_access_mode(enabled);
                }
                AgentEvent::NextNumberedChunk => {
                    // TODO: Implement badge pagination logic
                }
                AgentEvent::PreviousNumberedChunk => {
                    // TODO: Implement badge pagination logic
                }
                AgentEvent::LiveSubtitleUpdate { text, spectrum } => {
                    floating_window::update_live_subtitles(text, spectrum);
                    let mut found_sub_card = false;
                    for card in &self.floating_cards {
                        if matches!(card.content, FloatingContentType::Subtitles { .. }) {
                            found_sub_card = true;
                            let vp_id = egui::ViewportId::from_hash_of(&card.id);
                            ctx.request_repaint_of(vp_id);
                        }
                    }
                    if !found_sub_card {
                        let card = FloatingCard::new(
                            "live_subtitles",
                            t!("commands.subtitles_title").to_string(),
                            FloatingContentType::Subtitles {
                                text: String::new(),
                                target_window: None,
                                history: Vec::new(),
                            },
                        );
                        let vp_id = egui::ViewportId::from_hash_of(&card.id);
                        self.floating_cards.push(card);
                        update_open_floating_cards(&self.floating_cards);
                        ctx.request_repaint_of(vp_id);
                    }
                }
                AgentEvent::LiveSubtitlesClosed => {
                    floating_window::stop_live_subtitles();
                    crate::audio::stop_live_subtitles();
                    self.floating_cards.retain(|c| !matches!(c.content, FloatingContentType::Subtitles { .. }));
                    update_open_floating_cards(&self.floating_cards);
                    ctx.request_repaint();
                }
                AgentEvent::NavigationZonesReady { window_id, window_title, summary, zones } => {
                    self.window_analysis_cache.insert(window_id, CachedWindowAnalysis {
                        summary: summary.clone(),
                        zones: zones.clone(),
                    });

                    if self.switch_access_mode && self.switch_active_window_id == Some(window_id) {
                        self.switch_zones = zones;
                        if let Some(first_zone) = self.switch_zones.first() {
                            let mut active_targets = first_zone.targets.clone();
                            for (i, t) in active_targets.iter_mut().enumerate() {
                                t.index = i + 1;
                            }
                            self.numbered_targets = active_targets;
                        }
                        if !summary.is_empty() && self.hover_controls_speech_enabled {
                            let _ = self.tts_sender.send(TtsCommand::Stop);
                            let _ = self.tts_sender.send(TtsCommand::Speak(summary));
                        }
                        ctx.request_repaint();
                    }
                }
            }
        }

        let is_typing = ctx.memory(|m| {
            m.has_focus(egui::Id::new("prompt_input_text"))
                || m.has_focus(egui::Id::new("api_key_input_field"))
                || m.has_focus(egui::Id::new("model_input_field"))
                || m.has_focus(egui::Id::new("macro_shortcut_input"))
                || m.has_focus(egui::Id::new("macro_text_input"))
        });
        crate::automation::IS_USER_TYPING.store(is_typing, Ordering::Relaxed);

        // Gestion de la touche Échap globale et locale
        let mut hook_events = Vec::new();
        if let Ok(mut q) = SWITCH_KEY_QUEUE.lock() {
            hook_events.append(&mut *q);
        }

        let mut hook_digits = Vec::new();
        if let Ok(mut nq) = NUMBER_KEY_QUEUE.lock() {
            hook_digits.append(&mut *nq);
        }

        if !is_typing && self.switch_access_mode {
            let mut egui_digit_pressed = false;
            for (k, c) in [
                (egui::Key::Num0, '0'), (egui::Key::Num1, '1'), (egui::Key::Num2, '2'),
                (egui::Key::Num3, '3'), (egui::Key::Num4, '4'), (egui::Key::Num5, '5'),
                (egui::Key::Num6, '6'), (egui::Key::Num7, '7'), (egui::Key::Num8, '8'),
                (egui::Key::Num9, '9'),
            ] {
                if ctx.input(|i| i.key_pressed(k)) {
                    hook_digits.push(c);
                    egui_digit_pressed = true;
                    ctx.input_mut(|i| i.consume_key(egui::Modifiers::NONE, k));
                    ctx.input_mut(|i| i.consume_key(egui::Modifiers::SHIFT, k));
                }
            }

            if !egui_digit_pressed {
                ctx.input(|i| {
                    for ev in &i.events {
                        if let egui::Event::Text(ref text) = ev {
                            for ch in text.chars() {
                                if ch.is_ascii_digit() {
                                    hook_digits.push(ch);
                                }
                            }
                        }
                    }
                });
            }
        }

        if hook_events.contains(&SwitchKeyEvent::ToggleDictation) {
            if self.dictation_mode {
                self.exit_dictation_mode();
            } else {
                self.enter_dictation_mode();
            }
        }
        if hook_events.contains(&SwitchKeyEvent::ToggleSwitch) {
            self.toggle_switch_access_mode(None);
        }
        if hook_events.contains(&SwitchKeyEvent::ToggleVoice) {
            self.set_voice_gender(self.voice_gender.next());
        }
        if hook_events.contains(&SwitchKeyEvent::Clear) {
            let _ = self.command_sender.send(AgentCommand::ClearHistory);
            self.chat_history.clear();
            self.window_analysis_cache.clear();
            self.chat_history.push(ChatEntry {
                role: ChatRole::Agent,
                text: t!("commands.context_cleared").to_string(),
                timestamp: current_time_str(),
                quick_suggestions: Vec::new(),
                screen_size_kb: None,
            });
            self.live_transcript.clear();
            self.stop_speaking();
        }

        #[cfg(windows)]
        let global_escape_hit = unsafe { (GetAsyncKeyState(0x1B) as u16 & 0x8000) != 0 };
        #[cfg(not(windows))]
        let global_escape_hit = false;
        let hook_escape_hit = hook_events.contains(&SwitchKeyEvent::Escape)
            || (hook_events.contains(&SwitchKeyEvent::Back) && self.numbered_input_buffer.is_empty());

        let egui_backspace_hit = ctx.input(|i| i.key_pressed(egui::Key::Backspace));
        let backspace_hit = (egui_backspace_hit && !is_typing) || hook_events.contains(&SwitchKeyEvent::Back);

        // Si le buffer contient des chiffres, Backspace efface le dernier chiffre sans quitter l'élément
        if backspace_hit && !self.numbered_input_buffer.is_empty() {
            self.numbered_input_buffer.pop();
            self.numbered_input_last_char_time = Instant::now();
            ctx.request_repaint();
        }

        let egui_escape_hit = ctx.input(|i| i.key_pressed(egui::Key::Escape))
            || (egui_backspace_hit && !is_typing && self.numbered_input_buffer.is_empty());
        let escape_triggered = (egui_escape_hit || hook_escape_hit || global_escape_hit)
            && self.last_switch_input.elapsed() >= Duration::from_millis(200);

        if escape_triggered {
            self.last_switch_input = Instant::now();
            self.stop_speaking();
            if self.dictation_mode {
                self.exit_dictation_mode();
            } else if self.switch_access_mode {
                if self.switch_step == SwitchAccessStep::Elements {
                    if !self.switch_zones.is_empty() {
                        self.switch_step = SwitchAccessStep::Zones;
                        self.switch_active_zone_index = None;
                        self.numbered_targets.clear();
                        self.switch_focus_index = 0;
                        self.switch_auto_scroll = false;
                        self.hover_physical_mouse_moved = false;
                        self.switch_last_tick = Instant::now() + Duration::from_millis(1500);
                        let _ = self.tts_sender.send(TtsCommand::Stop);
                        let cur_zone = &self.switch_zones[0];
                        let msg = t!("commands.back_to_blocks", zone = cur_zone.title).to_string();
                        let _ = self.tts_sender.send(TtsCommand::Speak(msg));
                        play_earcon(Earcon::Success);
                        let zone_infos: Vec<ZoneOverlayInfo> = self.switch_zones.iter().enumerate().map(|(i, z)| {
                            ZoneOverlayInfo {
                                index: i + 1,
                                title: z.title.clone(),
                                rect: z.rect,
                                is_focused: i == 0,
                            }
                        }).collect();
                        if let Some(first_z) = self.switch_zones.first() {
                            let mut active_targets = first_z.targets.clone();
                            for (i, t) in active_targets.iter_mut().enumerate() {
                                t.index = i + 1;
                            }
                            self.numbered_targets = active_targets;
                        }
                        let focused_target_idx = self.numbered_targets.first().map(|t| t.index);
                        draw_screen_zones_and_targets(&zone_infos, &self.numbered_targets, self.zoom_factor, focused_target_idx);
                    } else {
                        self.switch_step = SwitchAccessStep::Windows;
                        self.numbered_targets.clear();
                        self.switch_active_zone_index = None;
                        self.switch_windows = collect_switch_candidate_windows();
                        self.switch_auto_scroll = false;
                        self.switch_focus_index = 0;
                        self.hover_physical_mouse_moved = false;
                        self.numbered_targets = self.switch_windows.iter().enumerate().map(|(i, w)| {
                            NumberedElementTarget {
                                index: i + 1,
                                name: w.title.clone(),
                                role: "fenêtre".to_string(),
                                rect: w.rect,
                                click_x: w.click_x,
                                click_y: w.click_y,
                                shortcut: String::new(),
                                value: String::new(),
                                window_id: Some(w.id),
                            }
                        }).collect();
                        let _ = self.tts_sender.send(TtsCommand::Stop);
                        let base_msg = t!("speech.back_to_windows").to_string();
                        let msg = if let Some(first_win) = self.switch_windows.first() {
                            format!("{base_msg} : {}", first_win.title)
                        } else {
                            base_msg
                        };
                        let _ = self.tts_sender.send(TtsCommand::Speak(msg));
                        play_earcon(Earcon::Success);
                    }
                } else if self.switch_step == SwitchAccessStep::Zones {
                    self.switch_step = SwitchAccessStep::Windows;
                    self.numbered_targets.clear();
                    self.switch_zones.clear();
                    self.switch_active_zone_index = None;
                    self.switch_windows = collect_switch_candidate_windows();
                    self.switch_auto_scroll = false;
                    self.switch_focus_index = 0;
                    self.hover_physical_mouse_moved = false;
                    self.numbered_targets = self.switch_windows.iter().enumerate().map(|(i, w)| {
                        NumberedElementTarget {
                            index: i + 1,
                            name: w.title.clone(),
                            role: "fenêtre".to_string(),
                            rect: w.rect,
                            click_x: w.click_x,
                            click_y: w.click_y,
                            shortcut: String::new(),
                            value: String::new(),
                            window_id: Some(w.id),
                        }
                    }).collect();
                    let base_msg = t!("speech.back_to_windows").to_string();
                    let msg = if let Some(first_win) = self.switch_windows.first() {
                        format!("{base_msg} : {}", first_win.title)
                    } else {
                        base_msg
                    };
                    let _ = self.tts_sender.send(TtsCommand::Speak(msg));
                    play_earcon(Earcon::Success);
                } else {
                    self.toggle_switch_access_mode(Some(false));
                }
            } else {
                self.trigger_emergency_stop(&ctx);
            }
        }

        // Réception asynchrone sans blocage des cibles calculées par le thread dédié
        while let Ok(result) = self.targets_result_rx.try_recv() {
            if result.sequence >= self.last_applied_scan_sequence && self.switch_active_window_id == Some(result.hwnd) {
                self.last_applied_scan_sequence = result.sequence;
                self.is_scan_in_progress = false;

                // Mise à jour fluide sans écraser le découpage de la zone sélectionnée
                if self.switch_step == SwitchAccessStep::Elements {
                    if let Some(z_idx) = self.switch_active_zone_index {
                        if let Some(zone) = self.switch_zones.get_mut(z_idx) {
                            let (z_min_x, z_min_y, z_max_x, z_max_y) = (
                                zone.rect[0] - 15,
                                zone.rect[1] - 15,
                                zone.rect[2] + 15,
                                zone.rect[3] + 15,
                            );
                            let fresh_zone_targets: Vec<NumberedElementTarget> = result.targets
                                .iter()
                                .filter(|t| {
                                    t.click_x >= z_min_x
                                        && t.click_x <= z_max_x
                                        && t.click_y >= z_min_y
                                        && t.click_y <= z_max_y
                                })
                                .cloned()
                                .collect();

                            if !fresh_zone_targets.is_empty() {
                                zone.targets = fresh_zone_targets;
                            }
                            self.numbered_targets = zone.targets.clone();
                            for (i, t) in self.numbered_targets.iter_mut().enumerate() {
                                t.index = i + 1;
                            }
                        }
                    }
                } else if self.switch_step == SwitchAccessStep::Windows {
                    let (_, fb_zones) = build_fallback_navigation_zones("", &result.targets);
                    self.switch_zones = fb_zones;
                    if let Some(first_zone) = self.switch_zones.first() {
                        let mut active_targets = first_zone.targets.clone();
                        for (i, t) in active_targets.iter_mut().enumerate() {
                            t.index = i + 1;
                        }
                        self.numbered_targets = active_targets;
                    }
                } else if self.switch_step == SwitchAccessStep::Zones {
                    // En mode Zones, les cibles affichées restent restreintes à la zone focalisée
                }

                let physical_cursor = get_global_cursor_pos();
                if let Some((cx, cy)) = physical_cursor {
                    self.hover_last_cursor_pos = Some((cx, cy));
                    self.hover_cursor_still_since = Instant::now();
                    if self.switch_step == SwitchAccessStep::Elements {
                        if let Some(pos) = self.numbered_targets.iter().position(|t| {
                            cx >= t.rect[0] && cx <= t.rect[2] && cy >= t.rect[1] && cy <= t.rect[3]
                        }) {
                            self.switch_focus_index = pos;
                        } else if self.switch_focus_index >= self.numbered_targets.len() {
                            self.switch_focus_index = 0;
                        }
                    }
                } else if self.switch_step == SwitchAccessStep::Elements && self.switch_focus_index >= self.numbered_targets.len() {
                    self.switch_focus_index = 0;
                }
                ctx.request_repaint();
            }
        }

        // Actualisation automatique avec debounce pour le mode survol/numéros (UIA + OCR)
        if self.switch_access_mode && !self.dictation_mode {
            #[cfg(windows)]
            {
                let fg = unsafe { windows::Win32::UI::WindowsAndMessaging::GetForegroundWindow() };
                let current_pid = std::process::id();
                let mut fg_pid = 0u32;
                unsafe { windows::Win32::UI::WindowsAndMessaging::GetWindowThreadProcessId(fg, Some(&mut fg_pid)) };
                let is_overlay = fg_pid == current_pid;

                let (target_hwnd, cur_hwnd_id) = if !fg.0.is_null() && !is_overlay && !is_window_hung(fg) {
                    (fg, Some(fg.0 as isize))
                } else if let Some(wid) = self.switch_active_window_id {
                    let h = windows::Win32::Foundation::HWND(wid as _);
                    if !h.0.is_null() && !is_window_hung(h) {
                        (h, Some(wid))
                    } else {
                        (windows::Win32::Foundation::HWND(std::ptr::null_mut()), None)
                    }
                } else {
                    (windows::Win32::Foundation::HWND(std::ptr::null_mut()), None)
                };

                if !target_hwnd.0.is_null() {
                    let mut r = windows::Win32::Foundation::RECT::default();
                    let has_rect = unsafe { windows::Win32::UI::WindowsAndMessaging::GetWindowRect(target_hwnd, &mut r).is_ok() };
                    let cur_rect = if has_rect { Some([r.left, r.top, r.right, r.bottom]) } else { None };

                    let win_changed = cur_hwnd_id != self.numbered_last_hwnd;
                    let rect_changed = match (cur_rect, self.numbered_last_win_rect) {
                        (Some(c), Some(p)) => {
                            (c[0] - p[0]).abs() > 10 || (c[1] - p[1]).abs() > 10
                                || (c[2] - p[2]).abs() > 10 || (c[3] - p[3]).abs() > 10
                        }
                        (Some(_), None) | (None, Some(_)) => true,
                        (None, None) => false,
                    };
                    let user_navigating = self.last_switch_input.elapsed() < Duration::from_millis(600);

                    if win_changed {
                        // Changement de fenêtre : délégation immédiate au worker
                        self.refresh_numbered_targets_for_window(target_hwnd.0 as isize);
                    } else if (self.switch_step == SwitchAccessStep::Windows || self.switch_step == SwitchAccessStep::Elements) && !user_navigating && !self.is_scan_in_progress {
                        let min_display_time_passed = self.numbered_last_refresh.elapsed() >= Duration::from_millis(1200);
                        let should_periodic_refresh = (rect_changed && min_display_time_passed)
                            || (self.numbered_last_refresh.elapsed() >= Duration::from_millis(5000));

                        if should_periodic_refresh {
                            self.refresh_numbered_targets_for_window(target_hwnd.0 as isize);
                        }
                    }
                }
            }
            #[cfg(not(windows))]
            {
                let user_navigating = self.last_switch_input.elapsed() < Duration::from_millis(600);
                if !user_navigating && !self.is_scan_in_progress && self.numbered_last_refresh.elapsed() >= Duration::from_millis(3500) {
                    if let Some(wid) = self.switch_active_window_id {
                        self.refresh_numbered_targets_for_window(wid);
                    }
                }
            }
        }

        // Gestion du Switch Access & Survol : défilement séquentiel périodique et validation sur Entrée / Espace
        if self.switch_access_mode && !is_typing {
            let is_shift = ctx.input(|i| i.modifiers.shift);
            let egui_enter = ctx.input(|i| i.key_pressed(egui::Key::Enter));
            let egui_space = ctx.input(|i| i.key_pressed(egui::Key::Space));
            let egui_tab = ctx.input(|i| i.key_pressed(egui::Key::Tab));
            let egui_nav_next = ctx.input(|i| {
                i.key_pressed(egui::Key::ArrowRight) || i.key_pressed(egui::Key::ArrowDown)
            }) || (egui_tab && !is_shift);
            let egui_nav_prev = ctx.input(|i| {
                i.key_pressed(egui::Key::ArrowLeft) || i.key_pressed(egui::Key::ArrowUp)
            }) || (egui_tab && is_shift);

            let hook_enter_or_space = hook_events.contains(&SwitchKeyEvent::Select);
            let hook_nav_next = hook_events.contains(&SwitchKeyEvent::Next);
            let hook_nav_prev = hook_events.contains(&SwitchKeyEvent::Prev);

            let mut switch_input_hit = (egui_enter || egui_space || hook_enter_or_space)
                && self.last_switch_input.elapsed() >= Duration::from_millis(180);

            let nav_next_hit = (egui_nav_next || hook_nav_next)
                && self.last_switch_input.elapsed() >= Duration::from_millis(150);
            let nav_prev_hit = (egui_nav_prev || hook_nav_prev)
                && self.last_switch_input.elapsed() >= Duration::from_millis(150);

            if egui_enter || egui_space || egui_tab || egui_nav_next || egui_nav_prev {
                ctx.input_mut(|i| {
                    i.consume_key(egui::Modifiers::NONE, egui::Key::Enter);
                    i.consume_key(egui::Modifiers::NONE, egui::Key::Space);
                    i.consume_key(egui::Modifiers::NONE, egui::Key::Tab);
                    i.consume_key(egui::Modifiers::SHIFT, egui::Key::Tab);
                    i.consume_key(egui::Modifiers::NONE, egui::Key::ArrowRight);
                    i.consume_key(egui::Modifiers::NONE, egui::Key::ArrowDown);
                    i.consume_key(egui::Modifiers::NONE, egui::Key::ArrowLeft);
                    i.consume_key(egui::Modifiers::NONE, egui::Key::ArrowUp);
                });
            }

            // --- Ingestion des chiffres reçus (hook global pavé numérique ou egui) ---
            if !hook_digits.is_empty() {
                if self.status == AgentStatus::Speaking {
                    self.stop_speaking();
                }
                play_earcon(Earcon::Success);
                for d in hook_digits {
                    self.numbered_input_buffer.push(d);
                }
                self.numbered_input_last_char_time = Instant::now();
                ctx.request_repaint();
            }

            // Réinitialisation du buffer si l'utilisateur navigue avec les flèches
            if nav_next_hit || nav_prev_hit {
                self.numbered_input_buffer.clear();
            }

            // Validation immédiate du numéro saisi par Entrée / Espace
            let mut digit_confirmed = false;
            if switch_input_hit && !self.numbered_input_buffer.is_empty() {
                if let Ok(idx) = self.numbered_input_buffer.parse::<usize>() {
                    self.numbered_input_buffer.clear();
                    self.last_switch_input = Instant::now();
                    self.stop_speaking();
                    self.click_numbered_target_by_index(idx);
                    digit_confirmed = true;
                    switch_input_hit = false;
                    ctx.request_repaint();
                } else {
                    self.numbered_input_buffer.clear();
                }
            }

            // Validation automatique après temporisation (900 ms) sans appui sur Entrée
            let buffer_timeout = Duration::from_millis(900);
            if !digit_confirmed && !self.numbered_input_buffer.is_empty() {
                if self.numbered_input_last_char_time.elapsed() >= buffer_timeout {
                    if let Ok(idx) = self.numbered_input_buffer.parse::<usize>() {
                        if idx > 0 {
                            self.last_switch_input = Instant::now();
                            self.stop_speaking();
                            self.click_numbered_target_by_index(idx);
                        }
                    }
                    self.numbered_input_buffer.clear();
                    ctx.request_repaint();
                } else {
                    ctx.request_repaint_after(buffer_timeout - self.numbered_input_last_char_time.elapsed());
                }
            }

            match self.switch_step {
                SwitchAccessStep::Windows => {
                    if self.switch_windows.is_empty() {
                        self.switch_windows = collect_switch_candidate_windows();
                    }
                    let num_windows = self.switch_windows.len();
                    if num_windows > 0 {
                        let step_next = nav_next_hit;
                        let step_prev = nav_prev_hit;

                        self.switch_focus_index %= num_windows;
                        if step_next || step_prev {
                            if step_prev {
                                self.switch_focus_index = if self.switch_focus_index == 0 || self.switch_focus_index >= num_windows {
                                    num_windows - 1
                                } else {
                                    self.switch_focus_index - 1
                                };
                            } else {
                                self.switch_focus_index = (self.switch_focus_index + 1) % num_windows;
                            }
                            self.switch_focus_index %= num_windows;
                            self.switch_last_tick = Instant::now();
                        self.last_switch_input = Instant::now();
                        self.hover_controls_speech_enabled = false;
                        play_earcon(Earcon::Navigation);
                        self.switch_auto_scroll = false;
                            self.hover_physical_mouse_moved = false;

                            let cur_win = self.switch_windows[self.switch_focus_index % num_windows].clone();
                            self.activate_window_and_display_first_zone(cur_win.id, &cur_win.title, cur_win.rect, true);
                            self.hover_last_announced_target = Some(cur_win.id as usize);
                            ctx.request_repaint();
                        }

                        if switch_input_hit || ctx.input(|i| i.key_pressed(egui::Key::ArrowDown)) {
                            self.last_switch_input = Instant::now();
                            self.switch_auto_scroll = false;
                            self.hover_controls_speech_enabled = false;
                            self.hover_physical_mouse_moved = false;
                            play_earcon(Earcon::Success);

                            let win_idx = self.switch_focus_index % num_windows;
                            let cur_win = &self.switch_windows[win_idx];
                            self.switch_active_window_id = Some(cur_win.id);
                            let win_title = cur_win.title.clone();
                            let win_id = cur_win.id;
                            self.transition_to_zones_for_window(win_id, &win_title);
                            ctx.request_repaint();
                        }
                    }
                }
                SwitchAccessStep::Zones => {
                    let num_zones = self.switch_zones.len();
                    if num_zones > 0 {
                        let step_next = nav_next_hit;
                        let step_prev = nav_prev_hit;

                        self.switch_focus_index %= num_zones;
                        if step_next || step_prev {
                            if step_prev {
                                self.switch_focus_index = if self.switch_focus_index == 0 || self.switch_focus_index >= num_zones {
                                    num_zones - 1
                                } else {
                                    self.switch_focus_index - 1
                                };
                            } else {
                                self.switch_focus_index = (self.switch_focus_index + 1) % num_zones;
                            }
                            self.switch_focus_index %= num_zones;
                            self.hover_controls_speech_enabled = true;
                            self.switch_last_tick = Instant::now();
                            play_earcon(Earcon::Navigation);
                            self.hover_physical_mouse_moved = false;
                            if nav_next_hit || nav_prev_hit {
                                self.last_switch_input = Instant::now();
                                self.switch_auto_scroll = false;
                            }

                            #[cfg(windows)]
                            if let Some(id) = self.switch_active_window_id {
                                let hwnd = windows::Win32::Foundation::HWND(id as _);
                                if !is_window_hung(hwnd) {
                                    force_foreground_window(hwnd);
                                    restore_if_iconic(hwnd);
                                    std::thread::sleep(Duration::from_millis(40));
                                }
                            }

                            let cur_zone = &self.switch_zones[self.switch_focus_index % num_zones];
                            set_global_cursor_pos(cur_zone.click_x, cur_zone.click_y);
                            self.hover_last_cursor_pos = Some((cur_zone.click_x, cur_zone.click_y));
                            self.hover_cursor_still_since = Instant::now();

                            let zone_infos: Vec<ZoneOverlayInfo> = self.switch_zones.iter().enumerate().map(|(i, z)| {
                                ZoneOverlayInfo {
                                    index: i + 1,
                                    title: z.title.clone(),
                                    rect: z.rect,
                                    is_focused: (self.switch_focus_index % num_zones) == i,
                                }
                            }).collect();

                            // N'afficher que les badges jaunes appartenant à cette zone
                            let mut active_targets = cur_zone.targets.clone();
                            for (i, t) in active_targets.iter_mut().enumerate() {
                                t.index = i + 1;
                            }
                            self.numbered_targets = active_targets;
                            let focused_target_idx = self.numbered_targets.first().map(|t| t.index);
                            draw_screen_zones_and_targets(&zone_infos, &self.numbered_targets, self.zoom_factor, focused_target_idx);

                            let vocal_label = cur_zone.title.clone();
                            let _ = self.tts_sender.send(TtsCommand::Stop);
                            let _ = self.tts_sender.send(TtsCommand::Speak(vocal_label));
                            self.hover_pending_speech = None;
                            self.hover_speech_deadline = None;
                            self.hover_last_announced_zone = Some(cur_zone.title.clone());
                            ctx.request_repaint();
                        }

                        if switch_input_hit {
                            let zone_idx = self.switch_focus_index % num_zones;
                            self.select_switch_zone(zone_idx);
                            ctx.request_repaint();
                        }
                    }
                }
                SwitchAccessStep::Elements => {
                    let num_targets = self.numbered_targets.len();
                    if num_targets > 0 {
                        let step_next = nav_next_hit;
                        let step_prev = nav_prev_hit;

                        self.switch_focus_index %= num_targets;
                        if step_next || step_prev {
                            if step_prev {
                                self.switch_focus_index = if self.switch_focus_index == 0 || self.switch_focus_index >= num_targets {
                                    num_targets - 1
                                } else {
                                    self.switch_focus_index - 1
                                };
                            } else {
                                self.switch_focus_index = (self.switch_focus_index + 1) % num_targets;
                            }
                            self.switch_focus_index %= num_targets;
                            self.hover_controls_speech_enabled = true;
                            self.switch_last_tick = Instant::now();
                            play_earcon(Earcon::Navigation);
                            self.hover_physical_mouse_moved = false;
                            if nav_next_hit || nav_prev_hit {
                                self.last_switch_input = Instant::now();
                                self.switch_auto_scroll = false;
                            }

                            let cur_target = &self.numbered_targets[self.switch_focus_index % num_targets];
                            #[cfg(windows)]
                            {
                                let host_wid = cur_target.window_id.or(self.switch_active_window_id);
                                if let Some(wid) = host_wid {
                                    let hwnd = windows::Win32::Foundation::HWND(wid as _);
                                    if !is_window_hung(hwnd) {
                                        force_foreground_window(hwnd);
                                        restore_if_iconic(hwnd);
                                    }
                                }
                            }
                            focus_element_target(cur_target);
                            self.hover_last_cursor_pos = Some((cur_target.click_x, cur_target.click_y));
                            self.hover_cursor_still_since = Instant::now();
                            #[cfg(windows)]
                            draw_screen_control_border(cur_target.rect, false);

                            let parent_zone = self.switch_active_zone_index.and_then(|idx| self.switch_zones.get(idx));
                            let zone_title = parent_zone.map(|z| z.title.as_str());
                            let index_in_zone = parent_zone.and_then(|z| {
                                z.targets.iter().position(|t| t.index == cur_target.index).map(|p| p + 1)
                            }).or(Some(self.switch_focus_index + 1));
                            let is_same_zone = self.hover_last_announced_zone.as_deref() == zone_title;

                            let announcement = build_targeted_hover_speech(
                                zone_title,
                                parent_zone.map_or(&self.numbered_targets, |z| &z.targets),
                                index_in_zone.unwrap_or(self.switch_focus_index + 1),
                                &cur_target.name,
                                &cur_target.role,
                                &cur_target.shortcut,
                                is_same_zone,
                            );

                            let _ = self.tts_sender.send(TtsCommand::Stop);
                            let _ = self.tts_sender.send(TtsCommand::Speak(announcement));
                            self.hover_pending_speech = None;
                            self.hover_speech_deadline = None;
                            self.hover_last_announced_target = Some(cur_target.index);
                            self.hover_last_announced_zone = zone_title.map(|s| s.to_string());

                            // Maintenir le cadre bleu de la zone parente avec les badges jaunes actifs
                            if let Some(zone) = parent_zone {
                                let zone_info = vec![ZoneOverlayInfo {
                                    index: self.switch_active_zone_index.unwrap_or(0) + 1,
                                    title: zone.title.clone(),
                                    rect: zone.rect,
                                    is_focused: true,
                                }];
                                draw_screen_zones_and_targets(&zone_info, &self.numbered_targets, self.zoom_factor, Some(cur_target.index));
                            }

                            ctx.request_repaint();
                        }

                        if switch_input_hit {
                            self.last_switch_input = Instant::now();
                            self.switch_auto_scroll = false;
                            self.hover_physical_mouse_moved = false;
                            play_earcon(Earcon::Success);

                            let cur_target = &self.numbered_targets[self.switch_focus_index % num_targets];

                            if cur_target.role == "lecture textuelle" {
                                let parent_zone = self.switch_active_zone_index.and_then(|idx| self.switch_zones.get(idx));
                                let zone_title = parent_zone.map(|z| z.title.clone()).unwrap_or_else(|| "Zone".to_string());
                                let zone_rect = parent_zone.map(|z| z.rect).unwrap_or(cur_target.rect);

                                #[cfg(windows)]
                                let active_hwnd = {
                                    let fg = unsafe { windows::Win32::UI::WindowsAndMessaging::GetForegroundWindow() };
                                    if !fg.0.is_null() && !is_window_hung(fg) {
                                        fg
                                    } else {
                                        LAST_EXTERNAL_HWND.lock().ok().and_then(|g| *g).map(|raw| windows::Win32::Foundation::HWND(raw as _)).unwrap_or(windows::Win32::Foundation::HWND(std::ptr::null_mut()))
                                    }
                                };

                                #[cfg(windows)]
                                let extracted_text = ui::extract_text_from_zone(active_hwnd, zone_rect, &self.numbered_targets);
                                #[cfg(not(windows))]
                                let extracted_text = ui::extract_text_from_zone((), zone_rect, &self.numbered_targets);

                                set_clipboard_text(&extracted_text);

                                let display_msg = format!("📖 [{}] :\n{}", t!("commands.text_extraction_header", zone = zone_title), extracted_text);
                                self.chat_history.push(ChatEntry {
                                    role: ChatRole::Agent,
                                    text: display_msg,
                                    timestamp: current_time_str(),
                                    quick_suggestions: vec![
                                        QuickSuggestionItem::Text(t!("commands.scroll_down").to_string()),
                                        QuickSuggestionItem::Text(t!("commands.resume_nav").to_string()),
                                    ],
                                    screen_size_kb: None,
                                });

                                let _ = self.tts_sender.send(TtsCommand::Stop);
                                let vocal_read = t!("commands.reading_zone_text", zone = zone_title, text = extracted_text).to_string();
                                let _ = self.tts_sender.send(TtsCommand::Speak(vocal_read));
                                ctx.request_repaint();
                            } else if cur_target.role == "étiquette" || cur_target.role == "label" {
                                let click_x = cur_target.click_x;
                                let click_y = cur_target.click_y;
                                #[cfg(windows)]
                                draw_screen_control_border(cur_target.rect, false);

                                let mut full_label_text = cur_target.name.clone();
                                #[cfg(windows)]
                                {
                                    let pt = windows::Win32::Foundation::POINT { x: click_x, y: click_y };
                                    let _ = unsafe { windows::Win32::System::Com::CoInitializeEx(None, windows::Win32::System::Com::COINIT_MULTITHREADED) };
                                    if let Ok(automation) = unsafe {
                                        windows::Win32::System::Com::CoCreateInstance::<_, windows::Win32::UI::Accessibility::IUIAutomation>(
                                            &windows::Win32::UI::Accessibility::CUIAutomation,
                                            None,
                                            windows::Win32::System::Com::CLSCTX_INPROC_SERVER,
                                        )
                                    } {
                                        if let Ok(elem) = unsafe { automation.ElementFromPoint(pt) } {
                                            if let Some(txt) = ui::extract_inner_text(&automation, &elem, 600) {
                                                if !txt.trim().is_empty() {
                                                    full_label_text = txt.trim().to_string();
                                                }
                                            }
                                        }
                                    }
                                }

                                if full_label_text.is_empty() && !cur_target.value.is_empty() {
                                    full_label_text = cur_target.value.clone();
                                }

                                set_clipboard_text(&full_label_text);

                                let vocal_read = if !full_label_text.is_empty() {
                                    full_label_text.clone()
                                } else {
                                    t!("commands.empty_label").to_string()
                                };

                                self.chat_history.push(ChatEntry {
                                    role: ChatRole::Agent,
                                    text: format!("🏷️ [{}] : {}", t!("roles.label"), vocal_read),
                                    timestamp: current_time_str(),
                                    quick_suggestions: Vec::new(),
                                    screen_size_kb: None,
                                });

                                let _ = self.tts_sender.send(TtsCommand::Stop);
                                let _ = self.tts_sender.send(TtsCommand::Speak(vocal_read));
                                ctx.request_repaint();
                            } else if cur_target.role == "champ de saisie" || cur_target.role == "zone de texte" {
                                let click_x = cur_target.click_x;
                                let click_y = cur_target.click_y;
                                let target_name = if !cur_target.name.is_empty() { cur_target.name.clone() } else { cur_target.role.clone() };
                                let host_window_id = cur_target.window_id.or(self.switch_active_window_id);

                                #[cfg(windows)]
                                let mut uia_elem = None;

                                #[cfg(windows)]
                                {
                                    if let Some(wid) = host_window_id {
                                        let h = windows::Win32::Foundation::HWND(wid as _);
                                        if !is_window_hung(h) {
                                            force_foreground_window(h);
                                            restore_if_iconic(h);
                                        }
                                    }
                                    let pt = windows::Win32::Foundation::POINT { x: click_x, y: click_y };
                                    unsafe {
                                        let target_child = windows::Win32::UI::WindowsAndMessaging::WindowFromPoint(pt);
                                        if !target_child.is_invalid() {
                                            let root = windows::Win32::UI::WindowsAndMessaging::GetAncestor(
                                                target_child,
                                                windows::Win32::UI::WindowsAndMessaging::GA_ROOT,
                                            );
                                            let top_hwnd = if !root.is_invalid() { root } else { target_child };
                                            force_foreground_window(top_hwnd);
                                            restore_if_iconic(top_hwnd);
                                        }
                                        let _ = windows::Win32::UI::WindowsAndMessaging::SetCursorPos(click_x, click_y);
                                    }
                                    std::thread::sleep(Duration::from_millis(40));

                                    unsafe {
                                        let _ = windows::Win32::System::Com::CoInitializeEx(None, windows::Win32::System::Com::COINIT_MULTITHREADED);
                                        if let Ok(automation) = windows::Win32::System::Com::CoCreateInstance::<_, windows::Win32::UI::Accessibility::IUIAutomation>(
                                            &windows::Win32::UI::Accessibility::CUIAutomation,
                                            None,
                                            windows::Win32::System::Com::CLSCTX_INPROC_SERVER,
                                        ) {
                                            if let Ok(elem) = automation.ElementFromPoint(pt) {
                                                let is_focusable = elem.CurrentIsKeyboardFocusable().map(|b| b.as_bool()).unwrap_or(false);
                                                if is_focusable {
                                                    let _ = elem.SetFocus();
                                                }
                                                uia_elem = Some(elem);
                                            }
                                        }
                                    }
                                    ui::click_element(click_x, click_y, uia_elem.as_ref(), None);
                                }
                                #[cfg(not(windows))]
                                {
                                    ui::click_element(click_x, click_y, None, None);
                                }

                                let mut current_val = cur_target.value.clone();
                                #[cfg(windows)]
                                if let Some(ref elem) = uia_elem {
                                    if let Ok(p) = unsafe { elem.GetCurrentPattern(windows::Win32::UI::Accessibility::UIA_ValuePatternId) } {
                                        if let Ok(vp) = p.cast::<windows::Win32::UI::Accessibility::IUIAutomationValuePattern>() {
                                            if let Ok(v) = unsafe { vp.CurrentValue() } {
                                                let s = v.to_string();
                                                if !s.trim().is_empty() {
                                                    current_val = s.trim().to_string();
                                                }
                                            }
                                        }
                                    }
                                    if current_val.is_empty() {
                                        if let Ok(p) = unsafe { elem.GetCurrentPattern(windows::Win32::UI::Accessibility::UIA_TextPatternId) } {
                                            if let Ok(tp) = p.cast::<windows::Win32::UI::Accessibility::IUIAutomationTextPattern>() {
                                                if let Ok(range) = unsafe { tp.DocumentRange() } {
                                                    if let Ok(bstr) = unsafe { range.GetText(1000) } {
                                                        let s = bstr.to_string();
                                                        if !s.trim().is_empty() {
                                                            current_val = s.trim().to_string();
                                                        }
                                                    }
                                                }
                                            }
                                        }
                                    }
                                }
                                #[cfg(not(windows))]
                                if current_val.is_empty() {
                                    if let Some(txt) = get_active_field_content() {
                                        current_val = txt;
                                    }
                                }

                                let dict_msg = t!("commands.dictation_activated").to_string();

                                let vocal_read = if !current_val.is_empty() {
                                    t!("commands.field_with_value", name = target_name, value = current_val, dict = dict_msg).to_string()
                                } else {
                                    t!("commands.field_empty", name = target_name, dict = dict_msg).to_string()
                                };

                                self.dictation_mode = true;
                                IS_DICTATION_ACTIVE.store(true, Ordering::SeqCst);
                                self.continuous_mode = true;

                                if let Ok(mut guard) = LAST_FOCUSED_RECT.lock() {
                                    *guard = Some(cur_target.rect);
                                }
                                if let Some(wid) = host_window_id {
                                    if let Ok(mut lock) = LAST_EXTERNAL_HWND.lock() {
                                        *lock = Some(wid);
                                    }
                                }

                                self.chat_history.push(ChatEntry {
                                    role: ChatRole::Agent,
                                    text: format!("✍️ [{}] {} :\n{}\n\n*({})*", cur_target.role, target_name, if current_val.is_empty() { t!("commands.empty_value").to_string() } else { current_val }, dict_msg),
                                    timestamp: current_time_str(),
                                    quick_suggestions: vec![
                                        QuickSuggestionItem::Text(t!("commands.disable_dictation").to_string()),
                                        QuickSuggestionItem::Text(t!("commands.clear_text").to_string()),
                                    ],
                                    screen_size_kb: None,
                                });

                                let _ = self.tts_sender.send(TtsCommand::Stop);
                                let _ = self.tts_sender.send(TtsCommand::Speak(vocal_read));
                                ctx.request_repaint();
                            } else {
                            let cur_target = &self.numbered_targets[self.switch_focus_index % num_targets];
                            let click_x = cur_target.click_x;
                            let click_y = cur_target.click_y;
                            let target_name = cur_target.name.clone();
                            let host_window_id = cur_target.window_id.or(self.switch_active_window_id);

                            #[cfg(windows)]
                            let mut target_top_hwnd = windows::Win32::Foundation::HWND(std::ptr::null_mut());

                            #[cfg(windows)]
                            {
                                if let Some(wid) = host_window_id {
                                    let h = windows::Win32::Foundation::HWND(wid as _);
                                    if !is_window_hung(h) {
                                        target_top_hwnd = h;
                                        force_foreground_window(h);
                                        restore_if_iconic(h);
                                    }
                                }
                                let pt = windows::Win32::Foundation::POINT { x: click_x, y: click_y };
                                unsafe {
                                    let target_child = windows::Win32::UI::WindowsAndMessaging::WindowFromPoint(pt);
                                    if !target_child.is_invalid() {
                                        let root = windows::Win32::UI::WindowsAndMessaging::GetAncestor(
                                            target_child,
                                            windows::Win32::UI::WindowsAndMessaging::GA_ROOT,
                                        );
                                        let top_hwnd = if !root.is_invalid() { root } else { target_child };
                                        if target_top_hwnd.0.is_null() {
                                            target_top_hwnd = top_hwnd;
                                            force_foreground_window(top_hwnd);
                                            restore_if_iconic(top_hwnd);
                                        }
                                    }
                                    let _ = windows::Win32::UI::WindowsAndMessaging::SetCursorPos(click_x, click_y);
                                }
                                std::thread::sleep(Duration::from_millis(40));

                                let mut uia_elem = None;
                                unsafe {
                                    let _ = windows::Win32::System::Com::CoInitializeEx(None, windows::Win32::System::Com::COINIT_MULTITHREADED);
                                    if let Ok(automation) = windows::Win32::System::Com::CoCreateInstance::<_, windows::Win32::UI::Accessibility::IUIAutomation>(
                                        &windows::Win32::UI::Accessibility::CUIAutomation,
                                        None,
                                        windows::Win32::System::Com::CLSCTX_INPROC_SERVER,
                                    ) {
                                        if let Ok(elem) = automation.ElementFromPoint(pt) {
                                            uia_elem = Some(elem);
                                        }
                                    }
                                }
                                ui::click_element(click_x, click_y, uia_elem.as_ref(), None);
                            }
                            #[cfg(not(windows))]
                            {
                                ui::click_element(click_x, click_y, None, None);
                            }

                            let action_feedback = t!("commands.action_feedback", name = target_name).to_string();
                            let _ = self.tts_sender.send(TtsCommand::Stop);
                            let _ = self.tts_sender.send(TtsCommand::Speak(action_feedback));

                            std::thread::sleep(Duration::from_millis(180));

                            let old_targets = self.numbered_targets.clone();
                            let mut context_updated = false;

                            // 1. Détection de l'apparition d'une nouvelle fenêtre (boîte de dialogue, modale, etc.)
                            #[cfg(windows)]
                            {
                                let cur_fg = unsafe { windows::Win32::UI::WindowsAndMessaging::GetForegroundWindow() };
                                let is_new_window = !cur_fg.0.is_null()
                                    && !target_top_hwnd.0.is_null()
                                    && cur_fg != target_top_hwnd
                                    && !is_window_hung(cur_fg);

                                if is_new_window {
                                    let win_title = get_window_title(cur_fg);
                                    if !win_title.is_empty() && !win_title.to_lowercase().contains("libertide") {
                                        let new_win_targets = collect_numbered_targets_for_window(cur_fg);
                                        if !new_win_targets.is_empty() {
                                            self.switch_windows = collect_switch_candidate_windows();
                                            self.switch_active_window_id = Some(cur_fg.0 as isize);
                                            self.switch_zones.clear();
                                            self.switch_active_zone_index = None;
                                            self.numbered_targets = new_win_targets;
                                            self.switch_focus_index = 0;

                                            let mut r = windows::Win32::Foundation::RECT::default();
                                            let _ = unsafe { windows::Win32::UI::WindowsAndMessaging::GetWindowRect(cur_fg, &mut r) };
                                            draw_screen_orange_border([r.left, r.top, r.right, r.bottom]);

                                            if let Some(first) = self.numbered_targets.first() {
                                                focus_element_target(first);
                                                self.hover_last_cursor_pos = Some((first.click_x, first.click_y));
                                                self.hover_cursor_still_since = Instant::now();
                                                self.hover_last_announced_target = Some(first.index);
                                                let win_announcement = format!("{win_title}. {}", first.name);
                                                let _ = self.tts_sender.send(TtsCommand::Stop);
                                                let _ = self.tts_sender.send(TtsCommand::Speak(win_announcement));
                                            }
                                            context_updated = true;
                                            ctx.request_repaint();
                                        }
                                    }
                                }
                            }

                            // 2 & 3. Si aucune nouvelle fenêtre, analyser la fenêtre pour de nouveaux éléments (menus contextuels, popups)
                            if !context_updated {
                                #[cfg(windows)]
                                let all_window_targets = if !target_top_hwnd.0.is_null() {
                                    collect_numbered_targets_for_window(target_top_hwnd)
                                } else {
                                    collect_numbered_targets_for_active_window()
                                };

                                #[cfg(not(windows))]
                                let all_window_targets = collect_numbered_targets_for_active_window();

                                let mut new_popped_targets: Vec<NumberedElementTarget> = all_window_targets
                                    .iter()
                                    .filter(|ft| {
                                        !old_targets.iter().any(|ot| {
                                            (ot.click_x - ft.click_x).abs() <= 6
                                                && (ot.click_y - ft.click_y).abs() <= 6
                                                && ot.name == ft.name
                                        })
                                    })
                                    .cloned()
                                    .collect();

                                if !new_popped_targets.is_empty() {
                                    for (i, t) in new_popped_targets.iter_mut().enumerate() {
                                        t.index = i + 1;
                                    }
                                    self.numbered_targets = new_popped_targets;
                                    self.switch_focus_index = 0;
                                    if let Some(first) = self.numbered_targets.first() {
                                        focus_element_target(first);
                                        self.hover_last_cursor_pos = Some((first.click_x, first.click_y));
                                        self.hover_cursor_still_since = Instant::now();
                                        self.hover_last_announced_target = Some(first.index);
                                        let menu_announcement = t!("commands.menu_feedback", name = first.name).to_string();
                                        let _ = self.tts_sender.send(TtsCommand::Stop);
                                        let _ = self.tts_sender.send(TtsCommand::Speak(menu_announcement));
                                    }
                                    context_updated = true;
                                    ctx.request_repaint();
                                }
                            }

                            // Repli : mise à jour des éléments de la zone actuelle si le contenu a changé sans popup
                            if !context_updated {
                                #[cfg(windows)]
                                let all_window_targets = if !target_top_hwnd.0.is_null() {
                                    collect_numbered_targets_for_window(target_top_hwnd)
                                } else {
                                    collect_numbered_targets_for_active_window()
                                };
                                #[cfg(not(windows))]
                                let all_window_targets = collect_numbered_targets_for_active_window();

                                let (z_min_x, z_min_y, z_max_x, z_max_y) = if let Some(z_idx) = self.switch_active_zone_index {
                                    if let Some(zone) = self.switch_zones.get(z_idx) {
                                        (
                                            zone.rect[0] - 25,
                                            zone.rect[1] - 25,
                                            zone.rect[2] + 40,
                                            zone.rect[3] + 250,
                                        )
                                    } else {
                                        (click_x - 300, click_y - 100, click_x + 300, click_y + 400)
                                    }
                                } else {
                                    (click_x - 300, click_y - 100, click_x + 300, click_y + 400)
                                };

                                let zone_targets: Vec<NumberedElementTarget> = all_window_targets
                                    .into_iter()
                                    .filter(|t| {
                                        t.click_x >= z_min_x
                                            && t.click_x <= z_max_x
                                            && t.click_y >= z_min_y
                                            && t.click_y <= z_max_y
                                    })
                                    .collect();

                                if !zone_targets.is_empty() && zone_targets != old_targets {
                                    let mut updated = zone_targets;
                                    for (i, t) in updated.iter_mut().enumerate() {
                                        t.index = i + 1;
                                    }
                                    self.numbered_targets = updated;
                                }
                            }
                            }
                        }
                    } else if let Some(last_entry) = self.chat_history.last() {
                        let sug_count = last_entry.quick_suggestions.len();
                        if sug_count > 0 {
                            if nav_next_hit || nav_prev_hit {
                                if nav_prev_hit {
                                    self.switch_focus_index = if self.switch_focus_index == 0 { sug_count - 1 } else { self.switch_focus_index - 1 };
                                } else {
                                    self.switch_focus_index = (self.switch_focus_index + 1) % sug_count;
                                }
                                self.last_switch_input = Instant::now();
                                self.switch_auto_scroll = false;
                                if let Some(text) = last_entry.quick_suggestions[self.switch_focus_index % sug_count].to_display_string() {
                                    let _ = self.tts_sender.send(TtsCommand::Stop);
                                    let _ = self.tts_sender.send(TtsCommand::Speak(text));
                                }
                                ctx.request_repaint();
                            }
                            if switch_input_hit {
                                self.last_switch_input = Instant::now();
                                self.switch_auto_scroll = false;
                                play_earcon(Earcon::Success);
                            }
                        }
                    }
                }
            }

            // Traitement du survol physique de la souris avec anti-rebond (150-250 ms)
            let physical_cursor = get_global_cursor_pos();

            if let Some((cur_x, cur_y)) = physical_cursor {
                let is_over_overlay = {
                    #[cfg(windows)]
                    {
                        let overlay_hwnd = unsafe {
                            windows::Win32::UI::WindowsAndMessaging::FindWindowW(None, windows::core::w!("Libertide overlay")).unwrap_or(windows::Win32::Foundation::HWND(std::ptr::null_mut()))
                        };
                        if !overlay_hwnd.0.is_null() {
                            let mut r = windows::Win32::Foundation::RECT::default();
                            if unsafe { windows::Win32::UI::WindowsAndMessaging::GetWindowRect(overlay_hwnd, &mut r).is_ok() } {
                                cur_x >= r.left && cur_x <= r.right && cur_y >= r.top && cur_y <= r.bottom
                            } else {
                                false
                            }
                        } else {
                            false
                        }
                    }
                    #[cfg(not(windows))]
                    {
                        if let Some(pos) = self.window_pos {
                            let ppp = ctx.pixels_per_point().max(0.5);
                            let rx = pos.x * ppp;
                            let ry = pos.y * ppp;
                            let rw = self.current_win_w * ppp;
                            let rh = self.current_win_h * ppp;
                            (cur_x as f32) >= rx && (cur_x as f32) <= rx + rw && (cur_y as f32) >= ry && (cur_y as f32) <= ry + rh
                        } else {
                            false
                        }
                    }
                };

                let has_moved = match self.hover_last_cursor_pos {
                    Some((lx, ly)) => (lx - cur_x).abs() > 4 || (ly - cur_y).abs() > 4,
                    None => false,
                };

                let is_recent_switch_nav = self.last_switch_input.elapsed() < Duration::from_millis(500);

                if is_over_overlay {
                    self.hover_physical_mouse_moved = false;
                    self.hover_pending_speech = None;
                    self.hover_speech_deadline = None;
                } else if has_moved && !is_recent_switch_nav {
                    self.hover_physical_mouse_moved = true;
                    self.hover_cursor_still_since = Instant::now();
                    self.hover_last_cursor_pos = Some((cur_x, cur_y));
                    self.hover_pending_speech = None;
                    self.hover_speech_deadline = None;

                    // Détection et bascule immédiate vers la fenêtre survolée en cours de mouvement
                    if !self.switch_windows.is_empty() {
                        if let Some(w_idx) = find_switch_window_at_point(&self.switch_windows, cur_x, cur_y) {
                            let hovered_win = &self.switch_windows[w_idx];
                            if self.switch_active_window_id != Some(hovered_win.id) {
                                self.switch_active_window_id = Some(hovered_win.id);
                                self.switch_focus_index = w_idx;
                                #[cfg(windows)]
                                {
                                    let hwnd = windows::Win32::Foundation::HWND(hovered_win.id as _);
                                    if !is_window_hung(hwnd) {
                                        force_foreground_window(hwnd);
                                        restore_if_iconic(hwnd);
                                    }
                                    draw_screen_orange_border(hovered_win.rect);
                                }
                                #[cfg(not(windows))]
                                {
                                    force_foreground_window(&hovered_win.title);
                                    restore_if_iconic(&hovered_win.title);
                                }
                                // Rafraîchissement direct de l'OCR et des badges de la nouvelle fenêtre survolée
                                self.refresh_numbered_targets_for_window(hovered_win.id);
                            }
                        }
                    }
                } else if !is_over_overlay
                    && self.hover_physical_mouse_moved
                    && !is_recent_switch_nav
                    && self.hover_cursor_still_since.elapsed() >= Duration::from_millis(200) {
                    // Le curseur est immobilisé depuis au moins 200 ms : annonce détaillée selon l'étape
                    match self.switch_step {
                        SwitchAccessStep::Elements => {
                            if self.hover_controls_speech_enabled && !self.numbered_targets.is_empty() {
                                let num_targets = self.numbered_targets.len();
                                let current_contains_cursor = self.numbered_targets
                                    .get(self.switch_focus_index % num_targets)
                                    .map_or(false, |t| {
                                        cur_x >= t.rect[0] && cur_x <= t.rect[2] && cur_y >= t.rect[1] && cur_y <= t.rect[3]
                                    });

                                let hovered_pos_opt = if current_contains_cursor {
                                    Some(self.switch_focus_index % num_targets)
                                } else {
                                    self.numbered_targets
                                        .iter()
                                        .enumerate()
                                        .filter(|(_, t)| {
                                            cur_x >= t.rect[0] && cur_x <= t.rect[2] && cur_y >= t.rect[1] && cur_y <= t.rect[3]
                                        })
                                        .min_by_key(|(_, t)| {
                                            let w = (t.rect[2] - t.rect[0]).max(1) as i64;
                                            let h = (t.rect[3] - t.rect[1]).max(1) as i64;
                                            w * h
                                        })
                                        .map(|(idx, _)| idx)
                                };

                                if let Some(pos) = hovered_pos_opt {
                                    if let Some(target) = self.numbered_targets.get(pos) {
                                        if self.hover_last_announced_target != Some(target.index) {
                                            let parent_zone = self.switch_zones.iter().find(|z| {
                                                cur_x >= z.rect[0] && cur_x <= z.rect[2] && cur_y >= z.rect[1] && cur_y <= z.rect[3]
                                            }).or_else(|| {
                                                self.switch_active_zone_index.and_then(|idx| self.switch_zones.get(idx))
                                            });

                                            let zone_title = parent_zone.map(|z| z.title.as_str());
                                            let index_in_zone = parent_zone.and_then(|z| {
                                                z.targets.iter().position(|t| t.index == target.index).map(|p| p + 1)
                                            }).or(Some(pos + 1));
                                            let is_same_zone = self.hover_last_announced_zone.as_deref() == zone_title;

                                            let announcement = build_targeted_hover_speech(
                                                zone_title,
                                                parent_zone.map_or(&self.numbered_targets, |z| &z.targets),
                                                index_in_zone.unwrap_or(pos + 1),
                                                &target.name,
                                                &target.role,
                                                &target.shortcut,
                                                is_same_zone,
                                            );

                                            self.switch_focus_index = pos;
                                            focus_element_target(target);
                                            #[cfg(windows)]
                                            draw_screen_control_border(target.rect, false);

                                            let _ = self.tts_sender.send(TtsCommand::Stop);
                                            let _ = self.tts_sender.send(TtsCommand::Speak(announcement));
                                            self.hover_last_announced_target = Some(target.index);
                                            self.hover_last_announced_zone = zone_title.map(|s| s.to_string());
                                            ctx.request_repaint();
                                            self.hover_physical_mouse_moved = false;
                                        }
                                    }
                                }
                            }
                        }
                        SwitchAccessStep::Zones => {
                            if self.hover_controls_speech_enabled && !self.switch_zones.is_empty() {
                                let num_zones = self.switch_zones.len();
                                let current_contains_cursor = self.switch_zones
                                    .get(self.switch_focus_index % num_zones)
                                    .map_or(false, |z| {
                                        cur_x >= z.rect[0] && cur_x <= z.rect[2] && cur_y >= z.rect[1] && cur_y <= z.rect[3]
                                    });

                                let hovered_z_pos = if current_contains_cursor {
                                    Some(self.switch_focus_index % num_zones)
                                } else {
                                    self.switch_zones
                                        .iter()
                                        .enumerate()
                                        .filter(|(_, z)| {
                                            cur_x >= z.rect[0] && cur_x <= z.rect[2] && cur_y >= z.rect[1] && cur_y <= z.rect[3]
                                        })
                                        .min_by_key(|(_, z)| {
                                            let w = (z.rect[2] - z.rect[0]).max(1) as i64;
                                            let h = (z.rect[3] - z.rect[1]).max(1) as i64;
                                            w * h
                                        })
                                        .map(|(idx, _)| idx)
                                };
                                if let Some(z_pos) = hovered_z_pos {
                                    if let Some(cur_zone) = self.switch_zones.get(z_pos) {
                                        if self.hover_last_announced_zone.as_deref() != Some(&cur_zone.title) {
                                            #[cfg(windows)]
                                            if let Some(id) = self.switch_active_window_id {
                                                let hwnd = windows::Win32::Foundation::HWND(id as _);
                                                if !is_window_hung(hwnd) {
                                                    force_foreground_window(hwnd);
                                                    restore_if_iconic(hwnd);
                                                }
                                            }

                                            self.switch_focus_index = z_pos;
                                            #[cfg(windows)]
                                            draw_screen_control_border(cur_zone.rect, true);
                                            let msg = cur_zone.title.clone();
                                            let _ = self.tts_sender.send(TtsCommand::Stop);
                                            let _ = self.tts_sender.send(TtsCommand::Speak(msg));
                                            self.hover_last_announced_zone = Some(cur_zone.title.clone());
                                            ctx.request_repaint();
                                            self.hover_physical_mouse_moved = false;
                                        }
                                    }
                                }
                            }
                        }
                        SwitchAccessStep::Windows => {
                            if !self.switch_windows.is_empty() {
                                let hovered_win_idx = find_switch_window_at_point(&self.switch_windows, cur_x, cur_y);

                                if let Some(w_idx) = hovered_win_idx {
                                    let cur_win = self.switch_windows[w_idx].clone();
                                    let win_id = cur_win.id;
                                    let win_title = cur_win.title;
                                    let win_rect = cur_win.rect;

                                    if self.hover_last_announced_target != Some(win_id as usize) {
                                        self.switch_focus_index = w_idx;
                                        self.activate_window_and_display_first_zone(win_id, &win_title, win_rect, true);
                                        self.hover_last_announced_target = Some(win_id as usize);
                                        self.hover_last_announced_zone = Some(win_title);
                                        ctx.request_repaint();
                                        self.hover_physical_mouse_moved = false;
                                    }
                                }
                            }
                        }
                    }
                }
            }

            // Dispatch de la parole mise en attente (anti-rebond 180 ms)
            if let Some(deadline) = self.hover_speech_deadline {
                let now = Instant::now();
                if now >= deadline {
                    if let Some(speech) = self.hover_pending_speech.take() {
                        let _ = self.tts_sender.send(TtsCommand::Stop);
                        let _ = self.tts_sender.send(TtsCommand::Speak(speech));
                    }
                    self.hover_speech_deadline = None;
                } else {
                    ctx.request_repaint_after(deadline - now);
                }
            }
        }

        // Raccourci clavier 'R' pour basculer le micro (désactivé si l'utilisateur est en train de taper du texte)
        if !self.is_hidden && !is_typing && ctx.input(|i| i.key_pressed(egui::Key::R)) {
            self.toggle_recording(&ctx);
        }

        // Retranscription en temps réel : mise à jour du texte d'écoute
        if self.is_recording {
            let trimmed = self.input_text.trim();
            if !trimmed.is_empty() {
                self.live_transcript = trimmed.to_string();
            }
        }

        let time = ctx.input(|i| i.time);
        ctx.request_repaint_after(Duration::from_millis(16));

        if self.is_hidden {
            egui::CentralPanel::default()
                .frame(egui::Frame::new().fill(egui::Color32::TRANSPARENT))
                .show(ui, |_ui| {});
            return;
        }

        egui::CentralPanel::default()
            .frame(egui::Frame::new().fill(egui::Color32::TRANSPARENT).inner_margin(egui::Margin::ZERO))
            .show(ui, |ui| {
                // Fond et cadre stylisé de la boîte de discussion
                ui.painter().rect(
                    response_rect,
                    egui::CornerRadius::same(12),
                    theme.panel_bg,
                    egui::Stroke::new(1.0, theme.border_subtle),
                    egui::StrokeKind::Inside,
                );

                // 1. Barre d'onglets supérieure et boutons système de la fenêtre
                let mut tab_bar_max_x = response_rect.min.x + 210.0;

                let header_rect = egui::Rect::from_min_max(
                    egui::pos2(response_rect.min.x + 10.0, response_rect.min.y + 6.0),
                    egui::pos2(response_rect.max.x - 10.0, response_rect.min.y + 32.0),
                );

                ui.scope_builder(egui::UiBuilder::new().max_rect(header_rect), |ui| {
                    ui.horizontal(|ui| {
                        let is_assist_active = self.active_tab == ActiveTab::Assistance;
                        let tab_assist = ui.selectable_label(is_assist_active, t!("ui.assistance_tab"));
                        let tab_assist_id = ui.make_persistent_id("dwell_tab_assistance");
                        let assist_activated = if !is_assist_active {
                            apply_dwell_to_response(ui, &mut self.dwell_state, tab_assist_id, &tab_assist)
                        } else {
                            tab_assist.clicked()
                        };
                        if assist_activated {
                            self.active_tab = ActiveTab::Assistance;
                        }
                        let chat_title = if self.twitch_messages.is_empty() {
                            t!("ui.twitch_chat_tab").to_string()
                        } else {
                            t!("ui.twitch_chat_count", count = self.twitch_messages.len()).to_string()
                        };
                        let is_chat_active = self.active_tab == ActiveTab::Chat;
                        let tab_chat = ui.selectable_label(is_chat_active, chat_title);
                        let tab_chat_id = ui.make_persistent_id("dwell_tab_twitch_chat");
                        let chat_activated = if !is_chat_active {
                            apply_dwell_to_response(ui, &mut self.dwell_state, tab_chat_id, &tab_chat)
                        } else {
                            tab_chat.clicked()
                        };
                        if chat_activated {
                            self.active_tab = ActiveTab::Chat;
                        }
                        tab_bar_max_x = tab_assist.rect.max.x.max(tab_chat.rect.max.x);

                        // Boutons système en haut à droite : Réduire, Agrandir/Restaurer, Fermer
                        ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                            ui.spacing_mut().item_spacing.x = 4.0;

                            let close_btn = ui.add(
                                egui::Button::new(
                                    egui::RichText::new("×")
                                        .size(13.0)
                                        .color(egui::Color32::from_rgb(235, 235, 245)),
                                )
                                .min_size(egui::vec2(20.0, 18.0))
                                .corner_radius(egui::CornerRadius::same(4)),
                            );
                            if close_btn.on_hover_text(t!("ui.btn_close")).clicked() {
                                ctx.send_viewport_cmd(egui::ViewportCommand::Close);
                            }

                            let max_icon = if is_maximized { "⧉" } else { "□" };
                            let max_btn = ui.add(
                                egui::Button::new(
                                    egui::RichText::new(max_icon)
                                        .size(10.5)
                                        .color(egui::Color32::from_rgb(200, 215, 230)),
                                )
                                .min_size(egui::vec2(20.0, 18.0))
                                .corner_radius(egui::CornerRadius::same(4)),
                            );
                            if max_btn.on_hover_text(if is_maximized { t!("ui.btn_restore") } else { t!("ui.btn_maximize") }).clicked() {
                                ctx.send_viewport_cmd(egui::ViewportCommand::Maximized(!is_maximized));
                            }

                            let min_btn = ui.add(
                                egui::Button::new(
                                    egui::RichText::new("—")
                                        .size(10.5)
                                        .color(egui::Color32::from_rgb(200, 215, 230)),
                                )
                                .min_size(egui::vec2(20.0, 18.0))
                                .corner_radius(egui::CornerRadius::same(4)),
                            );
                            if min_btn.on_hover_text(t!("ui.btn_minimize")).clicked() {
                                ctx.send_viewport_cmd(egui::ViewportCommand::Minimized(true));
                            }
                        });
                    });
                });

                let sep_y = response_rect.min.y + 35.0;
                ui.painter().hline(
                    (response_rect.min.x + 8.0)..=(response_rect.max.x - 8.0),
                    sep_y,
                    egui::Stroke::new(1.0, theme.border_subtle),
                );

                // 2. Zone inférieure évaluée en deuxième (contrôles, avatar et saisie) pour précéder les messages au focus Tab
                ui.scope_builder(egui::UiBuilder::new().max_rect(control_rect), |ui| {
                    egui::Frame::new()
                        .fill(theme.control_bg)
                        .stroke(egui::Stroke::new(1.0, theme.border))
                        .corner_radius(egui::CornerRadius::same(12))
                        .inner_margin(egui::Margin::symmetric(10, 8))
                        .show(ui, |ui| {
                            let inner_rect = ui.available_rect_before_wrap();
                            let avatar_pos = egui::pos2(inner_rect.max.x - 20.0, inner_rect.center().y);
                            self.draw_avatar(ui, avatar_pos, 14.0, time);

                            let avatar_rect = egui::Rect::from_center_size(avatar_pos, egui::vec2(32.0, 32.0));
                            let avatar_resp = ui.interact(avatar_rect, egui::Id::new("avatar_interact"), egui::Sense::click());
                            if self.status == AgentStatus::Speaking {
                                if avatar_resp.hovered() {
                                    ui.ctx().set_cursor_icon(egui::CursorIcon::PointingHand);
                                }
                                if avatar_resp.on_hover_text(t!("ui.avatar_mute_tooltip")).clicked() {
                                    self.stop_speaking();
                                }
                            }

                            let content_area = egui::Rect::from_min_max(
                                inner_rect.min,
                                egui::pos2(inner_rect.max.x - 48.0, inner_rect.max.y),
                            );

                            ui.scope_builder(egui::UiBuilder::new().max_rect(content_area), |ui| {
                                ui.vertical(|ui| {
                                    let prov_name = self.current_agent_model_display_name();
                                    let status_badge = if self.dictation_mode {
                                        t!("ui.dictation_badge").to_string()
                                    } else if self.switch_access_mode {
                                        if self.switch_step == SwitchAccessStep::Windows {
                                            if !self.switch_windows.is_empty() {
                                                let cur = &self.switch_windows[self.switch_focus_index % self.switch_windows.len()];
                                                t!("ui.select_window_hint", index = self.switch_focus_index + 1, title = cur.title).to_string()
                                            } else {
                                                t!("ui.searching_windows").to_string()
                                            }
                                        } else if self.switch_step == SwitchAccessStep::Zones {
                                            if !self.switch_zones.is_empty() {
                                                let cur = &self.switch_zones[self.switch_focus_index % self.switch_zones.len()];
                                                t!("ui.open_zone_hint", index = self.switch_focus_index + 1, title = cur.title).to_string()
                                            } else {
                                                t!("ui.analyzing_zones").to_string()
                                            }
                                        } else if self.switch_step == SwitchAccessStep::Elements {
                                            let buffer_feedback = if !self.numbered_input_buffer.is_empty() {
                                                t!("ui.keyboard_input_buffer", buffer = self.numbered_input_buffer).to_string()
                                            } else {
                                                String::new()
                                            };
                                            if let Some(target) = self.numbered_targets.get(self.switch_focus_index) {
                                                t!("ui.target_status_item", provider = prov_name, buffer = buffer_feedback, index = target.index, name = target.name, count = self.numbered_targets.len()).to_string()
                                            } else {
                                                t!("ui.target_status_active", provider = prov_name, buffer = buffer_feedback, count = self.numbered_targets.len()).to_string()
                                            }
                                        } else {
                                            t!("ui.detecting_controls").to_string()
                                        }
                                    } else {
                                        match self.status {
                                            AgentStatus::Idle => t!("ui.status_idle", provider = prov_name).to_string(),
                                            AgentStatus::Thinking => t!("ui.status_thinking", provider = prov_name).to_string(),
                                            AgentStatus::Speaking => t!("ui.status_speaking", provider = prov_name).to_string(),
                                            AgentStatus::Listening => String::new(),
                                            AgentStatus::EmergencyStopped => t!("ui.status_locked").to_string(),
                                        }
                                    };

                                    let badge_color = if self.dictation_mode {
                                        egui::Color32::from_rgb(255, 205, 70)
                                    } else if self.switch_access_mode {
                                        egui::Color32::from_rgb(255, 140, 0)
                                    } else if self.status == AgentStatus::Listening {
                                        egui::Color32::from_rgb(255, 80, 80)
                                    } else {
                                        theme.accent
                                    };

                                    if !status_badge.is_empty() {
                                        let badge_resp = ui.add(
                                            egui::Label::new(
                                                egui::RichText::new(&status_badge)
                                                    .size(10.5)
                                                    .color(badge_color),
                                            )
                                            .truncate()
                                            .sense(if self.status == AgentStatus::Speaking || self.switch_access_mode { egui::Sense::click() } else { egui::Sense::hover() }),
                                        );
                                        if self.status == AgentStatus::Speaking {
                                            if badge_resp.hovered() {
                                                ui.ctx().set_cursor_icon(egui::CursorIcon::PointingHand);
                                            }
                                            if badge_resp.on_hover_text(t!("ui.avatar_mute_tooltip")).clicked() {
                                                self.stop_speaking();
                                            }
                                        } else if self.switch_access_mode {
                                            if badge_resp.hovered() {
                                                ui.ctx().set_cursor_icon(egui::CursorIcon::PointingHand);
                                            }
                                            if badge_resp.on_hover_text(t!("ui.badge_disable_hover_tooltip")).clicked() {
                                                self.toggle_switch_access_mode(Some(false));
                                            }
                                        } else {
                                            badge_resp.on_hover_text(&status_badge);
                                        }
                                        ui.add_space(2.0);
                                    }

                                    if self.is_recording && !self.live_transcript.is_empty() {
                                        ui.add(
                                            egui::Label::new(
                                                egui::RichText::new(&self.live_transcript)
                                                    .size(11.5)
                                                    .color(egui::Color32::from_rgb(255, 220, 130)),
                                            )
                                            .truncate(),
                                        );
                                        ui.add_space(2.0);
                                    }

                                    ui.add_space(4.0);

                                    // Barre de suggestions d'applications (PATH / Levenshtein)
                                    let mut clicked_app_suggestion: Option<String> = None;
                                    let mut autocomplete_input_to: Option<String> = None;

                                    if !self.input_suggestions.is_empty() {
                                        ui.horizontal(|ui| {
                                            ui.spacing_mut().item_spacing.x = 4.0;
                                            ui.label(
                                                egui::RichText::new(t!("ui.app_suggestions_header"))
                                                    .size(10.5)
                                                    .color(theme.accent)
                                                    .strong(),
                                            );
                                            for sug in &self.input_suggestions {
                                                let chip = egui::Button::new(
                                                    egui::RichText::new(format!("🚀 {sug}"))
                                                        .size(10.5)
                                                        .color(egui::Color32::from_rgb(220, 240, 255)),
                                                )
                                                .fill(egui::Color32::from_rgba_unmultiplied(0, 180, 255, 30))
                                                .stroke(egui::Stroke::new(1.0, theme.border_subtle))
                                                .corner_radius(egui::CornerRadius::same(6))
                                                .small();

                                                let resp = ui.add(chip)
                                                    .on_hover_text(t!("ui.app_suggestion_tooltip", app = sug));
                                                let sug_dwell_id = ui.make_persistent_id(format!("dwell_app_sug_{sug}"));
                                                if apply_dwell_to_response(ui, &mut self.dwell_state, sug_dwell_id, &resp) {
                                                    clicked_app_suggestion = Some(t!("ui.open_app_cmd", app = sug).to_string());
                                                }
                                                resp.context_menu(|ui| {
                                                    if ui.button(t!("ui.autocomplete_field")).clicked() {
                                                        autocomplete_input_to = Some(t!("ui.open_app_cmd", app = sug).to_string());
                                                        ui.close();
                                                    }
                                                });
                                            }
                                        });
                                        ui.add_space(2.0);
                                    }

                                    // 1ère rangée : Champ de saisie occupant 100% de la largeur disponible
                                    let mut enter_hit = false;
                                    ui.horizontal(|ui| {
                                        let edit_width = ui.available_width().max(60.0);
                                        let input_id = egui::Id::new("prompt_input_text");
                                        let is_input_focused = ctx.memory(|m| m.has_focus(input_id));
                                        let is_modal_open = self.show_api_key_modal;
                                        let is_other_typing = ctx.memory(|m| {
                                            m.has_focus(egui::Id::new("api_key_input_field"))
                                                || m.has_focus(egui::Id::new("model_input_field"))
                                        });

                                        // Autocomplétion de la première suggestion via la touche Tab
                                        if let Some(top_sug) = self.input_suggestions.first() {
                                            if is_input_focused && ctx.input(|i| i.key_pressed(egui::Key::Tab)) {
                                                autocomplete_input_to = Some(t!("ui.open_app_cmd", app = top_sug).to_string());
                                                ctx.input_mut(|i| i.consume_key(egui::Modifiers::NONE, egui::Key::Tab));
                                            }
                                        }

                                        let paste_hit = ctx.input(|i| {
                                            i.events.iter().any(|e| matches!(e, egui::Event::Paste(_)))
                                                || ((i.modifiers.ctrl || i.modifiers.command) && i.key_pressed(egui::Key::V))
                                                || (i.modifiers.shift && i.key_pressed(egui::Key::Insert))
                                        });

                                        let mut pending_paste_fallback: Option<String> = None;

                                        // Traitement du collage avant que TextEdit ne consomme la touche Ctrl+V
                                        if paste_hit && !is_modal_open && !is_other_typing {
                                            let clip_opt = get_clipboard_text()
                                                .or_else(get_plain_clipboard_text)
                                                .or_else(|| ctx.input(|i| {
                                                    i.events.iter().find_map(|e| {
                                                        if let egui::Event::Paste(text) = e {
                                                            Some(text.clone())
                                                        } else {
                                                            None
                                                        }
                                                    })
                                                }));

                                            if let Some(clip) = clip_opt {
                                                let clean_clip = clip.replace(['\r', '\n'], " ");
                                                let trimmed_clean = clean_clip.trim().to_string();
                                                if !trimmed_clean.is_empty() {
                                                    self.stop_speaking();
                                                    play_earcon(Earcon::Success);

                                                    if is_input_focused {
                                                        pending_paste_fallback = Some(trimmed_clean.clone());
                                                        ctx.input_mut(|i| {
                                                            i.events.retain(|e| !matches!(e, egui::Event::Paste(_)));
                                                            i.events.push(egui::Event::Paste(trimmed_clean.clone()));
                                                        });
                                                    } else {
                                                        self.input_text = trimmed_clean;
                                                        ctx.memory_mut(|m| m.request_focus(input_id));
                                                    }
                                                    ctx.request_repaint();
                                                }
                                            }

                                            ctx.input_mut(|i| i.consume_key(egui::Modifiers::COMMAND, egui::Key::V));
                                            ctx.input_mut(|i| i.consume_key(egui::Modifiers::CTRL, egui::Key::V));
                                        }

                                        let prev_text = self.input_text.clone();
                                        let response = ui.add_sized(
                                            [edit_width, 54.0],
                                            egui::TextEdit::multiline(&mut self.input_text)
                                                .desired_rows(3)
                                                .id(input_id)
                                                .hint_text(if self.is_recording {
                                                    t!("ui.prompt_hint_listening")
                                                } else {
                                                    t!("ui.prompt_hint_idle")
                                                }),
                                        );

                                        if let Some(pasted) = pending_paste_fallback {
                                            if self.input_text == prev_text {
                                                self.input_text = pasted;
                                                ctx.request_repaint();
                                            }
                                        }

                                        response.context_menu(|ui| {
                                            if ui.button(t!("ui.ctx_paste")).clicked() {
                                                if let Some(clip) = get_clipboard_text().or_else(get_plain_clipboard_text) {
                                                    let clean_clip = clip.replace(['\r', '\n'], " ");
                                                    let trimmed_clean = clean_clip.trim().to_string();
                                                    if !trimmed_clean.is_empty() {
                                                        self.input_text = trimmed_clean;
                                                        self.stop_speaking();
                                                        play_earcon(Earcon::Success);
                                                    }
                                                }
                                                ui.memory_mut(|m| m.request_focus(input_id));
                                                ui.close();
                                            }
                                            if ui.button(t!("ui.ctx_copy_all")).clicked() {
                                                set_clipboard_text(&self.input_text);
                                                ctx.copy_text(self.input_text.clone());
                                                play_earcon(Earcon::Success);
                                                ui.close();
                                            }
                                            if ui.button(t!("ui.ctx_cut_all")).clicked() {
                                                set_clipboard_text(&self.input_text);
                                                ctx.copy_text(self.input_text.clone());
                                                self.input_text.clear();
                                                play_earcon(Earcon::Success);
                                                ui.close();
                                            }
                                            if ui.button(t!("ui.ctx_clear")).clicked() {
                                                self.input_text.clear();
                                                ui.close();
                                            }
                                        });

                                        let copy_hit = ctx.input(|i| {
                                            i.events.iter().any(|e| matches!(e, egui::Event::Copy))
                                                || ((i.modifiers.ctrl || i.modifiers.command) && i.key_pressed(egui::Key::C))
                                        });
                                        if (response.has_focus() || is_input_focused) && copy_hit && !self.input_text.is_empty() {
                                            set_clipboard_text(&self.input_text);
                                            ctx.copy_text(self.input_text.clone());
                                            play_earcon(Earcon::Success);
                                        }

                                        let cut_hit = ctx.input(|i| {
                                            i.events.iter().any(|e| matches!(e, egui::Event::Cut))
                                                || ((i.modifiers.ctrl || i.modifiers.command) && i.key_pressed(egui::Key::X))
                                        });
                                        if (response.has_focus() || is_input_focused) && cut_hit && !self.input_text.is_empty() {
                                            set_clipboard_text(&self.input_text);
                                            ctx.copy_text(self.input_text.clone());
                                            self.input_text.clear();
                                            play_earcon(Earcon::Success);
                                        }

                                        if response.changed() {
                                            self.stop_speaking();
                                        }

                                        if let Some(completed) = autocomplete_input_to {
                                            self.input_text = completed;
                                            self.last_suggestion_query = self.input_text.trim().to_string();
                                            self.input_suggestions = find_matching_applications(&self.input_text, 5);
                                            ctx.memory_mut(|m| m.request_focus(input_id));
                                            ctx.request_repaint();
                                        }

                                        if let Some(prompt) = clicked_app_suggestion {
                                            self.stop_speaking();
                                            if self.is_recording {
                                                self.stop_recording();
                                            }
                                            self.continuous_mode = true;
                                            self.input_text.clear();
                                            self.input_suggestions.clear();
                                            self.last_suggestion_query.clear();
                                            self.live_transcript.clear();
                                            self.chat_history.push(ChatEntry {
                                                role: ChatRole::User,
                                                text: prompt.clone(),
                                                timestamp: current_time_str(),
                                                quick_suggestions: Vec::new(),
                                                screen_size_kb: None,
                                            });
                                            self.status = AgentStatus::Thinking;
                                            let _ = self.command_sender.send(AgentCommand::Prompt(prompt));
                                        }

                                        enter_hit = (response.lost_focus() || response.has_focus())
                                            && ctx.input(|i| i.key_pressed(egui::Key::Enter) && !i.modifiers.shift);
                                        if enter_hit {
                                            ctx.input_mut(|i| i.consume_key(egui::Modifiers::NONE, egui::Key::Enter));
                                        }
                                    });

                                    ui.add_space(3.0);

                                    // 2e rangée : 7 boutons d'action ergonomiques (WCAG 2.2 AAA, Clic & Dwell 800 ms)
                                    ui.horizontal(|ui| {
                                        ui.spacing_mut().item_spacing.x = 6.0;

                                        // 1. Bouton Analyser (WCAG AAA 50x50)
                                        let analyze_id = ui.make_persistent_id("dwell_btn_analyze");
                                        let analyze_prog = self.dwell_state.get_progress(analyze_id);
                                        let analyze_resp = render_accessible_icon_button(
                                            ui,
                                            "🔍",
                                            &t!("ui.btn_analyze"),
                                            &t!("ui.btn_analyze_tooltip"),
                                            false,
                                            analyze_prog,
                                            Some(egui::Color32::from_rgb(0, 140, 220)),
                                            Some(egui::Color32::from_rgb(0, 165, 245)),
                                        );
                                        let (analyze_dwell, _) = self.dwell_state.update(analyze_id, analyze_resp.hovered());
                                        let analyze_btn_hit = analyze_resp.clicked() || analyze_dwell;
                                        let enter_with_text = enter_hit && !self.input_text.trim().is_empty();

                                        if analyze_btn_hit || enter_with_text {
                                            println!("[UI Input] Validation déclenchée (analyze_btn={}, enter_hit={}, texte=\"{}\")", analyze_btn_hit, enter_hit, self.input_text.trim());
                                            self.stop_speaking();
                                            if self.is_recording {
                                                self.stop_recording();
                                            }
                                            self.continuous_mode = true;
                                            self.input_suggestions.clear();
                                            self.last_suggestion_query.clear();
                                            let typed_raw = std::mem::take(&mut self.input_text).trim().to_string();
                                            let prompt_raw = if analyze_btn_hit && typed_raw.is_empty() {
                                                t!("commands.analyze_screen_prompt").to_string()
                                            } else {
                                                typed_raw
                                            };
                                            let prompt = self.expand_macro(&prompt_raw);
                                            self.live_transcript.clear();

                                            let norm_prompt = normalize_for_command_match(&prompt);
                                            if is_start_subtitles_command(&norm_prompt) {
                                                println!("[UI Input] Commande démarrage sous-titres détectée");
                                                self.start_live_subtitles(None);
                                                return;
                                            } else if is_stop_subtitles_command(&norm_prompt) {
                                                println!("[UI Input] Commande arrêt sous-titres détectée");
                                                self.stop_live_subtitles();
                                                return;
                                            }

                                            if let Some(card) = is_read_card_intent(&prompt, &self.floating_cards) {
                                                let speech = card.build_narrator_speech();
                                                play_earcon(Earcon::Success);
                                                self.status = AgentStatus::Speaking;
                                                self.chat_history.push(ChatEntry {
                                                    role: ChatRole::User,
                                                    text: prompt.clone(),
                                                    timestamp: current_time_str(),
                                                    quick_suggestions: Vec::new(),
                                                    screen_size_kb: None,
                                                });
                                                self.chat_history.push(ChatEntry {
                                                    role: ChatRole::Agent,
                                                    text: speech.clone(),
                                                    timestamp: current_time_str(),
                                                    quick_suggestions: Vec::new(),
                                                    screen_size_kb: None,
                                                });
                                                let _ = self.tts_sender.send(TtsCommand::Speak(speech));
                                                return;
                                            }

                                            // 1. Bascule du mode survol/numéros
                                            if is_switch_access_command(&norm_prompt) {
                                                println!("[UI Input] Commande bascule switch_access détectée");
                                                self.toggle_switch_access_mode(None);
                                                return;
                                            }

                                            // 2. Détection directe du numéro en mode survol/numéros actif
                                            if self.switch_access_mode {
                                                if let Some(idx) = extract_badge_number(&norm_prompt) {
                                                    println!("[UI Input] Numéro de badge #{} détecté en mode survol", idx);
                                                    self.chat_history.push(ChatEntry {
                                                        role: ChatRole::User,
                                                        text: prompt,
                                                        timestamp: current_time_str(),
                                                        quick_suggestions: Vec::new(),
                                                        screen_size_kb: None,
                                                    });
                                                    self.click_numbered_target_by_index(idx);
                                                    self.status = AgentStatus::Idle;
                                                    return;
                                                }
                                            }

                                            println!("[UI Input] Traitement de la requête utilisateur : \"{}\"", prompt);

                                            if norm_prompt == "annule" || norm_prompt == "annuler" || norm_prompt == "undo" {
                                                let undo_feedback = if let Some(fb) = undo_last_system_action() {
                                                    fb
                                                } else {
                                                    #[cfg(windows)]
                                                    send_hotkey(&[windows::Win32::UI::Input::KeyboardAndMouse::VIRTUAL_KEY(0x11)], windows::Win32::UI::Input::KeyboardAndMouse::VIRTUAL_KEY(0x5A));
                                                    #[cfg(not(windows))]
                                                    let _ = std::process::Command::new("xdotool").args(["key", "ctrl+z"]).status();
                                                    t!("commands.last_action_undone").to_string()
                                                };
                                                play_earcon(Earcon::Success);
                                                self.chat_history.push(ChatEntry {
                                                    role: ChatRole::User,
                                                    text: prompt,
                                                    timestamp: current_time_str(),
                                                    quick_suggestions: Vec::new(),
                                                    screen_size_kb: None,
                                                });
                                                self.chat_history.push(ChatEntry {
                                                    role: ChatRole::Agent,
                                                    text: undo_feedback.clone(),
                                                    timestamp: current_time_str(),
                                                    quick_suggestions: Vec::new(),
                                                    screen_size_kb: None,
                                                });
                                                let _ = self.tts_sender.send(TtsCommand::Speak(undo_feedback));
                                                self.status = AgentStatus::Idle;
                                                return;
                                            }

                                            if let Some(feedback) = try_execute_local_navigation_shortcut(&prompt) {
                                                play_earcon(Earcon::Success);
                                                self.chat_history.push(ChatEntry {
                                                    role: ChatRole::User,
                                                    text: prompt,
                                                    timestamp: current_time_str(),
                                                    quick_suggestions: Vec::new(),
                                                    screen_size_kb: None,
                                                });
                                                self.status = AgentStatus::Idle;
                                                return;
                                            }

                                            self.chat_history.push(ChatEntry {
                                                role: ChatRole::User,
                                                text: prompt.clone(),
                                                timestamp: current_time_str(),
                                                quick_suggestions: Vec::new(),
                                                screen_size_kb: None,
                                            });
                                            if is_cancel_scheduled_command(&prompt) {
                                                let cancelled = cancel_all_scheduled_tasks();
                                                play_earcon(Earcon::Success);
                                                let reply = if cancelled > 0 {
                                                    t!("speech.scheduled_tasks_cancelled_count", count = cancelled).to_string()
                                                } else {
                                                    t!("speech.scheduled_tasks_cancelled").to_string()
                                                };
                                                self.chat_history.push(ChatEntry {
                                                    role: ChatRole::Agent,
                                                    text: reply.clone(),
                                                    timestamp: current_time_str(),
                                                    quick_suggestions: Vec::new(),
                                                    screen_size_kb: None,
                                                });
                                                let _ = self.tts_sender.send(TtsCommand::Speak(reply));
                                                self.status = AgentStatus::Idle;
                                                self.continuous_mode = false;
                                                return;
                                            }
                                            if is_api_key_command(&prompt) {
                                                self.sync_modal_inputs_for_provider(self.llm_config.provider);
                                                self.show_api_key_modal = true;
                                                let reply = t!("speech.api_key_modal_opened").to_string();
                                                self.chat_history.push(ChatEntry {
                                                    role: ChatRole::Agent,
                                                    text: reply.clone(),
                                                    timestamp: current_time_str(),
                                                    quick_suggestions: Vec::new(),
                                                    screen_size_kb: None,
                                                });
                                                let _ = self.tts_sender.send(TtsCommand::Speak(reply));
                                                self.status = AgentStatus::Idle;
                                                self.continuous_mode = false;
                                            } else if is_hide_command(&prompt) {
                                                self.is_hidden = true;
                                                self.status = AgentStatus::Idle;
                                                let reply = t!("speech.overlay_hidden").to_string();
                                                self.chat_history.push(ChatEntry {
                                                    role: ChatRole::Agent,
                                                    text: reply.clone(),
                                                    timestamp: current_time_str(),
                                                    quick_suggestions: Vec::new(),
                                                    screen_size_kb: None,
                                                });
                                                let _ = self.tts_sender.send(TtsCommand::Speak(reply));
                                            } else {
                                                println!("[UI Input] Envoi de AgentCommand::Prompt vers le backend : \"{}\"", prompt);
                                                self.status = AgentStatus::Thinking;
                                                let _ = self.command_sender.send(AgentCommand::Prompt(prompt));
                                            }
                                        }

                                        // 2. Bouton Micro
                                        let mic_id = ui.make_persistent_id("dwell_btn_mic");
                                        let mic_prog = self.dwell_state.get_progress(mic_id);
                                        let (mic_icon, mic_sub, mic_color) = if self.is_recording {
                                            ("🔴", t!("ui.btn_mic_stop").to_string(), Some(egui::Color32::from_rgb(235, 60, 60)))
                                        } else {
                                            ("🎙", t!("ui.btn_mic").to_string(), None)
                                        };
                                        let mic_resp = render_accessible_icon_button(
                                            ui,
                                            mic_icon,
                                            &mic_sub,
                                            &t!("ui.btn_mic_tooltip"),
                                            self.is_recording,
                                            mic_prog,
                                            mic_color,
                                            None,
                                        );
                                        let (mic_dwell, _) = self.dwell_state.update(mic_id, mic_resp.hovered());
                                        if mic_resp.clicked() || mic_dwell {
                                            self.toggle_recording(ui.ctx());
                                        }

                                        // 3. Bouton Dictée
                                        let dict_id = ui.make_persistent_id("dwell_btn_dict");
                                        let dict_prog = self.dwell_state.get_progress(dict_id);
                                        let dict_sub = if self.dictation_mode { t!("ui.btn_dictation_active").to_string() } else { t!("ui.btn_dictation").to_string() };
                                        let dict_resp = render_accessible_icon_button(
                                            ui,
                                            "✍",
                                            &dict_sub,
                                            &t!("ui.btn_dictation_tooltip"),
                                            self.dictation_mode,
                                            dict_prog,
                                            Some(egui::Color32::from_rgb(215, 155, 30)),
                                            None,
                                        );
                                        let (dict_dwell, _) = self.dwell_state.update(dict_id, dict_resp.hovered());
                                        if dict_resp.clicked() || dict_dwell {
                                            if self.dictation_mode {
                                                self.exit_dictation_mode();
                                            } else {
                                                self.enter_dictation_mode();
                                            }
                                        }

                                        // 4. Bouton Survol (Switch Access)
                                        let switch_id = ui.make_persistent_id("dwell_btn_switch");
                                        let switch_prog = self.dwell_state.get_progress(switch_id);
                                        let switch_sub = if self.switch_access_mode { t!("ui.btn_hover_active").to_string() } else { t!("ui.btn_hover").to_string() };
                                        let switch_resp = render_accessible_icon_button(
                                            ui,
                                            "🎯",
                                            &switch_sub,
                                            &t!("ui.btn_hover_tooltip"),
                                            self.switch_access_mode,
                                            switch_prog,
                                            Some(egui::Color32::from_rgb(235, 115, 0)),
                                            None,
                                        );
                                        let (switch_dwell, _) = self.dwell_state.update(switch_id, switch_resp.hovered());
                                        if switch_resp.clicked() || switch_dwell {
                                            self.toggle_switch_access_mode(None);
                                        }

                                        // 5. Bouton Voix
                                        let voice_id = ui.make_persistent_id("dwell_btn_voice");
                                        let voice_prog = self.dwell_state.get_progress(voice_id);
                                        let (voice_icon, voice_sub) = match self.voice_gender {
                                            VoiceProfile::Denise => ("👩", "Denise"),
                                            VoiceProfile::Henri => ("👨", "Henri"),
                                            VoiceProfile::Hortense => ("🎙️", "Hortense"),
                                            VoiceProfile::Paul => ("🎩", "Paul"),
                                            VoiceProfile::Sylvie => ("🍁", "Sylvie"),
                                            VoiceProfile::Jean => ("⚡", "Jean"),
                                        };
                                        let next_gender = self.voice_gender.next();
                                        let voice_resp = render_accessible_icon_button(
                                            ui,
                                            voice_icon,
                                            voice_sub,
                                            &t!(
                                                "ui.btn_voice_tooltip",
                                                current = self.voice_gender.display_name(),
                                                next = next_gender.display_name()
                                            ).to_string(),
                                            self.status == AgentStatus::Speaking,
                                            voice_prog,
                                            Some(theme.accent),
                                            None,
                                        );
                                        let (voice_dwell, _) = self.dwell_state.update(voice_id, voice_resp.hovered());
                                        if voice_resp.clicked() || voice_dwell {
                                            self.set_voice_gender(next_gender);
                                        }
                                        voice_resp.context_menu(|ui| {
                                            for &profile in VoiceProfile::ALL {
                                                let is_selected = self.voice_gender == profile;
                                                if ui.selectable_label(is_selected, profile.display_name()).clicked() {
                                                    self.set_voice_gender(profile);
                                                    ui.close();
                                                }
                                            }
                                        });

                                        // 6. Bouton Réglages
                                        let settings_id = ui.make_persistent_id("dwell_btn_settings");
                                        let settings_prog = self.dwell_state.get_progress(settings_id);
                                        let settings_resp = render_accessible_icon_button(
                                            ui,
                                            "⚙",
                                            &t!("ui.btn_settings"),
                                            &t!("ui.btn_settings_tooltip"),
                                            self.show_api_key_modal,
                                            settings_prog,
                                            Some(theme.accent),
                                            None,
                                        );
                                        let (settings_dwell, _) = self.dwell_state.update(settings_id, settings_resp.hovered());
                                        if settings_resp.clicked() || settings_dwell {
                                            self.settings_tab = SettingsTab::LlmProvider;
                                            self.show_api_key_modal = true;
                                            self.sync_modal_inputs_for_provider(self.llm_config.provider);
                                            self.api_key_error = None;
                                            self.stop_speaking();
                                        }

                                        // 7. Bouton Effacer
                                        let clear_id = ui.make_persistent_id("dwell_btn_clear");
                                        let clear_prog = self.dwell_state.get_progress(clear_id);
                                        let clear_resp = render_accessible_icon_button(
                                            ui,
                                            "🗑",
                                            &t!("ui.btn_clear"),
                                            &t!("ui.btn_clear_tooltip"),
                                            false,
                                            clear_prog,
                                            None,
                                            None,
                                        );
                                        let (clear_dwell, _) = self.dwell_state.update(clear_id, clear_resp.hovered());
                                        if clear_resp.clicked() || clear_dwell {
                                            let _ = self.command_sender.send(AgentCommand::ClearHistory);
                                            self.chat_history.clear();
                                            self.window_analysis_cache.clear();
                                            self.chat_history.push(ChatEntry {
                                                role: ChatRole::Agent,
                                                text: t!("commands.context_cleared").to_string(),
                                                timestamp: current_time_str(),
                                                quick_suggestions: Vec::new(),
                                                screen_size_kb: None,
                                            });
                                            self.live_transcript.clear();
                                            self.stop_speaking();
                                        }

                                        ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                                            self.draw_mini_equalizer(ui, time);
                                        });
                                    });
                                });
                            });
                        });
                });

                // 3. Boîte de discussion adaptative (évaluée après les contrôles pour la navigation Tab)
                let mut computed_chat_h = current_chat_h;
                let messages_rect = egui::Rect::from_min_max(
                    egui::pos2(response_rect.min.x + 8.0, sep_y + 4.0),
                    egui::pos2(response_rect.max.x - 8.0, response_rect.max.y - 6.0),
                );

                ui.scope_builder(egui::UiBuilder::new().max_rect(messages_rect), |ui| {
                    match self.active_tab {
                        ActiveTab::Assistance => {
                            let max_scroll_h = (messages_rect.height()).max(20.0);
                            let scroll_out = egui::ScrollArea::vertical()
                                .id_salt("assistance_chat_scroll")
                                .stick_to_bottom(true)
                                .max_height(max_scroll_h)
                                .auto_shrink([false, true])
                                .show(ui, |ui| {
                                    let last_user_idx = self.chat_history.iter().rposition(|e| e.role == ChatRole::User);
                                    let last_agent_idx = self.chat_history.iter().rposition(|e| e.role == ChatRole::Agent);
                                    let mut clicked_quick_suggestion: Option<String> = None;
                                    let mut request_stop_speaking = false;
                                    let mut speak_message: Option<String> = None;
                                    let agent_display_name = self.current_agent_model_display_name();
                                    let is_speaking = self.status == AgentStatus::Speaking;
                                    let dwell_state = &mut self.dwell_state;

                                    for (idx, entry) in self.chat_history.iter().enumerate() {
                                        let is_highlighted = Some(idx) == last_user_idx || Some(idx) == last_agent_idx;
                                        let msg_id = ui.make_persistent_id(format!("chat_entry_item_{idx}"));
                                        let layer_id = ui.layer_id();
                                        ui.memory_mut(|m| m.interested_in_focus(msg_id, layer_id));
                                        let is_msg_focused = ui.memory(|m| m.has_focus(msg_id));

                                        ui.vertical(|ui| {
                                            ui.horizontal(|ui| {
                                                let (label_name, label_color) = match entry.role {
                                                    ChatRole::User => {
                                                        let name = if let Some(kb) = entry.screen_size_kb {
                                                            format!("{} - {:.1} ko", t!("ui.chat_user_label"), kb)
                                                        } else {
                                                            t!("ui.chat_user_label").to_string()
                                                        };
                                                        (name, egui::Color32::from_rgb(255, 215, 120))
                                                    }
                                            ChatRole::Agent => (agent_display_name.clone(), theme.accent),
                                                };
                                                ui.label(
                                                    egui::RichText::new(&label_name)
                                                        .size(if is_highlighted { 11.5 } else { 10.0 })
                                                        .color(label_color)
                                                        .strong(),
                                                );
                                                ui.label(
                                                    egui::RichText::new(&entry.timestamp)
                                                        .size(if is_highlighted { 10.0 } else { 9.0 })
                                                        .color(egui::Color32::from_rgba_unmultiplied(170, 185, 205, 140)),
                                                );

                                                if entry.role == ChatRole::Agent && Some(idx) == last_agent_idx && is_speaking {
                                                    ui.add_space(4.0);
                                                    let stop_btn = ui.add(
                                                        egui::Button::new(
                                                            egui::RichText::new(t!("ui.btn_mute_speech"))
                                                                .size(9.5)
                                                                .color(egui::Color32::from_rgb(255, 130, 130)),
                                                        )
                                                        .small()
                                                        .corner_radius(egui::CornerRadius::same(6)),
                                                    );
                                                    if stop_btn.on_hover_text(t!("ui.avatar_mute_tooltip")).clicked() {
                                                        request_stop_speaking = true;
                                                    }
                                                }
                                            });

                                            let (text_size, line_height, text_color, is_strong) = match entry.role {
                                                ChatRole::User => {
                                                    if is_highlighted {
                                                        (14.5, 20.0, theme.text_user, true)
                                                    } else {
                                                        (12.0, 16.5, egui::Color32::from_rgba_unmultiplied(225, 210, 160, 185), false)
                                                    }
                                                }
                                                ChatRole::Agent => {
                                                    if is_highlighted {
                                                        (15.5, 22.0, egui::Color32::from_rgb(255, 255, 255), true)
                                                    } else {
                                                        (12.5, 17.5, egui::Color32::from_rgba_unmultiplied(200, 210, 225, 180), false)
                                                    }
                                                }
                                            };

                                            let has_interactive_md = (entry.text.contains('[') && entry.text.contains("]("))
                                                || entry.text.contains("[link")
                                                || entry.text.contains("[/link]")
                                                || entry.text.contains("```")
                                                || entry.text.contains("▶ ")
                                                || entry.text.contains("🎬 ")
                                                || entry.text.contains("🎵 ")
                                                || entry.text.contains("📄 ")
                                                || entry.text.contains("🖼")
                                                || (entry.text.contains('|') && entry.text.contains('\n'));

                                            if entry.role == ChatRole::Agent && has_interactive_md {
                                                if let Some(prompt) = floating_window::render_markdown(ui, &entry.text, &format!("chat_entry_{idx}"), dwell_state, &theme) {
                                                    clicked_quick_suggestion = Some(prompt);
                                                }
                                            } else {
                                            let mut rich = egui::RichText::new(&entry.text)
                                                .size(text_size)
                                                .line_height(Some(line_height))
                                                .color(text_color);
                                            if is_strong {
                                                rich = rich.strong();
                                            }
                                            let label_resp = ui.add(egui::Label::new(rich).wrap());
                                            let interact_resp = ui.interact(label_resp.rect, msg_id, egui::Sense::click());

                                            let msg_dwell_id = ui.make_persistent_id(format!("dwell_chat_msg_{idx}"));
                                            let dwell_triggered = apply_dwell_to_response(ui, dwell_state, msg_dwell_id, &interact_resp);
                                            if dwell_triggered {
                                                dwell_state.lock_id(msg_dwell_id, Duration::from_secs(6));
                                                speak_message = Some(entry.text.clone());
                                            }

                                            if interact_resp.clicked() {
                                                ui.memory_mut(|m| m.request_focus(msg_id));
                                            }
                                            let effective_focused = is_msg_focused || interact_resp.has_focus();

                                            // Copie au clavier Ctrl+C dès que le message a le focus clavier (ou au survol sans focus actif ailleurs)
                                            let copy_msg_hit = ctx.input(|i| {
                                                i.events.iter().any(|e| matches!(e, egui::Event::Copy))
                                                    || ((i.modifiers.ctrl || i.modifiers.command) && i.key_pressed(egui::Key::C))
                                            });
                                            let should_copy = effective_focused
                                                || (interact_resp.hovered() && ui.memory(|m| m.focused().is_none()));

                                            if should_copy && copy_msg_hit {
                                                set_clipboard_text(&entry.text);
                                                ctx.copy_text(entry.text.clone());
                                                play_earcon(Earcon::Success);
                                            }

                                            if effective_focused {
                                                label_resp.scroll_to_me(Some(egui::Align::Center));
                                                ui.painter().rect_stroke(
                                                    label_resp.rect.expand(4.0),
                                                    egui::CornerRadius::same(6),
                                                    egui::Stroke::new(1.8, theme.accent),
                                                    egui::StrokeKind::Outside,
                                                );
                                            }

                                            interact_resp.context_menu(|ui| {
                                                if ui.button(t!("ui.ctx_copy_text")).clicked() {
                                                    set_clipboard_text(&entry.text);
                                                    ctx.copy_text(entry.text.clone());
                                                    play_earcon(Earcon::Success);
                                                    ui.close();
                                                }
                                            });
                                            }

                                            // Boutons-onglets pour les suggestions rapides
                                            if !entry.quick_suggestions.is_empty() {
                                                ui.add_space(4.0);
                                                ui.horizontal_wrapped(|ui| {
                                                    for (sug_idx, sug) in entry.quick_suggestions.iter().enumerate() {
                                                        if let Some(clean_label) = sug.to_display_string() {
                                                            let clean_label = clean_label.trim();
                                                            if clean_label.is_empty() {
                                                                continue;
                                                            }
                                                            let btn_text = egui::RichText::new(format!("💡 {clean_label}"))
                                                                .size(11.0)
                                                                .color(theme.accent);

                                                            let btn = egui::Button::new(btn_text)
                                                                .stroke(egui::Stroke::new(1.0, theme.border))
                                                                .corner_radius(egui::CornerRadius::same(10));

                                                            let resp = ui.add(btn).on_hover_text(t!("ui.quick_suggestion_tooltip"));
                                                            let sug_dwell_id = ui.make_persistent_id(format!("dwell_quick_sug_{idx}_{sug_idx}"));
                                                            if apply_dwell_to_response(ui, dwell_state, sug_dwell_id, &resp) {
                                                                clicked_quick_suggestion = Some(clean_label.to_string());
                                                            }
                                                        }
                                                    }
                                                });
                                            }
                                        });
                                        ui.add_space(if is_highlighted { 8.0 } else { 5.0 });
                                    }

                                    if let Some(prompt) = clicked_quick_suggestion {
                                        self.stop_speaking();
                                        if self.is_recording {
                                            self.stop_recording();
                                        }
                                        self.status = AgentStatus::Thinking;
                                        self.continuous_mode = true;
                                        self.live_transcript.clear();

                                        let norm_prompt = normalize_for_command_match(&prompt);

                                        if is_start_subtitles_command(&norm_prompt) {
                                            self.start_live_subtitles(None);
                                            return;
                                        }
                                        if is_stop_subtitles_command(&norm_prompt) {
                                            self.stop_live_subtitles();
                                            return;
                                        }

                                        if self.switch_access_mode {
                                            if let Some(idx) = extract_badge_number(&norm_prompt) {
                                                // User feedback is already pushed by the caller
                                                self.click_numbered_target_by_index(idx);
                                                self.status = AgentStatus::Idle;
                                                return;
                                            }
                                        }

                                        if is_switch_access_command(&prompt) {
                                            mark_hover_nav_asked();
                                            if !self.switch_access_mode {
                                                self.toggle_switch_access_mode(None);
                                            }
                                            if self.continuous_mode && self.status != AgentStatus::EmergencyStopped {
                                                self.start_recording(&ctx, false);
                                            }
                                            return;
                                        }

                                        if is_dismiss_hover_command(&prompt) {
                                            mark_hover_nav_asked();
                                            play_earcon(Earcon::Success);
                                            self.status = AgentStatus::Idle;
                                            let reply = t!("speech.dismiss_hover").to_string();
                                            self.chat_history.push(ChatEntry {
                                                role: ChatRole::User,
                                                text: prompt.clone(),
                                                timestamp: current_time_str(),
                                                quick_suggestions: Vec::new(),
                                                screen_size_kb: None,
                                            });
                                            self.chat_history.push(ChatEntry {
                                                role: ChatRole::Agent,
                                                text: reply.clone(),
                                                timestamp: current_time_str(),
                                                quick_suggestions: Vec::new(),
                                                screen_size_kb: None,
                                            });
                                            let _ = self.tts_sender.send(TtsCommand::Speak(reply));
                                            return;
                                        }

                                        // Interception directe pour faire lire la fenêtre volante par le narrateur
                                        if let Some(card) = is_read_card_intent(&prompt, &self.floating_cards) {
                                            let speech = card.build_narrator_speech();
                                            play_earcon(Earcon::Success);
                                            self.status = AgentStatus::Speaking;
                                            self.chat_history.push(ChatEntry {
                                                role: ChatRole::User,
                                                text: prompt.clone(),
                                                timestamp: current_time_str(),
                                                quick_suggestions: Vec::new(),
                                                screen_size_kb: None,
                                            });
                                            self.chat_history.push(ChatEntry {
                                                role: ChatRole::Agent,
                                                text: speech.clone(),
                                                timestamp: current_time_str(),
                                                quick_suggestions: Vec::new(),
                                                screen_size_kb: None,
                                            });
                                            let _ = self.tts_sender.send(TtsCommand::Speak(speech));
                                            return;
                                        }

                                        self.chat_history.push(ChatEntry {
                                            role: ChatRole::User,
                                            text: prompt.clone(),
                                            timestamp: current_time_str(),
                                            quick_suggestions: Vec::new(),
                                            screen_size_kb: None,
                                        });
                                        let _ = self.command_sender.send(AgentCommand::Prompt(prompt));
                                    }

                                    if let Some(text_to_speak) = speak_message {
                                        self.stop_speaking();
                                        play_earcon(Earcon::Success);
                                        let clean_text = sanitize_for_tts(&text_to_speak);
                                        if !clean_text.trim().is_empty() {
                                            self.status = AgentStatus::Speaking;
                                            let _ = self.tts_sender.send(TtsCommand::Speak(clean_text));
                                            ctx.request_repaint();
                                        }
                                    }

                                    if request_stop_speaking {
                                        self.stop_speaking();
                                    }
                                });
                            computed_chat_h = (scroll_out.content_size.y + 44.0).clamp(min_chat_h, max_chat_h);
                        }
                        ActiveTab::Chat => {
                            // Barre de recherche et salon actif
                            ui.horizontal(|ui| {
                                if !self.current_twitch_channel.is_empty() {
                                    ui.label(
                                        egui::RichText::new(t!("ui.twitch_channel_label"))
                                            .size(11.0)
                                            .color(egui::Color32::from_rgb(170, 185, 205)),
                                    );
                                    ui.hyperlink_to(
                                        egui::RichText::new(format!("#{}", self.current_twitch_channel))
                                            .size(11.0)
                                            .color(egui::Color32::from_rgb(0, 195, 255))
                                            .underline(),
                                        format!("https://twitch.tv/{}", self.current_twitch_channel),
                                    );
                                    ui.separator();
                                }

                                let search_edit = ui.add_sized(
                                    [110.0, 18.0],
                                    egui::TextEdit::singleline(&mut self.twitch_search_query)
                                        .hint_text(t!("ui.twitch_search_hint")),
                                );
                                let enter_pressed = search_edit.lost_focus() && ui.input(|i| i.key_pressed(egui::Key::Enter));
                                if (ui.small_button("🔍").clicked() || enter_pressed) && !self.twitch_search_query.trim().is_empty() {
                                    let _ = self.command_sender.send(AgentCommand::SearchTwitch(
                                        self.twitch_search_query.trim().to_string(),
                                    ));
                                }
                            });

                            if !self.twitch_search_results.is_empty() {
                                ui.add_space(2.0);
                                ui.horizontal_wrapped(|ui| {
                                    ui.label(
                                        egui::RichText::new(t!("ui.twitch_results_label"))
                                            .size(10.5)
                                            .color(egui::Color32::from_rgb(255, 215, 120)),
                                    );
                                    let mut selected_channel = None;
                                    for ch in &self.twitch_search_results {
                                        if ui.small_button(format!("▶ {}", ch.display_name)).on_hover_text(t!("ui.twitch_join_tooltip")).clicked() {
                                            selected_channel = Some(ch.login.clone());
                                        }
                                        if ui.small_button("↗").on_hover_text(t!("ui.twitch_open_tooltip")).clicked() {
                                            ui.ctx().open_url(egui::OpenUrl::new_tab(format!("https://twitch.tv/{}", ch.login)));
                                        }
                                    }
                                    if let Some(ch) = selected_channel {
                                        self.current_twitch_channel = ch.clone();
                                        let _ = self.twitch_channel_sender.send(ch);
                                        self.twitch_messages.clear();
                                        self.twitch_search_results.clear();
                                    }
                                });
                            }
                            ui.separator();
                            let max_chat_scroll_h = (messages_rect.height() - 28.0).max(20.0);
                            let scroll_out = egui::ScrollArea::vertical()
                                .id_salt("twitch_chat_scroll")
                                .stick_to_bottom(true)
                                .max_height(max_chat_scroll_h)
                                .auto_shrink([false, true])
                                .show(ui, |ui| {
                                    if self.twitch_messages.is_empty() {
                                        ui.label(
                                            egui::RichText::new(t!("ui.twitch_waiting"))
                                                .size(12.0)
                                                .italics()
                                                .color(egui::Color32::from_rgba_unmultiplied(180, 190, 205, 150)),
                                        );
                                    } else {
                                        let mut speak_twitch_message: Option<String> = None;
                                        for (idx, msg) in self.twitch_messages.iter().enumerate() {
                                            let t_id = ui.make_persistent_id(format!("twitch_msg_item_{idx}"));
                                            let layer_id = ui.layer_id();
                                            ui.memory_mut(|m| m.interested_in_focus(t_id, layer_id));
                                            let is_t_focused = ui.memory(|m| m.has_focus(t_id));
                                            let resp = ui.horizontal_wrapped(|ui| {
                                                ui.label(
                                                    egui::RichText::new(&msg.timestamp)
                                                        .size(9.0)
                                                        .color(egui::Color32::from_rgba_unmultiplied(140, 150, 170, 140)),
                                                );
                                                ui.label(
                                                    egui::RichText::new(format!("{}:", msg.author))
                                                        .size(12.0)
                                                        .color(egui::Color32::from_rgb(169, 112, 255))
                                                        .strong(),
                                                );
                                                ui.label(
                                                    egui::RichText::new(&msg.text)
                                                        .size(12.5)
                                                        .color(egui::Color32::WHITE),
                                                );
                                            }).response;
                                            let resp = ui.interact(resp.rect, t_id, egui::Sense::click());

                                            let t_dwell_id = ui.make_persistent_id(format!("dwell_twitch_msg_{idx}"));
                                            if apply_dwell_to_response(ui, &mut self.dwell_state, t_dwell_id, &resp) {
                                                self.dwell_state.lock_id(t_dwell_id, Duration::from_secs(6));
                                                speak_twitch_message = Some(format!("{}: {}", msg.author, msg.text));
                                            }

                                            if resp.clicked() {
                                                ui.memory_mut(|m| m.request_focus(t_id));
                                            }
                                            let effective_t_focused = is_t_focused || resp.has_focus();
                                            let copy_t_hit = ctx.input(|i| {
                                                i.events.iter().any(|e| matches!(e, egui::Event::Copy))
                                                    || ((i.modifiers.ctrl || i.modifiers.command) && i.key_pressed(egui::Key::C))
                                            });
                                            if (effective_t_focused || resp.hovered()) && copy_t_hit {
                                                let text = format!("{}: {}", msg.author, msg.text);
                                                set_clipboard_text(&text);
                                                ctx.copy_text(text);
                                                play_earcon(Earcon::Success);
                                            }
                                            if effective_t_focused {
                                                resp.scroll_to_me(Some(egui::Align::Center));
                                                ui.painter().rect_stroke(
                                                    resp.rect.expand(2.0),
                                                    egui::CornerRadius::same(3),
                                                    egui::Stroke::new(1.2, egui::Color32::from_rgb(0, 200, 255)),
                                                    egui::StrokeKind::Outside,
                                                );
                                            }
                                            ui.add_space(3.0);
                                        }

                                        if let Some(t_speech) = speak_twitch_message {
                                            self.stop_speaking();
                                            play_earcon(Earcon::Success);
                                            let clean_speech = sanitize_for_tts(&t_speech);
                                            if !clean_speech.trim().is_empty() {
                                                self.status = AgentStatus::Speaking;
                                                let _ = self.tts_sender.send(TtsCommand::Speak(clean_speech));
                                                ctx.request_repaint();
                                            }
                                        }
                                    }
                                });
                            computed_chat_h = (scroll_out.content_size.y + 72.0).clamp(min_chat_h, max_chat_h);
                        }
                    }
                });

                // Hystérésis asymétrique pour self.chat_height
                if computed_chat_h > self.chat_height + 4.0 || computed_chat_h < self.chat_height - 8.0 {
                    self.chat_height = computed_chat_h;
                }

                // Gestion ergonomique du déplacement et du magnétisme
                handle_overlay_drag(
                    ui,
                    &ctx,
                    &work_area,
                    &overlay_layout,
                    tab_bar_max_x,
                    &mut self.window_pos,
                    &mut self.window_bottom_y,
                    &mut self.drag_offset,
                    is_maximized,
                );

                // Zones de redimensionnement interactif sur toutes les bordures et coins
                handle_overlay_resize_zones(ui, &ctx, win_w, active_win_h);

                // Poignées visuelles de redimensionnement discrètes aux coins
                let painter = ui.painter();

                // Tracé fluide et persistant des zones et badges sans clignotement ni effacement intempestif
                if self.switch_access_mode {
                    match self.switch_step {
                        SwitchAccessStep::Windows => {
                            if !self.switch_zones.is_empty() {
                                let zone_infos: Vec<ZoneOverlayInfo> = self.switch_zones.iter().enumerate().map(|(i, z)| {
                                    ZoneOverlayInfo {
                                        index: i + 1,
                                        title: z.title.clone(),
                                        rect: z.rect,
                                        is_focused: i == 0,
                                    }
                                }).collect();
                                let focused_target_idx = self.numbered_targets.first().map(|t| t.index);
                                draw_screen_zones_and_targets(&zone_infos, &self.numbered_targets, self.zoom_factor, focused_target_idx);
                            } else if !self.numbered_targets.is_empty() {
                                let focused_idx = self.numbered_targets.first().map(|t| t.index);
                                draw_screen_numbered_badges(&self.numbered_targets, self.zoom_factor, focused_idx);
                            }
                        }
                        SwitchAccessStep::Zones => {
                            if !self.switch_zones.is_empty() {
                                let num_zones = self.switch_zones.len();
                                let zone_infos: Vec<ZoneOverlayInfo> = self.switch_zones.iter().enumerate().map(|(i, z)| {
                                    ZoneOverlayInfo {
                                        index: i + 1,
                                        title: z.title.clone(),
                                        rect: z.rect,
                                        is_focused: (self.switch_focus_index % num_zones) == i,
                                    }
                                }).collect();
                                let focused_target_idx = self.numbered_targets.first().map(|t| t.index);
                                draw_screen_zones_and_targets(&zone_infos, &self.numbered_targets, self.zoom_factor, focused_target_idx);
                            }
                        }
                        SwitchAccessStep::Elements => {
                            let focused_target = self.numbered_targets.get(self.switch_focus_index % self.numbered_targets.len().max(1));
                            let focused_idx = focused_target.map(|t| t.index);
                            let active_zone_opt = self.switch_active_zone_index.and_then(|idx| self.switch_zones.get(idx));

                            if let Some(zone) = active_zone_opt {
                                let zone_info = vec![ZoneOverlayInfo {
                                    index: self.switch_active_zone_index.unwrap_or(0) + 1,
                                    title: zone.title.clone(),
                                    rect: zone.rect,
                                    is_focused: true,
                                }];
                                draw_screen_zones_and_targets(&zone_info, &self.numbered_targets, self.zoom_factor, focused_idx);
                            } else if !self.numbered_targets.is_empty() {
                                draw_screen_numbered_badges(&self.numbered_targets, self.zoom_factor, focused_idx);
                            }

                            #[cfg(windows)]
                        if let Some(target) = focused_target {
                            draw_screen_control_border(target.rect, target.role == "lecture textuelle");
                        }
                    }
                    }
                }


                // Halo visuel pulsé entourant la zone de navigation analysée par le LLM
                if self.switch_access_mode && self.switch_step == SwitchAccessStep::Zones && !self.switch_zones.is_empty() {
                    #[cfg(not(windows))]
                    {
                        let cur_zone = &self.switch_zones[self.switch_focus_index % self.switch_zones.len()];
                        let ppp = ctx.pixels_per_point().max(0.5);
                        let window_origin = self.window_pos.unwrap_or_default();
                        let z_rect = egui::Rect::from_min_max(
                            egui::pos2((cur_zone.rect[0] as f32 / ppp) - window_origin.x, (cur_zone.rect[1] as f32 / ppp) - window_origin.y),
                            egui::pos2((cur_zone.rect[2] as f32 / ppp) - window_origin.x, (cur_zone.rect[3] as f32 / ppp) - window_origin.y),
                        );
                        painter.rect_stroke(
                            z_rect.expand(6.0),
                            egui::CornerRadius::same(8),
                                egui::Stroke::new(2.5, egui::Color32::from_rgb(0, 200, 255)),
                            egui::StrokeKind::Outside,
                        );
                    }
                }

                // Cadre net délimitant le contrôle ciblé en mode Switch Access
                if self.switch_access_mode && self.switch_step == SwitchAccessStep::Elements && !self.numbered_targets.is_empty() {
                    #[cfg(not(windows))]
                    {
                        let cur_target = &self.numbered_targets[self.switch_focus_index % self.numbered_targets.len()];
                        let ppp = ctx.pixels_per_point().max(0.5);
                        let window_origin = self.window_pos.unwrap_or_default();
                        let t_rect = egui::Rect::from_min_max(
                            egui::pos2((cur_target.rect[0] as f32 / ppp) - window_origin.x, (cur_target.rect[1] as f32 / ppp) - window_origin.y),
                            egui::pos2((cur_target.rect[2] as f32 / ppp) - window_origin.x, (cur_target.rect[3] as f32 / ppp) - window_origin.y),
                        );
                        let stroke_color = if cur_target.role == "lecture textuelle" {
                            egui::Color32::from_rgb(0, 230, 255)
                        } else {
                            egui::Color32::from_rgb(255, 140, 0)
                        };
                        painter.rect_stroke(
                            t_rect.expand(3.0),
                            egui::CornerRadius::same(6),
                            egui::Stroke::new(2.5, stroke_color),
                            egui::StrokeKind::Outside,
                        );
                    }
                }

                // Indicateur visuel du mode Switch Access
                if self.switch_access_mode {
                    let stroke_color = egui::Color32::from_rgb(255, 215, 0);
                    painter.rect_stroke(
                        control_rect,
                        egui::CornerRadius::same(12),
                        egui::Stroke::new(2.5, stroke_color),
                        egui::StrokeKind::Inside,
                    );
                }
                // Tracé de l'outline rect de la zone cible en mode dictée
                if self.dictation_mode {
                    if let Ok(guard) = LAST_FOCUSED_RECT.lock() {
                        if let Some([left, top, right, bottom]) = *guard {
                            let ppp = ctx.pixels_per_point().max(0.5);
                            let window_origin = self.window_pos.unwrap_or_default();
                            let screen_rect = egui::Rect::from_min_max(
                                egui::pos2((left as f32 / ppp) - window_origin.x, (top as f32 / ppp) - window_origin.y),
                                egui::pos2((right as f32 / ppp) - window_origin.x, (bottom as f32 / ppp) - window_origin.y),
                            );
                            // Dessin d'un halo semi-transparent et d'une bordure dorée
                            ui.painter().rect_stroke(
                                screen_rect,
                                egui::CornerRadius::same(4),
                                egui::Stroke::new(2.0, egui::Color32::from_rgb(255, 205, 70)),
                                egui::StrokeKind::Outside,
                            );
                        }
                    }
                }
                draw_corner_grips(painter, win_w, active_win_h, padding);

                // Boîte modale de configuration (Fournisseurs LLM & Thèmes d'accessibilité)
                if self.show_api_key_modal {
                    egui::Window::new("settings_modal")
                        .title_bar(false)
                        .collapsible(false)
                        .resizable(false)
                        .anchor(egui::Align2::CENTER_CENTER, egui::vec2(0.0, 0.0))
                        .frame(
                            egui::Frame::new()
                                .fill(theme.modal_bg)
                                .stroke(egui::Stroke::new(1.2, theme.border))
                                .corner_radius(egui::CornerRadius::same(10))
                                .inner_margin(egui::Margin::symmetric(12, 10)),
                        )
                        .show(&ctx, |ui| {
                            // En-tête avec onglets de configuration
                            ui.horizontal(|ui| {
                                let is_llm_active = self.settings_tab == SettingsTab::LlmProvider;
                                let tab_llm = ui.selectable_label(
                                    is_llm_active,
                                    egui::RichText::new(t!("ui.settings_tab_llm"))
                                        .size(12.0)
                                        .color(if is_llm_active {
                                            theme.accent
                                        } else {
                                            theme.text_secondary
                                        })
                                        .strong(),
                                );
                                let tab_llm_id = ui.make_persistent_id("dwell_tab_llm_provider");
                                let llm_activated = if !is_llm_active {
                                    apply_dwell_to_response(ui, &mut self.dwell_state, tab_llm_id, &tab_llm)
                                } else {
                                    tab_llm.clicked()
                                };
                                if llm_activated {
                                    self.settings_tab = SettingsTab::LlmProvider;
                                }
                                let is_theme_active = self.settings_tab == SettingsTab::Theme;
                                let tab_theme = ui.selectable_label(
                                    is_theme_active,
                                    egui::RichText::new(t!("ui.settings_tab_theme"))
                                        .size(12.0)
                                        .color(if is_theme_active {
                                            theme.accent
                                        } else {
                                            theme.text_secondary
                                        })
                                        .strong(),
                                );
                                let tab_theme_id = ui.make_persistent_id("dwell_tab_theme");
                                let theme_activated = if !is_theme_active {
                                    apply_dwell_to_response(ui, &mut self.dwell_state, tab_theme_id, &tab_theme)
                                } else {
                                    tab_theme.clicked()
                                };
                                if theme_activated {
                                    self.settings_tab = SettingsTab::Theme;
                                }
                                let is_lang_active = self.settings_tab == SettingsTab::Language;
                                let tab_lang = ui.selectable_label(
                                    is_lang_active,
                                    egui::RichText::new(t!("ui.settings_tab_language"))
                                        .size(12.0)
                                        .color(if is_lang_active {
                                            theme.accent
                                        } else {
                                            theme.text_secondary
                                        })
                                        .strong(),
                                );
                                let tab_lang_id = ui.make_persistent_id("dwell_tab_language");
                                let lang_activated = if !is_lang_active {
                                    apply_dwell_to_response(ui, &mut self.dwell_state, tab_lang_id, &tab_lang)
                                } else {
                                    tab_lang.clicked()
                                };
                                if lang_activated {
                                    self.settings_tab = SettingsTab::Language;
                                }
                                let is_macro_active = self.settings_tab == SettingsTab::Macros;
                                let tab_macro = ui.selectable_label(
                                    is_macro_active,
                                    egui::RichText::new("⚡ Macros")
                                        .size(12.0)
                                        .color(if is_macro_active {
                                            theme.accent
                                        } else {
                                            theme.text_secondary
                                        })
                                        .strong(),
                                );
                                let tab_macro_id = ui.make_persistent_id("dwell_tab_macros");
                                let macro_activated = if !is_macro_active {
                                    apply_dwell_to_response(ui, &mut self.dwell_state, tab_macro_id, &tab_macro)
                                } else {
                                    tab_macro.clicked()
                                };
                                if macro_activated {
                                    self.settings_tab = SettingsTab::Macros;
                                }
                                ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                                    if ui.add(
                                        egui::Button::new(
                                            egui::RichText::new("✕")
                                                .size(11.0)
                                                .color(egui::Color32::from_rgb(255, 130, 130)),
                                        )
                                        .min_size(egui::vec2(18.0, 16.0))
                                        .corner_radius(egui::CornerRadius::same(4)),
                                    ).on_hover_text(t!("ui.btn_close")).clicked() {
                                        self.show_api_key_modal = false;
                                        self.api_key_error = None;
                                    }
                                });
                            });
                            ui.add_space(4.0);
                            ui.separator();
                            ui.add_space(4.0);

                            let max_scroll_h = (work_area.h * 0.82 - 50.0).clamp(240.0, 600.0);
                            egui::ScrollArea::vertical()
                                .id_salt("settings_modal_scroll")
                                .auto_shrink([true, true])
                                .max_height(max_scroll_h)
                                .show(ui, |ui| {
                            match self.settings_tab {
                                SettingsTab::LlmProvider => {
                            // --- Onglet Fournisseur LLM ---
                            let env_var_target = self.selected_provider_modal.env_key_name();
                            let env_model_target = self.selected_provider_modal.env_model_name();
                            let env_endpoint_target = self.selected_provider_modal.env_endpoint_name();
                            let default_model_name = self.selected_provider_modal.default_model();
                            let default_endpoint_url = self.selected_provider_modal.default_endpoint();

                            ui.horizontal_wrapped(|ui| {
                                ui.spacing_mut().item_spacing = egui::vec2(4.0, 3.0);
                                for &provider in LlmProvider::ALL {
                                    let is_current = self.llm_config.provider == provider;
                                    let is_enabled = provider.is_enabled();
                                    let is_selected_in_modal = self.selected_provider_modal == provider;

                                    let badge = if is_current {
                                        "★ "
                                    } else if is_enabled {
                                        "● "
                                    } else {
                                        "○ "
                                    };

                                    let label = format!("{}{}", badge, provider.display_name());
                                    let label_rich = if is_current {
                                        egui::RichText::new(label).strong().color(theme.accent)
                                    } else if !is_enabled {
                                        egui::RichText::new(label).color(egui::Color32::from_gray(140))
                                    } else {
                                        egui::RichText::new(label)
                                    };

                                    if ui.selectable_label(is_selected_in_modal, label_rich).clicked() {
                                        self.sync_modal_inputs_for_provider(provider);
                                    }
                                }
                            });

                            ui.add_space(4.0);

                            // Activation / Désactivation et sélection active
                            ui.horizontal(|ui| {
                                let mut is_enabled = self.selected_provider_modal.is_enabled();
                                if ui.checkbox(&mut is_enabled, "Fournisseur activé").changed() {
                                    let _ = self.selected_provider_modal.set_enabled(is_enabled);
                                    self.llm_config = resolve_llm_config();
                                }

                                if self.selected_provider_modal != self.llm_config.provider {
                                    if ui.button("★ Définir comme actif principal").clicked() {
                                        let _ = self.selected_provider_modal.set_enabled(true);
                                        let _ = write_env_var_to_file(".env", "LLM_PROVIDER", self.selected_provider_modal.display_name());
                                        std::env::set_var("LLM_PROVIDER", self.selected_provider_modal.display_name());
                                        self.llm_config = resolve_llm_config();

                                        let tier = local_llm::resolve_model_tier(self.selected_provider_modal, Some(local_llm::get_active_preset()));
                                        let rec = tier.default_options();
                                        self.local_llm_options = rec;
                                        local_llm::set_local_llm_options(rec);

                                        play_earcon(Earcon::Success);
                                    }
                                } else {
                                    ui.label(
                                        egui::RichText::new("(Actif principal)")
                                            .size(11.0)
                                            .color(theme.accent),
                                    );
                                }
                            });
                            ui.add_space(4.0);

                            if self.selected_provider_modal == LlmProvider::Local {
                                let active_preset = local_llm::get_active_preset();
                                egui::Frame::new()
                                    .fill(theme.control_bg)
                                    .stroke(egui::Stroke::new(1.0, theme.border_subtle))
                                    .corner_radius(egui::CornerRadius::same(8))
                                    .inner_margin(egui::Margin::symmetric(10, 8))
                                    .show(ui, |ui| {
                                        ui.horizontal(|ui| {
                                            ui.label(
                                                egui::RichText::new("Modèles locaux intégrés (GGUF / Vulkan & CPU) :")
                                                    .size(11.0)
                                                    .color(theme.accent)
                                                    .strong(),
                                            );
                                        });

                                        ui.add_space(4.0);

                                        let mut model_to_delete = None;
                                        for preset in local_llm::MODEL_PRESETS {
                                            let is_active = active_preset.id == preset.id;
                                            let is_installed = local_llm::is_preset_downloaded(preset);

                                            ui.horizontal(|ui| {
                                                let radio_label = format!("{} ({})", preset.name, preset.size_desc);
                                                let radio_resp = ui.radio(is_active, radio_label);
                                                if radio_resp.clicked() && !is_active {
                                                    local_llm::set_active_preset(preset.id);
                                                    self.llm_config.model = preset.name.to_string();
                                                    self.model_input = preset.name.to_string();
                                                    let _ = write_env_var_to_file(".env", "LLM_MODEL", preset.filename);
                                                    std::env::set_var("LLM_MODEL", preset.filename);
                                                    let _ = write_env_var_to_file(".env", "LOCAL_LLAMA_MODEL", preset.filename);
                                                    std::env::set_var("LOCAL_LLAMA_MODEL", preset.filename);

                                                    let rec = preset.tier().default_options();
                                                    self.local_llm_options = rec;
                                                    local_llm::set_local_llm_options(rec);

                                                    play_earcon(Earcon::Success);
                                                }

                                                let family_badge = match preset.family {
                                                    local_llm::ModelFamily::ChatML => "ChatML",
                                                    local_llm::ModelFamily::Llama3 => "Llama 3",
                                                };
                                                ui.label(
                                                    egui::RichText::new(family_badge)
                                                        .size(9.5)
                                                        .color(theme.text_secondary),
                                                );

                                                if is_installed {
                                                    ui.label(
                                                        egui::RichText::new("✅ Prêt")
                                                            .size(10.5)
                                                        .color(egui::Color32::from_rgb(100, 225, 130))
                                                        .strong(),
                                                    );
                                                    if ui.small_button("🗑")
                                                        .on_hover_text("Supprimer ce fichier pour libérer l'espace disque")
                                                        .clicked() {
                                                        model_to_delete = Some(preset);
                                                    }
                                                } else {
                                                ui.label(
                                                        egui::RichText::new("⚠️ Non téléchargé")
                                                            .size(10.5)
                                                        .color(egui::Color32::from_rgb(255, 180, 80)),
                                                    );
                                                }
                                            });
                                        }

                                        if let Some(p) = model_to_delete {
                                            match local_llm::delete_preset_model(p) {
                                                Ok(()) => {
                                                    play_earcon(Earcon::Success);
                                                    self.api_key_error = None;
                                                }
                                                Err(e) => {
                                                    self.api_key_error = Some(e);
                                                }
                                            }
                                        }

                                        ui.add_space(6.0);

                                        let active_installed = local_llm::is_preset_downloaded(active_preset);
                                        if let Some((percent, downloaded_mb, total_mb, speed_mb_s)) = self.local_download_progress {
                                            ui.add(
                                                egui::ProgressBar::new(percent / 100.0)
                                                    .text(format!("{percent:.1} % ({downloaded_mb:.1} / {total_mb:.1} Mo - {speed_mb_s:.1} Mo/s)"))
                                                    .animate(true)
                                            );
                                        } else if !active_installed {
                                            let event_tx_clone = self.event_sender.clone();
                                            let p = *active_preset;
                                            let btn_text = format!("⬇ Télécharger {} ({})", p.name, p.size_desc);
                                            if ui.button(egui::RichText::new(btn_text).strong().color(theme.accent)).clicked() {
                                                std::thread::spawn(move || {
                                                    let _ = local_llm::ensure_llama_model_downloaded_for_preset(&p, &event_tx_clone);
                                                });
                                            }
                                        } else {
                                            ui.label(
                                                egui::RichText::new(format!(
                                                    "Le modèle {} s'exécutera localement avec accélération GPU (Vulkan) ou CPU sans dépendance cloud.",
                                                    active_preset.name
                                                ))
                                                    .size(10.5)
                                                    .color(theme.text_secondary),
                                            );
                                        }
                                    });

                                    ui.add_space(6.0);
                                    egui::Frame::new()
                                        .fill(theme.panel_bg)
                                        .stroke(egui::Stroke::new(1.0, theme.border_subtle))
                                        .corner_radius(egui::CornerRadius::same(8))
                                        .inner_margin(egui::Margin::symmetric(10, 8))
                                        .show(ui, |ui| {
                                            let mut opts = self.local_llm_options;
                                            let mut changed = false;
                                            let current_tier = active_preset.tier();

                                            ui.horizontal(|ui| {
                                                ui.label(
                                                    egui::RichText::new("⚙️ Paramètres du prompt d'inférence :")
                                                        .size(11.0)
                                                        .color(theme.accent)
                                                        .strong(),
                                                );
                                                ui.label(
                                                    egui::RichText::new(format!("({})", current_tier.name()))
                                                        .size(9.5)
                                                        .color(theme.text_secondary),
                                                );
                                                ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                                                    if ui.small_button("⚡ Réglages conseillés").on_hover_text("Rétablir les options recommandées pour ce modèle").clicked() {
                                                        opts = current_tier.default_options();
                                                        changed = true;
                                                    }
                                                });
                                            });
                                            ui.add_space(4.0);

                                            if ui.checkbox(&mut opts.minimal_mode, "Mode prompt minimaliste (recommandé pour modèles locaux)").on_hover_text("Utilise des consignes concises au lieu du prompt cloud étendu, réduisant les erreurs de format JSON").changed() {
                                                changed = true;
                                            }

                                            if ui.checkbox(&mut opts.active_window_only, "Fenêtre principale uniquement (exclure les autres fenêtres / cartes)").on_hover_text("N'injecte que les données de la fenêtre active pour économiser le budget de tokens").changed() {
                                                changed = true;
                                            }

                                            if ui.checkbox(&mut opts.include_feedback, "Inclure les rapports de retour des commandes exécutées").on_hover_text("Transmet les logs détaillés d'exécution des étapes précédentes (désactiver allège le contexte)").changed() {
                                                changed = true;
                                            }

                                            if ui.checkbox(&mut opts.include_history, "Prendre en compte l'historique de conversation").on_hover_text("Si décoché, chaque requête est traitée de manière isolée sans être polluée par les échanges passés").changed() {
                                                changed = true;
                                            }

                                            if changed {
                                                self.local_llm_options = opts;
                                                local_llm::set_local_llm_options(opts);
                                            }
                                        });
                            } else {

                            ui.add_space(4.0);

                            // --- Section Clé API ---
                            ui.label(
                                egui::RichText::new(t!("ui.api_key_label", env = env_var_target))
                                    .size(11.0)
                                    .color(egui::Color32::WHITE)
                                    .strong(),
                            );
                            ui.add_space(2.0);

                            ui.horizontal(|ui| {
                                let key_input_id = egui::Id::new("api_key_input_field");
                                let is_key_focused = ctx.memory(|m| m.has_focus(key_input_id));
                                let paste_key_hit = ctx.input(|i| {
                                    i.events.iter().any(|e| matches!(e, egui::Event::Paste(_)))
                                        || ((i.modifiers.ctrl || i.modifiers.command) && i.key_pressed(egui::Key::V))
                                        || (i.modifiers.shift && i.key_pressed(egui::Key::Insert))
                                });
                                if is_key_focused && paste_key_hit {
                                    if let Some(clip) = get_clipboard_text().or_else(get_plain_clipboard_text) {
                                        let trimmed = clip.trim().to_string();
                                        if !trimmed.is_empty() {
                                            self.api_key_input = trimmed;
                                            play_earcon(Earcon::Success);
                                        }
                                    }
                                    ctx.input_mut(|i| i.consume_key(egui::Modifiers::COMMAND, egui::Key::V));
                                    ctx.input_mut(|i| i.consume_key(egui::Modifiers::CTRL, egui::Key::V));
                                }
                                let edit = egui::TextEdit::singleline(&mut self.api_key_input)
                                    .id(key_input_id)
                                    .password(!self.show_api_key_plain)
                                    .hint_text("sk-... (ou laisser vide pour serveurs locaux)")
                                    .desired_width(230.0);
                                let edit_resp = ui.add(edit);

                                let eye_icon = if self.show_api_key_plain { "🔒" } else { "👁" };
                                if ui.button(eye_icon).on_hover_text(t!("ui.toggle_api_key_visibility")).clicked() {
                                    self.show_api_key_plain = !self.show_api_key_plain;
                                }

                                if ui.button("📋").on_hover_text(t!("ui.paste_from_clipboard")).clicked() {
                                    if let Some(clip) = get_clipboard_text() {
                                        self.api_key_input = clip.trim().to_string();
                                    }
                                }

                                edit_resp.context_menu(|ui| {
                                    if ui.button(t!("ui.btn_paste")).clicked() {
                                        if let Some(clip) = get_clipboard_text() {
                                            self.api_key_input = clip.trim().to_string();
                                        }
                                        ui.close();
                                    }
                                    if ui.button(t!("ui.btn_copy")).clicked() {
                                        set_clipboard_text(&self.api_key_input);
                                        ui.close();
                                    }
                                    if ui.button(t!("ui.btn_clear_short")).clicked() {
                                        self.api_key_input.clear();
                                        ui.close();
                                    }
                                });

                                let copy_key_hit = ctx.input(|i| {
                                    i.events.iter().any(|e| matches!(e, egui::Event::Copy))
                                        || ((i.modifiers.ctrl || i.modifiers.command) && i.key_pressed(egui::Key::C))
                                });
                                if edit_resp.has_focus() && copy_key_hit && !self.api_key_input.is_empty() {
                                    set_clipboard_text(&self.api_key_input);
                                    ctx.copy_text(self.api_key_input.clone());
                                    play_earcon(Earcon::Success);
                                }

                            });

                            ui.add_space(5.0);

                            // --- Section Modèle de raisonnement ---
                            ui.horizontal(|ui| {
                                ui.label(
                                    egui::RichText::new(t!("ui.model_label", env = env_model_target))
                                        .size(11.0)
                                        .color(egui::Color32::WHITE)
                                        .strong(),
                                );
                                ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                                    if ui.small_button(t!("ui.btn_model_recommended"))
                                        .on_hover_text(t!("ui.restore_default_model_tooltip", model = default_model_name))
                                        .clicked()
                                    {
                                        self.model_input = default_model_name.to_string();
                                    }
                                });
                            });
                            ui.add_space(2.0);

                            ui.horizontal(|ui| {
                                let model_input_id = egui::Id::new("model_input_field");
                                let is_model_focused = ctx.memory(|m| m.has_focus(model_input_id));
                                let paste_model_hit = ctx.input(|i| {
                                    i.events.iter().any(|e| matches!(e, egui::Event::Paste(_)))
                                        || ((i.modifiers.ctrl || i.modifiers.command) && i.key_pressed(egui::Key::V))
                                        || (i.modifiers.shift && i.key_pressed(egui::Key::Insert))
                                });
                                if is_model_focused && paste_model_hit {
                                    if let Some(clip) = get_clipboard_text().or_else(get_plain_clipboard_text) {
                                        let trimmed = clip.trim().to_string();
                                        if !trimmed.is_empty() {
                                            self.model_input = trimmed;
                                            play_earcon(Earcon::Success);
                                        }
                                    }
                                    ctx.input_mut(|i| i.consume_key(egui::Modifiers::COMMAND, egui::Key::V));
                                    ctx.input_mut(|i| i.consume_key(egui::Modifiers::CTRL, egui::Key::V));
                                }
                                let model_edit = egui::TextEdit::singleline(&mut self.model_input)
                                    .id(model_input_id)
                                    .hint_text(default_model_name)
                                    .desired_width(240.0);
                                let model_resp = ui.add(model_edit);

                                model_resp.context_menu(|ui| {
                                    if ui.button(t!("ui.btn_paste")).clicked() {
                                        if let Some(clip) = get_clipboard_text() {
                                            self.model_input = clip.trim().to_string();
                                        }
                                        ui.close();
                                    }
                                    if ui.button(t!("ui.btn_copy")).clicked() {
                                        set_clipboard_text(&self.model_input);
                                        ui.close();
                                    }
                                    if ui.button(t!("ui.restore_default_model", model = default_model_name)).clicked() {
                                        self.model_input = default_model_name.to_string();
                                        ui.close();
                                    }
                                    if ui.button(t!("ui.btn_clear_short")).clicked() {
                                        self.model_input.clear();
                                        ui.close();
                                    }
                                });

                            });

                            ui.add_space(5.0);

                            // --- Section Point d'accès (Endpoint) ---
                            ui.horizontal(|ui| {
                                ui.label(
                                    egui::RichText::new("Point d'accès API (Endpoint) :")
                                        .size(11.0)
                                        .color(egui::Color32::WHITE)
                                        .strong(),
                                );
                                ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                                    if ui.small_button("Défaut")
                                        .on_hover_text(format!("Réinitialiser vers : {}", default_endpoint_url))
                                        .clicked()
                                    {
                                        self.endpoint_input = default_endpoint_url.to_string();
                                    }
                                });
                            });
                            ui.add_space(2.0);

                            ui.horizontal(|ui| {
                                let endpoint_edit = egui::TextEdit::singleline(&mut self.endpoint_input)
                                    .hint_text(default_endpoint_url)
                                    .desired_width(320.0);
                                ui.add(endpoint_edit)
                                    .on_hover_text("URL HTTP/HTTPS complète pour Ollama, LM Studio, vLLM, LocalAI ou proxies compatibles");
                            });
                            }

                            if let Some(ref err) = self.api_key_error {
                                ui.add_space(2.0);
                                ui.label(egui::RichText::new(err).color(egui::Color32::from_rgb(255, 110, 110)).size(11.0));
                            }

                            if self.selected_provider_modal == LlmProvider::Local {
                                ui.add_space(4.0);
                                ui.label(egui::RichText::new("Aucune clé API requise pour le moteur local.").size(10.5).color(egui::Color32::from_rgb(140, 200, 255)));
                                ui.add_space(2.0);
                            }

                            ui.add_space(5.0);
                            ui.horizontal(|ui| {
                                if ui.button(t!("ui.btn_save_apply")).clicked() {
                                    let key = self.api_key_input.trim();
                                    let model = if self.model_input.trim().is_empty() {
                                        default_model_name
                                    } else {
                                        &self.model_input
                                    };
                                    let endpoint = if self.endpoint_input.trim().is_empty() {
                                        default_endpoint_url
                                    } else {
                                        &self.endpoint_input
                                    };

                                    if self.selected_provider_modal == LlmProvider::Local {
                                        let _ = write_env_var_to_file(".env", "LLM_PROVIDER", self.selected_provider_modal.display_name());
                                        std::env::set_var("LLM_PROVIDER", self.selected_provider_modal.display_name());
                                        let active_p = local_llm::get_active_preset();
                                        local_llm::set_active_preset(active_p.id);
                                        let _ = write_env_var_to_file(".env", "LLM_MODEL", active_p.filename);
                                        std::env::set_var("LLM_MODEL", active_p.filename);
                                        let _ = write_env_var_to_file(".env", "LOCAL_LLAMA_MODEL", active_p.filename);
                                        std::env::set_var("LOCAL_LLAMA_MODEL", active_p.filename);
                                        self.llm_config = resolve_llm_config();
                                        self.llm_config.model = active_p.name.to_string();
                                        self.show_api_key_modal = false;
                                        self.api_key_error = None;
                                    } else if let Err(e) = write_env_var_to_file(".env", env_var_target, key) {
                                        self.api_key_error = Some(t!("ui.api_key_write_err", err = e).to_string());
                                    } else if let Err(e) = write_env_var_to_file(".env", env_model_target, model) {
                                        self.api_key_error = Some(t!("ui.model_write_err", err = e).to_string());
                                    } else if let Err(e) = write_env_var_to_file(".env", env_endpoint_target, endpoint) {
                                        self.api_key_error = Some(format!("Erreur sauvegarde endpoint: {e}"));
                                    } else {
                                        let _ = write_env_var_to_file(".env", "LLM_PROVIDER", self.selected_provider_modal.display_name());
                                        std::env::set_var("LLM_PROVIDER", self.selected_provider_modal.display_name());
                                        std::env::set_var(env_var_target, key);
                                        std::env::set_var(env_model_target, model);
                                        std::env::set_var(env_endpoint_target, endpoint);
                                        self.llm_config = resolve_llm_config();
                                        self.show_api_key_modal = false;
                                    }
                                }
                                if ui.button(t!("ui.btn_cancel")).clicked() {
                                    self.show_api_key_modal = false;
                                    self.api_key_error = None;
                                }
                            });
                                }
                                SettingsTab::Theme => {
                            // --- Onglet Thèmes & Accessibilité ---
                            ui.label(
                                egui::RichText::new(t!("ui.theme_profiles_label"))
                                    .size(11.5)
                                    .color(theme.text_secondary),
                            );
                            ui.add_space(6.0);

                            let mut current_theme = self.theme;
                            ui.horizontal(|ui| {
                                ui.label(egui::RichText::new(t!("ui.theme_label")).size(12.0).strong().color(egui::Color32::WHITE));
                                egui::ComboBox::from_id_salt("settings_theme_combobox")
                                    .selected_text(localized_theme_name(current_theme))
                                    .width(260.0)
                                    .show_ui(ui, |ui| {
                                        for &t in AppTheme::ALL {
                                            let t_colors = t.colors();
                                            ui.horizontal(|ui| {
                                                if ui.selectable_value(&mut current_theme, t, localized_theme_name(t)).clicked() {
                                                    self.set_theme(t);
                                                }
                                                let (rect, _) = ui.allocate_exact_size(egui::vec2(44.0, 12.0), egui::Sense::hover());
                                                let p = ui.painter();
                                                p.rect_filled(egui::Rect::from_min_size(rect.min, egui::vec2(10.0, 12.0)), egui::CornerRadius::same(2), t_colors.control_bg);
                                                p.rect_filled(egui::Rect::from_min_size(egui::pos2(rect.min.x + 11.0, rect.min.y), egui::vec2(10.0, 12.0)), egui::CornerRadius::same(2), t_colors.border);
                                                p.rect_filled(egui::Rect::from_min_size(egui::pos2(rect.min.x + 22.0, rect.min.y), egui::vec2(10.0, 12.0)), egui::CornerRadius::same(2), t_colors.accent);
                                                p.rect_filled(egui::Rect::from_min_size(egui::pos2(rect.min.x + 33.0, rect.min.y), egui::vec2(10.0, 12.0)), egui::CornerRadius::same(2), t_colors.text_user);
                                            });
                                        }
                                    });
                            });

                            if current_theme != self.theme {
                                self.set_theme(current_theme);
                            }

                            ui.add_space(8.0);
                            let cur_colors = self.theme.colors();
                            egui::Frame::new()
                                .fill(cur_colors.control_bg)
                                .stroke(egui::Stroke::new(1.0, cur_colors.border))
                                .corner_radius(egui::CornerRadius::same(8))
                                .inner_margin(egui::Margin::symmetric(10, 8))
                                .show(ui, |ui| {
                                    ui.horizontal(|ui| {
                                        ui.label(egui::RichText::new(localized_theme_name(self.theme)).strong().size(12.0).color(cur_colors.accent));
                                        ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                                            let (rect, _) = ui.allocate_exact_size(egui::vec2(58.0, 14.0), egui::Sense::hover());
                                            let p = ui.painter();
                                            p.rect_filled(egui::Rect::from_min_size(rect.min, egui::vec2(14.0, 14.0)), egui::CornerRadius::same(2), cur_colors.control_bg);
                                            p.rect_filled(egui::Rect::from_min_size(egui::pos2(rect.min.x + 15.0, rect.min.y), egui::vec2(14.0, 14.0)), egui::CornerRadius::same(2), cur_colors.border);
                                            p.rect_filled(egui::Rect::from_min_size(egui::pos2(rect.min.x + 30.0, rect.min.y), egui::vec2(14.0, 14.0)), egui::CornerRadius::same(2), cur_colors.accent);
                                            p.rect_filled(egui::Rect::from_min_size(egui::pos2(rect.min.x + 45.0, rect.min.y), egui::vec2(13.0, 14.0)), egui::CornerRadius::same(2), cur_colors.text_user);
                                        });
                                    });
                                    ui.add_space(3.0);
                                    ui.label(
                                        egui::RichText::new(localized_theme_desc(self.theme))
                                            .size(10.5)
                                            .color(cur_colors.text_secondary)
                                            .italics(),
                                    );
                                });

                            ui.add_space(8.0);
                            ui.separator();
                            ui.add_space(4.0);
                            ui.label(
                                egui::RichText::new(t!("ui.font_size_label"))
                                    .size(11.5)
                                    .color(theme.text_secondary),
                            );
                            ui.add_space(6.0);
                            let mut current_zoom = self.zoom_factor;
                            let prev_zoom = current_zoom;
                            ui.horizontal(|ui| {
                                ui.spacing_mut().item_spacing.x = 8.0;
                                ui.label(egui::RichText::new(t!("ui.font_label")).size(12.0).strong().color(egui::Color32::WHITE));

                                let btn_minus = egui::Button::new(
                                    egui::RichText::new("A −")
                                        .size(13.0)
                                        .strong()
                                        .color(egui::Color32::WHITE),
                                )
                                .min_size(egui::vec2(52.0, 30.0))
                                .corner_radius(egui::CornerRadius::same(6));
                                let resp_minus = ui.add(btn_minus).on_hover_text(t!("ui.font_minus_tooltip"));
                                let id_minus = ui.make_persistent_id("dwell_zoom_minus");
                                if apply_dwell_to_response(ui, &mut self.dwell_state, id_minus, &resp_minus) {
                                    current_zoom = ((current_zoom - 0.10).max(0.70) * 100.0).round() / 100.0;
                                }

                                ui.add(
                                    egui::Label::new(
                                        egui::RichText::new(format!("{:.0} %", current_zoom * 100.0))
                                            .size(13.0)
                                            .strong()
                                            .color(theme.accent),
                                    )
                                );

                                let btn_plus = egui::Button::new(
                                    egui::RichText::new("A +")
                                        .size(13.0)
                                        .strong()
                                        .color(egui::Color32::WHITE),
                                )
                                .min_size(egui::vec2(52.0, 30.0))
                                .corner_radius(egui::CornerRadius::same(6));
                                let resp_plus = ui.add(btn_plus).on_hover_text(t!("ui.font_plus_tooltip"));
                                let id_plus = ui.make_persistent_id("dwell_zoom_plus");
                                if apply_dwell_to_response(ui, &mut self.dwell_state, id_plus, &resp_plus) {
                                    current_zoom = ((current_zoom + 0.10).min(2.50) * 100.0).round() / 100.0;
                                }

                                let btn_reset = egui::Button::new(
                                    egui::RichText::new("100 %")
                                        .size(12.0)
                                        .color(theme.text_secondary),
                                )
                                .min_size(egui::vec2(52.0, 30.0))
                                .corner_radius(egui::CornerRadius::same(6));
                                let resp_reset = ui.add(btn_reset).on_hover_text(t!("ui.font_reset_tooltip"));
                                let id_reset = ui.make_persistent_id("dwell_zoom_reset");
                                if apply_dwell_to_response(ui, &mut self.dwell_state, id_reset, &resp_reset) {
                                    current_zoom = 1.0;
                                }
                            });
                            if (current_zoom - prev_zoom).abs() > 0.001 {
                                self.set_zoom_factor(current_zoom);
                                ctx.set_zoom_factor(self.zoom_factor);
                            }

                            ui.add_space(8.0);
                            ui.separator();
                            ui.add_space(4.0);
                            ui.label(
                                egui::RichText::new(t!("ui.dwell_timing_label"))
                                    .size(11.5)
                                    .color(theme.text_secondary),
                            );
                            ui.add_space(3.0);
                            let mut current_dwell_ms = self.dwell_state.dwell_duration.as_millis() as u64;
                            let prev_dwell_ms = current_dwell_ms;
                            ui.horizontal(|ui| {
                                ui.label(egui::RichText::new(t!("ui.dwell_delay_label")).size(12.0).strong().color(egui::Color32::WHITE));
                                let slider = egui::Slider::new(&mut current_dwell_ms, 0..=2500)
                                    .step_by(50.0)
                                    .custom_formatter(|val, _| {
                                        if val <= 0.0 {
                                            t!("ui.dwell_disabled").to_string()
                                        } else {
                                            format!("{:.0} ms", val)
                                        }
                                    });
                                ui.add(slider)
                                    .on_hover_text(t!("ui.dwell_tooltip"));
                            });
                            if current_dwell_ms != prev_dwell_ms {
                                self.dwell_state.dwell_duration = Duration::from_millis(current_dwell_ms);
                                let _ = write_env_var_to_file(".env", "DWELL_TIME_MS", &current_dwell_ms.to_string());
                                std::env::set_var("DWELL_TIME_MS", current_dwell_ms.to_string());
                            }

                            if current_dwell_ms > 0 {
                                ui.add_space(3.0);
                                let mut current_act_delay_ms = self.dwell_state.activation_delay.as_millis() as u64;
                                let prev_act_delay_ms = current_act_delay_ms;
                                ui.horizontal(|ui| {
                                    ui.label(egui::RichText::new("Délai avant affichage de la jauge :").size(12.0).strong().color(egui::Color32::WHITE));
                                    let act_slider = egui::Slider::new(&mut current_act_delay_ms, 0..=600)
                                        .step_by(25.0)
                                        .custom_formatter(|val, _| {
                                            if val <= 0.0 {
                                                "Immédiat (0 ms)".to_string()
                                            } else {
                                                format!("{:.0} ms", val)
                                            }
                                        });
                                    ui.add(act_slider)
                                        .on_hover_text("Délai d'attente avant que la jauge verte ne commence à s'afficher et se remplir, évitant de perturber l'utilisateur lors du simple survol.");
                                });
                                if current_act_delay_ms != prev_act_delay_ms {
                                    self.dwell_state.activation_delay = Duration::from_millis(current_act_delay_ms);
                                    let _ = write_env_var_to_file(".env", "DWELL_ACTIVATION_DELAY_MS", &current_act_delay_ms.to_string());
                                    std::env::set_var("DWELL_ACTIVATION_DELAY_MS", current_act_delay_ms.to_string());
                                }
                            }

                            ui.add_space(6.0);
                            ui.horizontal(|ui| {
                                if ui.button(t!("ui.btn_close")).clicked() {
                                    self.show_api_key_modal = false;
                                }
                                ui.label(
                                    egui::RichText::new(t!("ui.theme_voice_tip"))
                                        .size(10.0)
                                        .color(theme.text_secondary),
                                );
                            });
                                }
                                SettingsTab::Language => {
                                    // --- Onglet Langage ---
                                    ui.label(
                                        egui::RichText::new(t!("ui.language_heading"))
                                            .size(11.5)
                                            .color(theme.text_secondary),
                                    );
                                    ui.add_space(6.0);

                                    let mut selected_lang = self.app_language.clone();
                                    ui.horizontal_wrapped(|ui| {
                                        ui.spacing_mut().item_spacing = egui::vec2(6.0, 6.0);

                                        let options = [
                                            ("auto", "🌐 Système (Auto)"),
                                            ("fr", "🇫🇷 Français"),
                                            ("en", "🇬🇧 English"),
                                            ("de", "🇩🇪 Deutsch"),
                                            ("es", "🇪🇸 Español"),
                                            ("it", "🇮🇹 Italiano"),
                                            ("pt", "🇵🇹 Português"),
                                            ("ru", "🇷🇺 Русский"),
                                            ("ja", "🇯🇵 日本語"),
                                            ("zh", "🇨🇳 中文"),
                                        ];

                                        for (code, label) in options {
                                            let is_selected = selected_lang == code;
                                            let btn = ui.selectable_label(is_selected, label);
                                            let btn_id = ui.make_persistent_id(format!("dwell_lang_{code}"));
                                            if apply_dwell_to_response(ui, &mut self.dwell_state, btn_id, &btn) {
                                                selected_lang = code.to_string();
                                            }
                                        }
                                    });
                                    if selected_lang != self.app_language {
                                        self.set_app_language(&selected_lang);
                                        ctx.request_repaint();
                                    }

                                    ui.add_space(8.0);
                                    ui.separator();
                                    ui.add_space(6.0);

                                    ui.horizontal(|ui| {
                                        if ui.button(t!("ui.btn_close")).clicked() {
                                            self.show_api_key_modal = false;
                                        }
                                        ui.label(
                                            egui::RichText::new(t!("ui.language_voice_tip"))
                                                .size(10.0)
                                                .color(theme.text_secondary),
                                        );
                                    });
                                }
                                SettingsTab::Macros => {
                                    // --- Onglet Raccourcis & Macros ---
                                    ui.label(
                                        egui::RichText::new("Configurez vos raccourcis vocaux et textuels ('raccourci' => texte complet).")
                                            .size(11.5)
                                            .color(theme.text_secondary),
                                    );
                                    ui.add_space(6.0);

                                    egui::Frame::new()
                                        .fill(theme.control_bg)
                                        .stroke(egui::Stroke::new(1.0, theme.border_subtle))
                                        .corner_radius(egui::CornerRadius::same(8))
                                        .inner_margin(egui::Margin::symmetric(10, 8))
                                        .show(ui, |ui| {
                                            ui.label(
                                                egui::RichText::new("Nouveau raccourci macro :")
                                                    .size(11.0)
                                                    .color(theme.accent)
                                                    .strong(),
                                            );
                                            ui.add_space(3.0);

                                            ui.horizontal(|ui| {
                                                ui.label(egui::RichText::new("Raccourci :").size(11.0).color(egui::Color32::WHITE));
                                                ui.add(
                                                    egui::TextEdit::singleline(&mut self.macro_shortcut_input)
                                                        .hint_text("ex: actu, colis, aide")
                                                        .desired_width(120.0),
                                                );
                                            });
                                            ui.add_space(3.0);

                                            ui.horizontal(|ui| {
                                                ui.label(egui::RichText::new("Texte complet :").size(11.0).color(egui::Color32::WHITE));
                                                ui.add(
                                                    egui::TextEdit::singleline(&mut self.macro_text_input)
                                                        .hint_text("ex: Donne le résumé des actualités du jour")
                                                        .desired_width(260.0),
                                                );
                                            });
                                            ui.add_space(4.0);

                                            if let Some(ref err) = self.macro_error {
                                                ui.label(egui::RichText::new(err).color(egui::Color32::from_rgb(255, 110, 110)).size(10.5));
                                                ui.add_space(2.0);
                                            }

                                            ui.horizontal(|ui| {
                                                if ui.button(egui::RichText::new("➕ Ajouter la macro").strong().color(theme.accent)).clicked() {
                                                    let sc = self.macro_shortcut_input.trim().to_string();
                                                    let tx = self.macro_text_input.trim().to_string();
                                                    if sc.is_empty() {
                                                        self.macro_error = Some("Le raccourci ne peut pas être vide.".to_string());
                                                    } else if tx.is_empty() {
                                                        self.macro_error = Some("Le texte complet ne peut pas être vide.".to_string());
                                                    } else {
                                                        self.macros.retain(|(k, _)| !k.eq_ignore_ascii_case(&sc));
                                                        self.macros.push((sc, tx));
                                                        let _ = save_macros(&self.macros);
                                                        self.macro_shortcut_input.clear();
                                                        self.macro_text_input.clear();
                                                        self.macro_error = None;
                                                        play_earcon(Earcon::Success);
                                                    }
                                                }
                                            });
                                        });

                                    ui.add_space(8.0);
                                    ui.separator();
                                    ui.add_space(6.0);

                                    ui.label(
                                        egui::RichText::new(format!("Macros actives ({}) :", self.macros.len()))
                                            .size(11.5)
                                            .strong()
                                            .color(egui::Color32::WHITE),
                                    );
                                    ui.add_space(4.0);

                                    let mut to_remove = None;
                                    egui::ScrollArea::vertical()
                                        .max_height(140.0)
                                        .auto_shrink([false, false])
                                        .show(ui, |ui| {
                                            if self.macros.is_empty() {
                                                ui.label(
                                                    egui::RichText::new("Aucune macro configurée.")
                                                        .size(11.0)
                                                        .italics()
                                                        .color(theme.text_secondary),
                                                );
                                            } else {
                                                for (idx, (sc, tx)) in self.macros.iter().enumerate() {
                                                    ui.horizontal(|ui| {
                                                        ui.label(
                                                            egui::RichText::new(format!("⚡ {}", sc))
                                                                .size(11.0)
                                                                .color(theme.accent)
                                                                .strong(),
                                                        );
                                                        ui.label(egui::RichText::new("➔").size(10.0).color(theme.text_secondary));
                                                        let preview = if tx.len() > 38 {
                                                            format!("{}...", &tx[..35])
                                                        } else {
                                                            tx.clone()
                                                        };
                                                        ui.label(egui::RichText::new(preview).size(11.0).color(egui::Color32::WHITE))
                                                            .on_hover_text(tx);

                                                        ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                                                            if ui.small_button("🗑").on_hover_text("Supprimer").clicked() {
                                                                to_remove = Some(idx);
                                                            }
                                                        });
                                                    });
                                                    ui.add_space(2.0);
                                                }
                                            }
                                        });

                                    if let Some(idx) = to_remove {
                                        self.macros.remove(idx);
                                        let _ = save_macros(&self.macros);
                                        play_earcon(Earcon::Success);
                                    }
                                }
                            }
                                });
                        });
                }

                // Affichage dynamique des fenêtres volantes (stats, tables, contenus, formulaires de requête)
                let mut submitted_card_prompts = Vec::new();
                let mut card_speech = None;
                let prev_card_count = self.floating_cards.len();
                let had_subtitles = self.floating_cards.iter().any(|c| matches!(c.content, FloatingContentType::Subtitles { .. }));
                let dwell_state = &mut self.dwell_state;
                let theme_colors = self.theme.colors();
                self.floating_cards.retain_mut(|card| {
                    if let Some(out) = card.show(&ctx, dwell_state, &theme_colors) {
                        match out {
                            FloatingCardOutput::Prompt(prompt) => submitted_card_prompts.push(prompt),
                            FloatingCardOutput::Speak(speech) => card_speech = Some(speech),
                        }
                    }
                    card.open
                });
                let has_subtitles = self.floating_cards.iter().any(|c| matches!(c.content, FloatingContentType::Subtitles { .. }));
                if had_subtitles && !has_subtitles {
                    floating_window::stop_live_subtitles();
                    crate::audio::stop_live_subtitles();
                }
                if self.floating_cards.len() != prev_card_count {
                    update_open_floating_cards(&self.floating_cards);
                    ctx.request_repaint();
                }
                if let Some(speech) = card_speech {
                    self.stop_speaking();
                    play_earcon(Earcon::Success);
                    self.status = AgentStatus::Speaking;
                    self.chat_history.push(ChatEntry {
                        role: ChatRole::Agent,
                        text: speech.clone(),
                        timestamp: current_time_str(),
                        quick_suggestions: Vec::new(),
                        screen_size_kb: None,
                    });
                    let _ = self.tts_sender.send(TtsCommand::Speak(speech));
                }
                for prompt in submitted_card_prompts {
                    self.stop_speaking();
                    if self.is_recording {
                        self.stop_recording();
                    }
                    self.status = AgentStatus::Thinking;
                    self.continuous_mode = true;
                    self.live_transcript.clear();

                    if let Some(card) = is_read_card_intent(&prompt, &self.floating_cards) {
                        let speech = card.build_narrator_speech();
                        play_earcon(Earcon::Success);
                        self.status = AgentStatus::Speaking;
                        self.chat_history.push(ChatEntry {
                            role: ChatRole::User,
                            text: prompt.clone(),
                            timestamp: current_time_str(),
                            quick_suggestions: Vec::new(),
                            screen_size_kb: None,
                        });
                        self.chat_history.push(ChatEntry {
                            role: ChatRole::Agent,
                            text: speech.clone(),
                            timestamp: current_time_str(),
                            quick_suggestions: Vec::new(),
                            screen_size_kb: None,
                        });
                        let _ = self.tts_sender.send(TtsCommand::Speak(speech));
                        continue;
                    }

                    let norm_prompt = normalize_for_command_match(&prompt);
                    if is_start_subtitles_command(&norm_prompt) {
                        self.start_live_subtitles(None);
                        continue;
                    }
                    if is_stop_subtitles_command(&norm_prompt) {
                        self.stop_live_subtitles();
                        continue;
                    }
                    self.chat_history.push(ChatEntry {
                        role: ChatRole::User,
                        text: prompt.clone(),
                        timestamp: current_time_str(),
                        quick_suggestions: Vec::new(),
                        screen_size_kb: None,
                    });
                    let _ = self.command_sender.send(AgentCommand::Prompt(prompt));
                }
            });
    }
}

struct BusyGuard;
impl Drop for BusyGuard {
    fn drop(&mut self) {
        AGENT_BUSY.store(false, Ordering::SeqCst);
    }
}

/// Envoi unifié d'une requête de complétion selon le fournisseur configuré
pub(crate) async fn send_llm_chat_completion(
    client: &reqwest::Client,
    config: &LlmConfig,
    system_prompt: &str,
    messages_history: &[ChatMessage],
    temperature: f32,
    max_tokens: u32,
) -> Result<reqwest::Response, reqwest::Error> {
    if config.provider == LlmProvider::Anthropic {
        let mut anthropic_messages: Vec<ChatMessage> = Vec::new();
        for msg in messages_history {
            if msg.role == "system" {
                continue;
            }
            let role = if msg.role == "assistant" { "assistant" } else { "user" };
            if let Some(last) = anthropic_messages.last_mut() {
                if last.role == role {
                    last.content.push_str("\n\n");
                    last.content.push_str(&msg.content);
                    continue;
                }
            }
            anthropic_messages.push(ChatMessage {
                role: role.to_string(),
                content: msg.content.clone(),
            });
        }
        if anthropic_messages.first().map_or(false, |m| m.role != "user") {
            anthropic_messages.insert(0, ChatMessage {
                role: "user".to_string(),
                content: "Bonjour".to_string(),
            });
        }

        let req_payload = AnthropicMessagesRequest {
            model: config.model.clone(),
            system: if system_prompt.trim().is_empty() { None } else { Some(system_prompt.to_string()) },
            messages: anthropic_messages,
            max_tokens,
            temperature,
        };

        let mut req = client
            .post(&config.endpoint)
            .header("anthropic-version", "2023-06-01")
            .header("content-type", "application/json");

        if !config.api_key.trim().is_empty() {
            req = req.header("x-api-key", &config.api_key);
        }

        req
            .json(&req_payload)
            .send()
            .await
    } else {
        let mut messages = Vec::new();
        if !system_prompt.trim().is_empty() {
            messages.push(ChatMessage {
                role: "system".to_string(),
                content: system_prompt.to_string(),
            });
        }
        messages.extend_from_slice(messages_history);

        let response_format = match config.provider {
            LlmProvider::DeepSeek | LlmProvider::OpenAI | LlmProvider::Mistral | LlmProvider::Groq => {
                Some(ChatResponseFormat {
                    r#type: "json_object".to_string(),
                })
            }
            _ => None,
        };

        let req_payload = DeepSeekChatRequest {
            model: config.model.clone(),
            messages,
            temperature,
            max_tokens,
            top_p: 1.0,
            stream: false,
            response_format,
        };

        let mut req = client.post(&config.endpoint);
        let key_clean = config.api_key.trim();
        if !key_clean.is_empty() {
            req = req.bearer_auth(key_clean);
        }

        req
            .json(&req_payload)
            .send()
            .await
    }
}

pub(crate) async fn parse_llm_response_text(res: reqwest::Response, provider: LlmProvider) -> Result<String, String> {
    if provider == LlmProvider::Anthropic {
        let body = res.json::<AnthropicMessagesResponse>().await
            .map_err(|e| format!("Erreur décodage JSON Anthropic: {e}"))?;
        let content = body.content
            .into_iter()
            .filter(|b| b.block_type == "text")
            .filter_map(|b| b.text)
            .collect::<Vec<_>>()
            .join("\n");
        if content.is_empty() {
            Err("Réponse Anthropic vide".to_string())
        } else {
            Ok(content)
        }
    } else {
        let body = res.json::<DeepSeekChatResponse>().await
            .map_err(|e| format!("Erreur décodage JSON OpenAI/DeepSeek: {e}"))?;
        if let Some(choice) = body.choices.first() {
            Ok(choice.message.content.clone())
        } else {
            Err("Aucun choix retourné dans la réponse".to_string())
        }
    }
}

async fn compact_history_if_needed(
    llm_config: &LlmConfig,
    history: &mut Vec<ChatMessage>,
    client: &reqwest::Client,
    last_call_time: &mut Option<Instant>,
) {
    const COMPACTION_CHAR_THRESHOLD: usize = 6000;
    const COMPACTION_MSG_THRESHOLD: usize = 8;
    const KEEP_RECENT_COUNT: usize = 3;

    let total_chars: usize = history.iter().map(|m| m.content.len()).sum();
    if (history.len() < COMPACTION_MSG_THRESHOLD && total_chars < COMPACTION_CHAR_THRESHOLD)
        || history.len() <= KEEP_RECENT_COUNT
    {
        return;
    }

    let split_idx = history.len().saturating_sub(KEEP_RECENT_COUNT);
    let to_summarize = &history[..split_idx];
    let recent = history[split_idx..].to_vec();

    let mut conversation_text = String::new();
    for msg in to_summarize {
        conversation_text.push_str(&format!("{}: {}\n", msg.role, msg.content));
    }

    let safe_conversation_text = truncate_with_notice(&conversation_text, 8000);

    // Respecter un délai de debounce avant l'appel de condensation
    let min_debounce = Duration::from_millis(3000);
    if let Some(prev) = *last_call_time {
        let elapsed = prev.elapsed();
        if elapsed < min_debounce {
            tokio::time::sleep(min_debounce - elapsed).await;
        }
    }

    let (_, system_lang_name) = get_system_locale_language();
    let sys_prompt = format!("Tu es un synthétiseur de mémoire conversationnelle. Résume fidèlement les points clés, décisions et actions passées en 250 mots maximum dans la langue du système de l'utilisateur ({system_lang_name}) sous forme de points synthétiques.");
    let messages_to_condense = vec![
        ChatMessage {
            role: "user".to_string(),
            content: format!("Voici les échanges passés à condenser :\n\n{}", safe_conversation_text),
        },
    ];

    let response = send_llm_chat_completion(
        client,
        llm_config,
        &sys_prompt,
        &messages_to_condense,
        0.2,
        400,
    ).await;

    *last_call_time = Some(Instant::now());

    if let Ok(res) = response {
        if res.status().is_success() {
            if let Ok(summary_text) = parse_llm_response_text(res, llm_config.provider).await {
                let summary = summary_text.trim();
                if !summary.is_empty() {
                    println!("[Mémoire] Compaction contextuelle réussie ({} messages condensés).", to_summarize.len());
                    let mut new_history = Vec::with_capacity(recent.len() + 2);
                    new_history.push(ChatMessage {
                        role: "user".to_string(),
                        content: format!("[Note contextuelle - résumé des échanges antérieurs] :\n{}", summary),
                    });
                    new_history.push(ChatMessage {
                        role: "assistant".to_string(),
                        content: "Contexte précédent bien assimilé.".to_string(),
                    });
                    new_history.extend(recent);
                    *history = new_history;
                    return;
                }
            }
        }
    }

    // Repli de sécurité en cas d'erreur de condensation
    println!("[Mémoire] Repli : éviction fifo sans résumé.");
    *history = recent;
}

pub fn build_standard_system_instructions(lang_name: &str) -> String {
    format!(r###"Tu es l'agent d'exploration Libertide multiplateforme (Windows, macOS, Linux).
RÈGLE LINGUISTIQUE PRIORITAIRE ET ABSOLUE :
Tu DOIS IMPÉRATIVEMENT t'exprimer et rédiger l'intégralité de tes réponses (champ "narration", "quick_suggestions" et fenêtres volantes) exclusivement dans la langue configurée par l'utilisateur : {lang_name}.
Même si les fenêtres, le texte de l'écran ou les pages web visitées sont dans une autre langue (par exemple en anglais, espagnol ou allemand), tu DOIS TOUT TRADUIRE et restituer dans la langue de l'utilisateur ({lang_name}). Aucune exception.

Ton rôle ultime (leimotiv) est de rendre compte des évolutions informationnelles à l'écran, mais aussi d'assister les personnes (handicapées ou non)
dans leur demande d'information, ou propension à discuter. Ne mentionne pas tes outils de navigation, mais utilise les à bon escient pour anticiper et appliquer les souhaits de l'utilisateur.

Si l'utilisateur demande quelque chose, le réflexe est de chercher sur google
L'analyse de l'écran ne doit servir qu'à la navigation et à l'information

INTERDICTION FORMELLE DE CONSEIL PASSIF ("Il faudrait ouvrir...", "Vous devez ouvrir...", "Je ne trouve pas...") :
Tu es un agent exécutant autonome, pas un conseiller. Si l'application ou le média demandé (ex: radio, musique, actualité, météo, service) n'est pas présent à l'écran, TU DOIS OUVRIR TOI-MÊME LE NAVIGATEUR immédiatement avec "navigate_to_url" (vers le site officiel de radio, streaming ou Google) ou "open_search_browser". Ne dis JAMAIS "il faudrait", FAIS-LE !

Utilise le "Je" et l'écriture inclusive, et projette toi une forte personnalité sans géner l'accessibilité.
N'anticipes pas des actions que l'utilisateur n'a pas demandé, sauf sur l'information retournée.

Réponds STRICTEMENT en JSON sans texte hors JSON. "narration" détaillée, explicative et bienveillante rédigée impérativement dans la langue de l'utilisateur : {lang_name} (2 à 4 phrases complètes). Décris précisément ce qui se passe à l'écran, l'agencement visuel, les changements constatés et le résultat des actions pour offrir un guidage riche et rassurant aux personnes en situation de handicap (visuel, moteur ou cognitif).

Règles de syntaxe JSON strictes :
- N'échappe JAMAIS les apostrophes simples : écris "d'accueil", "l'écran", "c'est" et SURTOUT PAS "d\'accueil" ou "l\'écran". L'échappement \' est une erreur de syntaxe en JSON.
- Dans "target_name" de "click_element" ou "button_name" de "click_button", écris UNIQUEMENT le texte lisible ou titre du bouton/lien, SANS le préfixe (#id), SANS indicateur de type ([lien], [bouton]), SANS URL et sans guillemets imbriqués (ex: pour '#48 [lien] "Écouter la radio"', écris {{"action": "click_element", "target_name": "Écouter la radio"}}).
- Vérifie toujours la fermeture de tes tableaux JSON : chaque crochet ouvrant [ dans "actions": [ DOIT être refermé par ] avant l'accolade finale }} (format : {{"actions": [{{...}}]}}).

Règles principales :
1. Filtrage requête & interdiction d'abandon : Bruit/inintelligible -> "invalid_request": true. Salutations/chit-chat -> "invalid_request": false, "actions": [], narration courtoise. Dès qu'un verbe d'action ou une demande concrète est exprimée ("mets", "ouvre", "cherche", "lance", "trouve", "clique"), INTERDICTION d'abandonner avec "actions": [] : déclenche l'action locale ou la recherche web.
2. Budget de raisonnement, continuation obligatoire et gestion de boucle ("next_passes": 0..15 ou action "set_reasoning_passes") :
    - CONTINUATION OBLIGATOIRE TANT QUE L'OBJECTIF N'EST PAS SATISFAIT :
      Tant que le but final exprimé par l'utilisateur n'est pas accompli ou résolu, TU DOIS CONTINUER les passes successives (en fournissant l'action concrète suivante et "next_passes" > 0, dans la limite du nombre de passes allouées). Ne t'arrête JAMAIS à mi-chemin avec "actions": [] si la tâche demande d'autres étapes.
    - DÉTECTION DE BOUCLE ET PIVOT STRATÉGIQUE :
      Si tu constates que tu boucles (action précédente sans effet, même écran, lien inopérant ou résultat redondant), INTERDICTION de répéter bêtement la même action. Change immédiatement d'approche : tente un autre résultat, fais défiler ("scroll"), reviens en arrière ("navigate_back"), affine la recherche, ou essaie une autre fenêtre/application. Si un blocage complet s'avère réellement insurmontable après avoir tout essayé, explique l'impasse dans "narration", stoppe ("next_passes": 0, "actions": []) et suggère des alternatives dans "quick_suggestions".
    - ARRÊT IMMÉDIAT DÈS OBJECTIF ATTEINT (RÈGLE PRIORITAIRE) :
      Dès que l'objectif principal exprimé par l'utilisateur est accompli ou satisfait (ex. application demandée ouverte, information trouvée, média/radio lancé, action ou saisie effectuée) :
      TU DOIS ARRÊTER TOUTE PASSE SUPPLÉMENTAIRE. Renvoie impérativement "actions": [], "next_passes": 0 et conclus dans "narration" avec "quick_suggestions".
      Il est formellement interdit de poursuivre l'exploration ou d'exécuter des clics superflus une fois la demande initiale satisfaite.
    - UN SEUL CLIC PAR PASSE (RÈGLE STRICTE) : "actions" ne doit contenir qu'UNE SEULE action de clic ("click_element", "click_button" ou "click_numbered_target") à la fois. INTERDICTION FORMELLE d'enchaîner deux clics consécutifs dans la même passe, car la page change dès le premier clic !
    - ÉVALUATION DU BUDGET TANT QUE L'OBJECTIF N'EST PAS ENCORE ATTEINT :
      * Basse importance / Réponse directe / Simple clic / Tâche ponctuelle rapide ("vite", "juste un coup d'œil", information simple) : 1 à 3 passes.
      * Importance normale / Tâche standard multi-étapes (navigation web, formulaire, lecture, interaction applicative standard) : 4 à 7 passes.
      * Haute importance / Recherche approfondie / Mission de contact / Comparatif de produits ou services : alloue un budget de passes profondes ("next_passes": 8 à 12) jusqu'à obtention des résultats complets.
      * Importance critique / Urgence vitale explicite / Grand départ précipité : 13 à 15 passes.
    - MODULATION DYNAMIQUE : Réévalue "next_passes" à chaque étape selon l'avancement réel. Dès que le résultat final est atteint, bascule sans délai sur "next_passes": 0 et "actions": [].
  3. PROTOCOLE URGENCE VITALE & SANTÉ :
   - CAS 1 - DEMANDE EXPLICITE D'ALERTE / SECOURS (ex: "alerte Robin je fais un malaise", "appelle le 15", "aide médicale d'urgence") :
     * IMMÉDIATEMENT "next_passes": 15.
     * SANS ATTENDRE : exécute immédiatement l'envoi d'alerte ("actions": [...]).
     * Narration : 1 phrase courte confirmant l'envoi + rappelle secours (15 SAMU, 18 Pompiers, 112).
     * Silence parasite : appelle "mute_audio" (isole bruits tiers, garde voix Libertide audible).
     * PIVOT OBLIGATOIRE : Tant que l'alerte n'est pas envoyée et qu'il reste des passes, INTERDICTION de renvoyer "actions": [].
   - CAS 2 - DIDASCALIES (*gémissements*, *soupir*, *aïe*, [pleurs], etc.), JEU DE RÔLE, CITATION OU SITUATION AMBIGUË SANS DEMANDE FORMELLE :
     * INTERDICTION STRICTE DE PANIQUER OU D'ENVOYER DES MESSAGES À DES TIERS : conserve impérativement "actions": [].
     * Reste calme et attentif. Vérifie la situation dans la narration (ex: "J'ai perçu des signes de malaise (*gémissements*). Avez-vous besoin d'une assistance ou d'alerter un proche ?").
     * Fournis OBLIGATOIREMENT une demande de confirmation dans "quick_suggestions" : ["Oui, alerter mes proches / secours", "Non, tout va bien / fausse alerte"].
   - Départ précipité : autonomie pour sauver fichiers et fermer session / verrouiller écran.
 3bis. PROTOCOLE D'ANALYSE APPROFONDIE STRATÉGIQUE (CONTACTS, PRODUITS ET SERVICES) :
   - Dès que la demande implique de trouver ou contacter une personne, trouver des coordonnées, ou rechercher / comparer des produits et services :
     * GESTION DU BUDGET : Définis immédiatement "next_passes": 8 à 12 (ou appelle l'action "set_reasoning_passes": 10). INTERDICTION de clore la mission en 1 seule passe ou de poser une question passive avec "actions": [].
     * VOLET CONTACTS & COMMUNICATION ("contacte...", "écris à...", "trouve le contact", "appelle...") :
       - Niveau 1 (Local) : Vérifie et sélectionne les messageries ou logiciels mail ouverts (Teams, Outlook, WhatsApp, Discord, Slack) et cherche le contact.
       - Niveau 2 (Web & Annuaires pros) : Si absent en local ou application fermée, ouvre le navigateur ("navigate_to_url") pour chercher sur le web, LinkedIn, annuaires d'entreprises ou pages professionnelles.
       - Niveau 3 (Projection fiche contact) : Dès identification des coordonnées (email, téléphone, poste, profil), projette OBLIGATOIREMENT une fiche volante de synthèse via "show_floating_card" (card_type: "stats", id: "fiche_contact", title: "Fiche contact : <Nom>", data: {{"Nom": "...", "Email": "...", "Téléphone": "...", "Poste / Société": "...", "Source": "..."}}), puis prépare le canal de communication (mailto:, message, appel).
     * VOLET PRODUITS & SERVICES ("compare...", "meilleur...", "prix de...", "trouve une offre/produit") :
       - Niveau 1 (Exploration web) : Ouvre le navigateur ("navigate_to_url") sur les moteurs, comparateurs ou sites marchands de référence.
       - Niveau 2 (Extraction métrique) : Relève systématiquement prix, spécifications, disponibilité et notes/avis.
       - Niveau 3 (Tableau comparatif volant) : Projette OBLIGATOIREMENT un tableau comparatif structuré dans une fenêtre volante via "show_floating_card" (card_type: "table", title: "Comparatif offres", data: {{"headers": ["Modèle / Offre", "Prix", "Caractéristiques", "Avis"], "rows": [["...", "...", "...", "..."]]}}).
     * POLITIQUE DE PIVOT EN CAS D'ÉCHEC (TOUS VOLETS) :
       - L'absence de résultat ou le blocage sur un premier canal (ex: contact absent dans Teams, page web non accessible) NE MET PAS FIN À LA MISSION. C'est un ordre de PIVOT IMMÉDIAT vers le canal suivant (local -> web -> annuaires) sans renvoyer "actions": [].
       - Tant que tous les canaux n'ont pas été explorés et que les résultats ne sont pas projetés dans une fenêtre volante ("show_floating_card"), INTERDICTION de renvoyer "actions": [].
 3ter. PROTOCOLE RÉSUMÉ DE PAGE ET SYNTHÈSE DE CONTENU ("résume cette page", "synthèse", "de quoi ça parle", "fais un résumé") :
   - Dès que l'utilisateur souhaite résumer ou analyser une page web, un document ou un ensemble d'informations à l'écran :
     * Analyse minutieusement le texte, titres et points clés de la page.
     * Projette SYSTÉMATIQUEMENT une fenêtre volante au format Markdown via l'action "show_floating_card" :
       card_type: "markdown", id: "resume_page", title: "Résumé : <Titre de la page/sujet>", data: {{"text": "# SLICE Docus — Synthèse de la chaîne\n\n## Identité\n- **Chaîne** : @SliceDocus — **56,4 k abonnés**, **875 vidéos**.\n\n## Vidéos récentes / populaires\n- ▶ La guerre des satellites qui redessine notre monde (2,7 k vues, il y a 20 h)\n- ▶ Galea: les trésors engloutis de Venise (5 k vues, il y a 3 j)\n- ▶ Bio-invasion | Fourmis de feu (95 k vues, il y a 7 j)\n\n## À retenir\n- Formats longs de 34 à 55 min...\n\n> Cliquez sur une vidéo ci-dessus pour la lancer."}}.
     * LIENS D'ACTION & VIDÉOS CLIQUABLES : Dans le texte Markdown et la narration, insère systématiquement des liens cliquables via la macro `[link="action:consigne_ou_url"]Icône Libellé[/link]` (ou en Markdown `Icône Libellé`). Un clic sur le lien ouvre l'URL ou envoie immédiatement la consigne à l'agent.
     * Dans ta "narration" vocale : donne un aperçu oral clair et substantiel des points clés du résumé tout en indiquant sa projection dans la fenêtre volante.
     * Fournis des suggestions rapides utiles dans "quick_suggestions" (ex: ["Lancer la vidéo 1", "Lancer la vidéo 2", "Fermer le résumé"]).
4. Interface & Ciblage :
   - RECHERCHE WEB & NAVIGATION (OUTIL PRINCIPAL) :
     * Dès qu'une information externe, un média (radio, musique), une actualité, ou un service est demandé, la navigation web est ton outil principal. Ouvre directement via "navigate_to_url" ou "open_search_browser".
     * Lorsque des résultats web ou liens sont affichés (Google, Bing, portail), cherche et clique DIRECTEMENT sur le lien textuel ou bouton du résultat qui répond à l'objectif de la demande
   - ACCEPTATION AUTOMATIQUE DES COOKIES : Dès qu'une bannière de cookies ou consentement RGPD est présent, clique SYSTÉMATIQUEMENT et immédiatement sur le bouton pour tout accepter ("Tout accepter", "Accepter tout", "Autoriser", "J'accepte", "Continuer sans refuser") sans demander de confirmation.
   - BANNIÈRES DE NOTIFICATION WEB, PUSH & NEWSLETTERS ("Activez les notifications", "Ne manquez rien", etc.) : Ce ne sont PAS des notifications système Windows (toasts). Ce sont des popups ou bannières intrusives sur la page web. Tu DOIS SYSTÉMATIQUEMENT les traiter immédiatement pour débloquer la page : clique en priorité sur la croix de fermeture ("Fermer (croix ✕)", "croix", "Fermer", "Plus tard", "Refuser", "Non merci"), ou sur "Activer" si demandé ou si aucune option de fermeture n'est présente. Ne laisse JAMAIS une bannière de notification web bloquer la page sans la valider ou la fermer !
   - LECTURE DES NOTIFICATIONS SYSTÈME (TOASTS) : Dès qu'une notification Windows ou un toast est présent, lis impérativement son titre et son contenu dans ta "narration" pour informer l'utilisateur, mais LAISSE-LA TRANQUILLE (ne tente pas de la fermer, ne clique pas dessus et laisse le système la gérer).
   - ALERTES & MESSAGES D'ERREUR (FERMETURE SYSTÉMATIQUE APRÈS LECTURE) : Dès qu'une boîte de dialogue d'alerte, un popup d'avertissement ou un message d'erreur est à l'écran, lis-le clairement ou résume-le dans ta "narration", puis FERME-LA SYSTÉMATIQUEMENT en cliquant sur son bouton de fermeture/validation ("OK", "Fermer", "Annuler", "Continuer", croix) pour débloquer l'écran et poursuivre ton travail. Ne JAMAIS envoyer "escape".
   - PRIORITÉ ABSOLUE AUX CHANGEMENTS WEB : Si des informations, boutons ou données sont présent sur la page web (signalés sous 'INFORMATIONS WEB MODIFIÉES / NOUVELLES'), ils priment impérativement sur tous les éléments statiques et les autres fenêtres. Base en priorité absolue tes actions ou réponses sur ces changements.
   - OBLIGATION DE CIBLAGE EXPLICITE DU CHAMP : Pour toute saisie, sélection ou clic ("write_text", "replace_field_text", "clear_text", "focus_element", "click_element", "click_button"), tu DOIS IMPÉRATIVEMENT renseigner le champ interactable précis visé ("target", "target_name" ou "button_name") en utilisant sa macro ou son identifiant exact observé à l'écran (ex: input, select, textarea, combobox, bouton, lien, video, audio).
   - LECTEURS VIDÉO & AUDIO : Les balises vidéo et audio sont des éléments interactables (signalés par [@video:...], [@audio:...], ou [video]/[audio]). Pour lancer la lecture, mettre en pause ou interagir avec un média, utilise "click_element" ou "click_button" en ciblant la vidéo, l'audio ou ses contrôles de lecture ("play", "lecture", etc.).
   - SÉCURITÉ DU CIBLAGE : "target": null et les cibles génériques ou fictives sont STRICTEMENT INTERDITES. Le moteur d'exécution valide déterministement chaque élément contre l'arbre réel de l'écran ; toute cible non vérifiable ou absente sera rejetée par sécurité.
   - Raccourcis clavier & touches : utilise OBLIGATOIREMENT "accessibility_shortcut" avec le nom ou la combinaison de touches exacte (ex: "action_center", "open_settings", "ctrl+shift+b", "ctrl+t", "enter", "close_tab", "copy", "undo", "space"). Ne JAMAIS envoyer "run_command" pour simuler des touches ou des raccourcis système !
   - Lecture code/fichier : "read_editor_content" (focalise et extrait Ctrl+A/Ctrl+C).
   - Onglet/page : "accessibility_shortcut": "close_tab" (jamais "close_app" pour un onglet).
   - DISTINCTION ABSOLUE ENTRE "open_search" ET "open_search_browser" (2 COMMANDES DISTINCTES) :
     * "open_search" (ou accessibility_shortcut: "open_search") : Recherche SYSTÈME / OS locale (Win+S). Sert STRICTEMENT ET UNIQUEMENT à rechercher un fichier local, une application installée ou un paramètre Windows sur l'ordinateur. INTERDICTION FORMELLE d'utiliser "open_search" pour chercher sur internet ou sur le web !
     * "open_search_browser" (ou "focus_address_bar" / accessibility_shortcut: "open_search_browser") : Recherche WEB / NAVIGATEUR. Sert SYSTÉMATIQUEMENT pour chercher sur le web, aller sur internet, ou saisir une requête dans l'omnibox/barre d'adresse du navigateur. Si l'utilisateur demande de chercher sur le web, Google, YouTube, la météo, un produit, etc., utilise TOUJOURS "open_search_browser" (ou "focus_address_bar" / "navigate_to_url").
 4bis. RÈGLES DE LIENS INTERACTIFS & MACRO [link="cible"]label[/link] (CHAT & FENÊTRES VOLANTES) :
   - RÈGLE D'OR : Tout élément multimédia, ressource, fichier, page web, action ou suggestion mentionné dans "narration" ou dans une fenêtre volante ("show_floating_card") DOIT IMPÉRATIVEMENT être formaté sous forme de lien cliquable, prioritairement avec la macro robuste `[link="cible"]Icône Libellé[/link]` (ou en Markdown `Icône Libellé`).
   - LIENS DANS LES TABLEAUX MARKDOWN : Tu PEUX et DOIS insérer des liens `[link="..."]...[/link]` à l'intérieur des cellules de tableaux Markdown ou de fenêtres volantes (ex: `| Produit | Action | Prix |\n| --- | --- | --- |\n| Drone Pro | [link="action:Commander Drone Pro"]⚡ Commander[/link] | 299€ |`).
   - AVANTAGE MAJEUR DE LA MACRO `[link="..."]...[/link]` : Elle évite tout conflit de parenthèses imbriquées dans les requêtes et les URLs complexes.
   - RÉACTIVITÉ DANS LE CHAT LIBERTIDE : Un clic sur un lien dans le chat ou dans une carte volante insère et envoie automatiquement la consigne dans le chat et déclenche l'exécution par l'agent. La synthèse vocale (TTS) prononce uniquement le libellé sans lire la cible technique ni les balises.
   - TYPOLOGIE DES MACROS OBLIGATOIRES :
     * Vidéos (YouTube, streaming) : `[link="action:Lancer la vidéo : Titre"]▶ Titre[/link]` ou `[link="https://..."]▶ Titre[/link]` (INTERDICTION d'énumérer des vidéos sans lien).
     * Audio & Musique (Spotify, radios, podcasts) : `[link="action:Lancer l'audio : Titre"]🎵 Titre[/link]`.
     * Images & Visuels : `[link="action:Ouvrir l'image : chemin_ou_titre"]🖼️ Titre[/link]`.
     * Documents & PDF (tableurs, rapports, logs) : `[link="action:Ouvrir le fichier : nom.pdf"]📄 nom.pdf[/link]`.
     * Liens Web & Articles : `[link="https://..."]🌐 Nom du site[/link]`.
     * Actions & Clics système : `[link="action:Valider le panier"]⚡ Valider le panier[/link]`.
     * Suggestions de choix : `[link="action:Consigne suggérée"]💡 Option recommandée[/link]`.
5. Audio, Volume & Paramètres système (RÈGLE ABSOLUE ANTI-TERMINAL) :
   - INTERDICTION STRICTE d'utiliser "run_command" avec "explorer.exe", "start ms-settings:sound", "cmd" ou toute commande console pour le son ou les réglages. L'invite de commande ne doit JAMAIS s'ouvrir pour gérer le son !
   - Actions dédiées pour l'audio et les paramètres :
     * Réglage natif du volume : action "set_volume" ("up"|"down"|"mute"|"unmute"|"min") ou "mute_audio".
     * Panneau des réglages audio et choix de sortie (casque/haut-parleur) : action "accessibility_shortcut": "action_center" (ou "sound_settings", simule Win+A sans aucune invite de commande) ou "open_settings" (Win+I).
     * Raccourcis multimédia universels : "accessibility_shortcut": "volume_up" | "volume_down" | "mute_volume".
     * Lecteurs vidéo & musique (YouTube, Deezer, Spotify) : utilise directement "click_button" avec le libellé observé (ex: "Désactiver le son (m)", "Pause (k)") ou "accessibility_shortcut": "m" / "k" / "space". Ne quitte jamais la page pour régler un média web !
   - "focus_address_bar" pour cibler la barre d'adresse du navigateur.
6. Navigation & Web :
   - "navigate_to_url" ouvre par défaut un nouvel onglet ("target": "new_tab").
   - Achat : panier -> valider panier -> AVANT paiement stopper avec "actions": [] et demander confirmation.
   - "scroll" ("down"|"up"|"bottom"|"top", amount: 1..2) pour explorer.
   - RETOUR EN ARRIÈRE & UNDO NAVIGATION : "navigate_back" (ou "accessibility_shortcut": "browser_back", Alt+Gauche) pour revenir immédiatement à la page ou aux résultats précédents si une navigation s'est trompée de page, mène à une impasse ou à un tag erroné. "navigate_forward" pour avancer.
7. Autres actions :
   - Mode survol / numéros ("click_numbered_target") :
     * Le mode survol (qui affiche les numéros) est activé/désactivé par l'utilisateur localement ("mode survol", bouton 🎯). Tu ne dois PAS utiliser d'action pour le basculer.
     * Pour cliquer directement sur un numéro ou badge visible à l'écran, utilise l'action `{{"action": "click_numbered_target", "index": N}}` (ex: 1, 2, 3...).
     * Après un clic, les numéros sont automatiquement rafraîchis.
   - Immersion : "activate_immersion" (Win+Ctrl+D), "deactivate_immersion".
   - Temporisation & Différé : "schedule_prompt" (delay_seconds: 1..3600, prompt: "...") ou "schedule_prompts" (tasks: [{{"delay_seconds": 10, "prompt": "..."}}]) pour différer des consignes futures (téléchargement en cours, rappel, temporisation). "cancel_scheduled_tasks" pour annuler immédiatement toutes les commandes différées / timers en attente.
   - Fichiers & Documents : "open_file" (path: "chemin/fichier.ext") pour ouvrir directement toute image, vidéo, PDF, tableur ou document dans son application système par défaut (selon extension / MIME type).
   - Fenêtres volantes d'analyse & formulaires de requête interactifs : "show_floating_card" (id: "...", title: "...", card_type: "form"|"table"|"stats"|"markdown", data: {{...}}) pour projeter une fenêtre volante avec un formulaire interactif de filtres/requête (inputs, jauges/sliders numériques, selectbox, radio, checkbox pour recherche personnalisée de vols, hôtels, shopping), un tableau comparatif, des données chiffrées ou une fiche technique. Pour "markdown", insère des liens cliquables `[link="action:..."]⚡ Libellé[/link]` pour permettre à l'utilisateur de piloter les choix ou actions directement depuis la fenêtre volante.
   - Fermeture carte : "close_floating_card" (id: "..."). Formate tous les choix, suggestions et références sous forme de liens cliquables cliquables au clic dans le chat.
   - Sous-titres en direct (Loopback audio système) : Dès que l'utilisateur demande d'afficher les sous-titres, transcrire le son du PC, une vidéo ou un stream en direct ("sous-titres en direct", "transcris le son", "active les sous-titres"), utilise impérativement l'action "live_subtitles" ({{"action": "live_subtitles", "enabled": true, "target_window": null}}) ou "show_floating_card" ({{"id": "live_subtitles", "title": "Sous-titres en direct", "card_type": "subtitles", "data": {{"text": ""}}}}). Pour les arrêter, utilise "live_subtitles" ({{"action": "live_subtitles", "enabled": false}}) ou "close_floating_card" ({{"id": "live_subtitles"}}).
 8. Clôture de la tâche : Dès que l'objectif initial est atteint ou que "actions" est vide, renvoie OBLIGATOIREMENT "actions": [], "next_passes": 0, avec 2 à 4 suggestions concrètes dans "quick_suggestions".

Format JSON obligatoire :
{{
  "narration": "Explication vocale concise ou vide.",
  "next_passes": 5,
  "actions": [
    {{"action": "open_app", "name": "nom_executable"}},
    {{"action": "open_search"}},
    {{"action": "open_search_browser"}},
    {{"action": "open_browser", "url": "https://..."}},
    {{"action": "navigate_back"}},
    {{"action": "navigate_forward"}},
    {{"action": "undo"}},
    {{"action": "redo"}},
    {{"action": "close_app", "name": "nom_ou_titre"}},
    {{"action": "click_element", "window": null, "target_name": "nom"}},
    {{"action": "click_button", "window": null, "button_name": "nom"}},
    {{"action": "focus_element", "window": null, "target_name": "nom"}},
    {{"action": "scroll", "direction": "down"|"up"|"bottom"|"top", "amount": 1, "window": null}},
    {{"action": "focus_window", "title": null, "pid": null}},
    {{"action": "focus_address_bar", "window": null}},
    {{"action": "read_editor_content", "window": null, "target": null}},
    {{"action": "clear_text", "window": null}},
    {{"action": "write_text", "text": "...", "target": null, "press_enter": false}},
    {{"action": "dictate", "text": "...", "window": null, "target": null, "press_enter": false}},
    {{"action": "activate_dictation", "window": null, "target": null}},
    {{"action": "deactivate_dictation"}},
    {{"action": "toggle_dictation"}},
    {{"action": "select_text_between", "from": "...", "to": "...", "window": null, "target": null}},
    {{"action": "replace_field_text", "text": "...", "target": null}},
    {{"action": "run_command", "command": "..."}},
    {{"action": "kill_process", "pid": null, "name": null}},
    {{"action": "navigate_to_url", "url": "...", "target": "new_tab"|"current_tab"|"new_window"}},
    {{"action": "accessibility_shortcut", "shortcut": "browser_back"|"browser_forward"|"open_search"|"open_search_browser"|"action_center"|"open_settings"|"sound_settings"|"volume_up"|"volume_down"|"mute_volume"|"enter"|"escape"|"space"|"close_tab"|"refresh_page"|"ctrl+shift+b"|"<combinaison_touches>"}},
    {{"action": "deactivate_immersion"}},
    {{"action": "set_reasoning_passes", "passes": 5, "reason": null}},
    {{"action": "schedule_prompt", "delay_seconds": 10, "prompt": "..."}},
    {{"action": "schedule_prompts", "tasks": [{{"delay_seconds": 10, "prompt": "..."}}]}},
    {{"action": "cancel_scheduled_tasks"}},
    {{"action": "mute_audio"}},
    {{"action": "set_volume", "command": "mute"|"unmute"|"down"|"up"|"min"}},
    {{"action": "live_subtitles", "enabled": true, "target_window": null}},
    {{"action": "click_numbered_target", "index": 1}},
    {{"action": "open_file", "path": "chemin/vers/image_ou_doc.png"}},
    {{"action": "show_floating_card", "id": "stats_1", "title": "Tableau comparatif", "card_type": "markdown", "data": {{"text": "| Modèle | Lien / Action | Prix |\n| --- | --- | --- |\n| Produit A | [link=\"action:Commander Produit A\"]⚡ Commander[/link] | 49€ |\n| Produit B | [link=\"action:Commander Produit B\"]⚡ Commander[/link] | 59€ |"}}}},
    {{"action": "show_floating_card", "id": "vol_search", "title": "Filtres Billet d'Avion", "card_type": "form", "data": {{"submit_label": "Rechercher les vols", "prompt_prefix": "Recherche de vol avec les critères suivants :", "fields": [{{"id": "depart", "label": "Ville de départ", "type": "text", "value": "Paris"}}, {{"id": "destination", "label": "Destination", "type": "text", "value": "Tokyo"}}, {{"id": "prix_max", "label": "Budget maximum", "type": "number_slider", "min": 200, "max": 3000, "step": 50, "value": 900, "unit": "€"}}, {{"id": "classe", "label": "Classe", "type": "select", "options": ["Économique", "Premium", "Affaires"], "selected": "Économique"}}, {{"id": "direct", "label": "Vols directs seulement", "type": "checkbox", "checked": true}}, {{"id": "bagage", "label": "Bagages", "type": "radio", "options": ["Cabine seul", "1 bagage en soute", "2 bagages en soute"], "selected": "1 bagage en soute"}}]}}}},
    {{"action": "show_floating_card", "id": "resume_page", "title": "Résumé de la page", "card_type": "markdown", "data": {{"text": "# SLICE Docus — Synthèse\n## Vidéos populaires\n- [link=\"action:Lancer la vidéo : La guerre des satellites\"]▶ La guerre des satellites (2,7 k vues)[/link]\n- [link=\"action:Lancer la vidéo : Bio-invasion\"]▶ Bio-invasion (95 k vues)[/link]\n\n> Cliquez sur une vidéo pour la lancer."}}}},
     {{"action": "close_floating_card", "id": "stats_1"}}
  ],
  "quick_suggestions": ["Option 1", "Option 2"]
}}"###)
}

const DICTATION_SYSTEM_INSTRUCTIONS: &str = r#"Tu es l'assistant de dictée vocale et de saisie textuelle pour Windows.
Tu ne traites AUCUNE tâche d'exploration, navigation web, clic d'interface ou manipulation de fenêtres.
Tu analyses STRICTEMENT la voix pour retranscrire du texte, déplacer le curseur ou stopper la dictée.
Réponds STRICTEMENT en JSON sans aucun texte autour.

Règles de décision :
1. Arrêt de la dictée ("deactivate_dictation") :
   - Si l'utilisateur demande d'arrêter, quitter, couper ou stopper la dictée (ex: "arrête la dictée", "stop dictée", "quitter la dictée", "pause dictée", "c'est fini", "termine la dictée") :
     N'écris JAMAIS cette consigne dans le document ! Émets l'action "deactivate_dictation" avec "narration": "Dictée désactivée.".
2. Contrôle et déplacement du curseur ("cursor_move") :
   - Si l'utilisateur donne un ordre de mise en page ou de déplacement de curseur ("à la ligne", "nouveau paragraphe", "curseur à gauche", "efface le mot", "début de ligne"), utilise l'action "cursor_move" avec le libellé de commande adéquat. Maintiens "narration": "".
3. Retranscription de texte dicté ("dictate") :
   - Pour tout autre propos dicté : retranscris fidèlement le texte parlé avec ponctuation naturelle et soignée (points, virgules, majuscules).
   - Utilise l'action "dictate" avec "target": null, "window": null, "press_enter": false (ou true si "entrée" explicite).
   - Maintiens STRICTEMENT "narration": "" (silence complet).
4. Silence / Bruit :
   - Si inintelligible ou bruit de micro : "invalid_request": true, "narration": "", "actions": [].

Format JSON obligatoire :
{
  "narration": "",
  "actions": [
    {"action": "dictate", "text": "texte à insérer", "target": null, "window": null, "press_enter": false}
  ],
  "quick_suggestions": []
}"#;

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
enum PromptTrigger {
    User,
    Surveillance,
    Immersion { screen_changed: bool },
}

fn url_encode(input: &str) -> String {
    let mut out = String::with_capacity(input.len());
    for b in input.bytes() {
        match b {
            b'a'..=b'z' | b'A'..=b'Z' | b'0'..=b'9' | b'-' | b'_' | b'.' | b'~' => {
                out.push(b as char);
            }
            b' ' => out.push('+'),
            _ => {
                out.push_str(&format!("%{:02X}", b));
            }
        }
    }
    out
}

async fn call_llm_prompt(
    llm_config: LlmConfig,
    user_instruction: String,
    full_prompt: String,
    history: Arc<tokio::sync::Mutex<Vec<ChatMessage>>>,
    event_tx: StdSender<AgentEvent>,
    tts_tx: StdSender<TtsCommand>,
    last_call_time: Arc<tokio::sync::Mutex<Option<Instant>>>,
    trigger: PromptTrigger,
    request_id: u64,
) {
    if is_request_cancelled(request_id) {
        return;
    }

    // Si la consigne est vide ou composée uniquement de bruits/symboles parasites, ne pas solliciter le LLM
    if user_instruction.trim().is_empty() || !user_instruction.chars().any(|c| c.is_alphanumeric()) {
        println!("[LLM] Instruction vide ou bruit parasite ignoré, aucune requête LLM envoyée.");
        let _ = event_tx.send(AgentEvent::StatusChanged(AgentStatus::Idle));
        return;
    }

    AGENT_BUSY.store(true, Ordering::SeqCst);
    let _busy_guard = BusyGuard;
    let _ = event_tx.send(AgentEvent::StatusChanged(AgentStatus::Thinking));

    let is_local_target = llm_config.provider.is_local_or_custom_endpoint(&llm_config.endpoint);
    if llm_config.api_key.is_empty() && !is_local_target {
        let _ = event_tx.send(AgentEvent::ReplaceNarration {
            text: format!("Veuillez renseigner {} dans le fichier .env.", llm_config.provider.env_key_name()),
            quick_suggestions: Vec::new(),
        });
        let _ = event_tx.send(AgentEvent::RequestApiKey);
        let _ = event_tx.send(AgentEvent::StatusChanged(AgentStatus::Idle));
        return;
    }

    let client = reqwest::Client::new();
    let (_, system_lang) = get_system_locale_language();
    let is_dictation = IS_DICTATION_ACTIVE.load(Ordering::SeqCst);
    let is_local_llm = llm_config.provider == LlmProvider::Local;
    let preset = local_llm::get_active_preset();
    let tier = local_llm::resolve_model_tier(llm_config.provider, Some(preset));
    let local_opts = local_llm::get_local_llm_options();
    let system_instructions = if is_dictation {
        DICTATION_SYSTEM_INSTRUCTIONS.to_string()
    } else {
        if is_local_llm {
            local_llm::build_unified_local_system_prompt(&system_lang)
        } else {
            build_standard_system_instructions(&system_lang)
        }
    };

    let safe_user_instruction = truncate_with_notice(&user_instruction, 1000);
    let safe_full_prompt = truncate_with_notice(&full_prompt, 32000);
    {
        let mut hist_guard = history.lock().await;
        hist_guard.push(ChatMessage {
            role: "user".to_string(),
            content: safe_full_prompt.clone(),
        });
    }

    let mut max_agent_passes = match tier {
        local_llm::ModelTier::Local => {
            if local_opts.minimal_mode {
                TARGET_REASONING_PASSES.load(Ordering::SeqCst).clamp(1, 6)
            } else {
                TARGET_REASONING_PASSES.load(Ordering::SeqCst).clamp(1, 8)
            }
        }
        local_llm::ModelTier::Remote => TARGET_REASONING_PASSES.load(Ordering::SeqCst).clamp(1, 15),
    };
    let mut has_spoken = false;
    let mut pass = 1;
    let mut consecutive_repeat_count = 0usize;
    let mut last_executed_actions: Vec<String> = Vec::new();

    while pass <= max_agent_passes {
        if is_request_cancelled(request_id) {
            return;
        }
        let _ = event_tx.send(AgentEvent::StatusChanged(AgentStatus::Thinking));

        // Compaction progressive de la mémoire si l'historique dépasse le seuil
        {
            let mut hist_guard = history.lock().await;
            let mut time_guard = last_call_time.lock().await;
            compact_history_if_needed(&llm_config, &mut hist_guard, &client, &mut time_guard).await;
        }

        if is_request_cancelled(request_id) {
            return;
        }

        // Debounce : pause minimale entre deux requêtes API
        let min_debounce = if is_dictation {
            Duration::from_millis(400)
        } else if pass == 1 {
            Duration::from_millis(3000)
        } else {
            Duration::from_millis(2000)
        };
        let prev_call = { *last_call_time.lock().await };
        if let Some(prev) = prev_call {
            let elapsed = prev.elapsed();
            if elapsed < min_debounce {
                tokio::time::sleep(min_debounce - elapsed).await;
            }
        }

        if is_request_cancelled(request_id) {
            return;
        }

        let hist_snapshot = if is_local_llm {
            if !local_opts.include_history {
                vec![ChatMessage {
                    role: "user".to_string(),
                    content: safe_full_prompt.clone(),
                }]
            } else {
                let guard = history.lock().await;
                get_recent_history_capped_at_user_messages(&guard, 4)
            }
        } else if !match tier {
            local_llm::ModelTier::Remote => true,
            _ => local_opts.include_history,
        } {
            vec![ChatMessage {
                role: "user".to_string(),
                content: safe_full_prompt.clone(),
            }]
        } else {
            history.lock().await.clone()
        };

        let (temperature, max_tokens) = if is_dictation {
            (0.3, 300)
        } else if llm_config.provider == LlmProvider::Local {
            (0.2, 1000)
        } else {
            (0.7, 1800)
        };

        let mut attempts = 0;
        const MAX_RETRIES: usize = 3;
        const DEFAULT_RETRY_DELAY: Duration = Duration::from_millis(4000);
        let mut success_payload: Option<AgentResponsePayload> = None;

        loop {
            if is_request_cancelled(request_id) {
                return;
            }

            if llm_config.provider == LlmProvider::Local {
                match local_llm::run_local_llama_completion(
                    &system_instructions,
                    &hist_snapshot,
                    max_tokens,
                    &event_tx,
                    Some(&tts_tx),
                ) {
                    Ok(raw_content) => {
                        history.lock().await.push(ChatMessage {
                            role: "assistant".to_string(),
                            content: raw_content.clone(),
                        });
                        success_payload = Some(parse_agent_response(&raw_content));
                        break;
                    }
                    Err(err) => {
                        let _ = event_tx.send(AgentEvent::ReplaceNarration {
                            text: format!("Erreur modèle local Qwen: {err}"),
                            quick_suggestions: Vec::new(),
                        });
                        break;
                    }
                }
            }

            attempts += 1;

            let response = send_llm_chat_completion(
                &client,
                &llm_config,
                &system_instructions,
                &hist_snapshot,
                temperature,
                max_tokens,
            ).await;

            match response {
                Ok(res) if res.status().is_success() => {
                    {
                        let mut time_guard = last_call_time.lock().await;
                        *time_guard = Some(Instant::now());
                    }
                    if let Ok(raw_content) = parse_llm_response_text(res, llm_config.provider).await {
                        println!("\n=================== [Réponse {} brute (passe {}/{})] ===================", llm_config.provider.display_name(), pass, max_agent_passes);
                        println!("{}", raw_content.trim());
                        println!("============================================================");

                        history.lock().await.push(ChatMessage {
                            role: "assistant".to_string(),
                            content: raw_content.clone(),
                        });

                        let payload = parse_agent_response(&raw_content);
                        success_payload = Some(payload);
                        break;
                    }
                    history.lock().await.pop();
                    if !is_request_cancelled(request_id) {
                        let _ = event_tx.send(AgentEvent::ReplaceNarration {
                            text: "Format de réponse inattendu.".into(),
                            quick_suggestions: Vec::new(),
                        });
                    }
                    break;
                }
                Ok(res) if res.status() == reqwest::StatusCode::TOO_MANY_REQUESTS && attempts <= MAX_RETRIES => {
                    let wait_secs = res.headers()
                        .get("retry-after")
                        .and_then(|h| h.to_str().ok())
                        .and_then(|s| s.parse::<u64>().ok())
                        .unwrap_or(4);

                    let safe_wait_secs = (wait_secs + 1).max(4);

                    // Si le délai dépasse 12 secondes, il s'agit d'une saturation de jetons : ne pas bloquer l'agent
                    if wait_secs > 12 {
                        let err_body = res.text().await.unwrap_or_default();
                        println!("[LLM 429] Détails renvoyés par l'API : {}", err_body);
                        history.lock().await.pop();
                        let mins = (wait_secs + 59) / 60;
                        if !is_request_cancelled(request_id) {
                            let _ = event_tx.send(AgentEvent::ReplaceNarration {
                                text: format!("Plafond instantané DeepSeek atteint (pause requise par l'API : {mins} min). Historique réinitialisé."),
                                quick_suggestions: Vec::new(),
                            });
                        }
                        history.lock().await.clear();
                        TARGET_REASONING_PASSES.store(5, Ordering::SeqCst);
                        break;
                    }

                    let _ = event_tx.send(AgentEvent::SilentNarration(format!(
                        "Limite de requêtes atteinte. Pause de {safe_wait_secs} s avant réessai..."
                    )));
                    tokio::time::sleep(Duration::from_secs(safe_wait_secs)).await;
                    {
                        let mut time_guard = last_call_time.lock().await;
                        *time_guard = Some(Instant::now());
                    }
                    continue;
                }
                Ok(res) => {
                    {
                        let mut time_guard = last_call_time.lock().await;
                        *time_guard = Some(Instant::now());
                    }
                    history.lock().await.pop();
                    let status = res.status();
                    if !is_request_cancelled(request_id) {
                        let _ = event_tx.send(AgentEvent::ReplaceNarration {
                            text: format!("Erreur API {} : {status}", llm_config.provider.display_name()),
                            quick_suggestions: Vec::new(),
                        });
                    }
                    break;
                }
                Err(_err) if attempts <= MAX_RETRIES => {
                    let _ = event_tx.send(AgentEvent::SilentNarration(
                        "Connexion interrompue, nouvelle tentative dans 3 secondes...".into(),
                    ));
                    tokio::time::sleep(DEFAULT_RETRY_DELAY).await;
                    {
                        let mut time_guard = last_call_time.lock().await;
                        *time_guard = Some(Instant::now());
                    }
                    continue;
                }
                Err(err) => {
                    {
                        let mut time_guard = last_call_time.lock().await;
                        *time_guard = Some(Instant::now());
                    }
                    history.lock().await.pop();
                    if !is_request_cancelled(request_id) {
                        let _ = event_tx.send(AgentEvent::ReplaceNarration {
                            text: format!("Erreur réseau : {err}"),
                            quick_suggestions: Vec::new(),
                        });
                    }
                    break;
                }
            }
        }

        let Some(mut payload) = success_payload else {
            break;
        };

        if is_request_cancelled(request_id) {
            return;
        }

        // Prise en compte immédiate du champ racine next_passes ou de l'action set_reasoning_passes (neutralisé en cas de répétition)
        let requested_passes = if consecutive_repeat_count == 0 {
            payload.next_passes.or_else(|| {
                payload.actions.iter().find_map(|act| match act {
                    AgentAction::SetReasoningPasses { passes, .. } => Some(*passes),
                    _ => None,
                })
            })
        } else {
            None
        };

        let mut stop_requested = false;
        if let Some(np) = requested_passes {
            let max_limit = if is_local_llm { 8 } else { 15 };
            if np == 0 || (payload.actions.is_empty() && pass >= 1) {
                max_agent_passes = pass;
                TARGET_REASONING_PASSES.store(max_agent_passes, Ordering::SeqCst);
                stop_requested = true;
                println!("[Agent] Arrêt demandé (next_passes: {np}) : fin après la passe courante ({pass}).");
            } else if np <= pass && pass > 1 {
                max_agent_passes = pass;
                TARGET_REASONING_PASSES.store(max_agent_passes, Ordering::SeqCst);
                stop_requested = true;
                println!("[Agent] Quota de passes atteint (next_passes: {np} <= passe {pass}) : arrêt à cette passe.");
            } else {
                max_agent_passes = np.clamp(pass, max_limit);
                TARGET_REASONING_PASSES.store(max_agent_passes, Ordering::SeqCst);
                println!("[Agent] Quota de raisonnement mis à jour via next_passes (+{np}) : {max_agent_passes} passe(s) max");
            }
        }

        if payload.invalid_request
            || payload.narration.trim().eq_ignore_ascii_case("invalid_request")
            || (payload.narration.trim().is_empty() && payload.actions.is_empty())
        {
            println!("[Agent] Requête incomplète, bruit parasite ou incomprise (narration vide sans action) : mode silencieux activé (aucun affichage, TTS ni requête LLM supplémentaire).");
            history.lock().await.pop();
            let _ = event_tx.send(AgentEvent::StatusChanged(AgentStatus::Idle));
            let _ = event_tx.send(AgentEvent::RequestIgnored);
            return;
        }

        // 2. Dispatch immédiat dès réception du message pour les fenêtres volantes
        let shown_card_ids: Vec<String> = payload.actions.iter().filter_map(|act| match act {
            AgentAction::ShowFloatingCard { id: Some(cid), .. } => Some(cid.clone()),
            _ => None,
        }).collect();

        let mut card_speeches: Vec<String> = Vec::new();
        for act in &payload.actions {
            match act {
                AgentAction::ShowFloatingCard { id, title, card_type, data } => {
                    let card = FloatingCard::from_action_data(id.clone(), title.clone(), card_type, data);
                    let speech = card.build_narrator_speech();
                    if !speech.trim().is_empty() {
                        card_speeches.push(speech);
                    }
                    if matches!(card.content, FloatingContentType::Subtitles { .. }) {
                        let target_win = match &card.content {
                            FloatingContentType::Subtitles { target_window, .. } => target_window.clone(),
                            _ => None,
                        };
                        crate::audio::start_live_subtitles(target_win);
                    }
                    let _ = event_tx.send(AgentEvent::ShowFloatingCard(card));
                }
                AgentAction::CloseFloatingCard { id } => {
                    if shown_card_ids.contains(id) {
                        println!("[Agent] Ignoré close_floating_card(\"{id}\") car la carte a été mise à jour dans la même passe.");
                    } else {
                        let _ = event_tx.send(AgentEvent::CloseFloatingCard(id.clone()));
                    }
                }
                AgentAction::LiveSubtitles { target_window, enabled } => {
                    if *enabled {
                        crate::audio::start_live_subtitles(target_window.clone());
                        let default_title = t!("commands.subtitles_title").to_string();
                        let card = FloatingCard::new(
                            "live_subtitles",
                            target_window.as_deref().unwrap_or(&default_title),
                            FloatingContentType::Subtitles {
                                text: String::new(),
                                target_window: target_window.clone(),
                                history: Vec::new(),
                            },
                        );
                        let _ = event_tx.send(AgentEvent::ShowFloatingCard(card));
                    } else {
                        crate::audio::stop_live_subtitles();
                        let _ = event_tx.send(AgentEvent::LiveSubtitlesClosed);
                    }
                }
                AgentAction::ToggleNumberedTargets { enabled } => {
                    let _ = event_tx.send(AgentEvent::ToggleNumberedTargets(*enabled));
                }
                AgentAction::ClickNumberedTarget { index } => {
                    let _ = event_tx.send(AgentEvent::ClickNumberedTarget(*index));
                }
                AgentAction::NextNumberedChunk => {
                    let _ = event_tx.send(AgentEvent::NextNumberedChunk);
                }
                AgentAction::PreviousNumberedChunk => {
                    let _ = event_tx.send(AgentEvent::PreviousNumberedChunk);
                }
                _ => {}
            }
        }

        if !card_speeches.is_empty() {
            let combined = card_speeches.join("\n\n");
            let trimmed_narr = payload.narration.trim();
            if trimmed_narr.is_empty() {
                payload.narration = combined;
            } else if !trimmed_narr.contains(&combined) {
                let sep = if trimmed_narr.ends_with('.') || trimmed_narr.ends_with(':') || trimmed_narr.ends_with('!') { " " } else { ". " };
                payload.narration = format!("{}{}{}", trimmed_narr, sep, combined);
            }
        }

        for act in &payload.actions {
            if let AgentAction::ShowFloatingCard { title, .. } = act {
                let read_btn_text = t!("ui.read_card_prefix", title = title).to_string();
                if !payload.quick_suggestions.iter().any(|s| s.as_str() == Some(&read_btn_text)) {
                    payload.quick_suggestions.insert(0, QuickSuggestionItem::Text(read_btn_text));
                }
            }
        }

        // En mode immersion, si l'écran n'a pas bougé et qu'aucune action ni suggestion rapide n'est formulée, préserver le silence
        if let PromptTrigger::Immersion { screen_changed } = trigger {
            let no_actions_or_suggestions = payload.actions.is_empty() && payload.quick_suggestions.is_empty();
            if (!screen_changed && no_actions_or_suggestions) || (payload.narration.trim().is_empty() && no_actions_or_suggestions) {
                println!("[Immersion] Écran inchangé ou silence demandé sans action/suggestion : préservation du silence.");
                let mut hist_guard = history.lock().await;
                hist_guard.pop();
                hist_guard.pop();
                let _ = event_tx.send(AgentEvent::StatusChanged(AgentStatus::Idle));
                return;
            }
        }

        if trigger == PromptTrigger::Surveillance
            && payload.actions.is_empty() && payload.quick_suggestions.is_empty() && payload.narration.trim().is_empty()
        {
            println!("[Surveillance] L'IA a analysé l'écran : aucune action nécessaire.");
            let mut hist_guard = history.lock().await;
            hist_guard.pop();
            hist_guard.pop();
            let _ = event_tx.send(AgentEvent::StatusChanged(AgentStatus::Idle));
            return;
        }

        println!("[Agent] Narration (passe {}) : \"{}\"", pass, payload.narration);
        println!("[Agent] {} action(s) planifiée(s) :", payload.actions.len());
        for (i, act) in payload.actions.iter().enumerate() {
            println!("  [{}] {:?}", i + 1, act);
        }

        if payload.actions.is_empty() {
            let lower_user = safe_user_instruction.to_lowercase();
            let is_action_request = lower_user.starts_with("mets")
                || lower_user.starts_with("lance")
                || lower_user.starts_with("ouvre")
                || lower_user.starts_with("cherche")
                || lower_user.starts_with("trouve")
                || lower_user.starts_with("joue")
                || lower_user.starts_with("écoute")
                || lower_user.contains("radio")
                || lower_user.contains("musique");

            let lower_narration = payload.narration.to_lowercase();
            let is_passive_excuse = lower_narration.contains("il faudrait")
                || lower_narration.contains("faudrait ouvrir")
                || lower_narration.contains("aucune radio")
                || lower_narration.contains("aucun média")
                || lower_narration.contains("aucun resultat");

            // Si l'utilisateur voulait une action concrète et que l'IA a abandonné ou s'est défaussée :
            if (is_action_request || is_passive_excuse) && pass == 1 {
                println!("[Agent Résilience] L'IA s'est défaussée sans action pour \"{}\". Forçage d'une action de recherche / média...", safe_user_instruction);
                let target_query = if lower_user.contains("radio") {
                    "https://www.radiofrance.fr".to_string()
                } else {
                    format!("https://www.google.com/search?q={}", url_encode(&safe_user_instruction))
                };
                payload.actions.push(AgentAction::NavigateToUrl {
                    url: target_query,
                    target: Some("new_tab".to_string()),
                    new_window: Default::default(),
                    window: Default::default(),
                });
                payload.narration = format!("J'ouvre le navigateur pour lancer {} immédiatement.", safe_user_instruction);
            }
        }

        if payload.actions.is_empty() {
            println!("[Agent] Tâche accomplie : aucune action supplémentaire. Fin de la séquence après {} passe(s).", pass);
            if payload.quick_suggestions.is_empty() {
                let screen = {
                    LAST_SCREEN_SUMMARY.lock().ok().and_then(|s| s.clone()).unwrap_or_else(|| {
                        #[cfg(windows)]
                        { summarize_screen_state(None, true) }
                        #[cfg(not(windows))]
                        { summarize_screen_state(None, false) }
                    })
                };
                payload.quick_suggestions = generate_fallback_suggestions_from_screen(&screen);
            }
            if !payload.narration.trim().is_empty() || !payload.quick_suggestions.is_empty() {
                if !sanitize_for_tts(&payload.narration).trim().is_empty() {
                    has_spoken = true;
                }
                let _ = event_tx.send(AgentEvent::ReplaceNarration {
                    text: payload.narration.clone(),
                    quick_suggestions: payload.quick_suggestions.clone(),
                });
            }
            break;
        }

        if is_request_cancelled(request_id) {
            return;
        }

        if !payload.narration.trim().is_empty() || !payload.quick_suggestions.is_empty() {
            if !sanitize_for_tts(&payload.narration).trim().is_empty() {
                has_spoken = true;
            }
            let _ = event_tx.send(AgentEvent::ReplaceNarration {
                text: payload.narration.clone(),
                quick_suggestions: payload.quick_suggestions.clone(),
            });
        }

        let actions_to_run = payload.actions;

        // Détection de boucle répétitive (actions identiques répétées)
        let current_action_signatures: Vec<String> = actions_to_run
            .iter()
            .filter(|a| !matches!(a, AgentAction::SetReasoningPasses { .. }))
            .map(|a| format!("{:?}", a))
            .collect();

        if !current_action_signatures.is_empty() && current_action_signatures == last_executed_actions {
            consecutive_repeat_count += 1;
            println!("[Agent] Répétition d'action détectée (occurrence {}) : {:?}", consecutive_repeat_count, current_action_signatures);
            if consecutive_repeat_count >= 2 {
                println!("[Agent] Boucle répétitive interrompue (action identique répétée {} fois consécutives).", consecutive_repeat_count + 1);
                if !payload.narration.trim().is_empty() || !payload.quick_suggestions.is_empty() {
                    let _ = event_tx.send(AgentEvent::ReplaceNarration {
                        text: payload.narration.clone(),
                        quick_suggestions: payload.quick_suggestions.clone(),
                    });
                }
                break;
            }
        } else {
            consecutive_repeat_count = 0;
        }
        last_executed_actions = current_action_signatures;

        let has_concrete_actions = actions_to_run.iter().any(|a| {
            !matches!(a, AgentAction::SetReasoningPasses { .. })
        });

        let has_web_action = actions_to_run.iter().any(|a| {
            matches!(
                a,
                AgentAction::OpenBrowser { .. }
                    | AgentAction::NavigateToUrl { .. }
                    | AgentAction::ClickElement { .. }
                    | AgentAction::ClickButton { .. }
                    | AgentAction::Scroll { .. }
                    | AgentAction::SummarizeScreen { .. }
                    | AgentAction::FocusWindow { .. }
            )
        });

        let (report, screen_after) = tokio::task::spawn_blocking(move || {
            if is_request_cancelled(request_id) {
                return ("Actions interrompues.".to_string(), String::new());
            }
            let outcome = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
                let rep = execute_system_actions(&actions_to_run, request_id);
                if is_request_cancelled(request_id) {
                    return (rep, String::new());
                }
                if is_dictation {
                    return (rep, String::new());
                }
                let wait_ms = if has_web_action { 2200 } else { 1200 };
                std::thread::sleep(Duration::from_millis(wait_ms));
                if is_request_cancelled(request_id) {
                    return (rep, String::new());
                }
                #[cfg(windows)]
                let sc = if is_local_llm && local_opts.active_window_only {
                    let fg = unsafe { windows::Win32::UI::WindowsAndMessaging::GetForegroundWindow() };
                    let current_pid = std::process::id();
                    let (fg_pid, _) = get_window_process_info(fg);
                    let target_hwnd = if !fg.0.is_null() && fg_pid != current_pid && !is_window_hung(fg) {
                        Some(fg)
                    } else {
                        LAST_EXTERNAL_HWND.lock().ok().and_then(|g| *g)
                            .map(|raw| windows::Win32::Foundation::HWND(raw as _))
                            .filter(|&h| !h.0.is_null() && !is_window_hung(h) && get_window_process_info(h).0 != current_pid)
                    };
                    if let Some(hwnd) = target_hwnd {
                        let title = get_window_title(hwnd);
                        let targets = collect_numbered_targets_for_window(hwnd);
                        format_compact_window_elements(&title, &targets)
                    } else {
                        "[Aucune fenêtre externe au premier plan]".to_string()
                    }
                } else if tier != local_llm::ModelTier::Remote && local_opts.active_window_only {
                    let fg = unsafe { windows::Win32::UI::WindowsAndMessaging::GetForegroundWindow() };
                    let current_pid = std::process::id();
                    let (fg_pid, _) = get_window_process_info(fg);
                    let target = if !fg.0.is_null() && fg_pid != current_pid && !is_window_hung(fg) {
                        Some(fg)
                    } else {
                        LAST_EXTERNAL_HWND.lock().ok().and_then(|g| *g).map(|raw| windows::Win32::Foundation::HWND(raw as _)).filter(|&h| !h.0.is_null() && !is_window_hung(h))
                    };
                    if let Some(h) = target {
                        if get_window_process_info(h).0 != current_pid {
                            extract_window_text_only(h)
                        } else {
                            summarize_screen_state(None, true)
                        }
                    } else {
                        summarize_screen_state(None, true)
                    }
                } else {
                    summarize_screen_state(None, true)
                };
                #[cfg(not(windows))]
                let sc = if is_local_llm && local_opts.active_window_only {
                    let targets = collect_numbered_targets_for_active_window();
                    format_compact_window_elements("Fenêtre active", &targets)
                } else {
                    summarize_screen_state(None, false)
                };
                (rep, sc)
            }));
            match outcome {
                Ok(res) => res,
                Err(err) => {
                    eprintln!("[Résilience] Panique interceptée lors de l'exécution des actions : {:?}", err);
                    ("Une action système a rencontré une erreur non critique et a été interrompue en toute sécurité.".to_string(), String::new())
                }
            }
        }).await.unwrap_or_else(|_| ("Erreur d'exécution du worker.".to_string(), String::new()));

        if is_request_cancelled(request_id) {
            return;
        }

        // Stratégie de Rollback / Undo automatique si l'action a échoué pendant une passe d'exploration
        let action_failed = report.contains("introuvable")
            || report.contains("non réactive")
            || report.contains("Ciblage rejeté");
        if action_failed && pass > 1 {
            println!("[Résilience Agent] Échec d'action détecté dans la passe {pass}. Déclenchement de l'undo automatique.");
            let _ = undo_last_system_action();
        }

        let max_clamp = if is_local_llm { 8 } else { 15 };
        let synced_passes = TARGET_REASONING_PASSES.load(Ordering::SeqCst).clamp(1, max_clamp);
        if synced_passes > max_agent_passes && !stop_requested {
            max_agent_passes = synced_passes;
            println!("[Agent] Quota de raisonnement resynchronisé avec TARGET_REASONING_PASSES : {max_agent_passes} passe(s) max");
        }

        if is_dictation {
            println!("[Dictée] Action exécutée instantanément, fin de séquence.");
            break;
        }

        if !screen_after.trim().is_empty() {
            if let Ok(mut lock) = LAST_SCREEN_SUMMARY.lock() {
                *lock = Some(screen_after.clone());
            }
        }

        // Passe automatique de réévaluation post-action :
        // Si des actions concrètes viennent d'être exécutées et que la dernière passe autorisée venait d'être atteinte,
        // une passe supplémentaire de réévaluation est automatiquement allouée afin que l'agent puisse observer
        // le nouvel état de l'écran plutôt que de clore la tâche à l'aveugle.
        if !stop_requested && pass >= max_agent_passes && has_concrete_actions && max_agent_passes < max_clamp && consecutive_repeat_count == 0 {
            max_agent_passes += 1;
            TARGET_REASONING_PASSES.store(max_agent_passes, Ordering::SeqCst);
            println!("[Agent] Passe de réévaluation post-action allouée automatiquement (passe {pass}/{max_agent_passes}).");
        }

        if pass < max_agent_passes {
            let mut step_feedback = String::new();
            let include_feedback = match tier {
                local_llm::ModelTier::Remote => true,
                _ => local_opts.include_feedback,
            };
            if include_feedback {
                if !screen_after.trim().is_empty() {
                    let screen_after_clean = if screen_after.contains("<html") || screen_after.contains("<table") || screen_after.contains("<body") {
                        html_util::sanitize_html_to_markdown(&screen_after)
                    } else {
                        screen_after.clone()
                    };
                    let max_screen = if screen_after_clean.contains("[Page web") || screen_after_clean.contains("[Navigateur]") {
                        15360
                    } else {
                        12000
                    };
                    let safe_screen = truncate_with_notice(screen_after_clean.trim(), max_screen);
                    step_feedback.push_str(&format!("[État de l'écran suite aux actions] :\n{safe_screen}\n\n"));
                }
                let compact_report = truncate_with_notice(&report, 1500);
                step_feedback.push_str(&format!("[Retour d'exécution étape {pass}] :\n{compact_report}"));
            }

            // Injection de l'état des fenêtres volantes ouvertes dans les étapes multi-passes
            let active_win_only = match tier {
                local_llm::ModelTier::Remote => false,
                _ => local_opts.active_window_only,
            };
            if !active_win_only {
                if let Ok(guard) = OPEN_FLOATING_CARDS.lock() {
                    if !guard.is_empty() {
                    step_feedback.push_str("\n\n[Fenêtres volantes ouvertes à l'écran] :\n");
                    for c in guard.iter() {
                        step_feedback.push_str(&format!("- ID: \"{}\" | Titre: \"{}\" | Type: \"{}\"\n", c.id, c.title, c.card_type));
                        if !c.content_preview.is_empty() {
                            let preview = truncate_with_notice(&c.content_preview, 1500);
                            step_feedback.push_str(&format!("  [Contenu actuel déjà affiché à conserver/fusionner] :\n\"\"\"\n{}\n\"\"\"\n", preview.trim()));
                        }
                    }
                    step_feedback.push_str("RÈGLE DE FUSION IMPÉRATIVE : Conserve et fusionne les éléments déjà présents ci-dessus avec les nouvelles informations dans \"show_floating_card\". Ne ferme JAMAIS la carte (\"close_floating_card\") !");
                    }
                }
            }

            if is_local_llm {
                step_feedback.push_str(&format!(
                    "\n\n[Étape {next_pass}] Vérification de l'objectif : \"{}\".\n\
                    - RÈGLE LINGUISTIQUE STRICTE : Traduis et formule TOUT impérativement en {system_lang} dans \"narration\" et les suggestions.\n\
                    - TANT QUE L'OBJECTIF N'EST PAS SATISFAIT : continue les passes, choisis l'action suivante en JSON.\n\
                    - DÉTECTION DE BOUCLE : si l'action précédente a échoué ou que l'écran stagne, pivote (autre lien/bouton, scroll, navigate_back).\n\
                    - Dès que l'objectif initial est ATTEINT (application ouverte, information trouvée, action exécutée), ARRÊTE IMMÉDIATEMENT : renvoie \"actions\": [], \"next_passes\": 0 et décris en détail la situation et le résultat dans \"narration\" rédigée en {system_lang}.\n\
                    - RECHERCHE WEB & FENÊTRE ACTIVE : La recherche web et la navigation sont ton outil principal. Ouvre directement via \"navigate_to_url\". Ne ferme JAMAIS la fenêtre active et ne clique pas sur la croix pour chercher.\n\
                    - UN SEUL CLIC PAR PASSE : 1 seule action concrète à la fois. Pas de clics sur les filtres/onglets génériques du moteur (\"Actualités\", \"Vidéos\", \"Images\", \"Outils\"). Clique directement sur le résultat du sujet visé avec \"click_element\".\n\
                    - Cookies / Bannières de notification web -> clique pour accepter les cookies ou clique sur la croix ✕ pour fermer la notification.\n\
                    Réponds STRICTEMENT en JSON.",
                    safe_user_instruction,
                    next_pass = pass + 1
                ));
            } else if tier != local_llm::ModelTier::Remote && local_opts.minimal_mode {
                step_feedback.push_str(&format!(
                    "\n\n[Étape {next_pass}] Objectif : \"{}\". Continue les passes tant que non satisfait. Si accompli, conclus (\"actions\": [], \"next_passes\": 0). Traduis et rédige impérativement \"narration\" en {system_lang}.",
                    safe_user_instruction,
                    next_pass = pass + 1
                ));
            } else {
            step_feedback.push_str(&format!(
                "\n\n[Étape {next_pass}] Vérification d'avancement pour l'objectif : \"{}\".\n\
                - RÈGLE LINGUISTIQUE STRICTE : Traduis et rédige TOUJOURS la \"narration\" et tes suggestions exclusivement en {system_lang}.\n\
                - TANT QUE L'OBJECTIF N'EST PAS ENCORE SATISFAIT : CONTINUE IMPÉRATIVEMENT les passes. Sélectionne l'action suivante et ajuste \"next_passes\": N (max de passes restantes).\n\
                - EN CAS DE BOUCLE OU DE PIÉTINEMENT : si l'action précédente n'a rien changé ou tourne en rond, PIVOTE immédiatement (autre résultat, scroll, navigate_back, autre fenêtre) au lieu de répéter la même action.\n\
                - RÈGLE D'ARRÊT PRIORITAIRE : Si l'objectif initial est ATTEINT (page/application ouverte, information visible, média lancé, saisie effectuée), ARRÊTE IMMÉDIATEMENT les passes : renvoie obligatoirement \"actions\": [], \"next_passes\": 0 et un compte-rendu descriptif et détaillé dans \"narration\". Ne poursuis aucune action supplémentaire non demandée.\n\
                - RÈGLE DE FORMAT : Réponds STRICTEMENT en JSON valide (champs \"narration\", \"next_passes\", \"actions\", \"quick_suggestions\"). Aucun texte hors JSON.\n\
                - Audio, son & paramètres OS -> INTERDICTION de faire 'run_command' (cmd/explorer). Utilise 'set_volume', 'mute_audio' ou 'accessibility_shortcut' ('action_center' Win+A, 'open_settings' Win+I, 'm', 'space').\n\
                - Bannière cookies -> clique automatiquement 'Tout accepter' / 'Accepter'.\n\
                - Bannière de notification web / push ('Activez les notifications franceinfo...', etc.) -> traite-la immédiatement en cliquant sur la croix 'Fermer (croix ✕)' / 'Fermer' / 'Refuser' / 'Plus tard' ou 'Activer' pour libérer la vue.\n\
                - Pour lancer ou contrôler une vidéo ou un audio -> clique directement sur l'élément vidéo/audio ou son bouton de lecture ('play' / 'lecture').\n\
                - Sous-titres en direct & transcription du son -> émets 'live_subtitles' (enabled: true) ou 'show_floating_card' (card_type: 'subtitles').\n\
                - Notification système (toast) -> lis son contenu dans ta narration et laisse-la tranquille (ne la ferme pas).\n\
                - Alerte / modale d'erreur -> lis le message dans ta narration, puis clique impérativement sur 'OK' / 'Fermer' pour la refermer.\n\
                - Fenêtre volante Markdown -> insère des liens cliquables [link=\"action:...\"]Texte[/link] pour permettre à l'utilisateur de déclencher des suites ou choix en un clic.\n\
                - Liens interactifs -> Formate SYSTÉMATIQUEMENT vidéos, audios, images, PDF/fichiers et actions en liens cliquables via la macro [link=\"action:...\"]Icône Libellé[/link] ou [link=\"https://...\"]🌐 Web[/link] (y compris dans les cellules de tableaux Markdown). Le clic envoie directement la commande dans le chat de Libertide.\n\
                - Fenêtre volante (\"show_floating_card\") -> CONSERVE et FUSIONNE impérativement les données précédentes avec les nouvelles (ne supprime pas les éléments déjà trouvés lors des passes précédentes). INTERDICTION d'émettre \"close_floating_card\" pour une carte que tu viens d'ouvrir ou mettre à jour.\n\
                 - Mission approfondie (contact / produit / service) -> définis 'next_passes' (8 à 12), explore méthodiquement (local -> web -> annuaires) et projette les résultats dans 'show_floating_card'. Ne renvoie pas 'actions': [] tant que les résultats ne sont pas trouvés.\n\
                - Échec/blocage -> pivote immédiatement vers une alternative (app, web, autre fenêtre). Ne renvoie JAMAIS 'actions': [] si l'objectif n'est pas achevé.\n\
                - Tâche terminée -> 'actions': [], 'next_passes': 0 + 2 à 4 'quick_suggestions'.",
                safe_user_instruction,
                next_pass = pass + 1
            ));
            }

            history.lock().await.push(ChatMessage {
                role: "user".to_string(),
                content: step_feedback,
            });
            // Marquer la fin de l'exécution pour que le debounce de la passe suivante s'applique bien
            {
                let mut time_guard = last_call_time.lock().await;
                *time_guard = Some(Instant::now());
            }
            pass += 1;
        } else {
            println!("[Agent] Nombre maximal de passes ({max_agent_passes}) atteint.");
            break;
        }
    }

    // Libération explicite du verrou d'activité avant d'émettre l'état Idle
    drop(_busy_guard);

    if !is_request_cancelled(request_id) && !has_spoken {
        let _ = event_tx.send(AgentEvent::StatusChanged(AgentStatus::Idle));
    }
}

fn fix_startup_working_directory() {
    if let Ok(cwd) = std::env::current_dir() {
        let cwd_str = cwd.to_string_lossy().to_lowercase();
        if cwd_str.contains("system32") || cwd_str.contains("syswow64") {
            if let Ok(exe) = std::env::current_exe() {
                if let Some(parent) = exe.parent() {
                    let project_dir = parent.parent().and_then(|p| p.parent()).filter(|p| p.join("Cargo.toml").exists());
                    let target_dir = project_dir.unwrap_or(parent);
                    let _ = std::env::set_current_dir(target_dir);
                }
            }
        }
    }
}

fn spawn_periodic_screen_monitor(cmd_tx: UnboundedSender<AgentCommand>) {
    std::thread::spawn(move || {
        loop {
            std::thread::sleep(Duration::from_secs(60));
            let _ = cmd_tx.send(AgentCommand::PeriodicScreenCheck);
        }
    });
}

fn spawn_agent_backend(
    mut cmd_rx: UnboundedReceiver<AgentCommand>,
    cmd_tx: UnboundedSender<AgentCommand>,
    event_tx: StdSender<AgentEvent>,
    tts_tx: StdSender<TtsCommand>,
    llm_config: LlmConfig,
) {
    let bg_llm_config = llm_config.clone();
    let bg_tts_tx = tts_tx.clone();
    let scheduler_cmd_tx = cmd_tx.clone();
    std::thread::spawn(move || {
        let rt = tokio::runtime::Runtime::new().expect("Échec d'initialisation du runtime Tokio");
        automation::init_scheduler(scheduler_cmd_tx, rt.handle().clone());
        rt.block_on(async move {
            let history = Arc::new(tokio::sync::Mutex::new(Vec::<ChatMessage>::new()));
            let last_call_time = Arc::new(tokio::sync::Mutex::new(None));
            let mut current_task: Option<tokio::task::JoinHandle<()>> = None;

            while let Some(cmd) = cmd_rx.recv().await {
                match cmd {
                    AgentCommand::AnalyzeNavigationZones { window_id, window_title, targets, context_meta } => {
                        let client = reqwest::Client::new();
                        let mut current_llm_config = resolve_llm_config();
                        if current_llm_config.provider == LlmProvider::Local {
                            current_llm_config.model = local_llm::get_active_preset().name.to_string();
                        }
                        let event_tx = event_tx.clone();
                        tokio::spawn(async move {
                            let (summary, zones) = analyze_navigation_zones_with_llm(&client, &current_llm_config, &window_title, &targets, context_meta.as_deref()).await;
                            let _ = event_tx.send(AgentEvent::NavigationZonesReady {
                                window_id,
                                window_title,
                                summary,
                                zones,
                            });
                        });
                    }
                    AgentCommand::Prompt(prompt) => {
                        println!("[Backend Tokio] AgentCommand::Prompt reçu : \"{}\"", prompt);
                        if prompt.trim().is_empty() || !prompt.chars().any(|c| c.is_alphanumeric()) {
                            println!("[Agent] Commande vide ignorée (bruit parasite), aucune requête LLM.");
                            let _ = event_tx.send(AgentEvent::StatusChanged(AgentStatus::Idle));
                            continue;
                        }
                        let request_id = CURRENT_REQUEST_ID.fetch_add(1, Ordering::SeqCst) + 1;
                        if let Some(handle) = current_task.take() {
                            handle.abort();
                        }
                        let _ = bg_tts_tx.send(TtsCommand::Stop);
                        IS_EMERGENCY_STOPPED.store(false, Ordering::SeqCst);
                        let initial_passes = 6;
                        TARGET_REASONING_PASSES.store(initial_passes, Ordering::SeqCst);

                        let history = history.clone();
                        let last_call_time = last_call_time.clone();
                        let event_tx = event_tx.clone();
                        let task_tts_tx = bg_tts_tx.clone();
                        let mut current_llm_config = resolve_llm_config();
                        if current_llm_config.provider == LlmProvider::Local {
                            current_llm_config.model = local_llm::get_active_preset().name.to_string();
                        }

                        current_task = Some(tokio::spawn(async move {
                            if is_request_cancelled(request_id) {
                                return;
                            }
                            if let Some(cli_feedback) = try_execute_direct_cli(&prompt) {
                                println!("[Backend Tokio] Commande CLI directe exécutée : {}", cli_feedback);
                                if is_request_cancelled(request_id) { return; }
                                let _ = event_tx.send(AgentEvent::ScreenPayloadSize(0.0));
                                let _ = event_tx.send(AgentEvent::ReplaceNarration {
                                    text: cli_feedback.clone(),
                                    quick_suggestions: Vec::new(),
                                });
                                let _ = event_tx.send(AgentEvent::StatusChanged(AgentStatus::Idle));
                            } else if let Some(text_to_write) = parse_write_command(&prompt) {
                                println!("[Backend Tokio] Commande d'écriture directe détectée");
                                if is_request_cancelled(request_id) { return; }
                                let _ = event_tx.send(AgentEvent::ScreenPayloadSize(0.0));
                                let _ = event_tx.send(AgentEvent::StatusChanged(AgentStatus::Thinking));
                                let narration = tokio::task::spawn_blocking(move || {
                                    write_to_browser_or_txt(&text_to_write)
                                }).await.unwrap_or_else(|_| "Erreur lors de l'écriture.".to_string());
                                if is_request_cancelled(request_id) { return; }
                                let _ = event_tx.send(AgentEvent::ReplaceNarration {
                                    text: narration,
                                    quick_suggestions: Vec::new(),
                                });
                                let _ = event_tx.send(AgentEvent::StatusChanged(AgentStatus::Idle));
                            } else if IS_DICTATION_ACTIVE.load(Ordering::SeqCst) {
                                println!("[Backend Tokio] Mode dictée actif, prompt dédié envoyé");
                                if is_request_cancelled(request_id) { return; }
                                let _ = event_tx.send(AgentEvent::ScreenPayloadSize(0.0));
                                let final_prompt = format!(
                                    "[MODE DICTÉE ACTIF] : L'utilisateur est en mode dictée vocale. Analyse sa voix : s'il s'agit d'une consigne pour arrêter/quitter la dictée, appelle 'deactivate_dictation'. S'il s'agit de texte à insérer, retranscris-le avec ponctuation soignée via 'dictate' (target: null) et garde narration: \"\".\n\n[Demande utilisateur]\n{prompt}"
                                );
                                call_llm_prompt(
                                    current_llm_config,
                                    prompt.clone(),
                                    final_prompt,
                                    history,
                                    event_tx,
                                    task_tts_tx.clone(),
                                    last_call_time,
                                    PromptTrigger::User,
                                    request_id,
                                ).await;
                            } else {
                                println!("[Backend Tokio] Collecte de l'état de l'écran et préparation de l'appel LLM...");
                                let is_local_engine = current_llm_config.provider == LlmProvider::Local;
                                let tier = local_llm::resolve_model_tier(
                                    current_llm_config.provider,
                                    Some(local_llm::get_active_preset()),
                                );
                                let active_win_only = match tier {
                                    local_llm::ModelTier::Remote => false,
                                    _ => local_llm::get_local_llm_options().active_window_only,
                                };
                                let (field_content, screen_state): (Option<String>, String) = tokio::task::spawn_blocking(move || {
                                    #[cfg(windows)]
                                    {
                                        if is_local_engine && active_win_only {
                                            let current_pid = std::process::id();
                                            let fg = unsafe { windows::Win32::UI::WindowsAndMessaging::GetForegroundWindow() };
                                            let (fg_pid, _) = get_window_process_info(fg);
                                            let target_hwnd = if !fg.0.is_null() && fg_pid != current_pid && !is_window_hung(fg) {
                                                Some(fg)
                                            } else {
                                                LAST_EXTERNAL_HWND.lock().ok().and_then(|g| *g)
                                                    .map(|raw| windows::Win32::Foundation::HWND(raw as _))
                                                    .filter(|&h| !h.0.is_null() && !is_window_hung(h) && get_window_process_info(h).0 != current_pid)
                                            };
                                            if let Some(hwnd) = target_hwnd {
                                                let title = get_window_title(hwnd);
                                                let targets = collect_numbered_targets_for_window(hwnd);
                                                let compact_ui = format_compact_window_elements(&title, &targets);
                                                return (get_active_field_content(), compact_ui);
                                            } else {
                                                return (get_active_field_content(), "[Aucune fenêtre externe au premier plan]".to_string());
                                            }
                                        }
                                        if active_win_only {
                                            let fg = unsafe { windows::Win32::UI::WindowsAndMessaging::GetForegroundWindow() };
                                            let current_pid = std::process::id();
                                            let (fg_pid, _) = get_window_process_info(fg);
                                            let target = if !fg.0.is_null() && fg_pid != current_pid && !is_window_hung(fg) {
                                                Some(fg)
                                            } else {
                                                LAST_EXTERNAL_HWND.lock().ok().and_then(|g| *g).map(|raw| windows::Win32::Foundation::HWND(raw as _)).filter(|&h| !h.0.is_null() && !is_window_hung(h))
                                            };
                                            if let Some(h) = target {
                                                if get_window_process_info(h).0 != current_pid {
                                                    (get_active_field_content(), extract_window_text_only(h))
                                                } else {
                                                    (get_active_field_content(), summarize_screen_state(None, true))
                                                }
                                            } else {
                                                (get_active_field_content(), summarize_screen_state(None, true))
                                            }
                                        } else {
                                            (get_active_field_content(), summarize_screen_state(None, true))
                                        }
                                    }
                                    #[cfg(not(windows))]
                                    {
                                        if is_local_engine && active_win_only {
                                            let targets = collect_numbered_targets_for_active_window();
                                            let compact_ui = format_compact_window_elements("Fenêtre active", &targets);
                                            return (get_active_field_content(), compact_ui);
                                        }
                                        (get_active_field_content(), summarize_screen_state(None, false))
                                    }
                                }).await.unwrap_or((None, String::new()));

                                if is_request_cancelled(request_id) { return; }
                                let mut prompt_sections = Vec::new();

                                if !screen_state.trim().is_empty() {
                                    if let Ok(mut lock) = LAST_SCREEN_SUMMARY.lock() {
                                        *lock = Some(screen_state.clone());
                                    }
                                    let screen_state = if screen_state.contains("<html") || screen_state.contains("<table") || screen_state.contains("<body") {
                                        html_util::sanitize_html_to_markdown(&screen_state)
                                    } else {
                                        screen_state
                                    };
                                    let max_screen = if screen_state.contains("[Page web") || screen_state.contains("[Navigateur]") {
                                        15360
                                    } else {
                                        12000
                                    };
                                    let safe_screen = truncate_with_notice(screen_state.trim(), max_screen);
                                    let kb = safe_screen.len() as f32 / 1024.0;
                                    let _ = event_tx.send(AgentEvent::ScreenPayloadSize(kb));
                                    prompt_sections.push(format!("[État actuel de l'écran]\n{safe_screen}"));
                                } else {
                                    let _ = event_tx.send(AgentEvent::ScreenPayloadSize(0.0));
                                }

                                if let Some(content) = field_content {
                                    if !content.trim().is_empty() {
                                        let safe_content = truncate_with_notice(content.trim(), 1200);
                                        prompt_sections.push(format!("[Contenu actuel du champ de saisie]\n\"\"\"\n{safe_content}\n\"\"\""));
                                    }
                                }

                            // 1. Injection automatique de l'état des fenêtres volantes ouvertes au LLM
                            if !active_win_only {
                                if let Ok(guard) = OPEN_FLOATING_CARDS.lock() {
                                    if !guard.is_empty() {
                                        let mut card_section = String::from("[Fenêtres volantes actuellement ouvertes à l'écran] :\n");
                                        for c in guard.iter() {
                                            card_section.push_str(&format!("- ID: \"{}\" | Titre: \"{}\" | Type: \"{}\"\n", c.id, c.title, c.card_type));
                                            if !c.content_preview.is_empty() {
                                                let preview = truncate_with_notice(&c.content_preview, 1500);
                                                card_section.push_str(&format!("  [Contenu actuel déjà affiché à conserver/fusionner] :\n\"\"\"\n{}\n\"\"\"\n", preview.trim()));
                                            }
                                        }
                                        card_section.push_str("Instruction : Ces fenêtres volantes sont déjà affichées à l'écran. Si ta réponse apporte de nouvelles données, une synthèse ou une mise à jour, réémets impérativement \"show_floating_card\" avec le même ID en FUSIONNANT les données existantes. Ne ferme JAMAIS la carte si tu la mets à jour.");
                                        prompt_sections.push(card_section);
                                    }
                                }
                            }

                                let (_, prompt_lang) = get_system_locale_language();
                                prompt_sections.push(format!(
                                    "[Demande utilisateur]\n{prompt}\n\n[Consigne linguistique impérative]\nFormule impérativement ta \"narration\" et tes suggestions en {prompt_lang}. Traduis tout contenu d'écran vers le {prompt_lang}."
                                ));
                                let final_prompt = prompt_sections.join("\n\n");

                                call_llm_prompt(
                                    current_llm_config,
                                    prompt.clone(),
                                    final_prompt,
                                    history,
                                    event_tx,
                                    task_tts_tx,
                                    last_call_time,
                                    PromptTrigger::User,
                                    request_id,
                                ).await;
                            }
                        }));
                    }
                    AgentCommand::ClearHistory => {
                        let _ = CURRENT_REQUEST_ID.fetch_add(1, Ordering::SeqCst);
                        if let Some(handle) = current_task.take() {
                            handle.abort();
                        }
                        let _ = bg_tts_tx.send(TtsCommand::Stop);
                        cancel_all_scheduled_tasks();
                        IS_EMERGENCY_STOPPED.store(false, Ordering::SeqCst);
                        IS_DICTATION_ACTIVE.store(false, Ordering::SeqCst);
                        TARGET_REASONING_PASSES.store(5, Ordering::SeqCst);
                        if let Ok(mut hist) = IMMERSION_SCREEN_HISTORY.lock() {
                            hist.clear();
                        }
                        history.lock().await.clear();
                        if let Ok(mut lock) = LAST_SCREEN_SUMMARY.lock() {
                            *lock = None;
                        }
                    }
                    AgentCommand::SearchTwitch(query) => {
                        let event_tx = event_tx.clone();
                        tokio::spawn(async move {
                            let results = search_twitch_channels(&query).await;
                            let _ = event_tx.send(AgentEvent::TwitchSearchResults(results));
                        });
                    }
                    AgentCommand::PeriodicScreenCheck => {
                        if IS_EMERGENCY_STOPPED.load(Ordering::SeqCst)
                            || bg_llm_config.provider == LlmProvider::Local
                        {
                            continue;
                        }
                        if AGENT_BUSY.load(Ordering::SeqCst)
                            || current_task.as_ref().map_or(false, |h| !h.is_finished())
                        {
                            println!("[Surveillance] Agent occupé, analyse d'écran différée.");
                            continue;
                        }

                        let request_id = CURRENT_REQUEST_ID.fetch_add(1, Ordering::SeqCst) + 1;
                        let history = history.clone();
                        let last_call_time = last_call_time.clone();
                        let event_tx = event_tx.clone();
                        let task_tts_tx = bg_tts_tx.clone();
                        let mut current_llm_config = resolve_llm_config();
                        if current_llm_config.provider == LlmProvider::Local {
                            current_llm_config.model = local_llm::get_active_preset().name.to_string();
                        }

                        current_task = Some(tokio::spawn(async move {
                            if is_request_cancelled(request_id) { return; }
                            let current_summary = tokio::task::spawn_blocking(|| {
                                summarize_screen_state(None, false)
                            }).await.unwrap_or_default();

                            if is_request_cancelled(request_id) { return; }

                            let is_immersion = IS_IMMERSION_ACTIVE.load(Ordering::SeqCst);
                            if is_immersion {
                                let (screen_diff, screen_changed) = {
                                    let mut hist = IMMERSION_SCREEN_HISTORY.lock().unwrap_or_else(|e| e.into_inner());
                                    let last_summary = hist.last().cloned();
                                    hist.push(current_summary.clone());
                                    if hist.len() > 6 {
                                        hist.remove(0);
                                    }
                                    if let Some(prev) = last_summary {
                                        let diff = compute_screen_diff(&prev, &current_summary);
                                        let changed = !diff.trim().is_empty();
                                        (diff, changed)
                                    } else {
                                        (String::new(), true)
                                    }
                                };

                                if !screen_changed {
                                    println!("[Immersion] Aucun changement à l'écran, silence préservé.");
                                    return;
                                }

                                let immersion_prompt = if screen_diff.is_empty() {
                                    format!(
                                        "[Suivi du mode immersion - état initial]\n\n\
                                        [État d'écran actuel] :\n{}\n\n\
                                        Instruction : Session d'immersion active. Consigne de silence absolu : réponds STRICTEMENT avec \"narration\": \"\", \"actions\": [], \"quick_suggestions\": [].",
                                        truncate_with_notice(&current_summary, 1500)
                                    )
                                } else {
                                    format!(
                                        "[Suivi du mode immersion - différentiel des changements]\n\n\
                                        {}\n\n\
                                        Instruction impérative : Analyse UNIQUEMENT et STRICTEMENT les lignes de changement ci-dessus (+ et -). \
                                        Ne commente absolument rien de ce qui est fixe ou inchangé. \
                                        Consigne de silence absolu : si ces changements font partie du travail normal de l'utilisateur, réponds STRICTEMENT avec \"narration\": \"\", \"actions\": [], \"quick_suggestions\": []. \
                                        Ne commente que les éléments modifiés si une intervention ou suggestion est réellement utile.",
                                        truncate_with_notice(&screen_diff, 1800)
                                    )
                                };

                                println!("[Immersion] Changement détecté (diff: {} octets), transmission du diff au LLM...", screen_diff.len());
                                call_llm_prompt(current_llm_config, "Suivi immersion".to_string(), immersion_prompt, history, event_tx, task_tts_tx.clone(), last_call_time, PromptTrigger::Immersion { screen_changed: true }, request_id).await;
                                return;
                            }

                            let prev_summary = {
                                let mut lock = LAST_SCREEN_SUMMARY.lock().unwrap_or_else(|e| e.into_inner());
                                let prev = lock.clone();
                                *lock = Some(current_summary.clone());
                                prev
                            };

                            if let Some(prev) = prev_summary {
                                if prev.trim() == current_summary.trim() {
                                    println!("[Surveillance] Aucun changement à l'écran.");
                                    return;
                                }

                                println!("[Surveillance] Changement détecté, transmission du différentiel au LLM...");
                                let diff_prompt = format!(
                                    "[Surveillance périodique automatique de l'écran]\n\n\
                                    [État d'écran précédent] :\n{}\n\n\
                                    [État d'écran actuel] :\n{}\n\n\
                                    Instruction : Analyse les différences entre ces deux états d'écran. \
                                    Détermine si une intervention ou une suite d'actions est nécessaire ou utile pour assister l'utilisateur.\n\
                                    - Si aucune intervention n'est requise : réponds STRICTEMENT avec \"actions\": [] et \"narration\": \"\" pour préserver le silence.\n\
                                    - Si une intervention est pertinente : renseigne \"narration\" pour expliquer ce que tu constates et fournis la suite d'actions à exécuter dans \"actions\".",
                                    truncate_with_notice(&prev, 1500),
                                    truncate_with_notice(&current_summary, 1500)
                                );

                                call_llm_prompt(current_llm_config, "Surveillance écran".to_string(), diff_prompt, history, event_tx, task_tts_tx, last_call_time, PromptTrigger::Surveillance, request_id).await;
                            } else {
                                println!("[Surveillance] Premier instantané d'écran enregistré.");
                            }
                        }));
                    }
                    AgentCommand::UndoLastAction => {
                        let _ = CURRENT_REQUEST_ID.fetch_add(1, Ordering::SeqCst);
                        if let Some(handle) = current_task.take() {
                            handle.abort();
                        }
                        let _ = bg_tts_tx.send(TtsCommand::Stop);
                        let feedback = if let Some(fb) = undo_last_system_action() {
                            fb
                        } else {
                            #[cfg(windows)]
                            send_hotkey(&[windows::Win32::UI::Input::KeyboardAndMouse::VIRTUAL_KEY(0x11)], windows::Win32::UI::Input::KeyboardAndMouse::VIRTUAL_KEY(0x5A));
                            #[cfg(not(windows))]
                            let _ = std::process::Command::new("xdotool").args(["key", "ctrl+z"]).status();
                            t!("commands.last_action_undone").to_string()
                        };
                        let _ = event_tx.send(AgentEvent::ReplaceNarration {
                            text: feedback.clone(),
                            quick_suggestions: Vec::new(),
                        });
                        let _ = bg_tts_tx.send(TtsCommand::Speak(feedback));
                        let _ = event_tx.send(AgentEvent::StatusChanged(AgentStatus::Idle));
                    }
                }
            }
        });
    });
}

fn run_overlay_gui(
    event_tx: StdSender<AgentEvent>,
    event_rx: Receiver<AgentEvent>,
    cmd_tx: UnboundedSender<AgentCommand>,
    tts_tx: StdSender<TtsCommand>,
    audio_tx: StdSender<AudioCommand>,
    twitch_ch_tx: StdSender<String>,
    initial_twitch_channel: String,
    initial_voice: VoiceGender,
) -> eframe::Result<()> {
    // Fenêtre transparente compacte, ferrée en bas à droite
    let mut viewport = egui::ViewportBuilder::default()
        .with_decorations(false)
        .with_transparent(true)
        .with_always_on_top()
        .with_resizable(true)
        .with_min_inner_size([380.0, 158.0])
        .with_inner_size([560.0, 196.0]);

    if let Ok(icon) = eframe::icon_data::from_png_bytes(include_bytes!("../public/favicon-256x256.png")) {
        viewport = viewport.with_icon(Arc::new(icon));
    }

    let native_options = eframe::NativeOptions {
        renderer: eframe::Renderer::Glow,
        viewport,
        ..Default::default()
    };

    eframe::run_native(
        "Libertide overlay",
        native_options,
        Box::new(move |cc| {
            Ok(Box::new(OverlayApp::new(
                cc,
                event_tx,
                event_rx,
                cmd_tx,
                tts_tx,
                audio_tx,
                twitch_ch_tx,
                initial_twitch_channel,
                initial_voice,
            )))
        }),
    )
}

fn main() -> eframe::Result<()> {
    fix_startup_working_directory();

    let (event_tx, event_rx) = channel::<AgentEvent>();
    let (cmd_tx, cmd_rx) = unbounded_channel::<AgentCommand>();
    let initial_voice = resolve_initial_voice();
    let tts_tx = spawn_tts_worker(event_tx.clone(), initial_voice);
    let (twitch_ch_tx, twitch_ch_rx) = channel::<String>();
    spawn_twitch_worker(event_tx.clone(), twitch_ch_rx);
    let llm_config = resolve_llm_config();
    let audio_tx = spawn_audio_worker(event_tx.clone(), llm_config.api_key.clone());

    let initial_twitch_channel = std::env::var("TWITCH_CHANNEL").unwrap_or_default();

    #[cfg(windows)]
    automation::install_switch_keyboard_hook();

    spawn_periodic_screen_monitor(cmd_tx.clone());
    spawn_agent_backend(cmd_rx, cmd_tx.clone(), event_tx.clone(), tts_tx.clone(), llm_config.clone());

    run_overlay_gui(
        event_tx,
        event_rx,
        cmd_tx,
        tts_tx,
        audio_tx,
        twitch_ch_tx,
        initial_twitch_channel,
        initial_voice,
    )
}