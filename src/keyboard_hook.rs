// ============================================================================
// Libertide - Module Keyboard Hook & Passerelle Contacteur (Partie 1/2)
// ============================================================================

use eframe::egui;
use serde::{Deserialize, Serialize};
use std::io::{Read, Write};
use std::net::{TcpListener, TcpStream, UdpSocket};
use std::sync::Mutex;
use std::sync::atomic::{AtomicBool, AtomicU32, AtomicU64, Ordering};
use std::time::{Duration, Instant};

use crate::automation::{trigger_repaint, IS_DICTATION_ACTIVE, IS_USER_TYPING};
#[cfg(windows)]
use crate::automation::LIBERTIDE_EXTRA_INFO;
use crate::fs_util::{read_env_var_from_file, write_env_var_to_file};
use crate::types::SwitchKeyEvent;
#[cfg(windows)]
use crate::window_util::clear_screen_numbered_badges;

#[cfg(windows)]
use windows::core::w;
#[cfg(windows)]
use windows::Win32::Foundation::{HWND, LPARAM, POINT, RECT, WPARAM};
#[cfg(windows)]
use windows::Win32::UI::Input::KeyboardAndMouse::VIRTUAL_KEY;
#[cfg(windows)]
use windows::Win32::UI::WindowsAndMessaging::{
    CallNextHookEx, DispatchMessageW, FindWindowW, GetForegroundWindow, GetMessageW, GetWindowRect, HHOOK, IsIconic,
    MSG, MSLLHOOKSTRUCT, PostThreadMessageW, SetCursorPos, SetForegroundWindow, SetWindowsHookExW, ShowWindow,
    TranslateMessage, UnhookWindowsHookEx, WH_KEYBOARD_LL, WH_MOUSE_LL, WM_LBUTTONDOWN, WM_LBUTTONUP, WM_MBUTTONDOWN,
    WM_MBUTTONUP, WM_MOUSEHWHEEL, WM_MOUSEMOVE, WM_MOUSEWHEEL, WM_QUIT, WM_RBUTTONDOWN, WM_RBUTTONUP,
};

// --- Suivi d'état des modificateurs clavier (Windows) ---
#[cfg(windows)]
pub static TRACKED_CTRL: AtomicBool = AtomicBool::new(false);
#[cfg(windows)]
pub static TRACKED_SHIFT: AtomicBool = AtomicBool::new(false);
#[cfg(windows)]
pub static TRACKED_ALT: AtomicBool = AtomicBool::new(false);
#[cfg(windows)]
pub static TRACKED_WIN: AtomicBool = AtomicBool::new(false);

// --- Suivi d'état des modificateurs clavier (Linux / macOS) ---
#[cfg(not(windows))]
pub static RDEV_TRACKED_CTRL: AtomicBool = AtomicBool::new(false);
#[cfg(not(windows))]
pub static RDEV_TRACKED_SHIFT: AtomicBool = AtomicBool::new(false);
#[cfg(not(windows))]
pub static RDEV_TRACKED_ALT: AtomicBool = AtomicBool::new(false);
#[cfg(not(windows))]
pub static RDEV_TRACKED_META: AtomicBool = AtomicBool::new(false);

pub static SWITCH_HOOK_ENABLED: AtomicBool = AtomicBool::new(false);
pub static IS_EXTERNAL_TEXT_INPUT_FOCUSED: AtomicBool = AtomicBool::new(false);
pub static SWITCH_KEY_QUEUE: Mutex<Vec<SwitchKeyEvent>> = Mutex::new(Vec::new());
pub static NUMBER_KEY_QUEUE: Mutex<Vec<char>> = Mutex::new(Vec::new());
pub static SWITCH_LONG_PRESS_MS: AtomicU64 = AtomicU64::new(1100);
pub static SWITCH_DEBOUNCE_MS: AtomicU64 = AtomicU64::new(180);

// ============================================================================
// Filtres Spasticité, Parkinson, Tremblements & Dystonie
// ============================================================================

pub static STEADY_CLICK_ENABLED: AtomicBool = AtomicBool::new(false);
pub static PREVENT_ACCIDENTAL_DRAG: AtomicBool = AtomicBool::new(false);
pub static BOUNCE_KEYS_ENABLED: AtomicBool = AtomicBool::new(false);
pub static BOUNCE_KEYS_MS: AtomicU64 = AtomicU64::new(180);
pub static TREMOR_FILTER_ENABLED: AtomicBool = AtomicBool::new(false);
pub static SPASM_SHOCK_GUARD: AtomicBool = AtomicBool::new(false);

pub fn is_steady_click_enabled() -> bool {
    STEADY_CLICK_ENABLED.load(Ordering::Relaxed)
}

pub fn set_steady_click_enabled(enabled: bool) {
    STEADY_CLICK_ENABLED.store(enabled, Ordering::Relaxed);
    let val_str = if enabled { "true" } else { "false" };
    let _ = write_env_var_to_file(".env", "STEADY_CLICK_ENABLED", val_str);
    std::env::set_var("STEADY_CLICK_ENABLED", val_str);
}

pub fn is_prevent_accidental_drag() -> bool {
    PREVENT_ACCIDENTAL_DRAG.load(Ordering::Relaxed)
}

pub fn set_prevent_accidental_drag(enabled: bool) {
    PREVENT_ACCIDENTAL_DRAG.store(enabled, Ordering::Relaxed);
    let val_str = if enabled { "true" } else { "false" };
    let _ = write_env_var_to_file(".env", "PREVENT_ACCIDENTAL_DRAG", val_str);
    std::env::set_var("PREVENT_ACCIDENTAL_DRAG", val_str);
}

pub fn is_bounce_keys_enabled() -> bool {
    BOUNCE_KEYS_ENABLED.load(Ordering::Relaxed)
}

pub fn set_bounce_keys_enabled(enabled: bool) {
    BOUNCE_KEYS_ENABLED.store(enabled, Ordering::Relaxed);
    let val_str = if enabled { "true" } else { "false" };
    let _ = write_env_var_to_file(".env", "BOUNCE_KEYS_ENABLED", val_str);
    std::env::set_var("BOUNCE_KEYS_ENABLED", val_str);
}

pub fn get_bounce_keys_ms() -> u64 {
    let ms = BOUNCE_KEYS_MS.load(Ordering::Relaxed);
    if ms < 50 { 180 } else { ms }
}

pub fn set_bounce_keys_ms(ms: u64) {
    let clamped = ms.clamp(50, 1000);
    BOUNCE_KEYS_MS.store(clamped, Ordering::Relaxed);
    let val_str = clamped.to_string();
    let _ = write_env_var_to_file(".env", "BOUNCE_KEYS_MS", &val_str);
    std::env::set_var("BOUNCE_KEYS_MS", val_str);
}

pub fn is_tremor_filter_enabled() -> bool {
    TREMOR_FILTER_ENABLED.load(Ordering::Relaxed)
}

pub fn set_tremor_filter_enabled(enabled: bool) {
    TREMOR_FILTER_ENABLED.store(enabled, Ordering::Relaxed);
    let val_str = if enabled { "true" } else { "false" };
    let _ = write_env_var_to_file(".env", "TREMOR_FILTER_ENABLED", val_str);
    std::env::set_var("TREMOR_FILTER_ENABLED", val_str);
}

pub fn is_spasm_shock_guard() -> bool {
    SPASM_SHOCK_GUARD.load(Ordering::Relaxed)
}

pub fn set_spasm_shock_guard(enabled: bool) {
    SPASM_SHOCK_GUARD.store(enabled, Ordering::Relaxed);
    let val_str = if enabled { "true" } else { "false" };
    let _ = write_env_var_to_file(".env", "SPASM_SHOCK_GUARD", val_str);
    std::env::set_var("SPASM_SHOCK_GUARD", val_str);
}

// Masque de touche de base et drapeaux de bits pour les touches modificatrices
pub const VK_KEY_MASK: u32 = 0x0000FFFF;
pub const MOD_ALT: u32 = 0x00010000;
pub const MOD_CTRL: u32 = 0x00020000;
pub const MOD_SHIFT: u32 = 0x00040000;
pub const MOD_WIN: u32 = 0x00080000;
pub const MOD_MASK: u32 = MOD_CTRL | MOD_ALT | MOD_SHIFT | MOD_WIN;

pub static KEY_BIND_SWITCH: AtomicU32 = AtomicU32::new(0x76);    // F7 (Par défaut : Contacteur / Survol)
pub static KEY_BIND_DICTATION: AtomicU32 = AtomicU32::new(0x77); // F8 (Par défaut : Dictée vocale)
pub static KEY_BIND_VOICE: AtomicU32 = AtomicU32::new(0x78);     // F9 (Par défaut : Synthèse vocale TTS)
pub static KEY_BIND_CLEAR: AtomicU32 = AtomicU32::new(0x73);     // F4 (Par défaut : Vider le contexte)
pub static KEY_BIND_SELECT: AtomicU32 = AtomicU32::new(0x20);    // Espace (Par défaut : Sélection Contacteur)
pub static KEY_BIND_NEXT: AtomicU32 = AtomicU32::new(0x27);      // Flèche Droite (Par défaut : Suivant)
pub static KEY_BIND_PREV: AtomicU32 = AtomicU32::new(0x25);      // Flèche Gauche (Par défaut : Précédent)
pub static KEY_BIND_BACK: AtomicU32 = AtomicU32::new(0x08);      // Retour Arrière (Par défaut : Retour)
pub static KEY_BIND_GAZE_CLICK: AtomicU32 = AtomicU32::new(0x0D);// Entrée (Par défaut : Clic regard)
pub static KEY_BIND_STOP_SPEECH: AtomicU32 = AtomicU32::new(0x13); // Pause/Break (Par défaut : Arrêter la voix TTS)
pub static KEY_BIND_EMERGENCY: AtomicU32 = AtomicU32::new(0x1B); // Échap (Par défaut : Arrêt d'urgence)
pub static REBIND_LISTENING_TARGET: Mutex<Option<String>> = Mutex::new(None);

/// Règle de remappage d'une touche physique quelconque vers une touche ou combinaison simulée
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct CustomKeyRemap {
    pub from_key: u32,
    pub to_key: u32,
    #[serde(default = "default_true")]
    pub enabled: bool,
}

fn default_true() -> bool {
    true
}

pub static CUSTOM_KEY_REMAPS: Mutex<Vec<CustomKeyRemap>> = Mutex::new(Vec::new());

pub fn load_custom_key_remaps() -> Vec<CustomKeyRemap> {
    let path = crate::fs_util::find_key_remaps_file();
    if let Ok(content) = std::fs::read_to_string(&path) {
        if let Ok(list) = serde_json::from_str::<Vec<CustomKeyRemap>>(&content) {
            println!("[KeyRemaps] {} remappage(s) chargé(s) depuis {}", list.len(), path.display());
            return list;
        }
    }
    Vec::new()
}

pub fn save_custom_key_remaps(remaps: &[CustomKeyRemap]) -> std::io::Result<()> {
    let path = crate::fs_util::find_key_remaps_file();
    let json = serde_json::to_string_pretty(remaps).unwrap_or_default();
    std::fs::write(path, json)
}

/// Détermine si un code virtuel correspond à une touche de modification isolée
pub fn is_modifier_vk(vk: u32) -> bool {
    matches!(
        vk,
        0x10 | 0xA0 | 0xA1 // Shift
        | 0x11 | 0xA2 | 0xA3 // Ctrl
        | 0x12 | 0xA4 | 0xA5 // Alt
        | 0x5B | 0x5C // Win / Super
    )
}

/// Vérifie rigoureusement si un code virtuel et un masque de modificateurs actifs correspondent à un binding configuré
pub fn matches_binding(binding: u32, vk: u32, active_mods: u32) -> bool {
    let target_vk = binding & VK_KEY_MASK;
    let target_mods = binding & MOD_MASK;
    let current_mods = active_mods & MOD_MASK;
    (vk & VK_KEY_MASK) == target_vk && current_mods == target_mods
}

/// Vérifie si une règle de remappage correspond à la touche physique
pub fn matches_remap(from_key: u32, vk: u32, active_mods: u32) -> bool {
    let target_vk = from_key & VK_KEY_MASK;
    let target_mods = from_key & MOD_MASK;
    if (vk & VK_KEY_MASK) != target_vk {
        return false;
    }
    if target_mods == 0 {
        true
    } else {
        (active_mods & MOD_MASK) == target_mods
    }
}

