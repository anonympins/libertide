use std::sync::Mutex;
use std::sync::atomic::{AtomicBool, AtomicU32, AtomicU64, Ordering};
use std::time::{Duration, Instant};

use crate::automation::{trigger_repaint, IS_DICTATION_ACTIVE, IS_USER_TYPING, LIBERTIDE_EXTRA_INFO};
use crate::fs_util::{read_env_var_from_file, write_env_var_to_file};
use crate::types::SwitchKeyEvent;
use crate::window_util::clear_screen_numbered_badges;

#[cfg(windows)]
use windows::core::w;
#[cfg(windows)]
use windows::Win32::Foundation::{HWND, LPARAM, WPARAM};
#[cfg(windows)]
use windows::Win32::UI::Input::KeyboardAndMouse::VIRTUAL_KEY;
#[cfg(windows)]
use windows::Win32::UI::WindowsAndMessaging::{
    CallNextHookEx, DispatchMessageW, FindWindowW, GetForegroundWindow, GetMessageW, HHOOK, IsIconic, MSG,
    PostThreadMessageW, SetWindowsHookExW, ShowWindow, TranslateMessage, UnhookWindowsHookEx,
    WH_KEYBOARD_LL, WH_MOUSE_LL, WM_QUIT,
};

#[cfg(not(windows))]
#[derive(Clone, Copy, Debug, PartialEq, Eq, Default)]
pub struct VIRTUAL_KEY(pub u16);

#[cfg(windows)]
static TRACKED_CTRL: AtomicBool = AtomicBool::new(false);
#[cfg(windows)]
static TRACKED_SHIFT: AtomicBool = AtomicBool::new(false);
#[cfg(windows)]
static TRACKED_ALT: AtomicBool = AtomicBool::new(false);
#[cfg(windows)]
static TRACKED_WIN: AtomicBool = AtomicBool::new(false);

pub static SWITCH_HOOK_ENABLED: AtomicBool = AtomicBool::new(false);
pub static IS_EXTERNAL_TEXT_INPUT_FOCUSED: AtomicBool = AtomicBool::new(false);
pub static SWITCH_KEY_QUEUE: Mutex<Vec<SwitchKeyEvent>> = Mutex::new(Vec::new());
pub static NUMBER_KEY_QUEUE: Mutex<Vec<char>> = Mutex::new(Vec::new());
pub static SWITCH_LONG_PRESS_MS: AtomicU64 = AtomicU64::new(1100);
pub static SWITCH_DEBOUNCE_MS: AtomicU64 = AtomicU64::new(180);

pub const VK_KEY_MASK: u32 = 0x0000FFFF;
pub const MOD_ALT: u32 = 0x00010000;
pub const MOD_CTRL: u32 = 0x00020000;
pub const MOD_SHIFT: u32 = 0x00040000;
pub const MOD_WIN: u32 = 0x00080000;

pub static KEY_BIND_SWITCH: AtomicU32 = AtomicU32::new(0x76);    // F7 (Par défaut : Contacteur / Survol)
pub static KEY_BIND_DICTATION: AtomicU32 = AtomicU32::new(0x77); // F8 (Par défaut : Dictée vocale)
pub static KEY_BIND_VOICE: AtomicU32 = AtomicU32::new(0x78);     // F9 (Par défaut : Synthèse vocale TTS)
pub static KEY_BIND_CLEAR: AtomicU32 = AtomicU32::new(0x73);     // F4 (Par défaut : Vider / Effacer)
pub static REBIND_LISTENING_TARGET: Mutex<Option<&'static str>> = Mutex::new(None);

pub const AVAILABLE_REBIND_KEYS: &[(u32, &str)] = &[
    (0x70, "F1"),
    (0x71, "F2"),
    (0x72, "F3"),
    (0x73, "F4"),
    (0x74, "F5"),
    (0x75, "F6"),
    (0x76, "F7"),
    (0x77, "F8"),
    (0x78, "F9"),
    (0x79, "F10"),
    (0x7A, "F11"),
    (0x7B, "F12"),
    (0x31, "1 (&)"),
    (0x32, "2 (é / ~)"),
    (0x33, "3 (\" / #)"),
    (0x34, "4 (' / {)"),
    (0x35, "5 (( / [)"),
    (0x36, "6 (- / |)"),
    (0x37, "7 (è / `)"),
    (0x38, "8 (_ / \\)"),
    (0x39, "9 (ç / ^)"),
    (0x30, "0 (à / @)"),
    (0xDE, "² (Carré)"),
    (0xBA, "$ / £"),
    (0xC0, "ù / %"),
    (0xDC, "* / µ"),
    (0xDD, "^ / ¨"),
    (0xDF, "! / §"),
    (0xDB, ") / °"),
    (0xE2, "< / >"),
    (0xBC, ", / ?"),
    (0xBE, "; / ."),
    (0xBF, ": / /"),
    (0xBB, "= / +"),
    (0xBD, "- / _"),
    (0x20, "Espace (Space)"),
    (0x0D, "Entrée (Return)"),
    (0x1B, "Échap (Escape)"),
    (0x09, "Tabulation (Tab)"),
    (0x08, "Retour Arrière (Backspace)"),
    (0x13, "Pause / Break"),
    (0x21, "Page Up"),
    (0x22, "Page Down"),
    (0x24, "Début (Home)"),
    (0x23, "Fin (End)"),
    (0x2D, "Inser (Insert)"),
    (0x2E, "Suppr (Delete)"),
    (0x25, "Flèche Gauche"),
    (0x26, "Flèche Haut"),
    (0x27, "Flèche Droite"),
    (0x28, "Flèche Bas"),
    (0x2C, "Impr. Écran (Print Screen)"),
    (0x14, "Verr. Maj (Caps Lock)"),
    (0x90, "Verr. Num (Num Lock)"),
    (0x91, "Arrêt Défil (Scroll Lock)"),
    (0x60, "Pavé num 0"),
    (0x61, "Pavé num 1"),
    (0x62, "Pavé num 2"),
    (0x63, "Pavé num 3"),
    (0x64, "Pavé num 4"),
    (0x65, "Pavé num 5"),
    (0x66, "Pavé num 6"),
    (0x67, "Pavé num 7"),
    (0x68, "Pavé num 8"),
    (0x69, "Pavé num 9"),
    (0x6A, "Pavé num *"),
    (0x6B, "Pavé num +"),
    (0x6D, "Pavé num -"),
    (0x6E, "Pavé num ."),
    (0x6F, "Pavé num /"),
    (0x04, "Souris Molette (Clic milieu)"),
    (0x05, "Souris 4 (XButton 1)"),
    (0x06, "Souris 5 (XButton 2)"),
    (0xAD, "Volume Sourdine (Mute)"),
    (0xAE, "Volume Moins (-)"),
    (0xAF, "Volume Plus (+)"),
    (0xB3, "Média Lecture / Pause"),
    (0xB0, "Média Suivant"),
    (0xB1, "Média Précédent"),
];

