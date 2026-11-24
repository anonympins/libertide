use std::sync::Mutex;
use std::sync::atomic::Ordering;
use std::time::Duration;

use crate::automation::{force_foreground_window, send_hotkey, IS_DICTATION_ACTIVE, VIRTUAL_KEY};
use crate::commands::execute_system_actions;
use crate::types::AgentAction;

#[cfg(windows)]
use windows::Win32::Foundation::{HWND, LPARAM, WPARAM};
#[cfg(windows)]
use windows::Win32::UI::WindowsAndMessaging::{PostMessageW, WM_CLOSE};
#[cfg(windows)]
use crate::ui::mute_other_audio_sessions;
#[cfg(windows)]
use crate::window_util::is_window_hung;

/// Opérations de compensation inversibles (Undo / Redo)
#[derive(Clone, Debug)]
pub enum UndoOperation {
    CloseWindow(isize),
    CloseTab {
        target_hwnd: Option<isize>,
    },
    ReopenTab {
        target_hwnd: Option<isize>,
    },
    NavigateBack {
        target_hwnd: Option<isize>,
    },
    NavigateForward {
        target_hwnd: Option<isize>,
    },
    RevertText {
        target_hwnd: Option<isize>,
    },
    RedoText {
        target_hwnd: Option<isize>,
    },
    ScrollOpposite {
        direction: String,
        amount: i32,
        target_hwnd: Option<isize>,
    },
    CloseFloatingCard(String),
    ActivateDictation,
    DeactivateDictation,
    MuteAudio,
    UnmuteAudio,
    CustomPrompt(String),
}

pub const MAX_HISTORY_STACK_SIZE: usize = 50;

#[derive(Default)]
pub struct ActionHistoryManager {
    pub undo_stack: Vec<UndoOperation>,
    pub redo_stack: Vec<UndoOperation>,
}

impl ActionHistoryManager {
    pub fn push_undo(&mut self, op: UndoOperation) {
        self.undo_stack.push(op);
        if self.undo_stack.len() > MAX_HISTORY_STACK_SIZE {
            self.undo_stack.remove(0);
        }
        self.redo_stack.clear();
    }

    pub fn pop_undo(&mut self) -> Option<UndoOperation> {
        self.undo_stack.pop()
    }

    pub fn pop_redo(&mut self) -> Option<UndoOperation> {
        self.redo_stack.pop()
    }

    pub fn clear(&mut self) {
        self.undo_stack.clear();
        self.redo_stack.clear();
    }
}

pub static ACTION_HISTORY: Mutex<ActionHistoryManager> = Mutex::new(ActionHistoryManager {
    undo_stack: Vec::new(),
    redo_stack: Vec::new(),
});

