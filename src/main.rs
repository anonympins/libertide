mod audio;
mod automation;
mod fs_util;
mod pattern;
mod twitch;
mod types;
mod window_util;

use automation::*;
use fs_util::*;
use pattern::clean_words;
use window_util::*;
use audio::spawn_audio_worker;
use twitch::{search_twitch_channels, spawn_twitch_worker};
use eframe::egui;
use serde::Deserialize;
use types::*;
use std::sync::{Arc, Mutex};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::mpsc::{channel, Receiver, Sender as StdSender};
use tokio::sync::mpsc::{unbounded_channel, UnboundedSender};
use std::time::{Duration, Instant};

#[cfg(windows)]
use windows::Win32::Foundation::POINT;
#[cfg(windows)]
use windows::Win32::Media::Speech::{ISpVoice, SpVoice, SPF_ASYNC, SPF_PURGEBEFORESPEAK, SPVOICESTATUS};
#[cfg(windows)]
use windows::Win32::System::Com::{CoCreateInstance, CoInitializeEx, CLSCTX_ALL, COINIT_MULTITHREADED};
#[cfg(windows)]
use windows::Win32::UI::WindowsAndMessaging::GetCursorPos;

pub struct OverlayApp {
    status: AgentStatus,
    chat_history: Vec<ChatEntry>,
    active_tab: ActiveTab,
    twitch_messages: Vec<TwitchMessage>,
    live_transcript: String,
    input_text: String,
    is_recording: bool,
    continuous_mode: bool,

    // Canaux de communication asynchrones
    event_receiver: Receiver<AgentEvent>,
    command_sender: UnboundedSender<AgentCommand>,
    tts_sender: StdSender<TtsCommand>,
    audio_sender: StdSender<AudioCommand>,
    twitch_channel_sender: StdSender<String>,
    current_twitch_channel: String,
    twitch_search_query: String,
    twitch_search_results: Vec<TwitchChannelItem>,
    position_initialized: bool,
    window_pos: Option<egui::Pos2>,
    drag_offset: Option<egui::Vec2>,
    is_hidden: bool,
    mouse_passthrough: bool,
}

#[cfg(windows)]
fn get_screen_cursor_pos(ctx: &egui::Context) -> Option<egui::Pos2> {
    let mut pt = POINT::default();
    unsafe {
        if GetCursorPos(&mut pt).is_ok() {
            let ppp = ctx.pixels_per_point();
            if ppp > 0.0 {
                return Some(egui::pos2(pt.x as f32 / ppp, pt.y as f32 / ppp));
            }
        }
    }
    None
}

#[cfg(not(windows))]
fn get_screen_cursor_pos(ctx: &egui::Context) -> Option<egui::Pos2> {
    ctx.input(|i| i.pointer.latest_pos())
}

fn sanitize_for_tts(text: &str) -> String {
    text.chars()
        .filter(|&c| c != '*' && c != '#' && c != '`' && c != '_' && c != '~')
        .collect()
}

fn is_hide_command(text: &str) -> bool {
    let words = clean_words(text);
    let has_verb = words.iter().any(|w| {
        w == "ferme" || w == "fermer" || w == "cache" || w == "cacher" || w == "masque" || w == "masquer" || w == "disparais"
    });
    let has_target = words.iter().any(|w| {
        w == "toi" || w == "overlay" || w == "deepseek" || w == "groq" || w == "libertide"
    });
    has_verb && has_target
}

fn is_show_command(text: &str) -> bool {
    let words = clean_words(text);
    let has_verb = words.iter().any(|w| {
        w == "ouvre" || w == "ouvrir" || w == "affiche" || w == "afficher" || w == "montre" || w == "montrer" || w == "reveille" || w == "réveille" || w == "reveiller" || w == "réveiller"
    });
    let has_target = words.iter().any(|w| {
        w == "toi" || w == "overlay" || w == "deepseek" || w == "groq" || w == "libertide"
    });
    has_verb && has_target
}

pub(crate) fn current_time_str() -> String {
    #[cfg(windows)]
    unsafe {
        #[repr(C)]
        struct SystemTimeWin {
            year: u16,
            month: u16,
            day_of_week: u16,
            day: u16,
            hour: u16,
            minute: u16,
            second: u16,
            milliseconds: u16,
        }
        extern "system" {
            fn GetLocalTime(st: *mut SystemTimeWin);
        }
        let mut st = SystemTimeWin {
            year: 0, month: 0, day_of_week: 0, day: 0,
            hour: 0, minute: 0, second: 0, milliseconds: 0,
        };
        GetLocalTime(&mut st);
        format!("{:02}:{:02}", st.hour, st.minute)
    }
    #[cfg(not(windows))]
    {
        let secs = std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).unwrap_or_default().as_secs();
        format!("{:02}:{:02}", (secs / 3600) % 24, (secs / 60) % 60)
    }
}