pub fn vk_to_label(vk: u32) -> String {
    for &(code, label) in AVAILABLE_REBIND_KEYS {
        if code == vk {
            return label.to_string();
        }
    }
    match vk {
        0x70..=0x87 => format!("F{}", vk - 0x70 + 1),
        0x30..=0x39 => format!("{}", (b'0' + (vk - 0x30) as u8) as char),
        0x41..=0x5A => format!("{}", (b'A' + (vk - 0x41) as u8) as char),
        0x60..=0x69 => format!("Pavé num {}", vk - 0x60),
        0x1B => "Échap (Escape)".to_string(),
        0x20 => "Espace (Space)".to_string(),
        0x0D => "Entrée (Return)".to_string(),
        0x09 => "Tabulation (Tab)".to_string(),
        0x08 => "Retour Arrière (Backspace)".to_string(),
        0x13 => "Pause / Break".to_string(),
        0x21 => "Page Up".to_string(),
        0x22 => "Page Down".to_string(),
        0x23 => "Fin (End)".to_string(),
        0x24 => "Début (Home)".to_string(),
        0x25 => "Flèche Gauche".to_string(),
        0x26 => "Flèche Haut".to_string(),
        0x27 => "Flèche Droite".to_string(),
        0x28 => "Flèche Bas".to_string(),
        0x2D => "Inser (Insert)".to_string(),
        0x2E => "Suppr (Delete)".to_string(),
        0xDE => "² (Carré)".to_string(),
        0xBA => "$ / £".to_string(),
        0xC0 => "ù / %".to_string(),
        0xDC => "* / µ".to_string(),
        0xDD => "^ / ¨".to_string(),
        0xDF => "! / §".to_string(),
        0xDB => ") / °".to_string(),
        0xE2 => "< / >".to_string(),
        0xBC => ", / ?".to_string(),
        0xBE => "; / .".to_string(),
        0xBF => ": / /".to_string(),
        0xBB => "= / +".to_string(),
        0xBD => "- / _".to_string(),
        other => format!("Touche (0x{:02X})", other),
    }
}

pub fn azerty_char_to_vk(text: &str) -> Option<u32> {
    for c in text.chars() {
        let vk = match c {
            '²' => Some(0xDE),
            '$' | '£' | '¤' => Some(0xBA),
            'ù' | '%' => Some(0xC0),
            '*' | 'µ' => Some(0xDC),
            '^' | '¨' => Some(0xDD),
            '!' | '§' => Some(0xDF),
            ')' | '°' | ']' => Some(0xDB),
            '<' | '>' => Some(0xE2),
            ',' | '?' => Some(0xBC),
            ';' | '.' => Some(0xBE),
            ':' | '/' => Some(0xBF),
            '=' | '+' | '}' => Some(0xBB),
            '-' | '_' => Some(0xBD),
            '&' | '1' => Some(0x31),
            'é' | '~' | '2' => Some(0x32),
            '"' | '#' | '3' => Some(0x33),
            '\'' | '{' | '4' => Some(0x34),
            '(' | '[' | '5' => Some(0x35),
            '|' | '6' => Some(0x36),
            'è' | '`' | '7' => Some(0x37),
            '\\' | '8' => Some(0x38),
            'ç' | '9' => Some(0x39),
            'à' | '@' | '0' => Some(0x30),
            _ => None,
        };
        if vk.is_some() {
            return vk;
        }
    }
    None
}

