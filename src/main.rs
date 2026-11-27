#![cfg_attr(not(debug_assertions), windows_subsystem = "windows")]

#[macro_use]
extern crate rust_i18n;
i18n!("locales", fallback = "en");

mod action_history;
mod autostart;
mod audio;
mod audio_player;
mod automation;
mod clipboard;
mod floating_window;
mod fs_util;
mod html_util;
mod keyboard_hook;
mod pattern;
mod screen_summary;
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
mod instrument;
mod eyetracking;
mod virtual_keyboard;
mod app_ui;
mod home_automation;

pub use eyetracking::*;
pub use agent_backend::*;
pub use locale::*;
pub use action_history::*;
pub use home_automation::*;

use audio::spawn_audio_worker;
use floating_window::{update_open_floating_cards, FloatingCard, FloatingContentType};
use automation::*;
use fs_util::*;
use theme::{AppTheme, SettingsTab};
use tts::spawn_tts_worker;
use twitch::spawn_twitch_worker;
use window_util::*;
use keyboard_hook::*;
use eframe::egui;
pub use overlay::*;
use types::*;
use std::collections::VecDeque;
use std::sync::Arc;
use std::sync::atomic::Ordering;
use std::sync::mpsc::{channel, Receiver, Sender as StdSender};
use tokio::sync::mpsc::{unbounded_channel, UnboundedSender};
use std::time::{Duration, Instant};

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
    mic_manually_disabled: bool,
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
    enter_was_physically_down: bool,
    last_eye_click_time: Instant,
    last_trackball_input: Instant,

    // Contrôle par le regard sans matériel (Eye-Tracking webcam standard)
    eye_tracker: eyetracking::EyeTrackingController,

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
    dictation_origin_target: Option<NumberedElementTarget>,

    // Mode survol : Détection du pointeur, anti-rebond & hiérarchie vocale
    hover_last_cursor_pos: Option<(i32, i32)>,
    hover_cursor_still_since: Instant,
    hover_pending_speech: Option<String>,
    hover_speech_deadline: Option<Instant>,
    hover_last_announced_target: Option<usize>,
    hover_last_announced_zone: Option<String>,
    hover_controls_speech_enabled: bool,
    hover_physical_mouse_moved: bool,
    hover_start_time: Instant,
    hover_had_lost_focus: bool,

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

    active_download: Option<ActiveDownloadState>,

    // Suggestions d'applications en direct (PATH & Levenshtein)
    input_suggestions: Vec<String>,
    last_suggestion_query: String,
    attached_file_path: Option<std::path::PathBuf>,
    file_picker_rx: Option<std::sync::mpsc::Receiver<Option<std::path::PathBuf>>>,

    // Macros personnalisées ('raccourci' => texte complet)
    macros: Vec<(String, String)>,
    macro_shortcut_input: String,
    macro_text_input: String,
    macro_error: Option<String>,

    // Domotique locale (Home Assistant / Matter)
    hass_url_input: String,
    hass_token_input: String,
    knx_gateway_input: String,
    hass_status_feedback: Option<String>,

    // Options d'accessibilité et presets cumulatifs
    active_disability_presets: std::collections::BTreeSet<DisabilityPreset>,
    auto_read_agent_messages: bool,
    auto_read_suggestions: bool,
    earcons_enabled: bool,

    // Périphériques matériels (Pointage et Microphones)
    detected_microphones: Vec<String>,
    selected_microphone: String,
    detected_pointing_devices: Vec<crate::devices::PointingDevice>,
    pending_twitch_speech: VecDeque<String>,
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

        let ctx_clone = cc.egui_ctx.clone();
        crate::automation::set_repaint_callback(move || {
            ctx_clone.request_repaint();
        });

        let macros = load_macros();
        load_keybindings();
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

        let hass_url_input = crate::home_automation::get_home_assistant_url();
        let hass_token_input = crate::home_automation::get_home_assistant_token();
        let knx_gateway_input = crate::home_automation::get_knx_gateway_ip();

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

        let switch_auto_scroll = std::env::var("SWITCH_AUTO_SCROLL")
            .ok()
            .or_else(|| read_env_var_from_file(".env", "SWITCH_AUTO_SCROLL"))
            .map(|s| s.trim().eq_ignore_ascii_case("true") || s.trim() == "1")
            .unwrap_or(false)
            || active_disability_presets.contains(&DisabilityPreset::MotorSwitch);

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

        let beacon_map_enabled = std::env::var("AUDITORY_BEACON_MAP")
            .ok()
            .or_else(|| read_env_var_from_file(".env", "AUDITORY_BEACON_MAP"))
            .map(|s| !s.trim().eq_ignore_ascii_case("false") && s.trim() != "0")
            .unwrap_or(true);
        crate::audio_player::set_auditory_beacon_map_enabled(beacon_map_enabled);

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
            mic_manually_disabled: false,
            whisper_ready: false,
            continuous_mode: true,
            dictation_mode: false,
            zoom_factor: initial_zoom,
            app_language,
            voice_gender: initial_voice,
            voice_clone_url_input,
            voice_reference_wav_input,
            voice_reference_text_input,
            voice_test_text_input: t!("voice.test_default_phrase").to_string(),
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
            voice_guide_phrase: t!("voice.guide_default_phrase").to_string(),
            voice_guide_generating: false,
            voice_guide_rx: None,
            llm_config: llm_config.clone(),
            theme,
            settings_tab: SettingsTab::LlmProvider,
            dwell_state: DwellState::new_with_delay(dwell_millis, dwell_activation_ms),
            enter_was_physically_down: false,
            last_eye_click_time: Instant::now() - Duration::from_secs(1),
            last_trackball_input: Instant::now() - Duration::from_secs(1),
            eye_tracker: eyetracking::EyeTrackingController::default(),

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
            switch_auto_scroll,
            switch_focus_index: 0,
            switch_last_tick: Instant::now(),
            dictation_origin_target: None,

            hover_last_cursor_pos: None,
            hover_cursor_still_since: Instant::now(),
            hover_pending_speech: None,
            hover_speech_deadline: None,
            hover_last_announced_target: None,
            hover_last_announced_zone: None,
            hover_controls_speech_enabled: false,
            hover_physical_mouse_moved: false,
            hover_start_time: Instant::now(),
            hover_had_lost_focus: false,

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
            active_download: None,
            input_suggestions: Vec::new(),
            last_suggestion_query: String::new(),
            attached_file_path: None,
            file_picker_rx: None,
            macros,
            macro_shortcut_input: String::new(),
            macro_text_input: String::new(),
            macro_error: None,
            hass_url_input,
            hass_token_input,
            knx_gateway_input,
            hass_status_feedback: None,

            active_disability_presets,
            auto_read_agent_messages,
            auto_read_suggestions,
            earcons_enabled,

            detected_microphones,
            selected_microphone,
            detected_pointing_devices,
            pending_twitch_speech: VecDeque::new(),
        };
        app.sync_modal_inputs_for_provider(llm_config.provider);
        update_open_floating_cards(&app.floating_cards);

        if !app.current_twitch_channel.trim().is_empty() {
            let _ = app.twitch_channel_sender.send(app.current_twitch_channel.clone());
        }

        if llm_config.provider == LlmProvider::Local {
            std::thread::spawn(|| {
                std::thread::sleep(Duration::from_millis(1500));
                println!("[Main] Préchargement sécurisé du LLM local après initialisation UI...");
                local_llm::init_local_llm();
                println!("[Main] Préchargement LLM local terminé.");
            });
        }

        app
    }

    pub fn get_libertide_window_id(&self) -> isize {
        #[cfg(windows)]
        {
            let overlay = unsafe {
                windows::Win32::UI::WindowsAndMessaging::FindWindowW(
                    None,
                    windows::core::w!("Libertide overlay"),
                )
                .unwrap_or(windows::Win32::Foundation::HWND(std::ptr::null_mut()))
            };
            if !overlay.0.is_null() {
                overlay.0 as isize
            } else {
                -999
            }
        }
        #[cfg(not(windows))]
        {
            -999
        }
    }

    pub fn is_libertide_window_id(&self, id: isize) -> bool {
        id == self.get_libertide_window_id() || id == -999
    }

    pub fn is_single_switch_mode(&self) -> bool {
        self.switch_auto_scroll
            || self.active_disability_presets.contains(&DisabilityPreset::MotorSwitch)
    }

    pub fn has_motor_disability(&self) -> bool {
        self.active_disability_presets.contains(&DisabilityPreset::MotorSwitch)
            || self.active_disability_presets.contains(&DisabilityPreset::Tremor)
            || self.switch_access_mode
    }

    pub fn should_show_hover_mode_button(&self) -> bool {
        self.switch_access_mode
            || self.active_disability_presets.contains(&DisabilityPreset::Visual)
            || self.active_disability_presets.contains(&DisabilityPreset::MotorSwitch)
            || self.active_disability_presets.contains(&DisabilityPreset::Tremor)
    }

    pub fn should_show_eye_tracking_button(&self) -> bool {
        (crate::eyetracking::is_eyetracking_model_installed()
            || crate::eyetracking::get_eye_tracker_source() != crate::eyetracking::EyeTrackerSource::WebcamNeural
            || crate::eyetracking::is_eye_tracking_enabled())
            && (self.has_motor_disability()
                || crate::eyetracking::is_eye_tracking_enabled())
    }

    pub fn open_instrument_card(&mut self) {
        let title = t!("instrument.title").to_string();
        let card = FloatingCard::new("instrument_toy", title, FloatingContentType::Instrument);
        if let Some(existing) = self.floating_cards.iter_mut().find(|c| c.id == "instrument_toy") {
            existing.open = true;
        } else {
            self.floating_cards.push(card);
        }
        update_open_floating_cards(&self.floating_cards);
        play_earcon(Earcon::Success);
    }

    pub fn is_instrument_open(&self) -> bool {
        self.floating_cards.iter().any(|c| c.id == "instrument_toy" && c.open)
    }

    pub fn open_keyboard_card(&mut self) {
        let title = t!("keyboard.title").to_string();
        let card = FloatingCard::new("virtual_keyboard", title, FloatingContentType::VirtualKeyboard);
        crate::virtual_keyboard::reset_keyboard_scan();
        if let Some(existing) = self.floating_cards.iter_mut().find(|c| c.id == "virtual_keyboard") {
            existing.open = true;
        } else {
            self.floating_cards.push(card);
        }
        update_open_floating_cards(&self.floating_cards);
        play_earcon(Earcon::Success);
    }

    pub fn is_keyboard_open(&self) -> bool {
        self.floating_cards.iter().any(|c| c.id == "virtual_keyboard" && c.open)
    }

    pub fn open_aac_card(&mut self) {
        let title = "💬 Communication CAA (ARASAAC)".to_string();
        let card = FloatingCard::new("caa_board", title, FloatingContentType::AacBoard);
        if let Some(existing) = self.floating_cards.iter_mut().find(|c| c.id == "caa_board") {
            existing.open = true;
        } else {
            self.floating_cards.push(card);
        }
        update_open_floating_cards(&self.floating_cards);
        play_earcon(Earcon::Success);
    }

    pub fn is_subtitles_open(&self) -> bool {
        crate::audio::is_live_subtitles_active()
            && self.floating_cards.iter().any(|c| matches!(c.content, FloatingContentType::Subtitles { .. }) && c.open)
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
            VoiceProfile::Chinese => "Voix chinoise configurée.".to_string(),
            VoiceProfile::Russian => "Voix russe configurée.".to_string(),
            VoiceProfile::Japanese => "Voix japonaise configurée.".to_string(),
            VoiceProfile::Clone => t!("speech.voice_clone").to_string(),
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
        self.pending_twitch_speech.clear();
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
        self.pending_twitch_speech.clear();
        IS_DICTATION_ACTIVE.store(false, Ordering::SeqCst);
        cancel_all_scheduled_tasks();
        floating_window::stop_live_subtitles();
        crate::audio::stop_live_subtitles();
        eyetracking::set_eye_tracking_enabled(false);
        self.eye_tracker.cancel_calibration();
        if let Ok(mut lock) = eyetracking::GLOBAL_GAZE_POS.lock() {
            *lock = None;
        }
        clear_all_screen_overlays();
        self.switch_access_mode = false;
        SWITCH_HOOK_ENABLED.store(false, Ordering::SeqCst);
        reset_switch_keys();
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
        let all_sources = t!("commands.subtitles_source_all").to_string();
        let source_desc = target_window.as_deref().filter(|s| !crate::audio::is_all_sources_target(Some(s))).unwrap_or(&all_sources);
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
        self.mic_manually_disabled = false;
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
        let announcement = if self.switch_access_mode && self.dictation_origin_target.is_some() {
            let target_name = self.dictation_origin_target.as_ref().map(|t| t.name.as_str()).unwrap_or("");
            let on_target = t!("speech.dictation_on_field", field = target_name).to_string();
            format!("{msg}. {on_target}")
        } else {
            msg.clone()
        };

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
        let _ = self.tts_sender.send(TtsCommand::Speak(announcement));
    }

    pub fn exit_dictation_mode(&mut self) {
        self.dictation_mode = false;
        self.continuous_mode = false;
        self.mic_manually_disabled = true;
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

        // Revenir à l'élément de contrôle actif qui a initié la dictée en mode survol
        if self.switch_access_mode {
            if let Some(target) = self.dictation_origin_target.take() {
                let target_clone = target.clone();
                if let Some(pos) = self.numbered_targets.iter().position(|t| t.index == target_clone.index) {
                    self.switch_focus_index = pos;
                }
                self.narrate_numbered_target(&target_clone);
                #[cfg(windows)]
                draw_screen_control_border(target_clone.rect, false);
            }
            self.sync_switch_access_overlays();
        } else {
            self.dictation_origin_target = None;
        }
    }

    pub fn toggle_switch_access_mode(&mut self, force_state: Option<bool>) {
        mark_hover_nav_asked();
        self.switch_access_mode = force_state.unwrap_or(!self.switch_access_mode);
        SWITCH_HOOK_ENABLED.store(self.switch_access_mode, Ordering::SeqCst);
        reset_switch_keys();

        if self.switch_access_mode {
            self.dictation_mode = false;
            IS_DICTATION_ACTIVE.store(false, Ordering::SeqCst);
            self.is_hidden = true;
            self.switch_step = SwitchAccessStep::Zones;
            self.switch_windows = collect_switch_candidate_windows();
            self.switch_auto_scroll = self.is_single_switch_mode();
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
            self.hover_start_time = Instant::now();
            self.hover_had_lost_focus = false;
            play_earcon(Earcon::Success);

            if !self.switch_windows.is_empty() {
                #[cfg(windows)]
                let active_idx = {
                    let fg = unsafe { windows::Win32::UI::WindowsAndMessaging::GetForegroundWindow() };
                    let overlay = unsafe { windows::Win32::UI::WindowsAndMessaging::FindWindowW(None, windows::core::w!("Libertide overlay")).unwrap_or(windows::Win32::Foundation::HWND(std::ptr::null_mut())) };
                    let ext_h = LAST_EXTERNAL_HWND.lock().ok().and_then(|g| *g);
                    let target_id = if !fg.0.is_null() && fg != overlay {
                        Some(fg.0 as isize)
                    } else {
                        ext_h
                    };
                    target_id.and_then(|tid| self.switch_windows.iter().position(|w| w.id == tid && w.id != overlay.0 as isize))
                };
                #[cfg(not(windows))]
                let active_idx = None;

                let overlay_id = self.get_libertide_window_id();
                let mut chosen_idx = active_idx.unwrap_or(0);
                if self.switch_windows.get(chosen_idx).map_or(false, |w| w.id == overlay_id) {
                    if let Some(first_ext) = self.switch_windows.iter().position(|w| w.id != overlay_id) {
                        chosen_idx = first_ext;
                    }
                }
                self.switch_focus_index = chosen_idx.min(self.switch_windows.len().saturating_sub(1));
                let cur_win = self.switch_windows[self.switch_focus_index].clone();

                if self.is_libertide_window_id(cur_win.id) {
                    self.switch_to_libertide_window();
                } else {
                    self.switch_active_window_id = Some(cur_win.id);
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
                    }
                    #[cfg(not(windows))]
                    {
                        focus_and_isolate_window(&cur_win.title);
                        set_global_cursor_pos(cur_win.click_x, cur_win.click_y);
                        self.hover_last_cursor_pos = Some((cur_win.click_x, cur_win.click_y));
                    }

                    self.transition_to_zones_for_window(cur_win.id, &cur_win.title);
                }
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
            self.switch_auto_scroll = self.is_single_switch_mode();
            self.hover_last_cursor_pos = None;
            self.hover_cursor_still_since = Instant::now();
            self.hover_pending_speech = None;
            self.hover_speech_deadline = None;
            self.hover_last_announced_target = None;
            self.hover_last_announced_zone = None;
            self.hover_controls_speech_enabled = false;
            self.hover_physical_mouse_moved = false;
            self.hover_had_lost_focus = false;
            self.numbered_input_buffer.clear();
            clear_all_screen_overlays();
            play_earcon(Earcon::MicClose);

            #[cfg(not(windows))]
            {
                self.position_initialized = false;
            }

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

    pub fn trigger_gaze_click(&mut self) {
        let (cur_x, cur_y) = match eyetracking::get_physical_cursor_pos() {
            Some(pos) => pos,
            None => return,
        };
        eprintln!("[Eye-Tracking] Clic (Entrée) déclenché aux coordonnées physiques ({}, {})", cur_x, cur_y);

        let pause_dur = self.eye_tracker.pause_duration_on_click;
        self.eye_tracker.freeze_gaze(pause_dur);
        self.last_trackball_input = Instant::now();
        self.last_switch_input = Instant::now();

        crate::eyetracking::perform_system_gaze_click();
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
            self.is_hidden = false;
            ctx.send_viewport_cmd(egui::ViewportCommand::Minimized(false));
            ctx.send_viewport_cmd(egui::ViewportCommand::Maximized(false));
            if let Some(pos) = self.window_pos {
                ctx.send_viewport_cmd(egui::ViewportCommand::OuterPosition(pos));
            }
            if self.switch_access_mode {
                self.switch_to_libertide_window();
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

    pub fn start_recording<'a>(&mut self, _ctx: impl Into<Option<&'a egui::Context>>, play_sound: bool) {
        if self.status == AgentStatus::EmergencyStopped || self.voice_is_recording_sample || self.mic_manually_disabled {
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

    pub fn stop_recording(&mut self) {
        self.is_recording = false;
        self.continuous_mode = false;
        self.mic_manually_disabled = true;
        let _ = self.audio_sender.send(AudioCommand::Stop);
        play_earcon(Earcon::MicClose);
        set_mic_audio_level(0.0);
        self.live_transcript.clear();
        self.status = AgentStatus::Idle;
    }

    pub fn toggle_recording<'a>(&mut self, ctx: impl Into<Option<&'a egui::Context>>) {
        if self.voice_is_recording_sample {
            return;
        }
        if self.is_recording {
            self.mic_manually_disabled = true;
            self.stop_recording();
        } else {
            self.mic_manually_disabled = false;
            let ctx_ref = ctx.into();
            self.start_recording(ctx_ref, true);
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
        crate::keyboard_hook::apply_egui_key_remaps(&ctx);
        ctx.set_zoom_factor(self.zoom_factor);

        IS_CHAT_TAB_ACTIVE.store(self.active_tab == ActiveTab::Chat, Ordering::Relaxed);
        update_last_external_focus();

        #[cfg(windows)]
        let is_libertide_fg = {
            use windows::core::w;
            use windows::Win32::UI::WindowsAndMessaging::{FindWindowW, GetForegroundWindow};
            let overlay = unsafe { FindWindowW(None, w!("Libertide overlay")).unwrap_or(windows::Win32::Foundation::HWND(std::ptr::null_mut())) };
            let fg = unsafe { GetForegroundWindow() };
            (!overlay.0.is_null() && fg == overlay) || ctx.input(|i| i.viewport().focused.unwrap_or(false))
        };
        #[cfg(not(windows))]
        let is_libertide_fg = ctx.input(|i| {
            i.viewport().focused.unwrap_or(false)
        });

        self.restore_window_if_minimized(&ctx);

        // Focus sur Libertide (barre des tâches, Alt+Tab, etc.) : on reste en mode survol et on bascule sur Libertide
        if self.switch_access_mode {
            if is_libertide_fg {
                if self.is_hidden && self.hover_start_time.elapsed() >= Duration::from_millis(300) {
                    self.switch_to_libertide_window();
                    ctx.send_viewport_cmd(egui::ViewportCommand::Maximized(false));
                    ctx.send_viewport_cmd(egui::ViewportCommand::Minimized(false));
                    ctx.request_repaint();
                }
            } else {
                self.hover_had_lost_focus = true;
            }
        } else if self.is_hidden && is_libertide_fg {
            self.is_hidden = false;
            ctx.send_viewport_cmd(egui::ViewportCommand::Maximized(false));
            ctx.send_viewport_cmd(egui::ViewportCommand::Minimized(false));
            ctx.request_repaint();
        }

        let is_egui_text_focused = !self.is_hidden
            && is_libertide_fg
            && (ctx.egui_wants_keyboard_input()
                || ctx.memory(|m| {
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
                        || m.has_focus(egui::Id::NULL.with("hass_url_input_field"))
                        || m.has_focus(egui::Id::NULL.with("hass_token_input_field"))
                        || m.has_focus(egui::Id::NULL.with("knx_gateway_input_field"))
                }));
        let is_typing = !self.is_hidden && is_libertide_fg && (self.show_api_key_modal || is_egui_text_focused);
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
            if self.last_trackball_input.elapsed() >= Duration::from_millis(250)
                && self.last_switch_input.elapsed() >= Duration::from_millis(250) {
                let xbutton1 = unsafe { (GetAsyncKeyState(0x05) as u16 & 0x8000) != 0 };
                let xbutton2 = unsafe { (GetAsyncKeyState(0x06) as u16 & 0x8000) != 0 };
                let mbutton  = unsafe { (GetAsyncKeyState(0x04) as u16 & 0x8000) != 0 };

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

        let work_area = get_desktop_work_area_scaled(&ctx);

        // Traitement Eye-Tracking
        self.process_gaze_tick(&ctx, &work_area, ppp, is_typing);

        let query_source = if !self.input_text.trim().is_empty() {
            self.input_text.trim()
        } else if self.is_recording && !self.live_transcript.trim().is_empty() {
            // Préchauffage des suggestions applicatives dès la parole en direct ("ouvre spotify" -> "spotify")
            let live = self.live_transcript.trim();
            let words: Vec<&str> = live.split_whitespace().collect();
            if words.len() >= 2 && (words[0].eq_ignore_ascii_case("ouvre") || words[0].eq_ignore_ascii_case("lance")) {
                words[1]
            } else {
                words.last().copied().unwrap_or(live)
            }
        } else {
            ""
        };
        if query_source != self.last_suggestion_query {
            self.last_suggestion_query = query_source.to_string();
            self.input_suggestions = if query_source.len() >= 2 {
                find_matching_applications(query_source, 5)
            } else {
                Vec::new()
            };
        }

        let has_suggestions = !self.input_suggestions.is_empty();
        let is_maximized = ctx.input(|i| i.viewport().maximized.unwrap_or(false));

        if self.eye_tracker.calibration.is_active {
            ctx.send_viewport_cmd(egui::ViewportCommand::InnerSize(egui::vec2(work_area.w, work_area.h)));
            ctx.send_viewport_cmd(egui::ViewportCommand::OuterPosition(egui::pos2(work_area.x, work_area.y)));
        }

        #[cfg(not(windows))]
        if self.switch_access_mode {
            ctx.send_viewport_cmd(egui::ViewportCommand::InnerSize(egui::vec2(work_area.w, work_area.h)));
            ctx.send_viewport_cmd(egui::ViewportCommand::OuterPosition(egui::pos2(work_area.x, work_area.y)));
        }

        let was_calibrating = self.eye_tracker.calibration.is_active;
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
            is_maximized || was_calibrating,
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

        // Dépouillement des événements asynchrones et entrées utilisateur
        self.process_agent_events(&ctx);
        self.process_input_events(&ctx, is_typing);

        let time = ctx.input(|i| i.time);
        ctx.request_repaint_after(Duration::from_millis(16));

        if !crate::scheduler::get_active_scheduled_tasks().is_empty() {
            ctx.request_repaint_after(Duration::from_millis(1000));
        }

        if self.is_hidden {
            self.render_hidden_state(ui, &ctx, &theme);
            return;
        }

        if self.eye_tracker.calibration.is_active {
            self.render_calibration_screen(ui, &ctx, &work_area);
            return;
        }

        if eyetracking::is_eye_tracking_enabled() {
            self.render_gaze_reticle(ui);
        }

        if eyetracking::should_show_eye_debug() {
            eyetracking::show_eye_debug_floating_window(&ctx, &theme);
        }

        egui::CentralPanel::default()
            .frame(egui::Frame::new().fill(egui::Color32::TRANSPARENT).inner_margin(egui::Margin::ZERO))
            .show(ui, |ui| {
                ui.painter().rect(
                    response_rect,
                    egui::CornerRadius::same(12),
                    theme.panel_bg,
                    egui::Stroke::new(1.0, theme.border_subtle),
                    egui::StrokeKind::Inside,
                );

                // 1. Barre d'onglets supérieure et boutons système
                let tab_bar_max_x = self.render_header_bar(ui, response_rect, is_maximized, &theme);

                let sep_y = response_rect.min.y + 35.0;
                ui.painter().hline(
                    (response_rect.min.x + 8.0)..=(response_rect.max.x - 8.0),
                    sep_y,
                    egui::Stroke::new(1.0, theme.border_subtle),
                );

                // 2. Dock de contrôle (avatar, saisie, barre d'actions, égaliseur)
                self.render_control_dock(ui, control_rect, time, &theme);

                // 3. Boîte de discussion adaptative (Assistance / Chat Twitch)
                let messages_rect = egui::Rect::from_min_max(
                    egui::pos2(response_rect.min.x + 8.0, sep_y + 4.0),
                    egui::pos2(response_rect.max.x - 8.0, response_rect.max.y - 6.0),
                );
                let computed_chat_h = self.render_chat_stream(
                    ui,
                    messages_rect,
                    current_chat_h,
                    min_chat_h,
                    max_chat_h,
                    &theme,
                );

                // Hystérésis asymétrique pour self.chat_height
                if computed_chat_h > self.chat_height + 4.0 || computed_chat_h < self.chat_height - 8.0 {
                    self.chat_height = computed_chat_h;
                }

                // Déplacement et magnétisme
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

                // Redimensionnement
                handle_overlay_resize_zones(ui, &ctx, win_w, active_win_h);

                let painter = ui.painter();
                self.sync_switch_access_overlays();
                self.render_switch_overlays(painter, &ctx);

                if self.switch_access_mode {
                    let stroke_color = egui::Color32::from_rgb(255, 215, 0);
                    painter.rect_stroke(
                        control_rect,
                        egui::CornerRadius::same(12),
                        egui::Stroke::new(2.5, stroke_color),
                        egui::StrokeKind::Inside,
                    );
                }

                draw_corner_grips(painter, win_w, active_win_h, padding);

                // Boîte modale de configuration
                if self.show_api_key_modal {
                    self.render_settings_modal(&ctx, &theme, win_w, active_win_h);
                }

                // Fenêtres volantes
                self.process_floating_cards(&ctx, &theme);
            });
    }

    fn on_exit(&mut self, _gl: Option<&eframe::glow::Context>) {
        uninstall_switch_keyboard_hook();
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
                         | AgentEvent::TranscriptionPartial(_)
                 );

                if should_wake {
                    if !matches!(event, AgentEvent::TranscriptionPartial(_)) {
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
                    }
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
    let mut viewport = egui::ViewportBuilder::default()
        .with_decorations(false)
        .with_transparent(true)
        .with_always_on_top()
        .with_resizable(true)
        .with_min_inner_size([400.0, 264.0])
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
    }

    unsafe {
        let _ = _set_invalid_parameter_handler(Some(silent_invalid_parameter));
    }
}

fn main() -> eframe::Result<()> {
    #[cfg(windows)]
    disable_crt_assertion_dialogs();
    fix_startup_working_directory();
    load_keybindings();

    let (event_tx, event_rx) = channel::<AgentEvent>();
    let (cmd_tx, cmd_rx) = unbounded_channel::<AgentCommand>();
    let initial_voice = resolve_initial_voice();
    let tts_tx = spawn_tts_worker(event_tx.clone(), initial_voice);
    let (twitch_ch_tx, twitch_ch_rx) = channel::<String>();
    spawn_twitch_worker(event_tx.clone(), twitch_ch_rx);
    let llm_config = resolve_llm_config();
    let audio_tx = spawn_audio_worker(event_tx.clone(), llm_config.api_key.clone());

    let initial_twitch_channel = std::env::var("TWITCH_CHANNEL").unwrap_or_default();

    install_switch_keyboard_hook();
    #[cfg(windows)]
    crate::window_util::ensure_window_event_hook();

    spawn_periodic_screen_monitor(cmd_tx.clone());
    crate::scheduler::spawn_proactive_companion_loop(cmd_tx.clone());
    spawn_agent_backend(cmd_rx, cmd_tx.clone(), event_tx.clone(), tts_tx.clone(), llm_config.clone());

    // Préchargement asynchrone du modèle de complétion ONNX pour le clavier virtuel
    {
        let event_tx_kb = event_tx.clone();
        virtual_keyboard::init_keyboard_model_async(event_tx_kb);
    }

    if llm_config.provider == LlmProvider::Local {
        std::thread::sleep(Duration::from_millis(500));
        std::thread::spawn(|| {
            println!("[Main] Démarrage du thread de préchargement LLM local...");
            local_llm::init_local_llm();
            println!("[Main] Thread de préchargement LLM local terminé.");
        });
    }
    if local_llm::is_vlm_grounding_enabled() {
        let event_tx_vlm = event_tx.clone();
        std::thread::spawn(move || {
            local_llm::init_vlm_async(event_tx_vlm);
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