/// Applique la touche capturée dans la variable atomique ou la liste des remappages personnalisés
pub fn apply_rebound_key(target_var: &str, full_binding: u32) {
    if let Some(idx_str) = target_var.strip_prefix("CUSTOM_FROM_") {
        if let Ok(idx) = idx_str.parse::<usize>() {
            let mut lock = match CUSTOM_KEY_REMAPS.lock() {
                Ok(guard) => guard,
                Err(poisoned) => poisoned.into_inner(),
            };
            if idx < lock.len() {
                lock[idx].from_key = full_binding;
                let _ = save_custom_key_remaps(&lock);
            }
        }
    } else if let Some(idx_str) = target_var.strip_prefix("CUSTOM_TO_") {
        if let Ok(idx) = idx_str.parse::<usize>() {
            let mut lock = match CUSTOM_KEY_REMAPS.lock() {
                Ok(guard) => guard,
                Err(poisoned) => poisoned.into_inner(),
            };
            if idx < lock.len() {
                lock[idx].to_key = full_binding;
                let _ = save_custom_key_remaps(&lock);
            }
        }
    } else {
        match target_var {
            "KEY_BIND_SWITCH" => KEY_BIND_SWITCH.store(full_binding, Ordering::SeqCst),
            "KEY_BIND_DICTATION" => KEY_BIND_DICTATION.store(full_binding, Ordering::SeqCst),
            "KEY_BIND_VOICE" => KEY_BIND_VOICE.store(full_binding, Ordering::SeqCst),
            "KEY_BIND_CLEAR" => KEY_BIND_CLEAR.store(full_binding, Ordering::SeqCst),
            "KEY_BIND_SELECT" => KEY_BIND_SELECT.store(full_binding, Ordering::SeqCst),
            "KEY_BIND_NEXT" => KEY_BIND_NEXT.store(full_binding, Ordering::SeqCst),
            "KEY_BIND_PREV" => KEY_BIND_PREV.store(full_binding, Ordering::SeqCst),
            "KEY_BIND_BACK" => KEY_BIND_BACK.store(full_binding, Ordering::SeqCst),
            "KEY_BIND_GAZE_CLICK" => KEY_BIND_GAZE_CLICK.store(full_binding, Ordering::SeqCst),
            "KEY_BIND_STOP_SPEECH" => KEY_BIND_STOP_SPEECH.store(full_binding, Ordering::SeqCst),
            "KEY_BIND_EMERGENCY" => KEY_BIND_EMERGENCY.store(full_binding, Ordering::SeqCst),
            _ => {}
        }
        let binding_str = full_binding.to_string();
        let _ = write_env_var_to_file(".env", target_var, &binding_str);
        std::env::set_var(target_var, binding_str);
    }
}

/// Retourne le masque binaire des modificateurs actuellement enfoncés au niveau du système
pub fn get_currently_pressed_modifiers() -> u32 {
    let mut mods = 0u32;
    #[cfg(windows)]
    unsafe {
        use windows::Win32::UI::Input::KeyboardAndMouse::GetAsyncKeyState;
        if TRACKED_CTRL.load(Ordering::Relaxed) || (GetAsyncKeyState(0x11) as u16 & 0x8000) != 0 {
            mods |= MOD_CTRL;
        }
        if TRACKED_ALT.load(Ordering::Relaxed) || (GetAsyncKeyState(0x12) as u16 & 0x8000) != 0 {
            mods |= MOD_ALT;
        }
        if TRACKED_SHIFT.load(Ordering::Relaxed) || (GetAsyncKeyState(0x10) as u16 & 0x8000) != 0 {
            mods |= MOD_SHIFT;
        }
        if TRACKED_WIN.load(Ordering::Relaxed)
            || (GetAsyncKeyState(0x5B) as u16 & 0x8000) != 0
            || (GetAsyncKeyState(0x5C) as u16 & 0x8000) != 0
        {
            mods |= MOD_WIN;
        }
    }
    #[cfg(not(windows))]
    {
        if RDEV_TRACKED_CTRL.load(Ordering::Relaxed) { mods |= MOD_CTRL; }
        if RDEV_TRACKED_ALT.load(Ordering::Relaxed) { mods |= MOD_ALT; }
        if RDEV_TRACKED_SHIFT.load(Ordering::Relaxed) { mods |= MOD_SHIFT; }
        if RDEV_TRACKED_META.load(Ordering::Relaxed) { mods |= MOD_WIN; }
    }
    mods
}

/// Convertit les drapeaux de modificateurs actifs en liste de libellés lisibles
pub fn modifiers_to_labels(mods: u32) -> Vec<&'static str> {
    let mut list = Vec::new();
    if (mods & MOD_CTRL) != 0 { list.push("Ctrl"); }
    if (mods & MOD_ALT) != 0 { list.push("Alt"); }
    if (mods & MOD_SHIFT) != 0 { list.push("Shift"); }
    if (mods & MOD_WIN) != 0 { list.push("Win"); }
    list
}

// ============================================================================
// Passerelle Contacteur Déporté sans fil (Wi-Fi / Smartphone / ESP32)
// ============================================================================

pub static COMPANION_SERVER_RUNNING: AtomicBool = AtomicBool::new(false);
pub static COMPANION_CLIENTS_COUNT: AtomicU32 = AtomicU32::new(0);
pub static LAST_COMPANION_EVENT: Mutex<Option<(String, Instant)>> = Mutex::new(None);

pub const COMPANION_HTTP_PORT: u16 = 8765;
pub const COMPANION_UDP_PORT: u16 = 8766;

pub fn is_companion_server_running() -> bool {
    COMPANION_SERVER_RUNNING.load(Ordering::Relaxed)
}

/// Récupère l'adresse IPv4 locale de la machine sur le réseau local
pub fn get_local_ip_address() -> Option<String> {
    let socket = UdpSocket::bind("0.0.0.0:0").ok()?;
    socket.connect("8.8.8.8:80").ok()?;
    let local_addr = socket.local_addr().ok()?;
    Some(local_addr.ip().to_string())
}

pub fn get_local_companion_url() -> String {
    let ip = get_local_ip_address().unwrap_or_else(|| "127.0.0.1".to_string());
    format!("http://{}:{}", ip, COMPANION_HTTP_PORT)
}

/// Injecte un événement de contacteur
pub fn inject_switch_event(event: SwitchKeyEvent) {
    if let Ok(mut q) = SWITCH_KEY_QUEUE.lock() {
        q.push(event);
    }
    if let Ok(mut last) = LAST_COMPANION_EVENT.lock() {
        let label = format!("{:?}", event);
        *last = Some((label, Instant::now()));
    }
    trigger_repaint();
}

/// Injecte un chiffre de badge interactif
pub fn inject_number_digit(digit: char) {
    if digit.is_ascii_digit() {
        if let Ok(mut nq) = NUMBER_KEY_QUEUE.lock() {
            nq.push(digit);
        }
        if let Ok(mut last) = LAST_COMPANION_EVENT.lock() {
            *last = Some((format!("Badge #{digit}"), Instant::now()));
        }
        trigger_repaint();
    }
}

fn sha1_digest(data: &[u8]) -> [u8; 20] {
    let mut h0: u32 = 0x67452301;
    let mut h1: u32 = 0xEFCDAB89;
    let mut h2: u32 = 0x98BADCFE;
    let mut h3: u32 = 0x10325476;
    let mut h4: u32 = 0xC3D2E1F0;

    let bit_len = (data.len() as u64).wrapping_mul(8);

    let mut msg = Vec::with_capacity(data.len() + 72);
    msg.extend_from_slice(data);
    msg.push(0x80);
    while (msg.len() % 64) != 56 {
        msg.push(0);
    }
    msg.extend_from_slice(&bit_len.to_be_bytes());

    for chunk in msg.chunks_exact(64) {
        let mut w = [0u32; 80];
        for i in 0..16 {
            w[i] = u32::from_be_bytes([
                chunk[i * 4],
                chunk[i * 4 + 1],
                chunk[i * 4 + 2],
                chunk[i * 4 + 3],
            ]);
        }
        for i in 16..80 {
            w[i] = (w[i - 3] ^ w[i - 8] ^ w[i - 14] ^ w[i - 16]).rotate_left(1);
        }

        let mut a = h0;
        let mut b = h1;
        let mut c = h2;
        let mut d = h3;
        let mut e = h4;

        for i in 0..80 {
            let (f, k) = match i {
                0..=19 => ((b & c) | ((!b) & d), 0x5A827999),
                20..=39 => (b ^ c ^ d, 0x6ED9EBA1),
                40..=59 => ((b & c) | (b & d) | (c & d), 0x8F1BBCDC),
                _ => (b ^ c ^ d, 0xCA62C1D6),
            };

            let temp = a
                .rotate_left(5)
                .wrapping_add(f)
                .wrapping_add(e)
                .wrapping_add(k)
                .wrapping_add(w[i]);
            e = d;
            d = c;
            c = b.rotate_left(30);
            b = a;
            a = temp;
        }

        h0 = h0.wrapping_add(a);
        h1 = h1.wrapping_add(b);
        h2 = h2.wrapping_add(c);
        h3 = h3.wrapping_add(d);
        h4 = h4.wrapping_add(e);
    }

    let mut out = [0u8; 20];
    out[0..4].copy_from_slice(&h0.to_be_bytes());
    out[4..8].copy_from_slice(&h1.to_be_bytes());
    out[8..12].copy_from_slice(&h2.to_be_bytes());
    out[12..16].copy_from_slice(&h3.to_be_bytes());
    out[16..20].copy_from_slice(&h4.to_be_bytes());
    out
}

fn base64_encode(data: &[u8]) -> String {
    const TABLE: &[u8; 64] = b"ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789+/";
    let mut out = String::with_capacity((data.len() + 2) / 3 * 4);

    for chunk in data.chunks(3) {
        let b0 = chunk[0] as u32;
        let b1 = if chunk.len() > 1 { chunk[1] as u32 } else { 0 };
        let b2 = if chunk.len() > 2 { chunk[2] as u32 } else { 0 };
        let triple = (b0 << 16) | (b1 << 8) | b2;

        out.push(TABLE[((triple >> 18) & 0x3F) as usize] as char);
        out.push(TABLE[((triple >> 12) & 0x3F) as usize] as char);
        if chunk.len() > 1 {
            out.push(TABLE[((triple >> 6) & 0x3F) as usize] as char);
        } else {
            out.push('=');
        }
        if chunk.len() > 2 {
            out.push(TABLE[(triple & 0x3F) as usize] as char);
        } else {
            out.push('=');
        }
    }
    out
}

fn parse_and_dispatch_companion_action(raw_action: &str) -> bool {
    let cleaned = raw_action.trim().to_lowercase();
    if cleaned.is_empty() {
        return false;
    }

    if cleaned.len() == 1 {
        let c = cleaned.chars().next().unwrap();
        if c.is_ascii_digit() {
            inject_number_digit(c);
            return true;
        }
    }

    match cleaned.as_str() {
        "select" | "click" | "valider" | "enter" | "space" => {
            inject_switch_event(SwitchKeyEvent::Select);
            true
        }
        "next" | "suivant" | "right" | "down" | "tab" => {
            inject_switch_event(SwitchKeyEvent::Next);
            true
        }
        "prev" | "previous" | "precedent" | "précédent" | "left" | "up" => {
            inject_switch_event(SwitchKeyEvent::Prev);
            true
        }
        "back" | "retour" | "backspace" => {
            inject_switch_event(SwitchKeyEvent::Back);
            true
        }
        "escape" | "echap" | "échap" | "cancel" | "annuler" => {
            inject_switch_event(SwitchKeyEvent::Escape);
            true
        }
        "gaze" | "gaze_click" | "clic_regard" => {
            inject_switch_event(SwitchKeyEvent::GazeClick);
            true
        }
        "switch" | "toggle_switch" | "mode_contacteur" => {
            inject_switch_event(SwitchKeyEvent::ToggleSwitch);
            true
        }
        "dictation" | "toggle_dictation" | "dictee" | "dictée" => {
            inject_switch_event(SwitchKeyEvent::ToggleDictation);
            true
        }
        "voice" | "toggle_voice" | "voix" => {
            inject_switch_event(SwitchKeyEvent::ToggleVoice);
            true
        }
        "clear" | "effacer" => {
            inject_switch_event(SwitchKeyEvent::Clear);
            true
        }
        _ => {
            if let Ok(val) = serde_json::from_str::<serde_json::Value>(&cleaned) {
                if let Some(act) = val.get("action").or_else(|| val.get("event")).and_then(|v| v.as_str()) {
                    return parse_and_dispatch_companion_action(act);
                }
                if let Some(num) = val.get("number").or_else(|| val.get("digit")).and_then(|v| v.as_u64()) {
                    for ch in num.to_string().chars() {
                        inject_number_digit(ch);
                    }
                    return true;
                }
            }
            false
        }
    }
}