pub fn egui_key_to_vk(key: eframe::egui::Key) -> Option<u32> {
    use eframe::egui::Key;
    match key {
        Key::A => Some(0x41), Key::B => Some(0x42), Key::C => Some(0x43),
        Key::D => Some(0x44), Key::E => Some(0x45), Key::F => Some(0x46),
        Key::G => Some(0x47), Key::H => Some(0x48), Key::I => Some(0x49),
        Key::J => Some(0x4A), Key::K => Some(0x4B), Key::L => Some(0x4C),
        Key::M => Some(0x4D), Key::N => Some(0x4E), Key::O => Some(0x4F),
        Key::P => Some(0x50), Key::Q => Some(0x51), Key::R => Some(0x52),
        Key::S => Some(0x53), Key::T => Some(0x54), Key::U => Some(0x55),
        Key::V => Some(0x56), Key::W => Some(0x57), Key::X => Some(0x58),
        Key::Y => Some(0x59), Key::Z => Some(0x5A),
        Key::Num0 => Some(0x30), Key::Num1 => Some(0x31), Key::Num2 => Some(0x32),
        Key::Num3 => Some(0x33), Key::Num4 => Some(0x34), Key::Num5 => Some(0x35),
        Key::Num6 => Some(0x36), Key::Num7 => Some(0x37), Key::Num8 => Some(0x38),
        Key::Num9 => Some(0x39),
        Key::F1 => Some(0x70), Key::F2 => Some(0x71), Key::F3 => Some(0x72),
        Key::F4 => Some(0x73), Key::F5 => Some(0x74), Key::F6 => Some(0x75),
        Key::F7 => Some(0x76), Key::F8 => Some(0x77), Key::F9 => Some(0x78),
        Key::F10 => Some(0x79), Key::F11 => Some(0x7A), Key::F12 => Some(0x7B),
        Key::Escape => Some(0x1B),
        Key::Space => Some(0x20),
        Key::Enter => Some(0x0D),
        Key::Tab => Some(0x09),
        Key::Backspace => Some(0x08),
        Key::Insert => Some(0x2D),
        Key::Delete => Some(0x2E),
        Key::Home => Some(0x24),
        Key::End => Some(0x23),
        Key::PageUp => Some(0x21),
        Key::PageDown => Some(0x22),
        Key::ArrowLeft => Some(0x25),
        Key::ArrowUp => Some(0x26),
        Key::ArrowRight => Some(0x27),
        Key::ArrowDown => Some(0x28),
        Key::Minus => Some(0xBD),
        Key::Plus | Key::Equals => Some(0xBB),
        Key::Comma => Some(0xBC),
        Key::Period => Some(0xBE),
        Key::Slash => Some(0xBF),
        Key::Backslash => Some(0xDC),
        Key::Colon | Key::Semicolon => Some(0xBA),
        Key::OpenBracket => Some(0xDD),
        Key::CloseBracket => Some(0xBA),
        _ => None,
    }
}

pub fn vk_to_egui_key(vk: u32) -> Option<eframe::egui::Key> {
    use eframe::egui::Key;
    match vk {
        0x70 => Some(Key::F1),
        0x71 => Some(Key::F2),
        0x72 => Some(Key::F3),
        0x73 => Some(Key::F4),
        0x74 => Some(Key::F5),
        0x75 => Some(Key::F6),
        0x76 => Some(Key::F7),
        0x77 => Some(Key::F8),
        0x78 => Some(Key::F9),
        0x79 => Some(Key::F10),
        0x7A => Some(Key::F11),
        0x7B => Some(Key::F12),
        0x21 => Some(Key::PageUp),
        0x22 => Some(Key::PageDown),
        0x23 => Some(Key::End),
        0x24 => Some(Key::Home),
        0x2D => Some(Key::Insert),
        0x2E => Some(Key::Delete),
        _ => None,
    }
}

pub fn load_keybindings() {
    let load_vk = |var_name: &str, default: u32| -> u32 {
        std::env::var(var_name)
            .ok()
            .or_else(|| read_env_var_from_file(".env", var_name))
            .and_then(|v| v.trim().parse::<u32>().ok())
            .unwrap_or(default)
    };
    KEY_BIND_SWITCH.store(load_vk("KEY_BIND_SWITCH", 0x76), Ordering::SeqCst);
    KEY_BIND_DICTATION.store(load_vk("KEY_BIND_DICTATION", 0x77), Ordering::SeqCst);
    KEY_BIND_VOICE.store(load_vk("KEY_BIND_VOICE", 0x78), Ordering::SeqCst);
    KEY_BIND_CLEAR.store(load_vk("KEY_BIND_CLEAR", 0x73), Ordering::SeqCst);
}

#[cfg(windows)]
static HOOK_KEYS_DOWN: Mutex<[bool; 256]> = Mutex::new([false; 256]);
static HOOK_KEYS_DOWN_TIME: Mutex<[Option<Instant>; 256]> = Mutex::new([None; 256]);

pub fn reset_switch_keys() {
    if let Ok(mut q) = SWITCH_KEY_QUEUE.lock() {
        q.clear();
    }
    if let Ok(mut nq) = NUMBER_KEY_QUEUE.lock() {
        nq.clear();
    }
    #[cfg(windows)]
    if let Ok(mut keys) = HOOK_KEYS_DOWN.lock() {
        *keys = [false; 256];
    }
    if let Ok(mut times) = HOOK_KEYS_DOWN_TIME.lock() {
        *times = [None; 256];
    }
}

#[cfg(windows)]
struct HookWorkerState {
    hook_handle: isize,
    mouse_hook_handle: isize,
    thread_id: u32,
}

