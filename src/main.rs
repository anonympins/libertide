#![cfg_attr(not(debug_assertions), windows_subsystem = "windows")]

#[macro_use]
extern crate rust_i18n;
i18n!("locales", fallback = "en");

mod autostart;
mod audio;
mod audio_player;
mod automation;
mod floating_window;
mod fs_util;
mod html_util;
mod pattern;
mod scheduler;
mod theme;
mod tts;
mod intent;
mod locale;
mod app_switch;
mod app_settings;
mod twitch;
mod types;
mod window_util;
mod commands;
mod overlay;
mod ui;
mod ocr;
mod devices;
mod local_llm;
mod agent_backend;
pub use agent_backend::*;
pub use locale::*;

use audio::spawn_audio_worker;
use floating_window::{update_open_floating_cards, FloatingCard, FloatingCardOutput, FloatingContentType};
use automation::*;
use fs_util::*;
use intent::*;
use theme::{localized_theme_desc, localized_theme_name, AppTheme, SettingsTab};
use tts::{sanitize_for_tts, spawn_tts_worker};
use twitch::spawn_twitch_worker;
use window_util::*;
use eframe::egui;
pub use overlay::*;
use types::*;
use std::sync::Arc;
use std::sync::atomic::Ordering;
use std::sync::mpsc::{channel, Receiver, Sender as StdSender};
use tokio::sync::mpsc::{unbounded_channel, UnboundedSender};
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
    voice_clone_url_input: String,
    voice_reference_wav_input: String,
    voice_reference_text_input: String,
    voice_test_text_input: String,
    voice_status_feedback: Option<String>,
    voice_is_recording_sample: bool,
    voice_record_append: bool,
    voice_record_start: Option<Instant>,
    voice_recorder_stop_tx: Option<std::sync::mpsc::Sender<()>>,
    voice_recorder_result_rx: Option<std::sync::mpsc::Receiver<Vec<f32>>>,
    voice_sample_player: Option<crate::audio_player::AudioPlayer>,
    voice_sample_duration_secs: f32,
    voice_clone_gender: String,
    voice_guide_phrase: String,
    voice_guide_generating: bool,
    voice_guide_rx: Option<std::sync::mpsc::Receiver<String>>,
    llm_config: LlmConfig,
    theme: AppTheme,
    settings_tab: SettingsTab,
    dwell_state: DwellState,
    last_trackball_input: Instant,

    // Accessibilité : Ciblage par numéros & Mode contacteur unique (Switch Access)
    numbered_targets: Vec<NumberedElementTarget>,
    numbered_chunk_offset: usize,
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
    twitch_filter: TwitchFilter,
    auto_scroll_to_bottom: bool,
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

    // Suggestions d'applications en direct (PATH & Levenshtein)
    input_suggestions: Vec<String>,
    last_suggestion_query: String,

    // Macros personnalisées ('raccourci' => texte complet)
    macros: Vec<(String, String)>,
    macro_shortcut_input: String,
    macro_text_input: String,
    macro_error: Option<String>,

    // Options d'accessibilité et presets cumulatifs
    active_disability_presets: std::collections::BTreeSet<DisabilityPreset>,
    auto_read_agent_messages: bool,
    auto_read_suggestions: bool,
    earcons_enabled: bool,

    // Périphériques matériels (Pointage et Microphones)
    detected_microphones: Vec<String>,
    selected_microphone: String,
    detected_pointing_devices: Vec<crate::devices::PointingDevice>,
    pending_twitch_speech: Option<String>,
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
        let initial_speech = if let Some(sug_speech) = format_suggestions_for_speech(&initial_suggestions) {
            format!("{initial_subtitle}\n\n{sug_speech}")
        } else {
            initial_subtitle.clone()
        };
        let _ = tts_sender.send(TtsCommand::Speak(initial_speech));

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

        #[cfg(windows)]
        {
            let ctx_clone = cc.egui_ctx.clone();
            crate::automation::set_repaint_callback(move || {
                ctx_clone.request_repaint();
            });
        }

        let macros = load_macros();
        let long_press_ms = std::env::var("SWITCH_LONG_PRESS_MS")
            .ok()
            .or_else(|| read_env_var_from_file(".env", "SWITCH_LONG_PRESS_MS"))
            .and_then(|s| s.trim().parse::<u64>().ok())
            .unwrap_or(1100);
        SWITCH_LONG_PRESS_MS.store(long_press_ms, Ordering::Relaxed);

        let debounce_ms = std::env::var("SWITCH_DEBOUNCE_MS")
            .ok()
            .or_else(|| read_env_var_from_file(".env", "SWITCH_DEBOUNCE_MS"))
            .and_then(|s| s.trim().parse::<u64>().ok())
            .unwrap_or(180);
        SWITCH_DEBOUNCE_MS.store(debounce_ms, Ordering::Relaxed);

        let detected_microphones = crate::devices::list_microphones();
        let selected_microphone = crate::devices::get_active_microphone();
        let detected_pointing_devices = crate::devices::list_pointing_devices();

        let voice_clone_url_input = std::env::var("VOICE_CLONE_URL")
            .ok()
            .or_else(|| read_env_var_from_file(".env", "VOICE_CLONE_URL"))
            .or_else(|| std::env::var("TTS_CLONE_URL").ok())
            .or_else(|| read_env_var_from_file(".env", "TTS_CLONE_URL"))
            .unwrap_or_else(|| "http://127.0.0.1:8020/tts".to_string());

        let voice_reference_wav_input = std::env::var("VOICE_REFERENCE_WAV")
            .ok()
            .or_else(|| read_env_var_from_file(".env", "VOICE_REFERENCE_WAV"))
            .unwrap_or_default();

        let voice_reference_text_input = std::env::var("VOICE_REFERENCE_TEXT")
            .ok()
            .or_else(|| read_env_var_from_file(".env", "VOICE_REFERENCE_TEXT"))
            .unwrap_or_default();

        let auto_read_agent_messages = std::env::var("AUTO_READ_MESSAGES")
            .ok()
            .or_else(|| read_env_var_from_file(".env", "AUTO_READ_MESSAGES"))
            .map(|s| !s.trim().eq_ignore_ascii_case("false") && s.trim() != "0")
            .unwrap_or(true);

        let auto_read_suggestions = std::env::var("AUTO_READ_SUGGESTIONS")
            .ok()
            .or_else(|| read_env_var_from_file(".env", "AUTO_READ_SUGGESTIONS"))
            .map(|s| !s.trim().eq_ignore_ascii_case("false") && s.trim() != "0")
            .unwrap_or(true);

        let earcons_enabled = std::env::var("EARCONS_ENABLED")
            .ok()
            .or_else(|| read_env_var_from_file(".env", "EARCONS_ENABLED"))
            .map(|s| !s.trim().eq_ignore_ascii_case("false") && s.trim() != "0")
            .unwrap_or(true);
        crate::overlay::EARCONS_ENABLED.store(earcons_enabled, Ordering::Relaxed);

        let active_disability_presets: std::collections::BTreeSet<DisabilityPreset> = std::env::var("DISABILITY_PRESETS")
            .ok()
            .or_else(|| read_env_var_from_file(".env", "DISABILITY_PRESETS"))
            .or_else(|| std::env::var("DISABILITY_PRESET").ok())
            .or_else(|| read_env_var_from_file(".env", "DISABILITY_PRESET"))
            .map(|s| {
                s.split([',', ';', ' '])
                    .filter_map(DisabilityPreset::from_key)
                    .collect()
            })
            .unwrap_or_default();

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
            voice_clone_url_input,
            voice_reference_wav_input,
            voice_reference_text_input,
            voice_test_text_input: "Bonjour, ceci est un test de synthèse vocale avec Libertide.".to_string(),
            voice_status_feedback: None,
            voice_is_recording_sample: false,
            voice_record_append: false,
            voice_record_start: None,
            voice_recorder_stop_tx: None,
            voice_recorder_result_rx: None,
            voice_sample_player: crate::audio_player::AudioPlayer::new().ok(),
            voice_sample_duration_secs: 0.0,
            voice_clone_gender: std::env::var("VOICE_CLONE_GENDER")
                .ok()
                .or_else(|| read_env_var_from_file(".env", "VOICE_CLONE_GENDER"))
                .map(|s| s.trim().to_lowercase())
                .filter(|s| s == "male" || s == "female" || s == "homme" || s == "femme")
                .map(|s| if s == "male" || s == "homme" { "male".to_string() } else { "female".to_string() })
                .unwrap_or_else(|| "female".to_string()),
            voice_guide_phrase: "« Bonjour, je configure ma voix personnalisée dans Libertide. Ce court extrait audio sert de référence pour le modèle de clonage vocal. »".to_string(),
            voice_guide_generating: false,
            voice_guide_rx: None,
            llm_config: llm_config.clone(),
            theme,
            settings_tab: SettingsTab::LlmProvider,
            dwell_state: DwellState::new_with_delay(dwell_millis, dwell_activation_ms),
            last_trackball_input: Instant::now() - Duration::from_secs(1),

            numbered_targets: Vec::new(),
            numbered_chunk_offset: 0,
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
            twitch_filter: TwitchFilter::All,
            auto_scroll_to_bottom: true,
            position_initialized: false,
            floating_cards: Vec::new(),
            window_pos: None,
            window_bottom_y: None,
            drag_offset: None,
            current_win_w: 560.0,
            chat_height: 70.0,
            current_win_h: 280.0,
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
            input_suggestions: Vec::new(),
            last_suggestion_query: String::new(),
            macros,
            macro_shortcut_input: String::new(),
            macro_text_input: String::new(),
            macro_error: None,

            active_disability_presets,
            auto_read_agent_messages,
            auto_read_suggestions,
            earcons_enabled,

            detected_microphones,
            selected_microphone,
            detected_pointing_devices,
            pending_twitch_speech: None,
        };
        app.sync_modal_inputs_for_provider(llm_config.provider);
        update_open_floating_cards(&app.floating_cards);
        app
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

    pub fn set_voice_gender_silent(&mut self, gender: VoiceGender) {
        self.voice_gender = gender;
        let _ = self.tts_sender.send(TtsCommand::SetGender(gender));
        std::env::set_var("VOICE_PROFILE", gender.to_key_str());
        let _ = write_env_var_to_file(".env", "VOICE_PROFILE", gender.to_key_str());
    }

    pub fn set_voice_gender(&mut self, gender: VoiceGender) {
        self.set_voice_gender_silent(gender);
        let confirmation = match gender {
            VoiceProfile::Denise => t!("speech.voice_denise").to_string(),
            VoiceProfile::Henri => t!("speech.voice_henri").to_string(),
            VoiceProfile::Hortense => t!("speech.voice_hortense").to_string(),
            VoiceProfile::Paul => t!("speech.voice_paul").to_string(),
            VoiceProfile::Sylvie => t!("speech.voice_sylvie").to_string(),
            VoiceProfile::Jean => t!("speech.voice_jean").to_string(),
            VoiceProfile::Clone => "Voix clonée personnalisée activée.".to_string(),
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
        self.pending_twitch_speech = None;
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
            self.is_hidden = true;
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
                self.switch_step = SwitchAccessStep::Windows;
                self.numbered_last_hwnd = Some(cur_win.id);
                self.numbered_targets.clear();

                crate::automation::IS_USER_TYPING.store(false, Ordering::SeqCst);

                #[cfg(windows)]
                {
                    let hwnd = windows::Win32::Foundation::HWND(cur_win.id as _);
                    focus_and_isolate_window(hwnd);
                    let mut r = windows::Win32::Foundation::RECT::default();
                    if unsafe { windows::Win32::UI::WindowsAndMessaging::GetWindowRect(hwnd, &mut r).is_ok() } {
                        draw_screen_orange_border([r.left, r.top, r.right, r.bottom]);
                    } else {
                        draw_screen_orange_border(cur_win.rect);
                    }
                    set_global_cursor_pos(cur_win.click_x, cur_win.click_y);
                    self.hover_last_cursor_pos = Some((cur_win.click_x, cur_win.click_y));
                    clear_screen_numbered_badges();
                }
                #[cfg(not(windows))]
                {
                    focus_and_isolate_window(&cur_win.title);
                    set_global_cursor_pos(cur_win.click_x, cur_win.click_y);
                    self.hover_last_cursor_pos = Some((cur_win.click_x, cur_win.click_y));
                }

                // Pré-chargement des zones et analyse UIA de la fenêtre active en tâche de fond
                self.refresh_numbered_targets_for_window(cur_win.id);

                let vocal_msg = format!("{}. {}", self.switch_focus_index + 1, cur_win.title);
                let _ = self.tts_sender.send(TtsCommand::Stop);
                let _ = self.tts_sender.send(TtsCommand::Speak(vocal_msg));
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
            self.is_hidden = false;
            self.numbered_targets.clear();
        self.numbered_chunk_offset = 0;
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

    pub fn restore_window_if_minimized(&mut self, ctx: &egui::Context) {
        let mut is_min = ctx.input(|i| i.viewport().minimized.unwrap_or(false));
        #[cfg(windows)]
        {
            unsafe {
                use windows::core::w;
                use windows::Win32::UI::WindowsAndMessaging::{FindWindowW, IsIconic};
                let overlay = FindWindowW(None, w!("Libertide overlay")).unwrap_or(windows::Win32::Foundation::HWND(std::ptr::null_mut()));
                if !overlay.0.is_null() && IsIconic(overlay).as_bool() {
                    is_min = true;
                }
            }
        }

        if is_min {
            if !self.switch_access_mode {
                self.is_hidden = false;
            }
            ctx.send_viewport_cmd(egui::ViewportCommand::Minimized(false));
            if let Some(pos) = self.window_pos {
                ctx.send_viewport_cmd(egui::ViewportCommand::OuterPosition(pos));
            }
            #[cfg(windows)]
            {
                unsafe {
                    use windows::core::w;
                    use windows::Win32::UI::WindowsAndMessaging::{FindWindowW, ShowWindow, SetForegroundWindow, SW_RESTORE};
                    let overlay = FindWindowW(None, w!("Libertide overlay")).unwrap_or(windows::Win32::Foundation::HWND(std::ptr::null_mut()));
                    if !overlay.0.is_null() {
                        let _ = ShowWindow(overlay, SW_RESTORE);
                        let _ = SetForegroundWindow(overlay);
                    }
                }
            }
            ctx.request_repaint();
        }
    }

    fn start_recording(&mut self, _ctx: &egui::Context, play_sound: bool) {
        if self.status == AgentStatus::EmergencyStopped || self.voice_is_recording_sample {
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
        if self.voice_is_recording_sample {
            return;
        }
        if self.is_recording {
            self.stop_recording(); // This will trigger prompt processing if text is present
        } else {
            self.start_recording(ctx, true);
        }
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

        IS_CHAT_TAB_ACTIVE.store(self.active_tab == ActiveTab::Chat, Ordering::Relaxed);

        update_last_external_focus();

        // Détection de saisie dans l'UI Libertide : seul un TextEdit de Libertide actif libère les touches
        let is_egui_text_focused = !self.is_hidden && ctx.memory(|m| {
            m.has_focus(egui::Id::NULL.with("prompt_input_text"))
                || m.has_focus(egui::Id::NULL.with("api_key_input_field"))
                || m.has_focus(egui::Id::NULL.with("model_input_field"))
                || m.has_focus(egui::Id::NULL.with("endpoint_input_field"))
                || m.has_focus(egui::Id::NULL.with("macro_shortcut_input"))
                || m.has_focus(egui::Id::NULL.with("macro_text_input"))
                || m.has_focus(egui::Id::NULL.with("twitch_search_query_input"))
                || m.has_focus(egui::Id::NULL.with("voice_clone_url_input"))
                || m.has_focus(egui::Id::NULL.with("voice_reference_wav_input"))
                || m.has_focus(egui::Id::NULL.with("voice_reference_text_input"))
                || m.has_focus(egui::Id::NULL.with("voice_test_text_input"))
        });
        let is_typing = !self.is_hidden && (ctx.egui_wants_keyboard_input()
            || self.show_api_key_modal
            || is_egui_text_focused);
        crate::automation::IS_USER_TYPING.store(is_typing, Ordering::Relaxed);

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
        visuals.window_fill = theme.control_bg;
        visuals.window_stroke = egui::Stroke::new(1.0, theme.border);
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
                                && !self.voice_is_recording_sample
                                && self.status != AgentStatus::EmergencyStopped
                                && (!self.is_hidden || self.switch_access_mode)
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
                    self.auto_scroll_to_bottom = true;
                    self.chat_history.push(ChatEntry {
                        role: ChatRole::Agent,
                        text: String::new(),
                        timestamp: current_time_str(),
                        quick_suggestions: Vec::new(),
                        screen_size_kb: None,
                    });
                }
                AgentEvent::NarrationChunk(chunk) => {
                    self.auto_scroll_to_bottom = true;
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
                    self.auto_scroll_to_bottom = true;
                    self.is_streaming_narration = false;

                    if text.trim().is_empty() && quick_suggestions.is_empty() {
                        continue;
                    }
                    if text.trim().is_empty() {
                        if let Some(last) = self.chat_history.last_mut().filter(|e| e.role == ChatRole::Agent) {
                            for sug in &quick_suggestions {
                                if !last.quick_suggestions.iter().any(|s| s.as_str() == sug.as_str()) {
                                    last.quick_suggestions.push(sug.clone());
                                }
                            }
                        }
                        if self.auto_read_suggestions && self.auto_read_agent_messages && self.active_tab == ActiveTab::Assistance {
                            if let Some(sug_speech) = format_suggestions_for_speech(&quick_suggestions) {
                                if !sanitize_for_tts(&sug_speech).trim().is_empty() {
                                    self.status = AgentStatus::Speaking;
                                    let _ = self.tts_sender.send(TtsCommand::Enqueue(sug_speech));
                                }
                            }
                        }
                    } else if was_streaming || self.llm_config.provider == LlmProvider::Local {
                        if let Some(last) = self.chat_history.last_mut().filter(|e| e.role == ChatRole::Agent) {
                            last.text = text.clone();
                            last.quick_suggestions = quick_suggestions.clone();
                        } else {
                            self.chat_history.push(ChatEntry {
                                role: ChatRole::Agent,
                                text: text.clone(),
                                timestamp: current_time_str(),
                                quick_suggestions: quick_suggestions.clone(),
                                screen_size_kb: None,
                            });
                        }
                        if self.auto_read_suggestions && self.auto_read_agent_messages && self.active_tab == ActiveTab::Assistance {
                            if let Some(sug_speech) = format_suggestions_for_speech(&quick_suggestions) {
                                if !sanitize_for_tts(&sug_speech).trim().is_empty() {
                                    self.status = AgentStatus::Speaking;
                                    let _ = self.tts_sender.send(TtsCommand::Enqueue(sug_speech));
                                }
                            }
                        }
                    } else {
                        let sug_speech_opt = if self.auto_read_suggestions && self.auto_read_agent_messages {
                            format_suggestions_for_speech(&quick_suggestions)
                        } else {
                            None
                        };

                        let full_speech = match (self.auto_read_agent_messages, sug_speech_opt) {
                            (true, Some(sug_speech)) => {
                                if !sanitize_for_tts(&text).trim().is_empty() {
                                    format!("{text}\n\n{sug_speech}")
                                } else {
                                    sug_speech
                                }
                            }
                            (true, None) => text.clone(),
                            (false, Some(sug_speech)) => sug_speech,
                            (false, None) => String::new(),
                        };
                        if self.active_tab == ActiveTab::Assistance {
                            if !sanitize_for_tts(&full_speech).trim().is_empty() {
                                self.status = AgentStatus::Speaking;
                                let _ = self.tts_sender.send(TtsCommand::Speak(full_speech));
                            }
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
                AgentEvent::ReminderTriggered { label, prompt: _ } => {
                    self.is_hidden = false;
                    self.restore_window_if_minimized(&ctx);
                    let reminder_display = format!("⏰ **Rappel :** {label}");
                    self.chat_history.push(ChatEntry {
                        role: ChatRole::Agent,
                        text: reminder_display,
                        timestamp: current_time_str(),
                        quick_suggestions: vec![
                            QuickSuggestionItem::Text("Merci !".to_string()),
                            QuickSuggestionItem::Text("Rappelle-moi dans 5 minutes".to_string()),
                        ],
                        screen_size_kb: None,
                    });
                    ctx.request_repaint();
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
                    if self.voice_is_recording_sample {
                        continue;
                    }
                    let is_status_msg = text == "Écoute de votre voix..." || text == "Transcription locale en cours...";
                    if !is_status_msg && !text.trim().is_empty() && !self.switch_access_mode {
                        self.stop_speaking();
                    }
                    if self.is_recording && (!self.is_hidden || self.switch_access_mode) {
                        self.live_transcript = text;
                    }
                }
                AgentEvent::VoicePromptReady(prompt) => {
                    if self.voice_is_recording_sample {
                        continue;
                    }
                    self.is_recording = false;
                    self.live_transcript.clear();
                    let is_user_typing = ctx.memory(|m| m.has_focus(egui::Id::NULL.with("prompt_input_text")))
                        && !self.input_text.trim().is_empty();
                    if is_user_typing {
                        // L'utilisateur est activement en train de taper au clavier : on préserve son texte
                        if self.continuous_mode && self.status != AgentStatus::EmergencyStopped {
                            self.start_recording(&ctx, false);
                        }
                        return;
                    }
                    self.stop_speaking();

                    let raw_prompt = prompt.clone();
                    let prompt = self.expand_macro(&raw_prompt);
                    if prompt != raw_prompt {
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
                    if norm_prompt.contains("voix clone") || norm_prompt.contains("voix clonée") || norm_prompt == "clone" {
                        play_earcon(Earcon::Success);
                        self.set_voice_gender(VoiceProfile::Clone);
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

                    // Contrôle vocal du mode pensées / compagnon régulier
                    if norm_prompt.contains("active les pensees")
                        || norm_prompt.contains("active les pensées")
                        || norm_prompt.contains("active le compagnon proactif")
                        || norm_prompt.contains("pensees regulieres")
                        || norm_prompt.contains("pensées régulières")
                    {
                        crate::scheduler::set_proactive_companion_config(true, None);
                        play_earcon(Earcon::Success);
                        let reply = "Pensées régulières et présence proactive du compagnon activées.".to_string();
                        self.chat_history.push(ChatEntry { role: ChatRole::Agent, text: reply.clone(), timestamp: current_time_str(), quick_suggestions: Vec::new(), screen_size_kb: None });
                        let _ = self.tts_sender.send(TtsCommand::Speak(reply));
                        return;
                    }
                    if norm_prompt.contains("desactive les pensees")
                        || norm_prompt.contains("désactive les pensées")
                        || norm_prompt.contains("arrete les pensees")
                        || norm_prompt.contains("arrête les pensées")
                    {
                        crate::scheduler::set_proactive_companion_config(false, None);
                        play_earcon(Earcon::Success);
                        let reply = "Pensées régulières du compagnon mises en veille.".to_string();
                        self.chat_history.push(ChatEntry { role: ChatRole::Agent, text: reply.clone(), timestamp: current_time_str(), quick_suggestions: Vec::new(), screen_size_kb: None });
                        let _ = self.tts_sender.send(TtsCommand::Speak(reply));
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
                        self.chat_history.push(ChatEntry {
                            role: ChatRole::Agent,
                            text: feedback.to_string(),
                            timestamp: current_time_str(),
                            quick_suggestions: Vec::new(),
                            screen_size_kb: None,
                        });
                        let _ = self.tts_sender.send(TtsCommand::Speak(feedback.to_string()));
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
                    if self.is_hidden && !self.switch_access_mode {
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
                    if self.active_tab == ActiveTab::Chat {
                        if let Some(recent_speech) = self.pending_twitch_speech.take() {
                            self.status = AgentStatus::Speaking;
                            let _ = self.tts_sender.send(TtsCommand::Speak(recent_speech));
                            continue;
                        }
                    }
                    if self.continuous_mode
                        && !crate::audio::is_live_subtitles_active()
                        && self.status != AgentStatus::EmergencyStopped
                        && !self.show_api_key_modal
                        && !self.voice_is_recording_sample
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
                        m.has_focus(egui::Id::NULL.with("prompt_input_text"))
                            || m.has_focus(egui::Id::NULL.with("api_key_input_field"))
                            || m.has_focus(egui::Id::NULL.with("model_input_field"))
                    });
                    if self.continuous_mode
                        && self.status != AgentStatus::EmergencyStopped
                        && self.status != AgentStatus::Speaking
                        && !self.show_api_key_modal
                        && !self.voice_is_recording_sample
                        && !AGENT_BUSY.load(Ordering::SeqCst)
                        && !is_user_typing
                    {
                        self.start_recording(&ctx, true);
                    }
                    ctx.request_repaint();
                }
                AgentEvent::TwitchChatReceived(twitch_msg) => {
                    self.auto_scroll_to_bottom = true;
                    if self.active_tab == ActiveTab::Chat {
                        let speech = format!("{} : {}", twitch_msg.author, twitch_msg.text);
                        if self.status == AgentStatus::Speaking {
                            self.pending_twitch_speech = Some(speech);
                        } else {
                            self.pending_twitch_speech = None;
                            self.status = AgentStatus::Speaking;
                            let _ = self.tts_sender.send(TtsCommand::Speak(speech));
                        }
                    }
                    self.twitch_messages.push(twitch_msg);
                    if self.twitch_messages.len() > 300 {
                        self.twitch_messages.drain(0..self.twitch_messages.len() - 300);
                    }
                }
                AgentEvent::TwitchSearchResults(results) => {
                    self.twitch_search_results = results;
                }
                AgentEvent::OpenTwitchChannel { channel, open_stream } => {
                    self.stop_speaking();
                    self.active_tab = ActiveTab::Chat;
                    let clean = channel.trim().trim_start_matches('#').to_lowercase();
                    if !clean.is_empty() {
                        self.current_twitch_channel = clean.clone();
                        let _ = self.twitch_channel_sender.send(clean.clone());
                        self.twitch_messages.clear();
                        self.twitch_search_results.clear();
                        if open_stream {
                            open_browser_smart(&format!("https://twitch.tv/{}", clean));
                        }
                    }
                }
                AgentEvent::SwitchTab(tab) => {
                    self.pending_twitch_speech = None;
                    self.stop_speaking();
                    self.active_tab = tab;
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
                    let total = self.numbered_targets.len();
                    if total > 50 {
                        self.numbered_chunk_offset = (self.numbered_chunk_offset + 50) % total;
                        let end = (self.numbered_chunk_offset + 50).min(total);
                        let slice = &self.numbered_targets[self.numbered_chunk_offset..end];
                        let focused_idx = slice.first().map(|t| t.index);
                        draw_screen_numbered_badges(slice, self.zoom_factor, focused_idx);
                        let msg = format!("Affichage des badges {} à {}.", self.numbered_chunk_offset + 1, end);
                        let _ = self.tts_sender.send(TtsCommand::Speak(msg));
                        ctx.request_repaint();
                    }
                }
                AgentEvent::PreviousNumberedChunk => {
                    let total = self.numbered_targets.len();
                    if total > 0 {
                        self.numbered_chunk_offset = if self.numbered_chunk_offset >= 50 {
                            self.numbered_chunk_offset - 50
                        } else {
                            (total / 50) * 50
                        };
                        if self.numbered_chunk_offset >= total {
                            self.numbered_chunk_offset = 0;
                        }
                        let end = (self.numbered_chunk_offset + 50).min(total);
                        let slice = &self.numbered_targets[self.numbered_chunk_offset..end];
                        let focused_idx = slice.first().map(|t| t.index);
                        draw_screen_numbered_badges(slice, self.zoom_factor, focused_idx);
                        let msg = format!("Affichage des badges {} à {}.", self.numbered_chunk_offset + 1, end);
                        let _ = self.tts_sender.send(TtsCommand::Speak(msg));
                        ctx.request_repaint();
                    }
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
                        if self.switch_step == SwitchAccessStep::Elements {
                            if let Some(first_zone) = self.switch_zones.first() {
                                let mut active_targets = first_zone.targets.clone();
                                for (i, t) in active_targets.iter_mut().enumerate() {
                                    t.index = i + 1;
                                }
                                self.numbered_targets = active_targets;
                            }
                        } else if self.switch_step == SwitchAccessStep::Windows {
                            self.numbered_targets.clear();
                            clear_screen_numbered_badges();
                        } else if self.switch_step == SwitchAccessStep::Zones {
                            let zone_infos: Vec<ZoneOverlayInfo> = self.switch_zones.iter().enumerate().map(|(i, z)| {
                                ZoneOverlayInfo {
                                    index: i + 1,
                                    title: z.title.clone(),
                                    rect: z.rect,
                                    is_focused: i == 0,
                                }
                            }).collect();
                            draw_screen_zones_and_targets(&zone_infos, &[], self.zoom_factor, None);
                        }
                        if !summary.is_empty() && (self.hover_controls_speech_enabled || self.switch_step == SwitchAccessStep::Zones) {
                            let _ = self.tts_sender.send(TtsCommand::Stop);
                            let _ = self.tts_sender.send(TtsCommand::Speak(summary));
                        }
                        ctx.request_repaint();
                    }
                }
            }
        }

        // Récupération des événements prioritaires envoyés par le hook bas niveau Win32
        let mut hook_events = Vec::new();
        if let Ok(mut q) = SWITCH_KEY_QUEUE.lock() {
            hook_events.append(&mut *q);
        }

        let mut hook_digits = Vec::new();
        if let Ok(mut nq) = NUMBER_KEY_QUEUE.lock() {
            hook_digits.append(&mut *nq);
        }

        if !is_typing && self.switch_access_mode {
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

        let egui_f7 = ctx.input_mut(|i| i.consume_key(egui::Modifiers::NONE, egui::Key::F7));
        let egui_f8 = ctx.input_mut(|i| i.consume_key(egui::Modifiers::NONE, egui::Key::F8));
        let egui_f9 = ctx.input_mut(|i| i.consume_key(egui::Modifiers::NONE, egui::Key::F9));
        let egui_f4 = ctx.input_mut(|i| i.consume_key(egui::Modifiers::NONE, egui::Key::F4));

        if (egui_f8 || hook_events.contains(&SwitchKeyEvent::ToggleDictation))
            && self.last_switch_input.elapsed() >= Duration::from_millis(200)
        {
            self.last_switch_input = Instant::now();
            if self.dictation_mode {
                self.exit_dictation_mode();
            } else {
                self.enter_dictation_mode();
            }
            ctx.request_repaint();
        }
        if (egui_f7 || hook_events.contains(&SwitchKeyEvent::ToggleSwitch))
            && self.last_switch_input.elapsed() >= Duration::from_millis(200)
        {
            self.last_switch_input = Instant::now();
            self.toggle_switch_access_mode(None);
            ctx.request_repaint();
        }
        if (egui_f9 || hook_events.contains(&SwitchKeyEvent::ToggleVoice))
            && self.last_switch_input.elapsed() >= Duration::from_millis(200)
        {
            self.last_switch_input = Instant::now();
            self.set_voice_gender(self.voice_gender.next());
            ctx.request_repaint();
        }
        if (egui_f4 || hook_events.contains(&SwitchKeyEvent::Clear))
            && self.last_switch_input.elapsed() >= Duration::from_millis(200)
        {
            self.last_switch_input = Instant::now();
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
            ctx.request_repaint();
        }

        #[cfg(windows)]
        let global_escape_hit = unsafe { (GetAsyncKeyState(0x1B) as u16 & 0x8000) != 0 };
        #[cfg(not(windows))]
        let global_escape_hit = false;
        let hook_escape_hit = hook_events.contains(&SwitchKeyEvent::Escape)
            || (hook_events.contains(&SwitchKeyEvent::Back) && self.numbered_input_buffer.is_empty());

        let egui_backspace_hit = ctx.input(|i| {
            i.events.iter().any(|e| matches!(e, egui::Event::Key { key: egui::Key::Backspace, pressed: true, .. }))
        });
        let backspace_hit = (egui_backspace_hit && !is_typing) || hook_events.contains(&SwitchKeyEvent::Back);

        // Si le buffer contient des chiffres, Backspace efface le dernier chiffre sans quitter l'élément
        if backspace_hit && !self.numbered_input_buffer.is_empty() {
            self.numbered_input_buffer.pop();
            self.numbered_input_last_char_time = Instant::now();
            ctx.request_repaint();
        }

        let egui_escape_hit = ctx.input(|i| {
            i.events.iter().any(|e| matches!(e, egui::Event::Key { key: egui::Key::Escape, pressed: true, .. }))
        })
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
                        clear_screen_numbered_badges();
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
                    clear_screen_numbered_badges();
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
                    self.numbered_targets.clear();
                    clear_screen_numbered_badges();
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

                    if win_changed && (self.switch_step == SwitchAccessStep::Windows || self.switch_step == SwitchAccessStep::Elements) {
                        self.numbered_last_hwnd = cur_hwnd_id;
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
                if self.switch_step == SwitchAccessStep::Elements && !user_navigating && !self.is_scan_in_progress && self.numbered_last_refresh.elapsed() >= Duration::from_millis(3500) {
                    if let Some(wid) = self.switch_active_window_id {
                        self.refresh_numbered_targets_for_window(wid);
                    }
                }
            }
        }

        // Gestion du Switch Access & Survol : défilement séquentiel périodique et validation sur Entrée / Espace
        if self.switch_access_mode && !is_typing {
            let is_shift = ctx.input(|i| i.modifiers.shift);
            let (egui_enter, egui_space, egui_tab, egui_nav_next, egui_nav_prev) = ctx.input(|i| {
                let mut enter = false;
                let mut space = false;
                let mut tab = false;
                let mut next = false;
                let mut prev = false;
                for ev in &i.events {
                    match ev {
                        egui::Event::Key { key, pressed: true, repeat: false, .. } => match key {
                            egui::Key::Enter => enter = true,
                            egui::Key::Space => space = true,
                            egui::Key::Tab => tab = true,
                            egui::Key::ArrowRight | egui::Key::ArrowDown => next = true,
                            egui::Key::ArrowLeft | egui::Key::ArrowUp => prev = true,
                            _ => {}
                        },
                        egui::Event::Text(ref text) => {
                            if text == "\n" || text == "\r" {
                                enter = true;
                            } else if text == " " {
                                space = true;
                            }
                        }
                        _ => {}
                    }
                }
                (enter, space, tab, next || (tab && !is_shift), prev || (tab && is_shift))
            });

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
            // N'ingérer les chiffres pour le ciblage que si l'utilisateur n'est pas dans un champ texte
            if !is_typing && !hook_digits.is_empty() {
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
                            self.switch_active_window_id = Some(cur_win.id);
                            self.numbered_last_hwnd = Some(cur_win.id);
                            self.numbered_targets.clear();
                            clear_screen_numbered_badges();

                            #[cfg(windows)]
                            {
                                let hwnd = windows::Win32::Foundation::HWND(cur_win.id as _);
                                focus_and_isolate_window(hwnd);
                                let mut r = windows::Win32::Foundation::RECT::default();
                                if unsafe { windows::Win32::UI::WindowsAndMessaging::GetWindowRect(hwnd, &mut r).is_ok() } {
                                    draw_screen_orange_border([r.left, r.top, r.right, r.bottom]);
                                } else {
                                    draw_screen_orange_border(cur_win.rect);
                                }
                                set_global_cursor_pos(cur_win.click_x, cur_win.click_y);
                                self.hover_last_cursor_pos = Some((cur_win.click_x, cur_win.click_y));
                            }
                            #[cfg(not(windows))]
                            {
                                focus_and_isolate_window(&cur_win.title);
                                set_global_cursor_pos(cur_win.click_x, cur_win.click_y);
                                self.hover_last_cursor_pos = Some((cur_win.click_x, cur_win.click_y));
                            }

                            // Pré-scan en tâche de fond de la nouvelle fenêtre sélectionnée
                            self.refresh_numbered_targets_for_window(cur_win.id);

                            let _ = self.tts_sender.send(TtsCommand::Stop);
                            let vocal_msg = format!("{}. {}", self.switch_focus_index + 1, cur_win.title);
                            let _ = self.tts_sender.send(TtsCommand::Speak(vocal_msg));
                            self.hover_last_announced_target = Some(cur_win.id as usize);
                            ctx.request_repaint();
                        }

                        if switch_input_hit {
                            self.last_switch_input = Instant::now();
                            self.switch_auto_scroll = false;
                            self.hover_controls_speech_enabled = true;
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
                                crate::automation::IS_EXTERNAL_TEXT_INPUT_FOCUSED.store(true, Ordering::SeqCst);
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
                    Some((lx, ly)) => (lx - cur_x).abs() > 15 || (ly - cur_y).abs() > 15,
                    None => false,
                };

                let is_recent_switch_nav = self.last_switch_input.elapsed() < Duration::from_millis(800);

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
                                    focus_and_isolate_window(hwnd);
                                    let mut r = windows::Win32::Foundation::RECT::default();
                                    if unsafe { windows::Win32::UI::WindowsAndMessaging::GetWindowRect(hwnd, &mut r).is_ok() } {
                                        draw_screen_orange_border([r.left, r.top, r.right, r.bottom]);
                                    } else {
                                        draw_screen_orange_border(hovered_win.rect);
                                    }
                                }
                                #[cfg(not(windows))]
                                {
                                    focus_and_isolate_window(&hovered_win.title);
                                }
                                if self.switch_step == SwitchAccessStep::Elements {
                                    // Rafraîchissement direct de l'OCR et des badges de la nouvelle fenêtre survolée
                                    self.refresh_numbered_targets_for_window(hovered_win.id);
                                }
                            }
                        }
                    }
                } else {
                    let hover_still_min = if self.active_disability_presets.contains(&DisabilityPreset::Tremor) {
                        Duration::from_millis(450)
                    } else if self.dwell_state.is_enabled() {
                        self.dwell_state.effective_activation_delay().clamp(Duration::from_millis(350), Duration::from_millis(800))
                    } else {
                        Duration::from_millis(350)
                    };
                    if !is_over_overlay
                        && self.hover_physical_mouse_moved
                        && !is_recent_switch_nav
                        && self.hover_cursor_still_since.elapsed() >= hover_still_min {
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
                                        self.switch_active_window_id = Some(win_id);
                                        #[cfg(windows)]
                                        {
                                            let hwnd = windows::Win32::Foundation::HWND(win_id as _);
                                            focus_and_isolate_window(hwnd);
                                            let mut r = windows::Win32::Foundation::RECT::default();
                                            if unsafe { windows::Win32::UI::WindowsAndMessaging::GetWindowRect(hwnd, &mut r).is_ok() } {
                                                draw_screen_orange_border([r.left, r.top, r.right, r.bottom]);
                                            } else {
                                                draw_screen_orange_border(win_rect);
                                            }
                                        }
                                        #[cfg(not(windows))]
                                        {
                                            focus_and_isolate_window(&win_title);
                                        }
                                        let _ = self.tts_sender.send(TtsCommand::Stop);
                                        let _ = self.tts_sender.send(TtsCommand::Speak(format!("{}. {}", w_idx + 1, win_title)));
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

        // Raccourci clavier 'R' pour basculer le micro via Event::Text & Event::Key
        let r_pressed = !self.is_hidden && !is_typing && ctx.input(|i| {
            i.events.iter().any(|ev| match ev {
                egui::Event::Key { key: egui::Key::R, pressed: true, .. } => true,
                egui::Event::Text(t) => t.eq_ignore_ascii_case("r"),
                _ => false,
            })
        });
        if r_pressed {
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

        if !crate::scheduler::get_active_scheduled_tasks().is_empty() {
            ctx.request_repaint_after(Duration::from_millis(1000));
        }

        if self.is_hidden {
            self.sync_switch_access_overlays();
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
                        let is_assist_key_hit = make_widget_accessible_and_activated(ui, &tab_assist, theme.accent);
                        let tab_assist_id = ui.make_persistent_id("dwell_tab_assistance");
                        let assist_activated = if !is_assist_active {
                            apply_dwell_to_response(ui, &mut self.dwell_state, tab_assist_id, &tab_assist)
                        } else {
                            tab_assist.clicked()
                        };
                        if assist_activated || is_assist_key_hit {
                            if self.active_tab != ActiveTab::Assistance {
                                self.stop_speaking();
                            }
                            self.active_tab = ActiveTab::Assistance;
                        }
                        let chat_title = if self.twitch_messages.is_empty() {
                            t!("ui.twitch_chat_tab").to_string()
                        } else {
                            t!("ui.twitch_chat_count", count = self.twitch_messages.len()).to_string()
                        };
                        let is_chat_active = self.active_tab == ActiveTab::Chat;
                        let tab_chat = ui.selectable_label(is_chat_active, chat_title);
                        let is_chat_key_hit = make_widget_accessible_and_activated(ui, &tab_chat, theme.accent);
                        let tab_chat_id = ui.make_persistent_id("dwell_tab_twitch_chat");
                        let chat_activated = if !is_chat_active {
                            apply_dwell_to_response(ui, &mut self.dwell_state, tab_chat_id, &tab_chat)
                        } else {
                            tab_chat.clicked()
                        };
                        if chat_activated || is_chat_key_hit {
                            if self.active_tab != ActiveTab::Chat {
                                self.stop_speaking();
                            }
                            self.active_tab = ActiveTab::Chat;
                            if self.twitch_search_results.is_empty() {
                                let _ = self.command_sender.send(AgentCommand::SearchTwitch(String::new()));
                            }
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
                            ).on_hover_text(t!("ui.btn_close"));
                            close_btn.widget_info(|| {
                                egui::WidgetInfo::labeled(egui::WidgetType::Button, ui.is_enabled(), t!("ui.btn_close").to_string())
                            });
                            let close_btn_hit = make_widget_accessible_and_activated(ui, &close_btn, theme.accent);
                            if close_btn_hit {
                                ctx.send_viewport_cmd(egui::ViewportCommand::Close);
                            }

                            let max_icon = if is_maximized { "⧉" } else { "□" };
                            let max_label = if is_maximized { t!("ui.btn_restore").to_string() } else { t!("ui.btn_maximize").to_string() };
                            let max_btn = ui.add(
                                egui::Button::new(
                                    egui::RichText::new(max_icon)
                                        .size(10.5)
                                        .color(egui::Color32::from_rgb(200, 215, 230)),
                                )
                                .min_size(egui::vec2(20.0, 18.0))
                                .corner_radius(egui::CornerRadius::same(4)),
                            ).on_hover_text(&max_label);
                            max_btn.widget_info(|| {
                                egui::WidgetInfo::labeled(egui::WidgetType::Button, ui.is_enabled(), max_label.clone())
                            });
                            let max_btn_hit = make_widget_accessible_and_activated(ui, &max_btn, theme.accent);
                            if max_btn_hit {
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
                            ).on_hover_text(t!("ui.btn_minimize"));
                            min_btn.widget_info(|| {
                                egui::WidgetInfo::labeled(egui::WidgetType::Button, ui.is_enabled(), t!("ui.btn_minimize").to_string())
                            });
                            let min_btn_hit = make_widget_accessible_and_activated(ui, &min_btn, theme.accent);
                            if min_btn_hit {
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
                            let avatar_resp = ui.interact(avatar_rect, ui.id().with("avatar_interact"), egui::Sense::click());
                            let layer_id = ui.layer_id();
                            let avatar_label = if self.status == AgentStatus::Speaking {
                                t!("ui.avatar_mute_tooltip").to_string()
                            } else {
                                format!("Avatar Libertide ({:?})", self.status)
                            };
                            avatar_resp.widget_info(|| {
                                egui::WidgetInfo::labeled(egui::WidgetType::Button, ui.is_enabled(), avatar_label.clone())
                            });
                            ui.memory_mut(|m| m.interested_in_focus(avatar_resp.id, layer_id));
                            if avatar_resp.has_focus() {
                                ui.painter().rect_stroke(
                                    avatar_rect.expand(3.0),
                                    egui::CornerRadius::same(16),
                                    egui::Stroke::new(2.0, theme.accent),
                                    egui::StrokeKind::Outside,
                                );
                            }
                            let is_avatar_key_hit = avatar_resp.has_focus() && ui.input(|i| {
                                i.events.iter().any(|ev| match ev {
                                    egui::Event::Key { key, pressed: true, repeat: false, .. } => *key == egui::Key::Space || *key == egui::Key::Enter,
                                    egui::Event::Text(t) => t == " " || t == "\n" || t == "\r",
                                    _ => false,
                                })
                            });
                            if self.status == AgentStatus::Speaking {
                                if avatar_resp.hovered() {
                                    ui.ctx().set_cursor_icon(egui::CursorIcon::PointingHand);
                                }
                                if avatar_resp.on_hover_text(t!("ui.avatar_mute_tooltip")).clicked() || is_avatar_key_hit {
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
                                                let is_chip_hit = make_widget_accessible_and_activated(ui, &resp, theme.accent);
                                                if apply_dwell_to_response(ui, &mut self.dwell_state, sug_dwell_id, &resp) || is_chip_hit {
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
                                        let input_id = egui::Id::NULL.with("prompt_input_text");
                                        let is_input_focused = ctx.memory(|m| m.has_focus(input_id));
                                        let is_modal_open = self.show_api_key_modal;
                                        let is_other_typing = ctx.egui_wants_keyboard_input() && !is_input_focused;

                                        // Dépouillement des événements clavier de la file egui (support des overriders / remappeurs)
                                        let mut enter_from_events = false;
                                        let mut tab_from_events = false;
                                        if is_input_focused && !is_modal_open && !is_other_typing {
                                            ctx.input(|i| {
                                                for ev in &i.events {
                                                    match ev {
                                                        egui::Event::Key { key: egui::Key::Enter, pressed: true, modifiers, .. } if !modifiers.shift => {
                                                            enter_from_events = true;
                                                        }
                                                        egui::Event::Key { key: egui::Key::Tab, pressed: true, .. } => {
                                                            tab_from_events = true;
                                                        }
                                                        egui::Event::Text(ref text) if text == "\n" || text == "\r" => {
                                                            enter_from_events = true;
                                                        }
                                                        _ => {}
                                                    }
                                                }
                                            });
                                        }

                                        // Autocomplétion de la première suggestion via Tab
                                        if tab_from_events {
                                            if let Some(top_sug) = self.input_suggestions.first() {
                                                autocomplete_input_to = Some(t!("ui.open_app_cmd", app = top_sug).to_string());
                                                ctx.input_mut(|i| i.consume_key(egui::Modifiers::NONE, egui::Key::Tab));
                                            }
                                        }

                                        // Interception de la touche Entrée avant TextEdit
                                        let enter_pressed_before = enter_from_events;

                                        if enter_pressed_before {
                                            enter_hit = true;
                                            ctx.input_mut(|i| {
                                                i.consume_key(egui::Modifiers::NONE, egui::Key::Enter);
                                                i.events.retain(|e| !matches!(e, egui::Event::Text(ref t) if t == "\n" || t == "\r"));
                                            });
                                            while self.input_text.ends_with('\n') || self.input_text.ends_with('\r') {
                                                self.input_text.pop();
                                            }
                                        }

                                        let paste_hit = ctx.input(|i| {
                                            i.events.iter().any(|e| match e {
                                                egui::Event::Paste(_) => true,
                                                egui::Event::Key { key: egui::Key::V, pressed: true, modifiers, .. } if modifiers.ctrl || modifiers.command => true,
                                                egui::Event::Key { key: egui::Key::Insert, pressed: true, modifiers, .. } if modifiers.shift => true,
                                                _ => false,
                                            })
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
                                        let hint = if self.is_recording {
                                            t!("ui.prompt_hint_listening").to_string()
                                        } else {
                                            t!("ui.prompt_hint_idle").to_string()
                                        };
                                        let response = render_accessible_input(
                                            ui,
                                            input_id,
                                            &mut self.input_text,
                                            &hint,
                                            true,
                                            Some(edit_width),
                                            Some(54.0),
                                            false,
                                            &theme,
                                        );

                                        if response.has_focus() {
                                            crate::automation::IS_USER_TYPING.store(true, Ordering::Relaxed);
                                        }

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
                                            i.events.iter().any(|e| match e {
                                                egui::Event::Copy => true,
                                                egui::Event::Key { key: egui::Key::C, pressed: true, modifiers, .. } if modifiers.ctrl || modifiers.command => true,
                                                _ => false,
                                            })
                                        });
                                        if (response.has_focus() || is_input_focused) && copy_hit && !self.input_text.is_empty() {
                                            set_clipboard_text(&self.input_text);
                                            ctx.copy_text(self.input_text.clone());
                                            play_earcon(Earcon::Success);
                                        }

                                        let cut_hit = ctx.input(|i| {
                                            i.events.iter().any(|e| match e {
                                                egui::Event::Cut => true,
                                                egui::Event::Key { key: egui::Key::X, pressed: true, modifiers, .. } if modifiers.ctrl || modifiers.command => true,
                                                _ => false,
                                            })
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

                                        // Validation immédiate sur appui Entrée dans le TextEdit sans Shift
                                        let shift_down = ctx.input(|i| i.modifiers.shift);
                                        if response.has_focus() && !shift_down {
                                            if self.input_text.ends_with('\n') || self.input_text.ends_with('\r') {
                                                while self.input_text.ends_with('\n') || self.input_text.ends_with('\r') {
                                                    self.input_text.pop();
                                                }
                                                enter_hit = true;
                                            }
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
                                            self.auto_scroll_to_bottom = true;
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

                                        if !enter_hit && !is_modal_open && !is_other_typing {
                                            enter_hit = enter_from_events || (response.has_focus() && ctx.input(|i| {
                                                i.events.iter().any(|e| match e {
                                                    egui::Event::Key { key: egui::Key::Enter, pressed: true, modifiers, .. } if !modifiers.shift => true,
                                                    egui::Event::Text(t) if t == "\n" || t == "\r" => true,
                                                    _ => false,
                                                })
                                            }));
                                            if enter_hit {
                                                ctx.input_mut(|i| i.consume_key(egui::Modifiers::NONE, egui::Key::Enter));
                                                while self.input_text.ends_with('\n') || self.input_text.ends_with('\r') {
                                                    self.input_text.pop();
                                                }
                                            }
                                        }
                                    });

                                    ui.add_space(3.0);

                                    // 2e rangée : 6 boutons d'action ergonomiques (WCAG 2.2 AAA, Clic & Dwell 800 ms)
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
                                        let is_analyze_key = analyze_resp.has_focus() && ui.input(|i| {
                                            i.events.iter().any(|ev| match ev {
                                                egui::Event::Key { key, pressed: true, repeat: false, .. } => *key == egui::Key::Space || *key == egui::Key::Enter,
                                                egui::Event::Text(t) => t == " " || t == "\n" || t == "\r",
                                                _ => false,
                                            })
                                        });
                                        let analyze_btn_hit = analyze_resp.clicked() || analyze_dwell || is_analyze_key;
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
                                            self.auto_scroll_to_bottom = true;
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
                        self.chat_history.push(ChatEntry {
                            role: ChatRole::Agent,
                            text: feedback.to_string(),
                            timestamp: current_time_str(),
                            quick_suggestions: Vec::new(),
                            screen_size_kb: None,
                        });
                        let _ = self.tts_sender.send(TtsCommand::Speak(feedback.to_string()));
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
                                        let is_mic_key = mic_resp.has_focus() && ui.input(|i| {
                                            i.events.iter().any(|ev| match ev {
                                                egui::Event::Key { key, pressed: true, repeat: false, .. } => *key == egui::Key::Space || *key == egui::Key::Enter,
                                                egui::Event::Text(t) => t == " " || t == "\n" || t == "\r",
                                                _ => false,
                                            })
                                        });
                                        if mic_resp.clicked() || mic_dwell || is_mic_key {
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
                                        let is_dict_key = dict_resp.has_focus() && ui.input(|i| {
                                            i.events.iter().any(|ev| match ev {
                                                egui::Event::Key { key, pressed: true, repeat: false, .. } => *key == egui::Key::Space || *key == egui::Key::Enter,
                                                egui::Event::Text(t) => t == " " || t == "\n" || t == "\r",
                                                _ => false,
                                            })
                                        });
                                        if dict_resp.clicked() || dict_dwell || is_dict_key {
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
                                        let is_switch_key = switch_resp.has_focus() && ui.input(|i| {
                                            i.events.iter().any(|ev| match ev {
                                                egui::Event::Key { key, pressed: true, repeat: false, .. } => *key == egui::Key::Space || *key == egui::Key::Enter,
                                                egui::Event::Text(t) => t == " " || t == "\n" || t == "\r",
                                                _ => false,
                                            })
                                        });
                                        if switch_resp.clicked() || switch_dwell || is_switch_key {
                                            self.toggle_switch_access_mode(None);
                                        }

                                        // 5. Bouton Réglages
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
                                        let is_settings_key = settings_resp.has_focus() && ui.input(|i| {
                                            i.events.iter().any(|ev| match ev {
                                                egui::Event::Key { key, pressed: true, repeat: false, .. } => *key == egui::Key::Space || *key == egui::Key::Enter,
                                                egui::Event::Text(t) => t == " " || t == "\n" || t == "\r",
                                                _ => false,
                                            })
                                        });
                                        if settings_resp.clicked() || settings_dwell || is_settings_key {
                                            self.settings_tab = SettingsTab::LlmProvider;
                                            self.show_api_key_modal = true;
                                            self.sync_modal_inputs_for_provider(self.llm_config.provider);
                                            self.api_key_error = None;
                                            self.stop_speaking();
                                            if self.is_recording {
                                                self.stop_recording();
                                            }
                                        }

                                        // 6. Bouton Effacer
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
                                        let is_clear_key = clear_resp.has_focus() && ui.input(|i| {
                                            i.events.iter().any(|ev| match ev {
                                                egui::Event::Key { key, pressed: true, repeat: false, .. } => *key == egui::Key::Space || *key == egui::Key::Enter,
                                                egui::Event::Text(t) => t == " " || t == "\n" || t == "\r",
                                                _ => false,
                                            })
                                        });
                                        if clear_resp.clicked() || clear_dwell || is_clear_key {
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
                    ui.set_clip_rect(messages_rect);
                    match self.active_tab {
                        ActiveTab::Assistance => {
                            // Encadré accessible des rappels & minuteurs en cours
                            if let Some(reminder_feedback) = draw_active_reminders_box(ui, &theme, &mut self.dwell_state) {
                                self.chat_history.push(ChatEntry {
                                    role: ChatRole::Agent,
                                    text: reminder_feedback.clone(),
                                    timestamp: current_time_str(),
                                    quick_suggestions: Vec::new(),
                                    screen_size_kb: None,
                                });
                                let _ = self.tts_sender.send(TtsCommand::Speak(reminder_feedback));
                            }
                            ui.add_space(2.0);

                            let max_scroll_h = (messages_rect.height()).max(30.0);
                            let scroll_out = egui::ScrollArea::vertical()
                                .id_salt("assistance_chat_scroll")
                                .stick_to_bottom(true)
                                .max_height(max_scroll_h)
                                .auto_shrink([false, false])
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

                                            ui.horizontal(|ui| {
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
                                            interact_resp.widget_info(|| {
                                                egui::WidgetInfo::labeled(
                                                    egui::WidgetType::Label,
                                                    ui.is_enabled(),
                                                    format!("{}: {}", label_name, entry.text),
                                                )
                                            });

                                            let msg_dwell_id = ui.make_persistent_id(format!("dwell_chat_msg_{idx}"));
                                            let dwell_triggered = apply_dwell_to_response(ui, dwell_state, msg_dwell_id, &interact_resp);
                                            if dwell_triggered {
                                                dwell_state.lock_id(msg_dwell_id, Duration::from_secs(6));
                                                let speech = if let Some(sug_speech) = format_suggestions_for_speech(&entry.quick_suggestions) {
                                                    format!("{}\n\n{}", entry.text, sug_speech)
                                                } else {
                                                    entry.text.clone()
                                                };
                                                speak_message = Some(speech);
                                            }

                                            if interact_resp.clicked() {
                                                ui.memory_mut(|m| m.request_focus(msg_id));
                                            }
                                            let effective_focused = is_msg_focused || interact_resp.has_focus();

                                            // Copie au clavier Ctrl+C dès que le message a le focus clavier (ou au survol sans focus actif ailleurs)
                                            let copy_msg_hit = ctx.input(|i| {
                                                i.events.iter().any(|e| match e {
                                                    egui::Event::Copy => true,
                                                    egui::Event::Key { key: egui::Key::C, pressed: true, modifiers, .. } if modifiers.ctrl || modifiers.command => true,
                                                    _ => false,
                                                })
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
                                                            let is_quick_hit = make_widget_accessible_and_activated(ui, &resp, theme.accent);
                                                            if apply_dwell_to_response(ui, dwell_state, sug_dwell_id, &resp) || is_quick_hit {
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

                                    if self.auto_scroll_to_bottom || self.is_streaming_narration || is_speaking {
                                        ui.scroll_to_cursor(Some(egui::Align::BOTTOM));
                                    }
                                });
                            if self.auto_scroll_to_bottom {
                                self.auto_scroll_to_bottom = false;
                            }
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
                                    let ch_link = ui.add(
                                        egui::Label::new(
                                            egui::RichText::new(format!("#{}", self.current_twitch_channel))
                                                .size(11.0)
                                                .color(egui::Color32::from_rgb(0, 195, 255))
                                                .underline(),
                                        )
                                        .sense(egui::Sense::click()),
                                    ).on_hover_cursor(egui::CursorIcon::PointingHand);
                                    if ch_link.clicked() {
                                        open_browser_smart(&format!("https://twitch.tv/{}", self.current_twitch_channel));
                                    }
                                    ui.separator();
                                }

                                let search_id = egui::Id::NULL.with("twitch_search_query_input");
                                let search_hint = t!("ui.twitch_search_hint").to_string();
                                let search_edit = render_accessible_input(
                                    ui,
                                    search_id,
                                    &mut self.twitch_search_query,
                                    &search_hint,
                                    false,
                                    Some(110.0),
                                    Some(18.0),
                                    false,
                                    &theme,
                                );
                                let enter_pressed = search_edit.lost_focus() && ui.input(|i| {
                                    i.events.iter().any(|e| match e {
                                        egui::Event::Key { key: egui::Key::Enter, pressed: true, .. } => true,
                                        egui::Event::Text(t) if t == "\n" || t == "\r" => true,
                                        _ => false,
                                    })
                                });
                                if ui.small_button("🔍").clicked() || enter_pressed {
                                    let _ = self.command_sender.send(AgentCommand::SearchTwitch(
                                        self.twitch_search_query.trim().to_string(),
                                    ));
                                }
                                if !self.twitch_search_results.is_empty() {
                                    if ui.small_button("✕").on_hover_text(t!("ui.btn_close")).clicked() {
                                        self.twitch_search_results.clear();
                                    }
                                }
                            });

                            let mut search_viewport_h = 0.0;
                            if !self.twitch_search_results.is_empty() {
                                ui.add_space(3.0);
                                let live_count = self.twitch_search_results.iter().filter(|c| c.is_live).count();
                                let total_count = self.twitch_search_results.len();

                                draw_twitch_filter_bar(ui, &mut self.twitch_filter, live_count, total_count, &theme);
                                ui.add_space(2.0);

                                let max_search_h = if self.twitch_messages.is_empty() {
                                    (messages_rect.height() - 65.0).max(60.0)
                                } else {
                                    ((messages_rect.height() - 75.0) * 0.45).clamp(55.0, 130.0)
                                };

                                let mut selected_channel = None;
                                let search_scroll = egui::ScrollArea::vertical()
                                    .id_salt("twitch_search_results_scroll")
                                    .max_height(max_search_h)
                                    .auto_shrink([false, false])
                                    .show(ui, |ui| {
                                        let btn_w = 24.0;
                                        let spacing = 4.0;
                                        let total_row_w = ui.available_width();
                                        let item_w = (total_row_w - btn_w - spacing).max(80.0);

                                        for ch in self.twitch_search_results.iter().filter(|c| self.twitch_filter.matches(c)) {
                                            ui.horizontal(|ui| {
                                                ui.spacing_mut().item_spacing.x = spacing;
                                                let resp = draw_twitch_channel_search_item(ui, ch, &theme, item_w);
                                                if resp.clicked() {
                                                    selected_channel = Some(ch.login.clone());
                                                }
                                                if ui.add_sized([btn_w, 28.0], egui::Button::new("↗")).on_hover_text(t!("ui.twitch_open_tooltip")).clicked() {
                                                    open_browser_smart(&format!("https://twitch.tv/{}", ch.login));
                                                }
                                            });
                                            ui.add_space(1.0);
                                        }
                                    });

                                search_viewport_h = search_scroll.content_size.y.min(max_search_h) + 36.0;

                                if let Some(ch) = selected_channel {
                                    self.current_twitch_channel = ch.clone();
                                    let _ = self.twitch_channel_sender.send(ch);
                                    self.twitch_messages.clear();
                                    self.twitch_search_results.clear();
                                }
                            }
                            ui.separator();

                            // En-tête du canal avec bouton pour rejoindre sur Twitch
                            let mut channel_header_h = 0.0;
                            if !self.current_twitch_channel.is_empty() {
                                ui.horizontal(|ui| {
                                    ui.label(
                                        egui::RichText::new(format!("💬 Chat #{}", self.current_twitch_channel))
                                            .size(11.0)
                                            .strong()
                                            .color(egui::Color32::from_rgb(169, 112, 255)),
                                    );
                                    ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                                        let join_btn = ui.add(
                                            egui::Button::new(
                                                egui::RichText::new("🌐 Rejoindre sur Twitch")
                                                    .size(10.5)
                                                    .strong()
                                                    .color(egui::Color32::from_rgb(220, 245, 255)),
                                            )
                                            .fill(egui::Color32::from_rgba_unmultiplied(145, 70, 255, 60))
                                            .stroke(egui::Stroke::new(1.0, egui::Color32::from_rgb(145, 70, 255)))
                                            .corner_radius(egui::CornerRadius::same(6)),
                                        ).on_hover_text(format!("Ouvrir le stream de {} sur Twitch dans le navigateur", self.current_twitch_channel));
                                        if join_btn.clicked() {
                                            open_browser_smart(&format!("https://twitch.tv/{}", self.current_twitch_channel));
                                        }
                                    });
                                });
                                ui.add_space(2.0);
                                channel_header_h = 24.0;
                            }

                            let max_chat_scroll_h = (messages_rect.height() - 28.0 - search_viewport_h - channel_header_h).max(30.0);
                            let scroll_out = egui::ScrollArea::vertical()
                                .id_salt("twitch_chat_scroll")
                                .stick_to_bottom(true)
                                .max_height(max_chat_scroll_h)
                                .auto_shrink([false, false])
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
                                            resp.widget_info(|| {
                                                egui::WidgetInfo::labeled(
                                                    egui::WidgetType::Label,
                                                    ui.is_enabled(),
                                                    format!("{}: {}", msg.author, msg.text),
                                                )
                                            });

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
                                                i.events.iter().any(|e| match e {
                                                    egui::Event::Copy => true,
                                                    egui::Event::Key { key: egui::Key::C, pressed: true, modifiers, .. } if modifiers.ctrl || modifiers.command => true,
                                                    _ => false,
                                                })
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

                                    if self.auto_scroll_to_bottom {
                                        ui.scroll_to_cursor(Some(egui::Align::BOTTOM));
                                    }
                                    }
                                });
                            if self.auto_scroll_to_bottom {
                                self.auto_scroll_to_bottom = false;
                            }
                            computed_chat_h = (scroll_out.content_size.y + search_viewport_h + 72.0).clamp(min_chat_h, max_chat_h);
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
                self.sync_switch_access_overlays();


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
                    self.render_settings_modal(&ctx, &theme, win_w, active_win_h);
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

    fn on_exit(&mut self, _gl: Option<&eframe::glow::Context>) {
        #[cfg(windows)]
        {
            crate::automation::uninstall_switch_keyboard_hook();
        }
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

fn spawn_event_wake_watcher(
    incoming_rx: Receiver<AgentEvent>,
    outgoing_tx: StdSender<AgentEvent>,
    egui_ctx: egui::Context,
) {
    std::thread::Builder::new()
        .name("event_wake_watcher".to_string())
        .spawn(move || {
            while let Ok(event) = incoming_rx.recv() {
                let should_wake = matches!(
                    event,
                    AgentEvent::VoicePromptReady(_)
                        | AgentEvent::ReminderTriggered { .. }
                );

                if should_wake {
                    #[cfg(windows)]
                    unsafe {
                        use windows::core::w;
                        use windows::Win32::UI::WindowsAndMessaging::{FindWindowW, ShowWindow, SetForegroundWindow, SW_RESTORE};
                        let overlay = FindWindowW(None, w!("Libertide overlay"))
                            .unwrap_or(windows::Win32::Foundation::HWND(std::ptr::null_mut()));
                        if !overlay.0.is_null() {
                            let _ = ShowWindow(overlay, SW_RESTORE);
                            let _ = SetForegroundWindow(overlay);
                        }
                    }

                    egui_ctx.send_viewport_cmd(egui::ViewportCommand::Minimized(false));
                    egui_ctx.send_viewport_cmd(egui::ViewportCommand::Focus);
                    egui_ctx.request_repaint();
                }

                if outgoing_tx.send(event).is_err() {
                    break;
                }
            }
        })
        .expect("Échec lancement worker event_wake_watcher");
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
        .with_min_inner_size([380.0, 264.0])
        .with_inner_size([560.0, 280.0]);

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
            let (app_event_tx, app_event_rx) = channel::<AgentEvent>();
            spawn_event_wake_watcher(event_rx, app_event_tx, cc.egui_ctx.clone());

            Ok(Box::new(OverlayApp::new(
                cc,
                event_tx,
                app_event_rx,
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

#[cfg(windows)]
pub fn disable_crt_assertion_dialogs() {
    extern "C" {
        fn _set_invalid_parameter_handler(
            handler: Option<unsafe extern "C" fn(*const u16, *const u16, *const u16, u32, usize)>,
        ) -> Option<unsafe extern "C" fn(*const u16, *const u16, *const u16, u32, usize)>;
    }

    unsafe extern "C" fn silent_invalid_parameter(
        _expression: *const u16,
        _function: *const u16,
        _file: *const u16,
        _line: u32,
        _reserved: usize,
    ) {
        // Intercepte silencieusement les assertions CRT (ex: _osfile(fh) & FOPEN dans lowio\read.cpp)
    }

    unsafe {
        let _ = _set_invalid_parameter_handler(Some(silent_invalid_parameter));
    }
}

fn main() -> eframe::Result<()> {
    #[cfg(windows)]
    disable_crt_assertion_dialogs();
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
    crate::scheduler::spawn_proactive_companion_loop(cmd_tx.clone());
    spawn_agent_backend(cmd_rx, cmd_tx.clone(), event_tx.clone(), tts_tx.clone(), llm_config.clone());

    if llm_config.provider == LlmProvider::Local {
        // Légère temporisation (500 ms) pour laisser Whisper initialiser ses handles
        // et éviter la collision concurrente sur la table des descripteurs CRT de bas niveau
        std::thread::sleep(Duration::from_millis(500));
        std::thread::spawn(|| {
            println!("[Main] Démarrage du thread de préchargement LLM local...");
            local_llm::init_local_llm();
            println!("[Main] Thread de préchargement LLM local terminé.");
        });
    }

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