const COMPANION_HTML_PAGE: &str = r#"<!DOCTYPE html>
<html lang="fr">
<head>
<meta charset="UTF-8">
<meta name="viewport" content="width=device-width, initial-scale=1.0, maximum-scale=1.0, user-scalable=no">
<meta name="apple-mobile-web-app-capable" content="yes">
<meta name="mobile-web-app-capable" content="yes">
<title>Libertide Switch Companion</title>
<style>
  * { box-sizing: border-box; -webkit-tap-highlight-color: transparent; user-select: none; }
  body {
    margin: 0; padding: 12px; height: 100vh; height: 100dvh;
    background: #0B0E14; color: #FFFFFF; font-family: -apple-system, BlinkMacSystemFont, "Segoe UI", Roboto, sans-serif;
    display: flex; flex-direction: column; touch-action: manipulation;
  }
  .header { display: flex; justify-content: space-between; align-items: center; margin-bottom: 8px; }
  .status { font-size: 13px; font-weight: bold; color: #00E676; display: flex; align-items: center; gap: 6px; }
  .status-dot { width: 10px; height: 10px; border-radius: 50%; background: #00E676; }
  .status-dot.disconnected { background: #FF3D00; }
  .switch-main-btn {
    flex: 1; background: #FFD600; color: #000000; border: none; border-radius: 20px;
    font-size: 32px; font-weight: 900; text-transform: uppercase; letter-spacing: 1px;
    display: flex; flex-direction: column; align-items: center; justify-content: center;
    box-shadow: 0 10px 25px rgba(255, 214, 0, 0.4); margin-bottom: 10px; cursor: pointer;
    transition: transform 0.06s ease, background 0.06s ease;
  }
  .switch-main-btn:active { transform: scale(0.97); background: #FFAB00; }
  .switch-main-btn .sub { font-size: 14px; font-weight: 600; opacity: 0.8; margin-top: 4px; }
  .nav-grid { display: grid; grid-template-columns: repeat(3, 1fr); gap: 8px; margin-bottom: 8px; }
  .nav-btn {
    background: #18202C; color: #00E5FF; border: 1.5px solid #00B0FF; border-radius: 12px;
    padding: 14px 6px; font-size: 15px; font-weight: 700; text-align: center; cursor: pointer;
  }
  .nav-btn:active { background: #00B0FF; color: #000000; }
  .actions-grid { display: grid; grid-template-columns: repeat(4, 1fr); gap: 6px; }
  .act-btn {
    background: #151A22; color: #E0E6ED; border: 1px solid #303E4D; border-radius: 10px;
    padding: 10px 4px; font-size: 12px; font-weight: 600; text-align: center; cursor: pointer;
  }
  .act-btn:active { background: #00E676; color: #000; }
  .numpad-drawer { display: none; grid-template-columns: repeat(3, 1fr); gap: 6px; margin-top: 8px; }
  .numpad-drawer.open { display: grid; }
  .num-btn {
    background: #1E2836; color: #FFF; border: 1px solid #00E5FF; border-radius: 8px;
    padding: 10px; font-size: 18px; font-weight: bold; text-align: center;
  }
</style>
</head>
<body>
  <div class="header">
    <div style="font-weight: 800; font-size: 16px; color: #00E5FF;">⚡ LIBERTIDE COMPANION</div>
    <div class="status" id="status"><span class="status-dot" id="dot"></span><span id="st-txt">Connecté</span></div>
  </div>

  <button class="switch-main-btn" id="btn-select">
    <span>CONTACTEUR</span>
    <span class="sub">Toucher / Clic Principal (Sélection)</span>
  </button>

  <div class="nav-grid">
    <button class="nav-btn" id="btn-prev">⬅ PRÉC</button>
    <button class="nav-btn" id="btn-back">↩ RETOUR</button>
    <button class="nav-btn" id="btn-next">SUIV ➔</button>
  </div>

  <div class="actions-grid">
    <button class="act-btn" id="btn-dict">🎙 DICTÉE</button>
    <button class="act-btn" id="btn-voice">🗣 VOIX</button>
    <button class="act-btn" id="btn-pad">🔢 BADGES</button>
    <button class="act-btn" id="btn-esc" style="color: #FF5252; border-color: #FF5252;">✕ ÉCHAP</button>
  </div>

  <div class="numpad-drawer" id="numpad">
    <button class="num-btn" data-n="1">1</button>
    <button class="num-btn" data-n="2">2</button>
    <button class="num-btn" data-n="3">3</button>
    <button class="num-btn" data-n="4">4</button>
    <button class="num-btn" data-n="5">5</button>
    <button class="num-btn" data-n="6">6</button>
    <button class="num-btn" data-n="7">7</button>
    <button class="num-btn" data-n="8">8</button>
    <button class="num-btn" data-n="9">9</button>
    <button class="num-btn" data-n="back">⌫</button>
    <button class="num-btn" data-n="0">0</button>
    <button class="num-btn" data-n="enter" style="color: #FFD600;">OK</button>
  </div>

<script>
  let ws = null;
  const host = window.location.host;

  function vibrate() {
    if (navigator.vibrate) navigator.vibrate(35);
  }

  function sendCmd(action) {
    vibrate();
    if (ws && ws.readyState === WebSocket.OPEN) {
      ws.send(JSON.stringify({ action: action }));
    } else {
      fetch('/api/switch?action=' + encodeURIComponent(action), { method: 'POST', keepalive: true }).catch(()=>{});
    }
  }

  function sendDigit(d) {
    vibrate();
    if (ws && ws.readyState === WebSocket.OPEN) {
      ws.send(JSON.stringify({ digit: d }));
    } else {
      fetch('/api/digit?n=' + encodeURIComponent(d), { method: 'POST', keepalive: true }).catch(()=>{});
    }
  }

  function initWs() {
    try {
      ws = new WebSocket('ws://' + host + '/ws');
      ws.onopen = () => {
        document.getElementById('dot').className = 'status-dot';
        document.getElementById('st-txt').innerText = 'Connecté (WS)';
      };
      ws.onclose = () => {
        document.getElementById('dot').className = 'status-dot disconnected';
        document.getElementById('st-txt').innerText = 'Déconnecté';
        setTimeout(initWs, 2000);
      };
      ws.onerror = () => { ws.close(); };
    } catch(e) {
      setTimeout(initWs, 2000);
    }
  }
  initWs();

  document.getElementById('btn-select').addEventListener('pointerdown', (e) => { e.preventDefault(); sendCmd('select'); });
  document.getElementById('btn-next').addEventListener('pointerdown', (e) => { e.preventDefault(); sendCmd('next'); });
  document.getElementById('btn-prev').addEventListener('pointerdown', (e) => { e.preventDefault(); sendCmd('prev'); });
  document.getElementById('btn-back').addEventListener('pointerdown', (e) => { e.preventDefault(); sendCmd('back'); });
  document.getElementById('btn-esc').addEventListener('pointerdown', (e) => { e.preventDefault(); sendCmd('escape'); });
  document.getElementById('btn-dict').addEventListener('pointerdown', (e) => { e.preventDefault(); sendCmd('dictation'); });
  document.getElementById('btn-voice').addEventListener('pointerdown', (e) => { e.preventDefault(); sendCmd('voice'); });

  const numpad = document.getElementById('numpad');
  document.getElementById('btn-pad').addEventListener('pointerdown', (e) => {
    e.preventDefault();
    vibrate();
    numpad.classList.toggle('open');
  });

  document.querySelectorAll('.num-btn').forEach(btn => {
    btn.addEventListener('pointerdown', (e) => {
      e.preventDefault();
      const n = btn.getAttribute('data-n');
      if (n === 'back') sendCmd('back');
      else if (n === 'enter') sendCmd('select');
      else sendDigit(n);
    });
  });
</script>
</body>
</html>"#;

fn handle_companion_client(mut stream: TcpStream) -> std::io::Result<()> {
    stream.set_read_timeout(Some(Duration::from_millis(1500)))?;
    stream.set_write_timeout(Some(Duration::from_millis(1500)))?;

    let mut header_buf = [0u8; 2048];
    let mut bytes_read = 0;

    while bytes_read < header_buf.len() {
        match stream.read(&mut header_buf[bytes_read..bytes_read + 1]) {
            Ok(1) => {
                bytes_read += 1;
                if bytes_read >= 4 && &header_buf[bytes_read - 4..bytes_read] == b"\r\n\r\n" {
                    break;
                }
            }
            _ => break,
        }
    }

    if bytes_read == 0 {
        return Ok(());
    }

    let request_str = String::from_utf8_lossy(&header_buf[..bytes_read]);
    let mut lines = request_str.lines();
    let req_line = lines.next().unwrap_or("");
    let mut parts = req_line.split_whitespace();
    let _method = parts.next().unwrap_or("GET");
    let path = parts.next().unwrap_or("/");

    let is_upgrade = lines.clone().any(|l| l.to_lowercase().contains("upgrade: websocket"));
    if is_upgrade {
        let sec_key = lines
            .filter(|l| l.to_lowercase().starts_with("sec-websocket-key:"))
            .filter_map(|l| l.split_once(':').map(|(_, v)| v.trim()))
            .next();

        if let Some(key) = sec_key {
            let magic = "258EAFA5-E914-47DA-95CA-C5AB0DC85B11";
            let combined = format!("{}{}", key, magic);
            let digest = sha1_digest(combined.as_bytes());
            let accept = base64_encode(&digest);

            let response = format!(
                "HTTP/1.1 101 Switching Protocols\r\n\
                 Upgrade: websocket\r\n\
                 Connection: Upgrade\r\n\
                 Sec-WebSocket-Accept: {}\r\n\r\n",
                accept
            );
            stream.write_all(response.as_bytes())?;
            stream.flush()?;

            COMPANION_CLIENTS_COUNT.fetch_add(1, Ordering::Relaxed);
            stream.set_read_timeout(Some(Duration::from_secs(60)))?;

            while COMPANION_SERVER_RUNNING.load(Ordering::Relaxed) {
                let mut frame_hdr = [0u8; 2];
                if stream.read_exact(&mut frame_hdr).is_err() {
                    break;
                }
                let opcode = frame_hdr[0] & 0x0F;
                if opcode == 0x08 {
                    break;
                }
                let masked = (frame_hdr[1] & 0x80) != 0;
                let payload_len_raw = frame_hdr[1] & 0x7F;

                let payload_len: usize = if payload_len_raw <= 125 {
                    payload_len_raw as usize
                } else if payload_len_raw == 126 {
                    let mut ext = [0u8; 2];
                    if stream.read_exact(&mut ext).is_err() {
                        break;
                    }
                    u16::from_be_bytes(ext) as usize
                } else {
                    let mut ext = [0u8; 8];
                    if stream.read_exact(&mut ext).is_err() {
                        break;
                    }
                    u64::from_be_bytes(ext) as usize
                };

                let mask_key = if masked {
                    let mut m = [0u8; 4];
                    if stream.read_exact(&mut m).is_err() {
                        break;
                    }
                    Some(m)
                } else {
                    None
                };

                let mut payload = vec![0u8; payload_len];
                if stream.read_exact(&mut payload).is_err() {
                    break;
                }
                if let Some(mask) = mask_key {
                    for (i, b) in payload.iter_mut().enumerate() {
                        *b ^= mask[i % 4];
                    }
                }

                if opcode == 0x01 {
                    if let Ok(text) = std::str::from_utf8(&payload) {
                        parse_and_dispatch_companion_action(text);
                    }
                } else if opcode == 0x09 {
                    let pong = [0x8A, 0x00];
                    let _ = stream.write_all(&pong);
                }
            }

            COMPANION_CLIENTS_COUNT.fetch_sub(1, Ordering::Relaxed);
            return Ok(());
        }
    }

    if path == "/" || path == "/index.html" {
        let html = COMPANION_HTML_PAGE;
        let resp = format!(
            "HTTP/1.1 200 OK\r\n\
             Content-Type: text/html; charset=utf-8\r\n\
             Content-Length: {}\r\n\
             Connection: close\r\n\r\n\
             {}",
            html.len(),
            html
        );
        stream.write_all(resp.as_bytes())?;
        stream.flush()?;
    } else if path.starts_with("/api/switch") || path.starts_with("/switch") {
        let action = path.split_once("action=").map(|(_, a)| a.split('&').next().unwrap_or(a)).unwrap_or("select");
        parse_and_dispatch_companion_action(action);
        let body = r#"{"status":"ok"}"#;
        let resp = format!(
            "HTTP/1.1 200 OK\r\n\
             Content-Type: application/json\r\n\
             Content-Length: {}\r\n\
             Access-Control-Allow-Origin: *\r\n\
             Connection: close\r\n\r\n\
             {}",
            body.len(),
            body
        );
        stream.write_all(resp.as_bytes())?;
        stream.flush()?;
    } else if path.starts_with("/api/digit") || path.starts_with("/digit") {
        let digit_str = path.split_once("n=").map(|(_, a)| a.split('&').next().unwrap_or(a)).unwrap_or("1");
        for ch in digit_str.chars() {
            inject_number_digit(ch);
        }
        let body = r#"{"status":"ok"}"#;
        let resp = format!(
            "HTTP/1.1 200 OK\r\n\
             Content-Type: application/json\r\n\
             Content-Length: {}\r\n\
             Access-Control-Allow-Origin: *\r\n\
             Connection: close\r\n\r\n\
             {}",
            body.len(),
            body
        );
        stream.write_all(resp.as_bytes())?;
        stream.flush()?;
    } else if path == "/api/status" || path == "/status" {
        let body = format!(
            r#"{{"server":"Libertide Companion","ip":"{}","clients":{}}}"#,
            get_local_ip_address().unwrap_or_default(),
            COMPANION_CLIENTS_COUNT.load(Ordering::Relaxed)
        );
        let resp = format!(
            "HTTP/1.1 200 OK\r\n\
             Content-Type: application/json\r\n\
             Content-Length: {}\r\n\
             Access-Control-Allow-Origin: *\r\n\
             Connection: close\r\n\r\n\
             {}",
            body.len(),
            body
        );
        stream.write_all(resp.as_bytes())?;
        stream.flush()?;
    } else {
        let not_found = "404 Not Found";
        let resp = format!(
            "HTTP/1.1 404 Not Found\r\n\
             Content-Length: {}\r\n\
             Connection: close\r\n\r\n\
             {}",
            not_found.len(),
            not_found
        );
        stream.write_all(resp.as_bytes())?;
        stream.flush()?;
    }

    Ok(())
}

pub fn start_companion_switch_server() {
    if COMPANION_SERVER_RUNNING.swap(true, Ordering::SeqCst) {
        return;
    }

    std::thread::Builder::new()
        .name("companion_http_ws_server".to_string())
        .spawn(move || {
            let bind_addr = format!("0.0.0.0:{}", COMPANION_HTTP_PORT);
            let listener = match TcpListener::bind(&bind_addr) {
                Ok(l) => {
                    let _ = l.set_nonblocking(true);
                    println!(
                        "[Companion Switch] Passerelle Web & WebSocket active sur {} (URL: {})",
                        bind_addr,
                        get_local_companion_url()
                    );
                    l
                }
                Err(e) => {
                    eprintln!("[Companion Switch] Impossible de lier le port TCP {}: {e}", COMPANION_HTTP_PORT);
                    return;
                }
            };

            while COMPANION_SERVER_RUNNING.load(Ordering::Relaxed) {
                match listener.accept() {
                    Ok((stream, _)) => {
                        std::thread::spawn(move || {
                            let _ = handle_companion_client(stream);
                        });
                    }
                    Err(ref e) if e.kind() == std::io::ErrorKind::WouldBlock => {
                        std::thread::sleep(Duration::from_millis(50));
                    }
                    Err(_) => {
                        std::thread::sleep(Duration::from_millis(100));
                    }
                }
            }
        })
        .expect("Échec lancement worker companion_http_ws_server");

    std::thread::Builder::new()
        .name("companion_udp_server".to_string())
        .spawn(move || {
            let bind_addr = format!("0.0.0.0:{}", COMPANION_UDP_PORT);
            let socket = match UdpSocket::bind(&bind_addr) {
                Ok(s) => {
                    let _ = s.set_read_timeout(Some(Duration::from_millis(250)));
                    println!("[Companion Switch] Passerelle UDP (ESP32 / Pédales) active sur {}", bind_addr);
                    s
                }
                Err(e) => {
                    eprintln!("[Companion Switch] Impossible de lier le port UDP {}: {e}", COMPANION_UDP_PORT);
                    return;
                }
            };

            let mut buf = [0u8; 512];
            while COMPANION_SERVER_RUNNING.load(Ordering::Relaxed) {
                match socket.recv_from(&mut buf) {
                    Ok((len, _src)) if len > 0 => {
                        if let Ok(text) = std::str::from_utf8(&buf[..len]) {
                            parse_and_dispatch_companion_action(text);
                        }
                    }
                    _ => {
                        std::thread::sleep(Duration::from_millis(50));
                    }
                }
            }
        })
        .expect("Échec lancement worker companion_udp_server");
}

pub fn stop_companion_switch_server() {
    COMPANION_SERVER_RUNNING.store(false, Ordering::SeqCst);
}

// ============================================================================
// Récepteur BCI / EEG / EMG (Lab Streaming Layer - LSL)
// ============================================================================

pub static BCI_LSL_RUNNING: AtomicBool = AtomicBool::new(false);
pub static BCI_LSL_CONNECTED: AtomicBool = AtomicBool::new(false);
pub static BCI_LSL_PACKETS_COUNT: AtomicU64 = AtomicU64::new(0);
pub static BCI_EMG_THRESHOLD: AtomicU32 = AtomicU32::new(0.065f32.to_bits());
pub static BCI_LSL_PORT: AtomicU32 = AtomicU32::new(16572);
pub static LAST_BCI_EVENT: Mutex<Option<(String, Instant)>> = Mutex::new(None);

pub fn is_bci_lsl_running() -> bool {
    BCI_LSL_RUNNING.load(Ordering::Relaxed)
}

pub fn is_bci_lsl_connected() -> bool {
    BCI_LSL_CONNECTED.load(Ordering::Relaxed)
}

pub fn get_bci_emg_threshold() -> f32 {
    let val = f32::from_bits(BCI_EMG_THRESHOLD.load(Ordering::Relaxed));
    if val < 0.005 { 0.065 } else { val }
}

pub fn set_bci_emg_threshold(th: f32) {
    let clamped = th.clamp(0.005, 0.500);
    BCI_EMG_THRESHOLD.store(clamped.to_bits(), Ordering::Relaxed);
    let val_str = format!("{:.4}", clamped);
    let _ = write_env_var_to_file(".env", "BCI_EMG_THRESHOLD", &val_str);
    std::env::set_var("BCI_EMG_THRESHOLD", val_str);
}

pub fn get_bci_lsl_port() -> u16 {
    (BCI_LSL_PORT.load(Ordering::Relaxed) as u16).max(1024)
}

pub fn set_bci_lsl_port(port: u16) {
    let p = port.max(1024);
    BCI_LSL_PORT.store(p as u32, Ordering::Relaxed);
    let val_str = p.to_string();
    let _ = write_env_var_to_file(".env", "LSL_BCI_PORT", &val_str);
    std::env::set_var("LSL_BCI_PORT", val_str);
}

/// Analyse un paquet LSL (marqueur textuel, JSON ou tableau de floats bruts EEG/EMG)
fn process_bci_lsl_packet(payload: &[u8], last_trigger: &mut Instant, baseline_rms: &mut f32) {
    BCI_LSL_PACKETS_COUNT.fetch_add(1, Ordering::Relaxed);
    BCI_LSL_CONNECTED.store(true, Ordering::Relaxed);
    let now = Instant::now();

    // 1. Détection de marqueur textuel ou JSON (P300, Trigger, Clench, Select...)
    if let Ok(text) = std::str::from_utf8(payload) {
        let clean = text.trim();
        let lower = clean.to_lowercase();

        let is_p300_or_select = lower.contains("p300")
            || lower.contains("target")
            || lower.contains("select")
            || lower.contains("clench")
            || lower.contains("jaw")
            || lower.contains("blink")
            || lower.contains("switch")
            || lower.contains("trigger")
            || lower == "1"
            || lower == "click";

        if is_p300_or_select {
            if now.duration_since(*last_trigger) >= Duration::from_millis(350) {
                *last_trigger = now;
                inject_switch_event(SwitchKeyEvent::Select);
                if let Ok(mut last) = LAST_BCI_EVENT.lock() {
                    *last = Some((format!("Marqueur LSL: \"{}\"", clean.chars().take(24).collect::<String>()), now));
                }
                crate::overlay::play_earcon(crate::overlay::Earcon::Success);
            }
            return;
        } else if lower.contains("next") || lower.contains("right") {
            if now.duration_since(*last_trigger) >= Duration::from_millis(250) {
                *last_trigger = now;
                inject_switch_event(SwitchKeyEvent::Next);
                if let Ok(mut last) = LAST_BCI_EVENT.lock() {
                    *last = Some(("Commande BCI: Suivant".to_string(), now));
                }
            }
            return;
        } else if lower.contains("prev") || lower.contains("left") {
            if now.duration_since(*last_trigger) >= Duration::from_millis(250) {
                *last_trigger = now;
                inject_switch_event(SwitchKeyEvent::Prev);
                if let Ok(mut last) = LAST_BCI_EVENT.lock() {
                    *last = Some(("Commande BCI: Précédent".to_string(), now));
                }
            }
            return;
        } else if lower.contains("back") || lower.contains("escape") {
            if now.duration_since(*last_trigger) >= Duration::from_millis(300) {
                *last_trigger = now;
                inject_switch_event(SwitchKeyEvent::Back);
                if let Ok(mut last) = LAST_BCI_EVENT.lock() {
                    *last = Some(("Commande BCI: Retour".to_string(), now));
                }
            }
            return;
        }
    }

    // 2. Traitement des flux binaires bruts de micro-contractions musculaires EMG (flotteurs 32-bit LE/BE)
    if payload.len() >= 16 && (payload.len() % 4 == 0) {
        let num_floats = payload.len() / 4;
        let mut sum_sq = 0.0f32;
        let mut sum = 0.0f32;

        for i in 0..num_floats {
            let b: [u8; 4] = payload[i * 4..(i + 1) * 4].try_into().unwrap_or_default();
            let val = f32::from_le_bytes(b);
            if val.is_finite() {
                sum += val;
                sum_sq += val * val;
            }
        }

        let count = num_floats as f32;
        let mean = sum / count;
        let variance = ((sum_sq / count) - (mean * mean)).max(0.0);
        let rms = variance.sqrt();

        // Suivi adaptatif doux du bruit de fond
        *baseline_rms = *baseline_rms * 0.95 + rms * 0.05;
        let net_rms = (rms - *baseline_rms).max(0.0);
        let threshold = get_bci_emg_threshold();

        if net_rms >= threshold && now.duration_since(*last_trigger) >= Duration::from_millis(380) {
            *last_trigger = now;
            inject_switch_event(SwitchKeyEvent::Select);
            if let Ok(mut last) = LAST_BCI_EVENT.lock() {
                *last = Some((format!("Contraction EMG (RMS: {:.3} > {:.3})", net_rms, threshold), now));
            }
            crate::overlay::play_earcon(crate::overlay::Earcon::Success);
        }
    }
}

pub fn start_bci_lsl_receiver() {
    if BCI_LSL_RUNNING.swap(true, Ordering::SeqCst) {
        return;
    }

    let port = get_bci_lsl_port();

    // 1. Thread d'écoute UDP (Broadcast / Multicast LSL)
    std::thread::Builder::new()
        .name("bci_lsl_udp_worker".to_string())
        .spawn(move || {
            let bind_addr = format!("0.0.0.0:{}", port);
            let socket = match UdpSocket::bind(&bind_addr) {
                Ok(s) => {
                    let _ = s.set_broadcast(true);
                    let _ = s.set_read_timeout(Some(Duration::from_millis(300)));
                    println!("[BCI/LSL] Récepteur UDP actif sur {}", bind_addr);
                    s
                }
                Err(e) => {
                    eprintln!("[BCI/LSL] Impossible de lier le port UDP {port}: {e}");
                    return;
                }
            };

            let mut buf = [0u8; 4096];
            let mut last_trigger = Instant::now() - Duration::from_secs(1);
            let mut baseline_rms = 0.005f32;

            while BCI_LSL_RUNNING.load(Ordering::Relaxed) {
                match socket.recv_from(&mut buf) {
                    Ok((len, _src)) if len > 0 => {
                        process_bci_lsl_packet(&buf[..len], &mut last_trigger, &mut baseline_rms);
                    }
                    _ => {
                        std::thread::sleep(Duration::from_millis(15));
                    }
                }
            }
        })
        .expect("Échec lancement worker bci_lsl_udp_worker");

    // 2. Thread d'écoute TCP pour les ponts LSL dédiés
    std::thread::Builder::new()
        .name("bci_lsl_tcp_worker".to_string())
        .spawn(move || {
            let bind_addr = format!("0.0.0.0:{}", port);
            let listener = match TcpListener::bind(&bind_addr) {
                Ok(l) => {
                    let _ = l.set_nonblocking(true);
                    println!("[BCI/LSL] Serveur TCP actif sur {}", bind_addr);
                    l
                }
                Err(_) => return,
            };

            while BCI_LSL_RUNNING.load(Ordering::Relaxed) {
                if let Ok((mut stream, _)) = listener.accept() {
                    let _ = stream.set_read_timeout(Some(Duration::from_millis(200)));
                    let mut buf = [0u8; 1024];
                    let mut last_trigger = Instant::now() - Duration::from_secs(1);
                    let mut baseline = 0.005f32;
                    while BCI_LSL_RUNNING.load(Ordering::Relaxed) {
                        match stream.read(&mut buf) {
                            Ok(n) if n > 0 => {
                                process_bci_lsl_packet(&buf[..n], &mut last_trigger, &mut baseline);
                            }
                            Ok(_) => break,
                            Err(ref e) if e.kind() == std::io::ErrorKind::WouldBlock || e.kind() == std::io::ErrorKind::TimedOut => {
                                std::thread::sleep(Duration::from_millis(10));
                            }
                            Err(_) => break,
                        }
                    }
                } else {
                    std::thread::sleep(Duration::from_millis(100));
                }
            }
        })
        .expect("Échec lancement worker bci_lsl_tcp_worker");
}

pub fn stop_bci_lsl_receiver() {
    BCI_LSL_RUNNING.store(false, Ordering::SeqCst);
    BCI_LSL_CONNECTED.store(false, Ordering::Relaxed);
}

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
    (0x41, "A"),
    (0x42, "B"),
    (0x43, "C"),
    (0x44, "D"),
    (0x45, "E"),
    (0x46, "F"),
    (0x47, "G"),
    (0x48, "H"),
    (0x49, "I"),
    (0x4A, "J"),
    (0x4B, "K"),
    (0x4C, "L"),
    (0x4D, "M"),
    (0x4E, "N"),
    (0x4F, "O"),
    (0x50, "P"),
    (0x51, "Q"),
    (0x52, "R"),
    (0x53, "S"),
    (0x54, "T"),
    (0x55, "U"),
    (0x56, "V"),
    (0x57, "W"),
    (0x58, "X"),
    (0x59, "Y"),
    (0x5A, "Z"),
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

pub fn vk_to_label(binding: u32) -> String {
    let vk = binding & VK_KEY_MASK;
    let mut parts = Vec::new();
    if (binding & MOD_CTRL) != 0 { parts.push("Ctrl"); }
    if (binding & MOD_ALT) != 0 { parts.push("Alt"); }
    if (binding & MOD_SHIFT) != 0 { parts.push("Shift"); }
    if (binding & MOD_WIN) != 0 { parts.push("Win"); }

    let base_label = get_base_vk_label(vk);
    parts.push(&base_label);
    parts.join(" + ")
}

pub fn get_base_vk_label(vk: u32) -> String {
    for &(code, label) in AVAILABLE_REBIND_KEYS {
        if code == vk { return label.to_string(); }
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
        Key::Quote => Some(0xDE),
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
        0x41 => Some(Key::A), 0x42 => Some(Key::B), 0x43 => Some(Key::C),
        0x44 => Some(Key::D), 0x45 => Some(Key::E), 0x46 => Some(Key::F),
        0x47 => Some(Key::G), 0x48 => Some(Key::H), 0x49 => Some(Key::I),
        0x4A => Some(Key::J), 0x4B => Some(Key::K), 0x4C => Some(Key::L),
        0x4D => Some(Key::M), 0x4E => Some(Key::N), 0x4F => Some(Key::O),
        0x50 => Some(Key::P), 0x51 => Some(Key::Q), 0x52 => Some(Key::R),
        0x53 => Some(Key::S), 0x54 => Some(Key::T), 0x55 => Some(Key::U),
        0x56 => Some(Key::V), 0x57 => Some(Key::W), 0x58 => Some(Key::X),
        0x59 => Some(Key::Y), 0x5A => Some(Key::Z),
        0x30 => Some(Key::Num0), 0x31 => Some(Key::Num1), 0x32 => Some(Key::Num2),
        0x33 => Some(Key::Num3), 0x34 => Some(Key::Num4), 0x35 => Some(Key::Num5),
        0x36 => Some(Key::Num6), 0x37 => Some(Key::Num7), 0x38 => Some(Key::Num8),
        0x39 => Some(Key::Num9),
        0x1B => Some(Key::Escape),
        0x20 => Some(Key::Space),
        0x0D => Some(Key::Enter),
        0x09 => Some(Key::Tab),
        0x08 => Some(Key::Backspace),
        0x25 => Some(Key::ArrowLeft),
        0x26 => Some(Key::ArrowUp),
        0x27 => Some(Key::ArrowRight),
        0x28 => Some(Key::ArrowDown),
        0xBD => Some(Key::Minus),
        0xBB => Some(Key::Plus),
        0xBC => Some(Key::Comma),
        0xBE => Some(Key::Period),
        0xBF => Some(Key::Slash),
        0xDC => Some(Key::Backslash),
        0xBA => Some(Key::Colon),
        0xDD => Some(Key::OpenBracket),
        0xDE => Some(Key::Quote),
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
    KEY_BIND_SELECT.store(load_vk("KEY_BIND_SELECT", 0x20), Ordering::SeqCst);
    KEY_BIND_NEXT.store(load_vk("KEY_BIND_NEXT", 0x27), Ordering::SeqCst);
    KEY_BIND_PREV.store(load_vk("KEY_BIND_PREV", 0x25), Ordering::SeqCst);
    KEY_BIND_BACK.store(load_vk("KEY_BIND_BACK", 0x08), Ordering::SeqCst);
    KEY_BIND_GAZE_CLICK.store(load_vk("KEY_BIND_GAZE_CLICK", 0x0D), Ordering::SeqCst);
    KEY_BIND_STOP_SPEECH.store(load_vk("KEY_BIND_STOP_SPEECH", 0x13), Ordering::SeqCst);
    KEY_BIND_EMERGENCY.store(load_vk("KEY_BIND_EMERGENCY", 0x1B), Ordering::SeqCst);

    if let Some(th_str) = std::env::var("BCI_EMG_THRESHOLD").ok().or_else(|| read_env_var_from_file(".env", "BCI_EMG_THRESHOLD")) {
        if let Ok(th) = th_str.trim().parse::<f32>() {
            BCI_EMG_THRESHOLD.store(th.to_bits(), Ordering::Relaxed);
        }
    }
    if let Some(p_str) = std::env::var("LSL_BCI_PORT").ok().or_else(|| read_env_var_from_file(".env", "LSL_BCI_PORT")) {
        if let Ok(p) = p_str.trim().parse::<u32>() {
            BCI_LSL_PORT.store(p, Ordering::Relaxed);
        }
    }

    if let Some(val) = std::env::var("STEADY_CLICK_ENABLED").ok().or_else(|| read_env_var_from_file(".env", "STEADY_CLICK_ENABLED")) {
        STEADY_CLICK_ENABLED.store(val.trim().eq_ignore_ascii_case("true"), Ordering::Relaxed);
    }
    if let Some(val) = std::env::var("PREVENT_ACCIDENTAL_DRAG").ok().or_else(|| read_env_var_from_file(".env", "PREVENT_ACCIDENTAL_DRAG")) {
        PREVENT_ACCIDENTAL_DRAG.store(val.trim().eq_ignore_ascii_case("true"), Ordering::Relaxed);
    }
    if let Some(val) = std::env::var("BOUNCE_KEYS_ENABLED").ok().or_else(|| read_env_var_from_file(".env", "BOUNCE_KEYS_ENABLED")) {
        BOUNCE_KEYS_ENABLED.store(val.trim().eq_ignore_ascii_case("true"), Ordering::Relaxed);
    }
    if let Some(val) = std::env::var("BOUNCE_KEYS_MS").ok().or_else(|| read_env_var_from_file(".env", "BOUNCE_KEYS_MS")) {
        if let Ok(ms) = val.trim().parse::<u64>() {
            BOUNCE_KEYS_MS.store(ms, Ordering::Relaxed);
        }
    }
    if let Some(val) = std::env::var("TREMOR_FILTER_ENABLED").ok().or_else(|| read_env_var_from_file(".env", "TREMOR_FILTER_ENABLED")) {
        TREMOR_FILTER_ENABLED.store(val.trim().eq_ignore_ascii_case("true"), Ordering::Relaxed);
    }
    if let Some(val) = std::env::var("SPASM_SHOCK_GUARD").ok().or_else(|| read_env_var_from_file(".env", "SPASM_SHOCK_GUARD")) {
        SPASM_SHOCK_GUARD.store(val.trim().eq_ignore_ascii_case("true"), Ordering::Relaxed);
    }

    let mut lock = match CUSTOM_KEY_REMAPS.lock() {
        Ok(guard) => guard,
        Err(poisoned) => poisoned.into_inner(),
    };
    *lock = load_custom_key_remaps();
}

#[cfg(windows)]
extern "system" {
    fn VkKeyScanW(ch: u16) -> i16;
}

/// Résout le code virtuel (VK) associé à un caractère selon le layout clavier actif
pub fn char_to_vk(c: char) -> Option<u32> {
    #[cfg(windows)]
    unsafe {
        let mut buf = [0u16; 2];
        let enc = c.encode_utf16(&mut buf);
        if enc.len() == 1 {
            let res = VkKeyScanW(enc[0]);
            if res != -1 {
                let vk = (res as u32) & 0xFF;
                return Some(vk);
            }
        }
    }
    azerty_char_to_vk(&c.to_string())
}

/// Résout le caractère généré par un code virtuel selon l'état de la touche Majuscule
pub fn vk_to_char_layout(vk: u32, shift: bool) -> Option<char> {
    let base_vk = vk & VK_KEY_MASK;
    match base_vk {
        0x41..=0x5A => {
            let offset = (base_vk - 0x41) as u8;
            Some(if shift { (b'A' + offset) as char } else { (b'a' + offset) as char })
        }
        0x30..=0x39 => Some((b'0' + (base_vk - 0x30) as u8) as char),
        // Pavé numérique (Numpad 0..=9 et opérateurs)
        0x60..=0x69 => Some((b'0' + (base_vk - 0x60) as u8) as char),
        0x6A => Some('*'),
        0x6B => Some('+'),
        0x6D => Some('-'),
        0x6E => Some('.'),
        0x6F => Some('/'),
        0x20 => Some(' '),
        0x0D => Some('\n'),
        0x09 => Some('\t'),
        // Symboles et ponctuation usuels (AZERTY / standard)
        0xDE => Some('²'),
        0xBD => Some(if shift { '_' } else { '-' }),
        0xBB => Some(if shift { '+' } else { '=' }),
        0xBC => Some(if shift { '?' } else { ',' }),
        0xBE => Some(if shift { '.' } else { ';' }),
        0xBF => Some(if shift { '/' } else { ':' }),
        0xBA => Some(if shift { '£' } else { '$' }),
        0xC0 => Some(if shift { '%' } else { 'ù' }),
        0xDC => Some(if shift { 'µ' } else { '*' }),
        0xDD => Some(if shift { '¨' } else { '^' }),
        0xDF => Some(if shift { '§' } else { '!' }),
        0xDB => Some(if shift { '°' } else { ')' }),
        _ => None,
    }
}

/// Réécrit directement les événements egui (`Event::Key` et `Event::Text`) selon `key_remaps.json`
pub fn apply_egui_key_remaps(ctx: &egui::Context) {
    let remaps = {
        let mut lock = match CUSTOM_KEY_REMAPS.lock() {
            Ok(guard) => guard,
            Err(poisoned) => poisoned.into_inner(),
        };
        if lock.is_empty() {
            *lock = load_custom_key_remaps();
        }
        if lock.is_empty() {
            return;
        }
        lock.clone()
    };

    let shift = ctx.input(|i| i.modifiers.shift);

    ctx.input_mut(|i| {
        let mut injected_text_events = Vec::new();

        for ev in &mut i.events {
            match ev {
                egui::Event::Key { key, physical_key, pressed, .. } => {
                    let vk_opt = egui_key_to_vk(*key)
                        .or_else(|| physical_key.and_then(egui_key_to_vk));
                    if let Some(vk) = vk_opt {
                        if let Some(r) = remaps.iter().find(|r| r.enabled && (r.from_key & VK_KEY_MASK) == (vk & VK_KEY_MASK)) {
                            let target_base_vk = r.to_key & VK_KEY_MASK;
                            if let Some(new_key) = vk_to_egui_key(target_base_vk) {
                                *key = new_key;
                                if let Some(pk) = physical_key {
                                    *pk = new_key;
                                }
                            }
                            // Si la touche source est non imprimable (F1..=F12, etc.) et ne produit pas d'Event::Text,
                            // alors que la touche de destination est un caractère textuel (Espace, lettre, chiffre...),
                            // on injecte l'Event::Text manquant pour alimenter les TextEdit d'egui.
                            if *pressed && vk_to_char_layout(vk, shift).is_none() {
                                if let Some(c) = vk_to_char_layout(target_base_vk, shift) {
                                    injected_text_events.push(egui::Event::Text(c.to_string()));
                                }
                            }
                        }
                    }
                }
                egui::Event::Text(text) => {
                    let matched_remap = text.chars().next().and_then(|c| {
                        char_to_vk(c).and_then(|vk| {
                            remaps.iter().find(|r| r.enabled && (r.from_key & VK_KEY_MASK) == (vk & VK_KEY_MASK)).copied()
                        })
                    });

                    if let Some(r) = matched_remap {
                        let target_base_vk = r.to_key & VK_KEY_MASK;
                        if let Some(c) = vk_to_char_layout(target_base_vk, shift) {
                            *text = c.to_string();
                        }
                    }
                }
                _ => {}
            }
        }

        if !injected_text_events.is_empty() {
            i.events.extend(injected_text_events);
        }
    });
}

#[cfg(windows)]
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
enum HookCaptureKind {
    None,
    GlobalAction,
    GazeClick,
    Digit,
    SwitchNav(SwitchKeyEvent),
}

#[cfg(windows)]
static HOOK_KEYS_DOWN: Mutex<[HookCaptureKind; 256]> = Mutex::new([HookCaptureKind::None; 256]);
static HOOK_KEYS_DOWN_TIME: Mutex<[Option<Instant>; 256]> = Mutex::new([None; 256]);
#[cfg(windows)]
static REMAPPED_KEYS_DOWN: Mutex<[Option<u32>; 256]> = Mutex::new([None; 256]);
#[cfg(windows)]
static BOUNCE_KEYS_LAST_UP: Mutex<[Option<Instant>; 256]> = Mutex::new([None; 256]);

#[cfg(windows)]
static MOUSE_ANCHOR_POS: Mutex<Option<POINT>> = Mutex::new(None);
#[cfg(windows)]
static MOUSE_BUTTON_DOWN: AtomicBool = AtomicBool::new(false);
#[cfg(windows)]
static LAST_STABLE_MOUSE_POS: Mutex<Option<(POINT, Instant)>> = Mutex::new(None);
#[cfg(windows)]
static LAST_RAW_MOUSE_POS: Mutex<Option<(POINT, Instant)>> = Mutex::new(None);

pub fn reset_switch_keys() {
    if let Ok(mut q) = SWITCH_KEY_QUEUE.lock() {
        q.clear();
    }
    if let Ok(mut nq) = NUMBER_KEY_QUEUE.lock() {
        nq.clear();
    }
    #[cfg(windows)]
    if let Ok(mut keys) = HOOK_KEYS_DOWN.lock() {
        *keys = [HookCaptureKind::None; 256];
    }
    #[cfg(windows)]
    if let Ok(mut rkd) = REMAPPED_KEYS_DOWN.lock() {
        *rkd = [None; 256];
    }
    if let Ok(mut times) = HOOK_KEYS_DOWN_TIME.lock() {
        *times = [None; 256];
    }
    #[cfg(windows)]
    if let Ok(mut bu) = BOUNCE_KEYS_LAST_UP.lock() {
        *bu = [None; 256];
    }
    #[cfg(windows)]
    {
        MOUSE_BUTTON_DOWN.store(false, Ordering::Relaxed);
        if let Ok(mut a) = MOUSE_ANCHOR_POS.lock() {
            *a = None;
        }
        if let Ok(mut s) = LAST_STABLE_MOUSE_POS.lock() {
            *s = None;
        }
        if let Ok(mut r) = LAST_RAW_MOUSE_POS.lock() {
            *r = None;
        }
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
pub fn inject_remapped_key(to_binding: u32, is_down: bool) {
    use windows::Win32::UI::Input::KeyboardAndMouse::{
        SendInput, INPUT, INPUT_0, INPUT_KEYBOARD, KEYBDINPUT, KEYBD_EVENT_FLAGS,
        KEYEVENTF_KEYUP, KEYEVENTF_EXTENDEDKEY,
    };
    extern "system" {
        fn MapVirtualKeyW(uCode: u32, uMapType: u32) -> u32;
    }

    let target_vk = (to_binding & VK_KEY_MASK) as u16;
    let target_mods = to_binding & MOD_MASK;
    let scan = unsafe { MapVirtualKeyW(target_vk as u32, 0) as u16 };

    let is_extended_vk = |vk_code: u16| -> bool {
        matches!(
            vk_code,
            0x21..=0x28 // VK_PRIOR, VK_NEXT, VK_END, VK_HOME, VK_LEFT, VK_UP, VK_RIGHT, VK_DOWN
            | 0x2D | 0x2E // VK_INSERT, VK_DELETE
            | 0x5B..=0x5D // VK_LWIN, VK_RWIN, VK_APPS
            | 0x6F        // VK_DIVIDE
            | 0x90        // VK_NUMLOCK
            | 0xAD..=0xB3 // Touches média et volume
        )
    };

    let make_key = |vk: u16, sc: u16, up: bool| -> INPUT {
        let mut flags = KEYBD_EVENT_FLAGS(0);
        if up {
            flags |= KEYEVENTF_KEYUP;
        }
        if is_extended_vk(vk) {
            flags |= KEYEVENTF_EXTENDEDKEY;
        }
        INPUT {
            r#type: INPUT_KEYBOARD,
            Anonymous: INPUT_0 {
                ki: KEYBDINPUT {
                    wVk: VIRTUAL_KEY(vk),
                    wScan: sc,
                    dwFlags: flags,
                    time: 0,
                    dwExtraInfo: LIBERTIDE_EXTRA_INFO,
                },
            },
        }
    };

    let mut inputs = Vec::new();
    if is_down {
        if (target_mods & MOD_CTRL) != 0 { inputs.push(make_key(0x11, unsafe { MapVirtualKeyW(0x11, 0) as u16 }, false)); }
        if (target_mods & MOD_ALT) != 0 { inputs.push(make_key(0x12, unsafe { MapVirtualKeyW(0x12, 0) as u16 }, false)); }
        if (target_mods & MOD_SHIFT) != 0 { inputs.push(make_key(0x10, unsafe { MapVirtualKeyW(0x10, 0) as u16 }, false)); }
        if (target_mods & MOD_WIN) != 0 { inputs.push(make_key(0x5B, unsafe { MapVirtualKeyW(0x5B, 0) as u16 }, false)); }
        inputs.push(make_key(target_vk, scan, false));
    } else {
        inputs.push(make_key(target_vk, scan, true));
        if (target_mods & MOD_WIN) != 0 { inputs.push(make_key(0x5B, unsafe { MapVirtualKeyW(0x5B, 0) as u16 }, true)); }
        if (target_mods & MOD_SHIFT) != 0 { inputs.push(make_key(0x10, unsafe { MapVirtualKeyW(0x10, 0) as u16 }, true)); }
        if (target_mods & MOD_ALT) != 0 { inputs.push(make_key(0x12, unsafe { MapVirtualKeyW(0x12, 0) as u16 }, true)); }
        if (target_mods & MOD_CTRL) != 0 { inputs.push(make_key(0x11, unsafe { MapVirtualKeyW(0x11, 0) as u16 }, true)); }
    }

    if !inputs.is_empty() {
        unsafe {
            SendInput(&inputs, std::mem::size_of::<INPUT>() as i32);
        }
    }
}

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

        let vk_idx = (vk as usize) & 0xFF;

        // 3. Filtre Anti-Mitraillette Clavier (Adaptive BounceKeys)
        if is_bounce_keys_enabled() && !is_injected {
            if is_up {
                if let Ok(mut ups) = BOUNCE_KEYS_LAST_UP.lock() {
                    ups[vk_idx] = Some(Instant::now());
                }
            } else if is_down {
                let bounce_ms = get_bounce_keys_ms();
                let mut is_bouncing = false;
                if let Ok(mut ups) = BOUNCE_KEYS_LAST_UP.lock() {
                    if let Some(last_up) = ups[vk_idx] {
                        if last_up.elapsed() < Duration::from_millis(bounce_ms) {
                            is_bouncing = true;
                        } else {
                            ups[vk_idx] = None;
                        }
                    }
                }
                if is_bouncing {
                    return windows::Win32::Foundation::LRESULT(1);
                }
            }
        }

        let is_alt = (kbd.flags.0 & 0x20) != 0
            || TRACKED_ALT.load(Ordering::Relaxed)
            || (!is_powertoys
                && !is_injected
                && (windows::Win32::UI::Input::KeyboardAndMouse::GetAsyncKeyState(0x12) as u16 & 0x8000) != 0);
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
        let is_win = if is_powertoys {
            false
        } else if is_injected {
            TRACKED_WIN.load(Ordering::Relaxed)
        } else {
            TRACKED_WIN.load(Ordering::Relaxed)
                || (windows::Win32::UI::Input::KeyboardAndMouse::GetAsyncKeyState(0x5B) as u16 & 0x8000) != 0
                || (windows::Win32::UI::Input::KeyboardAndMouse::GetAsyncKeyState(0x5C) as u16 & 0x8000) != 0
        };

        let mut active_mods = 0u32;
        if is_ctrl { active_mods |= MOD_CTRL; }
        if is_alt { active_mods |= MOD_ALT; }
        if is_shift { active_mods |= MOD_SHIFT; }
        if is_win { active_mods |= MOD_WIN; }

        let is_switch_hook_on = SWITCH_HOOK_ENABLED.load(Ordering::Relaxed);
        let is_dictation = IS_DICTATION_ACTIVE.load(Ordering::Relaxed);

        let key_switch = KEY_BIND_SWITCH.load(Ordering::Relaxed);
        let key_dictation = KEY_BIND_DICTATION.load(Ordering::Relaxed);
        let key_voice = KEY_BIND_VOICE.load(Ordering::Relaxed);
        let key_clear = KEY_BIND_CLEAR.load(Ordering::Relaxed);
        let key_stop_speech = KEY_BIND_STOP_SPEECH.load(Ordering::Relaxed);
        let key_emergency = KEY_BIND_EMERGENCY.load(Ordering::Relaxed);

        if is_down {
            if let Ok(mut lock) = REBIND_LISTENING_TARGET.lock() {
                if let Some(target_var) = lock.take() {
                    // Ignorer les appuis isolés sur les touches modificatrices pour attendre la touche finale
                    if is_modifier_vk(vk) {
                        *lock = Some(target_var);
                        return CallNextHookEx(None, code, wparam, lparam);
                    }

                    // Annulation sécurisée par Échap uniquement si AUCUN modificateur n'est pressé
                    if vk == 0x1B && active_mods == 0 {
                        trigger_repaint();
                        return windows::Win32::Foundation::LRESULT(1);
                    }

                    let full_binding = (vk & VK_KEY_MASK) | (active_mods & MOD_MASK);
                    apply_rebound_key(&target_var, full_binding);
                    trigger_repaint();
                    return windows::Win32::Foundation::LRESULT(1);
                }
            }
        }

        let fg_hwnd = GetForegroundWindow();
        let current_pid = std::process::id();
        let mut fg_pid = 0u32;
        windows::Win32::UI::WindowsAndMessaging::GetWindowThreadProcessId(fg_hwnd, Some(&mut fg_pid));
        let is_libertide_fg = fg_pid == current_pid;

        // Exécution du remappage personnalisé de n'importe quelle touche (actif partout, y compris dans les inputs)
        let is_rebinding = if let Ok(lock) = REBIND_LISTENING_TARGET.lock() {
            lock.is_some()
        } else {
            false
        };

        // Pour Libertide, on ne bloque pas les messages clavier avec SendInput :
        // apply_egui_key_remaps() transforme directement les événements dans le pipeline d'egui.
        if !is_injected && !is_rebinding && !is_libertide_fg {
            let was_remapped = if let Ok(down_keys) = REMAPPED_KEYS_DOWN.lock() {
                down_keys[vk_idx]
            } else {
                None
            };

            if is_down {
                let custom_remap = {
                    let mut remaps = match CUSTOM_KEY_REMAPS.lock() {
                        Ok(guard) => guard,
                        Err(poisoned) => poisoned.into_inner(),
                    };
                    if remaps.is_empty() {
                        *remaps = load_custom_key_remaps();
                    }
                    remaps.iter().find(|r| r.enabled && matches_remap(r.from_key, vk, active_mods)).copied()
                };

                if let Some(remap) = custom_remap {
                    if let Ok(mut down_keys) = REMAPPED_KEYS_DOWN.lock() {
                        down_keys[vk_idx] = Some(remap.to_key);
                    }
                    inject_remapped_key(remap.to_key, true);
                    return windows::Win32::Foundation::LRESULT(1);
                } else if let Some(to_key) = was_remapped {
                    // Répétition de touche lors d'un appui maintenu (auto-repeat)
                    inject_remapped_key(to_key, true);
                    return windows::Win32::Foundation::LRESULT(1);
                }
            } else if is_up {
                let to_key_opt = if let Ok(mut down_keys) = REMAPPED_KEYS_DOWN.lock() {
                    down_keys[vk_idx].take()
                } else {
                    None
                };

                let target_to_key = to_key_opt.or_else(|| {
                    let mut remaps = match CUSTOM_KEY_REMAPS.lock() {
                        Ok(guard) => guard,
                        Err(poisoned) => poisoned.into_inner(),
                    };
                    if remaps.is_empty() {
                        *remaps = load_custom_key_remaps();
                    }
                    remaps.iter().find(|r| r.enabled && matches_remap(r.from_key, vk, active_mods)).map(|r| r.to_key)
                });

                if let Some(to_key) = target_to_key {
                    inject_remapped_key(to_key, false);
                    return windows::Win32::Foundation::LRESULT(1);
                }
            }
        }

        let overlay_hwnd = FindWindowW(None, w!("Libertide overlay")).unwrap_or(HWND(std::ptr::null_mut()));
        let fg_hwnd = GetForegroundWindow();
        let is_libertide_fg = !overlay_hwnd.0.is_null() && fg_hwnd == overlay_hwnd;
        let is_user_typing = IS_USER_TYPING.load(Ordering::Relaxed) && is_libertide_fg;

        let captured_kind = if let Ok(keys) = HOOK_KEYS_DOWN.lock() {
            keys[vk_idx]
        } else {
            HookCaptureKind::None
        };
        let was_captured_down = captured_kind != HookCaptureKind::None;

        let global_action_event = if matches_binding(key_switch, vk, active_mods) {
            Some(SwitchKeyEvent::ToggleSwitch)
        } else if matches_binding(key_dictation, vk, active_mods) {
            Some(SwitchKeyEvent::ToggleDictation)
        } else if matches_binding(key_voice, vk, active_mods) {
            Some(SwitchKeyEvent::ToggleVoice)
        } else if matches_binding(key_clear, vk, active_mods) {
            Some(SwitchKeyEvent::Clear)
        } else if matches_binding(key_stop_speech, vk, active_mods) || (key_stop_speech == 0x13 && vk == 0x13 && active_mods == 0) {
            Some(SwitchKeyEvent::StopSpeech)
        } else if matches_binding(key_emergency, vk, active_mods) || (vk == 0x1B && active_mods == 0 && (is_dictation || is_switch_hook_on)) {
            Some(SwitchKeyEvent::Escape)
        } else {
            None
        };

        if let Some(event) = global_action_event {
            let now = Instant::now();
            if is_down {
                let mut already_down = false;
                if let Ok(mut keys) = HOOK_KEYS_DOWN.lock() {
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
                    already_down = keys[vk_idx] != HookCaptureKind::None;
                    keys[vk_idx] = HookCaptureKind::GlobalAction;
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
                    keys[vk_idx] = HookCaptureKind::None;
                }
                if let Ok(mut times) = HOOK_KEYS_DOWN_TIME.lock() {
                    times[vk_idx] = None;
                }
            }
            if is_down || is_up {
                return windows::Win32::Foundation::LRESULT(1);
            }
        } else if is_up && was_captured_down && captured_kind == HookCaptureKind::GlobalAction {
            if let Ok(mut keys) = HOOK_KEYS_DOWN.lock() {
                keys[vk_idx] = HookCaptureKind::None;
            }
            if let Ok(mut times) = HOOK_KEYS_DOWN_TIME.lock() {
                times[vk_idx] = None;
            }
            return windows::Win32::Foundation::LRESULT(1);
        }

        if is_user_typing {
            return CallNextHookEx(None, code, wparam, lparam);
        }

        let is_eye_active = crate::eyetracking::is_eye_tracking_enabled()
            && !crate::eyetracking::is_calibration_active();
        let key_gaze = KEY_BIND_GAZE_CLICK.load(Ordering::Relaxed);
        let matches_gaze = matches_binding(key_gaze, vk, active_mods)
            || (key_gaze == 0x0D && vk == 0x0D && active_mods == 0);

        if is_eye_active && !is_dictation && matches_gaze {
            crate::eyetracking::ENTER_PRESSED.store(is_down, Ordering::Relaxed);
            let now = Instant::now();
            if is_down {
                let mut already_down = false;
                if let Ok(mut keys) = HOOK_KEYS_DOWN.lock() {
                    if keys[vk_idx] != HookCaptureKind::None {
                        already_down = true;
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
                    keys[vk_idx] = HookCaptureKind::GazeClick;
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
                    keys[vk_idx] = HookCaptureKind::None;
                }
                if let Ok(mut times) = HOOK_KEYS_DOWN_TIME.lock() {
                    times[vk_idx] = None;
                }
            }
            return windows::Win32::Foundation::LRESULT(1);
        }
        if is_up && was_captured_down && captured_kind == HookCaptureKind::GazeClick {
            crate::eyetracking::ENTER_PRESSED.store(false, Ordering::Relaxed);
            if let Ok(mut keys) = HOOK_KEYS_DOWN.lock() {
                keys[vk_idx] = HookCaptureKind::None;
            }
            if let Ok(mut times) = HOOK_KEYS_DOWN_TIME.lock() {
                times[vk_idx] = None;
            }
            return windows::Win32::Foundation::LRESULT(1);
        }

        if !is_switch_hook_on {
            return CallNextHookEx(None, code, wparam, lparam);
        }

        if !overlay_hwnd.0.is_null() && IsIconic(overlay_hwnd).as_bool() && is_switch_hook_on {
            let _ = ShowWindow(overlay_hwnd, windows::Win32::UI::WindowsAndMessaging::SW_SHOWNOACTIVATE);
        }

        if is_switch_hook_on && active_mods == 0 {
            let digit_char = match vk {
                0x60..=0x69 => Some((b'0' + (vk as u8 - 0x60)) as char),
                0x30..=0x39 => Some((b'0' + (vk as u8 - 0x30)) as char),
                _ => None,
            };
            if let Some(digit) = digit_char {
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
                        keys[vk_idx] = HookCaptureKind::Digit;
                    }
                    if allow_digit {
                        if let Ok(mut nq) = NUMBER_KEY_QUEUE.lock() {
                            nq.push(digit);
                        }
                        trigger_repaint();
                    }
                } else if is_up {
                    if let Ok(mut keys) = HOOK_KEYS_DOWN.lock() {
                        keys[vk_idx] = HookCaptureKind::None;
                    }
                    if let Ok(mut times) = HOOK_KEYS_DOWN_TIME.lock() {
                        times[vk_idx] = None;
                    }
                }
                if is_down || is_up {
                    return windows::Win32::Foundation::LRESULT(1);
                }
            }
        } else if is_up && was_captured_down && captured_kind == HookCaptureKind::Digit {
            if let Ok(mut keys) = HOOK_KEYS_DOWN.lock() {
                keys[vk_idx] = HookCaptureKind::None;
            }
            if let Ok(mut times) = HOOK_KEYS_DOWN_TIME.lock() {
                times[vk_idx] = None;
            }
            return windows::Win32::Foundation::LRESULT(1);
        }

        let key_select = KEY_BIND_SELECT.load(Ordering::Relaxed);
        let key_next = KEY_BIND_NEXT.load(Ordering::Relaxed);
        let key_prev = KEY_BIND_PREV.load(Ordering::Relaxed);
        let key_back = KEY_BIND_BACK.load(Ordering::Relaxed);

        let switch_event = if matches_binding(key_select, vk, active_mods)
            || (key_select == 0x20 && active_mods == 0 && (vk == 0x0D || vk == 0x20))
        {
            Some(SwitchKeyEvent::Select)
        } else if matches_binding(key_next, vk, active_mods)
            || (key_next == 0x27 && (active_mods == 0 && (vk == 0x27 || vk == 0x28 || vk == 0x22 || vk == 0x09)))
        {
            Some(SwitchKeyEvent::Next)
        } else if matches_binding(key_prev, vk, active_mods)
            || (key_prev == 0x25 && ((active_mods == 0 && (vk == 0x25 || vk == 0x26 || vk == 0x21)) || (active_mods == MOD_SHIFT && vk == 0x09)))
        {
            Some(SwitchKeyEvent::Prev)
        } else if matches_binding(key_back, vk, active_mods)
            || (key_back == 0x08 && active_mods == 0 && vk == 0x08)
        {
            Some(SwitchKeyEvent::Back)
        } else {
            None
        };

        if let Some(event) = switch_event {
            let now = Instant::now();
            if is_down {
                let mut already_down = false;
                if let Ok(mut keys) = HOOK_KEYS_DOWN.lock() {
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
                    already_down = keys[vk_idx] != HookCaptureKind::None;
                    keys[vk_idx] = HookCaptureKind::SwitchNav(event);
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
                    keys[vk_idx] = HookCaptureKind::None;
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
        } else if is_up && was_captured_down {
            if let HookCaptureKind::SwitchNav(event) = captured_kind {
                let press_duration = if let Ok(mut times) = HOOK_KEYS_DOWN_TIME.lock() {
                    times[vk_idx].take().map(|t| t.elapsed())
                } else {
                    None
                };
                if let Ok(mut keys) = HOOK_KEYS_DOWN.lock() {
                    keys[vk_idx] = HookCaptureKind::None;
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
    if code >= 0 {
        let ms = *(lparam.0 as *const MSLLHOOKSTRUCT);

        if ms.dwExtraInfo != LIBERTIDE_EXTRA_INFO {
            let msg = wparam.0 as u32;

            if SWITCH_HOOK_ENABLED.load(Ordering::Relaxed) && (msg == WM_MOUSEWHEEL || msg == WM_MOUSEHWHEEL) {
                clear_screen_numbered_badges();
                trigger_repaint();
            }

            // 1 & 2. Steady-Click & Drag-Lock Guard (Pression physique sur bouton souris)
            if msg == WM_LBUTTONDOWN || msg == WM_RBUTTONDOWN || msg == WM_MBUTTONDOWN {
                MOUSE_BUTTON_DOWN.store(true, Ordering::Relaxed);
                if let Ok(mut anchor) = MOUSE_ANCHOR_POS.lock() {
                    *anchor = Some(ms.pt);
                }
            } else if msg == WM_LBUTTONUP || msg == WM_RBUTTONUP || msg == WM_MBUTTONUP {
                MOUSE_BUTTON_DOWN.store(false, Ordering::Relaxed);
                let anchor_pt = if let Ok(mut anchor) = MOUSE_ANCHOR_POS.lock() {
                    anchor.take()
                } else {
                    None
                };
                if is_steady_click_enabled() {
                    if let Some(anchor) = anchor_pt {
                        let _ = SetCursorPos(anchor.x, anchor.y);
                    }
                }
            } else if msg == WM_MOUSEMOVE {
                let button_is_down = MOUSE_BUTTON_DOWN.load(Ordering::Relaxed);
                if button_is_down {
                    let steady_on = is_steady_click_enabled();
                    let prevent_drag_on = is_prevent_accidental_drag();

                    let mods = get_currently_pressed_modifiers();
                    let voluntary_drag = (mods & (MOD_CTRL | MOD_SHIFT)) != 0;

                    if (steady_on || prevent_drag_on) && !voluntary_drag {
                        if let Ok(anchor) = MOUSE_ANCHOR_POS.lock() {
                            if let Some(pt) = *anchor {
                                let _ = SetCursorPos(pt.x, pt.y);
                                return windows::Win32::Foundation::LRESULT(1);
                            }
                        }
                    }
                }

                let now = Instant::now();

                // 5. Absorbeur de Choc / Myoclonies (Spasm Shock Guard)
                if is_spasm_shock_guard() {
                    if let Ok(mut last_raw) = LAST_RAW_MOUSE_POS.lock() {
                        if let Some((prev_pt, prev_time)) = *last_raw {
                            let dt = now.duration_since(prev_time);
                            if dt <= Duration::from_millis(35) {
                                let dx = (ms.pt.x - prev_pt.x) as f64;
                                let dy = (ms.pt.y - prev_pt.y) as f64;
                                let dist = (dx * dx + dy * dy).sqrt();
                                // Détection d'accélération anormale (> 300 px en moins de 35 ms)
                                if dist > 300.0 {
                                    let _ = SetCursorPos(prev_pt.x, prev_pt.y);
                                    *last_raw = Some((prev_pt, now));
                                    return windows::Win32::Foundation::LRESULT(1);
                                }
                            }
                        }
                        *last_raw = Some((ms.pt, now));
                    }
                }

                // 4. Filtre Anti-Tremblement Souris (Tremor Deadzone)
                if is_tremor_filter_enabled() {
                    if let Ok(mut last_stable) = LAST_STABLE_MOUSE_POS.lock() {
                        if let Some((stable_pt, stable_time)) = *last_stable {
                            let dx = ms.pt.x - stable_pt.x;
                            let dy = ms.pt.y - stable_pt.y;
                            let dist_sq = dx * dx + dy * dy;
                            // Rayon de 7 px (7 * 7 = 49)
                            if dist_sq <= 49 {
                                if now.duration_since(stable_time) < Duration::from_millis(250) {
                                    let _ = SetCursorPos(stable_pt.x, stable_pt.y);
                                    return windows::Win32::Foundation::LRESULT(1);
                                } else {
                                    *last_stable = Some((ms.pt, now));
                                }
                            } else {
                                *last_stable = Some((ms.pt, now));
                            }
                        } else {
                            *last_stable = Some((ms.pt, now));
                        }
                    }
                }
            }
        }
    }
    CallNextHookEx(None, code, wparam, lparam)
}

#[cfg(windows)]
pub fn install_switch_keyboard_hook() {
    load_keybindings();
    start_companion_switch_server();
    start_bci_lsl_receiver();

    let mut guard = HOOK_WORKER.lock().unwrap_or_else(|e| e.into_inner());
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
    stop_companion_switch_server();
    stop_bci_lsl_receiver();

    let mut guard = HOOK_WORKER.lock().unwrap_or_else(|e| e.into_inner());
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

/// Convertit une touche rdev en code virtuel standard
#[cfg(not(windows))]
pub fn rdev_key_to_vk(key: rdev::Key) -> Option<u32> {
    match key {
        rdev::Key::F1 => Some(0x70),
        rdev::Key::F2 => Some(0x71),
        rdev::Key::F3 => Some(0x72),
        rdev::Key::F4 => Some(0x73),
        rdev::Key::F5 => Some(0x74),
        rdev::Key::F6 => Some(0x75),
        rdev::Key::F7 => Some(0x76),
        rdev::Key::F8 => Some(0x77),
        rdev::Key::F9 => Some(0x78),
        rdev::Key::F10 => Some(0x79),
        rdev::Key::F11 => Some(0x7A),
        rdev::Key::F12 => Some(0x7B),
        rdev::Key::Num0 => Some(0x30),
        rdev::Key::Num1 => Some(0x31),
        rdev::Key::Num2 => Some(0x32),
        rdev::Key::Num3 => Some(0x33),
        rdev::Key::Num4 => Some(0x34),
        rdev::Key::Num5 => Some(0x35),
        rdev::Key::Num6 => Some(0x36),
        rdev::Key::Num7 => Some(0x37),
        rdev::Key::Num8 => Some(0x38),
        rdev::Key::Num9 => Some(0x39),
        rdev::Key::Kp0 => Some(0x60),
        rdev::Key::Kp1 => Some(0x61),
        rdev::Key::Kp2 => Some(0x62),
        rdev::Key::Kp3 => Some(0x63),
        rdev::Key::Kp4 => Some(0x64),
        rdev::Key::Kp5 => Some(0x65),
        rdev::Key::Kp6 => Some(0x66),
        rdev::Key::Kp7 => Some(0x67),
        rdev::Key::Kp8 => Some(0x68),
        rdev::Key::Kp9 => Some(0x69),
        rdev::Key::KpMultiply => Some(0x6A),
        rdev::Key::KpMinus => Some(0x6D),
        rdev::Key::KpDelete => Some(0x6E),
        rdev::Key::KpDivide => Some(0x6F),
        rdev::Key::KpReturn => Some(0x0D),
        rdev::Key::KeyA => Some(0x41),
        rdev::Key::KeyB => Some(0x42),
        rdev::Key::KeyC => Some(0x43),
        rdev::Key::KeyD => Some(0x44),
        rdev::Key::KeyE => Some(0x45),
        rdev::Key::KeyF => Some(0x46),
        rdev::Key::KeyG => Some(0x47),
        rdev::Key::KeyH => Some(0x48),
        rdev::Key::KeyI => Some(0x49),
        rdev::Key::KeyJ => Some(0x4A),
        rdev::Key::KeyK => Some(0x4B),
        rdev::Key::KeyL => Some(0x4C),
        rdev::Key::KeyM => Some(0x4D),
        rdev::Key::KeyN => Some(0x4E),
        rdev::Key::KeyO => Some(0x4F),
        rdev::Key::KeyP => Some(0x50),
        rdev::Key::KeyQ => Some(0x51),
        rdev::Key::KeyR => Some(0x52),
        rdev::Key::KeyS => Some(0x53),
        rdev::Key::KeyT => Some(0x54),
        rdev::Key::KeyU => Some(0x55),
        rdev::Key::KeyV => Some(0x56),
        rdev::Key::KeyW => Some(0x57),
        rdev::Key::KeyX => Some(0x58),
        rdev::Key::KeyY => Some(0x59),
        rdev::Key::KeyZ => Some(0x5A),
        rdev::Key::Escape => Some(0x1B),
        rdev::Key::Space => Some(0x20),
        rdev::Key::Return => Some(0x0D),
        rdev::Key::Tab => Some(0x09),
        rdev::Key::Backspace => Some(0x08),
        rdev::Key::Pause => Some(0x13),
        rdev::Key::PageUp => Some(0x21),
        rdev::Key::PageDown => Some(0x22),
        rdev::Key::End => Some(0x23),
        rdev::Key::Home => Some(0x24),
        rdev::Key::LeftArrow => Some(0x25),
        rdev::Key::UpArrow => Some(0x26),
        rdev::Key::RightArrow => Some(0x27),
        rdev::Key::DownArrow => Some(0x28),
        rdev::Key::Insert => Some(0x2D),
        rdev::Key::Delete => Some(0x2E),
        rdev::Key::CapsLock => Some(0x14),
        rdev::Key::NumLock => Some(0x90),
        rdev::Key::ScrollLock => Some(0x91),
        rdev::Key::PrintScreen => Some(0x2C),
        rdev::Key::ControlLeft | rdev::Key::ControlRight => Some(0x11),
        rdev::Key::ShiftLeft | rdev::Key::ShiftRight => Some(0x10),
        rdev::Key::Alt | rdev::Key::AltGr => Some(0x12),
        rdev::Key::MetaLeft | rdev::Key::MetaRight => Some(0x5B),
        _ => None,
    }
}

#[cfg(not(windows))]
pub fn install_switch_keyboard_hook() {
    load_keybindings();
    start_companion_switch_server();
    start_bci_lsl_receiver();

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

                let key_switch = KEY_BIND_SWITCH.load(Ordering::Relaxed);
                let key_dictation = KEY_BIND_DICTATION.load(Ordering::Relaxed);
                let key_voice = KEY_BIND_VOICE.load(Ordering::Relaxed);
                let key_clear = KEY_BIND_CLEAR.load(Ordering::Relaxed);

                match event.event_type {
                    rdev::EventType::KeyPress(key) => {
                        match key {
                            rdev::Key::ControlLeft | rdev::Key::ControlRight => {
                                RDEV_TRACKED_CTRL.store(true, Ordering::Relaxed);
                            }
                            rdev::Key::ShiftLeft | rdev::Key::ShiftRight => {
                                RDEV_TRACKED_SHIFT.store(true, Ordering::Relaxed);
                            }
                            rdev::Key::Alt | rdev::Key::AltGr => {
                                RDEV_TRACKED_ALT.store(true, Ordering::Relaxed);
                            }
                            rdev::Key::MetaLeft | rdev::Key::MetaRight => {
                                RDEV_TRACKED_META.store(true, Ordering::Relaxed);
                            }
                            _ => {}
                        }

                        let active_mods = get_currently_pressed_modifiers();
                        let vk = rdev_key_to_vk(key);

                        if let Some(code) = vk {
                            if let Ok(mut lock) = REBIND_LISTENING_TARGET.lock() {
                                if let Some(target_var) = lock.take() {
                                    if is_modifier_vk(code) {
                                        *lock = Some(target_var);
                                        return;
                                    }
                                    if code == 0x1B && active_mods == 0 {
                                        trigger_repaint();
                                        return;
                                    }
                                    let full_binding = (code & VK_KEY_MASK) | (active_mods & MOD_MASK);
                                    apply_rebound_key(&target_var, full_binding);
                                    trigger_repaint();
                                    return;
                                }
                            }
                        }

                        if matches!(key, rdev::Key::Escape) {
                            crate::eyetracking::ESCAPE_PRESSED.store(true, Ordering::Relaxed);
                        }
                        if matches!(key, rdev::Key::Return) {
                            crate::eyetracking::ENTER_PRESSED.store(true, Ordering::Relaxed);
                        }

                        if let Some(code) = vk {
                            let global_evt = if matches_binding(key_switch, code, active_mods) {
                                Some(SwitchKeyEvent::ToggleSwitch)
                            } else if matches_binding(key_dictation, code, active_mods) || (code == 0x13 && active_mods == 0) {
                                Some(SwitchKeyEvent::ToggleDictation)
                            } else if matches_binding(key_voice, code, active_mods) {
                                Some(SwitchKeyEvent::ToggleVoice)
                            } else if matches_binding(key_clear, code, active_mods) {
                                Some(SwitchKeyEvent::Clear)
                            } else if code == 0x1B && active_mods == 0 && (is_dictation || is_switch_on) {
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
                        }

                        let is_eye_active = crate::eyetracking::is_eye_tracking_enabled()
                            && !crate::eyetracking::is_calibration_active();

                        if is_eye_active && !is_dictation && !is_typing && matches!(key, rdev::Key::Return) && active_mods == 0 {
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

                        if active_mods == 0 {
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
                        }

                        let is_shift = (active_mods & MOD_SHIFT) != 0;
                        if active_mods == 0 || (active_mods == MOD_SHIFT && matches!(key, rdev::Key::Tab)) {
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
                                rdev::Key::Tab => {
                                    let evt = if is_shift { SwitchKeyEvent::Prev } else { SwitchKeyEvent::Next };
                                    if let Ok(mut q) = SWITCH_KEY_QUEUE.lock() {
                                        q.push(evt);
                                    }
                                    trigger_repaint();
                                }
                                rdev::Key::RightArrow | rdev::Key::DownArrow | rdev::Key::PageDown => {
                                    if let Ok(mut q) = SWITCH_KEY_QUEUE.lock() {
                                        q.push(SwitchKeyEvent::Next);
                                    }
                                    trigger_repaint();
                                }
                                rdev::Key::LeftArrow | rdev::Key::UpArrow | rdev::Key::PageUp => {
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
                    }
                    rdev::EventType::KeyRelease(key) => {
                        match key {
                            rdev::Key::ControlLeft | rdev::Key::ControlRight => {
                                RDEV_TRACKED_CTRL.store(false, Ordering::Relaxed);
                            }
                            rdev::Key::ShiftLeft | rdev::Key::ShiftRight => {
                                RDEV_TRACKED_SHIFT.store(false, Ordering::Relaxed);
                            }
                            rdev::Key::Alt | rdev::Key::AltGr => {
                                RDEV_TRACKED_ALT.store(false, Ordering::Relaxed);
                            }
                            rdev::Key::MetaLeft | rdev::Key::MetaRight => {
                                RDEV_TRACKED_META.store(false, Ordering::Relaxed);
                            }
                            _ => {}
                        }
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
                    rdev::EventType::Wheel { .. } => {
                        if is_switch_on {
                            crate::window_util::clear_screen_numbered_badges();
                            trigger_repaint();
                        }
                    }
                    rdev::EventType::ButtonPress(btn) => {
                        match btn {
                            rdev::Button::Middle => {
                                inject_switch_event(SwitchKeyEvent::StopSpeech);
                            }
                            rdev::Button::Unknown(4) => {
                                inject_switch_event(SwitchKeyEvent::ToggleDictation);
                            }
                            rdev::Button::Unknown(5) => {
                                inject_switch_event(SwitchKeyEvent::ToggleSwitch);
                            }
                            _ => {}
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
    stop_companion_switch_server();
    stop_bci_lsl_receiver();

    HOOK_THREAD_ACTIVE.store(false, Ordering::SeqCst);
}