#[cfg(windows)]
static HOOK_WORKER: Mutex<Option<HookWorkerState>> = Mutex::new(None);

#[cfg(windows)]
unsafe extern "system" fn switch_keyboard_hook_proc(
    code: i32,
    wparam: WPARAM,
    lparam: LPARAM,
) -> windows::Win32::Foundation::LRESULT {
    use windows::Win32::UI::WindowsAndMessaging::{
        KBDLLHOOKSTRUCT, WM_KEYDOWN, WM_KEYUP, WM_SYSKEYDOWN, WM_SYSKEYUP,
    };

    extern "system" {
        fn MapVirtualKeyW(uCode: u32, uMapType: u32) -> u32;
    }

    if code >= 0 {
        let kbd = *(lparam.0 as *const KBDLLHOOKSTRUCT);

        if kbd.dwExtraInfo == LIBERTIDE_EXTRA_INFO {
            return CallNextHookEx(None, code, wparam, lparam);
        }

        let is_down = wparam.0 == WM_KEYDOWN as usize || wparam.0 == WM_SYSKEYDOWN as usize;
        let is_up = wparam.0 == WM_KEYUP as usize || wparam.0 == WM_SYSKEYUP as usize;

        let mut vk = kbd.vkCode;
        if vk == 0 && kbd.scanCode != 0 {
            vk = MapVirtualKeyW(kbd.scanCode, 1);
        }

        match vk {
            0x11 | 0xA2 | 0xA3 => TRACKED_CTRL.store(is_down, Ordering::Relaxed),
            0x10 | 0xA0 | 0xA1 => TRACKED_SHIFT.store(is_down, Ordering::Relaxed),
            0x12 | 0xA4 | 0xA5 => TRACKED_ALT.store(is_down, Ordering::Relaxed),
            0x5B | 0x5C => TRACKED_WIN.store(is_down, Ordering::Relaxed),
            _ => {}
        }

        let is_injected = (kbd.flags.0 & 0x10) != 0;
        let is_powertoys = kbd.dwExtraInfo == 0x1 || kbd.dwExtraInfo == 0x2;

        let is_alt = (kbd.flags.0 & 0x20) != 0;
        let is_ctrl = if is_powertoys {
            false
        } else if is_injected {
            TRACKED_CTRL.load(Ordering::Relaxed)
        } else {
            TRACKED_CTRL.load(Ordering::Relaxed)
                || (windows::Win32::UI::Input::KeyboardAndMouse::GetAsyncKeyState(0x11) as u16 & 0x8000) != 0
        };
        let is_shift = if is_powertoys {
            false
        } else if is_injected {
            TRACKED_SHIFT.load(Ordering::Relaxed)
        } else {
            TRACKED_SHIFT.load(Ordering::Relaxed)
                || (windows::Win32::UI::Input::KeyboardAndMouse::GetAsyncKeyState(0x10) as u16 & 0x8000) != 0
        };

        let is_switch_hook_on = SWITCH_HOOK_ENABLED.load(Ordering::Relaxed);
        let is_dictation = IS_DICTATION_ACTIVE.load(Ordering::Relaxed);

        let key_switch = KEY_BIND_SWITCH.load(Ordering::Relaxed);
        let key_dictation = KEY_BIND_DICTATION.load(Ordering::Relaxed);
        let key_voice = KEY_BIND_VOICE.load(Ordering::Relaxed);
        let key_clear = KEY_BIND_CLEAR.load(Ordering::Relaxed);

        if is_down {
            if let Ok(mut lock) = REBIND_LISTENING_TARGET.lock() {
                if let Some(target_var) = *lock {
                    if vk == 0x1B {
                        *lock = None;
                        trigger_repaint();
                        return windows::Win32::Foundation::LRESULT(1);
                    }
                    match target_var {
                        "KEY_BIND_SWITCH" => KEY_BIND_SWITCH.store(vk, Ordering::SeqCst),
                        "KEY_BIND_DICTATION" => KEY_BIND_DICTATION.store(vk, Ordering::SeqCst),
                        "KEY_BIND_VOICE" => KEY_BIND_VOICE.store(vk, Ordering::SeqCst),
                        "KEY_BIND_CLEAR" => KEY_BIND_CLEAR.store(vk, Ordering::SeqCst),
                        _ => {}
                    }
                    let _ = write_env_var_to_file(".env", target_var, &vk.to_string());
                    std::env::set_var(target_var, vk.to_string());
                    *lock = None;
                    trigger_repaint();
                    return windows::Win32::Foundation::LRESULT(1);
                }
            }
        }

        let global_action_event = if (!is_ctrl && !is_alt) || is_powertoys {
            if vk == key_switch {
                Some(SwitchKeyEvent::ToggleSwitch)
            } else if vk == key_dictation || vk == 0x13 {
                Some(SwitchKeyEvent::ToggleDictation)
            } else if vk == key_voice {
                Some(SwitchKeyEvent::ToggleVoice)
            } else if vk == key_clear {
                Some(SwitchKeyEvent::Clear)
            } else if vk == 0x1B && (is_dictation || is_switch_hook_on) {
                Some(SwitchKeyEvent::Escape)
            } else {
                None
            }
        } else {
            None
        };

        if let Some(event) = global_action_event {
            let vk_idx = (vk as usize) & 0xFF;
            let now = Instant::now();
            if is_down {
                let mut already_down = false;
                if let Ok(mut keys) = HOOK_KEYS_DOWN.lock() {
                    already_down = keys[vk_idx];
                    if already_down {
                        if let Ok(times) = HOOK_KEYS_DOWN_TIME.lock() {
                            if let Some(t) = times[vk_idx] {
                                if now.duration_since(t) > Duration::from_millis(500) {
                                    already_down = false;
                                }
                            } else {
                                already_down = false;
                            }
                        }
                    }
                    keys[vk_idx] = true;
                }
                if !already_down {
                    if let Ok(mut times) = HOOK_KEYS_DOWN_TIME.lock() {
                        times[vk_idx] = Some(now);
                    }
                    if let Ok(mut q) = SWITCH_KEY_QUEUE.lock() {
                        q.push(event);
                    }
                    trigger_repaint();
                }
            } else if is_up {
                if let Ok(mut keys) = HOOK_KEYS_DOWN.lock() {
                    keys[vk_idx] = false;
                }
                if let Ok(mut times) = HOOK_KEYS_DOWN_TIME.lock() {
                    times[vk_idx] = None;
                }
            }
            if is_down || is_up {
                return windows::Win32::Foundation::LRESULT(1);
            }
        }

        let overlay_hwnd = FindWindowW(None, w!("Libertide overlay")).unwrap_or(HWND(std::ptr::null_mut()));
        let fg_hwnd = GetForegroundWindow();
        let is_libertide_fg = !overlay_hwnd.0.is_null() && fg_hwnd == overlay_hwnd;
        let is_user_typing = IS_USER_TYPING.load(Ordering::Relaxed) && is_libertide_fg;

        let is_eye_active = crate::eyetracking::is_eye_tracking_enabled()
            && !crate::eyetracking::is_calibration_active();

        if is_eye_active && !is_dictation && vk == 0x0D && ((!is_ctrl && !is_alt) || is_powertoys) {
            if is_user_typing {
                return CallNextHookEx(None, code, wparam, lparam);
            }
            crate::eyetracking::ENTER_PRESSED.store(is_down, Ordering::Relaxed);
            let vk_idx = (vk as usize) & 0xFF;
            let now = Instant::now();
            if is_down {
                let mut already_down = false;
                if let Ok(mut keys) = HOOK_KEYS_DOWN.lock() {
                    already_down = keys[vk_idx];
                    if already_down {
                        if let Ok(times) = HOOK_KEYS_DOWN_TIME.lock() {
                            if let Some(t) = times[vk_idx] {
                                if now.duration_since(t) > Duration::from_millis(500) {
                                    already_down = false;
                                }
                            } else {
                                already_down = false;
                            }
                        }
                    }
                    keys[vk_idx] = true;
                }
                if !already_down {
                    if let Ok(mut times) = HOOK_KEYS_DOWN_TIME.lock() {
                        times[vk_idx] = Some(now);
                    }
                    crate::eyetracking::request_gaze_click();
                    std::thread::spawn(|| {
                        crate::eyetracking::perform_system_gaze_click();
                    });
                    if let Ok(mut q) = SWITCH_KEY_QUEUE.lock() {
                        q.push(SwitchKeyEvent::GazeClick);
                    }
                    trigger_repaint();
                }
            } else if is_up {
                if let Ok(mut keys) = HOOK_KEYS_DOWN.lock() {
                    keys[vk_idx] = false;
                }
                if let Ok(mut times) = HOOK_KEYS_DOWN_TIME.lock() {
                    times[vk_idx] = None;
                }
            }
            return windows::Win32::Foundation::LRESULT(1);
        }

        if is_user_typing {
            return CallNextHookEx(None, code, wparam, lparam);
        }

        if !is_switch_hook_on {
            return CallNextHookEx(None, code, wparam, lparam);
        }

        if !overlay_hwnd.0.is_null() && IsIconic(overlay_hwnd).as_bool() && is_switch_hook_on {
            let _ = ShowWindow(overlay_hwnd, windows::Win32::UI::WindowsAndMessaging::SW_SHOWNOACTIVATE);
        }

        if is_switch_hook_on && ((!is_ctrl && !is_alt) || is_powertoys) {
            let digit_char = match vk {
                0x60..=0x69 => Some((b'0' + (vk as u8 - 0x60)) as char),
                0x30..=0x39 => Some((b'0' + (vk as u8 - 0x30)) as char),
                _ => None,
            };
            if let Some(digit) = digit_char {
                let vk_idx = (vk as usize) & 0xFF;
                let now = Instant::now();
                if is_down {
                    let mut allow_digit = true;
                    if let Ok(mut times) = HOOK_KEYS_DOWN_TIME.lock() {
                        if let Some(t) = times[vk_idx] {
                            if now.duration_since(t) < Duration::from_millis(40) {
                                allow_digit = false;
                            }
                        }
                        if allow_digit {
                            times[vk_idx] = Some(now);
                        }
                    }
                    if let Ok(mut keys) = HOOK_KEYS_DOWN.lock() {
                        keys[vk_idx] = true;
                    }
                    if allow_digit {
                        if let Ok(mut nq) = NUMBER_KEY_QUEUE.lock() {
                            nq.push(digit);
                        }
                        trigger_repaint();
                    }
                } else if is_up {
                    if let Ok(mut keys) = HOOK_KEYS_DOWN.lock() {
                        keys[vk_idx] = false;
                    }
                    if let Ok(mut times) = HOOK_KEYS_DOWN_TIME.lock() {
                        times[vk_idx] = None;
                    }
                }
                if is_down || is_up {
                    return windows::Win32::Foundation::LRESULT(1);
                }
            }
        }

        let switch_event = match vk {
            0x09 => Some(if is_shift { SwitchKeyEvent::Prev } else { SwitchKeyEvent::Next }),
            0x25 | 0x26 | 0x21 => Some(SwitchKeyEvent::Prev),
            0x27 | 0x28 | 0x22 => Some(SwitchKeyEvent::Next),
            0x0D | 0x20 => Some(SwitchKeyEvent::Select),
            0x08 => Some(SwitchKeyEvent::Back),
            _ => None,
        };

        if let Some(event) = switch_event {
            let vk_idx = (vk as usize) & 0xFF;
            let now = Instant::now();
            if is_down {
                let mut already_down = false;
                if let Ok(mut keys) = HOOK_KEYS_DOWN.lock() {
                    already_down = keys[vk_idx];
                    if already_down {
                        if let Ok(times) = HOOK_KEYS_DOWN_TIME.lock() {
                            if let Some(t) = times[vk_idx] {
                                if now.duration_since(t) > Duration::from_millis(800) {
                                    already_down = false;
                                }
                            } else {
                                already_down = false;
                            }
                        }
                    }
                    keys[vk_idx] = true;
                }
                if !already_down {
                    if let Ok(mut times) = HOOK_KEYS_DOWN_TIME.lock() {
                        times[vk_idx] = Some(now);
                    }
                    if event != SwitchKeyEvent::Select {
                        if let Ok(mut q) = SWITCH_KEY_QUEUE.lock() {
                            q.push(event);
                        }
                        trigger_repaint();
                    }
                }
            } else if is_up {
                let press_duration = if let Ok(mut times) = HOOK_KEYS_DOWN_TIME.lock() {
                    times[vk_idx].take().map(|t| t.elapsed())
                } else {
                    None
                };
                if let Ok(mut keys) = HOOK_KEYS_DOWN.lock() {
                    keys[vk_idx] = false;
                }
                if event == SwitchKeyEvent::Select {
                    let long_press_threshold = Duration::from_millis(SWITCH_LONG_PRESS_MS.load(Ordering::Relaxed));
                    let is_long_press = press_duration.map_or(false, |d| d >= long_press_threshold);
                    if let Ok(mut q) = SWITCH_KEY_QUEUE.lock() {
                        if is_long_press {
                            q.push(SwitchKeyEvent::Back);
                        } else {
                            q.push(SwitchKeyEvent::Select);
                        }
                    }
                    trigger_repaint();
                }
            }
            if is_down || is_up {
                return windows::Win32::Foundation::LRESULT(1);
            }
        }
    }

    CallNextHookEx(None, code, wparam, lparam)
}