#[cfg(windows)]
fn spawn_tts_worker(event_tx: StdSender<AgentEvent>) -> StdSender<TtsCommand> {
    let (tx, rx) = channel::<TtsCommand>();

    std::thread::spawn(move || {
        unsafe {
            let _ = CoInitializeEx(None, COINIT_MULTITHREADED);
            let voice: Result<ISpVoice, _> = CoCreateInstance(&SpVoice, None, CLSCTX_ALL);

            let Ok(voice) = voice else {
                eprintln!("Échec de l'initialisation du synthétiseur vocal SAPI Windows");
                return;
            };

            while let Ok(cmd) = rx.recv() {
                match cmd {
                    TtsCommand::Stop => {
                        let _ = voice.Speak(
                            windows::core::PCWSTR::null(),
                            SPF_PURGEBEFORESPEAK.0 as u32,
                            None,
                        );
                    }
                    TtsCommand::Speak(raw_text) => {
                        let clean_text = sanitize_for_tts(&raw_text);
                        if clean_text.trim().is_empty() {
                            let _ = event_tx.send(AgentEvent::StatusChanged(AgentStatus::Idle));
                            let _ = event_tx.send(AgentEvent::TtsFinished);
                            continue;
                        }

                        let wide: Vec<u16> = clean_text.encode_utf16().chain(std::iter::once(0)).collect();
                        let flags = (SPF_ASYNC.0 | SPF_PURGEBEFORESPEAK.0) as u32;

                        if voice
                            .Speak(windows::core::PCWSTR(wide.as_ptr()), flags, None)
                            .is_ok()
                        {
                            let _ = event_tx.send(AgentEvent::StatusChanged(AgentStatus::Speaking));
                            std::thread::sleep(Duration::from_millis(80));

                            loop {
                                std::thread::sleep(Duration::from_millis(50));

                                if let Ok(next_cmd) = rx.try_recv() {
                                    match next_cmd {
                                        TtsCommand::Stop => {
                                            let _ = voice.Speak(
                                                windows::core::PCWSTR::null(),
                                                SPF_PURGEBEFORESPEAK.0 as u32,
                                                None,
                                            );
                                            break;
                                        }
                                        TtsCommand::Speak(next_text) => {
                                            let clean_next = sanitize_for_tts(&next_text);
                                            if !clean_next.trim().is_empty() {
                                                let wide_next: Vec<u16> =
                                                    clean_next.encode_utf16().chain(std::iter::once(0)).collect();
                                                let _ = voice.Speak(
                                                    windows::core::PCWSTR(wide_next.as_ptr()),
                                                    flags,
                                                    None,
                                                );
                                                std::thread::sleep(Duration::from_millis(80));
                                            }
                                        }
                                    }
                                }

                                let mut status = SPVOICESTATUS::default();
                                if voice.GetStatus(&mut status, std::ptr::null_mut()).is_ok() {
                                    if status.dwRunningState != 2 {
                                        let _ = event_tx.send(AgentEvent::StatusChanged(AgentStatus::Idle));
                                        let _ = event_tx.send(AgentEvent::TtsFinished);
                                        break;
                                    }
                                } else {
                                    let _ = event_tx.send(AgentEvent::StatusChanged(AgentStatus::Idle));
                                    let _ = event_tx.send(AgentEvent::TtsFinished);
                                    break;
                                }
                            }
                        } else {
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

#[cfg(not(windows))]
fn spawn_tts_worker(_event_tx: StdSender<AgentEvent>) -> StdSender<TtsCommand> {
    let (tx, rx) = channel::<TtsCommand>();
    std::thread::spawn(move || {
        while let Ok(cmd) = rx.recv() {
            if let TtsCommand::Speak(_) = cmd {
                let _ = _event_tx.send(AgentEvent::TtsFinished);
            }
        }
    });
    tx
}

impl OverlayApp {
    pub fn new(
        _cc: &eframe::CreationContext<'_>,
        event_receiver: Receiver<AgentEvent>,
        command_sender: UnboundedSender<AgentCommand>,
        tts_sender: StdSender<TtsCommand>,
        audio_sender: StdSender<AudioCommand>,
        twitch_channel_sender: StdSender<String>,
        initial_twitch_channel: String,
    ) -> Self {
        let initial_subtitle = "En attente d'instructions d'exploration...".to_string();
        let _ = tts_sender.send(TtsCommand::Speak(initial_subtitle.clone()));

        Self {
            status: AgentStatus::Idle,
            chat_history: vec![ChatEntry {
                role: ChatRole::Agent,
                text: initial_subtitle,
                timestamp: current_time_str(),
                quick_suggestions: Vec::new(),
                screen_size_kb: None,
            }],
            active_tab: ActiveTab::Assistance,
            twitch_messages: Vec::new(),
            live_transcript: String::new(),
            input_text: String::new(),
            is_recording: false,
            continuous_mode: true,
            event_receiver,
            command_sender,
            tts_sender,
            audio_sender,
            twitch_channel_sender,
            current_twitch_channel: initial_twitch_channel,
            twitch_search_query: String::new(),
            twitch_search_results: Vec::new(),
            position_initialized: false,
            window_pos: None,
            drag_offset: None,
            is_hidden: false,
            mouse_passthrough: false,
        }
    }

    fn draw_avatar(&self, ui: &mut egui::Ui, center: egui::Pos2, base_radius: f32, time: f64) {
        let painter = ui.painter();

        let pulse_speed = match self.status {
            AgentStatus::Idle => 1.6,
            AgentStatus::Thinking => 4.2,
            AgentStatus::Speaking => 2.8,
            AgentStatus::Listening => 5.5,
            AgentStatus::EmergencyStopped => 0.0,
        };

        let wave = ((time * pulse_speed).sin() as f32).max(0.0);

        // Teinte rouge/corail en écoute micro, bleu cyan le reste du temps
        let (ring_base_color, core_color) = if self.status == AgentStatus::Listening {
            ([255, 65, 80], egui::Color32::from_rgb(255, 50, 70))
        } else {
            ([0, 180, 255], egui::Color32::from_rgba_unmultiplied(0, 180, 255, 220))
        };

        // Cercles concentriques
        let rings = [
            (base_radius + 26.0 + (wave * 6.0), egui::Color32::from_rgba_unmultiplied(ring_base_color[0], ring_base_color[1], ring_base_color[2], 35), 1.5),
            (base_radius + 18.0 + (wave * 4.0), egui::Color32::from_rgba_unmultiplied(ring_base_color[0], ring_base_color[1], ring_base_color[2], 60), 1.8),
            (base_radius + 10.0 + (wave * 2.5), egui::Color32::from_rgba_unmultiplied(ring_base_color[0], ring_base_color[1], ring_base_color[2], 110), 2.0),
            (base_radius + 2.0, egui::Color32::from_rgba_unmultiplied(ring_base_color[0].saturating_add(30), ring_base_color[1].saturating_add(30), ring_base_color[2], 180), 2.2),
        ];

        for (radius, color, stroke_width) in rings {
            painter.circle_stroke(center, radius, egui::Stroke::new(stroke_width, color));
        }

        // Noyau central bleu
        painter.circle_filled(
            center,
            base_radius * 0.45,
            core_color,
        );
        painter.circle_filled(center, base_radius * 0.2, egui::Color32::WHITE);
    }

    fn trigger_emergency_stop(&mut self, _ctx: &egui::Context) {
        self.continuous_mode = false;
        IS_EMERGENCY_STOPPED.store(true, Ordering::SeqCst);
        let _ = CURRENT_REQUEST_ID.fetch_add(1, Ordering::SeqCst);
        if self.is_recording {
            self.stop_recording();
        }
        self.live_transcript.clear();
        self.is_hidden = false;
        self.status = AgentStatus::EmergencyStopped;
        let stop_msg = "Arrêt d'urgence : contrôle rendu à l'utilisateur.".to_string();
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

    fn start_recording(&mut self, ctx: &egui::Context) {
        if self.status == AgentStatus::EmergencyStopped {
            return;
        }
        IS_EMERGENCY_STOPPED.store(false, Ordering::SeqCst);
        self.continuous_mode = true;
        self.is_recording = true;
        self.status = AgentStatus::Listening;
        self.input_text.clear();
        let _ = self.tts_sender.send(TtsCommand::Stop);
        if !self.is_hidden {
            self.live_transcript = "Écoute Whisper active... parlez à votre micro.".to_string();
        } else {
            self.live_transcript.clear();
        }
        ctx.memory_mut(|m| m.request_focus(egui::Id::new("prompt_input_text")));
        let _ = self.audio_sender.send(AudioCommand::Start);
    }

    fn stop_recording(&mut self) {
        self.is_recording = false;
        self.continuous_mode = false;
        let _ = self.audio_sender.send(AudioCommand::Stop);
        self.input_text.clear();
        self.live_transcript.clear();
        self.status = AgentStatus::Idle;
    }

    fn toggle_recording(&mut self, ctx: &egui::Context) {
        if self.is_recording {
            self.stop_recording();
        } else {
            self.start_recording(ctx);
        }
    }
}

impl eframe::App for OverlayApp {
    fn clear_color(&self, _visuals: &egui::Visuals) -> [f32; 4] {
        // Fond transparent pour laisser transparaître l'arrière-plan sur les 70 % supérieurs
        [0.0, 0.0, 0.0, 0.0]
    }

    fn ui(&mut self, ui: &mut egui::Ui, _frame: &mut eframe::Frame) {
        let ctx = ui.ctx().clone();
        let mut visuals = egui::Visuals::dark();
        visuals.panel_fill = egui::Color32::TRANSPARENT;
        visuals.window_fill = egui::Color32::TRANSPARENT;
        visuals.extreme_bg_color = egui::Color32::from_rgb(18, 20, 26);
        ctx.set_visuals(visuals);

        // Ancrage automatique de la fenêtre en bas à droite
        if !self.position_initialized {
            if let Some(mon_size) = ctx.input(|i| i.viewport().monitor_size) {
                if mon_size.x > 100.0 && mon_size.y > 100.0 {
                    let win_w = 560.0;
                    let win_h = 380.0;
                    let margin_x = 24.0;
                    let margin_y = 36.0;
                    let target_pos = egui::pos2(
                        mon_size.x - win_w - margin_x,
                        mon_size.y - win_h - margin_y,
                    );
                    ctx.send_viewport_cmd(egui::ViewportCommand::OuterPosition(target_pos));
                    self.window_pos = Some(target_pos);
                    self.position_initialized = true;
                }
            }
        }

        if let Some(outer) = ctx.input(|i| i.viewport().outer_rect) {
            self.window_pos = Some(outer.min);
        }

        let win_w: f32 = 560.0;
        let win_h: f32 = 380.0;
        let response_h = (win_h * 0.70 - 4.0).max(60.0);

        // Gestion dynamique du clic traversant : la zone transparente laisse passer les événements,
        // seuls les contrôles inférieurs et l'en-tête de glissement capturent la souris.
        let win_pos = self.window_pos.unwrap_or(egui::pos2(0.0, 0.0));
        let cursor_screen = get_screen_cursor_pos(&ctx);
        let wants_interaction = if self.is_hidden {
            false
        } else if self.drag_offset.is_some() {
            true
        } else if let Some(cursor) = cursor_screen {
            let rel_x = cursor.x - win_pos.x;
            let rel_y = cursor.y - win_pos.y;
            let in_window_x = rel_x >= 0.0 && rel_x <= win_w;
            let in_drag_header = in_window_x && rel_y >= 0.0 && rel_y <= 38.0;
            let in_response_area = in_window_x && rel_y >= 0.0 && rel_y <= response_h;
            let in_control_panel = in_window_x && rel_y >= (response_h + 4.0) && rel_y <= win_h;
            in_drag_header || in_response_area || in_control_panel
        } else {
            false
        };

        let should_passthrough = !wants_interaction;
        if should_passthrough != self.mouse_passthrough {
            self.mouse_passthrough = should_passthrough;
            ctx.send_viewport_cmd(egui::ViewportCommand::MousePassthrough(should_passthrough));
        }

        // Dépouillement des événements asynchrones
        while let Ok(event) = self.event_receiver.try_recv() {
            match event {
                AgentEvent::StatusChanged(new_status) => {
                    if self.status != AgentStatus::EmergencyStopped || new_status == AgentStatus::EmergencyStopped {
                        self.status = new_status;
                        if new_status == AgentStatus::Idle && self.is_recording {
                            self.is_recording = false;
                        }
                    }
                }
                AgentEvent::ScreenPayloadSize(kb) => {
                    if let Some(entry) = self.chat_history.iter_mut().rev().find(|e| e.role == ChatRole::User) {
                        entry.screen_size_kb = Some(kb);
                    }
                }
                AgentEvent::NarrationChunk(chunk) => {
                    if let Some(last) = self.chat_history.last_mut() {
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
                    self.chat_history.push(ChatEntry {
                        role: ChatRole::Agent,
                        text: text.clone(),
                        timestamp: current_time_str(),
                        quick_suggestions,
                        screen_size_kb: None,
                    });
                    let _ = self.tts_sender.send(TtsCommand::Speak(text));
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
                    if self.is_recording && !self.is_hidden {
                        self.input_text = text.clone();
                        self.live_transcript = text;
                    }
                }
                AgentEvent::VoicePromptReady(prompt) => {
                    self.is_recording = false;
                    self.live_transcript.clear();
                    let _ = self.tts_sender.send(TtsCommand::Stop);
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
                            let reply = "Me revoilà, overlay réaffiché.".to_string();
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
                            self.start_recording(&ctx);
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
                            let reply = "Overlay masqué. Je reste à l'écoute pour « deepseek ouvre toi ».".to_string();
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
                            let reply = "L'overlay est déjà actif et visible.".to_string();
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
                            self.start_recording(&ctx);
                        }
                    }
                }
                AgentEvent::TtsFinished => {
                    if self.continuous_mode && self.status != AgentStatus::EmergencyStopped {
                        self.start_recording(&ctx);
                    }
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
                        self.start_recording(&ctx);
                    }
                }
            }
        }

        // Raccourci clavier d'urgence global (Échap)
        if ctx.input(|i| i.key_pressed(egui::Key::Escape)) {
            self.trigger_emergency_stop(&ctx);
        }

        // Raccourci clavier 'R' pour basculer le micro (autorisé pour couper le micro même avec focus)
        let is_typing = ctx.memory(|m| m.focused().is_some()) && !self.is_recording;
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
            .frame(egui::Frame::new().fill(egui::Color32::TRANSPARENT))
            .show(ui, |ui| {
                let total_rect = ui.available_rect_before_wrap();
                let total_h = total_rect.height();

                // Répartition 70 % zone transparente (réponses de l'agent) et 30 % panneau de contrôle
                let control_h = (total_h - response_h - 8.0).max(80.0);

                let response_rect = egui::Rect::from_min_size(
                    total_rect.min,
                    egui::vec2(total_rect.width(), response_h),
                );
                let control_rect = egui::Rect::from_min_size(
                    egui::pos2(total_rect.min.x, total_rect.min.y + response_h + 8.0),
                    egui::vec2(total_rect.width(), control_h),
                );

                // 1. Zone supérieure transparente (70 %) pour l'historique du chat
                let mut tab_bar_max_x = total_rect.min.x + 210.0;
                ui.scope_builder(egui::UiBuilder::new().max_rect(response_rect), |ui| {
                    egui::Frame::new()
                        .fill(egui::Color32::from_rgba_unmultiplied(0, 0, 0, 180))
                        .stroke(egui::Stroke::new(1.0, egui::Color32::from_rgba_unmultiplied(0, 180, 255, 55)))
                        .corner_radius(egui::CornerRadius::same(14))
                        .inner_margin(egui::Margin::symmetric(14, 10))
                        .show(ui, |ui| {
                            // Barre d'onglets
                            ui.horizontal(|ui| {
                                let tab_assist = ui.selectable_label(self.active_tab == ActiveTab::Assistance, "🤖 Assistance");
                                if tab_assist.clicked() {
                                    self.active_tab = ActiveTab::Assistance;
                                }
                                let chat_title = if self.twitch_messages.is_empty() {
                                    "💬 Chat twitch".to_string()
                                } else {
                                    format!("💬 Chat ({})", self.twitch_messages.len())
                                };
                                let tab_chat = ui.selectable_label(self.active_tab == ActiveTab::Chat, chat_title);
                                if tab_chat.clicked() {
                                    self.active_tab = ActiveTab::Chat;
                                }
                                tab_bar_max_x = tab_assist.rect.max.x.max(tab_chat.rect.max.x);
                            });
                            ui.separator();

                            match self.active_tab {
                                ActiveTab::Assistance => {
                            let max_scroll_h = (response_h - 45.0).max(40.0);
                            egui::ScrollArea::vertical()
                                .stick_to_bottom(true)
                                .max_height(max_scroll_h)
                                .auto_shrink([false, true])
                                .show(ui, |ui| {
                                    let last_user_idx = self.chat_history.iter().rposition(|e| e.role == ChatRole::User);
                                    let last_agent_idx = self.chat_history.iter().rposition(|e| e.role == ChatRole::Agent);
                                    let mut clicked_quick_suggestion: Option<String> = None;

                                    for (idx, entry) in self.chat_history.iter().enumerate() {
                                        let is_highlighted = Some(idx) == last_user_idx || Some(idx) == last_agent_idx;

                                        ui.vertical(|ui| {
                                            ui.horizontal(|ui| {
                                                let (label_name, label_color) = match entry.role {
                                                    ChatRole::User => {
                                                        let name = if let Some(kb) = entry.screen_size_kb {
                                                            format!("🗣 Vous - {:.1} ko", kb)
                                                        } else {
                                                            "🗣 Vous".to_string()
                                                        };
                                                        (name, egui::Color32::from_rgb(255, 215, 120))
                                                    }
                                                    ChatRole::Agent => ("DeepSeek".to_string(), egui::Color32::from_rgb(0, 195, 255)),
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
                                            });

                                            let (text_size, line_height, text_color, is_strong) = match entry.role {
                                                ChatRole::User => {
                                                    if is_highlighted {
                                                        (14.5, 20.0, egui::Color32::from_rgb(255, 230, 150), true)
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

                                            let mut rich = egui::RichText::new(&entry.text)
                                                .size(text_size)
                                                .line_height(Some(line_height))
                                                .color(text_color);
                                            if is_strong {
                                                rich = rich.strong();
                                            }
                                            ui.add(egui::Label::new(rich).wrap());

                                            // Boutons-onglets pour les suggestions rapides
                                            if !entry.quick_suggestions.is_empty() {
                                                ui.add_space(4.0);
                                                ui.horizontal_wrapped(|ui| {
                                                    for sug in &entry.quick_suggestions {
                                                        if let Some(clean_label) = sug.to_display_string() {
                                                            let clean_label = clean_label.trim();
                                                            if clean_label.is_empty() {
                                                                continue;
                                                            }
                                                            let btn_text = egui::RichText::new(format!("💡 {clean_label}"))
                                                                .size(11.0)
                                                                .color(egui::Color32::from_rgb(130, 220, 255));

                                                            let btn = egui::Button::new(btn_text)
                                                                .stroke(egui::Stroke::new(1.0, egui::Color32::from_rgba_unmultiplied(0, 180, 255, 95)))
                                                                .corner_radius(egui::CornerRadius::same(10));

                                                            if ui.add(btn).on_hover_text("Cliquer pour choisir cette suggestion").clicked() {
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
                                        if self.is_recording {
                                            self.stop_recording();
                                        }
                                        let _ = self.tts_sender.send(TtsCommand::Stop);
                                        self.status = AgentStatus::Thinking;
                                        self.continuous_mode = true;
                                        self.live_transcript.clear();
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
                                ActiveTab::Chat => {
                                    // Barre de recherche et salon actif
                                    ui.horizontal(|ui| {
                                        if !self.current_twitch_channel.is_empty() {
                                            ui.label(
                                                egui::RichText::new("Salon :")
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
                                                .hint_text("Chaîne..."),
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
                                                egui::RichText::new("Résultats :")
                                                    .size(10.5)
                                                    .color(egui::Color32::from_rgb(255, 215, 120)),
                                            );
                                            let mut selected_channel = None;
                                            for ch in &self.twitch_search_results {
                                                if ui.small_button(format!("▶ {}", ch.display_name)).on_hover_text("Rejoindre ce salon irc").clicked() {
                                                    selected_channel = Some(ch.login.clone());
                                                }
                                                if ui.small_button("↗").on_hover_text("Ouvrir sur twitch").clicked() {
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
                                    let max_chat_scroll_h = (ui.available_height() - 4.0).max(40.0);
                                    egui::ScrollArea::vertical()
                                        .stick_to_bottom(true)
                                        .max_height(max_chat_scroll_h)
                                        .auto_shrink([false, true])
                                        .show(ui, |ui| {
                                            if self.twitch_messages.is_empty() {
                                                ui.label(
                                                    egui::RichText::new("En attente de messages twitch...")
                                                        .size(12.0)
                                                        .italics()
                                                        .color(egui::Color32::from_rgba_unmultiplied(180, 190, 205, 150)),
                                                );
                                            } else {
                                                for msg in &self.twitch_messages {
                                                    ui.horizontal_wrapped(|ui| {
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
                                                    });
                                                    ui.add_space(3.0);
                                                }
                                            }
                                        });
                                }
                            }
                        });
                });

                // 2. Zone inférieure (30 %) : contrôles, avatar et saisie
                ui.scope_builder(egui::UiBuilder::new().max_rect(control_rect), |ui| {
                    egui::Frame::new()
                        .fill(egui::Color32::from_rgb(18, 20, 26))
                        .stroke(egui::Stroke::new(1.0, egui::Color32::from_rgba_unmultiplied(0, 180, 255, 75)))
                        .corner_radius(egui::CornerRadius::same(14))
                        .inner_margin(egui::Margin::symmetric(14, 10))
                        .show(ui, |ui| {
                            let inner_rect = ui.available_rect_before_wrap();
                            let avatar_pos = egui::pos2(inner_rect.max.x - 22.0, inner_rect.min.y + 24.0);
                            self.draw_avatar(ui, avatar_pos, 15.0, time);

                            let content_area = egui::Rect::from_min_max(
                                inner_rect.min,
                                egui::pos2(inner_rect.max.x - 48.0, inner_rect.max.y),
                            );

                            ui.scope_builder(egui::UiBuilder::new().max_rect(content_area), |ui| {
                                ui.vertical(|ui| {
                                    let status_badge = match self.status {
                                        AgentStatus::Idle => "DeepSeek · en veille",
                                        AgentStatus::Thinking => "DeepSeek · réflexion...",
                                        AgentStatus::Speaking => "DeepSeek · en direct",
                                        AgentStatus::Listening => "DeepSeek · écoute active (r)...",
                                        AgentStatus::EmergencyStopped => "Système verrouillé",
                                    };

                                    let badge_color = if self.status == AgentStatus::Listening {
                                        egui::Color32::from_rgb(255, 80, 80)
                                    } else {
                                        egui::Color32::from_rgb(0, 200, 255)
                                    };

                                    ui.label(
                                        egui::RichText::new(status_badge)
                                            .size(11.0)
                                            .color(badge_color),
                                    );

                                    if self.is_recording && !self.live_transcript.is_empty() {
                                        ui.add_space(2.0);
                                        ui.add(
                                            egui::Label::new(
                                                egui::RichText::new(&self.live_transcript)
                                                    .size(11.5)
                                                    .color(egui::Color32::from_rgb(255, 220, 130)),
                                            )
                                            .truncate(),
                                        );
                                    }

                                    ui.add_space(4.0);

                                    ui.horizontal(|ui| {
                                        let mic_text = if self.is_recording {
                                            egui::RichText::new("🔴 Stop (r)")
                                                .color(egui::Color32::from_rgb(255, 90, 90))
                                                .strong()
                                        } else {
                                            egui::RichText::new("🎙 Rec (r)")
                                                .color(egui::Color32::from_rgb(180, 220, 255))
                                        };

                                        let mic_btn = ui.button(mic_text).on_hover_text(
                                            "Activer ou désactiver l'écoute instantanée (raccourci r)",
                                        );

                                        if mic_btn.clicked() {
                                            self.toggle_recording(ui.ctx());
                                        }

                                        let edit_width = (ui.available_width() - 124.0).max(80.0);
                                        let input_id = egui::Id::new("prompt_input_text");
                                        let response = ui.add_sized(
                                            [edit_width, 24.0],
                                            egui::TextEdit::singleline(&mut self.input_text)
                                                .id(input_id)
                                                .hint_text(if self.is_recording {
                                                    "Écoute Whisper active... parlez ou tapez"
                                                } else {
                                                    "Consigne d'exploration... (entrée)"
                                                }),
                                        );

                                        let enter_hit = (response.lost_focus() || response.has_focus())
                                            && ctx.input(|i| i.key_pressed(egui::Key::Enter));
                                        if (ui.button("Envoyer").clicked() || enter_hit) && !self.input_text.trim().is_empty() {
                                            if self.is_recording {
                                                self.stop_recording();
                                            }
                                            let _ = self.tts_sender.send(TtsCommand::Stop);
                                            self.continuous_mode = true;
                                            let prompt = std::mem::take(&mut self.input_text).trim().to_string();
                                            self.live_transcript.clear();
                                            self.chat_history.push(ChatEntry {
                                                role: ChatRole::User,
                                                text: prompt.clone(),
                                                timestamp: current_time_str(),
                                                quick_suggestions: Vec::new(),
                                                screen_size_kb: None,
                                            });
                                            if is_hide_command(&prompt) {
                                                self.is_hidden = true;
                                                self.status = AgentStatus::Idle;
                                                let reply = "Overlay masqué. Je reste à l'écoute pour « deepseek ouvre toi ».".to_string();
                                                self.chat_history.push(ChatEntry {
                                                    role: ChatRole::Agent,
                                                    text: reply.clone(),
                                                    timestamp: current_time_str(),
                                                    quick_suggestions: Vec::new(),
                                                    screen_size_kb: None,
                                                });
                                                let _ = self.tts_sender.send(TtsCommand::Speak(reply));
                                            } else {
                                                self.status = AgentStatus::Thinking;
                                                let _ = self.command_sender.send(AgentCommand::Prompt(prompt));
                                            }
                                        }

                                        let clear_btn = ui
                                            .button("🗑")
                                            .on_hover_text("Effacer l'historique et la mémoire contextuelle");
                                        if clear_btn.clicked() {
                                            self.command_sender.send(AgentCommand::ClearHistory).ok();
                                            self.chat_history.clear();
                                            self.chat_history.push(ChatEntry {
                                                role: ChatRole::Agent,
                                                text: "Mémoire contextuelle réinitialisée.".to_string(),
                                                timestamp: current_time_str(),
                                                quick_suggestions: Vec::new(),
                                                screen_size_kb: None,
                                            });
                                            self.live_transcript.clear();
                                            let _ = self.tts_sender.send(TtsCommand::Stop);
                                        }
                                    });
                                });
                            });
                        });
                });

                // Zone de déplacement restreinte à l'en-tête supérieur en donnant priorité aux contrôles
                let drag_rect = egui::Rect::from_min_max(
                    egui::pos2(tab_bar_max_x + 8.0, total_rect.min.y),
                    egui::pos2(total_rect.max.x, total_rect.min.y + 36.0),
                );
                let drag_response = ui.interact(drag_rect, ui.id().with("capsule_drag"), egui::Sense::drag());
                if drag_response.hovered() {
                    ui.ctx().set_cursor_icon(egui::CursorIcon::Grab);
                }

                if drag_response.drag_started() {
                    if let Some(pointer_pos) = ctx.input(|i| i.pointer.latest_pos()) {
                        self.drag_offset = Some(pointer_pos.to_vec2());
                    }
                }

                if drag_response.dragged() {
                    ui.ctx().set_cursor_icon(egui::CursorIcon::Grabbing);
                    let offset = match self.drag_offset {
                        Some(off) => off,
                        None => {
                            let off = ctx.input(|i| i.pointer.latest_pos())
                                .map(|p| p.to_vec2())
                                .unwrap_or(egui::vec2(280.0, 20.0));
                            self.drag_offset = Some(off);
                            off
                        }
                    };

                    let cursor_screen = get_screen_cursor_pos(&ctx);
                    if let Some(cursor) = cursor_screen {
                        if let Some(mon_size) = ctx.input(|i| i.viewport().monitor_size) {
                            let win_w = 560.0;
                            let win_h = 380.0;
                            let max_x = (mon_size.x - win_w).max(0.0);
                            let max_y = (mon_size.y - win_h).max(0.0);

                            let target_x = (cursor.x - offset.x).clamp(0.0, max_x);
                            let target_y = (cursor.y - offset.y).clamp(0.0, max_y);
                            let new_pos = egui::pos2(target_x, target_y);

                            self.window_pos = Some(new_pos);
                            ctx.send_viewport_cmd(egui::ViewportCommand::OuterPosition(new_pos));
                        }
                    }
                }
                if drag_response.drag_stopped() {
                    self.drag_offset = None;
                }
            });
    }
}

fn resolve_deepseek_key() -> String {
    if let Ok(key) = std::env::var("DEEPSEEK_API_KEY") {
        let trimmed = key.trim();
        if !trimmed.is_empty() {
            return trimmed.to_string();
        }
    }

    if let Some(val) = read_env_var_from_file(".env", "DEEPSEEK_API_KEY") {
        return val;
    }

    String::new()
}

fn parse_agent_response(raw: &str) -> AgentResponsePayload {
    let clean = raw.trim();

    if clean.eq_ignore_ascii_case("invalid_request") {
        return AgentResponsePayload {
            narration: String::new(),
            actions: Vec::new(),
            quick_suggestions: Vec::new(),
            invalid_request: true,
        };
    }

    if let Ok(payload) = serde_json::from_str::<AgentResponsePayload>(clean) {
        return payload;
    }

    let unquoted = if let Some(start) = clean.find("```json") {
        let rest = &clean[start + 7..];
        if let Some(end) = rest.rfind("```") {
            rest[..end].trim()
        } else {
            rest.trim()
        }
    } else if let Some(start) = clean.find("```") {
        let rest = &clean[start + 3..];
        if let Some(end) = rest.rfind("```") {
            rest[..end].trim()
        } else {
            rest.trim()
        }
    } else {
        clean
    };

    // 1. Désérialisation directe
    if let Ok(payload) = serde_json::from_str::<AgentResponsePayload>(unquoted) {
        return payload;
    }

    // 2. Extraction du bloc JSON entre la première accolade '{' et la dernière '}'
    if let (Some(start), Some(end)) = (unquoted.find('{'), unquoted.rfind('}')) {
        if start <= end {
            let json_candidate = &unquoted[start..=end];
            if let Ok(payload) = serde_json::from_str::<AgentResponsePayload>(json_candidate) {
                return payload;
            }

            // 3. Repli dynamique : extraction ciblée de la narration et des actions via serde_json::Value
            if let Ok(val) = serde_json::from_str::<serde_json::Value>(json_candidate) {
                let narration = val
                    .get("narration")
                    .and_then(|v| v.as_str())
                    .unwrap_or("")
                    .to_string();

                let invalid_request = val
                    .get("invalid_request")
                    .and_then(|v| v.as_bool())
                    .unwrap_or(false);

                let mut actions = Vec::new();
                if let Some(arr) = val.get("actions").and_then(|v| v.as_array()) {
                    for item in arr {
                        if let Ok(act) = serde_json::from_value::<AgentAction>(item.clone()) {
                            actions.push(act);
                        }
                    }
                }

                let mut quick_suggestions = Vec::new();
                if let Some(arr) = val.get("quick_suggestions").and_then(|v| v.as_array()) {
                    for item in arr {
                        if let Ok(sug) = serde_json::from_value::<QuickSuggestionItem>(item.clone()) {
                            quick_suggestions.push(sug);
                        }
                    }
                }

                if !narration.is_empty() || !actions.is_empty() || invalid_request {
                    return AgentResponsePayload {
                        narration,
                        actions,
                        quick_suggestions,
                        invalid_request,
                    };
                }
            }
        }
    }

    if clean.contains("invalid_request") {
        return AgentResponsePayload {
            narration: String::new(),
            actions: Vec::new(),
            quick_suggestions: Vec::new(),
            invalid_request: true,
        };
    }

    AgentResponsePayload {
        narration: clean.to_string(),
        actions: Vec::new(),
        quick_suggestions: Vec::new(),
        invalid_request: false,
    }
}

static AGENT_BUSY: AtomicBool = AtomicBool::new(false);

struct BusyGuard;
impl Drop for BusyGuard {
    fn drop(&mut self) {
        AGENT_BUSY.store(false, Ordering::SeqCst);
    }
}

async fn compact_history_if_needed(
    api_key: &str,
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

    let summary_request = DeepSeekChatRequest {
        model: "deepseek-chat".to_string(),
        messages: vec![
            ChatMessage {
                role: "system".to_string(),
                content: "Tu es un synthétiseur de mémoire conversationnelle. Résume fidèlement les points clés, décisions et actions passées en 250 mots maximum en français sous forme de points synthétiques.".to_string(),
            },
            ChatMessage {
                role: "user".to_string(),
                content: format!("Voici les échanges passés à condenser :\n\n{}", safe_conversation_text),
            },
        ],
        temperature: 0.2,
        max_tokens: 400,
        top_p: 1.0,
        stream: false,
    };

    let response = client
        .post("https://api.deepseek.com/chat/completions")
        .bearer_auth(api_key)
        .json(&summary_request)
        .send()
        .await;

    *last_call_time = Some(Instant::now());

    if let Ok(res) = response {
        if res.status().is_success() {
            if let Ok(body) = res.json::<DeepSeekChatResponse>().await {
                if let Some(choice) = body.choices.first() {
                    let summary = choice.message.content.trim();
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
    }

    // Repli de sécurité en cas d'erreur de condensation
    println!("[Mémoire] Repli : éviction fifo sans résumé.");
    *history = recent;
}

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
enum PromptTrigger {
    User,
    Surveillance,
    Immersion { screen_changed: bool },
}

async fn call_deepseek_prompt(
    api_key: String,
    user_prompt: String,
    history: Arc<tokio::sync::Mutex<Vec<ChatMessage>>>,
    event_tx: StdSender<AgentEvent>,
    last_call_time: Arc<tokio::sync::Mutex<Option<Instant>>>,
    trigger: PromptTrigger,
    request_id: u64,
) {
    if is_request_cancelled(request_id) {
        return;
    }

    AGENT_BUSY.store(true, Ordering::SeqCst);
    let _busy_guard = BusyGuard;
    let _ = event_tx.send(AgentEvent::StatusChanged(AgentStatus::Thinking));

    if api_key.is_empty() {
        let _ = event_tx.send(AgentEvent::ReplaceNarration {
            text: "Veuillez renseigner DEEPSEEK_API_KEY dans le fichier .env.".into(),
            quick_suggestions: Vec::new(),
        });
        let _ = event_tx.send(AgentEvent::StatusChanged(AgentStatus::Idle));
        return;
    }

    let client = reqwest::Client::new();
    let system_instructions = r#"Tu es l'agent d'exploration Libertide pour Windows.
Réponds STRICTEMENT en JSON sans texte autour. Narration concise en français (max 2 phrases).

Règles d'action et d'arbitrage :
- Silence ("invalid_request": true) : réservé aux bruits de micro et phrases inintelligibles.
- Salutations et discussion : valide ("invalid_request": false), réponds courtoisement dans "narration" avec "actions": [].
- Interface : consulte toujours "[État actuel de l'écran]" (fenêtres, onglets, boutons, liens).
  * Les contrôles de l'interface du navigateur portent le préfixe "[Navigateur]" (ex: "[Navigateur] Barre d'adresse", "[Navigateur] Actualiser"). Pour interagir avec le navigateur hôte, inclus explicitement ce préfixe dans la cible.
  * Les éléments internes au site web (champs de formulaire, boutons d'action, liens) figurent dans "[Page web - ...]" sans préfixe. Cible-les par leur libellé direct (ex: "Destination", "Rechercher", "Tout accepter").
- Fermeture d'onglets ou sous-pages (ex: "ferme Google") : JAMAIS "close_app" ! Enchaîne "focus_window" puis "accessibility_shortcut": "close_tab" ou clique la croix ("click_button" / "click_element"). Réserve "close_app" à la fermeture d'une application entière.
- Détection d'obstruction et éléments surgissants (bannières cookies, popups, notifications Windows) :
  * Analyse systématiquement [État actuel de l'écran] pour repérer toute notification Windows, toast, modale ou bannière de consentement qui masque l'écran ou demande validation.
  * Sélection impérative du sous-bouton d'action ou de fermeture : n'envoie JAMAIS "escape" ! Identifie et clique directement sur le sous-bouton approprié présent dans [Boutons / contrôles cliquables] ou [Liens / résultats cliquables] :
    - Bannières de cookies / RGPD : sous-bouton "Tout accepter", "Accepter", "J'accepte", "Autoriser", "Continuer sans accepter" ou croix de fermeture.
    - Notifications Windows et toasts système : sous-bouton "Fermer", "Ignorer", "Supprimer", "Dismiss" ou le bouton d'action contextuel de la notification.
    - Popups, modales et dialogues d'application : sous-bouton "Fermer", "Annuler", "Plus tard", "Non merci" ou bouton de rejet.
  * Dès que l'obstacle n'apparaît plus à l'écran, reprends immédiatement le cours de la consigne initiale là où elle s'était arrêtée.
- Éléments cliquables et saisie : utilise "click_element" pour les liens et onglets, "click_button" pour les boutons. Pour saisir sans cible précise, mets "target": null dans "write_text" ou "replace_field_text".
- Défilement de page et scrollbar ("scroll") :
  * L'analyse d'écran intègre automatiquement un autoscroll par tranches de viewport (jusqu'à 10 chunks) pour agréger l'ensemble des éléments de la page.
  * Si une barre de défilement est signalée ou si tu as besoin de positionner la vue sur un conteneur précis, utilise "scroll" ("direction": "down" | "up" | "bottom" | "top", "amount": 1 ou 2).
  * Tu réévalueras le nouvel état de l'écran à l'étape suivante pour cliquer ou interagir avec les éléments révélés.
- Navigation et shopping autonome : génère l'URL pertinente (ex: "https://www.google.com/search?tbm=shop&q=..." pour shopping) avec "navigate_to_url".
  * Achat multi-étapes : 1. Ajoute au panier ("click_element" ou "click_button") -> 2. Ouvre le panier / commande ("click_element") -> 3. Devant le paiement, stoppe toute action et demande confirmation orale dans "narration" avec "actions": [].
  * Recherche exploratoire : dès l'affichage des résultats, résume 2-3 options observées et pose une question de cadrage (budget, dimensions) dans "narration" avec "actions": [].
- Suggestions rapides ("quick_suggestions") : quand tu poses une question ou suggères des options, propose 2 à 4 choix brefs sous forme de tableau de chaînes textuelles dans "quick_suggestions" (ex: ["Tennis de course, 80-120€", "Modèle lifestyle"]). Ces options seront affichées sous forme de boutons-onglets directement cliquables par l'utilisateur.
- Immersion : "activate_immersion" pour créer le bureau virtuel (Win+Ctrl+D). Silence absolu lors du suivi périodique si l'activité est normale. "deactivate_immersion" pour le fermer.
- Cycle d'exécution et fin de mission : analyse le retour d'étape et l'état d'écran. Dès que la tâche est finie ou nécessite une précision de l'utilisateur, renvoie "actions": [].
  * OBLIGATION DE FIN DE PASSES : Dès que "actions" est vide (tâche terminée ou attente), fournis TOUJOURS dans "quick_suggestions" 2 à 4 suggestions contextuelles concrètes basées sur ton analyse de l'écran et des éléments observés (ex: produits aperçus, avis, défilement, choix suivants).

Format json obligatoire :
{
  "narration": "Explication vocale en français (2 phrases max ou vide pour silence).",
  "actions": [
    {"action": "open_app", "name": "nom_executable"},
    {"action": "close_app", "name": "nom_ou_titre"},
    {"action": "click_element", "window": "titre_optionnel", "target_name": "nom_du_lien_ou_element"},
    {"action": "click_button", "window": "titre_optionnel", "button_name": "nom_du_bouton"},
    {"action": "focus_element", "window": "titre_optionnel", "target_name": "nom_ou_id_element"},
    {"action": "scroll", "direction": "down" | "up" | "bottom" | "top", "amount": 1, "window": "titre_optionnel"},
    {"action": "focus_window", "title": "titre_optionnel", "pid": 1234},
    {"action": "clear_text", "window": "titre_optionnel"},
    {"action": "write_text", "text": "texte à écrire", "target": null},
    {"action": "replace_field_text", "text": "texte complet modifié", "target": null},
    {"action": "run_command", "command": "commande_ou_outil"},
    {"action": "kill_process", "pid": 1234, "name": "app.exe"},
    {"action": "open_browser", "url": "https://..."},
    {"action": "navigate_to_url", "url": "https://..."},
    {"action": "tile_windows", "layout": "split_horizontal" | "split_vertical" | "grid_2x2" | "master_stack", "windows": ["titre_fenetre_1", "titre_fenetre_2"]},
    {"action": "arrange_window", "title": "mot_cle", "position": "left" | "right" | "top" | "bottom" | "center" | "maximize" | "minimize"},
    {"action": "move_window", "title": "mot_cle", "x": 0, "y": 0, "width": 960, "height": 1040},
    {"action": "accessibility_shortcut", "shortcut": "close_tab" | "snap_left" | "snap_right" | "snap_up" | "snap_down" | "task_manager" | "undo" | "copy" | "refresh_page"},
    {"action": "summarize_screen", "window": "titre_optionnel"},
    {"action": "activate_immersion", "apps": ["code"], "urls": ["https://..."], "layout": "grid_2x2"},
    {"action": "deactivate_immersion"}
  ],
  "quick_suggestions": ["Suggestion 1", "Suggestion 2"]
}"#;

    let safe_user_prompt = truncate_with_notice(&user_prompt, 4000);
    {
        let mut hist_guard = history.lock().await;
        hist_guard.push(ChatMessage {
            role: "user".to_string(),
            content: safe_user_prompt.clone(),
        });
    }

    const MAX_AGENT_PASSES: usize = 5;

    for pass in 1..=MAX_AGENT_PASSES {
        if is_request_cancelled(request_id) {
            return;
        }
        let _ = event_tx.send(AgentEvent::StatusChanged(AgentStatus::Thinking));

        // Compaction progressive de la mémoire si l'historique dépasse le seuil
        {
            let mut hist_guard = history.lock().await;
            let mut time_guard = last_call_time.lock().await;
            compact_history_if_needed(&api_key, &mut hist_guard, &client, &mut time_guard).await;
        }

        if is_request_cancelled(request_id) {
            return;
        }

        // Debounce : pause minimale entre deux requêtes API
        let min_debounce = if pass == 1 { Duration::from_millis(3000) } else { Duration::from_millis(2000) };
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

        let mut messages = Vec::new();
        messages.push(ChatMessage {
            role: "system".to_string(),
            content: system_instructions.to_string(),
        });
        messages.extend(history.lock().await.iter().cloned());

        let request = DeepSeekChatRequest {
            model: "deepseek-chat".to_string(),
            messages,
            temperature: 1.0,
            max_tokens: 600,
            top_p: 1.0,
            stream: false,
        };

        let mut attempts = 0;
        const MAX_RETRIES: usize = 3;
        const DEFAULT_RETRY_DELAY: Duration = Duration::from_millis(4000);
        let mut success_payload: Option<AgentResponsePayload> = None;

        loop {
            if is_request_cancelled(request_id) {
                return;
            }
            attempts += 1;

            let response = client
                .post("https://api.deepseek.com/chat/completions")
                .bearer_auth(&api_key)
                .json(&request)
                .send()
                .await;

            match response {
                Ok(res) if res.status().is_success() => {
                    {
                        let mut time_guard = last_call_time.lock().await;
                        *time_guard = Some(Instant::now());
                    }
                    if let Ok(body) = res.json::<DeepSeekChatResponse>().await {
                        if let Some(choice) = body.choices.first() {
                            let raw_content = &choice.message.content;
                            println!("\n=================== [Réponse deepseek brute (passe {}/{})] ===================", pass, MAX_AGENT_PASSES);
                            println!("{}", raw_content.trim());
                            println!("============================================================");

                            history.lock().await.push(ChatMessage {
                                role: "assistant".to_string(),
                                content: raw_content.clone(),
                            });

                            let payload = parse_agent_response(raw_content);
                            success_payload = Some(payload);
                            break;
                        }
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
                        println!("[DeepSeek 429] Détails renvoyés par l'API : {}", err_body);
                        history.lock().await.pop();
                        let mins = (wait_secs + 59) / 60;
                        if !is_request_cancelled(request_id) {
                            let _ = event_tx.send(AgentEvent::ReplaceNarration {
                                text: format!("Plafond instantané DeepSeek atteint (pause requise par l'API : {mins} min). Historique réinitialisé."),
                                quick_suggestions: Vec::new(),
                            });
                        }
                        history.lock().await.clear();
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
                            text: format!("Erreur api deepseek : {status}"),
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

        if payload.invalid_request || payload.narration.trim().eq_ignore_ascii_case("invalid_request") {
            println!("[Agent] Requête incomplète ou incomprise : mode silencieux activé (aucun affichage ni TTS).");
            history.lock().await.pop();
            let _ = event_tx.send(AgentEvent::StatusChanged(AgentStatus::Idle));
            let _ = event_tx.send(AgentEvent::RequestIgnored);
            return;
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
            println!("[Agent] Tâche accomplie : aucune action supplémentaire. Fin de la séquence après {} passe(s).", pass);
            if payload.quick_suggestions.is_empty() {
                let screen = {
                    LAST_SCREEN_SUMMARY.lock().ok().and_then(|s| s.clone()).unwrap_or_else(|| {
                        #[cfg(windows)]
                        { summarize_screen_state(None, false) }
                        #[cfg(not(windows))]
                        { String::new() }
                    })
                };
                payload.quick_suggestions = generate_fallback_suggestions_from_screen(&screen);
            }
            if !payload.narration.trim().is_empty() || !payload.quick_suggestions.is_empty() {
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
            let _ = event_tx.send(AgentEvent::ReplaceNarration {
                text: payload.narration.clone(),
                quick_suggestions: payload.quick_suggestions.clone(),
            });
        }

        let actions_to_run = payload.actions;
        let (report, screen_after) = tokio::task::spawn_blocking(move || {
            if is_request_cancelled(request_id) {
                return ("Actions interrompues.".to_string(), String::new());
            }
            let outcome = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
                let rep = execute_system_actions(&actions_to_run, request_id);
                if is_request_cancelled(request_id) {
                    return (rep, String::new());
                }
                std::thread::sleep(Duration::from_millis(1200));
                if is_request_cancelled(request_id) {
                    return (rep, String::new());
                }
                #[cfg(windows)]
                let sc = summarize_screen_state(None, false);
                #[cfg(not(windows))]
                let sc = String::new();
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

        if !screen_after.trim().is_empty() {
            if let Ok(mut lock) = LAST_SCREEN_SUMMARY.lock() {
                *lock = Some(screen_after.clone());
            }
        }

        if pass < MAX_AGENT_PASSES {
            let compact_report = truncate_with_notice(&report, 1200);
            let mut step_feedback = format!("[Retour d'exécution étape {pass}] :\n{compact_report}");
            if !screen_after.trim().is_empty() {
                let safe_screen = truncate_with_notice(screen_after.trim(), 3500);
                step_feedback.push_str(&format!("\n\n[État de l'écran suite aux actions] :\n{safe_screen}"));
            }

            step_feedback.push_str(&format!(
                "\n\nConsignes pour cette nouvelle étape :\n\
                - Analyse attentivement [État de l'écran suite aux actions].\n\
                - Pour cibler un contrôle de l'application hôte (navigateur), utilise le libellé commençant par '[Navigateur]'. Pour la page web, utilise le nom direct affiché dans les sections '[Page web - ...]'.\n\
                - Détection d'obstacle : si une notification Windows, un popup ou une bannière de consentement obstrue la vue, sélectionne et clique immédiatement sur le sous-bouton approprié de la notification ('click_button' ou 'click_element' vers 'Tout accepter', 'Accepter', 'Fermer', 'Ignorer', etc.). Ne jamais envoyer 'escape'.\n\
                - Si l'élément cible n'est pas encore visible sur la page, émets un défilement ('scroll' direction: 'down') pour explorer le reste de la page.\n\
                - Si l'écran est dégagé, poursuis immédiatement l'exécution de la consigne initiale : \"{}\".\n\
                - Pour une recherche ou un achat web : relève les modèles ou prix observés et formule 2 ou 3 suggestions concrètes dans \"narration\" si un choix utilisateur est nécessaire.\n\
                - Si la tâche est terminée ou attend un choix, confirme-le dans \"narration\" avec \"actions\": [] et fournis impérativement 2 à 4 suggestions contextuelles dans \"quick_suggestions\" basées sur ton analyse des éléments à l'écran.",
                safe_user_prompt
            ));

            history.lock().await.push(ChatMessage {
                role: "user".to_string(),
                content: step_feedback,
            });
            // Marquer la fin de l'exécution pour que le debounce de la passe suivante s'applique bien
            {
                let mut time_guard = last_call_time.lock().await;
                *time_guard = Some(Instant::now());
            }
        } else {
            println!("[Agent] Nombre maximal de passes ({MAX_AGENT_PASSES}) atteint.");
            let screen_for_sug = if !screen_after.trim().is_empty() {
                screen_after.clone()
            } else {
                LAST_SCREEN_SUMMARY.lock().ok().and_then(|s| s.clone()).unwrap_or_default()
            };
            let auto_suggestions = generate_fallback_suggestions_from_screen(&screen_for_sug);
            let _ = event_tx.send(AgentEvent::ReplaceNarration {
                text: "Actions terminées. Voici les suites possibles identifiées à l'écran :".to_string(),
                quick_suggestions: auto_suggestions,
            });
        }
    }

    if !is_request_cancelled(request_id) {
        let _ = event_tx.send(AgentEvent::StatusChanged(AgentStatus::Idle));
    }
}

fn main() -> eframe::Result<()> {
    let (event_tx, event_rx) = channel::<AgentEvent>();
    let (cmd_tx, mut cmd_rx) = unbounded_channel::<AgentCommand>();
    let tts_tx = spawn_tts_worker(event_tx.clone());
    let (twitch_ch_tx, twitch_ch_rx) = channel::<String>();
    spawn_twitch_worker(event_tx.clone(), twitch_ch_rx);
    let deepseek_key = resolve_deepseek_key();
    let audio_tx = spawn_audio_worker(event_tx.clone(), deepseek_key.clone());

    let initial_twitch_channel = std::env::var("TWITCH_CHANNEL").unwrap_or_default();

    // Horloge d'analyse périodique d'écran toutes les 60 secondes
    let periodic_cmd_tx = cmd_tx.clone();
    std::thread::spawn(move || {
        loop {
            std::thread::sleep(Duration::from_secs(60));
            let _ = periodic_cmd_tx.send(AgentCommand::PeriodicScreenCheck);
        }
    });

    // Runtime Tokio en arrière-plan pour requêter DeepSeek
    let deepseek_chat_key = deepseek_key.clone();
    let bg_tts_tx = tts_tx.clone();
    std::thread::spawn(move || {
        let rt = tokio::runtime::Runtime::new().expect("Échec d'initialisation du runtime Tokio");
        rt.block_on(async move {
            let history = Arc::new(tokio::sync::Mutex::new(Vec::<ChatMessage>::new()));
            let last_call_time = Arc::new(tokio::sync::Mutex::new(None));
            let mut current_task: Option<tokio::task::JoinHandle<()>> = None;

            while let Some(cmd) = cmd_rx.recv().await {
                match cmd {
                    AgentCommand::Prompt(prompt) => {
                        let request_id = CURRENT_REQUEST_ID.fetch_add(1, Ordering::SeqCst) + 1;
                        if let Some(handle) = current_task.take() {
                            handle.abort();
                        }
                        let _ = bg_tts_tx.send(TtsCommand::Stop);
                        IS_EMERGENCY_STOPPED.store(false, Ordering::SeqCst);

                        let history = history.clone();
                        let last_call_time = last_call_time.clone();
                        let event_tx = event_tx.clone();
                        let deepseek_chat_key = deepseek_chat_key.clone();

                        current_task = Some(tokio::spawn(async move {
                            if is_request_cancelled(request_id) {
                                return;
                            }
                            if let Some(cli_feedback) = try_execute_direct_cli(&prompt) {
                                if is_request_cancelled(request_id) { return; }
                                let _ = event_tx.send(AgentEvent::ScreenPayloadSize(0.0));
                                let _ = event_tx.send(AgentEvent::ReplaceNarration {
                                    text: cli_feedback.clone(),
                                    quick_suggestions: Vec::new(),
                                });
                                let _ = event_tx.send(AgentEvent::StatusChanged(AgentStatus::Idle));
                            } else if let Some(text_to_write) = parse_write_command(&prompt) {
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
                            } else {
                                let (field_content, screen_state) = tokio::task::spawn_blocking(|| {
                                    #[cfg(windows)]
                                    {
                                        (get_active_field_content(), summarize_screen_state(None, false))
                                    }
                                    #[cfg(not(windows))]
                                    {
                                        (None, String::new())
                                    }
                                }).await.unwrap_or((None, String::new()));

                                if is_request_cancelled(request_id) { return; }

                                let mut prompt_sections = Vec::new();

                                if !screen_state.trim().is_empty() {
                                    if let Ok(mut lock) = LAST_SCREEN_SUMMARY.lock() {
                                        *lock = Some(screen_state.clone());
                                    }
                                    let safe_screen = truncate_with_notice(screen_state.trim(), 3800);
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

                                prompt_sections.push(format!("[Demande utilisateur]\n{prompt}"));
                                let final_prompt = prompt_sections.join("\n\n");

                                call_deepseek_prompt(
                                    deepseek_chat_key,
                                    final_prompt,
                                    history,
                                    event_tx,
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
                        IS_EMERGENCY_STOPPED.store(false, Ordering::SeqCst);
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
                        if IS_EMERGENCY_STOPPED.load(Ordering::SeqCst) {
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
                        let deepseek_chat_key = deepseek_chat_key.clone();

                        current_task = Some(tokio::spawn(async move {
                        #[cfg(windows)]
                        {
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
                                call_deepseek_prompt(deepseek_chat_key, immersion_prompt, history, event_tx, last_call_time, PromptTrigger::Immersion { screen_changed: true }, request_id).await;
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

                                call_deepseek_prompt(deepseek_chat_key, diff_prompt, history, event_tx, last_call_time, PromptTrigger::Surveillance, request_id).await;
                            } else {
                                println!("[Surveillance] Premier instantané d'écran enregistré.");
                            }
                        }
                        }));
                    }
                }
            }
        });
    });

    // Fenêtre transparente compacte, ferrée en bas à droite
    let native_options = eframe::NativeOptions {
        renderer: eframe::Renderer::Glow,
        viewport: egui::ViewportBuilder::default()
            .with_decorations(false)
            .with_transparent(true)
            .with_always_on_top()
            .with_resizable(false)
            .with_inner_size([560.0, 380.0]),
        ..Default::default()
    };

    eframe::run_native(
        "Libertide overlay",
        native_options,
        Box::new(move |cc| {
            Ok(Box::new(OverlayApp::new(
                cc,
                event_rx,
                cmd_tx,
                tts_tx,
                audio_tx,
                twitch_ch_tx,
                initial_twitch_channel,
            )))
        }),
    )
}