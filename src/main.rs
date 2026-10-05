use eframe::egui;
use serde::{Deserialize, Serialize};
use std::io::Cursor;
use std::sync::{Arc, Mutex};
use std::sync::mpsc::{channel, Receiver, Sender};
use std::time::{Duration, Instant};

use cpal::traits::{DeviceTrait, HostTrait, StreamTrait};

#[cfg(windows)]
use std::os::windows::process::CommandExt;
#[cfg(windows)]
use windows::core::{w, Interface};
#[cfg(windows)]
use windows::Win32::Foundation::{BOOL, HWND, LPARAM, POINT, RECT, WPARAM};
#[cfg(windows)]
use windows::Win32::Media::Speech::{ISpVoice, SpVoice, SPF_ASYNC, SPF_PURGEBEFORESPEAK, SPVOICESTATUS};
#[cfg(windows)]
use windows::Win32::System::Com::{CoCreateInstance, CoInitializeEx, CLSCTX_ALL, CLSCTX_INPROC_SERVER, COINIT_MULTITHREADED};
#[cfg(windows)]
use windows::Win32::UI::Accessibility::{
    CUIAutomation, IUIAutomation, IUIAutomationElement, IUIAutomationInvokePattern,
    TreeScope_Descendants, UIA_DocumentControlTypeId, UIA_EditControlTypeId, UIA_InvokePatternId,
};
#[cfg(windows)]
use windows::Win32::UI::Input::KeyboardAndMouse::GetActiveWindow;
#[cfg(windows)]
use windows::Win32::UI::Input::KeyboardAndMouse::{
    mouse_event, SendInput, INPUT, INPUT_0, INPUT_KEYBOARD, KEYBDINPUT,
    KEYBD_EVENT_FLAGS, KEYEVENTF_KEYUP, KEYEVENTF_UNICODE, MOUSEEVENTF_LEFTDOWN,
    MOUSEEVENTF_LEFTUP, VIRTUAL_KEY,
};
#[cfg(windows)]
use windows::Win32::UI::WindowsAndMessaging::{
    EnumWindows, FindWindowW, GetClassNameW, GetCursorPos, GetForegroundWindow, GetSystemMetrics,
    GetWindowLongPtrW, GetWindowRect, GetWindowTextLengthW, GetWindowTextW, IsWindow, IsWindowVisible,
    PostMessageW, SetCursorPos, SetForegroundWindow, SetWindowLongPtrW, SetWindowPos, ShowWindow,
    GWL_EXSTYLE, SM_CXSCREEN, SM_CYSCREEN, SWP_NOZORDER, SWP_SHOWWINDOW, SW_MAXIMIZE,
    SW_RESTORE, WM_CLOSE,
};

#[cfg(windows)]
mod clipboard {
    use std::ffi::c_void;

    extern "system" {
        fn OpenClipboard(hwnd: *mut c_void) -> i32;
        fn CloseClipboard() -> i32;
        fn EmptyClipboard() -> i32;
        fn SetClipboardData(uformat: u32, hmem: *mut c_void) -> *mut c_void;
        fn GlobalAlloc(uflags: u32, dwbytes: usize) -> *mut c_void;
        fn GlobalLock(hmem: *mut c_void) -> *mut c_void;
        fn GlobalUnlock(hmem: *mut c_void) -> i32;
    }

    const CF_UNICODETEXT: u32 = 13;
    const GMEM_MOVEABLE: u32 = 0x0002;

    pub fn set_text(text: &str) -> bool {
        let utf16: Vec<u16> = text.encode_utf16().chain(std::iter::once(0)).collect();
        let bytes_len = utf16.len() * std::mem::size_of::<u16>();
        unsafe {
            let mut opened = false;
            for _ in 0..10 {
                if OpenClipboard(std::ptr::null_mut()) != 0 {
                    opened = true;
                    break;
                }
                std::thread::sleep(std::time::Duration::from_millis(20));
            }
            if !opened {
                return false;
            }

            EmptyClipboard();
            let h_mem = GlobalAlloc(GMEM_MOVEABLE, bytes_len);
            if !h_mem.is_null() {
                let p_data = GlobalLock(h_mem) as *mut u16;
                if !p_data.is_null() {
                    std::ptr::copy_nonoverlapping(utf16.as_ptr(), p_data, utf16.len());
                    GlobalUnlock(h_mem);
                    SetClipboardData(CF_UNICODETEXT, h_mem);
                }
            }
            CloseClipboard();
        }
        true
    }
}

#[cfg(windows)]
fn send_paste() {
    let keys = [
        (VIRTUAL_KEY(0x11), KEYBD_EVENT_FLAGS(0)), // Ctrl enfoncé
        (VIRTUAL_KEY(0x56), KEYBD_EVENT_FLAGS(0)), // V enfoncé
        (VIRTUAL_KEY(0x56), KEYEVENTF_KEYUP),      // V relâché
        (VIRTUAL_KEY(0x11), KEYEVENTF_KEYUP),      // Ctrl relâché
    ];
    let mut inputs = Vec::new();
    for (vk, flags) in keys {
        inputs.push(INPUT {
            r#type: INPUT_KEYBOARD,
            Anonymous: INPUT_0 {
                ki: KEYBDINPUT {
                    wVk: vk,
                    wScan: 0,
                    dwFlags: flags,
                    time: 0,
                    dwExtraInfo: 0,
                },
            },
        });
    }
    unsafe {
        SendInput(&inputs, std::mem::size_of::<INPUT>() as i32);
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum AgentStatus {
    Idle,
    Thinking,
    Speaking,
    Listening,
    EmergencyStopped,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ChatRole {
    User,
    Agent,
}

#[derive(Debug, Clone)]
pub struct ChatEntry {
    pub role: ChatRole,
    pub text: String,
}

#[derive(Debug, Clone)]
pub enum AgentEvent {
    StatusChanged(AgentStatus),
    NarrationChunk(String),
    ReplaceNarration(String),
    TranscriptionPartial(String),
    VoicePromptReady(String),
    TtsFinished,
}

#[derive(Debug, Clone)]
pub enum AudioCommand {
    Start,
    Stop,
}

#[derive(Debug, Clone)]
pub enum TtsCommand {
    Speak(String),
    Stop,
}

#[derive(Debug, Clone)]
pub enum AgentCommand {
    Prompt(String),
    ClearHistory,
}

#[derive(Serialize, Deserialize, Debug, Clone)]
#[serde(tag = "action", rename_all = "snake_case")]
pub enum AgentAction {
    OpenApp {
        name: String,
    },
    CloseApp {
        name: String,
    },
    ClickButton {
        window: Option<String>,
        button_name: String,
    },
    OpenBrowser {
        url: Option<String>,
    },
    WriteText {
        text: String,
    },
    ArrangeWindow {
        title: String,
        position: String,
    },
    TileWindows {
        layout: Option<String>,
        windows: Vec<String>,
    },
    MoveWindow {
        title: String,
        x: i32,
        y: i32,
        width: i32,
        height: i32,
    },
}

#[derive(Serialize, Deserialize, Debug, Clone)]
pub struct AgentResponsePayload {
    pub narration: String,
    #[serde(default)]
    pub actions: Vec<AgentAction>,
}

#[derive(Serialize, Deserialize, Debug, Clone)]
struct ChatMessage {
    role: String,
    content: String,
}

#[derive(Serialize, Debug)]
struct GroqChatRequest {
    model: String,
    messages: Vec<ChatMessage>,
    temperature: f32,
    max_completion_tokens: u32,
    top_p: f32,
    stream: bool,
}

#[derive(Deserialize, Debug)]
struct GroqChoice {
    message: ChatMessage,
}

#[derive(Deserialize, Debug)]
struct GroqChatResponse {
    choices: Vec<GroqChoice>,
}

pub struct OverlayApp {
    status: AgentStatus,
    chat_history: Vec<ChatEntry>,
    live_transcript: String,
    input_text: String,
    is_recording: bool,
    continuous_mode: bool,

    // Canaux de communication asynchrones
    event_receiver: Receiver<AgentEvent>,
    command_sender: Sender<AgentCommand>,
    tts_sender: Sender<TtsCommand>,
    audio_sender: Sender<AudioCommand>,
    position_initialized: bool,
    window_pos: Option<egui::Pos2>,
    drag_offset: Option<egui::Vec2>,
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

#[cfg(windows)]
fn spawn_tts_worker(event_tx: Sender<AgentEvent>) -> Sender<TtsCommand> {
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
                                    break;
                                }
                            }
                        }
                    }
                }
            }
        }
    });

    tx
}

#[cfg(not(windows))]
fn spawn_tts_worker(_event_tx: Sender<AgentEvent>) -> Sender<TtsCommand> {
    let (tx, _rx) = channel::<TtsCommand>();
    tx
}