#[cfg(windows)]
unsafe extern "system" fn switch_mouse_hook_proc(
    code: i32,
    wparam: WPARAM,
    lparam: LPARAM,
) -> windows::Win32::Foundation::LRESULT {
    use windows::Win32::UI::WindowsAndMessaging::{WM_MOUSEHWHEEL, WM_MOUSEWHEEL};
    if code >= 0 && SWITCH_HOOK_ENABLED.load(Ordering::Relaxed) {
        let msg = wparam.0 as u32;
        if msg == WM_MOUSEWHEEL || msg == WM_MOUSEHWHEEL {
            clear_screen_numbered_badges();
            trigger_repaint();
        }
    }
    CallNextHookEx(None, code, wparam, lparam)
}

#[cfg(windows)]
pub fn install_switch_keyboard_hook() {
    let mut guard = HOOK_WORKER.lock().unwrap();
    if guard.is_some() {
        return;
    }

    let (init_tx, init_rx) = std::sync::mpsc::channel::<(isize, isize, u32)>();

    std::thread::Builder::new()
        .name("win32_keyboard_hook_worker".to_string())
        .spawn(move || {
            use windows::Win32::Foundation::HINSTANCE;
            extern "system" {
                fn GetCurrentThreadId() -> u32;
            }
            let thread_id = unsafe { GetCurrentThreadId() };

            let kbd_hook_res = unsafe {
                SetWindowsHookExW(
                    WH_KEYBOARD_LL,
                    Some(switch_keyboard_hook_proc),
                    HINSTANCE(std::ptr::null_mut()),
                    0,
                )
            };
            let mouse_hook_res = unsafe {
                SetWindowsHookExW(
                    WH_MOUSE_LL,
                    Some(switch_mouse_hook_proc),
                    HINSTANCE(std::ptr::null_mut()),
                    0,
                )
            };

            match (kbd_hook_res, mouse_hook_res) {
                (Ok(kbd_hook), Ok(mouse_hook)) => {
                    let kbd_raw = kbd_hook.0 as isize;
                    let mouse_raw = mouse_hook.0 as isize;
                    let _ = init_tx.send((kbd_raw, mouse_raw, thread_id));
                    let mut msg = MSG::default();
                    unsafe {
                        while GetMessageW(&mut msg, HWND(std::ptr::null_mut()), 0, 0).as_bool() {
                            let _ = TranslateMessage(&msg);
                            DispatchMessageW(&msg);
                        }
                        let _ = UnhookWindowsHookEx(kbd_hook);
                        let _ = UnhookWindowsHookEx(mouse_hook);
                    }
                }
                (Ok(kbd_hook), Err(_)) => {
                    let _ = init_tx.send((kbd_hook.0 as isize, 0, thread_id));
                }
                (Err(_), Ok(mouse_hook)) => {
                    let _ = init_tx.send((0, mouse_hook.0 as isize, thread_id));
                }
                (Err(_), Err(_)) => {
                    let _ = init_tx.send((0, 0, thread_id));
                }
            }
        })
        .expect("Échec du lancement du worker de hook clavier Win32");

    if let Ok((hook_raw, mouse_raw, thread_id)) = init_rx.recv() {
        if hook_raw != 0 {
            *guard = Some(HookWorkerState {
                hook_handle: hook_raw,
                mouse_hook_handle: mouse_raw,
                thread_id,
            });
        }
    }
}