/// Exécute l'opération d'annulation physique (Undo) et renvoie son inverse pour le Redo
pub fn execute_undo_operation(op: UndoOperation) -> (String, Option<UndoOperation>) {
    match op {
        UndoOperation::CloseWindow(raw_hwnd) => {
            #[cfg(windows)]
            {
                let hwnd = HWND(raw_hwnd as _);
                if !hwnd.0.is_null() && !is_window_hung(hwnd) {
                    unsafe {
                        let _ = PostMessageW(hwnd, WM_CLOSE, WPARAM(0), LPARAM(0));
                    }
                    ("Fenêtre ouverte fermée.".to_string(), None)
                } else {
                    ("Fenêtre cible déjà fermée ou introuvable.".to_string(), None)
                }
            }
            #[cfg(target_os = "macos")]
            {
                let _ = std::process::Command::new("osascript")
                    .args(["-e", "tell application \"System Events\" to tell (first process whose frontmost is true) to close window 1"])
                    .status();
                ("Fenêtre fermée.".to_string(), None)
            }
            #[cfg(all(not(windows), not(target_os = "macos")))]
            {
                let id_str = format!("0x{:x}", raw_hwnd);
                let _ = std::process::Command::new("wmctrl")
                    .args(["-i", "-c", &id_str])
                    .status()
                    .or_else(|_| {
                        std::process::Command::new("xdotool")
                            .args(["windowclose", &raw_hwnd.to_string()])
                            .status()
                    });
                ("Fenêtre fermée.".to_string(), None)
            }
        }
        UndoOperation::CloseTab { target_hwnd } => {
            #[cfg(windows)]
            if let Some(raw) = target_hwnd {
                let hwnd = HWND(raw as _);
                if !hwnd.0.is_null() && !is_window_hung(hwnd) {
                    force_foreground_window(hwnd);
                    std::thread::sleep(Duration::from_millis(50));
                }
            }
            #[cfg(not(windows))]
            let _ = target_hwnd;
            const VK_CONTROL: VIRTUAL_KEY = VIRTUAL_KEY(0x11);
            const VK_W: VIRTUAL_KEY = VIRTUAL_KEY(0x57);
            send_hotkey(&[VK_CONTROL], VK_W);
            ("Onglet ouvert fermé (Ctrl+W).".to_string(), Some(UndoOperation::ReopenTab { target_hwnd }))
        }
        UndoOperation::ReopenTab { target_hwnd } => {
            #[cfg(windows)]
            if let Some(raw) = target_hwnd {
                let hwnd = HWND(raw as _);
                if !hwnd.0.is_null() && !is_window_hung(hwnd) {
                    force_foreground_window(hwnd);
                    std::thread::sleep(Duration::from_millis(50));
                }
            }
            #[cfg(not(windows))]
            let _ = target_hwnd;
            const VK_CONTROL: VIRTUAL_KEY = VIRTUAL_KEY(0x11);
            const VK_SHIFT: VIRTUAL_KEY = VIRTUAL_KEY(0x10);
            const VK_T: VIRTUAL_KEY = VIRTUAL_KEY(0x54);
            send_hotkey(&[VK_CONTROL, VK_SHIFT], VK_T);
            ("Onglet rouvert (Ctrl+Shift+T).".to_string(), Some(UndoOperation::CloseTab { target_hwnd }))
        }
        UndoOperation::NavigateBack { target_hwnd } => {
            #[cfg(windows)]
            if let Some(raw) = target_hwnd {
                let hwnd = HWND(raw as _);
                if !hwnd.0.is_null() && !is_window_hung(hwnd) {
                    force_foreground_window(hwnd);
                    std::thread::sleep(Duration::from_millis(50));
                }
            }
            #[cfg(not(windows))]
            let _ = target_hwnd;
            const VK_MENU: VIRTUAL_KEY = VIRTUAL_KEY(0x12);
            const VK_LEFT: VIRTUAL_KEY = VIRTUAL_KEY(0x25);
            send_hotkey(&[VK_MENU], VK_LEFT);
            ("Retour arrière dans l'historique du navigateur.".to_string(), Some(UndoOperation::NavigateForward { target_hwnd }))
        }
        UndoOperation::NavigateForward { target_hwnd } => {
            #[cfg(windows)]
            if let Some(raw) = target_hwnd {
                let hwnd = HWND(raw as _);
                if !hwnd.0.is_null() && !is_window_hung(hwnd) {
                    force_foreground_window(hwnd);
                    std::thread::sleep(Duration::from_millis(50));
                }
            }
            #[cfg(not(windows))]
            let _ = target_hwnd;
            const VK_MENU: VIRTUAL_KEY = VIRTUAL_KEY(0x12);
            const VK_RIGHT: VIRTUAL_KEY = VIRTUAL_KEY(0x27);
            send_hotkey(&[VK_MENU], VK_RIGHT);
            ("Navigation avant dans l'historique du navigateur.".to_string(), Some(UndoOperation::NavigateBack { target_hwnd }))
        }
        UndoOperation::RevertText { target_hwnd } => {
            #[cfg(windows)]
            if let Some(raw) = target_hwnd {
                let hwnd = HWND(raw as _);
                if !hwnd.0.is_null() && !is_window_hung(hwnd) {
                    force_foreground_window(hwnd);
                    std::thread::sleep(Duration::from_millis(40));
                }
            }
            #[cfg(not(windows))]
            let _ = target_hwnd;
            const VK_CONTROL: VIRTUAL_KEY = VIRTUAL_KEY(0x11);
            const VK_Z: VIRTUAL_KEY = VIRTUAL_KEY(0x5A);
            send_hotkey(&[VK_CONTROL], VK_Z);
            ("Texte annulé (Ctrl+Z).".to_string(), Some(UndoOperation::RedoText { target_hwnd }))
        }
        UndoOperation::RedoText { target_hwnd } => {
            #[cfg(windows)]
            if let Some(raw) = target_hwnd {
                let hwnd = HWND(raw as _);
                if !hwnd.0.is_null() && !is_window_hung(hwnd) {
                    force_foreground_window(hwnd);
                    std::thread::sleep(Duration::from_millis(40));
                }
            }
            #[cfg(not(windows))]
            let _ = target_hwnd;
            const VK_CONTROL: VIRTUAL_KEY = VIRTUAL_KEY(0x11);
            const VK_Y: VIRTUAL_KEY = VIRTUAL_KEY(0x59);
            send_hotkey(&[VK_CONTROL], VK_Y);
            ("Texte rétabli (Ctrl+Y).".to_string(), Some(UndoOperation::RevertText { target_hwnd }))
        }
        UndoOperation::ScrollOpposite { direction, amount, target_hwnd } => {
            let opp_dir = if direction == "down" { "up" } else { "down" };
            let _ = execute_system_actions(&[AgentAction::Scroll {
                direction: opp_dir.to_string(),
                amount: Some(amount),
                window: None,
            }], 0);
            (
                format!("Défilement inverse ({opp_dir}) appliqué."),
                Some(UndoOperation::ScrollOpposite {
                    direction: opp_dir.to_string(),
                    amount,
                    target_hwnd,
                }),
            )
        }
        UndoOperation::CloseFloatingCard(id) => {
            if id == "live_subtitles" {
                crate::floating_window::stop_live_subtitles();
                crate::audio::stop_live_subtitles();
            }
            if let Ok(mut cards) = crate::floating_window::OPEN_FLOATING_CARDS.lock() {
                cards.retain(|c| c.id != id);
            }
            (format!("Carte volante '{id}' fermée."), None)
        }
        UndoOperation::ActivateDictation => {
            IS_DICTATION_ACTIVE.store(true, Ordering::SeqCst);
            ("Mode dictée activé.".to_string(), Some(UndoOperation::DeactivateDictation))
        }
        UndoOperation::DeactivateDictation => {
            IS_DICTATION_ACTIVE.store(false, Ordering::SeqCst);
            ("Mode dictée désactivé.".to_string(), Some(UndoOperation::ActivateDictation))
        }
        UndoOperation::MuteAudio => {
            #[cfg(windows)]
            {
                let _ = unsafe { mute_other_audio_sessions() };
            }
            #[cfg(not(windows))]
            {
                let _ = execute_system_actions(&[AgentAction::MuteAudio], 0);
            }
            ("Audio mis en sourdine.".to_string(), Some(UndoOperation::UnmuteAudio))
        }
        UndoOperation::UnmuteAudio => {
            #[cfg(windows)]
            {
                const VK_VOLUME_MUTE: VIRTUAL_KEY = VIRTUAL_KEY(0xAD);
                send_hotkey(&[], VK_VOLUME_MUTE);
            }
            #[cfg(not(windows))]
            {
                let _ = execute_system_actions(&[AgentAction::SetVolume { command: "unmute".to_string() }], 0);
            }
            ("Audio réactivé.".to_string(), Some(UndoOperation::MuteAudio))
        }
        UndoOperation::CustomPrompt(prompt) => {
            (format!("Annulation via consigne : {prompt}"), None)
        }
    }
}

/// Réinitialise l'ensemble des piles Undo et Redo
pub fn clear_action_history() {
    if let Ok(mut hist) = ACTION_HISTORY.lock() {
        hist.clear();
    }
}

/// Annule la dernière action système enregistrée et empile son inverse sur le Redo
pub fn undo_last_system_action() -> Option<String> {
    let op = {
        let mut hist = ACTION_HISTORY.lock().ok()?;
        hist.pop_undo()?
    };
    let (msg, inverse) = execute_undo_operation(op);
    if let Some(inv) = inverse {
        if let Ok(mut hist) = ACTION_HISTORY.lock() {
            hist.redo_stack.push(inv);
        }
    }
    Some(msg)
}

/// Rétablit la dernière action annulée et empile son inverse sur le Undo
pub fn redo_last_system_action() -> Option<String> {
    let op = {
        let mut hist = ACTION_HISTORY.lock().ok()?;
        hist.pop_redo()?
    };
    let (msg, inverse) = execute_undo_operation(op);
    if let Some(inv) = inverse {
        if let Ok(mut hist) = ACTION_HISTORY.lock() {
            hist.undo_stack.push(inv);
        }
    }
    Some(msg)
}