impl OverlayApp {
    pub fn new(
        _cc: &eframe::CreationContext<'_>,
        event_receiver: Receiver<AgentEvent>,
        command_sender: Sender<AgentCommand>,
        tts_sender: Sender<TtsCommand>,
        audio_sender: Sender<AudioCommand>,
    ) -> Self {
        let initial_subtitle = "En attente d'instructions d'exploration...".to_string();
        let _ = tts_sender.send(TtsCommand::Speak(initial_subtitle.clone()));

        Self {
            status: AgentStatus::Idle,
            chat_history: vec![ChatEntry {
                role: ChatRole::Agent,
                text: initial_subtitle,
            }],
            live_transcript: String::new(),
            input_text: String::new(),
            is_recording: false,
            continuous_mode: false,
            event_receiver,
            command_sender,
            tts_sender,
            audio_sender,
            position_initialized: false,
            window_pos: None,
            drag_offset: None,
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

    fn trigger_emergency_stop(&mut self, ctx: &egui::Context) {
        self.continuous_mode = false;
        self.is_recording = false;
        let _ = self.audio_sender.send(AudioCommand::Stop);
        self.live_transcript.clear();
        self.status = AgentStatus::EmergencyStopped;
        let stop_msg = "Arrêt d'urgence : contrôle rendu à l'utilisateur.".to_string();
        self.chat_history.push(ChatEntry {
            role: ChatRole::Agent,
            text: stop_msg.clone(),
        });
        let _ = self.tts_sender.send(TtsCommand::Stop);
        let _ = self.tts_sender.send(TtsCommand::Speak(stop_msg));
    }

    fn start_recording(&mut self) {
        if self.status == AgentStatus::EmergencyStopped {
            return;
        }
        self.continuous_mode = true;
        self.is_recording = true;
        self.status = AgentStatus::Listening;
        self.input_text.clear();
        let _ = self.tts_sender.send(TtsCommand::Stop);
        self.live_transcript = "Écoute en direct... parlez, la retranscription s'affiche en temps réel.".to_string();
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

    fn toggle_recording(&mut self) {
        if self.is_recording {
            self.stop_recording();
        } else {
            self.start_recording();
        }
    }
}

impl eframe::App for OverlayApp {
    fn clear_color(&self, _visuals: &egui::Visuals) -> [f32; 4] {
        // Fond transparent pour laisser transparaître l'arrière-plan sur les 70 % supérieurs
        [0.0, 0.0, 0.0, 0.0]
    }

    fn update(&mut self, ctx: &egui::Context, _frame: &mut eframe::Frame) {
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
        let cursor_screen = get_screen_cursor_pos(ctx);
        let wants_interaction = if self.drag_offset.is_some() {
            true
        } else if let Some(cursor) = cursor_screen {
            let rel_x = cursor.x - win_pos.x;
            let rel_y = cursor.y - win_pos.y;
            let in_window_x = rel_x >= 0.0 && rel_x <= win_w;
            let in_drag_header = in_window_x && rel_y >= 0.0 && rel_y <= 28.0;
            let in_control_panel = in_window_x && rel_y >= (response_h + 4.0) && rel_y <= win_h;
            in_drag_header || in_control_panel
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
                AgentEvent::NarrationChunk(chunk) => {
                    if let Some(last) = self.chat_history.last_mut() {
                        if matches!(last.role, ChatRole::Agent) {
                            last.text.push_str(&chunk);
                        } else {
                            self.chat_history.push(ChatEntry {
                                role: ChatRole::Agent,
                                text: chunk,
                            });
                        }
                    } else {
                        self.chat_history.push(ChatEntry {
                            role: ChatRole::Agent,
                            text: chunk,
                        });
                    }
                }
                AgentEvent::ReplaceNarration(full_text) => {
                    self.chat_history.push(ChatEntry {
                        role: ChatRole::Agent,
                        text: full_text.clone(),
                    });
                    let _ = self.tts_sender.send(TtsCommand::Speak(full_text));
                }
                AgentEvent::TranscriptionPartial(text) => {
                    if self.is_recording {
                        self.input_text = text.clone();
                        self.live_transcript = text;
                    }
                }
                AgentEvent::VoicePromptReady(prompt) => {
                    self.is_recording = false;
                    self.live_transcript.clear();
                    if self.status != AgentStatus::Thinking && !prompt.trim().is_empty() {
                        self.status = AgentStatus::Thinking;
                        self.input_text.clear();
                        self.chat_history.push(ChatEntry {
                            role: ChatRole::User,
                            text: prompt.clone(),
                        });
                        let _ = self.command_sender.send(AgentCommand::Prompt(prompt));
                    }
                }
                AgentEvent::TtsFinished => {
                    if self.continuous_mode && self.status != AgentStatus::EmergencyStopped {
                        self.start_recording();
                    }
                }
            }
        }

        // Raccourci clavier d'urgence global (Échap)
        if ctx.input(|i| i.key_pressed(egui::Key::Escape)) {
            self.trigger_emergency_stop(ctx);
        }

        // Raccourci clavier 'R' pour basculer le micro (autorisé pour couper le micro même avec focus)
        let is_typing = ctx.memory(|m| m.focused().is_some()) && !self.is_recording;
        if !is_typing && ctx.input(|i| i.key_pressed(egui::Key::R)) {
            self.toggle_recording();
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

        egui::CentralPanel::default()
            .frame(egui::Frame::none().fill(egui::Color32::TRANSPARENT))
            .show(ctx, |ui| {
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
                ui.allocate_ui_at_rect(response_rect, |ui| {
                    egui::Frame::none()
                        .fill(egui::Color32::from_rgba_unmultiplied(16, 18, 24, 45))
                        .stroke(egui::Stroke::new(1.0, egui::Color32::from_rgba_unmultiplied(0, 180, 255, 55)))
                        .rounding(14.0)
                        .inner_margin(egui::Margin::symmetric(14.0, 10.0))
                        .show(ui, |ui| {
                            egui::ScrollArea::vertical()
                                .stick_to_bottom(true)
                                .auto_shrink([false, false])
                                .show(ui, |ui| {
                                    for entry in &self.chat_history {
                                        match entry.role {
                                            ChatRole::User => {
                                                ui.horizontal_wrapped(|ui| {
                                                    ui.add(
                                                        egui::Label::new(
                                                            egui::RichText::new(format!("🗣 {}", entry.text))
                                                                .size(13.0)
                                                                .line_height(Some(18.0))
                                                                .color(egui::Color32::from_rgb(255, 220, 130))
                                                                .strong(),
                                                        )
                                                        .wrap(),
                                                    );
                                                });
                                            }
                                            ChatRole::Agent => {
                                                ui.horizontal_wrapped(|ui| {
                                                    ui.add(
                                                        egui::Label::new(
                                                            egui::RichText::new(&entry.text)
                                                                .size(13.5)
                                                                .line_height(Some(19.0))
                                                                .color(egui::Color32::from_rgb(240, 245, 255)),
                                                        )
                                                        .wrap(),
                                                    );
                                                });
                                            }
                                        }
                                        ui.add_space(4.0);
                                    }
                                });
                        });
                });

                // 2. Zone inférieure (30 %) : contrôles, avatar et saisie
                ui.allocate_ui_at_rect(control_rect, |ui| {
                    egui::Frame::none()
                        .fill(egui::Color32::from_rgb(18, 20, 26))
                        .stroke(egui::Stroke::new(1.0, egui::Color32::from_rgba_unmultiplied(0, 180, 255, 75)))
                        .rounding(14.0)
                        .inner_margin(egui::Margin::symmetric(14.0, 10.0))
                        .show(ui, |ui| {
                            let inner_rect = ui.available_rect_before_wrap();
                            let avatar_pos = egui::pos2(inner_rect.max.x - 22.0, inner_rect.min.y + 24.0);
                            self.draw_avatar(ui, avatar_pos, 15.0, time);

                            let content_area = egui::Rect::from_min_max(
                                inner_rect.min,
                                egui::pos2(inner_rect.max.x - 48.0, inner_rect.max.y),
                            );

                            ui.allocate_ui_at_rect(content_area, |ui| {
                                ui.vertical(|ui| {
                                    let status_badge = match self.status {
                                        AgentStatus::Idle => "Groq · en veille",
                                        AgentStatus::Thinking => "Groq · réflexion...",
                                        AgentStatus::Speaking => "Groq · en direct",
                                        AgentStatus::Listening => "Groq · écoute active (r)...",
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
                                            self.toggle_recording();
                                        }

                                        let edit_width = (ui.available_width() - 124.0).max(80.0);
                                        let response = ui.add_sized(
                                            [edit_width, 24.0],
                                            egui::TextEdit::singleline(&mut self.input_text)
                                                .hint_text(if self.is_recording {
                                                    "Écoute active... saisissez ou validez"
                                                } else {
                                                    "Consigne d'exploration... (entrée)"
                                                }),
                                        );

                                        let enter_hit = (response.lost_focus() || response.has_focus())
                                            && ctx.input(|i| i.key_pressed(egui::Key::Enter));
                                        if (ui.button("Envoyer").clicked() || enter_hit) && !self.input_text.trim().is_empty() {
                                            if self.is_recording {
                                                self.is_recording = false;
                                                let _ = self.audio_sender.send(AudioCommand::Stop);
                                            }
                                            self.continuous_mode = true;
                                            let prompt = std::mem::take(&mut self.input_text).trim().to_string();
                                            self.live_transcript.clear();
                                            self.chat_history.push(ChatEntry {
                                                role: ChatRole::User,
                                                text: prompt.clone(),
                                            });
                                            let _ = self.command_sender.send(AgentCommand::Prompt(prompt));
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
                                            });
                                            self.live_transcript.clear();
                                            let _ = self.tts_sender.send(TtsCommand::Stop);
                                        }
                                    });
                                });
                            });
                        });
                });

                // Zone de déplacement restreinte à l'en-tête supérieur
                let drag_rect = egui::Rect::from_min_max(
                    total_rect.min,
                    egui::pos2(total_rect.max.x, total_rect.min.y + 28.0),
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

                    let cursor_screen = get_screen_cursor_pos(ctx);
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

fn resolve_groq_key() -> String {
    if let Ok(key) = std::env::var("GROQ_API_KEY") {
        let trimmed = key.trim();
        if !trimmed.is_empty() {
            return trimmed.to_string();
        }
    }

    if let Ok(content) = std::fs::read_to_string(".env") {
        for line in content.lines() {
            let line = line.trim();
            if line.starts_with('#') {
                continue;
            }
            if let Some(rest) = line.strip_prefix("GROQ_API_KEY=") {
                let val = rest.trim().trim_matches('"').trim_matches('\'');
                if !val.is_empty() {
                    return val.to_string();
                }
            }
        }
    }

    String::new()
}

fn encode_wav(samples: &[f32], sample_rate: u32, channels: u16) -> Vec<u8> {
    let spec = hound::WavSpec {
        channels: 1,
        sample_rate,
        bits_per_sample: 16,
        sample_format: hound::SampleFormat::Int,
    };
    let mut cursor = Cursor::new(Vec::new());
    if let Ok(mut writer) = hound::WavWriter::new(&mut cursor, spec) {
        let ch = channels as usize;
        for chunk in samples.chunks(ch) {
            let mono: f32 = chunk.iter().sum::<f32>() / ch as f32;
            let sample_i16 = (mono.clamp(-1.0, 1.0) * i16::MAX as f32) as i16;
            let _ = writer.write_sample(sample_i16);
        }
        let _ = writer.finalize();
    }
    cursor.into_inner()
}

async fn transcribe_audio(
    api_key: &str,
    wav_data: Vec<u8>,
) -> Result<String, Box<dyn std::error::Error + Send + Sync>> {
    let client = reqwest::Client::new();
    let part = reqwest::multipart::Part::bytes(wav_data)
        .file_name("speech.wav")
        .mime_str("audio/wav")?;

    let form = reqwest::multipart::Form::new()
        .part("file", part)
        .text("model", "whisper-large-v3-turbo")
        .text("language", "fr")
        .text("prompt", "Transcription en français uniquement.")
        .text("response_format", "json");

    let res = client
        .post("https://api.groq.com/openai/v1/audio/transcriptions")
        .bearer_auth(api_key)
        .multipart(form)
        .send()
        .await?;

    if !res.status().is_success() {
        let err = res.text().await.unwrap_or_default();
        return Err(format!("Erreur transcription: {err}").into());
    }

    #[derive(Deserialize)]
    struct TranscribeResp {
        text: String,
    }

    let body = res.json::<TranscribeResp>().await?;
    Ok(body.text.trim().to_string())
}

fn spawn_audio_worker(event_tx: Sender<AgentEvent>, groq_key: String) -> Sender<AudioCommand> {
    let (cmd_tx, cmd_rx) = channel::<AudioCommand>();

    std::thread::spawn(move || {
        let rt = tokio::runtime::Builder::new_current_thread()
            .enable_all()
            .build()
            .expect("Échec runtime audio");

        let host = cpal::default_host();

        while let Ok(cmd) = cmd_rx.recv() {
            if !matches!(cmd, AudioCommand::Start) {
                continue;
            }

            if groq_key.is_empty() {
                let _ = event_tx.send(AgentEvent::ReplaceNarration(
                    "Veuillez renseigner GROQ_API_KEY dans le fichier .env pour activer Whisper.".into(),
                ));
                let _ = event_tx.send(AgentEvent::StatusChanged(AgentStatus::Idle));
                continue;
            }

            let Some(device) = host.default_input_device() else {
                let _ = event_tx.send(AgentEvent::ReplaceNarration(
                    "Aucun microphone disponible.".into(),
                ));
                let _ = event_tx.send(AgentEvent::StatusChanged(AgentStatus::Idle));
                continue;
            };

            let Ok(config) = device.default_input_config() else {
                continue;
            };

            let sample_rate = config.sample_rate().0;
            let channels = config.channels();
            let audio_buffer = Arc::new(Mutex::new(Vec::<f32>::new()));
            let buf_clone = audio_buffer.clone();

            let stream_res = match config.sample_format() {
                cpal::SampleFormat::F32 => device.build_input_stream(
                    &config.into(),
                    move |data: &[f32], _| {
                        if let Ok(mut b) = buf_clone.lock() {
                            b.extend_from_slice(data);
                        }
                    },
                    |_| {},
                    None,
                ),
                cpal::SampleFormat::I16 => {
                    let buf_clone = audio_buffer.clone();
                    device.build_input_stream(
                        &config.into(),
                        move |data: &[i16], _| {
                            if let Ok(mut b) = buf_clone.lock() {
                                for &s in data {
                                    b.push(s as f32 / i16::MAX as f32);
                                }
                            }
                        },
                        |_| {},
                        None,
                    )
                }
                _ => continue,
            };

            let Ok(stream) = stream_res else { continue; };
            let _ = stream.play();

            let mut last_voice_instant = Instant::now();
            let mut has_spoken = false;
            let mut last_interim_instant = Instant::now();
            let mut last_processed_len = 0;

            loop {
                std::thread::sleep(Duration::from_millis(100));

                if let Ok(AudioCommand::Stop) = cmd_rx.try_recv() {
                    break;
                }

                let (current_len, recent_rms) = {
                    let b = audio_buffer.lock().unwrap();
                    let len = b.len();
                    if len > last_processed_len {
                        let slice = &b[last_processed_len..];
                        let sum_sq: f32 = slice.iter().map(|&x| x * x).sum();
                        let rms = (sum_sq / slice.len().max(1) as f32).sqrt();
                        (len, rms)
                    } else {
                        (len, 0.0)
                    }
                };
                last_processed_len = current_len;

                if recent_rms > 0.015 {
                    last_voice_instant = Instant::now();
                    has_spoken = true;
                }

                // Retranscription intermédiaire en temps réel pendant l'élocution
                if has_spoken && last_interim_instant.elapsed() >= Duration::from_millis(1100) {
                    last_interim_instant = Instant::now();
                    let snapshot = audio_buffer.lock().unwrap().clone();
                    if !snapshot.is_empty() {
                        let wav = encode_wav(&snapshot, sample_rate, channels);
                        let key = groq_key.clone();
                        let tx_clone = event_tx.clone();
                        rt.spawn(async move {
                            if let Ok(text) = transcribe_audio(&key, wav).await {
                                if !text.is_empty() {
                                    let _ = tx_clone.send(AgentEvent::TranscriptionPartial(text));
                                }
                            }
                        });
                    }
                }

                // Si l'utilisateur s'arrête de parler pendant 1,4 s, la consigne est validée
                if has_spoken && last_voice_instant.elapsed() >= Duration::from_millis(1400) {
                    break;
                }
            }

            drop(stream);

            let final_samples = audio_buffer.lock().unwrap().clone();
            if has_spoken && !final_samples.is_empty() {
                let wav = encode_wav(&final_samples, sample_rate, channels);
                if let Ok(final_text) = rt.block_on(transcribe_audio(&groq_key, wav)) {
                    if !final_text.is_empty() {
                        let _ = event_tx.send(AgentEvent::VoicePromptReady(final_text));
                        continue;
                    }
                }
            }
            let _ = event_tx.send(AgentEvent::StatusChanged(AgentStatus::Idle));
        }
    });

    cmd_tx
}

fn parse_agent_response(raw: &str) -> AgentResponsePayload {
    let clean = raw.trim();

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

    if let Ok(payload) = serde_json::from_str::<AgentResponsePayload>(unquoted) {
        return payload;
    }

    if let (Some(start), Some(end)) = (unquoted.find('{'), unquoted.rfind('}')) {
        if start < end {
            if let Ok(payload) = serde_json::from_str::<AgentResponsePayload>(&unquoted[start..=end]) {
                return payload;
            }
        }
    }

    AgentResponsePayload {
        narration: clean.to_string(),
        actions: Vec::new(),
    }
}

#[cfg(windows)]
const CREATE_NEW_CONSOLE: u32 = 0x00000010;

#[cfg(windows)]
fn get_system_search_dirs() -> Vec<std::path::PathBuf> {
    let mut dirs = Vec::new();

    if let Some(path_var) = std::env::var_os("PATH") {
        dirs.extend(std::env::split_paths(&path_var));
    }

    if let Ok(sysroot) = std::env::var("SystemRoot") {
        let p = std::path::PathBuf::from(&sysroot);
        dirs.push(p.join("System32"));
        dirs.push(p.clone());
        dirs.push(p.join("SysWOW64"));
    } else {
        dirs.push(std::path::PathBuf::from(r"C:\Windows\System32"));
        dirs.push(std::path::PathBuf::from(r"C:\Windows"));
    }

    if let Ok(localappdata) = std::env::var("LOCALAPPDATA") {
        dirs.push(std::path::PathBuf::from(localappdata).join(r"Microsoft\WindowsApps"));
    }

    dirs
}

#[cfg(windows)]
fn find_in_program_dirs(name: &str, extensions: &[String]) -> Option<std::path::PathBuf> {
    let mut base_dirs = Vec::new();
    if let Some(pf) = std::env::var_os("ProgramFiles") {
        base_dirs.push(std::path::PathBuf::from(pf));
    }
    if let Some(pfx86) = std::env::var_os("ProgramFiles(x86)") {
        base_dirs.push(std::path::PathBuf::from(pfx86));
    }
    if let Some(pfw64) = std::env::var_os("ProgramW6432") {
        base_dirs.push(std::path::PathBuf::from(pfw64));
    }
    if let Ok(localappdata) = std::env::var("LOCALAPPDATA") {
        base_dirs.push(std::path::PathBuf::from(localappdata).join("Programs"));
    }

    let stem = std::path::Path::new(name)
        .file_stem()
        .and_then(|s| s.to_str())
        .unwrap_or(name)
        .to_lowercase();
    let clean_stems = [stem];

    for base in &base_dirs {
        let entries = match std::fs::read_dir(base) {
            Ok(e) => e,
            Err(_) => continue,
        };

        for entry in entries.flatten() {
            let path = entry.path();
            if !path.is_dir() {
                continue;
            }

            // 1. Recherche directe dans le dossier racine de l'application (ex: Mozilla Thunderbird\thunderbird.exe)
            for stem in &clean_stems {
                for ext in extensions {
                    let ext_clean = if ext.starts_with('.') { ext.clone() } else { format!(".{ext}") };
                    let candidate_file = path.join(format!("{stem}{ext_clean}"));
                    if candidate_file.is_file() {
                        return Some(candidate_file);
                    }
                }
            }

            // 2. Recherche à un niveau de sous-dossier (ex: Application\chrome.exe ou bin\...)
            if let Ok(sub_entries) = std::fs::read_dir(&path) {
                for sub in sub_entries.flatten() {
                    let sub_path = sub.path();
                    if sub_path.is_dir() {
                        for stem in &clean_stems {
                            for ext in extensions {
                                let ext_clean = if ext.starts_with('.') { ext.clone() } else { format!(".{ext}") };
                                let candidate_file = sub_path.join(format!("{stem}{ext_clean}"));
                                if candidate_file.is_file() {
                                    return Some(candidate_file);
                                }
                            }
                        }
                    }
                }
            }
        }
    }
    None
}

#[cfg(windows)]
fn find_executable_in_path(name: &str) -> Option<std::path::PathBuf> {
    let name_trimmed = name.trim();
    if name_trimmed.is_empty() {
        return None;
    }

    let pathext = std::env::var_os("PATHEXT").unwrap_or_else(|| std::ffi::OsString::from(".EXE;.CMD;.BAT;.COM"));
    let mut extensions: Vec<String> = pathext
        .to_str()
        .unwrap_or(".EXE;.CMD;.BAT;.COM")
        .split(';')
        .filter(|s| !s.is_empty())
        .map(|s| s.to_string())
        .collect();

    if !extensions.iter().any(|e| e.eq_ignore_ascii_case(".exe")) {
        extensions.push(".EXE".to_string());
    }

    let dirs = get_system_search_dirs();
    let p = std::path::Path::new(name_trimmed);
    if p.is_absolute() && p.exists() {
        return Some(p.to_path_buf());
    }
    let has_ext = p.extension().is_some();
    for dir in &dirs {
        let direct = dir.join(name_trimmed);
        if direct.exists() {
            return Some(direct);
        }
        if !has_ext {
            for ext in &extensions {
                let ext_clean = if ext.starts_with('.') { ext.clone() } else { format!(".{ext}") };
                let full_candidate = dir.join(format!("{name_trimmed}{ext_clean}"));
                if full_candidate.exists() {
                    return Some(full_candidate);
                }
            }
        }
    }

    find_in_program_dirs(name_trimmed, &extensions)
}

#[cfg(windows)]
fn launch_browser_new_window(url: &str) {
    let chrome_paths = [
        r"C:\Program Files\Google\Chrome\Application\chrome.exe",
        r"C:\Program Files (x86)\Google\Chrome\Application\chrome.exe",
        r"C:\Program Files\BraveSoftware\Brave-Browser\Application\brave.exe",
    ];
    let edge_paths = [
        r"C:\Program Files (x86)\Microsoft\Edge\Application\msedge.exe",
        r"C:\Program Files\Microsoft\Edge\Application\msedge.exe",
    ];

    // Recherche prioritaire de Chrome, puis Edge sur les chemins standards
    for path in chrome_paths.iter().chain(edge_paths.iter()) {
        if std::path::Path::new(path).exists() {
            if std::process::Command::new(path)
                .args(["--force-renderer-accessibility", "--new-window", url])
                .spawn()
                .is_ok()
            {
                return;
            }
        }
    }

    if std::process::Command::new("chrome")
        .args(["--force-renderer-accessibility", "--new-window", url])
        .spawn()
        .is_ok()
    {
        return;
    }

    if std::process::Command::new("msedge")
        .args(["--force-renderer-accessibility", "--new-window", url])
        .spawn()
        .is_ok()
    {
        return;
    }

    let _ = std::process::Command::new("cmd")
        .args(["/C", "start", "chrome", "--new-window", url])
        .spawn();
}

#[cfg(windows)]
unsafe extern "system" fn enum_windows_callback(hwnd: HWND, lparam: LPARAM) -> BOOL {
    let list = &mut *(lparam.0 as *mut Vec<(HWND, String)>);
    if IsWindowVisible(hwnd).as_bool() {
        let length = GetWindowTextLengthW(hwnd);
        if length > 0 {
            let mut buf = vec![0u16; (length + 1) as usize];
            let copied = GetWindowTextW(hwnd, &mut buf);
            if copied > 0 {
                let title = String::from_utf16_lossy(&buf[..copied as usize]);
                list.push((hwnd, title));
            }
        }
    }
    BOOL(1)
}

#[cfg(windows)]
fn list_user_windows() -> Vec<(HWND, String)> {
    let mut list: Vec<(HWND, String)> = Vec::new();
    unsafe {
        let _ = EnumWindows(Some(enum_windows_callback), LPARAM(&mut list as *mut _ as isize));
    }
    list.into_iter()
        .filter(|(_, title): &(_, _)| {
            let t = title.trim();
            !t.is_empty()
                && t != "Libertide overlay"
                && t != "Program Manager"
                && t != "Settings"
                && t != "Paramètres"
        })
        .collect()
}

#[cfg(windows)]
fn find_windows_matching(
    keyword: &str,
    user_windows: &[(HWND, String)],
    preferred_hwnd: Option<HWND>,
) -> Vec<HWND> {
    let kw = keyword.to_lowercase();
    let is_browser = kw == "browser" || kw == "navigateur" || kw == "web" || kw == "internet";
    let is_terminal = kw == "cmd" || kw == "terminal" || kw == "console" || kw.contains("invite");
    let mut matches = Vec::new();

    if let Some(pref) = preferred_hwnd {
        if is_browser || user_windows.iter().any(|(h, _)| *h == pref) {
            matches.push(pref);
        }
    }

    for &(hwnd, ref title) in user_windows {
        if Some(hwnd) == preferred_hwnd {
            continue;
        }
        let t_lower = title.to_lowercase();
        if is_browser
            && (t_lower.contains("edge")
                || t_lower.contains("chrome")
                || t_lower.contains("firefox")
                || t_lower.contains("brave"))
        {
            matches.push(hwnd);
            continue;
        }
        if is_terminal
            && (t_lower.contains("cmd")
                || t_lower.contains("terminal")
                || t_lower.contains("powershell")
                || t_lower.contains("invite"))
        {
            matches.push(hwnd);
            continue;
        }
        if t_lower.contains(&kw) {
            matches.push(hwnd);
        }
    }
    matches
}

#[cfg(windows)]
#[derive(Clone)]
struct UiaElementInfo {
    element: IUIAutomationElement,
    name: String,
    class_name: String,
    localized_type: String,
    is_edit_or_textarea: bool,
    is_explicit_textarea: bool,
    click_x: i32,
    click_y: i32,
    rect: RECT,
    pattern: Option<IUIAutomationInvokePattern>,
    area: i64,
}

#[cfg(windows)]
fn list_interactive_elements(hwnd: HWND) -> Vec<UiaElementInfo> {
    let mut results = Vec::new();
    unsafe {
        let _ = CoInitializeEx(None, COINIT_MULTITHREADED);
        let uia: Result<IUIAutomation, _> = CoCreateInstance(&CUIAutomation, None, CLSCTX_INPROC_SERVER);
        let Ok(uia) = uia else {
            return results;
        };

        let Ok(window_element) = uia.ElementFromHandle(hwnd) else {
            return results;
        };

        let Ok(cond) = uia.CreateTrueCondition() else {
            return results;
        };

        let Ok(elements) = window_element.FindAll(TreeScope_Descendants, &cond) else {
            return results;
        };

        let count = elements.Length().unwrap_or(0);
        for i in 0..count {
            if let Ok(item) = elements.GetElement(i) {
                let is_offscreen = item.CurrentIsOffscreen().map(|b| b.as_bool()).unwrap_or(false);
                if is_offscreen {
                    continue;
                }
                let rect = item.CurrentBoundingRectangle().unwrap_or_default();
                let width = rect.right - rect.left;
                let height = rect.bottom - rect.top;
                if width > 4 && height > 4 {
                    let ctype = item.CurrentControlType().unwrap_or_default();
                    let is_edit_type = ctype == UIA_EditControlTypeId || ctype == UIA_DocumentControlTypeId;
                    let class_name = item.CurrentClassName().map(|b| b.to_string()).unwrap_or_default().to_lowercase();
                    let loc_type = item.CurrentLocalizedControlType().map(|b| b.to_string()).unwrap_or_default().to_lowercase();
                    let raw_name = item.CurrentName().map(|b| b.to_string()).unwrap_or_default();
                    let name = raw_name.trim().to_string();

                    let is_explicit_textarea = class_name.contains("textarea")
                        || loc_type.contains("textarea")
                        || loc_type.contains("zone de texte")
                        || (is_edit_type && height >= 35);

                    let is_edit_or_textarea = is_edit_type
                        || is_explicit_textarea
                        || class_name.contains("edit")
                        || loc_type.contains("edit")
                        || loc_type.contains("saisie");

                    if name.is_empty() && !is_edit_or_textarea {
                        continue;
                    }

                    let click_x = rect.left + ((width / 2).min(80)).max(5);
                    let click_y = rect.top + ((height / 2).min(30)).max(5);
                    let pattern = item.GetCurrentPattern(UIA_InvokePatternId)
                        .ok()
                        .and_then(|p| p.cast::<IUIAutomationInvokePattern>().ok());
                    let area = (width as i64) * (height as i64);
                    results.push(UiaElementInfo {
                        element: item,
                        name,
                        class_name,
                        localized_type: loc_type,
                        is_edit_or_textarea,
                        is_explicit_textarea,
                        click_x,
                        click_y,
                        rect,
                        pattern,
                        area,
                    });
                }
            }
        }
    }
    results
}

#[cfg(windows)]
fn is_textarea_query(target: &str) -> (bool, Vec<String>) {
    let lower = target.to_lowercase();
    let words = clean_words(&lower);

    let has_textarea_kw = words.iter().any(|w| {
        w == "textarea"
            || w == "champ"
            || w == "champs"
            || w == "zone"
            || w == "texte"
            || w == "saisie"
            || w == "input"
    });

    if !has_textarea_kw {
        return (false, Vec::new());
    }

    let filter_words: Vec<String> = words
        .into_iter()
        .filter(|w| {
            w != "champ"
                && w != "champs"
                && w != "de"
                && w != "la"
                && w != "le"
                && w != "les"
                && w != "du"
                && w != "sur"
                && w != "dans"
                && w != "zone"
                && w != "texte"
                && w != "textarea"
                && w != "saisie"
                && w != "input"
                && w != "clique"
                && w != "clic"
        })
        .collect();

    (true, filter_words)
}

#[cfg(windows)]
static LAST_TXT_HWND: Mutex<Option<isize>> = Mutex::new(None);

#[cfg(windows)]
fn send_unicode_text(text: &str) {
    let mut inputs = Vec::new();
    for ch in text.encode_utf16() {
        if ch == 10 {
            inputs.push(INPUT {
                r#type: INPUT_KEYBOARD,
                Anonymous: INPUT_0 {
                    ki: KEYBDINPUT {
                        wVk: VIRTUAL_KEY(0x0D),
                        wScan: 0,
                        dwFlags: KEYBD_EVENT_FLAGS(0),
                        time: 0,
                        dwExtraInfo: 0,
                    },
                },
            });
            inputs.push(INPUT {
                r#type: INPUT_KEYBOARD,
                Anonymous: INPUT_0 {
                    ki: KEYBDINPUT {
                        wVk: VIRTUAL_KEY(0x0D),
                        wScan: 0,
                        dwFlags: KEYEVENTF_KEYUP,
                        time: 0,
                        dwExtraInfo: 0,
                    },
                },
            });
        } else if ch != 13 {
            inputs.push(INPUT {
                r#type: INPUT_KEYBOARD,
                Anonymous: INPUT_0 {
                    ki: KEYBDINPUT {
                        wVk: VIRTUAL_KEY(0),
                        wScan: ch,
                        dwFlags: KEYEVENTF_UNICODE,
                        time: 0,
                        dwExtraInfo: 0,
                    },
                },
            });
            inputs.push(INPUT {
                r#type: INPUT_KEYBOARD,
                Anonymous: INPUT_0 {
                    ki: KEYBDINPUT {
                        wVk: VIRTUAL_KEY(0),
                        wScan: ch,
                        dwFlags: KEYEVENTF_UNICODE | KEYEVENTF_KEYUP,
                        time: 0,
                        dwExtraInfo: 0,
                    },
                },
            });
        }
    }
    if !inputs.is_empty() {
        unsafe {
            SendInput(&inputs, std::mem::size_of::<INPUT>() as i32);
        }
    }
}

#[cfg(windows)]
fn append_to_window(hwnd: HWND, text: &str) -> String {
    unsafe {
        let _ = ShowWindow(hwnd, SW_RESTORE);
        let _ = SetForegroundWindow(hwnd);
    }
    std::thread::sleep(Duration::from_millis(80));

    // Raccourci universel : Ctrl + Fin pour aller à la fin du document, puis Entrée
    let nav_keys = [
        (VIRTUAL_KEY(0x11), KEYBD_EVENT_FLAGS(0)), // Ctrl DOWN
        (VIRTUAL_KEY(0x23), KEYBD_EVENT_FLAGS(0)), // Fin DOWN
        (VIRTUAL_KEY(0x23), KEYEVENTF_KEYUP),      // Fin UP
        (VIRTUAL_KEY(0x11), KEYEVENTF_KEYUP),      // Ctrl UP
        (VIRTUAL_KEY(0x0D), KEYBD_EVENT_FLAGS(0)), // Entrée DOWN
        (VIRTUAL_KEY(0x0D), KEYEVENTF_KEYUP),      // Entrée UP
    ];

    let mut inputs = Vec::new();
    for (vk, flags) in nav_keys {
        inputs.push(INPUT {
            r#type: INPUT_KEYBOARD,
            Anonymous: INPUT_0 {
                ki: KEYBDINPUT {
                    wVk: vk,
                    wScan: 0,
                    dwFlags: flags,
                    time: 0,
                    dwExtraInfo: 0,
                },
            },
        });
    }
    unsafe {
        SendInput(&inputs, std::mem::size_of::<INPUT>() as i32);
    }
    std::thread::sleep(Duration::from_millis(50));

    clipboard::set_text(text);
    send_paste();
    "Texte ajouté à la suite dans le document.".to_string()
}

#[cfg(windows)]
fn is_browser_hwnd(hwnd: HWND, title: &str) -> bool {
    let t = title.to_lowercase();
    if t.contains("chrome")
        || t.contains("edge")
        || t.contains("firefox")
        || t.contains("brave")
        || t.contains("opera")
        || t.contains("vivaldi")
    {
        return true;
    }
    unsafe {
        let mut class_buf = [0u16; 256];
        let len = GetClassNameW(hwnd, &mut class_buf);
        if len > 0 {
            let class_name = String::from_utf16_lossy(&class_buf[..len as usize]);
            if class_name.contains("Chrome_WidgetWin") || class_name.contains("MozillaWindowClass") {
                return true;
            }
        }
    }
    false
}

#[cfg(windows)]
fn write_to_temp_txt_file(text: &str) -> String {
    let initial_hwnds: std::collections::HashSet<isize> = list_user_windows()
        .into_iter()
        .map(|(h, _)| h.0 as isize)
        .collect();

    let temp_dir = std::env::temp_dir();
    let timestamp = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap_or_default()
        .as_millis();
    let temp_path = temp_dir.join(format!("note_{timestamp}.txt"));
    if std::fs::write(&temp_path, text).is_ok() {
        let path_str = temp_path.to_string_lossy().to_string();
        let launched = std::process::Command::new("notepad.exe")
            .arg(&temp_path)
            .spawn()
            .is_ok();
        if !launched {
            let _ = std::process::Command::new("cmd")
                .args(["/C", "start", "", &path_str])
                .spawn();
        }

        for _ in 0..20 {
            std::thread::sleep(Duration::from_millis(100));
            let current_windows = list_user_windows();
            for (hwnd, _) in &current_windows {
                let raw = hwnd.0 as isize;
                if !initial_hwnds.contains(&raw) {
                    if let Ok(mut lock) = LAST_TXT_HWND.lock() {
                        *lock = Some(raw);
                    }
                    break;
                }
            }
        }
        "Texte écrit dans un nouveau fichier texte temporaire.".to_string()
    } else {
        "Échec de création du fichier temporaire.".to_string()
    }
}

#[cfg(windows)]
fn write_to_browser_or_txt(text: &str) -> String {
    let overlay_hwnd = unsafe {
        FindWindowW(None, w!("Libertide overlay")).unwrap_or(HWND(std::ptr::null_mut()))
    };
    let fg = unsafe { GetForegroundWindow() };
    let user_windows = list_user_windows();
    let active_hwnd = if !fg.0.is_null() && fg != overlay_hwnd {
        Some(fg)
    } else {
        user_windows.first().map(|(h, _)| *h)
    };

    // Si un éditeur texte a déjà été ouvert et qu'il est en focus ou toujours existant
    if let Ok(lock) = LAST_TXT_HWND.lock() {
        if let Some(raw_h) = *lock {
            let tracked = HWND(raw_h as *mut _);
            if unsafe { IsWindow(tracked).as_bool() } {
                if active_hwnd == Some(tracked) || active_hwnd.map(|h| !is_browser_hwnd(h, "")).unwrap_or(true) {
                    return append_to_window(tracked, text);
                }
            }
        }
    }

    let (is_browser, target_hwnd) = if let Some(hwnd) = active_hwnd {
        let title = user_windows
            .iter()
            .find(|(h, _)| *h == hwnd)
            .map(|(_, t)| t.as_str())
            .unwrap_or("");
        (is_browser_hwnd(hwnd, title), hwnd)
    } else {
        return write_to_temp_txt_file(text);
    };

    if !is_browser {
        return write_to_temp_txt_file(text);
    }

    unsafe {
        let _ = ShowWindow(target_hwnd, SW_RESTORE);
        let _ = SetForegroundWindow(target_hwnd);
    }
    std::thread::sleep(Duration::from_millis(80));

    // Injection par copier-coller direct sans modifier le focus de la page
    clipboard::set_text(text);
    std::thread::sleep(Duration::from_millis(40));
    send_paste();

    "Texte collé dans le champ actif.".to_string()
}

#[cfg(not(windows))]
fn write_to_browser_or_txt(text: &str) -> String {
    let temp_dir = std::env::temp_dir();
    let temp_path = temp_dir.join("libertide_note.txt");
    let _ = std::fs::write(&temp_path, text);
    "Texte écrit dans le fichier temporaire.".to_string()
}

fn parse_write_command(prompt: &str) -> Option<String> {
    let trimmed = prompt.trim();
    let lower = trimmed.to_lowercase();
    let prefixes = ["ecris ", "écris ", "ecris :", "écris :", "ecrire ", "écrire "];
    for prefix in prefixes {
        if lower.starts_with(prefix) {
            let rest = trimmed[prefix.len()..].trim();
            let clean = rest.strip_prefix(':').unwrap_or(rest).trim();
            if !clean.is_empty() {
                return Some(clean.to_string());
            }
        }
    }
    None
}

#[cfg(windows)]
fn click_element(x: i32, y: i32, invoke_pattern: Option<&IUIAutomationInvokePattern>) {
    if let Some(pattern) = invoke_pattern {
        unsafe {
            if pattern.Invoke().is_ok() {
                return;
            }
        }
    }

    // Simulation curseur et clic matériel si l'invocation UIA n'est pas supportée
    unsafe {
        let _ = SetCursorPos(x, y);
        std::thread::sleep(Duration::from_millis(30));
        mouse_event(MOUSEEVENTF_LEFTDOWN, 0, 0, 0, 0);
        std::thread::sleep(Duration::from_millis(40));
        mouse_event(MOUSEEVENTF_LEFTUP, 0, 0, 0, 0);
    }
}

#[cfg(windows)]
fn clean_words(text: &str) -> Vec<String> {
    text.to_lowercase()
        .chars()
        .map(|c| if c.is_alphanumeric() { c } else { ' ' })
        .collect::<String>()
        .split_whitespace()
        .filter(|w| !w.is_empty())
        .map(|w| w.to_string())
        .collect()
}

#[cfg(windows)]
fn rank_button_match(button_name: &str, target_name: &str) -> Option<usize> {
    let b_raw = button_name.trim().to_lowercase();
    let t_raw = target_name.trim().to_lowercase();
    if b_raw.is_empty() || t_raw.is_empty() {
        return None;
    }

    if b_raw == t_raw {
        return Some(0);
    }
    if b_raw.contains(&t_raw) {
        return Some(10 + (b_raw.len() - t_raw.len()).min(50));
    }
    if t_raw.contains(&b_raw) {
        return Some(20 + (t_raw.len() - b_raw.len()).min(50));
    }

    let b_words = clean_words(&b_raw);
    let t_words = clean_words(&t_raw);
    if b_words.is_empty() || t_words.is_empty() {
        return None;
    }

    let mut matched_target_words = 0;
    for tw in &t_words {
        let matched = b_words.iter().any(|bw| {
            bw == tw || (bw.len() >= 3 && tw.len() >= 3 && (bw.starts_with(tw) || tw.starts_with(bw)))
        });
        if matched {
            matched_target_words += 1;
        }
    }

    if matched_target_words == t_words.len() {
        let penalty = b_words.len().saturating_sub(t_words.len());
        Some(50 + penalty.min(40))
    } else if matched_target_words > 0 && matched_target_words * 2 >= t_words.len() {
        let missing = t_words.len() - matched_target_words;
        Some(100 + missing * 20 + b_words.len().min(30))
    } else {
        None
    }
}

#[cfg(windows)]
fn apply_window_rect(hwnd: HWND, x: i32, y: i32, width: i32, height: i32) {
    unsafe {
        let _ = ShowWindow(hwnd, SW_RESTORE);
        let _ = SetWindowPos(
            hwnd,
            HWND(std::ptr::null_mut()),
            x,
            y,
            width,
            height,
            SWP_NOZORDER | SWP_SHOWWINDOW,
        );
    }
}

#[cfg(windows)]
fn execute_system_actions(actions: &[AgentAction]) {
    if actions.is_empty() {
        return;
    }

    let mut newly_spawned_hwnd: Option<HWND> = None;

    for action in actions {
        if let AgentAction::OpenApp { name } = action {
            let initial_hwnds: std::collections::HashSet<isize> = list_user_windows()
                .into_iter()
                .map(|(h, _)| h.0 as isize)
                .collect();

            let launched = if let Some(exe_path) = find_executable_in_path(name) {
                let stem = exe_path
                    .file_stem()
                    .and_then(|s| s.to_str())
                    .unwrap_or("")
                    .to_lowercase();
                let is_console = stem == "cmd" || stem == "powershell" || stem == "pwsh";
                let mut cmd = std::process::Command::new(exe_path);
                if is_console {
                    cmd.creation_flags(CREATE_NEW_CONSOLE);
                }
                cmd.spawn().is_ok()
            } else {
                let mut cmd = std::process::Command::new(name.trim());
                cmd.creation_flags(CREATE_NEW_CONSOLE);
                cmd.spawn().is_ok()
            };

            // Repli automatique sur la recherche web si l'exécutable n'est pas trouvable
            if !launched {
                let search_url = format!("https://www.google.com/search?q={}", urlencoding_simple(name));
                launch_browser_new_window(&search_url);
            }

            for _ in 0..25 {
                std::thread::sleep(Duration::from_millis(100));
                let current_windows = list_user_windows();
                for (hwnd, _title) in &current_windows {
                    if !initial_hwnds.contains(&(hwnd.0 as isize)) {
                        newly_spawned_hwnd = Some(*hwnd);
                        break;
                    }
                }
                if newly_spawned_hwnd.is_some() {
                    break;
                }
            }
        }
    }

    for action in actions {
        if let AgentAction::OpenBrowser { url } = action {
            let initial_hwnds: std::collections::HashSet<isize> = list_user_windows()
                .into_iter()
                .map(|(h, _)| h.0 as isize)
                .collect();

            let raw_url = url.as_deref().unwrap_or("").trim();
            let target = if raw_url.is_empty() || raw_url.starts_with("about:") {
                "https://www.google.com".to_string()
            } else if !raw_url.starts_with("http://") && !raw_url.starts_with("https://") {
                if raw_url.contains('.') {
                    format!("https://{raw_url}")
                } else {
                    format!("https://www.google.com/search?q={}", urlencoding_simple(raw_url))
                }
            } else {
                raw_url.to_string()
            };

            launch_browser_new_window(&target);

            // Scrutation active pendant l'initialisation de la nouvelle fenêtre
            for _ in 0..25 {
                std::thread::sleep(Duration::from_millis(100));
                let current_windows = list_user_windows();
                for (hwnd, title) in &current_windows {
                    if !initial_hwnds.contains(&(hwnd.0 as isize)) {
                        let t = title.to_lowercase();
                        if t.contains("chrome")
                            || t.contains("edge")
                            || t.contains("brave")
                            || t.contains("google")
                            || !t.is_empty()
                        {
                            newly_spawned_hwnd = Some(*hwnd);
                            break;
                        }
                    }
                }
                if newly_spawned_hwnd.is_some() {
                    break;
                }
            }
        }
    }

    let screen_w = unsafe { GetSystemMetrics(SM_CXSCREEN).max(800) };
    let screen_h = unsafe { (GetSystemMetrics(SM_CYSCREEN) - 48).max(600) };
    let user_windows = list_user_windows();

    for action in actions {
        match action {
            AgentAction::OpenApp { .. } => {}
            AgentAction::CloseApp { name } => {
                let kw = name.trim();
                if !kw.is_empty() {
                    let targets = find_windows_matching(kw, &user_windows, None);
                    if let Some(&hwnd) = targets.first() {
                        unsafe {
                            let _ = PostMessageW(hwnd, WM_CLOSE, WPARAM(0), LPARAM(0));
                        }
                    } else {
                        let proc_name = if kw.to_lowercase().ends_with(".exe") {
                            kw.to_string()
                        } else {
                            let stem = std::path::Path::new(kw)
                                .file_stem()
                                .and_then(|s| s.to_str())
                                .unwrap_or(kw);
                            format!("{stem}.exe")
                        };
                        let _ = std::process::Command::new("taskkill")
                            .args(["/IM", &proc_name])
                            .spawn();
                    }
                }
            }
            AgentAction::ClickButton { window, button_name } => {
                let overlay_hwnd = unsafe {
                    FindWindowW(None, w!("Libertide overlay")).unwrap_or(HWND(std::ptr::null_mut()))
                };
                let fg = unsafe { GetForegroundWindow() };
                let active_user_hwnd = if !fg.0.is_null() && fg != overlay_hwnd {
                    Some(fg)
                } else {
                    user_windows.first().map(|(h, _)| *h)
                };

                let target_hwnd = match window.as_deref() {
                    Some(w) if !w.trim().is_empty() => {
                        find_windows_matching(w, &user_windows, active_user_hwnd).first().copied()
                    }
                    _ => active_user_hwnd,
                };

                if let Some(hwnd) = target_hwnd {
                    unsafe {
                        let _ = ShowWindow(hwnd, SW_RESTORE);
                        let _ = SetForegroundWindow(hwnd);
                    }
                    std::thread::sleep(Duration::from_millis(80));

                    let mut elements = list_interactive_elements(hwnd);
                    if elements.is_empty() {
                        std::thread::sleep(Duration::from_millis(150));
                        elements = list_interactive_elements(hwnd);
                    }

                    let (is_textarea_req, filter_words) = is_textarea_query(button_name);

                    if is_textarea_req {
                        let mut candidates: Vec<&UiaElementInfo> = elements
                            .iter()
                            .filter(|e| e.is_edit_or_textarea)
                            .collect();

                        let wants_textarea_tag = button_name.to_lowercase().contains("textarea");
                        let mut win_rect = RECT::default();
                        unsafe { let _ = GetWindowRect(hwnd, &mut win_rect); }
                        let page_top = win_rect.top + 70;

                        candidates.sort_by(|a, b| {
                            let a_in_page = a.rect.top >= page_top;
                            let b_in_page = b.rect.top >= page_top;
                            if a_in_page != b_in_page {
                                return b_in_page.cmp(&a_in_page);
                            }

                            if wants_textarea_tag && a.is_explicit_textarea != b.is_explicit_textarea {
                                return b.is_explicit_textarea.cmp(&a.is_explicit_textarea);
                            }

                            a.rect.top.cmp(&b.rect.top).then_with(|| a.rect.left.cmp(&b.rect.left))
                        });

                        let target_elem = if !filter_words.is_empty() {
                            candidates.iter().copied().find(|elem| {
                                let n = elem.name.to_lowercase();
                                let c = &elem.class_name;
                                let l = &elem.localized_type;
                                filter_words.iter().all(|w| n.contains(w) || c.contains(w) || l.contains(w))
                            }).or_else(|| {
                                candidates.iter().copied().find(|elem| {
                                    let n = elem.name.to_lowercase();
                                    let c = &elem.class_name;
                                    let l = &elem.localized_type;
                                    filter_words.iter().any(|w| n.contains(w) || c.contains(w) || l.contains(w))
                                })
                            }).or_else(|| candidates.first().copied())
                        } else {
                            candidates.first().copied()
                        };

                        if let Some(elem) = target_elem {
                            unsafe { let _ = elem.element.SetFocus(); }
                            click_element(elem.click_x, elem.click_y, elem.pattern.as_ref());
                        }
                    } else {
                        let mut matches: Vec<_> = elements
                            .iter()
                            .filter_map(|elem| {
                                rank_button_match(&elem.name, button_name)
                                    .map(|score| (score, elem))
                            })
                            .collect();

                        matches.sort_by_key(|(score, elem)| (*score, elem.area));

                        if let Some((_, elem)) = matches.first() {
                            unsafe { let _ = elem.element.SetFocus(); }
                            click_element(elem.click_x, elem.click_y, elem.pattern.as_ref());
                        }
                    }
                }
            }
            AgentAction::WriteText { text } => {
                let _ = write_to_browser_or_txt(text);
            }
            AgentAction::OpenBrowser { .. } => {}
            AgentAction::ArrangeWindow { title, position } => {
                let targets = find_windows_matching(title, &user_windows, newly_spawned_hwnd);
                if let Some(&hwnd) = targets.first() {
                    match position.to_lowercase().as_str() {
                        "left" | "left_half" => apply_window_rect(hwnd, 0, 0, screen_w / 2, screen_h),
                        "right" | "right_half" => apply_window_rect(hwnd, screen_w / 2, 0, screen_w / 2, screen_h),
                        "top" | "top_half" => apply_window_rect(hwnd, 0, 0, screen_w, screen_h / 2),
                        "bottom" | "bottom_half" => apply_window_rect(hwnd, 0, screen_h / 2, screen_w, screen_h / 2),
                        "maximize" => unsafe { let _ = ShowWindow(hwnd, SW_MAXIMIZE); },
                        "center" => {
                            let w = (screen_w * 2) / 3;
                            let h = (screen_h * 2) / 3;
                            apply_window_rect(hwnd, (screen_w - w) / 2, (screen_h - h) / 2, w, h);
                        }
                        _ => apply_window_rect(hwnd, 0, 0, screen_w / 2, screen_h),
                    }
                }
            }
            AgentAction::TileWindows { layout, windows } => {
                let layout_mode = layout.as_deref().unwrap_or("split_horizontal");
                let mut target_hwnds = Vec::new();

                for win_key in windows {
                    let hits = find_windows_matching(win_key, &user_windows, newly_spawned_hwnd);
                    if let Some(&h) = hits.first() {
                        if !target_hwnds.contains(&h) {
                            target_hwnds.push(h);
                        }
                    }
                }

                if target_hwnds.len() < 2 {
                    for &(h, _) in &user_windows {
                        if !target_hwnds.contains(&h) {
                            target_hwnds.push(h);
                        }
                        if target_hwnds.len() >= 2 {
                            break;
                        }
                    }
                }

                let count = target_hwnds.len().max(1) as i32;
                for (idx, &hwnd) in target_hwnds.iter().enumerate() {
                    let i = idx as i32;
                    if layout_mode == "split_vertical" {
                        let h = screen_h / count;
                        apply_window_rect(hwnd, 0, i * h, screen_w, h);
                    } else {
                        let w = screen_w / count;
                        apply_window_rect(hwnd, i * w, 0, w, screen_h);
                    }
                }
            }
            AgentAction::MoveWindow { title, x, y, width, height } => {
                let targets = find_windows_matching(title, &user_windows, newly_spawned_hwnd);
                if let Some(&hwnd) = targets.first() {
                    apply_window_rect(hwnd, *x, *y, *width, *height);
                }
            }
        }
    }
}

#[cfg(not(windows))]
fn execute_system_actions(_actions: &[AgentAction]) {}

fn urlencoding_simple(query: &str) -> String {
    query
        .chars()
        .map(|c| if c.is_alphanumeric() { c.to_string() } else { format!("%{:02X}", c as u32) })
        .collect()
}

async fn call_groq_prompt(
    api_key: String,
    user_prompt: String,
    history: &mut Vec<ChatMessage>,
    event_tx: Sender<AgentEvent>,
) {
    let _ = event_tx.send(AgentEvent::StatusChanged(AgentStatus::Thinking));

    if api_key.is_empty() {
        let _ = event_tx.send(AgentEvent::ReplaceNarration(
            "Veuillez renseigner GROQ_API_KEY dans le fichier .env.".into(),
        ));
        let _ = event_tx.send(AgentEvent::StatusChanged(AgentStatus::Idle));
        return;
    }

    let client = reqwest::Client::new();
    let system_instructions = r#"Tu es l'agent d'exploration Libertide pour Windows.
Tu dois IMPÉRATIVEMENT répondre uniquement avec un JSON strict sans texte autour.
Exprime-toi exclusivement en français dans la narration.
Prends en compte l'historique des échanges pour assurer la continuité de la conversation et adapter tes actions.

Règles d'action importantes :
- Si l'utilisateur demande d'ouvrir une application, un outil ou un logiciel (ex: invite de commande, terminal, bloc-notes, messagerie, calculatrice, etc.), détermine TOI-MÊME le nom exact de son exécutable Windows binaire (ex: "cmd", "wt", "notepad", "calc", "thunderbird", "explorer", "code", "mspaint", etc.) et utilise l'action "open_app" avec ce nom direct d'exécutable dans "name".
- Si l'utilisateur demande de fermer une application ou une fenêtre, utilise l'action "close_app" avec le nom de l'exécutable ou un mot-clé du titre dans "name".
- Si l'utilisateur demande d'ouvrir le navigateur sans préciser d'adresse ou pour une page vierge, renseigne toujours "url": "https://www.google.com".
- Si l'utilisateur demande d'écrire ou de saisir du texte, utilise l'action "write_text" avec le texte dans "text".
- Si l'utilisateur demande de cliquer sur un bouton, un lien ou une zone de texte/saisie (ex: "clic sur imaginary world", "clique sur le champ textarea"), utilise l'action "click_button" avec les mots-clés ou le type d'élément dans "button_name" (ex: "champ textarea", "imaginary world") et optionnellement la fenêtre dans "window" si mentionnée (sinon null pour la fenêtre active au premier plan).
- N'utilise JAMAIS d'adresse interne de type "about:blank" ou "about:".
- Pour une recherche, utilise l'URL Google correspondante.
- Ne formule aucune réflexion intermédiaire : la narration doit annoncer directement l'action à l'oral (ex: "J'ouvre le navigateur.").

Format json obligatoire :
{
  "narration": "Explication vocale en français (2 phrases max, concis, naturel).",
  "actions": [
    {"action": "open_app", "name": "nom_executable"},
    {"action": "click_button", "window": "titre_optionnel", "button_name": "nom_du_bouton"},
    {"action": "write_text", "text": "texte à écrire"},
    {"action": "close_app", "name": "nom_ou_titre"},
    {"action": "open_browser", "url": "https://..."},
    {"action": "tile_windows", "layout": "split_horizontal", "windows": ["titre_fenetre_1", "titre_fenetre_2"]},
    {"action": "arrange_window", "title": "mot_cle", "position": "left" | "right" | "top" | "bottom" | "maximize" | "center"},
    {"action": "move_window", "title": "mot_cle", "x": 0, "y": 0, "width": 960, "height": 1040}
  ]
}

Si aucune action système n'est requise, laisse "actions": [].
Si l'utilisateur demande d'ouvrir un site, un sujet de recherche ou d'organiser son espace, choisis les actions adéquates."#;

    history.push(ChatMessage {
        role: "user".to_string(),
        content: user_prompt,
    });

    if history.len() > 20 {
        history.drain(0..history.len() - 20);
    }

    let mut messages = Vec::with_capacity(history.len() + 1);
    messages.push(ChatMessage {
        role: "system".to_string(),
        content: system_instructions.to_string(),
    });
    messages.extend(history.iter().cloned());

    let request = GroqChatRequest {
        model: "openai/gpt-oss-20b".to_string(),
        messages,
        temperature: 1.0,
        max_completion_tokens: 2048,
        top_p: 1.0,
        stream: false,
    };

    let response = client
        .post("https://api.groq.com/openai/v1/chat/completions")
        .bearer_auth(api_key)
        .json(&request)
        .send()
        .await;

    match response {
        Ok(res) if res.status().is_success() => {
            if let Ok(body) = res.json::<GroqChatResponse>().await {
                if let Some(choice) = body.choices.first() {
                    history.push(ChatMessage {
                        role: "assistant".to_string(),
                        content: choice.message.content.clone(),
                    });

                    let payload = parse_agent_response(&choice.message.content);
                    let _ = event_tx.send(AgentEvent::ReplaceNarration(payload.narration));

                    tokio::task::spawn_blocking(move || {
                        execute_system_actions(&payload.actions);
                    }).await.ok();
                    return;
                }
            }
            history.pop();
            let _ = event_tx.send(AgentEvent::ReplaceNarration("Format de réponse inattendu.".into()));
        }
        Ok(res) => {
            history.pop();
            let status = res.status();
            let _ = event_tx.send(AgentEvent::ReplaceNarration(format!("Erreur API Groq : {status}")));
        }
        Err(err) => {
            history.pop();
            let _ = event_tx.send(AgentEvent::ReplaceNarration(format!("Erreur réseau : {err}")));
        }
    }

    let _ = event_tx.send(AgentEvent::StatusChanged(AgentStatus::Idle));
}

fn main() -> eframe::Result<()> {
    let (event_tx, event_rx) = channel::<AgentEvent>();
    let (cmd_tx, cmd_rx) = channel::<AgentCommand>();
    let tts_tx = spawn_tts_worker(event_tx.clone());
    let groq_key = resolve_groq_key();
    let audio_tx = spawn_audio_worker(event_tx.clone(), groq_key.clone());

    // Runtime Tokio en arrière-plan pour requêter Groq
    let groq_chat_key = groq_key.clone();
    std::thread::spawn(move || {
        let rt = tokio::runtime::Runtime::new().expect("Échec d'initialisation du runtime Tokio");
        rt.block_on(async move {
            let mut history: Vec<ChatMessage> = Vec::new();

            while let Ok(cmd) = cmd_rx.recv() {
                match cmd {
                    AgentCommand::Prompt(prompt) => {
                        if let Some(text_to_write) = parse_write_command(&prompt) {
                            let _ = event_tx.send(AgentEvent::StatusChanged(AgentStatus::Thinking));
                            let narration = tokio::task::spawn_blocking(move || {
                                write_to_browser_or_txt(&text_to_write)
                            }).await.unwrap_or_else(|_| "Erreur lors de l'écriture.".to_string());
                            let _ = event_tx.send(AgentEvent::ReplaceNarration(narration));
                            let _ = event_tx.send(AgentEvent::StatusChanged(AgentStatus::Idle));
                        } else {
                            call_groq_prompt(groq_chat_key.clone(), prompt, &mut history, event_tx.clone()).await;
                        }
                    }
                    AgentCommand::ClearHistory => {
                        history.clear();
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
        Box::new(|cc| Ok(Box::new(OverlayApp::new(cc, event_rx, cmd_tx, tts_tx, audio_tx)))),
    )
}