#[cfg(windows)]
pub fn uninstall_switch_keyboard_hook() {
    let mut guard = HOOK_WORKER.lock().unwrap();
    if let Some(state) = guard.take() {
        unsafe {
            let _ = UnhookWindowsHookEx(HHOOK(state.hook_handle as _));
            if state.mouse_hook_handle != 0 {
                let _ = UnhookWindowsHookEx(HHOOK(state.mouse_hook_handle as _));
            }
            let _ = PostThreadMessageW(
                state.thread_id,
                WM_QUIT,
                WPARAM(0),
                LPARAM(0),
            );
        }
    }
}

#[cfg(not(windows))]
static HOOK_THREAD_ACTIVE: AtomicBool = AtomicBool::new(false);
#[cfg(not(windows))]
static SPACE_DOWN_TIME: Mutex<Option<Instant>> = Mutex::new(None);
#[cfg(not(windows))]
static RETURN_DOWN_TIME: Mutex<Option<Instant>> = Mutex::new(None);

#[cfg(not(windows))]
fn vk_to_rdev_key(vk: u32) -> Option<rdev::Key> {
    match vk {
        0x70 => Some(rdev::Key::F1),
        0x71 => Some(rdev::Key::F2),
        0x72 => Some(rdev::Key::F3),
        0x73 => Some(rdev::Key::F4),
        0x74 => Some(rdev::Key::F5),
        0x75 => Some(rdev::Key::F6),
        0x76 => Some(rdev::Key::F7),
        0x77 => Some(rdev::Key::F8),
        0x78 => Some(rdev::Key::F9),
        0x79 => Some(rdev::Key::F10),
        0x7A => Some(rdev::Key::F11),
        0x7B => Some(rdev::Key::F12),
        0x13 => Some(rdev::Key::Pause),
        0x21 => Some(rdev::Key::PageUp),
        0x22 => Some(rdev::Key::PageDown),
        0x23 => Some(rdev::Key::End),
        0x24 => Some(rdev::Key::Home),
        0x2D => Some(rdev::Key::Insert),
        0x2E => Some(rdev::Key::Delete),
        0x1B => Some(rdev::Key::Escape),
        0x20 => Some(rdev::Key::Space),
        0x0D => Some(rdev::Key::Return),
        0x09 => Some(rdev::Key::Tab),
        0x08 => Some(rdev::Key::Backspace),
        0x25 => Some(rdev::Key::LeftArrow),
        0x26 => Some(rdev::Key::UpArrow),
        0x27 => Some(rdev::Key::RightArrow),
        0x28 => Some(rdev::Key::DownArrow),
        _ => None,
    }
}

#[cfg(not(windows))]
pub fn install_switch_keyboard_hook() {
    if HOOK_THREAD_ACTIVE.swap(true, Ordering::SeqCst) {
        return;
    }

    std::thread::Builder::new()
        .name("rdev_switch_hook_worker".to_string())
        .spawn(move || {
            let callback = move |event: rdev::Event| {
                let is_switch_on = SWITCH_HOOK_ENABLED.load(Ordering::Relaxed);
                let is_dictation = IS_DICTATION_ACTIVE.load(Ordering::Relaxed);
                let is_typing = IS_USER_TYPING.load(Ordering::Relaxed);

                let key_switch = vk_to_rdev_key(KEY_BIND_SWITCH.load(Ordering::Relaxed)).unwrap_or(rdev::Key::F7);
                let key_dictation = vk_to_rdev_key(KEY_BIND_DICTATION.load(Ordering::Relaxed)).unwrap_or(rdev::Key::F8);
                let key_voice = vk_to_rdev_key(KEY_BIND_VOICE.load(Ordering::Relaxed)).unwrap_or(rdev::Key::F9);
                let key_clear = vk_to_rdev_key(KEY_BIND_CLEAR.load(Ordering::Relaxed)).unwrap_or(rdev::Key::F4);

                match event.event_type {
                    rdev::EventType::KeyPress(key) => {
                        if matches!(key, rdev::Key::Escape) {
                            crate::eyetracking::ESCAPE_PRESSED.store(true, Ordering::Relaxed);
                        }
                        if matches!(key, rdev::Key::Return) {
                            crate::eyetracking::ENTER_PRESSED.store(true, Ordering::Relaxed);
                        }

                        let global_evt = if key == key_switch {
                            Some(SwitchKeyEvent::ToggleSwitch)
                        } else if key == key_dictation || matches!(key, rdev::Key::Pause) {
                            Some(SwitchKeyEvent::ToggleDictation)
                        } else if key == key_voice {
                            Some(SwitchKeyEvent::ToggleVoice)
                        } else if key == key_clear {
                            Some(SwitchKeyEvent::Clear)
                        } else if matches!(key, rdev::Key::Escape) && (is_dictation || is_switch_on) {
                            Some(SwitchKeyEvent::Escape)
                        } else {
                            None
                        };
                        if let Some(evt) = global_evt {
                            if let Ok(mut q) = SWITCH_KEY_QUEUE.lock() {
                                q.push(evt);
                            }
                            trigger_repaint();
                            return;
                        }

                        let is_eye_active = crate::eyetracking::is_eye_tracking_enabled()
                            && !crate::eyetracking::is_calibration_active();

                        if is_eye_active && !is_dictation && !is_typing && matches!(key, rdev::Key::Return) {
                            crate::eyetracking::request_gaze_click();
                            if let Ok(mut q) = SWITCH_KEY_QUEUE.lock() {
                                q.push(SwitchKeyEvent::GazeClick);
                            }
                            trigger_repaint();
                            return;
                        }

                        if !is_switch_on || is_dictation || is_typing {
                            return;
                        }

                        let digit = match key {
                            rdev::Key::Num0 | rdev::Key::Kp0 => Some('0'),
                            rdev::Key::Num1 | rdev::Key::Kp1 => Some('1'),
                            rdev::Key::Num2 | rdev::Key::Kp2 => Some('2'),
                            rdev::Key::Num3 | rdev::Key::Kp3 => Some('3'),
                            rdev::Key::Num4 | rdev::Key::Kp4 => Some('4'),
                            rdev::Key::Num5 | rdev::Key::Kp5 => Some('5'),
                            rdev::Key::Num6 | rdev::Key::Kp6 => Some('6'),
                            rdev::Key::Num7 | rdev::Key::Kp7 => Some('7'),
                            rdev::Key::Num8 | rdev::Key::Kp8 => Some('8'),
                            rdev::Key::Num9 | rdev::Key::Kp9 => Some('9'),
                            _ => None,
                        };
                        if let Some(d) = digit {
                            if let Ok(mut nq) = NUMBER_KEY_QUEUE.lock() {
                                nq.push(d);
                            }
                            trigger_repaint();
                            return;
                        }

                        match key {
                            rdev::Key::Space => {
                                if let Ok(mut t) = SPACE_DOWN_TIME.lock() {
                                    if t.is_none() { *t = Some(Instant::now()); }
                                }
                            }
                            rdev::Key::Return => {
                                if let Ok(mut t) = RETURN_DOWN_TIME.lock() {
                                    if t.is_none() { *t = Some(Instant::now()); }
                                }
                            }
                            rdev::Key::Tab | rdev::Key::RightArrow | rdev::Key::DownArrow => {
                                if let Ok(mut q) = SWITCH_KEY_QUEUE.lock() {
                                    q.push(SwitchKeyEvent::Next);
                                }
                                trigger_repaint();
                            }
                            rdev::Key::LeftArrow | rdev::Key::UpArrow => {
                                if let Ok(mut q) = SWITCH_KEY_QUEUE.lock() {
                                    q.push(SwitchKeyEvent::Prev);
                                }
                                trigger_repaint();
                            }
                            rdev::Key::Backspace => {
                                if let Ok(mut q) = SWITCH_KEY_QUEUE.lock() {
                                    q.push(SwitchKeyEvent::Back);
                                }
                                trigger_repaint();
                            }
                            _ => {}
                        }
                    }
                    rdev::EventType::KeyRelease(key) => {
                        if matches!(key, rdev::Key::Escape) {
                            crate::eyetracking::ESCAPE_PRESSED.store(false, Ordering::Relaxed);
                        }
                        if matches!(key, rdev::Key::Return) {
                            crate::eyetracking::ENTER_PRESSED.store(false, Ordering::Relaxed);
                        }

                        if !is_switch_on || is_dictation || is_typing {
                            return;
                        }
                        let down_dur = match key {
                            rdev::Key::Space => SPACE_DOWN_TIME.lock().ok().and_then(|mut t| t.take()).map(|s| s.elapsed()),
                            rdev::Key::Return => RETURN_DOWN_TIME.lock().ok().and_then(|mut t| t.take()).map(|s| s.elapsed()),
                            _ => None,
                        };
                        if let Some(dur) = down_dur {
                            let long_press_thresh = Duration::from_millis(SWITCH_LONG_PRESS_MS.load(Ordering::Relaxed));
                            let evt = if dur >= long_press_thresh {
                                SwitchKeyEvent::Back
                            } else {
                                SwitchKeyEvent::Select
                            };
                            if let Ok(mut q) = SWITCH_KEY_QUEUE.lock() {
                                q.push(evt);
                            }
                            trigger_repaint();
                        }
                    }
                    _ => {}
                }
            };

            if let Err(e) = rdev::listen(callback) {
                eprintln!("[rdev Hook Worker] Erreur écoute globale des événements matériels: {:?}", e);
            }
            HOOK_THREAD_ACTIVE.store(false, Ordering::SeqCst);
        })
        .expect("Échec lancement worker rdev");
}

#[cfg(not(windows))]
pub fn uninstall_switch_keyboard_hook() {
    HOOK_THREAD_ACTIVE.store(false, Ordering::SeqCst);
}