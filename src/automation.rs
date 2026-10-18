use std::sync::Mutex;
use std::sync::atomic::{AtomicBool, AtomicU64, AtomicUsize, Ordering};
use std::time::{Duration, Instant};

pub use crate::scheduler::*;
pub use crate::autostart::*;

use crate::fs_util::{
    create_temp_note_file, find_executable_in_path, find_file_in_workspace,
    open_path_with_default_app,
};
use crate::pattern::{clean_words, extract_target_propositions, matches_pattern};
use crate::types::{AgentAction, AgentCommand, QuickSuggestionItem, ScheduledTaskItem};
use crate::window_util::*;

#[cfg(windows)]
use std::os::windows::process::CommandExt;
#[cfg(windows)]
use windows::core::{w, Interface};
#[cfg(windows)]
use windows::Win32::Foundation::{HWND, LPARAM, RECT, WPARAM};
#[cfg(windows)]
use windows::Win32::System::Com::{CoCreateInstance, CoInitializeEx, CLSCTX_INPROC_SERVER, COINIT_MULTITHREADED};
#[cfg(windows)]
use windows::Win32::UI::Accessibility::{
    CUIAutomation, IUIAutomation, IUIAutomation2, IUIAutomationElement, IUIAutomationElement2, IUIAutomationInvokePattern,
    IUIAutomationScrollItemPattern, IUIAutomationScrollPattern, IUIAutomationTextPattern, IUIAutomationValuePattern,
    TreeScope_Descendants, UIA_ButtonControlTypeId, UIA_DocumentControlTypeId, UIA_EditControlTypeId,
    UIA_TabControlTypeId, UIA_TabItemControlTypeId,
    UIA_HyperlinkControlTypeId, UIA_InvokePatternId, UIA_ScrollItemPatternId, UIA_ScrollPatternId,
    UIA_TextPatternId, UIA_ValuePatternId,
};
#[cfg(windows)]
use windows::Win32::UI::Input::KeyboardAndMouse::{
    mouse_event, SendInput, INPUT, INPUT_0, INPUT_KEYBOARD, INPUT_MOUSE, KEYBDINPUT,
    SetFocus,
    KEYBD_EVENT_FLAGS, KEYEVENTF_KEYUP, KEYEVENTF_UNICODE, MOUSEEVENTF_LEFTDOWN, MOUSEEVENTF_WHEEL,
    MOUSEEVENTF_LEFTUP, MOUSEEVENTF_RIGHTDOWN, MOUSEEVENTF_RIGHTUP, MOUSEINPUT, VIRTUAL_KEY,
};
#[cfg(windows)]
use windows::Win32::UI::WindowsAndMessaging::{
    AllowSetForegroundWindow,
    FindWindowW, GetClassNameW, GetForegroundWindow, GetWindowRect, GetWindowThreadProcessId, IsIconic,
    GetGUIThreadInfo, GUITHREADINFO,
    PostMessageW, SendMessageW, SetCursorPos, SetForegroundWindow, ShowWindow,
    SW_MAXIMIZE, SW_MINIMIZE, SW_RESTORE, WM_CLOSE,
};

#[cfg(windows)]
const CREATE_NEW_CONSOLE: u32 = 0x00000010;
#[cfg(windows)]
const CREATE_NO_WINDOW: u32 = 0x08000000;

#[derive(Clone, Debug)]
pub struct NumberedElementTarget {
    pub index: usize,
    pub click_x: i32,
    pub click_y: i32,
    pub rect: [i32; 4],
    pub name: String,
}

#[cfg(windows)]
pub fn collect_numbered_targets_for_active_window() -> Vec<NumberedElementTarget> {
    let user_windows = list_user_windows();
    let Some(active_hwnd) = get_active_or_best_window(&user_windows) else {
        return Vec::new();
    };
    let elements = list_interactive_elements(active_hwnd);
    elements
        .into_iter()
        .filter(|e| !e.is_offscreen && (e.is_button || e.is_link || e.is_tab || e.is_edit_or_textarea || e.is_tabindex))
        .take(30)
        .enumerate()
        .map(|(i, e)| NumberedElementTarget {
            index: i + 1,
            click_x: e.click_x,
            click_y: e.click_y,
            rect: [e.rect.left, e.rect.top, e.rect.right, e.rect.bottom],
            name: e.name,
        })
        .collect()
}

#[cfg(not(windows))]
pub fn collect_numbered_targets_for_active_window() -> Vec<NumberedElementTarget> {
    Vec::new()
}

#[cfg(windows)]
static LAST_TXT_HWND: Mutex<Option<isize>> = Mutex::new(None);
pub static LAST_EXTERNAL_HWND: Mutex<Option<isize>> = Mutex::new(None);
pub static LAST_FOCUSED_CHILD_HWND: Mutex<Option<isize>> = Mutex::new(None);
pub static LAST_FOCUSED_RECT: Mutex<Option<[i32; 4]>> = Mutex::new(None);

pub static CURRENT_REQUEST_ID: AtomicU64 = AtomicU64::new(0);
pub static IS_EMERGENCY_STOPPED: AtomicBool = AtomicBool::new(false);
pub static AGENT_BUSY: AtomicBool = AtomicBool::new(false);
pub static TARGET_REASONING_PASSES: AtomicUsize = AtomicUsize::new(5);
pub fn is_request_cancelled(request_id: u64) -> bool {
    CURRENT_REQUEST_ID.load(Ordering::SeqCst) != request_id || IS_EMERGENCY_STOPPED.load(Ordering::SeqCst)
}

#[cfg(windows)]
static LAST_FOCUS_UPDATE: Mutex<Option<Instant>> = Mutex::new(None);

pub static LAST_SCREEN_SUMMARY: Mutex<Option<String>> = Mutex::new(None);
pub static IS_IMMERSION_ACTIVE: AtomicBool = AtomicBool::new(false);
pub static IS_DICTATION_ACTIVE: AtomicBool = AtomicBool::new(false);
pub static IMMERSION_SCREEN_HISTORY: Mutex<Vec<String>> = Mutex::new(Vec::new());

#[cfg(windows)]
pub fn update_last_external_focus() {
    // Ne pas surcharger le thread UI egui si l'agent est actif ou en dictée
    if AGENT_BUSY.load(Ordering::Relaxed) || IS_DICTATION_ACTIVE.load(Ordering::SeqCst) {
        return;
    }

    // Limitation stricte de fréquence à 5 Hz max (200 ms)
    {
        let mut last_update = match LAST_FOCUS_UPDATE.lock() {
            Ok(g) => g,
            Err(p) => p.into_inner(),
        };
        if let Some(prev) = *last_update {
            if prev.elapsed() < Duration::from_millis(200) {
                return;
            }
        }
        *last_update = Some(Instant::now());
    }

    let fg = unsafe { GetForegroundWindow() };
    if fg.0.is_null() || is_window_hung(fg) {
        return;
    }
    let current_pid = std::process::id();
    let mut fg_pid = 0u32;
    let fg_thread_id = unsafe { GetWindowThreadProcessId(fg, Some(&mut fg_pid)) };
    if fg_pid != current_pid {
        if let Ok(mut lock) = LAST_EXTERNAL_HWND.lock() {
            *lock = Some(fg.0 as isize);
        }
        let mut gti = GUITHREADINFO::default();
        gti.cbSize = std::mem::size_of::<GUITHREADINFO>() as u32;
        if unsafe { GetGUIThreadInfo(fg_thread_id, &mut gti).is_ok() } && !gti.hwndFocus.0.is_null() {
            if let Ok(mut clock) = LAST_FOCUSED_CHILD_HWND.lock() {
                *clock = Some(gti.hwndFocus.0 as isize);
            }
            let mut r = RECT::default();
            if unsafe { GetWindowRect(gti.hwndFocus, &mut r).is_ok() } && (r.right - r.left) > 10 && (r.bottom - r.top) > 10 {
                if let Ok(mut rlock) = LAST_FOCUSED_RECT.lock() {
                    *rlock = Some([r.left, r.top, r.right, r.bottom]);
                }
            }
        }
    }
}

#[cfg(not(windows))]
pub fn update_last_external_focus() {}

#[cfg(windows)]
pub fn restore_external_focus_target(target: Option<HWND>) -> bool {
    extern "system" {
        fn GetCurrentThreadId() -> u32;
        fn AttachThreadInput(idattach: u32, idattachto: u32, fattach: i32) -> i32;
    }

    let target_hwnd = target.or_else(|| {
        LAST_EXTERNAL_HWND.lock().ok().and_then(|g| *g).map(|raw| HWND(raw as _))
    });

    if let Some(hwnd) = target_hwnd {
        if !hwnd.0.is_null() && !is_window_hung(hwnd) {
            let (_pid, exe_name) = get_window_process_info(hwnd);
            let is_browser = is_browser_executable(&exe_name);

            if is_browser {
                focus_browser_address_bar(hwnd);
                return true;
            }

            // Contournement universel de l'inhibition de focus Windows via simulation de la touche Alt
            const VK_MENU: VIRTUAL_KEY = VIRTUAL_KEY(0x12);
            send_key_event(VK_MENU, KEYBD_EVENT_FLAGS(0));
            send_key_event(VK_MENU, KEYEVENTF_KEYUP);

            force_foreground_window(hwnd);
            std::thread::sleep(Duration::from_millis(40));

            // 2. Si c'est un contrôle Win32 enfant (ex: canevas Java Swing IntelliJ, Notepad, champs de dialogue)
            if let Ok(clock) = LAST_FOCUSED_CHILD_HWND.lock() {
                if let Some(child_raw) = *clock {
                    let child_hwnd = HWND(child_raw as _);
                    if !child_hwnd.0.is_null() {
                        let current_tid = unsafe { GetCurrentThreadId() };
                        let mut target_pid = 0u32;
                        let target_tid = unsafe { GetWindowThreadProcessId(hwnd, Some(&mut target_pid)) };
                        if current_tid != target_tid && target_tid != 0 {
                            unsafe {
                                let _ = AttachThreadInput(current_tid, target_tid, 1);
                                let _ = SetFocus(child_hwnd);
                                let _ = AttachThreadInput(current_tid, target_tid, 0);
                            }
                        } else {
                            unsafe { let _ = SetFocus(child_hwnd); }
                        }
                    }
                }
            }
            // Note : aucun clic de souris (click_element) n'est simulé ici afin de préserver
            // intégralement la sélection de texte et le curseur actif dans l'éditeur.
            return true;
        }
    }
    false
}

#[cfg(windows)]
pub fn restore_external_focus() -> bool {
    restore_external_focus_target(None)
}

#[cfg(windows)]
pub fn release_modifier_keys() {
    const VK_CONTROL: VIRTUAL_KEY = VIRTUAL_KEY(0x11);
    const VK_MENU: VIRTUAL_KEY = VIRTUAL_KEY(0x12);
    const VK_SHIFT: VIRTUAL_KEY = VIRTUAL_KEY(0x10);
    const VK_LWIN: VIRTUAL_KEY = VIRTUAL_KEY(0x5B);
    const VK_RWIN: VIRTUAL_KEY = VIRTUAL_KEY(0x5C);

    send_key_event(VK_CONTROL, KEYEVENTF_KEYUP);
    send_key_event(VK_MENU, KEYEVENTF_KEYUP);
    send_key_event(VK_SHIFT, KEYEVENTF_KEYUP);
    send_key_event(VK_LWIN, KEYEVENTF_KEYUP);
    send_key_event(VK_RWIN, KEYEVENTF_KEYUP);
}

#[cfg(not(windows))]
pub fn restore_external_focus_target(_target: Option<()>) -> bool {
    false
}

#[cfg(not(windows))]
pub fn restore_external_focus() -> bool {
    false
}

#[cfg(target_os = "macos")]
mod clipboard {
    use std::io::Write;
    pub fn set_text(text: &str) -> bool {
        let mut child = match std::process::Command::new("pbcopy")
            .stdin(std::process::Stdio::piped())
            .spawn() {
            Ok(c) => c,
            Err(_) => return false,
        };
        if let Some(mut stdin) = child.stdin.take() {
            let _ = stdin.write_all(text.as_bytes());
        }
        child.wait().map(|s| s.success()).unwrap_or(false)
    }
    pub fn get_text() -> Option<String> {
        let output = std::process::Command::new("pbpaste").output().ok()?;
        Some(String::from_utf8_lossy(&output.stdout).to_string())
    }
}

#[cfg(all(not(windows), not(target_os = "macos")))]
mod clipboard {
    use std::io::Write;

    pub fn set_text(text: &str) -> bool {
        let mut child = match std::process::Command::new("xclip")
            .args(["-selection", "clipboard"])
            .stdin(std::process::Stdio::piped())
            .spawn()
        {
            Ok(c) => c,
            Err(_) => return false,
        };
        if let Some(mut stdin) = child.stdin.take() {
            let _ = stdin.write_all(text.as_bytes());
        }
        child.wait().map(|s| s.success()).unwrap_or(false)
    }

    pub fn get_text() -> Option<String> {
        let output = std::process::Command::new("xclip")
            .args(["-selection", "clipboard", "-o"])
            .output()
            .ok()?;
        Some(String::from_utf8_lossy(&output.stdout).to_string())
    }
}

pub fn get_clipboard_text() -> Option<String> {
    clipboard::get_text()
}

pub fn set_clipboard_text(text: &str) -> bool {
    clipboard::set_text(text)
}

#[cfg(not(windows))]
pub fn release_modifier_keys() {}

#[cfg(windows)]
mod clipboard {
    use std::ffi::c_void;

    extern "system" {
        fn OpenClipboard(hwnd: *mut c_void) -> i32;
        fn CloseClipboard() -> i32;
        fn EmptyClipboard() -> i32;
        fn SetClipboardData(uformat: u32, hmem: *mut c_void) -> *mut c_void;
        fn GetClipboardData(uformat: u32) -> *mut c_void;
        fn IsClipboardFormatAvailable(uformat: u32) -> i32;
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

    pub fn get_text() -> Option<String> {
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
                return None;
            }

            if IsClipboardFormatAvailable(CF_UNICODETEXT) == 0 {
                let _ = CloseClipboard();
                return None;
            }

            let h_mem = GetClipboardData(CF_UNICODETEXT);
            if h_mem.is_null() {
                let _ = CloseClipboard();
                return None;
            }

            let p_data = GlobalLock(h_mem) as *const u16;
            if p_data.is_null() {
                let _ = CloseClipboard();
                return None;
            }

            let mut len = 0;
            while *p_data.add(len) != 0 {
                len += 1;
            }
            let slice = std::slice::from_raw_parts(p_data, len);
            let s = String::from_utf16_lossy(slice);
            let _ = GlobalUnlock(h_mem);
            let _ = CloseClipboard();
            Some(s)
        }
    }
}

#[cfg(windows)]
fn extract_terminal_uia_text(hwnd: HWND) -> Option<String> {
    unsafe {
        let _ = CoInitializeEx(None, COINIT_MULTITHREADED);
        let uia: Result<IUIAutomation, _> = CoCreateInstance(&CUIAutomation, None, CLSCTX_INPROC_SERVER);
        let Ok(uia) = uia else {
            return None;
        };
        let Ok(window_element) = uia.ElementFromHandle(hwnd) else {
            return None;
        };

        // 1. Tenter le TextPattern directement sur la fenêtre
        if let Ok(pattern_unk) = window_element.GetCurrentPattern(UIA_TextPatternId) {
            if let Ok(text_pattern) = pattern_unk.cast::<IUIAutomationTextPattern>() {
                if let Ok(range) = text_pattern.DocumentRange() {
                    if let Ok(bstr) = range.GetText(15000) {
                        let s = bstr.to_string();
                        let trimmed = s.trim();
                        if !trimmed.is_empty() {
                            return Some(trimmed.to_string());
                        }
                    }
                }
            }
        }

        // 2. Parcourir les descendants pour trouver le composant console (ex: TermControl dans Windows Terminal)
        let Ok(cond) = uia.CreateTrueCondition() else {
            return None;
        };
        let Ok(elements) = window_element.FindAll(TreeScope_Descendants, &cond) else {
            return None;
        };

        let count = elements.Length().unwrap_or(0).min(60);
        for i in 0..count {
            if let Ok(item) = elements.GetElement(i) {
                let class_name = item.CurrentClassName().map(|b| b.to_string()).unwrap_or_default().to_lowercase();
                let loc_type = item.CurrentLocalizedControlType().map(|b| b.to_string()).unwrap_or_default().to_lowercase();
                let is_term = class_name.contains("termcontrol")
                    || class_name.contains("console")
                    || loc_type.contains("terminal")
                    || loc_type.contains("document")
                    || loc_type.contains("edit");

                if is_term {
                    if let Ok(pattern_unk) = item.GetCurrentPattern(UIA_TextPatternId) {
                        if let Ok(text_pattern) = pattern_unk.cast::<IUIAutomationTextPattern>() {
                            if let Ok(range) = text_pattern.DocumentRange() {
                                if let Ok(bstr) = range.GetText(15000) {
                                    let s = bstr.to_string();
                                    let trimmed = s.trim();
                                    if !trimmed.is_empty() {
                                        return Some(trimmed.to_string());
                                    }
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
pub fn read_terminal_window_content(hwnd: HWND, _win_title: &str) -> Option<String> {
    if is_putty_window(hwnd) {
        if let Some(text) = read_putty_terminal_content(hwnd) {
            return Some(text);
        }
    }
    extract_terminal_uia_text(hwnd)
}

#[cfg(not(windows))]
pub fn read_terminal_window_content(_hwnd: (), _win_title: &str) -> Option<String> {
    None
}

#[cfg(windows)]
pub fn read_putty_terminal_content(hwnd: HWND) -> Option<String> {
    const WM_SYSCOMMAND: u32 = 0x0112;
    const IDM_COPYALL: usize = 0x0170;

    let old_clip = clipboard::get_text();
    unsafe {
        SendMessageW(hwnd, WM_SYSCOMMAND, WPARAM(IDM_COPYALL), LPARAM(0));
    }
    std::thread::sleep(Duration::from_millis(60));

    let new_clip = clipboard::get_text();
    if let Some(text) = new_clip {
        let trimmed = text.trim();
        if !trimmed.is_empty() && (old_clip.as_deref() != Some(&text) || !trimmed.is_empty()) {
            return Some(text);
        }
    }
    None
}

#[cfg(not(windows))]
pub fn read_putty_terminal_content(_hwnd: ()) -> Option<String> {
    None
}

#[cfg(windows)]
pub fn force_foreground_window(hwnd: HWND) -> bool {
    if is_window_hung(hwnd) {
        eprintln!("[Windows] Fenêtre {:?} figée / ne répond pas (sablier). Focus évité pour prévenir un gel.", hwnd.0);
        return false;
    }
    extern "system" {
        fn GetCurrentThreadId() -> u32;
        fn AttachThreadInput(idattach: u32, idattachto: u32, fattach: i32) -> i32;
        fn BringWindowToTop(hwnd: HWND) -> i32;
    }
    unsafe {
        if IsIconic(hwnd).as_bool() {
            let _ = ShowWindow(hwnd, SW_RESTORE);
        }
        let fg_hwnd = GetForegroundWindow();
        if fg_hwnd == hwnd {
            return true;
        }
        let current_thread_id = GetCurrentThreadId();
        let mut target_pid = 0u32;
        let target_thread_id = GetWindowThreadProcessId(hwnd, Some(&mut target_pid));
        let fg_thread_id = if !fg_hwnd.0.is_null() {
            GetWindowThreadProcessId(fg_hwnd, None)
        } else {
            0
        };

        let (_pid, exe_name) = get_window_process_info(hwnd);
        let is_browser = is_browser_executable(&exe_name);

        const ASFW_ANY: u32 = 0xFFFFFFFF;
        let _ = AllowSetForegroundWindow(ASFW_ANY);

        if current_thread_id != target_thread_id && target_thread_id != 0 {
            let _ = AttachThreadInput(current_thread_id, target_thread_id, 1);
            if fg_thread_id != 0 && fg_thread_id != target_thread_id {
                let _ = AttachThreadInput(fg_thread_id, target_thread_id, 1);
            }
            let _ = BringWindowToTop(hwnd);
            let _ = SetForegroundWindow(hwnd);

            // Pour les navigateurs, ne pas forcer le focus sur le widget de rendu interne de la page web
            if !is_browser {
                let mut gti = GUITHREADINFO::default();
                gti.cbSize = std::mem::size_of::<GUITHREADINFO>() as u32;
                if GetGUIThreadInfo(target_thread_id, &mut gti).is_ok() && !gti.hwndFocus.0.is_null() {
                    let _ = SetFocus(gti.hwndFocus);
                }
            }

            if fg_thread_id != 0 && fg_thread_id != target_thread_id {
                let _ = AttachThreadInput(fg_thread_id, target_thread_id, 0);
            }
            let _ = AttachThreadInput(current_thread_id, target_thread_id, 0);
        } else {
            let _ = BringWindowToTop(hwnd);
            let _ = SetForegroundWindow(hwnd);
        }
    }
    true
}

#[cfg(windows)]
fn send_key_event(vk: VIRTUAL_KEY, flags: KEYBD_EVENT_FLAGS) {
    let input = [INPUT {
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
    }];
    unsafe {
        SendInput(&input, std::mem::size_of::<INPUT>() as i32);
    }
}

#[cfg(windows)]
fn send_paste() {
    const VK_CONTROL: VIRTUAL_KEY = VIRTUAL_KEY(0x11);
    const VK_V: VIRTUAL_KEY = VIRTUAL_KEY(0x56);

    send_key_event(VK_CONTROL, KEYBD_EVENT_FLAGS(0));
    std::thread::sleep(Duration::from_millis(20));
    send_key_event(VK_V, KEYBD_EVENT_FLAGS(0));
    std::thread::sleep(Duration::from_millis(35));
    send_key_event(VK_V, KEYEVENTF_KEYUP);
    std::thread::sleep(Duration::from_millis(20));
    send_key_event(VK_CONTROL, KEYEVENTF_KEYUP);
}

#[cfg(windows)]
fn send_char_unicode(c: char) {
    let mut utf16_buf = [0u16; 2];
    let encoded = c.encode_utf16(&mut utf16_buf);
    for &mut u in encoded {
        let down = INPUT {
            r#type: INPUT_KEYBOARD,
            Anonymous: INPUT_0 {
                ki: KEYBDINPUT {
                    wVk: VIRTUAL_KEY(0),
                    wScan: u,
                    dwFlags: KEYEVENTF_UNICODE,
                    time: 0,
                    dwExtraInfo: 0,
                },
            },
        };
        let up = INPUT {
            r#type: INPUT_KEYBOARD,
            Anonymous: INPUT_0 {
                ki: KEYBDINPUT {
                    wVk: VIRTUAL_KEY(0),
                    wScan: u,
                    dwFlags: KEYBD_EVENT_FLAGS(KEYEVENTF_UNICODE.0 | KEYEVENTF_KEYUP.0),
                    time: 0,
                    dwExtraInfo: 0,
                },
            },
        };
        unsafe {
            SendInput(&[down, up], std::mem::size_of::<INPUT>() as i32);
        }
    }
}

#[cfg(windows)]
fn send_terminal_input(hwnd: HWND, text: &str, auto_enter: bool) {
    const VK_RETURN: VIRTUAL_KEY = VIRTUAL_KEY(0x0D);
    const VK_SHIFT: VIRTUAL_KEY = VIRTUAL_KEY(0x10);
    const VK_CONTROL: VIRTUAL_KEY = VIRTUAL_KEY(0x11);
    const VK_INSERT: VIRTUAL_KEY = VIRTUAL_KEY(0x2D);
    const VK_V: VIRTUAL_KEY = VIRTUAL_KEY(0x56);

    let trimmed = text.trim_end_matches(['\r', '\n']);
    let has_trailing_newline = text.ends_with('\n') || text.ends_with('\r');
    let should_press_enter = auto_enter || has_trailing_newline;

    // 1. Saisie directe de caractères Unicode : infaillible sur tous les terminaux
    // sans dépendre du presse-papiers ni des raccourcis clavier propres à une application.
    if trimmed.chars().count() <= 1200 {
        for c in trimmed.chars() {
            if c == '\n' || c == '\r' {
                send_hotkey(&[], VK_RETURN);
                std::thread::sleep(Duration::from_millis(20));
            } else {
                send_char_unicode(c);
                std::thread::sleep(Duration::from_millis(3));
            }
        }
    } else {
        // 2. Pour les très gros textes : collage adapté au type de terminal
        clipboard::set_text(trimmed);
        std::thread::sleep(Duration::from_millis(40));

        if is_windows_terminal(hwnd) {
            send_hotkey(&[VK_CONTROL, VK_SHIFT], VK_V);
        } else if is_putty_window(hwnd) {
            send_hotkey(&[VK_SHIFT], VK_INSERT);
        } else {
            // ConHost / CMD / PowerShell / MinTTY
            send_hotkey(&[VK_SHIFT], VK_INSERT);
        }
        std::thread::sleep(Duration::from_millis(50));
    }

    if should_press_enter {
        std::thread::sleep(Duration::from_millis(40));
        send_hotkey(&[], VK_RETURN);
    }
}

#[cfg(windows)]
pub fn send_hotkey(modifiers: &[VIRTUAL_KEY], key: VIRTUAL_KEY) {
    for &m in modifiers {
        send_key_event(m, KEYBD_EVENT_FLAGS(0));
        std::thread::sleep(Duration::from_millis(15));
    }

    if key.0 != 0 {
        send_key_event(key, KEYBD_EVENT_FLAGS(0));
        std::thread::sleep(Duration::from_millis(35));
        send_key_event(key, KEYEVENTF_KEYUP);
        std::thread::sleep(Duration::from_millis(15));
    }

    for &m in modifiers.iter().rev() {
        send_key_event(m, KEYEVENTF_KEYUP);
        std::thread::sleep(Duration::from_millis(15));
    }
}

#[cfg(windows)]
pub fn try_send_key_combination(combo: &str) -> bool {
    let clean = combo.trim().to_lowercase();
    if clean.is_empty() {
        return false;
    }

    let parts: Vec<&str> = clean
        .split(|c| c == '+' || c == '-')
        .map(str::trim)
        .filter(|s| !s.is_empty())
        .collect();

    if parts.is_empty() {
        return false;
    }

    const VK_CONTROL: VIRTUAL_KEY = VIRTUAL_KEY(0x11);
    const VK_SHIFT: VIRTUAL_KEY = VIRTUAL_KEY(0x10);
    const VK_MENU: VIRTUAL_KEY = VIRTUAL_KEY(0x12);
    const VK_LWIN: VIRTUAL_KEY = VIRTUAL_KEY(0x5B);

    let mut modifiers = Vec::new();
    let mut key: Option<VIRTUAL_KEY> = None;

    for part in parts {
        match part {
            "ctrl" | "control" => modifiers.push(VK_CONTROL),
            "shift" | "sift" | "shif" | "maj" => modifiers.push(VK_SHIFT),
            "alt" | "menu" => modifiers.push(VK_MENU),
            "win" | "windows" | "super" | "meta" => modifiers.push(VK_LWIN),
            "enter" | "return" | "entree" | "entrée" => key = Some(VIRTUAL_KEY(0x0D)),
            "esc" | "escape" | "echap" | "échap" => key = Some(VIRTUAL_KEY(0x1B)),
            "tab" => key = Some(VIRTUAL_KEY(0x09)),
            "space" | "espace" => key = Some(VIRTUAL_KEY(0x20)),
            "backspace" | "back" | "retour" => key = Some(VIRTUAL_KEY(0x08)),
            "del" | "delete" | "suppr" => key = Some(VIRTUAL_KEY(0x2E)),
            "ins" | "insert" => key = Some(VIRTUAL_KEY(0x2D)),
            "home" | "debut" | "début" => key = Some(VIRTUAL_KEY(0x24)),
            "end" | "fin" => key = Some(VIRTUAL_KEY(0x23)),
            "pageup" | "prior" | "pgup" => key = Some(VIRTUAL_KEY(0x21)),
            "pagedown" | "next" | "pgdown" => key = Some(VIRTUAL_KEY(0x22)),
            "left" | "gauche" => key = Some(VIRTUAL_KEY(0x25)),
            "up" | "haut" => key = Some(VIRTUAL_KEY(0x26)),
            "right" | "droite" => key = Some(VIRTUAL_KEY(0x27)),
            "down" | "bas" => key = Some(VIRTUAL_KEY(0x28)),
            single if single.len() == 1 => {
                let c = single.chars().next().unwrap();
                if c.is_ascii_alphabetic() {
                    key = Some(VIRTUAL_KEY(0x41 + (c.to_ascii_uppercase() as u16 - b'A' as u16)));
                } else if c.is_ascii_digit() {
                    key = Some(VIRTUAL_KEY(0x30 + (c as u16 - b'0' as u16)));
                } else {
                    return false;
                }
            }
            f_key if f_key.starts_with('f') && f_key.len() <= 3 => {
                if let Ok(num) = f_key[1..].parse::<u16>() {
                    if (1..=24).contains(&num) {
                        key = Some(VIRTUAL_KEY(0x70 + num - 1));
                    } else {
                        return false;
                    }
                } else {
                    return false;
                }
            }
            _ => return false,
        }
    }

    if let Some(k) = key {
        send_hotkey(&modifiers, k);
        true
    } else {
        false
    }
}

pub fn urlencoding_simple(query: &str) -> String {
    query
        .chars()
        .map(|c| if c.is_alphanumeric() { c.to_string() } else { format!("%{:02X}", c as u32) })
        .collect()
}

pub fn truncate_with_notice(text: &str, max_chars: usize) -> String {
    if text.chars().count() > max_chars {
        let truncated: String = text.chars().take(max_chars).collect();
        format!("{truncated}\n[...sortie tronquée...]")
    } else {
        text.to_string()
    }
}

pub fn is_address_bar_target(target: &str) -> bool {
    let t = target.trim().to_lowercase();
    let clean = t
        .strip_prefix("[navigateur]").unwrap_or(&t)
        .strip_prefix("navigateur:").unwrap_or(&t)
        .strip_prefix("[browser]").unwrap_or(&t)
        .strip_prefix("browser:").unwrap_or(&t)
        .trim();
    clean == "url"
        || clean.contains("url")
        || clean.contains("adresse")
        || clean.contains("address")
        || clean.contains("omnibox")
        || clean.contains("omnibar")
        || clean.contains("omniboxview")
        || clean == "navigateur"
        || clean == "browser"
        || clean == "chrome"
        || clean == "edge"
        || clean == "firefox"
        || clean == "brave"
}

pub fn is_browser_chrome_target(target: &str) -> bool {
    let t = target.trim().to_lowercase();
    t.starts_with("[navigateur]")
        || t.starts_with("navigateur:")
        || t.starts_with("[browser]")
        || t.starts_with("browser:")
        || is_address_bar_target(target)
}

pub fn is_generic_textarea_target(target: &str) -> bool {
    let t = target.trim().to_lowercase().replace('’', "'");
    t.is_empty()
        || t == "zone de texte"
        || t == "la zone de texte"
        || t == "une zone de texte"
        || t == "champ"
        || t == "le champ"
        || t == "un champ"
        || t == "textarea"
        || t == "input"
        || t == "champ de texte"
        || t == "zone de saisie"
        || t == "texte"
}

pub fn format_url_for_navigation(raw_url: &str) -> String {
    let trimmed = raw_url.trim();
    if trimmed.is_empty() {
        return "https://www.google.com".to_string();
    }

    // 1. Détection de tous les schémas complets comportant :// (chrome://, edge://, file://, ftp://, http://, etc.)
    if let Some(pos) = trimmed.find("://") {
        let scheme = &trimmed[..pos];
        if !scheme.is_empty()
            && scheme
                .chars()
                .all(|c| c.is_ascii_alphanumeric() || c == '+' || c == '-' || c == '.')
        {
            return trimmed.to_string();
        }
    }

    // 2. Détection des pseudo-protocoles sans double barre oblique (about:, data:, view-source:, file:, blob:, etc.)
    const PSEUDO_SCHEMES: &[&str] = &[
        "about:",
        "data:",
        "view-source:",
        "file:",
        "blob:",
        "javascript:",
        "mailto:",
        "tel:",
    ];

    let lower = trimmed.to_lowercase();
    for &pseudo in PSEUDO_SCHEMES {
        if lower.starts_with(pseudo) {
            return trimmed.to_string();
        }
    }

    // 3. Support de localhost (ex: localhost, localhost:8080, localhost/path)
    if lower == "localhost" || lower.starts_with("localhost:") || lower.starts_with("localhost/") {
        return format!("http://{trimmed}");
    }

    // 4. Domaines et adresses valides sans protocole explicite -> préfixage https://
    if !trimmed.contains(' ') && trimmed.contains('.') {
        format!("https://{trimmed}")
    } else {
        // 5. Repli : terme de recherche vers Google
        format!("https://www.google.com/search?q={}", urlencoding_simple(trimmed))
    }
}

#[cfg(windows)]
pub fn focus_browser_address_bar(hwnd: HWND) {
    force_foreground_window(hwnd);
    unsafe {
        let _ = ShowWindow(hwnd, SW_MAXIMIZE);
    }
    std::thread::sleep(Duration::from_millis(50));

    release_modifier_keys();

    const VK_CONTROL: VIRTUAL_KEY = VIRTUAL_KEY(0x11);
    const VK_L: VIRTUAL_KEY = VIRTUAL_KEY(0x4C);
    const VK_MENU: VIRTUAL_KEY = VIRTUAL_KEY(0x12);
    const VK_D: VIRTUAL_KEY = VIRTUAL_KEY(0x44);

    // 1. Sélection universelle directe de l'omnibar (Alt+D puis Ctrl+L) sans clic souris parasite
    send_hotkey(&[VK_MENU], VK_D);
    std::thread::sleep(Duration::from_millis(40));
    send_hotkey(&[VK_CONTROL], VK_L);
    std::thread::sleep(Duration::from_millis(50));

    // 2. Renforcement du focus UIA sur l'omnibar si accessible
    let elements = list_interactive_elements(hwnd);
    let address_elem = elements.iter().find(|e| {
        e.is_browser_chrome
            && (e.is_edit_or_textarea || e.is_input || e.class_name.contains("omnibox") || e.automation_id.to_lowercase().contains("omnibox"))
    });

    if let Some(elem) = address_elem {
        unsafe { let _ = elem.element.SetFocus(); }
    }
}

#[cfg(windows)]
fn navigate_browser_address_bar(hwnd: HWND, url: &str) {
    let formatted_url = format_url_for_navigation(url);
    focus_browser_address_bar(hwnd);

    const VK_RETURN: VIRTUAL_KEY = VIRTUAL_KEY(0x0D);

    // 3. Écriture de l'URL via le presse-papiers
    clipboard::set_text(&formatted_url);
    std::thread::sleep(Duration::from_millis(60));
    send_paste();
    std::thread::sleep(Duration::from_millis(180));

    // 4. Validation par la touche Entrée avec un délai de maintien suffisant
    send_hotkey(&[], VK_RETURN);
    std::thread::sleep(Duration::from_millis(100));
    println!("[Actions] Navigation vers {} effectuée avec succès via barre d'adresse", formatted_url);
}

#[cfg(windows)]
fn open_browser_new_tab(hwnd: HWND, url: &str) {
    let formatted_url = format_url_for_navigation(url);
    force_foreground_window(hwnd);
    unsafe {
        let _ = ShowWindow(hwnd, SW_MAXIMIZE);
    }
    std::thread::sleep(Duration::from_millis(100));

    const VK_CONTROL: VIRTUAL_KEY = VIRTUAL_KEY(0x11);
    const VK_T: VIRTUAL_KEY = VIRTUAL_KEY(0x54);
    const VK_RETURN: VIRTUAL_KEY = VIRTUAL_KEY(0x0D);

    // 1. Ouverture d'un nouvel onglet via Ctrl+T
    send_hotkey(&[VK_CONTROL], VK_T);
    std::thread::sleep(Duration::from_millis(150));

    // 2. Écriture de l'URL dans la barre d'adresse automatiquement focalisée
    clipboard::set_text(&formatted_url);
    std::thread::sleep(Duration::from_millis(50));
    send_paste();
    std::thread::sleep(Duration::from_millis(120));
    send_hotkey(&[], VK_RETURN);
    std::thread::sleep(Duration::from_millis(80));
}

#[cfg(windows)]
#[derive(Clone)]
struct UiaElementInfo {
    index: usize,
    element: IUIAutomationElement,
    automation_id: String,
    help_text: String,
    value_text: String,
    name: String,
    class_name: String,
    primary_class: String,
    data_attributes: Vec<(String, String)>,
    json_metadata: Option<serde_json::Value>,
    localized_type: String,
    aria_role: String,
    aria_label: String,
    aria_description: String,
    aria_indications: String,
    is_edit_or_textarea: bool,
    is_explicit_textarea: bool,
    is_input: bool,
    is_button: bool,
    is_link: bool,
    is_tab: bool,
    is_focusable: bool,
    is_tabindex: bool,
    is_web_content: bool,
    is_browser_chrome: bool,
    is_offscreen: bool,
    click_x: i32,
    click_y: i32,
    rect: RECT,
    pattern: Option<IUIAutomationInvokePattern>,
    area: i64,
}

#[cfg(windows)]
fn parse_aria_property_value(aria_properties: &str, target_key: &str) -> Option<String> {
    let target_clean = target_key.strip_prefix("aria-").unwrap_or(target_key).to_ascii_lowercase();
    for part in aria_properties.split(';') {
        let part = part.trim();
        if let Some((key, val)) = part.split_once('=') {
            let k = key.trim().to_ascii_lowercase();
            let k_clean = k.strip_prefix("aria-").unwrap_or(&k);
            if k_clean == target_clean {
                let v = val.trim().trim_matches('\'').trim_matches('"').trim();
                if !v.is_empty() {
                    return Some(v.to_string());
                }
            }
        }
    }
    None
}

#[cfg(windows)]
fn extract_aria_details(aria_properties: &str) -> (Option<String>, Option<String>, Vec<String>) {
    let label = parse_aria_property_value(aria_properties, "label")
        .or_else(|| parse_aria_property_value(aria_properties, "labelledby"));
    let desc = parse_aria_property_value(aria_properties, "description")
        .or_else(|| parse_aria_property_value(aria_properties, "describedby"))
        .or_else(|| parse_aria_property_value(aria_properties, "details"));

    let mut indications = Vec::new();
    if let Some(roledesc) = parse_aria_property_value(aria_properties, "roledescription") {
        indications.push(format!("rôle: {roledesc}"));
    }
    if let Some(ph) = parse_aria_property_value(aria_properties, "placeholder") {
        indications.push(format!("placeholder: \"{ph}\""));
    }
    if let Some(exp) = parse_aria_property_value(aria_properties, "expanded") {
        if exp == "true" { indications.push("déroulé".to_string()); }
        else if exp == "false" { indications.push("replié".to_string()); }
    }
    if let Some(chk) = parse_aria_property_value(aria_properties, "checked") {
        if chk == "true" { indications.push("coché".to_string()); }
        else if chk == "false" { indications.push("décoché".to_string()); }
    }
    if let Some(sel) = parse_aria_property_value(aria_properties, "selected") {
        if sel == "true" { indications.push("sélectionné".to_string()); }
    }
    if let Some(dis) = parse_aria_property_value(aria_properties, "disabled") {
        if dis == "true" { indications.push("désactivé".to_string()); }
    }
    if let Some(req) = parse_aria_property_value(aria_properties, "required") {
        if req == "true" { indications.push("requis".to_string()); }
    }
    if let Some(inv) = parse_aria_property_value(aria_properties, "invalid") {
        if inv == "true" { indications.push("invalide".to_string()); }
    }
    if let Some(ks) = parse_aria_property_value(aria_properties, "keyshortcuts") {
        indications.push(format!("raccourci: {ks}"));
    }

    (label, desc, indications)
}

#[cfg(windows)]
fn parse_data_attributes_and_json(
    aria_properties: &str,
    item_status: &str,
    help_text: &str,
) -> (Vec<(String, String)>, Option<serde_json::Value>) {
    let mut data_attrs = Vec::new();
    let mut json_val: Option<serde_json::Value> = None;

    // 1. Détection et extraction de blocs JSON dans item_status, help_text ou aria_properties
    for candidate in [item_status, help_text, aria_properties] {
        let trimmed = candidate.trim();
        if (trimmed.starts_with('{') && trimmed.ends_with('}'))
            || (trimmed.starts_with('[') && trimmed.ends_with(']'))
        {
            if let Ok(v) = serde_json::from_str::<serde_json::Value>(trimmed) {
                json_val = Some(v);
                break;
            }
        }
    }

    // Aplatir le JSON en paires clé-valeur dans data_attrs
    if let Some(serde_json::Value::Object(ref map)) = json_val {
        for (k, v) in map {
            let str_val = match v {
                serde_json::Value::String(s) => s.clone(),
                serde_json::Value::Number(n) => n.to_string(),
                serde_json::Value::Bool(b) => b.to_string(),
                _ => v.to_string(),
            };
            data_attrs.push((k.to_lowercase(), str_val));
        }
    }

    // 2. Extraction des attributs data-*, testid, target, action, etc.
    for source in [aria_properties, item_status] {
        for part in source.split(';') {
            let part = part.trim();
            if let Some((key, val)) = part.split_once('=') {
                let k = key.trim().to_ascii_lowercase();
                let v = val.trim().trim_matches('\'').trim_matches('"').trim();
                if !v.is_empty() {
                    let is_target_attr = k.starts_with("data-")
                        || k.starts_with("data_")
                        || k == "testid"
                        || k == "test-id"
                        || k == "target"
                        || k == "action"
                        || k == "tag"
                        || k == "describedby";
                    if is_target_attr && !data_attrs.iter().any(|(dk, _)| dk == &k) {
                        data_attrs.push((k, v.to_string()));
                    }
                }
            }
        }
    }

    (data_attrs, json_val)
}

#[cfg(windows)]
fn extract_inner_text(uia: &IUIAutomation, element: &IUIAutomationElement, max_chars: usize) -> Option<String> {
    unsafe {
        // 1. TextPattern DocumentRange
        if let Ok(pattern_unk) = element.GetCurrentPattern(UIA_TextPatternId) {
            if let Ok(text_pattern) = pattern_unk.cast::<IUIAutomationTextPattern>() {
                if let Ok(range) = text_pattern.DocumentRange() {
                    if let Ok(bstr) = range.GetText(max_chars as i32) {
                        let s = bstr.to_string();
                        let trimmed = s.trim();
                        if !trimmed.is_empty() {
                            return Some(trimmed.chars().take(max_chars).collect());
                        }
                    }
                }
            }
        }

        // 2. Direct Name
        if let Ok(name_bstr) = element.CurrentName() {
            let s = name_bstr.to_string();
            let trimmed = s.trim();
            if !trimmed.is_empty() {
                return Some(trimmed.chars().take(max_chars).collect());
            }
        }

        // 3. ValuePattern
        if let Ok(pattern_unk) = element.GetCurrentPattern(UIA_ValuePatternId) {
            if let Ok(val_pattern) = pattern_unk.cast::<IUIAutomationValuePattern>() {
                if let Ok(bstr) = val_pattern.CurrentValue() {
                    let s = bstr.to_string();
                    let trimmed = s.trim();
                    if !trimmed.is_empty() {
                        return Some(trimmed.chars().take(max_chars).collect());
                    }
                }
            }
        }

        // 4. Parcourir les descendants pour extraire le texte innerText des sous-nœuds (spans, labels, etc.)
        if let Ok(cond) = uia.ControlViewCondition() {
            if let Ok(children) = element.FindAll(TreeScope_Descendants, &cond) {
                let child_count = children.Length().unwrap_or(0).min(8);
                let mut collected = String::new();
                for ci in 0..child_count {
                    if let Ok(child) = children.GetElement(ci) {
                        if let Ok(cname) = child.CurrentName() {
                            let cstr = cname.to_string();
                            let ctrim = cstr.trim();
                            if !ctrim.is_empty() && !collected.contains(ctrim) {
                                if !collected.is_empty() {
                                    collected.push(' ');
                                }
                                collected.push_str(ctrim);
                                if collected.chars().count() >= max_chars {
                                    break;
                                }
                            }
                        }
                    }
                }
                if !collected.is_empty() {
                    return Some(collected.chars().take(max_chars).collect());
                }
            }
        }
    }
    None
}

#[cfg(windows)]
fn extract_web_document_text(element: &IUIAutomationElement, max_chars: usize) -> Option<String> {
    unsafe {
        if let Ok(pattern_unk) = element.GetCurrentPattern(UIA_TextPatternId) {
            if let Ok(text_pattern) = pattern_unk.cast::<IUIAutomationTextPattern>() {
                if let Ok(range) = text_pattern.DocumentRange() {
                    if let Ok(bstr) = range.GetText(max_chars as i32) {
                        let s = bstr.to_string();
                        let trimmed = s.trim();
                        if !trimmed.is_empty() {
                            return Some(trimmed.to_string());
                        }
                    }
                }
            }
        }
        if let Ok(pattern_unk) = element.GetCurrentPattern(UIA_ValuePatternId) {
            if let Ok(val_pattern) = pattern_unk.cast::<IUIAutomationValuePattern>() {
                if let Ok(bstr) = val_pattern.CurrentValue() {
                    let s = bstr.to_string();
                    let trimmed = s.trim();
                    if !trimmed.is_empty() {
                        return Some(trimmed.to_string());
                    }
                }
            }
        }
    }
    None
}

#[cfg(windows)]
fn extract_scrollable_viewports_content(uia: &IUIAutomation, hwnd: HWND) -> String {
    let mut output = String::new();
    unsafe {
        let Ok(window_element) = uia.ElementFromHandle(hwnd) else {
            return output;
        };
        let Ok(cond) = uia.ControlViewCondition() else {
            return output;
        };
        let Ok(elements) = window_element.FindAll(TreeScope_Descendants, &cond) else {
            return output;
        };

        let count = elements.Length().unwrap_or(0).clamp(0, 100);
        for i in 0..count {
            if let Ok(item) = elements.GetElement(i) {
                if let Ok(p_unk) = item.GetCurrentPattern(UIA_ScrollPatternId) {
                    if let Ok(scroll_pattern) = p_unk.cast::<IUIAutomationScrollPattern>() {
                        let is_v_scrollable = scroll_pattern.CurrentVerticallyScrollable().map(|b| b.as_bool()).unwrap_or(false);
                        if is_v_scrollable {
                            let v_percent = scroll_pattern.CurrentVerticalScrollPercent().unwrap_or(0.0);
                            let is_at_max = v_percent >= 99.0 || v_percent == 100.0;
                            let name = item.CurrentName().map(|b| b.to_string()).unwrap_or_default();
                            let loc_type = item.CurrentLocalizedControlType().map(|b| b.to_string()).unwrap_or_default();
                            let label = if !name.trim().is_empty() { name.trim().to_string() } else { loc_type };

                            if let Some(content) = extract_inner_text(uia, &item, 1800) {
                                let clean_md = if content.contains('<') && content.contains('>') {
                                    crate::html_util::sanitize_html_to_markdown(&content)
                                } else {
                                    content
                                };
                                let status = if is_at_max { "Scrollbar au MAXIMUM (100% - fin de liste/chat)" } else { "Viewport défilable actif" };
                                output.push_str(&format!("\n> ### Viewport : {} [{}]\n> Défilement : {:.0}%\n```\n{}\n```\n", label, status, v_percent, clean_md.trim()));
                            }
                        }
                    }
                }
            }
        }
    }
    output
}

#[cfg(windows)]
fn list_interactive_elements(hwnd: HWND) -> Vec<UiaElementInfo> {
    let mut results = Vec::new();
    if is_window_hung(hwnd) {
        eprintln!("[UIA] Fenêtre {:?} figée (sablier). Inspection UI ignorée.", hwnd.0);
        return results;
    }
    unsafe {
        let _ = CoInitializeEx(None, COINIT_MULTITHREADED);
        let uia: Result<IUIAutomation, _> = CoCreateInstance(&CUIAutomation, None, CLSCTX_INPROC_SERVER);
        let Ok(uia) = uia else {
            return results;
        };

        if let Ok(uia2) = uia.cast::<IUIAutomation2>() {
            let _ = uia2.SetTransactionTimeout(2000);
            let _ = uia2.SetConnectionTimeout(2000);
        }

        let Ok(window_element) = uia.ElementFromHandle(hwnd) else {
            return results;
        };

        let Ok(cond) = uia.ControlViewCondition() else {
            return results;
        };

        let Ok(elements) = window_element.FindAll(TreeScope_Descendants, &cond) else {
            return results;
        };

        let mut class_win_buf = [0u16; 256];
        let cw_len = GetClassNameW(hwnd, &mut class_win_buf);
        let is_toast_window = if cw_len > 0 {
            let cname = String::from_utf16_lossy(&class_win_buf[..cw_len as usize]);
            is_toast_notification_window(hwnd, &cname, "")
        } else {
            false
        };

        let count = elements.Length().unwrap_or(0).clamp(0, 250);

        // Délimitation géométrique du document web par rapport à la fenêtre
        let mut doc_rect: Option<RECT> = None;
        for i in 0..count {
            if let Ok(item) = elements.GetElement(i) {
                let ctype = item.CurrentControlType().unwrap_or_default();
                let class_name = item.CurrentClassName().map(|b| b.to_string()).unwrap_or_default().to_lowercase();
                let is_doc = ctype == UIA_DocumentControlTypeId
                    || class_name.contains("renderwidget")
                    || class_name.contains("rootwebarea");
                if is_doc {
                    if let Ok(r) = item.CurrentBoundingRectangle() {
                        let w = r.right - r.left;
                        let h = r.bottom - r.top;
                        if w > 200 && h > 150 {
                            let replace = match doc_rect {
                                Some(cur) => (w as i64 * h as i64) > ((cur.right - cur.left) as i64 * (cur.bottom - cur.top) as i64),
                                None => true,
                            };
                            if replace {
                                doc_rect = Some(r);
                            }
                        }
                    }
                }
            }
        }

        for i in 0..count {
            if let Ok(item) = elements.GetElement(i) {
                let rect = item.CurrentBoundingRectangle().unwrap_or_default();
                let width = rect.right - rect.left;
                let height = rect.bottom - rect.top;
                if width > 4 && height > 4 {
                    let ctype = item.CurrentControlType().unwrap_or_default();
                    let class_name = item.CurrentClassName().map(|b| b.to_string()).unwrap_or_default().to_lowercase();
                    let loc_type = item.CurrentLocalizedControlType().map(|b| b.to_string()).unwrap_or_default().to_lowercase();
                    let raw_name = item.CurrentName().map(|b| b.to_string()).unwrap_or_default();
                    let mut name = raw_name.trim().to_string();
                    let automation_id = item.CurrentAutomationId().map(|b| b.to_string()).unwrap_or_default();

                    // Filtrage des éléments techniques générés par les Chrome DevTools (F12)
                    let is_devtools = class_name.contains("devtools")
                        || automation_id.to_lowercase().contains("devtools")
                        || loc_type.contains("devtools")
                        || name.to_lowercase().starts_with("devtools")
                        || class_name.contains("inspector")
                        || automation_id.to_lowercase().contains("inspector")
                        || class_name.contains("treeview") && (name.contains("<") && name.contains(">"))
                        || class_name.contains("consoleview")
                        || class_name.contains("networkpanel")
                        || class_name.contains("sourcespanel")
                        || automation_id.to_lowercase().starts_with("elements-tab")
                        || automation_id.to_lowercase().starts_with("console-tab");
                    if is_devtools {
                        continue;
                    }

                    let is_swing_tab_class = class_name.contains("editortabs")
                        || class_name.contains("jbtabs")
                        || class_name.contains("tablabel")
                        || class_name.contains("singlerowlayout");

                    let has_source_ext = {
                        let n = name.to_lowercase();
                        n.ends_with(".rs")
                            || n.ends_with(".toml")
                            || n.ends_with(".json")
                            || n.ends_with(".md")
                            || n.ends_with(".txt")
                            || n.ends_with(".js")
                            || n.ends_with(".ts")
                            || n.ends_with(".py")
                            || n.ends_with(".java")
                            || n.ends_with(".c")
                            || n.ends_with(".cpp")
                    };

                    let is_top_editor_area = doc_rect.map_or(rect.top < 350, |dr| rect.top <= dr.top + 80);

                    let is_tab_candidate = ctype == UIA_TabItemControlTypeId
                        || ctype == UIA_TabControlTypeId
                        || loc_type.contains("onglet")
                        || loc_type.contains("tab")
                        || class_name.contains("tab")
                        || automation_id.to_lowercase().contains("tab")
                        || is_swing_tab_class
                        || (has_source_ext && is_top_editor_area && height < 50);

                    let is_offscreen = item.CurrentIsOffscreen().map(|b| b.as_bool()).unwrap_or(false);
                    let is_in_web = doc_rect.is_some() || is_browser_hwnd(hwnd, "");
                    // Conserver les éléments hors viewport dans les pages web pour permettre leur détection et ScrollIntoView
                    if is_offscreen && !is_tab_candidate && !is_toast_window && !is_in_web {
                        continue;
                    }
                    let ctype = item.CurrentControlType().unwrap_or_default();
                    let is_edit_type = ctype == UIA_EditControlTypeId || ctype == UIA_DocumentControlTypeId;
                    let help_text = item.CurrentHelpText().map(|b| b.to_string()).unwrap_or_default().trim().to_string();
                    let item_type = item.CurrentItemType().map(|b| b.to_string()).unwrap_or_default().to_lowercase();
                    let item_status_raw = item.CurrentItemStatus().map(|b| b.to_string()).unwrap_or_default().trim().to_string();
                    let item_status = item_status_raw.to_lowercase();

                    let primary_class = class_name
                        .split_whitespace()
                        .next()
                        .unwrap_or("")
                        .to_string();

                    let (aria_role, aria_properties) = if let Ok(e2) = item.cast::<IUIAutomationElement2>() {
                        let r = e2.CurrentAriaRole().ok()
                            .map(|b| b.to_string())
                            .unwrap_or_default();
                        let p = e2.CurrentAriaProperties().ok()
                            .map(|b| b.to_string())
                            .unwrap_or_default();
                        (r, p)
                    } else {
                        (String::new(), String::new())
                    };

                    let (data_attrs, json_metadata) = parse_data_attributes_and_json(&aria_properties, &item_status_raw, &help_text);
                    let (aria_label_opt, aria_desc_opt, indications_vec) = extract_aria_details(&aria_properties);
                    let aria_label = aria_label_opt.unwrap_or_default();
                    let aria_description = aria_desc_opt.unwrap_or_default();
                    let aria_indications = indications_vec.join(", ");

                    let is_root_web_area = class_name.contains("rootwebarea")
                        || loc_type.contains("rootwebarea")
                        || automation_id.to_lowercase().contains("rootwebarea");
                    if is_root_web_area {
                        continue;
                    }

                    let value_text = item
                        .GetCurrentPattern(UIA_ValuePatternId)
                        .ok()
                        .and_then(|p| p.cast::<IUIAutomationValuePattern>().ok())
                        .and_then(|vp| vp.CurrentValue().ok())
                        .map(|b| b.to_string())
                        .unwrap_or_default()
                        .trim().to_string();
                    let is_focusable = item.CurrentIsKeyboardFocusable().map(|b| b.as_bool()).unwrap_or(false);

                    let is_explicit_textarea = class_name.contains("textarea")
                        || loc_type.contains("textarea")
                        || loc_type.contains("zone de texte")
                        || aria_role.to_lowercase() == "textbox" && height >= 35
                        || (is_edit_type && height >= 35);

                    let is_aria_input = aria_role.to_lowercase().contains("textbox")
                        || aria_role.to_lowercase().contains("searchbox")
                        || aria_role.to_lowercase().contains("combobox")
                        || aria_properties.to_lowercase().contains("role=textbox")
                        || aria_properties.to_lowercase().contains("role=searchbox");

                    let is_input = !is_explicit_textarea && (is_edit_type
                        || is_aria_input
                        || class_name.contains("input")
                        || loc_type.contains("input")
                        || class_name.contains("edit")
                        || loc_type.contains("edit")
                        || loc_type.contains("saisie"));

                    let is_edit_or_textarea = is_explicit_textarea || is_input;

                    let is_role_button = aria_role == "button"
                        || aria_role.contains("button")
                        || aria_role.contains("bouton")
                        || aria_properties.contains("role=button")
                        || aria_properties.contains("role='button'")
                        || aria_properties.contains("role=\"button\"");

                    let has_button_data = data_attrs.iter().any(|(k, v)| {
                        (k.contains("role") && v == "button")
                            || (k.contains("action") && (v.contains("click") || v.contains("submit") || v.contains("btn")))
                            || (k.contains("test") && (v.contains("btn") || v.contains("button")))
                    });

                    let is_button = ctype == UIA_ButtonControlTypeId
                        || loc_type.contains("bouton")
                        || loc_type.contains("button")
                        || is_role_button
                        || has_button_data
                        || class_name.contains("button")
                        || class_name.contains("btn")
                        || automation_id.to_lowercase().contains("button")
                        || automation_id.to_lowercase().contains("btn");

                    let is_role_link = aria_role == "link"
                        || aria_role.contains("link")
                        || aria_role.contains("lien")
                        || aria_properties.contains("role=link")
                        || aria_properties.contains("role='link'")
                        || aria_properties.contains("role=\"link\"")
                        || item_type.contains("link")
                        || item_type.contains("lien")
                        || item_status.contains("link")
                        || class_name.contains("role-link")
                        || class_name.contains("role_link")
                        || class_name.contains("role=link")
                        || (class_name.contains("link") && !is_edit_type);

                    let is_link = ctype == UIA_HyperlinkControlTypeId
                        || loc_type.contains("lien")
                        || loc_type.contains("link")
                        || loc_type.contains("hyperlink")
                        || is_role_link;

                    let mut is_tab = is_tab_candidate;

                    let (is_web_content, is_browser_chrome) = match doc_rect {
                        Some(dr) => {
                            if rect.bottom <= (dr.top + 5) {
                                (false, true)
                            } else {
                                (true, false)
                            }
                        }
                        None => (false, false),
                    };

                    // Prise en compte et intégration sémantique prioritaire de l'aria-label
                    if !aria_label.is_empty() {
                        if name.is_empty() || is_poor_or_redundant_link(&name) {
                            name = aria_label.clone();
                        } else if !name.to_lowercase().contains(&aria_label.to_lowercase()) {
                            if aria_label.to_lowercase().contains(&name.to_lowercase()) {
                                name = aria_label.clone();
                            } else {
                                name = format!("{name} [aria: {aria_label}]");
                            }
                        }
                    }

                    let has_tabindex = (aria_properties.contains("tabindex")
                        && !aria_properties.contains("tabindex=-1")
                        && !aria_properties.contains("tabindex=\"-1\"")
                        && !aria_properties.contains("tabindex='-1'"))
                        || (is_web_content && is_focusable && !is_edit_or_textarea && !is_link && !is_button);
                    let is_tabindex = has_tabindex;

                    if is_browser_chrome && !name.is_empty() && !name.starts_with("[Navigateur]") {
                        name = format!("[Navigateur] {name}");
                    }

                    const MAX_INTERACTIVE_TEXT_LEN: usize = 110;
                    if (name.is_empty() || is_tabindex) && (is_link || is_button || is_tab || is_tabindex || is_focusable) {
                        if let Some(txt) = extract_inner_text(&uia, &item, MAX_INTERACTIVE_TEXT_LEN) {
                            if name.is_empty() {
                                name = txt;
                                if !is_tab {
                                    let n_low = name.to_lowercase();
                                    let has_ext = n_low.ends_with(".rs")
                                        || n_low.ends_with(".toml")
                                        || n_low.ends_with(".json")
                                        || n_low.ends_with(".md")
                                        || n_low.ends_with(".txt")
                                        || n_low.ends_with(".py")
                                        || n_low.ends_with(".java")
                                        || n_low.ends_with(".js")
                                        || n_low.ends_with(".ts");
                                    if has_ext && is_top_editor_area && height < 50 {
                                        is_tab = true;
                                    }
                                }
                            } else if is_tabindex && name != txt && !name.contains(&txt) {
                                let combined = format!("{name} {txt}");
                                name = combined.chars().take(MAX_INTERACTIVE_TEXT_LEN).collect();
                            }
                        }
                    }

                    if name.chars().count() > MAX_INTERACTIVE_TEXT_LEN {
                        name = name.chars().take(MAX_INTERACTIVE_TEXT_LEN).collect();
                    }

                    if name.is_empty() {
                        if let Some((_, val)) = data_attrs.iter().find(|(k, _)| k.contains("test") || k.contains("action") || k.contains("target")) {
                            name = format!("[data: {val}]");
                        } else if !primary_class.is_empty() && (is_button || is_link || is_tab || is_edit_or_textarea) {
                            name = format!(".{primary_class}");
                        }
                    }

                    if name.is_empty() && aria_label.is_empty() && automation_id.is_empty() && help_text.is_empty() && value_text.is_empty()
                        && data_attrs.is_empty() && primary_class.is_empty()
                        && !is_edit_or_textarea && !is_button && !is_link && !is_tab && !is_focusable && !is_tabindex {
                        continue;
                    }

                    let click_x = rect.left + width / 2;
                    let click_y = rect.top + height / 2;
                    let pattern = item.GetCurrentPattern(UIA_InvokePatternId)
                        .ok()
                        .and_then(|p| p.cast::<IUIAutomationInvokePattern>().ok());
                    let area = (width as i64) * (height as i64);
                    let index = results.len() + 1;
                    results.push(UiaElementInfo {
                        index,
                        element: item,
                        automation_id,
                        help_text,
                        value_text,
                        name,
                        class_name,
                        primary_class,
                        data_attributes: data_attrs,
                        json_metadata,
                        localized_type: loc_type,
                        aria_role,
                        aria_label,
                        aria_description,
                        aria_indications,
                        is_edit_or_textarea,
                        is_explicit_textarea,
                        is_input,
                        is_button,
                        is_link,
                        is_tab,
                        is_focusable,
                        is_tabindex,
                        is_web_content,
                        is_browser_chrome,
                        is_offscreen,
                        click_x,
                        click_y,
                        rect,
                        pattern,
                        area,
                    });
                }
            }
        }
        results.sort_by_key(|e| !e.is_tab);
    }
    results
}

#[cfg(windows)]
fn has_focused_textarea(_hwnd: HWND) -> bool {
    unsafe {
        let _ = CoInitializeEx(None, COINIT_MULTITHREADED);
        let Ok(uia): Result<IUIAutomation, _> = CoCreateInstance(&CUIAutomation, None, CLSCTX_INPROC_SERVER) else {
            return false;
        };
        let Ok(focused) = uia.GetFocusedElement() else {
            return false;
        };

        let overlay_hwnd = FindWindowW(None, w!("Libertide overlay")).unwrap_or(HWND(std::ptr::null_mut()));
        if let Ok(native_handle) = focused.CurrentNativeWindowHandle() {
            let h = HWND(native_handle.0 as _);
            if !h.0.is_null() && h == overlay_hwnd {
                return false;
            }
        }

        let ctype = focused.CurrentControlType().unwrap_or_default();
        let is_edit = ctype == UIA_EditControlTypeId || ctype == UIA_DocumentControlTypeId;
        let class_name = focused.CurrentClassName().map(|b| b.to_string()).unwrap_or_default().to_lowercase();
        let loc_type = focused.CurrentLocalizedControlType().map(|b| b.to_string()).unwrap_or_default().to_lowercase();
        let automation_id = focused.CurrentAutomationId().map(|b| b.to_string()).unwrap_or_default().to_lowercase();
        let height = focused.CurrentBoundingRectangle().map(|r| r.bottom - r.top).unwrap_or(0);

        let is_root_web_area = class_name.contains("rootwebarea")
            || loc_type.contains("rootwebarea")
            || automation_id.contains("rootwebarea");
        if is_root_web_area {
            return false;
        }

        class_name.contains("textarea")
            || loc_type.contains("textarea")
            || loc_type.contains("zone de texte")
            || (is_edit && height >= 35)
    }
}

#[cfg(windows)]
fn refocus_largest_textarea_or_fallback(hwnd: HWND) -> bool {
    let mut elements = list_interactive_elements(hwnd);
    if elements.is_empty() {
        return false;
    }

    let has_web = elements.iter().any(|e| e.is_web_content);
    let mut textareas: Vec<&UiaElementInfo> = elements
        .iter()
        .filter(|e| (!has_web || !e.is_browser_chrome) && e.is_explicit_textarea)
        .collect();
    textareas.sort_by(|a, b| b.area.cmp(&a.area));

    if let Some(target) = textareas.first() {
        unsafe { let _ = target.element.SetFocus(); }
        click_element(target.click_x, target.click_y, Some(&target.element), target.pattern.as_ref());
        return true;
    }

    let mut inputs: Vec<&UiaElementInfo> = elements
        .iter()
        .filter(|e| (!has_web || !e.is_browser_chrome) && (e.is_input || e.is_edit_or_textarea))
        .collect();
    inputs.sort_by(|a, b| b.area.cmp(&a.area));

    if let Some(target) = inputs.first() {
        unsafe { let _ = target.element.SetFocus(); }
        click_element(target.click_x, target.click_y, Some(&target.element), target.pattern.as_ref());
        return true;
    }

    let mut focusables: Vec<&UiaElementInfo> = elements
        .iter()
        .filter(|e| e.is_focusable || (e.rect.right > e.rect.left && e.rect.bottom > e.rect.top))
        .collect();
    focusables.sort_by(|a, b| b.area.cmp(&a.area));

    if let Some(target) = focusables.first() {
        unsafe { let _ = target.element.SetFocus(); }
        click_element(target.click_x, target.click_y, Some(&target.element), target.pattern.as_ref());
        return true;
    }

    false
}

#[cfg(windows)]
fn find_largest_textarea(hwnd: HWND) -> Option<UiaElementInfo> {
    let mut elements = list_interactive_elements(hwnd);
    if elements.is_empty() {
        return None;
    }

    let has_web = elements.iter().any(|e| e.is_web_content && (e.is_explicit_textarea || e.is_edit_or_textarea || e.is_input));
    let mut textareas: Vec<UiaElementInfo> = elements
        .into_iter()
        .filter(|e| {
            if has_web && e.is_browser_chrome {
                return false;
            }
            e.is_explicit_textarea || e.is_edit_or_textarea || e.is_input
        })
        .collect();

    textareas.sort_by(|a, b| {
        if a.is_web_content != b.is_web_content {
            return b.is_web_content.cmp(&a.is_web_content);
        }
        if a.is_explicit_textarea != b.is_explicit_textarea {
            return b.is_explicit_textarea.cmp(&a.is_explicit_textarea);
        }
        b.area.cmp(&a.area)
    });

    textareas.into_iter().next()
}

#[cfg(windows)]
fn find_largest_textarea_on_screen(
    user_windows: &[(HWND, String)],
    preferred_hwnd: Option<HWND>,
) -> Option<(HWND, UiaElementInfo)> {
    let mut ordered: Vec<HWND> = Vec::new();
    if let Some(pref) = preferred_hwnd {
        ordered.push(pref);
    }
    for &(h, _) in user_windows {
        if !ordered.contains(&h) {
            ordered.push(h);
        }
    }

    for hwnd in ordered.into_iter().take(3) {
        if let Some(target) = find_largest_textarea(hwnd) {
            return Some((hwnd, target));
        }
    }

    None
}

#[cfg(windows)]
fn ensure_window_textarea_focus(hwnd: HWND) {
    if !has_focused_textarea(hwnd) {
        refocus_largest_textarea_or_fallback(hwnd);
    }
}

#[cfg(windows)]
fn clear_window_text(hwnd: HWND) {
    force_foreground_window(hwnd);
    unsafe {
        let _ = ShowWindow(hwnd, SW_RESTORE);
    }
    std::thread::sleep(Duration::from_millis(60));
    ensure_window_textarea_focus(hwnd);
    std::thread::sleep(Duration::from_millis(40));
    const VK_CONTROL: VIRTUAL_KEY = VIRTUAL_KEY(0x11);
    const VK_A: VIRTUAL_KEY = VIRTUAL_KEY(0x41);
    const VK_BACK: VIRTUAL_KEY = VIRTUAL_KEY(0x08);
    send_hotkey(&[VK_CONTROL], VK_A);
    std::thread::sleep(Duration::from_millis(40));
    send_hotkey(&[], VK_BACK);
}

#[cfg(windows)]
pub fn run_system_command(command: &str) -> bool {
    let trimmed = command.trim();
    if trimmed.is_empty() {
        return false;
    }

    let mut process = std::process::Command::new("cmd.exe");
    process.creation_flags(CREATE_NEW_CONSOLE);

    let mut cur = trimmed;

    // 1. Déballage des préfixes "cmd.exe /c", "cmd.exe /k", "cmd /c", "cmd /k" superflus
    if cur.len() >= 11 && (cur[..11].eq_ignore_ascii_case("cmd.exe /c ") || cur[..11].eq_ignore_ascii_case("cmd.exe /k ")) {
        cur = cur[11..].trim();
    } else if cur.len() >= 7 && (cur[..7].eq_ignore_ascii_case("cmd /c ") || cur[..7].eq_ignore_ascii_case("cmd /k ")) {
        cur = cur[7..].trim();
    }

    // 2. Déballage d'un éventuel "start \"\" " ou "start "
    if cur.len() >= 9 && cur[..9].eq_ignore_ascii_case("start \"\" ") {
        cur = cur[9..].trim();
    } else if cur.len() >= 6 && cur[..6].eq_ignore_ascii_case("start ") {
        cur = cur[6..].trim();
    }

    // 3. Maintien impératif de la console ouverte via /K au lieu de /C
    // afin d'afficher la sortie sans fermer la fenêtre à la fin de l'exécution.
    process.raw_arg(format!("/K {cur}"));

    process.spawn().is_ok()
}

#[cfg(not(windows))]
pub fn run_system_command(_command: &str) -> bool {
    false
}

#[cfg(windows)]
fn write_to_temp_txt_file(text: &str) -> String {
    let initial_hwnds: std::collections::HashSet<isize> = list_user_windows()
        .into_iter()
        .map(|(h, _)| h.0 as isize)
        .collect();

    if let Ok(temp_path) = create_temp_note_file(text) {
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
pub fn write_to_browser_or_txt(text: &str) -> String {
    let user_windows = list_user_windows();
    let active_hwnd = get_active_or_best_window(&user_windows);

    if let Some(target_hwnd) = active_hwnd {
        let win_title = user_windows
            .iter()
            .find(|(h, _)| *h == target_hwnd)
            .map(|(_, t)| t.as_str())
            .unwrap_or("");
        if is_terminal_window(target_hwnd, win_title) {
            force_foreground_window(target_hwnd);
            unsafe { let _ = ShowWindow(target_hwnd, SW_RESTORE); }
            std::thread::sleep(Duration::from_millis(60));
            let auto_enter = text.ends_with('\n') || text.ends_with('\r');
            send_terminal_input(target_hwnd, text, auto_enter);
            return if auto_enter {
                "Commande envoyée et validée dans le terminal actif.".to_string()
            } else {
                "Texte inséré dans le terminal actif.".to_string()
            };
        }

        force_foreground_window(target_hwnd);
        unsafe {
            let _ = ShowWindow(target_hwnd, SW_RESTORE);
        }
        std::thread::sleep(Duration::from_millis(80));

        if let Some(elem) = find_largest_textarea(target_hwnd) {
            unsafe { let _ = elem.element.SetFocus(); }
            click_element(elem.click_x, elem.click_y, Some(&elem.element), elem.pattern.as_ref());
            std::thread::sleep(Duration::from_millis(50));
        } else {
            ensure_window_textarea_focus(target_hwnd);
        }
        std::thread::sleep(Duration::from_millis(50));

        const VK_CONTROL: VIRTUAL_KEY = VIRTUAL_KEY(0x11);
        const VK_A: VIRTUAL_KEY = VIRTUAL_KEY(0x41);
        send_hotkey(&[VK_CONTROL], VK_A);
        std::thread::sleep(Duration::from_millis(30));

        clipboard::set_text(text);
        std::thread::sleep(Duration::from_millis(40));
        send_paste();

        return "Texte collé dans la fenêtre active.".to_string();
    }

    write_to_temp_txt_file(text)
}

#[cfg(not(windows))]
pub fn write_to_browser_or_txt(text: &str) -> String {
    if create_temp_note_file(text).is_ok() {
        "Texte écrit dans le fichier temporaire.".to_string()
    } else {
        "Échec de création du fichier temporaire.".to_string()
    }
}

#[cfg(windows)]
fn extract_element_text(element: &IUIAutomationElement) -> Option<String> {
    unsafe {
        if let Ok(pattern_unk) = element.GetCurrentPattern(UIA_ValuePatternId) {
            if let Ok(val_pattern) = pattern_unk.cast::<IUIAutomationValuePattern>() {
                if let Ok(bstr) = val_pattern.CurrentValue() {
                    let s = bstr.to_string();
                    if !s.is_empty() {
                        return Some(s);
                    }
                }
            }
        }

        if let Ok(pattern_unk) = element.GetCurrentPattern(UIA_TextPatternId) {
            if let Ok(text_pattern) = pattern_unk.cast::<IUIAutomationTextPattern>() {
                if let Ok(range) = text_pattern.DocumentRange() {
                    if let Ok(bstr) = range.GetText(-1) {
                        let s = bstr.to_string();
                        if !s.is_empty() {
                            return Some(s);
                        }
                    }
                }
            }
        }

        if let Ok(name_bstr) = element.CurrentName() {
            let s = name_bstr.to_string();
            if !s.is_empty() {
                return Some(s);
            }
        }
    }
    None
}

#[cfg(windows)]
pub fn get_active_field_content() -> Option<String> {
    unsafe {
        let _ = CoInitializeEx(None, COINIT_MULTITHREADED);
        let uia: Result<IUIAutomation, _> = CoCreateInstance(&CUIAutomation, None, CLSCTX_INPROC_SERVER);
        let Ok(uia) = uia else {
            return None;
        };

        let overlay_hwnd = FindWindowW(None, w!("Libertide overlay")).unwrap_or(HWND(std::ptr::null_mut()));

        if let Ok(focused) = uia.GetFocusedElement() {
            let is_overlay = if let Ok(native_handle) = focused.CurrentNativeWindowHandle() {
                HWND(native_handle.0 as _) == overlay_hwnd
            } else {
                false
            };

            let class_name = focused.CurrentClassName().map(|b| b.to_string()).unwrap_or_default().to_lowercase();
            let loc_type = focused.CurrentLocalizedControlType().map(|b| b.to_string()).unwrap_or_default().to_lowercase();
            let is_root_web_area = class_name.contains("rootwebarea") || loc_type.contains("rootwebarea");

            if !is_overlay && !is_root_web_area {
                if let Some(text) = extract_element_text(&focused) {
                    if !text.is_empty() {
                        return Some(text);
                    }
                }
            }
        }

        let user_windows = list_user_windows();
        let target_hwnd = get_active_or_best_window(&user_windows);

        if let Some(hwnd) = target_hwnd {
            let mut elements = list_interactive_elements(hwnd);
            elements.sort_by(|a, b| b.area.cmp(&a.area));
            for elem in elements.iter().filter(|e| e.is_edit_or_textarea) {
                if let Some(text) = extract_element_text(&elem.element) {
                    if !text.is_empty() {
                        return Some(text);
                    }
                }
            }
        }
    }
    None
}

#[cfg(not(windows))]
pub fn get_active_field_content() -> Option<String> {
    None
}

#[cfg(windows)]
fn replace_active_field_text(new_text: &str) -> String {
    unsafe {
        let _ = CoInitializeEx(None, COINIT_MULTITHREADED);
        let _uia: Result<IUIAutomation, _> = CoCreateInstance(&CUIAutomation, None, CLSCTX_INPROC_SERVER);
        let user_windows = list_user_windows();
        let target_hwnd = get_active_or_best_window(&user_windows);

        if let Some(hwnd) = target_hwnd {
            force_foreground_window(hwnd);
            let _ = ShowWindow(hwnd, SW_RESTORE);
            std::thread::sleep(Duration::from_millis(80));

            ensure_window_textarea_focus(hwnd);
            std::thread::sleep(Duration::from_millis(50));

            const VK_CONTROL: VIRTUAL_KEY = VIRTUAL_KEY(0x11);
            const VK_A: VIRTUAL_KEY = VIRTUAL_KEY(0x41);
            send_hotkey(&[VK_CONTROL], VK_A);
            std::thread::sleep(Duration::from_millis(40));

            clipboard::set_text(new_text);
            std::thread::sleep(Duration::from_millis(40));
            send_paste();

            return "Texte mis à jour dans le champ actif.".to_string();
        }
    }

    write_to_browser_or_txt(new_text)
}

#[cfg(not(windows))]
fn replace_active_field_text(text: &str) -> String {
    write_to_browser_or_txt(text)
}

pub fn parse_write_command(prompt: &str) -> Option<String> {
    let trimmed = prompt.trim();
    let lower = trimmed.to_lowercase();

    let is_smart_edit = true;
    if is_smart_edit {
        return None;
    }

    let prefixes = [
        "ecris :", "écris :", "ecrit :", "écrit :",
        "ecris ", "écris ", "ecrit ", "écrit ", "ecrire ", "écrire ",
    ];
    for prefix in prefixes {
        if lower.starts_with(prefix) {
            let rest = trimmed[prefix.len()..].trim();
            let clean = rest.strip_prefix(':').unwrap_or(rest).trim();
            let clean_lower = clean.to_lowercase();

            if clean_lower.starts_with("la suite")
                || clean_lower.starts_with("un ")
                || clean_lower.starts_with("une ")
                || clean_lower.starts_with("dans ")
                || clean_lower.starts_with("sur ")
                || clean_lower.starts_with("a ")
                || clean_lower.starts_with("à ")
                || clean_lower.starts_with("pour ")
            {
                return None;
            }

            let unquoted = clean
                .strip_prefix('"').and_then(|s| s.strip_suffix('"'))
                .or_else(|| clean.strip_prefix('«').and_then(|s| s.strip_suffix('»')))
                .unwrap_or(clean)
                .trim();
            if !unquoted.is_empty() {
                return Some(unquoted.to_string());
            }
        }
    }
    None
}

pub fn try_execute_direct_cli(prompt: &str) -> Option<String> {
    let trimmed = prompt.trim();
    if trimmed.is_empty() {
        return None;
    }

    let (is_forced, cmd_str) = if let Some(stripped) = trimmed.strip_prefix('>') {
        (true, stripped.trim())
    } else if let Some(stripped) = trimmed.strip_prefix('$') {
        (true, stripped.trim())
    } else {
        (false, trimmed)
    };

    let first_token = cmd_str.split_whitespace().next().unwrap_or("");
    if first_token.is_empty() {
        return None;
    }

    #[cfg(windows)]
    if is_forced || find_executable_in_path(first_token).is_some() {
        if run_system_command(cmd_str) {
            return Some(format!("Commande exécutée : {cmd_str}"));
        }
    }

    None
}

#[cfg(windows)]
fn click_element(
    x: i32,
    y: i32,
    element: Option<&IUIAutomationElement>,
    invoke_pattern: Option<&IUIAutomationInvokePattern>,
) {
    if let Some(elem) = element {
        unsafe {
            if let Ok(pattern_unk) = elem.GetCurrentPattern(UIA_ScrollItemPatternId) {
                if let Ok(scroll_pattern) = pattern_unk.cast::<IUIAutomationScrollItemPattern>() {
                    let _ = scroll_pattern.ScrollIntoView();
                    std::thread::sleep(Duration::from_millis(50));
                }
            }
            let _ = elem.SetFocus();
            std::thread::sleep(Duration::from_millis(30));
        }
    }

    let mut click_x = x;
    let mut click_y = y;
    if let Some(elem) = element {
        unsafe {
            if let Ok(rect) = elem.CurrentBoundingRectangle() {
                let width = rect.right - rect.left;
                let height = rect.bottom - rect.top;
                if width > 0 && height > 0 {
                    click_x = rect.left + (width / 2);
                    click_y = rect.top + (height / 2);
                }
            }
        }
    }

    if let Some(pattern) = invoke_pattern {
        unsafe {
            let _ = pattern.Invoke();
        }
        std::thread::sleep(Duration::from_millis(20));
    }

    unsafe {
        let _ = SetCursorPos(click_x, click_y);
        std::thread::sleep(Duration::from_millis(30));

        let mouse_down = INPUT {
            r#type: INPUT_MOUSE,
            Anonymous: INPUT_0 {
                mi: MOUSEINPUT {
                    dx: 0,
                    dy: 0,
                    mouseData: 0,
                    dwFlags: MOUSEEVENTF_LEFTDOWN,
                    time: 0,
                    dwExtraInfo: 0,
                },
            },
        };
        let mouse_up = INPUT {
            r#type: INPUT_MOUSE,
            Anonymous: INPUT_0 {
                mi: MOUSEINPUT {
                    dx: 0,
                    dy: 0,
                    mouseData: 0,
                    dwFlags: MOUSEEVENTF_LEFTUP,
                    time: 0,
                    dwExtraInfo: 0,
                },
            },
        };
        SendInput(&[mouse_down], std::mem::size_of::<INPUT>() as i32);
        std::thread::sleep(Duration::from_millis(40));
        SendInput(&[mouse_up], std::mem::size_of::<INPUT>() as i32);
    }
}

#[cfg(windows)]
fn is_ecommerce_action_term(term: &str) -> bool {
    let t = term.to_lowercase();
    t.contains("commande")
        || t.contains("panier")
        || t.contains("cart")
        || t.contains("checkout")
        || t.contains("valider")
        || t.contains("payer")
        || t.contains("commander")
}

#[cfg(windows)]
fn ecommerce_match_bonus(prop: &str, target_text: &str) -> f32 {
    let p = prop.to_lowercase();
    let t = target_text.to_lowercase();

    if is_ecommerce_action_term(&p) {
        const CART_KEYWORDS: &[&str] = &[
            "panier", "cart", "commander", "commande", "valider", "passer la commande",
            "finaliser", "checkout", "payer", "achat", "payer maintenant", "voir le panier",
        ];
        let p_has = CART_KEYWORDS.iter().any(|k| p.contains(k));
        let t_has = CART_KEYWORDS.iter().any(|k| t.contains(k));
        if p_has && t_has {
            // Bonus significatif pour relier "valide la commande" à "panier" / "commander"
            let exact = if (p.contains("valide") || p.contains("commande"))
                && (t.contains("panier") || t.contains("commander") || t.contains("checkout") || t.contains("valider"))
            {
                75.0
            } else {
                45.0
            };
            return exact;
        }
    }
    0.0
}

#[cfg(windows)]
fn score_single_proposition(elem: &UiaElementInfo, prop: &str) -> Option<f32> {
    let prop_clean = prop.trim();
    let prop_lower = prop_clean.to_lowercase();

    // 1. Détection des sélecteurs d'ID directs (ex: "#submit-btn")
    if let Some(id_part) = prop_clean.strip_prefix('#') {
        let id_clean = id_part.trim();
        if !id_clean.is_empty() {
            if elem.automation_id.eq_ignore_ascii_case(id_clean) {
                return Some(500.0);
            }
            for (k, v) in &elem.data_attributes {
                if (k == "id" || k == "testid" || k.ends_with("id")) && v.eq_ignore_ascii_case(id_clean) {
                    return Some(480.0);
                }
            }
            if elem.name.eq_ignore_ascii_case(id_clean) {
                return Some(420.0);
            }
        }
    }

    // 2. Détection des sélecteurs macro indexés (ex: "@btn:12", "@link:15", "@input:3", "@12")
    if prop_clean.starts_with('@') && !prop_clean.starts_with("[@") {
        let tag = prop_clean.trim_start_matches('@');
        let num_str = tag
            .strip_prefix("btn:")
            .or_else(|| tag.strip_prefix("link:"))
            .or_else(|| tag.strip_prefix("input:"))
            .or_else(|| tag.strip_prefix("textarea:"))
            .or_else(|| tag.strip_prefix("select:"))
            .unwrap_or(tag);

        if let Ok(target_idx) = num_str.trim().parse::<usize>() {
            if elem.index == target_idx {
                return Some(500.0);
            }
        }
    }

    // 3. Détection des macros complètes (ex: [@btn:12 id="submit-btn" text="Valider"])
    if prop_clean.starts_with("[@") && prop_clean.ends_with(']') {
        let inner = &prop_clean[2..prop_clean.len() - 1];

        let tag_part = inner.split_whitespace().next().unwrap_or("");
        if let Some((_, idx_str)) = tag_part.split_once(':') {
            if let Ok(idx) = idx_str.parse::<usize>() {
                if elem.index == idx {
                    return Some(520.0);
                }
            }
        }

        if let Some(id_val) = crate::html_util::extract_attribute(inner, "id") {
            if !id_val.is_empty() && elem.automation_id.eq_ignore_ascii_case(id_val) {
                return Some(500.0);
            }
        }
        if let Some(testid_val) = crate::html_util::extract_attribute(inner, "data-testid") {
            if !testid_val.is_empty() && (elem.automation_id.eq_ignore_ascii_case(testid_val)
                || elem.data_attributes.iter().any(|(_, v)| v.eq_ignore_ascii_case(testid_val)))
            {
                return Some(490.0);
            }
        }
        if let Some(name_val) = crate::html_util::extract_attribute(inner, "name") {
            if !name_val.is_empty() && (elem.automation_id.eq_ignore_ascii_case(name_val) || elem.name.eq_ignore_ascii_case(name_val)) {
                return Some(460.0);
            }
        }
        if let Some(text_val) = crate::html_util::extract_attribute(inner, "text") {
            if !text_val.is_empty() && (elem.name.eq_ignore_ascii_case(text_val) || elem.aria_label.eq_ignore_ascii_case(text_val)) {
                return Some(450.0);
            }
        }
    }

    let name_trim = elem.name.trim();
    let name_lower = name_trim.to_lowercase();
    let aria_label_trim = elem.aria_label.trim();
    let aria_label_lower = aria_label_trim.to_lowercase();

    if (!name_lower.is_empty() && name_lower == prop_lower)
        || (!aria_label_lower.is_empty() && aria_label_lower == prop_lower)
    {
        return Some(350.0);
    }

    let ecom_bonus = ecommerce_match_bonus(prop_clean, &elem.name)
        .max(ecommerce_match_bonus(prop_clean, &elem.aria_label));
    let mut best_score: Option<f32> = None;

    let mut regex_score = 0.0f32;
    if matches_pattern(prop, &elem.name) {
        regex_score = regex_score.max(180.0);
    }
    if !elem.aria_label.is_empty() && matches_pattern(prop, &elem.aria_label) {
        regex_score = regex_score.max(190.0);
    }
    if !elem.aria_description.is_empty() && matches_pattern(prop, &elem.aria_description) {
        regex_score = regex_score.max(150.0);
    }
    if !elem.aria_indications.is_empty() && matches_pattern(prop, &elem.aria_indications) {
        regex_score = regex_score.max(130.0);
    }
    if matches_pattern(prop, &elem.help_text) {
        regex_score = regex_score.max(160.0);
    }
    if matches_pattern(prop, &elem.automation_id) {
        regex_score = regex_score.max(150.0);
    }
    if matches_pattern(prop, &elem.value_text) {
        regex_score = regex_score.max(140.0);
    }
    if matches_pattern(prop, &elem.localized_type) {
        regex_score = regex_score.max(110.0);
    }
    if matches_pattern(prop, &elem.aria_role) {
        regex_score = regex_score.max(120.0);
    }
    if matches_pattern(prop, &elem.class_name) {
        regex_score = regex_score.max(90.0);
    }
    if !elem.primary_class.is_empty() && matches_pattern(prop, &elem.primary_class) {
        regex_score = regex_score.max(140.0);
    }
    for (k, v) in &elem.data_attributes {
        if matches_pattern(prop, v) {
            regex_score = regex_score.max(170.0);
        } else if matches_pattern(prop, k) {
            regex_score = regex_score.max(120.0);
        }
    }

    regex_score += ecom_bonus;

    if regex_score > 0.0 {
        if !elem.name.is_empty() && prop_clean.len() >= 3 {
            let ratio = (elem.name.len() as f32) / (prop_clean.len() as f32);
            if ratio > 3.0 {
                let len_penalty = ((ratio - 3.0) * 4.0).min(120.0);
                regex_score = (regex_score - len_penalty).max(10.0);
            }
        }
        best_score = Some(regex_score);
    }

    let q_words = clean_words(prop);
    if q_words.is_empty() {
        return best_score;
    }

    let id_lower = elem.automation_id.to_lowercase();
    let help_lower = elem.help_text.to_lowercase();
    let val_lower = elem.value_text.to_lowercase();
    let class_lower = elem.class_name.to_lowercase();
    let type_lower = elem.localized_type.to_lowercase();
    let aria_lower = elem.aria_role.to_lowercase();
    let aria_desc_lower = elem.aria_description.to_lowercase();
    let aria_ind_lower = elem.aria_indications.to_lowercase();

    let name_words = clean_words(&name_lower);
    let aria_label_words = clean_words(&aria_label_lower);
    let aria_desc_words = clean_words(&aria_desc_lower);
    let aria_ind_words = clean_words(&aria_ind_lower);
    let id_words = clean_words(&id_lower);
    let help_words = clean_words(&help_lower);
    let val_words = clean_words(&val_lower);
    let class_words = clean_words(&class_lower);
    let type_words = clean_words(&type_lower);
    let aria_words = clean_words(&aria_lower);

    let mut total_score = 0.0f32;
    let mut matched_words_count = 0usize;

    for qw in &q_words {
        let weight = (qw.len() as f32).max(1.0);

        let check_attr = |attr_full: &str, attr_words: &[String], mult: f32| -> f32 {
            if attr_words.iter().any(|w| w == qw) {
                10.0 * mult
            } else if attr_full.contains(qw) {
                6.0 * mult
            } else if attr_words.iter().any(|w| (w.starts_with(qw) || qw.starts_with(w)) && qw.len() >= 3 && w.len() >= 3) {
                4.0 * mult
            } else {
                0.0
            }
        };

        let s_name = check_attr(&name_lower, &name_words, 3.2);
        let s_aria_label = check_attr(&aria_label_lower, &aria_label_words, 3.4);
        let s_aria_desc = check_attr(&aria_desc_lower, &aria_desc_words, 2.8);
        let s_aria_ind = check_attr(&aria_ind_lower, &aria_ind_words, 2.2);
        let s_help = check_attr(&help_lower, &help_words, 3.0);
        let s_id = check_attr(&id_lower, &id_words, 2.5);
        let s_val = check_attr(&val_lower, &val_words, 2.0);
        let s_type = check_attr(&type_lower, &type_words, 1.8);
        let s_class = check_attr(&class_lower, &class_words, 1.2);
        let s_aria = check_attr(&aria_lower, &aria_words, 2.2);
        let s_primary_class = check_attr(&elem.primary_class, &clean_words(&elem.primary_class), 2.2);
        let mut s_data = 0.0f32;
        for (k, v) in &elem.data_attributes {
            let v_words = clean_words(v);
            let s_val = check_attr(v, &v_words, 2.6);
            let k_words = clean_words(k);
            let s_k = check_attr(k, &k_words, 1.5);
            s_data = s_data.max(s_val).max(s_k);
        }

        let best_match = s_name
            .max(s_aria_label)
            .max(s_aria_desc)
            .max(s_aria_ind)
            .max(s_help)
            .max(s_id)
            .max(s_val)
            .max(s_type)
            .max(s_class)
            .max(s_primary_class)
            .max(s_data)
            .max(s_aria);
        if best_match > 0.0 {
            matched_words_count += 1;
            total_score += best_match * weight;
        }
    }

    if matched_words_count == 0 {
        return best_score;
    }

    if matched_words_count == q_words.len() {
        total_score += 50.0 * (q_words.len() as f32);
        if !name_words.is_empty() && name_words.len() == q_words.len() {
            total_score += 80.0;
        }
    } else {
        total_score *= (matched_words_count as f32) / (q_words.len() as f32);
    }

    if !name_words.is_empty() && name_words.len() > q_words.len() * 4 {
        let excess = (name_words.len() - q_words.len() * 4) as f32;
        total_score = (total_score - excess * 2.0).max(5.0);
    }

    total_score += ecom_bonus;

    Some(best_score.map_or(total_score, |s| s.max(total_score)))
}

#[cfg(windows)]
fn score_element(elem: &UiaElementInfo, query: &str) -> Option<f32> {
    let propositions = extract_target_propositions(query);
    let mut max_score: Option<f32> = None;

    for (idx, prop) in propositions.iter().enumerate() {
        if let Some(score) = score_single_proposition(elem, prop) {
            let line_priority_penalty = (idx as f32) * 1.5;
            let adjusted = (score - line_priority_penalty).max(1.0);
            max_score = Some(max_score.map_or(adjusted, |s| s.max(adjusted)));
        }
    }

    max_score
}

#[cfg(windows)]
fn find_best_element<'a>(elements: &'a [UiaElementInfo], target: &str) -> Option<&'a UiaElementInfo> {
    let target_words = clean_words(target);
    let is_target_short = target_words.len() <= 5 && target.len() <= 40;
    let is_chrome_target = is_browser_chrome_target(target);
    let has_web_content = elements.iter().any(|e| e.is_web_content);

    let mut scored: Vec<(f32, &'a UiaElementInfo)> = elements
        .iter()
        .filter_map(|elem| {
            let mut chrome_bias = 0.0f32;
            if has_web_content {
                if is_chrome_target {
                    if elem.is_browser_chrome {
                        chrome_bias += 50.0;
                    } else {
                        chrome_bias -= 40.0;
                    }
                } else if elem.is_browser_chrome {
                    chrome_bias -= 45.0;
                }
            }

            let word_count = elem.name.split_whitespace().count();
            if is_target_short && (word_count > 12 || elem.name.len() > 90) {
                return None;
            }

            score_element(elem, target).map(|score| {
                let mut final_score = score + chrome_bias;
                if elem.is_tab {
                    final_score += 80.0;
                } else if elem.is_link {
                    final_score += 40.0;
                } else if elem.is_tabindex {
                    final_score += 35.0;
                } else if elem.pattern.is_some() {
                    final_score += 20.0;
                }
                if elem.is_web_content {
                    final_score += 35.0;
                }
                if !elem.is_offscreen {
                    final_score += 20.0;
                }
                (final_score, elem)
            })
        })
        .collect();

    scored.sort_by(|(score_a, elem_a), (score_b, elem_b)| {
        if (score_a - score_b).abs() < 0.5 {
            elem_a.area.cmp(&elem_b.area)
        } else {
            score_b.partial_cmp(score_a).unwrap_or(std::cmp::Ordering::Equal)
        }
    });

    scored.first().map(|(_, elem)| *elem)
}

#[cfg(windows)]
fn find_best_button_element<'a>(elements: &'a [UiaElementInfo], target: &str) -> Option<&'a UiaElementInfo> {
    let is_chrome_target = is_browser_chrome_target(target);
    let has_web_content = elements.iter().any(|e| e.is_web_content);
    let mut scored: Vec<(f32, &'a UiaElementInfo)> = elements
        .iter()
        .filter_map(|elem| {
            // Un bouton ne peut pas être un paragraphe explicatif
            let word_count = elem.name.split_whitespace().count();
            if word_count > 15 || elem.name.len() > 120 {
                return None;
            }
            let mut chrome_bias = 0.0f32;
            if has_web_content {
                if is_chrome_target {
                    if elem.is_browser_chrome {
                        chrome_bias += 50.0;
                    } else {
                        chrome_bias -= 40.0;
                    }
                } else if elem.is_browser_chrome {
                    chrome_bias -= 45.0;
                }
            }

            score_element(elem, target).map(|score| {
                let mut final_score = score + chrome_bias;
                if elem.is_button || elem.is_tabindex {
                    final_score += 100.0;
                } else if elem.pattern.is_some() {
                    final_score += 40.0;
                } else {
                    final_score -= 60.0;
                }
                if elem.is_web_content {
                    final_score += 40.0;
                }
                if elem.area > 150_000 {
                    final_score -= 100.0;
                }
                (final_score, elem)
            })
        })
        .collect();

    scored.sort_by(|(score_a, elem_a), (score_b, elem_b)| {
        if (score_a - score_b).abs() < 0.5 {
            elem_a.area.cmp(&elem_b.area)
        } else {
            score_b.partial_cmp(score_a).unwrap_or(std::cmp::Ordering::Equal)
        }
    });

    scored.first().map(|(_, elem)| *elem)
}

#[cfg(windows)]
fn find_best_input_element<'a>(elements: &'a [UiaElementInfo], target: &str) -> Option<&'a UiaElementInfo> {
    let is_chrome_target = is_browser_chrome_target(target);
    let has_web_content = elements.iter().any(|e| e.is_web_content);
    let mut scored: Vec<(f32, &'a UiaElementInfo)> = elements
        .iter()
        .filter_map(|elem| {
            let mut chrome_bias = 0.0f32;
            if has_web_content {
                if is_chrome_target {
                    if elem.is_browser_chrome {
                        chrome_bias += 50.0;
                    } else {
                        chrome_bias -= 40.0;
                    }
                } else if elem.is_browser_chrome {
                    chrome_bias -= 45.0;
                }
            }

            score_element(elem, target).map(|score| {
                let mut final_score = score + chrome_bias;
                if elem.is_edit_or_textarea || elem.is_input {
                    final_score += 40.0;
                } else if elem.is_focusable {
                    final_score += 15.0;
                }
                if elem.is_web_content {
                    final_score += 50.0;
                }
                (final_score, elem)
            })
        })
        .collect();

    scored.sort_by(|(score_a, elem_a), (score_b, elem_b)| {
        if (score_a - score_b).abs() < 0.5 {
            elem_b.area.cmp(&elem_a.area)
        } else {
            score_b.partial_cmp(score_a).unwrap_or(std::cmp::Ordering::Equal)
        }
    });

    scored.first().map(|(_, elem)| *elem)
}

fn clean_diff_line(line: &str) -> String {
    let mut s = line.trim();
    loop {
        if let Some(rest) = s.strip_prefix("- ") {
            s = rest.trim();
        } else if let Some(rest) = s.strip_prefix("* ") {
            s = rest.trim();
        } else if let Some(rest) = s.strip_prefix("• ") {
            s = rest.trim();
        } else {
            break;
        }
    }
    while (s.starts_with("~~") && s.ends_with("~~")) || (s.starts_with("**") && s.ends_with("**")) {
        if let Some(inside) = s.strip_prefix("~~").and_then(|r| r.strip_suffix("~~")) {
            s = inside.trim();
        } else if let Some(inside) = s.strip_prefix("**").and_then(|r| r.strip_suffix("**")) {
            s = inside.trim();
        } else {
            break;
        }
    }
    s.trim().to_string()
}

pub fn compute_screen_diff(old_summary: &str, new_summary: &str) -> String {
    let is_ignored_diff_meta = |l: &str| -> bool {
        let t = l.trim();
        t.starts_with("===")
            || t.starts_with("---")
            || t.starts_with("####")
            || t.starts_with("Espace de travail")
            || t.starts_with("[Consigne prioritaire]")
            || t.starts_with("[Note d'intention")
            || t.starts_with("Fenêtre au premier plan")
    };

    let old_lines: Vec<String> = old_summary
        .lines()
        .filter(|l| !is_ignored_diff_meta(l))
        .map(clean_diff_line)
        .filter(|l| !l.is_empty())
        .collect();

    let new_lines: Vec<String> = new_summary
        .lines()
        .filter(|l| !is_ignored_diff_meta(l))
        .map(clean_diff_line)
        .filter(|l| !l.is_empty())
        .collect();

    let mut added = Vec::new();
    let mut removed = Vec::new();

    for line in &new_lines {
        if !old_lines.contains(line) && !added.contains(line) {
            added.push(line.clone());
        }
    }

    for line in &old_lines {
        if !new_lines.contains(line) && !removed.contains(line) {
            removed.push(line.clone());
        }
    }

    if added.is_empty() && removed.is_empty() {
        return String::new();
    }

    let mut diff = String::new();
    if !removed.is_empty() {
        diff.push_str("#### 🔻 Éléments disparus ou remplacés :\n");
        for r in removed {
            let clean_r = if r.contains('<') && r.contains('>') {
                crate::html_util::sanitize_html_to_markdown(&r)
            } else {
                r
            };
            diff.push_str(&format!("- ~~{}~~\n", clean_r.trim()));
        }
    }
    if !added.is_empty() {
        diff.push_str("\n#### 🟢 Nouveaux éléments / Données modifiées (Markdown) :\n");
        for a in added {
            let clean_a = if a.contains('<') && a.contains('>') {
                crate::html_util::sanitize_html_to_markdown(&a)
            } else {
                a
            };
            diff.push_str(&format!("- **{}**\n", clean_a.trim()));
        }
    }
    diff
}

pub fn generate_fallback_suggestions_from_screen(screen_summary: &str) -> Vec<QuickSuggestionItem> {
    let mut suggestions = Vec::new();
    let mut seen = std::collections::HashSet::new();

    if screen_summary.contains("[Défilement possible]") || screen_summary.to_lowercase().contains("défilement") {
        suggestions.push(QuickSuggestionItem::Text("Faire défiler vers le bas".to_string()));
        seen.insert("faire défiler vers le bas".to_string());
    }

    for line in screen_summary.lines() {
        let trimmed = line.trim();
        if trimmed.starts_with("[Liens / résultats cliquables] :")
            || trimmed.starts_with("[Boutons / contrôles cliquables] :")
            || trimmed.starts_with("[Onglets / pages ouvertes] :")
        {
            if let Some((_, items_part)) = trimmed.split_once(':') {
                for item in items_part.split('|').chain(items_part.split(',')) {
                    let clean = item.trim().trim_matches('"').trim();
                    let lower = clean.to_lowercase();
                    if clean.len() >= 3
                        && clean.len() <= 40
                        && !seen.contains(&lower)
                        && !lower.contains("fermer")
                        && !lower.contains("close")
                        && !lower.contains("annuler")
                        && !lower.contains("inconnue")
                    {
                        seen.insert(lower);
                        suggestions.push(QuickSuggestionItem::Text(clean.to_string()));
                        if suggestions.len() >= 4 {
                            return suggestions;
                        }
                    }
                }
            }
        }
    }

    if suggestions.is_empty() {
        suggestions.push(QuickSuggestionItem::Text("Faire défiler vers le bas".to_string()));
        suggestions.push(QuickSuggestionItem::Text("Actualiser la page".to_string()));
    }

    suggestions
}

#[cfg(windows)]
fn is_poor_or_redundant_link(text: &str) -> bool {
    let lower = text.trim().to_lowercase();
    if lower.len() < 2 {
        return true;
    }
    const BLACKLIST: &[&str] = &[

    ];
    BLACKLIST.iter().any(|&b| lower == b)
}

#[cfg(windows)]
fn score_enriched_link(text: &str) -> f32 {
    let clean = text.trim();
    let char_count = clean.chars().count();
    let lower = clean.to_lowercase();

    let mut score = 10.0f32;

    if (18..=95).contains(&char_count) {
        score += 15.0;
    } else if char_count < 10 {
        score -= 8.0;
    } else if char_count > 120 {
        score -= 5.0;
    }

    if clean.contains("[aria:") {
        score += 25.0;
    }
    if clean.contains("[desc:") {
        score += 15.0;
    }
    if clean.contains("[data:") || clean.contains("testid:") {
        score += 20.0;
    }

    const TECH_KEYWORDS: &[&str] = &[
        "go", "gb", "to", "tb", "ram", "pro", "max", "ultra", "plus", "lite",
        "mini", "ghz", "core", "ssd", "oled", "led", "4k", "5g", "wifi",
        "intel", "amd", "ryzen", "rtx", "gtx", "apple", "samsung", "asus",
        "sony", "dell", "hp", "lenovo",
    ];
    for &kw in TECH_KEYWORDS {
        if lower.split(|c: char| !c.is_alphanumeric()).any(|w| w == kw) {
            score += 8.0;
        }
    }

    if clean.contains('€') || clean.contains('$') || lower.contains("eur") || lower.contains("usd") {
        score += 20.0;
    }

    if clean.contains('★') || clean.contains('☆') || lower.contains("avis") || lower.contains("étoile") || lower.contains("etoile") {
        score += 15.0;
    }

    score
}

#[cfg(windows)]
fn score_enriched_button(text: &str) -> f32 {
    let clean = text.trim();
    let lower = clean.to_lowercase();
    let mut score = 10.0f32;

    // Boutons cruciaux de parcours utilisateur (panier, validation, checkout, soumission)
    const HIGH_VALUE_BUTTONS: &[&str] = &[
        "panier", "cart", "commander", "commande", "valider", "passer la commande",
        "voir le panier", "payer", "checkout", "finaliser", "confirmer", "continuer",
        "suivant", "ajouter", "soumettre", "submit", "acheter", "terminer", "accepter",
    ];

    for &kw in HIGH_VALUE_BUTTONS {
        if lower.contains(kw) {
            score += 60.0;
            break;
        }
    }

    if clean.contains('€') || clean.contains('$') || lower.contains("eur") || lower.contains("usd") {
        score += 30.0;
    }

    if clean.contains("[aria:") {
        score += 15.0;
    }
    if clean.contains("[desc:") {
        score += 15.0;
    }
    if clean.contains("[data:") || clean.contains("testid:") || clean.contains("action:") {
        score += 20.0;
    }

    let words = clean.split_whitespace().count();
    if (1..=4).contains(&words) {
        score += 10.0;
    } else if words > 8 {
        score -= 15.0;
    }

    score
}

#[cfg(windows)]
pub fn summarize_screen_state(target_window: Option<&str>, _autoscroll: bool) -> String {
    let work_area = get_desktop_work_area();
    let wa_x = work_area.left;
    let wa_y = work_area.top;
    let wa_w = work_area.right - work_area.left;
    let wa_h = work_area.bottom - work_area.top;

    let user_windows = list_user_windows();
    let raw_fg = unsafe { GetForegroundWindow() };    let current_pid = std::process::id();    let (raw_fg_pid, _) = get_window_process_info(raw_fg);    let is_libertide_or_unknown = raw_fg.0.is_null()
        || raw_fg_pid == current_pid
        || !user_windows.iter().any(|(h, _)| *h == raw_fg);

    let active_hwnd = if !is_libertide_or_unknown && !is_window_hung(raw_fg) {
        raw_fg
    } else {
        LAST_EXTERNAL_HWND
            .lock()
            .ok()
            .and_then(|g| *g)
            .map(|raw| HWND(raw as _))
            .filter(|&h| user_windows.iter().any(|(w, _)| *w == h) && !is_window_hung(h))
            .or_else(|| get_active_or_best_window(&user_windows))
            .or_else(|| find_best_browser_window(&user_windows, None))
            .or_else(|| user_windows.first().map(|(h, _)| *h))
            .unwrap_or(raw_fg)
    };

    let prev_screen_snapshot = LAST_SCREEN_SUMMARY.lock().ok().and_then(|g| g.clone());

    let fg_title = user_windows
        .iter()
        .find(|(h, _)| *h == active_hwnd)
        .map(|(_, t)| t.as_str())
        .unwrap_or("Application active");

    let mut inspect_hwnds = Vec::new();

    if let Some(target) = target_window.filter(|t| !t.trim().is_empty()) {
        let matched = find_windows_matching(target, &user_windows, Some(active_hwnd));
        if let Some(&h) = matched.first() {
            inspect_hwnds.push(h);
        }
    }

    if inspect_hwnds.is_empty() {
        inspect_hwnds.push(active_hwnd);
        for (h, _) in &user_windows {
            if !inspect_hwnds.contains(h) && inspect_hwnds.len() < 3 {
                inspect_hwnds.push(*h);
            }
        }
    }

    // Inclusion systématique des notifications Windows détectées pour inspection UIA
    for (h, title) in &user_windows {
        if title.to_lowercase().contains("notification") || title.to_lowercase().contains("toast") {
            if !inspect_hwnds.contains(h) {
                inspect_hwnds.push(*h);
            }
        }
    }

    // 2. Priorisation : l'arbre UIA de la fenêtre active est généré en premier
    let mut active_window_content = String::new();
    active_window_content.push_str("=== Contenu détaillé de la fenêtre active ===\n");
    for hwnd in &inspect_hwnds {
        let win_title = user_windows
            .iter()
            .find(|(h, _)| *h == *hwnd)
            .map(|(_, t)| t.as_str())
            .unwrap_or("Fenêtre");

        let is_active = *hwnd == active_hwnd;
        if is_active {
            active_window_content.push_str(&format!("\n--- Fenêtre : \"{}\" [AU PREMIER PLAN - CONTEXTE ACTIF PRIORITAIRE] ---\n", win_title));
        } else {
            active_window_content.push_str(&format!("\n--- Fenêtre secondaire : \"{}\" ---\n", win_title));
        }
        if is_window_hung(*hwnd) {
            active_window_content.push_str("  (Application figée / ne répond pas - UIA ignoré)\n");
            continue;
        }

        let initial_elements = list_interactive_elements(*hwnd);
        let is_browser = is_browser_hwnd(*hwnd, win_title);
        let (_pid, exe_name) = get_window_process_info(*hwnd);
        let is_editor = is_editor_window(*hwnd, win_title, &exe_name);
        let detected_file = extract_filename_from_title(win_title);
        let is_terminal = is_terminal_window(*hwnd, win_title);

        // Extraction des viewports défilables et des conteneurs dont la scrollbar est à son maximum
        unsafe {
            if let Ok(uia) = CoCreateInstance::<_, IUIAutomation>(&CUIAutomation, None, CLSCTX_INPROC_SERVER) {
                let viewports_text = extract_scrollable_viewports_content(&uia, *hwnd);
                if !viewports_text.trim().is_empty() {
                    active_window_content.push_str(&format!("\n=== Viewports et zones défilables (Scrollbar max / conteneurs actifs) ===\n{}", viewports_text));
                }
            }
        }

        // Extraction du corps textuel natif du Document / RootWebArea via IUIAutomationTextPattern
        if is_browser {
            unsafe {
                if let Ok(uia) = CoCreateInstance::<_, IUIAutomation>(&CUIAutomation, None, CLSCTX_INPROC_SERVER) {
                    if let Ok(win_elem) = uia.ElementFromHandle(*hwnd) {
                        if let Ok(cond_doc) = uia.CreatePropertyCondition(
                            windows::Win32::UI::Accessibility::UIA_ControlTypePropertyId,
                            &windows::core::VARIANT::from(UIA_DocumentControlTypeId.0),
                        ) {
                            let mut best_text = String::new();
                            if let Ok(doc_elements) = win_elem.FindAll(TreeScope_Descendants, &cond_doc) {
                                let doc_count = doc_elements.Length().unwrap_or(0).min(6);
                                for di in 0..doc_count {
                                    if let Ok(doc_elem) = doc_elements.GetElement(di) {
                                        if let Some(doc_text) = extract_web_document_text(&doc_elem, 15000) {
                                            if doc_text.len() > best_text.len() {
                                                best_text = doc_text;
                                            }
                                        }
                                    }
                                }
                            }
                            if !best_text.is_empty() {
                                let clean_text = if best_text.contains('<') && best_text.contains('>') {
                                    crate::html_util::sanitize_html_to_markdown(&best_text)
                                } else {
                                    best_text
                                };
                                let filtered_lines: Vec<&str> = clean_text
                                    .lines()
                                    .map(str::trim)
                                    .filter(|l| !l.is_empty() && l.len() >= 2)
                                    .take(100)
                                    .collect();
                                if !filtered_lines.is_empty() {
                                    active_window_content.push_str(&format!(
                                        "  [Page web - Corps textuel et résultats rendus] :\n```\n{}\n```\n",
                                        filtered_lines.join("\n")
                                    ));
                                }
                            }
                        }
                    }
                }
            }
        }

        if is_terminal {
            if let Some(term_text) = read_terminal_window_content(*hwnd, win_title) {
                let preview = truncate_with_notice(&term_text, 2200);
                active_window_content.push_str(&format!("  [Terminal '{}' - Sortie console active] :\n```\n{}\n```\n", win_title, preview.trim()));
            }
        }
        let has_scrollbar = initial_elements.iter().any(|e| {
            e.class_name.contains("scrollbar")
                || e.localized_type.contains("scrollbar")
                || e.localized_type.contains("défilement")
        });

        let elements = initial_elements;
        if elements.is_empty() {
            if !is_browser && (is_editor || detected_file.is_some()) {
                if let Some(ref fname) = detected_file {
                    if let Some((fpath, _)) = find_file_in_workspace(fname) {
                        active_window_content.push_str(&format!("  [Fichier actif dans l'éditeur] : \"{}\" (chemin: {})\n", fname, fpath));
                    } else {
                        active_window_content.push_str(&format!("  [Fichier actif détecté d'après le titre] : \"{}\"\n", fname));
                    }
                } else {
                    active_window_content.push_str(&format!("  [Éditeur de code actif] : \"{}\"\n", win_title));
                }
            } else if is_terminal {
                // Déjà renseigné plus haut via read_terminal_window_content
            } else {
                active_window_content.push_str("  (Aucun élément UI interactif accessible)\n");
            }
            continue;
        }

        let has_web_doc = elements.iter().any(|e| e.is_web_content);

        if !is_browser && (is_editor || detected_file.is_some()) {
            if let Some(ref fname) = detected_file {
                if let Some((fpath, _)) = find_file_in_workspace(fname) {
                    active_window_content.push_str(&format!("  [Fichier actif dans l'éditeur] : \"{}\" (chemin: {})\n", fname, fpath));
                } else {
                    active_window_content.push_str(&format!("  [Fichier actif détecté d'après le titre] : \"{}\"\n", fname));
                }
            }
        }

        // A. Champs de formulaire et barres de recherche
        let edits: Vec<&UiaElementInfo> = elements.iter()
            .filter(|e| (!has_web_doc || e.is_web_content) && (e.is_edit_or_textarea || e.is_input))
            .collect();
        if !edits.is_empty() {
            let prefix = if has_web_doc { "  [Page web - champs de saisie / recherche] :\n" } else { "  [Champs de texte / saisie] :\n" };
            active_window_content.push_str(prefix);
            for edit in edits.iter().take(8) {
                let mut label = if !edit.name.is_empty() {
                    edit.name.clone()
                } else if !edit.primary_class.is_empty() {
                    format!(".{}", edit.primary_class)
                } else {
                    "Champ".to_string()
                };
                if !edit.aria_indications.is_empty() {
                    label.push_str(&format!(" ({})", edit.aria_indications));
                }
                if !edit.aria_description.is_empty() && edit.aria_description.len() <= 60 {
                    label.push_str(&format!(" [info: {}]", edit.aria_description));
                }
                if !edit.data_attributes.is_empty() {
                    let data_summary = edit.data_attributes.iter()
                        .take(2)
                        .map(|(k, v)| format!("{k}={v}"))
                        .collect::<Vec<_>>()
                        .join(", ");
                    label.push_str(&format!(" [data: {data_summary}]"));
                }
                let val = if !edit.value_text.is_empty() {
                    format!(" = \"{}\"", edit.value_text.chars().take(120).collect::<String>())
                } else {
                    String::new()
                };
                active_window_content.push_str(&format!("  • {}{}\n", label, val));
            }
        }

        // B. Titres et en-têtes de sections
        let mut headings: Vec<String> = elements
            .iter()
            .filter(|e| {
                (!has_web_doc || e.is_web_content)
                    && !e.name.is_empty()
                    && e.name.len() >= 2
                    && (e.localized_type.contains("en-tête")
                        || e.localized_type.contains("heading")
                        || e.aria_role.contains("heading")
                        || e.class_name.contains("heading")
                        || e.class_name.contains("title"))
            })
            .map(|e| e.name.clone())
            .collect();
        headings.dedup();
        if !headings.is_empty() {
            let display_headings: Vec<String> = headings.into_iter().take(15).collect();
            let prefix = if has_web_doc { "  [Page web - titres / en-têtes] : " } else { "  [Titres / en-têtes] : " };
            active_window_content.push_str(&format!("{}{}\n", prefix, display_headings.join(" | ")));
        }

        // C. Textes informatifs, contacts, e-mails, téléphones, météo, données et prix
        let mut text_items: Vec<String> = elements
            .iter()
            .filter(|e| {
                (!has_web_doc || e.is_web_content)
                    && !e.is_edit_or_textarea
                    && !e.is_button
                    && !e.name.is_empty()
                    && e.name.len() >= 2
                    && !e.is_link
                    && !e.aria_role.contains("link")
                    && !e.localized_type.contains("lien")
                    && !e.localized_type.contains("link")
                    && (e.name.contains('@')
                        || e.name.chars().filter(|c| c.is_ascii_digit()).count() >= 5
                        || e.localized_type.contains("texte")
                        || e.localized_type.contains("text")
                        || e.localized_type.contains("en-tête")
                        || e.localized_type.contains("heading")
                        || e.name.contains('°')
                        || e.name.contains('€')
                        || e.name.contains('$')
                        || e.name.contains('%')
                        || e.name.to_lowercase().contains("contact")
                        || e.name.to_lowercase().contains("email")
                        || e.name.to_lowercase().contains("mail")
                        || e.name.to_lowercase().contains("tél")
                        || e.name.to_lowercase().contains("tel")
                        || e.name.to_lowercase().contains("adresse")
                        || e.name.to_lowercase().contains("météo")
                        || e.name.to_lowercase().contains("meteo")
                        || e.name.to_lowercase().contains("km/h")
                        || e.name.to_lowercase().contains("humidité")
                        || e.name.to_lowercase().contains("humidite")
                        || e.name.to_lowercase().contains("vent")
                        || e.name.to_lowercase().contains("pluie")
                        || e.name.to_lowercase().contains("soleil")
                        || e.name.to_lowercase().contains("nuag")
                        || e.name.to_lowercase().contains("orage")
                        || e.name.to_lowercase().contains("eur")
                        || e.name.to_lowercase().contains("usd"))
            })
            .map(|e| e.name.clone())
            .collect();
        text_items.dedup();
        if !text_items.is_empty() {
            let display_texts: Vec<String> = text_items.into_iter().take(80).collect();
            let prefix = if has_web_doc { "  [Page web - encarts météo / données / prix observés] : " } else { "  [Textes / données / prix observés] : " };
            active_window_content.push_str(&format!("{}{}\n", prefix, display_texts.join(" | ")));
        }

        // 2. PRIMAUTÉ DU CONTENU HTML / WEB : Boutons cliquables d'action
        let buttons: Vec<(bool, String)> = elements
            .iter()
            .filter(|e| {
                (!has_web_doc || e.is_web_content)
                    && !e.is_edit_or_textarea
                    && (!e.name.is_empty() || !e.help_text.is_empty() || !e.automation_id.is_empty() || !e.primary_class.is_empty() || !e.data_attributes.is_empty())
                    && (e.is_button
                        || e.is_tabindex
                        || e.pattern.is_some()
                        || e.localized_type.contains("bouton")
                        || e.localized_type.contains("button")
                        || e.automation_id.to_lowercase().contains("close")
                        || e.name.to_lowercase().contains("fermer")
                        || e.name.to_lowercase().contains("close"))
                    && e.name.len() <= 120
                    && e.name.split_whitespace().count() <= 15
                    && !e.name.eq_ignore_ascii_case("système")
                    && !e.name.eq_ignore_ascii_case("system")
            })
            .map(|e| {
                let mut base = if !e.name.is_empty() {
                    e.name.clone()
                } else if !e.help_text.is_empty() {
                    e.help_text.clone()
                } else if !e.automation_id.is_empty() {
                    e.automation_id.clone()
                } else if !e.primary_class.is_empty() {
                    format!(".{}", e.primary_class)
                } else {
                    String::new()
                };
                if !e.aria_indications.is_empty() {
                    base = format!("{base} ({})", e.aria_indications);
                }
                if !e.aria_description.is_empty() && e.aria_description.len() <= 50 {
                    base.push_str(&format!(" [desc: {}]", e.aria_description));
                }
                if let Some((k, v)) = e.data_attributes.iter().find(|(k, _)| k.contains("test") || k.contains("action") || k.contains("target")) {
                    base.push_str(&format!(" [{k}: {v}]"));
                }
                (e.is_offscreen, base)
            })
            .collect();

        let mut scored_buttons: Vec<(f32, String)> = buttons.into_iter().map(|(is_off, b)| {
            let base_score = score_enriched_button(&b);
            let score = if is_off { base_score - 10.0 } else { base_score + 15.0 };
            (score, b)
        }).collect();
        scored_buttons.sort_by(|a, b| b.0.partial_cmp(&a.0).unwrap_or(std::cmp::Ordering::Equal));
        let mut display_btns: Vec<String> = scored_buttons.into_iter().map(|(_, b)| b).collect();
        display_btns.dedup();
        if !display_btns.is_empty() {
            display_btns.truncate(40);
            let prefix = if has_web_doc { "  [Page web - boutons cliquables] : " } else { "  [Boutons / contrôles cliquables] : " };
            active_window_content.push_str(&format!("{}{}\n", prefix, display_btns.join(", ")));
        }

        // 3. PRIMAUTÉ DU CONTENU HTML / WEB : Liens et résultats
        let raw_links: Vec<(bool, String)> = elements
            .iter()
            .filter(|e| {
                (!has_web_doc || e.is_web_content)
                    && !e.is_edit_or_textarea
                    && (!e.name.is_empty() || !e.primary_class.is_empty() || !e.data_attributes.is_empty())
                    && (e.is_link
                        || e.localized_type.contains("lien")
                        || e.localized_type.contains("link")
                        || e.localized_type.contains("hyperlink")
                        || e.aria_role.contains("link")
                        || e.aria_role.contains("lien"))
            })
            .map(|e| {
                let mut label = if !e.name.is_empty() {
                    e.name.clone()
                } else if !e.primary_class.is_empty() {
                    format!(".{}", e.primary_class)
                } else {
                    "Lien".to_string()
                };
                if !e.aria_indications.is_empty() {
                    label = format!("{} ({})", label, e.aria_indications);
                }
                if !e.aria_description.is_empty() && e.aria_description.len() <= 50 {
                    label.push_str(&format!(" [desc: {}]", e.aria_description));
                }
                if let Some((k, v)) = e.data_attributes.iter().find(|(k, _)| k.contains("test") || k.contains("target")) {
                    label.push_str(&format!(" [{k}: {v}]"));
                }
                (e.is_offscreen, label)
            })
            .collect();

        let mut seen_links = std::collections::HashSet::new();
        let mut scored_links: Vec<(f32, String)> = raw_links
            .into_iter()
            .filter(|(_, l)| !is_poor_or_redundant_link(l))
            .filter(|(_, l)| {
                let lower = l.trim().to_lowercase();
                if seen_links.contains(&lower) {
                    false
                } else {
                    seen_links.insert(lower);
                    true
                }
            })
            .map(|(is_off, l)| {
                let base_score = score_enriched_link(&l);
                let score = if is_off { base_score - 10.0 } else { base_score + 15.0 };
                (score, l)
            })
            .collect();

        scored_links.sort_by(|a, b| b.0.partial_cmp(&a.0).unwrap_or(std::cmp::Ordering::Equal));
        let display_links: Vec<String> = scored_links.into_iter().take(40).map(|(_, l)| l).collect();

        if !display_links.is_empty() {
            let prefix = if has_web_doc { "  [Page web - liens cliquables] : " } else { "  [Liens / résultats cliquables] : " };
            active_window_content.push_str(&format!("{}{}\n", prefix, display_links.join(" | ")));
        }

        // 5. Informations secondaires de la fenêtre hôte (Onglets et contrôles navigateur)
        let mut tabs: Vec<String> = elements
            .iter()
            .filter(|e| !e.name.is_empty() && (e.is_tab || e.localized_type.contains("onglet") || e.localized_type.contains("tab")))
            .map(|e| e.name.clone())
            .collect();
        tabs.dedup();
        if !tabs.is_empty() {
            let display_tabs: Vec<String> = tabs.into_iter().take(10).collect();
            active_window_content.push_str(&format!("  [Onglets / pages ouvertes] : {}\n", display_tabs.join(" | ")));
        }

        if has_web_doc {
            let chrome_controls: Vec<String> = elements
                .iter()
                .filter(|e| e.is_browser_chrome && (!e.name.is_empty() || !e.value_text.is_empty()))
                .map(|e| {
                    if !e.value_text.is_empty() {
                        format!("{}: \"{}\"", e.name, e.value_text.chars().take(70).collect::<String>())
                    } else {
                        e.name.clone()
                    }
                })
                .take(4)
                .collect();
            if !chrome_controls.is_empty() {
                active_window_content.push_str(&format!("  [Interface navigateur] : {}\n", chrome_controls.join(" | ")));
            }
        }

        if has_scrollbar {
            active_window_content.push_str("  [Défilement possible] : Une barre de défilement est présente. Utilise l'action 'scroll' ('down'/'up') pour révéler la suite de la page.\n");
        }
    }

    // 3. Relégation et compactage strict des autres fenêtres (8 fenêtres max pour préserver les tokens)
    let other_windows: Vec<String> = user_windows
        .iter()
        .filter(|(h, _)| !inspect_hwnds.contains(h))
        .take(8)
        .map(|(hwnd, title)| {
            let (pid, exe_name) = get_window_process_info(*hwnd);
            let is_hung = is_window_hung(*hwnd);
            let is_minimized = unsafe { IsIconic(*hwnd).as_bool() };
            let state = if is_hung {
                " [ne répond pas]"
            } else if is_minimized {
                " [réduite]"
            } else {
                ""
            };
            format!("- \"{}\" ({}, PID: {}){}", title, exe_name, pid, state)
        })
        .collect();

    let mut other_windows_content = String::new();
    other_windows_content.push_str("\n=== Autres fenêtres ouvertes ===\n");
    if other_windows.is_empty() {
        other_windows_content.push_str("(Aucune autre fenêtre ouverte)\n");
    } else {
        other_windows_content.push_str(&other_windows.join("\n"));
        other_windows_content.push('\n');
    }

    // 4. Détection et extraction prioritaire du différentiel des changements sur la page web
    let mut web_changes_section = String::new();
    if let Some(ref prev_summary) = prev_screen_snapshot {
        let diff = compute_screen_diff(prev_summary, &active_window_content);
        if !diff.trim().is_empty() {
            web_changes_section.push_str("=== INFORMATIONS WEB MODIFIÉES / NOUVELLES (PRIORITÉ ABSOLUE) ===\n");
            web_changes_section.push_str("[Consigne prioritaire] : Les éléments et données ci-dessous viennent d'apparaître ou d'être modifiés sur la page web active suite à la dernière action. Ils priment sur le reste de la page et sur les autres fenêtres :\n");
            web_changes_section.push_str(diff.trim());
            web_changes_section.push_str("\n\n");
        }
    }

    let mut out = String::new();
    out.push_str(&format!(
        "=== Rapport d'analyse de l'écran ===\nEspace de travail : {}x{}\n",
        wa_w, wa_h
    ));
    out.push_str(&format!(
        "Fenêtre au premier plan (contexte actif) : \"{}\"\n[Note d'intention contextuelle] : Cette fenêtre est le contexte direct de l'utilisateur. Toute instruction relative à une question, recherche, météo, achat ou formulaire doit être résolue prioritairement dans cette fenêtre active.\n\n",
        fg_title
    ));

    // Injection au tout début pour garantir la primauté de tokens
    if !web_changes_section.is_empty() {
        out.push_str(&web_changes_section);
    }
    out.push_str(&active_window_content);
    out.push_str(&other_windows_content);

    // 3. Quota interne limité strictement à 15 Ko (15 360 caractères)
    let summary = truncate_with_notice(&out, 15360);
    println!("\n=================== [Analyse d'écran UIA] ===================");
    println!("{}", summary.trim());
    println!("============================================================\n");
    summary
}

#[cfg(not(windows))]
pub fn summarize_screen_state(target_window: Option<&str>, _autoscroll: bool) -> String {
    let user_windows = list_user_windows();
    let active_id = get_active_x11_window();
    let prev_screen_snapshot = LAST_SCREEN_SUMMARY.lock().ok().and_then(|g| g.clone());

    let mut out = String::new();
    out.push_str("=== Rapport d'analyse de l'écran (Serveur X11 Unix) ===\n");

    if let Some(ref act) = active_id {
        let active_title = user_windows
            .iter()
            .find(|(id, _)| id.contains(act) || act.contains(id))
            .map(|(_, t)| t.as_str())
            .unwrap_or("Fenêtre active");
        out.push_str(&format!("Fenêtre au premier plan : \"{}\" [ID: {}]\n\n", active_title, act));
    }

    let mut x11_active_text = String::new();

    out.push_str("=== Fenêtres X11 détectées ===\n");
    for (win_id, title) in &user_windows {
        let (wm_class, pid) = get_x11_window_properties(win_id);
        let is_term = is_terminal_window(win_id, title);
        let is_browser = is_browser_window(title);
        let kind = if is_term {
            " [Terminal]"
        } else if is_browser {
            " [Navigateur web]"
        } else {
            ""
        };

        out.push_str(&format!(
            "- Fenêtre \"{}\" (ID: {}, Classe: {}, PID: {}){}\n",
            title, win_id, wm_class, pid, kind
        ));

        // Si c'est la fenêtre ciblée ou la fenêtre active, essayer de capter son texte
        let is_target = target_window.map_or(false, |tw| title.to_lowercase().contains(&tw.to_lowercase()));
        let is_current = active_id.as_deref().map_or(false, |act| win_id.contains(act) || act.contains(win_id));

        if is_target || is_current {
            // Tenter de lire le contenu sélectionné (Primary selection X11)
            if let Ok(output) = std::process::Command::new("xclip")
                .args(["-selection", "primary", "-o"])
                .output()
            {
                let sel = String::from_utf8_lossy(&output.stdout).trim().to_string();
                if !sel.is_empty() {
                    x11_active_text.push_str(&sel);
                    out.push_str(&format!(
                        "  [Sélection texte active (X11 Primary)] :\n\"\"\"\n{}\n\"\"\"\n",
                        truncate_with_notice(&sel, 800)
                    ));
                }
            }
        }
    }

    if let Some(clip) = clipboard::get_text() {
        let clip_trim = clip.trim();
        if !clip_trim.is_empty() {
            out.push_str(&format!("\n[Contenu presse-papiers X11 (Clipboard)] :\n\"\"\"\n{}\n\"\"\"\n", truncate_with_notice(clip_trim, 600)));
        }
    }

    if let Some(ref prev) = prev_screen_snapshot {
        let diff = compute_screen_diff(prev, &out);
        if !diff.trim().is_empty() {
            let changes_hdr = format!("=== CHANGEMENTS RÉCENTS (PRIORITÉ ABSOLUE) ===\n{}\n\n", diff.trim());
            out = format!("{}{}", changes_hdr, out);
        }
    }

    truncate_with_notice(&out, 12000)
}

#[cfg(windows)]
pub fn execute_system_actions(actions: &[AgentAction], request_id: u64) -> String {
    if actions.is_empty() {
        println!("[Actions] Aucune action système à exécuter.");
        return "Aucune action système à exécuter.".to_string();
    }
    if is_request_cancelled(request_id) {
        println!("[Actions] Exécution annulée avant le début des actions (nouvelle consigne reçue).");
        return "Actions interrompues par une nouvelle consigne.".to_string();
    }

    let mut feedback = Vec::new();
    println!("[Actions] Exécution de {} action(s) système...", actions.len());

    let mut newly_spawned_hwnd: Option<HWND> = None;

    for action in actions {
        if is_request_cancelled(request_id) {
            return "Actions interrompues par une nouvelle consigne.".to_string();
        }
        if let AgentAction::OpenApp { name } = action {
            let current_windows = list_user_windows();
            let matched_hwnds = find_windows_matching(name, &current_windows, None);
            if let Some(&hwnd) = matched_hwnds.first() {
                force_foreground_window(hwnd);
                unsafe {
                    let _ = ShowWindow(hwnd, SW_RESTORE);
                }
                println!("[Actions] Fenêtre déjà existante pour '{}' [HWND {:?}], restaurée et placée au premier plan.", name, hwnd.0);
                newly_spawned_hwnd = Some(hwnd);
                feedback.push(format!("Application '{}' déjà ouverte : fenêtre restaurée et placée au premier plan.", name));
                continue;
            }

            let initial_hwnds: std::collections::HashSet<isize> = current_windows
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

            if !launched {
                let search_url = format!("https://www.google.com/search?q={}", urlencoding_simple(name));
                println!("[Actions] Exécutable introuvable, ouverture de la recherche : {}", search_url);
                let browser_hwnd = launch_browser_new_window(&search_url);
                newly_spawned_hwnd = browser_hwnd;
                feedback.push(format!("Application non trouvée localement ; recherche web lancée pour '{}'.", name));
            } else {
                feedback.push(format!("Application '{}' lancée.", name));
            }

            for _ in 0..25 {
                if is_request_cancelled(request_id) {
                    return "Lancement d'application interrompu par une nouvelle consigne.".to_string();
                }
                std::thread::sleep(Duration::from_millis(100));
                let current_windows = list_user_windows();
                for (hwnd, _title) in &current_windows {
                    if !initial_hwnds.contains(&(hwnd.0 as isize)) {
                        force_foreground_window(*hwnd);
                        unsafe {
                            let _ = ShowWindow(*hwnd, SW_MAXIMIZE);
                        }
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
        if is_request_cancelled(request_id) {
            return "Actions interrompues par une nouvelle consigne.".to_string();
        }
        if let AgentAction::OpenBrowser { url } = action {
            let raw_url = url.as_deref().unwrap_or("");
            let target = format_url_for_navigation(raw_url);

            let spawned = launch_browser_new_window(&target);
            if spawned.is_some() {
                newly_spawned_hwnd = spawned;
            }
        }
    }

    let work_area = get_desktop_work_area();
    let wa_x = work_area.left;
    let wa_y = work_area.top;
    let wa_w = (work_area.right - work_area.left).max(800);
    let wa_h = (work_area.bottom - work_area.top).max(600);

    let mut user_windows = list_user_windows();
    let mut active_user_hwnd = get_active_or_best_window(&user_windows);
    let mut preferred_target_hwnd = newly_spawned_hwnd.or(active_user_hwnd);

    for (idx, action) in actions.iter().enumerate() {
        if is_request_cancelled(request_id) {
            println!("[Actions] Interruption immédiate de la séquence d'actions (nouvelle consigne reçue).");
            feedback.push("Séquence d'actions interrompue par une nouvelle consigne.".to_string());
            break;
        }

        // Réactualisation dynamique du premier plan réel entre chaque action
        let fg = unsafe { GetForegroundWindow() };
        let overlay_hwnd = unsafe {
            FindWindowW(None, w!("Libertide overlay")).unwrap_or(HWND(std::ptr::null_mut()))
        };
        let current_pid = std::process::id();
        let (fg_pid, _) = get_window_process_info(fg);
        if !fg.0.is_null() && fg != overlay_hwnd && fg_pid != current_pid && !is_window_hung(fg) {
            active_user_hwnd = Some(fg);
        }

        println!("[Actions] [{}/{}] Action en cours : {:?}", idx + 1, actions.len(), action);
        match action {
            AgentAction::OpenApp { .. } => {}
            AgentAction::CloseApp { name } => {
                feedback.push(format!("Demande de fermeture de l'application ou fenêtre '{}'.", name));
                let kw = name.trim();
                if !kw.is_empty() {
                    let targets = find_windows_matching(kw, &user_windows, None);
                    if let Some(&hwnd) = targets.first() {
                        println!("[Actions] Fermeture de la fenêtre [HWND {:?}]", hwnd.0);
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
                        println!("[Actions] Arrêt du processus {}", proc_name);
                        let _ = std::process::Command::new("taskkill")
                            .args(["/IM", &proc_name])
                            .spawn();
                    }
                }
            }
            AgentAction::FocusWindow { title, pid } => {
                let mut found_hwnd = None;
                if let Some(target_pid) = pid {
                    for &(h, _) in &user_windows {
                        let (w_pid, _) = get_window_process_info(h);
                        if w_pid == *target_pid {
                            found_hwnd = Some(h);
                            break;
                        }
                    }
                }
                if found_hwnd.is_none() {
                    if let Some(t) = title.as_deref().filter(|s| !s.trim().is_empty()) {
                        found_hwnd = find_windows_matching(t, &user_windows, active_user_hwnd).first().copied();
                    }
                }
                if let Some(hwnd) = found_hwnd {
                    if is_window_hung(hwnd) {
                        feedback.push(format!("La fenêtre [HWND {:?}] ne répond pas (sablier / bloquée). Focus annulé.", hwnd.0));
                    } else {
                        let win_title = user_windows
                            .iter()
                            .find(|(h, _)| *h == hwnd)
                            .map(|(_, t)| t.as_str())
                            .unwrap_or("");
                        let is_browser = is_browser_hwnd(hwnd, win_title);
                        force_foreground_window(hwnd);
                        unsafe {
                            let _ = ShowWindow(hwnd, if is_browser { SW_MAXIMIZE } else { SW_RESTORE });
                        }
                        if is_browser {
                            focus_browser_address_bar(hwnd);
                        }
                        active_user_hwnd = Some(hwnd);
                        preferred_target_hwnd = Some(hwnd);
                        feedback.push(format!("Fenêtre passée au premier plan [HWND {:?}].", hwnd.0));
                    }
                } else {
                    feedback.push("Fenêtre introuvable pour focus.".to_string());
                }
            }
            AgentAction::FocusAddressBar { window } => {
                let target_hwnd = match window.as_deref() {
                    Some(w) if !w.trim().is_empty() => {
                        find_windows_matching(w, &user_windows, active_user_hwnd).first().copied()
                    }
                    _ => find_best_browser_window(&user_windows, active_user_hwnd),
                };

                if let Some(hwnd) = target_hwnd {
                    if is_window_hung(hwnd) {
                        feedback.push(format!("Navigateur [HWND {:?}] non réactif. Focus barre d'adresse ignoré.", hwnd.0));
                        continue;
                    }
                    force_foreground_window(hwnd);
                    unsafe {
                        let _ = ShowWindow(hwnd, SW_MAXIMIZE);
                    }
                    std::thread::sleep(Duration::from_millis(60));
                    focus_browser_address_bar(hwnd);
                    active_user_hwnd = Some(hwnd);
                    preferred_target_hwnd = Some(hwnd);
                    feedback.push("Barre d'adresse sélectionnée via focus_address_bar (Alt+D / Ctrl+L).".to_string());
                } else {
                    println!("[Actions] Aucun navigateur trouvé pour focus_address_bar, ouverture d'un navigateur.");
                    let spawned = launch_browser_new_window("https://www.google.com");
                    if let Some(h) = spawned {
                        newly_spawned_hwnd = Some(h);
                        active_user_hwnd = Some(h);
                        preferred_target_hwnd = Some(h);
                        user_windows = list_user_windows();
                    }
                    feedback.push("Navigateur ouvert pour sélection de la barre d'adresse.".to_string());
                }
            }
            AgentAction::ClickElement { window, target_name } => {
                let is_browser_target = is_address_bar_target(target_name) || is_browser_chrome_target(target_name);
                let target_hwnd = match window.as_deref() {
                    Some(w) if !w.trim().is_empty() => {
                        find_windows_matching(w, &user_windows, active_user_hwnd).first().copied()
                    }
                    _ => {
                        if is_browser_target {
                            find_best_browser_window(&user_windows, active_user_hwnd)
                        } else {
                            active_user_hwnd
                        }
                    }
                };

                if let Some(hwnd) = target_hwnd {
                    if is_window_hung(hwnd) {
                        feedback.push(format!("Fenêtre cible [HWND {:?}] non réactive (sablier). Clic ignoré.", hwnd.0));
                        continue;
                    }
                    let is_browser = user_windows.iter().find(|(h, _)| *h == hwnd).map_or(false, |(h, t)| is_browser_hwnd(*h, t));

                    // Dérivation immédiate vers le raccourci universel Alt+D / Ctrl+L si la barre d'adresse est ciblée
                    if is_browser && is_address_bar_target(target_name) {
                        println!("[Actions] Activation directe barre d'adresse (Alt+D / Ctrl+L) sur [HWND {:?}]", hwnd.0);
                        focus_browser_address_bar(hwnd);
                        feedback.push("Barre d'adresse sélectionnée via raccourci universel (Alt+D / Ctrl+L).".to_string());
                        continue;
                    }

                    println!("[Actions] Clic sur l'élément [HWND {:?}] pour '{}'", hwnd.0, target_name);
                    let show_mode = if is_browser { SW_MAXIMIZE } else { SW_RESTORE };
                    force_foreground_window(hwnd);
                    unsafe {
                        let _ = ShowWindow(hwnd, show_mode);
                    }
                    std::thread::sleep(Duration::from_millis(80));

                    let mut elements = list_interactive_elements(hwnd);
                    if elements.is_empty() {
                        std::thread::sleep(Duration::from_millis(150));
                        elements = list_interactive_elements(hwnd);
                    }

                    if let Some(elem) = find_best_element(&elements, target_name) {
                        // Détection structurelle : zone de saisie située dans le chrome navigateur -> barre d'adresse
                        if is_browser && elem.is_browser_chrome && (elem.is_edit_or_textarea || elem.is_input) {
                            println!("[Actions] Détection structurelle : zone de saisie dans le chrome navigateur résolue -> focus_browser_address_bar");
                            focus_browser_address_bar(hwnd);
                            feedback.push("Barre d'adresse sélectionnée via détection structurelle UIA (Alt+D / Ctrl+L).".to_string());
                            continue;
                        }
                        println!("[Actions] Élément/lien trouvé : name='{}', id='{}', type='{}', clic en ({}, {})", elem.name, elem.automation_id, elem.localized_type, elem.click_x, elem.click_y);
                        unsafe { let _ = elem.element.SetFocus(); }
                        click_element(elem.click_x, elem.click_y, Some(&elem.element), elem.pattern.as_ref());
                        feedback.push(format!("Clic effectué sur l'élément '{}'.", target_name));
                    } else {
                        // Repli : vérifier si l'élément se trouve dans une notification active
                        let mut found_toast = None;
                        for (w_h, w_t) in &user_windows {
                            if *w_h != hwnd && (w_t.to_lowercase().contains("notification") || w_t.to_lowercase().contains("toast")) {
                                let toast_elems = list_interactive_elements(*w_h);
                                if let Some(elem) = find_best_element(&toast_elems, target_name) {
                                    found_toast = Some((*w_h, elem.clone()));
                                    break;
                                }
                            }
                        }
                        if let Some((toast_hwnd, elem)) = found_toast {
                            force_foreground_window(toast_hwnd);
                            unsafe { let _ = elem.element.SetFocus(); }
                            click_element(elem.click_x, elem.click_y, Some(&elem.element), elem.pattern.as_ref());
                            feedback.push(format!("Clic effectué sur l'élément '{}' (Notification).", target_name));
                        } else {
                        println!("[Actions] Aucun élément/lien correspondant trouvé pour '{}'", target_name);
                        feedback.push(format!("Élément ou lien '{}' introuvable à l'écran.", target_name));
                        }
                    }
                }
            }
            AgentAction::Scroll { direction, window, amount } => {
                let target_hwnd = match window.as_deref() {
                    Some(w) if !w.trim().is_empty() => {
                        find_windows_matching(w, &user_windows, active_user_hwnd).first().copied()
                    }
                    _ => preferred_target_hwnd.or(active_user_hwnd),
                };

                let dir_clean = direction.trim().to_lowercase();
                let steps = amount.unwrap_or(1).clamp(1, 10);

                if let Some(hwnd) = target_hwnd {
                    if is_window_hung(hwnd) {
                        feedback.push(format!("Fenêtre cible [HWND {:?}] non réactive (sablier). Défilement ignoré.", hwnd.0));
                        continue;
                    }
                    let win_title = user_windows
                        .iter()
                        .find(|(h, _)| *h == hwnd)
                        .map(|(_, t)| t.as_str())
                        .unwrap_or("");
                    let is_browser = user_windows.iter().find(|(h, _)| *h == hwnd).map_or(false, |(h, t)| is_browser_hwnd(*h, t));
                    let show_mode = if is_browser { SW_MAXIMIZE } else { SW_RESTORE };
                    force_foreground_window(hwnd);
                    unsafe {
                        let _ = ShowWindow(hwnd, show_mode);
                    }
                    std::thread::sleep(Duration::from_millis(60));

                    let mut r = RECT::default();
                    let (cx, cy) = if unsafe { GetWindowRect(hwnd, &mut r).is_ok() } {
                        let w = (r.right - r.left).max(10);
                        let h = (r.bottom - r.top).max(10);
                        (r.left + w / 2, r.top + h / 2)
                    } else {
                        (wa_x + wa_w / 2, wa_y + wa_h / 2)
                    };

                    unsafe {
                        let _ = SetCursorPos(cx, cy);
                    }
                    std::thread::sleep(Duration::from_millis(40));

                    const VK_NEXT: VIRTUAL_KEY = VIRTUAL_KEY(0x22);
                    const VK_PRIOR: VIRTUAL_KEY = VIRTUAL_KEY(0x21);
                    const VK_CONTROL: VIRTUAL_KEY = VIRTUAL_KEY(0x11);
                    const VK_HOME: VIRTUAL_KEY = VIRTUAL_KEY(0x24);
                    const VK_END: VIRTUAL_KEY = VIRTUAL_KEY(0x23);

                    if dir_clean == "up" || dir_clean == "haut" {
                        for _ in 0..steps {
                            if is_request_cancelled(request_id) { break; }
                            unsafe { mouse_event(MOUSEEVENTF_WHEEL, 0, 0, 360, 0); }
                            std::thread::sleep(Duration::from_millis(30));
                            send_hotkey(&[], VK_PRIOR);
                            std::thread::sleep(Duration::from_millis(40));
                        }
                        feedback.push(format!("Défilement vers le haut ({steps} pas) effectué."));
                    } else if dir_clean == "top" || dir_clean == "debut" || dir_clean == "début" {
                        send_hotkey(&[VK_CONTROL], VK_HOME);
                        feedback.push("Défilement vers le début de page effectué.".to_string());
                    } else if dir_clean == "bottom" || dir_clean == "fin" {
                        send_hotkey(&[VK_CONTROL], VK_END);
                        feedback.push("Défilement vers la fin de page effectué.".to_string());
                    } else {
                        for _ in 0..steps {
                            if is_request_cancelled(request_id) { break; }
                            unsafe { mouse_event(MOUSEEVENTF_WHEEL, 0, 0, -360, 0); }
                            std::thread::sleep(Duration::from_millis(30));
                            send_hotkey(&[], VK_NEXT);
                            std::thread::sleep(Duration::from_millis(40));
                        }
                        feedback.push(format!("Défilement vers le bas ({steps} pas) effectué."));
                    }

                    std::thread::sleep(Duration::from_millis(150));
                    let scrolled_summary = summarize_screen_state(if win_title.is_empty() { None } else { Some(win_title) }, false);
                    if !scrolled_summary.trim().is_empty() {
                        if let Ok(mut lock) = LAST_SCREEN_SUMMARY.lock() {
                            *lock = Some(scrolled_summary.clone());
                        }
                        feedback.push(format!(
                            "[Contenu et éléments révélés après défilement] :\n{}",
                            truncate_with_notice(&scrolled_summary, 2500)
                        ));
                    }
                }
            }
            AgentAction::KillProcess { pid, name } => {
                if let Some(p) = pid {
                    println!("[Actions] Terminaison du PID {}", p);
                    let _ = std::process::Command::new("taskkill")
                        .args(["/F", "/PID", &p.to_string()])
                        .spawn();
                    feedback.push(format!("Processus PID {} arrêté.", p));
                } else if let Some(n) = name.as_deref().filter(|s| !s.trim().is_empty()) {
                    let proc_name = if n.to_lowercase().ends_with(".exe") {
                        n.to_string()
                    } else {
                        format!("{n}.exe")
                    };
                    println!("[Actions] Terminaison de {}", proc_name);
                    let _ = std::process::Command::new("taskkill")
                        .args(["/F", "/IM", &proc_name])
                        .spawn();
                    feedback.push(format!("Processus '{}' arrêté.", proc_name));
                } else {
                    if let Some(hwnd) = active_user_hwnd {
                        let (target_pid, _) = get_window_process_info(hwnd);
                        let _ = std::process::Command::new("taskkill")
                            .args(["/F", "/PID", &target_pid.to_string()])
                            .spawn();
                    }
                }
            }
            AgentAction::ClickButton { window, button_name } => {
                let is_browser_target = is_browser_chrome_target(button_name);
                let target_hwnd = match window.as_deref() {
                    Some(w) if !w.trim().is_empty() => {
                        find_windows_matching(w, &user_windows, active_user_hwnd).first().copied()
                    }
                    _ => {
                        if is_browser_target {
                            find_best_browser_window(&user_windows, active_user_hwnd)
                        } else {
                            active_user_hwnd
                        }
                    }
                };

                if let Some(hwnd) = target_hwnd {
                    if is_window_hung(hwnd) {
                        feedback.push(format!("Fenêtre cible [HWND {:?}] non réactive (sablier). Clic bouton ignoré.", hwnd.0));
                        continue;
                    }
                    println!("[Actions] Clic sur le bouton [HWND {:?}] pour '{}'", hwnd.0, button_name);
                    let is_browser = user_windows.iter().find(|(h, _)| *h == hwnd).map_or(false, |(h, t)| is_browser_hwnd(*h, t));
                    let show_mode = if is_browser { SW_MAXIMIZE } else { SW_RESTORE };
                    force_foreground_window(hwnd);
                    unsafe {
                        let _ = ShowWindow(hwnd, show_mode);
                    }
                    std::thread::sleep(Duration::from_millis(100));

                    let mut elements = list_interactive_elements(hwnd);
                    if elements.is_empty() {
                        std::thread::sleep(Duration::from_millis(150));
                        elements = list_interactive_elements(hwnd);
                    }

                    if let Some(elem) = find_best_button_element(&elements, button_name) {
                        println!("[Actions] Bouton trouvé : name='{}', id='{}', clic en ({}, {})", elem.name, elem.automation_id, elem.click_x, elem.click_y);
                        unsafe { let _ = elem.element.SetFocus(); }
                        click_element(elem.click_x, elem.click_y, Some(&elem.element), elem.pattern.as_ref());
                        feedback.push(format!("Clic effectué sur '{}'.", button_name));
                    } else {
                        // Repli : vérifier si le bouton se trouve dans une notification active
                        let mut found_toast = None;
                        for (w_h, w_t) in &user_windows {
                            if *w_h != hwnd && (w_t.to_lowercase().contains("notification") || w_t.to_lowercase().contains("toast")) {
                                let toast_elems = list_interactive_elements(*w_h);
                                if let Some(elem) = find_best_button_element(&toast_elems, button_name) {
                                    found_toast = Some((*w_h, elem.clone()));
                                    break;
                                }
                            }
                        }
                        if let Some((toast_hwnd, elem)) = found_toast {
                            force_foreground_window(toast_hwnd);
                            unsafe { let _ = elem.element.SetFocus(); }
                            click_element(elem.click_x, elem.click_y, Some(&elem.element), elem.pattern.as_ref());
                            feedback.push(format!("Clic effectué sur le bouton '{}' de la notification.", button_name));
                        } else {
                            println!("[Actions] Aucun bouton correspondant trouvé pour '{}'", button_name);
                            feedback.push(format!("Bouton '{}' introuvable à l'écran.", button_name));
                        }
                    }
                }
            }
            AgentAction::FocusElement { window, target_name } => {
                let is_browser_target = is_address_bar_target(target_name) || is_browser_chrome_target(target_name);
                let target_hwnd = match window.as_deref() {
                    Some(w) if !w.trim().is_empty() => {
                        find_windows_matching(w, &user_windows, active_user_hwnd).first().copied()
                    }
                    _ => {
                        if is_browser_target {
                            find_best_browser_window(&user_windows, active_user_hwnd)
                        } else {
                            active_user_hwnd
                        }
                    }
                };

                if let Some(hwnd) = target_hwnd {
                    if is_window_hung(hwnd) {
                        feedback.push(format!("Fenêtre cible [HWND {:?}] non réactive (sablier). Focus ignoré.", hwnd.0));
                        continue;
                    }
                    let is_browser = user_windows.iter().find(|(h, _)| *h == hwnd).map_or(false, |(h, t)| is_browser_hwnd(*h, t));

                    // Focus direct et infaillible de la barre d'adresse / omnibar
                    if is_browser && (is_address_bar_target(target_name) || target_name.trim().is_empty()) {
                        println!("[Actions] Focus direct omnibar (Alt+D / Ctrl+L) sur [HWND {:?}]", hwnd.0);
                        focus_browser_address_bar(hwnd);
                        active_user_hwnd = Some(hwnd);
                        preferred_target_hwnd = Some(hwnd);
                        feedback.push("Omnibar du navigateur sélectionnée avec succès.".to_string());
                        continue;
                    }

                    println!("[Actions] Focus sur l'élément [HWND {:?}] pour '{}'", hwnd.0, target_name);
                    let show_mode = if is_browser { SW_MAXIMIZE } else { SW_RESTORE };
                    force_foreground_window(hwnd);
                    unsafe {
                        let _ = ShowWindow(hwnd, show_mode);
                    }
                    std::thread::sleep(Duration::from_millis(60));

                    let t_lower = target_name.trim().to_lowercase();
                    let is_generic_textarea = t_lower.is_empty()
                        || t_lower == "textarea"
                        || t_lower == "champ"
                        || t_lower == "input"
                        || t_lower == "zone de texte";

                    if is_generic_textarea {
                        refocus_largest_textarea_or_fallback(hwnd);
                    } else {
                        let mut elements = list_interactive_elements(hwnd);
                        if elements.is_empty() {
                            std::thread::sleep(Duration::from_millis(150));
                            elements = list_interactive_elements(hwnd);
                        }
                        if let Some(elem) = find_best_element(&elements, target_name) {
                            if is_browser && elem.is_browser_chrome && (elem.is_edit_or_textarea || elem.is_input) {
                                println!("[Actions] Détection structurelle : zone de saisie dans le chrome navigateur résolue -> focus_browser_address_bar");
                                focus_browser_address_bar(hwnd);
                                feedback.push("Barre d'adresse sélectionnée via détection structurelle UIA (Alt+D / Ctrl+L).".to_string());
                                continue;
                            }
                        }

                        if let Some(elem) = find_best_element(&elements, target_name) {
                            println!("[Actions] Élément focusable trouvé : name='{}', id='{}'", elem.name, elem.automation_id);
                            unsafe { let _ = elem.element.SetFocus(); }
                            click_element(elem.click_x, elem.click_y, Some(&elem.element), elem.pattern.as_ref());
                        } else {
                            println!("[Actions] Repli focus textarea/champ principal");
                            refocus_largest_textarea_or_fallback(hwnd);
                        }
                        feedback.push(format!("Focus positionné sur '{}'.", target_name));
                    }
                }
                feedback.push(format!("Focus demandé sur '{}'.", target_name));
            }
            AgentAction::ClearText { window, target } => {
                let target_hwnd = match window.as_deref() {
                    Some(w) if !w.trim().is_empty() => {
                        find_windows_matching(w, &user_windows, active_user_hwnd).first().copied()
                    }
                    _ => active_user_hwnd,
                };

                if let Some(hwnd) = target_hwnd {
                    if is_window_hung(hwnd) {
                        feedback.push(format!("Fenêtre cible [HWND {:?}] non réactive (sablier). Effacement ignoré.", hwnd.0));
                        continue;
                    }
                    if let Some(t) = target.as_deref().filter(|s| !s.trim().is_empty()) {
                        force_foreground_window(hwnd);
                        unsafe {
                            let _ = ShowWindow(hwnd, SW_RESTORE);
                        }
                        std::thread::sleep(Duration::from_millis(60));
                        let elements = list_interactive_elements(hwnd);
                        if let Some(elem) = find_best_input_element(&elements, t) {
                            unsafe { let _ = elem.element.SetFocus(); }
                            click_element(elem.click_x, elem.click_y, Some(&elem.element), elem.pattern.as_ref());
                            std::thread::sleep(Duration::from_millis(40));
                            const VK_CONTROL: VIRTUAL_KEY = VIRTUAL_KEY(0x11);
                            const VK_A: VIRTUAL_KEY = VIRTUAL_KEY(0x41);
                            const VK_BACK: VIRTUAL_KEY = VIRTUAL_KEY(0x08);
                            send_hotkey(&[VK_CONTROL], VK_A);
                            std::thread::sleep(Duration::from_millis(30));
                            send_hotkey(&[], VK_BACK);
                        } else {
                            clear_window_text(hwnd);
                        }
                    } else {
                        clear_window_text(hwnd);
                    }
                    feedback.push("Champ de texte réinitialisé.".to_string());
                }
            }
            AgentAction::RunCommand { command } => {
                let cmd_clean = command.trim().to_lowercase();
                if cmd_clean == "press_enter" || cmd_clean == "enter" || cmd_clean == "return" {
                    const VK_RETURN: VIRTUAL_KEY = VIRTUAL_KEY(0x0D);
                    send_hotkey(&[], VK_RETURN);
                    feedback.push("Touche Entrée envoyée.".to_string());
                    continue;
                }
                if cmd_clean == "escape" || cmd_clean == "esc" {
                    const VK_ESCAPE: VIRTUAL_KEY = VIRTUAL_KEY(0x1B);
                    send_hotkey(&[], VK_ESCAPE);
                    feedback.push("Touche Échap envoyée.".to_string());
                    continue;
                }
                // Interception préventive : évite de lancer un shell cmd.exe pour une combinaison de touches
                if try_send_key_combination(&cmd_clean) {
                    println!("[Actions] Combinaison de touches détectée dans run_command et simulée : {}", command);
                    feedback.push(format!("Combinaison de touches '{}' simulée avec succès.", command));
                    continue;
                }

                let initial_hwnds: std::collections::HashSet<isize> = user_windows
                    .iter()
                    .map(|(h, _)| h.0 as isize)
                    .collect();

                let ok = run_system_command(command);
                let status = if ok { "exécutée avec succès" } else { "échec d'exécution" };
                feedback.push(format!("Commande système '{}' ({status}).", command));

                if ok {
                    for _ in 0..15 {
                        if is_request_cancelled(request_id) { break; }
                        std::thread::sleep(Duration::from_millis(80));
                        let current_windows = list_user_windows();
                        if let Some((new_hwnd, _)) = current_windows.iter().find(|(h, _)| !initial_hwnds.contains(&(h.0 as isize))) {
                            force_foreground_window(*new_hwnd);
                            unsafe { let _ = ShowWindow(*new_hwnd, SW_RESTORE); }
                            active_user_hwnd = Some(*new_hwnd);
                            preferred_target_hwnd = Some(*new_hwnd);
                            user_windows = current_windows;
                            break;
                        }
                    }
                }
            }
            AgentAction::WriteText { text, target, window, press_enter } => {
                const VK_RETURN: VIRTUAL_KEY = VIRTUAL_KEY(0x0D);
                const VK_CONTROL: VIRTUAL_KEY = VIRTUAL_KEY(0x11);
                const VK_A: VIRTUAL_KEY = VIRTUAL_KEY(0x41);
                let should_enter = press_enter.unwrap_or(false) || text.ends_with('\n');
                let target_desc_opt = target.as_deref().filter(|s| !s.trim().is_empty());
                let is_address_bar = target_desc_opt.map_or(false, is_address_bar_target);
                let is_generic = target_desc_opt.map_or(true, is_generic_textarea_target);

                let target_hwnd = match window.as_deref() {
                    Some(w) if !w.trim().is_empty() => {
                        find_windows_matching(w, &user_windows, active_user_hwnd).first().copied()
                    }
                    _ => {
                        if is_address_bar {
                            find_best_browser_window(&user_windows, active_user_hwnd)
                        } else {
                            active_user_hwnd
                        }
                    }
                };

                let mut handled = false;

                if !is_address_bar {
                let candidate_hwnd = target_hwnd.or(active_user_hwnd).or(preferred_target_hwnd);
                if let Some(hwnd) = candidate_hwnd {
                    let win_title = user_windows
                        .iter()
                        .find(|(h, _)| *h == hwnd)
                        .map(|(_, t)| t.as_str())
                        .unwrap_or("");
                    if is_terminal_window(hwnd, win_title) {
                        if is_window_hung(hwnd) {
                            feedback.push(format!("Terminal [HWND {:?}] non réactif. Écriture ignorée.", hwnd.0));
                            continue;
                        }
                        force_foreground_window(hwnd);
                        unsafe { let _ = ShowWindow(hwnd, SW_RESTORE); }
                        std::thread::sleep(Duration::from_millis(60));
                        let auto_enter = text.ends_with('\n') || text.ends_with('\r');
                        send_terminal_input(hwnd, text, auto_enter);
                        println!("[Actions] Texte inséré dans le terminal [HWND {:?}] '{}'", hwnd.0, win_title);
                        if auto_enter {
                            feedback.push(format!("Commande envoyée et validée dans le terminal '{}'.", win_title));
                        } else {
                            feedback.push(format!("Texte inséré dans le terminal '{}'.", win_title));
                        }
                        handled = true;
                    }
                }
                }

                if !handled {
                    if is_address_bar {
                        let browser_hwnd = target_hwnd.filter(|&h| {
                            let title = user_windows.iter().find(|(wh, _)| *wh == h).map(|(_, t)| t.as_str()).unwrap_or("");
                            is_browser_hwnd(h, title)
                        });

                        if let Some(hwnd) = browser_hwnd {
                            if is_window_hung(hwnd) {
                                feedback.push(format!("Navigateur [HWND {:?}] non réactif (sablier). Écriture ignorée.", hwnd.0));
                                continue;
                            }
                            let win_title = user_windows.iter().find(|(h, _)| *h == hwnd).map(|(_, t)| t.as_str()).unwrap_or("Navigateur");
                            println!("[Actions] Ciblage barre d'adresse sur [HWND {:?}] '{}'", hwnd.0, win_title);
                            force_foreground_window(hwnd);
                            unsafe {
                                let _ = ShowWindow(hwnd, SW_MAXIMIZE);
                            }
                            std::thread::sleep(Duration::from_millis(50));
                            navigate_browser_address_bar(hwnd, text);
                            handled = true;
                        } else {
                            println!("[Actions] Aucun navigateur ouvert trouvé pour la barre d'adresse, ouverture d'une nouvelle fenêtre pour : {}", text);
                            let spawned = launch_browser_new_window(text);
                            if let Some(h) = spawned {
                                newly_spawned_hwnd = Some(h);
                                active_user_hwnd = Some(h);
                                preferred_target_hwnd = Some(h);
                                user_windows = list_user_windows();
                            }
                            feedback.push(format!("Navigateur ouvert vers '{}'.", text));
                            handled = true;
                        }
                } else if is_generic {
                    let largest_target = if let Some(hwnd) = window.as_deref().and_then(|w| {
                        if !w.trim().is_empty() {
                            find_windows_matching(w, &user_windows, active_user_hwnd).first().copied()
                        } else {
                            None
                        }
                    }) {
                        find_largest_textarea(hwnd).map(|elem| (hwnd, elem))
                    } else {
                        find_largest_textarea_on_screen(&user_windows, preferred_target_hwnd)
                    };

                    if let Some((hwnd, elem)) = largest_target {
                        if is_window_hung(hwnd) {
                            feedback.push(format!("Fenêtre cible [HWND {:?}] non réactive. Écriture ignorée.", hwnd.0));
                            continue;
                        }
                        println!("[Actions] Sélection de la plus grande zone de texte à l'écran : [HWND {:?}] name='{}', id='{}', area={}, clic en ({}, {})", hwnd.0, elem.name, elem.automation_id, elem.area, elem.click_x, elem.click_y);
                        force_foreground_window(hwnd);
                        unsafe {
                            let _ = ShowWindow(hwnd, SW_RESTORE);
                        }
                        std::thread::sleep(Duration::from_millis(80));
                        unsafe { let _ = elem.element.SetFocus(); }
                        click_element(elem.click_x, elem.click_y, Some(&elem.element), elem.pattern.as_ref());
                        std::thread::sleep(Duration::from_millis(50));
                        send_hotkey(&[VK_CONTROL], VK_A);
                        std::thread::sleep(Duration::from_millis(30));
                        clipboard::set_text(text);
                        std::thread::sleep(Duration::from_millis(30));
                        send_paste();
                        if should_enter {
                            std::thread::sleep(Duration::from_millis(60));
                            send_hotkey(&[], VK_RETURN);
                        }
                        println!("[Actions] Texte inséré avec succès dans la plus grande zone de texte : {:?}", text);
                        handled = true;
                    }
                } else if let (Some(hwnd), Some(target_desc)) = (target_hwnd, target_desc_opt) {
                    if is_window_hung(hwnd) {
                        feedback.push(format!("Fenêtre [HWND {:?}] non réactive. Écriture ignorée.", hwnd.0));
                        continue;
                    }
                    force_foreground_window(hwnd);
                    unsafe {
                        let _ = ShowWindow(hwnd, SW_RESTORE);
                    }
                    std::thread::sleep(Duration::from_millis(70));

                    let elements = list_interactive_elements(hwnd);
                    if let Some(elem) = find_best_input_element(&elements, target_desc) {
                        println!("[Actions] Saisie dans l'élément : name='{}', id='{}'", elem.name, elem.automation_id);
                        unsafe { let _ = elem.element.SetFocus(); }
                        click_element(elem.click_x, elem.click_y, Some(&elem.element), elem.pattern.as_ref());
                        std::thread::sleep(Duration::from_millis(50));

                        send_hotkey(&[VK_CONTROL], VK_A);
                        std::thread::sleep(Duration::from_millis(30));

                        clipboard::set_text(text);
                        std::thread::sleep(Duration::from_millis(30));
                        send_paste();
                        if should_enter {
                            std::thread::sleep(Duration::from_millis(60));
                            send_hotkey(&[], VK_RETURN);
                        }
                        println!("[Actions] Texte inséré avec succès : {:?}", text);
                        feedback.push(format!("Texte inséré avec succès ({} caractères).", text.len()));
                        handled = true;
                    } else {
                        println!("[Actions] Aucun champ d'entrée trouvé pour {:?}", target_desc);
                        if let Some((fallback_hwnd, elem)) = find_largest_textarea_on_screen(&user_windows, preferred_target_hwnd) {
                            println!("[Actions] Repli sur la plus grande zone de texte : [HWND {:?}], clic en ({}, {})", fallback_hwnd.0, elem.click_x, elem.click_y);
                            force_foreground_window(fallback_hwnd);
                            unsafe {
                                let _ = ShowWindow(fallback_hwnd, SW_RESTORE);
                            }
                            std::thread::sleep(Duration::from_millis(80));
                            unsafe { let _ = elem.element.SetFocus(); }
                            click_element(elem.click_x, elem.click_y, Some(&elem.element), elem.pattern.as_ref());
                            std::thread::sleep(Duration::from_millis(50));
                            send_hotkey(&[VK_CONTROL], VK_A);
                            std::thread::sleep(Duration::from_millis(30));
                            clipboard::set_text(text);
                            std::thread::sleep(Duration::from_millis(30));
                            send_paste();
                            if should_enter {
                                std::thread::sleep(Duration::from_millis(60));
                                send_hotkey(&[], VK_RETURN);
                            }
                            feedback.push(format!("Texte inséré dans la zone principale ({} caractères).", text.len()));
                            handled = true;
                        }
                    }
                }
                }

                if !handled {
                    println!("[Actions] Repli d'écriture dans le document ou fenêtre de repli");
                    let _ = write_to_browser_or_txt(text);
                    if should_enter {
                        std::thread::sleep(Duration::from_millis(60));
                        send_hotkey(&[], VK_RETURN);
                    }
                    feedback.push(format!("Texte écrit via repli système ({} caractères).", text.len()));
                }
            }
            AgentAction::ReplaceFieldText { text, target, window } => {
                let target_desc_opt = target.as_deref().filter(|s| !s.trim().is_empty());
                let is_address_bar = target_desc_opt.map_or(false, is_address_bar_target);
                let is_generic = target_desc_opt.map_or(true, is_generic_textarea_target);

                let target_hwnd = match window.as_deref() {
                    Some(w) if !w.trim().is_empty() => {
                        find_windows_matching(w, &user_windows, active_user_hwnd).first().copied()
                    }
                    _ => {
                        if is_address_bar {
                            find_best_browser_window(&user_windows, active_user_hwnd)
                        } else {
                            active_user_hwnd
                        }
                    }
                };

                let mut handled = false;

                if !is_address_bar {
                let candidate_hwnd = target_hwnd.or(active_user_hwnd).or(preferred_target_hwnd);
                if let Some(hwnd) = candidate_hwnd {
                    let win_title = user_windows
                        .iter()
                        .find(|(h, _)| *h == hwnd)
                        .map(|(_, t)| t.as_str())
                        .unwrap_or("");
                    if is_terminal_window(hwnd, win_title) {
                        if is_window_hung(hwnd) {
                            feedback.push(format!("Terminal [HWND {:?}] non réactif. Remplacement ignoré.", hwnd.0));
                            continue;
                        }
                        force_foreground_window(hwnd);
                        unsafe { let _ = ShowWindow(hwnd, SW_RESTORE); }
                        std::thread::sleep(Duration::from_millis(50));
                        // Annulation de la commande courante
                        const VK_CONTROL: VIRTUAL_KEY = VIRTUAL_KEY(0x11);
                        const VK_C: VIRTUAL_KEY = VIRTUAL_KEY(0x43);
                        send_hotkey(&[VK_CONTROL], VK_C);
                        std::thread::sleep(Duration::from_millis(50));
                        let auto_enter = text.ends_with('\n') || text.ends_with('\r');
                        send_terminal_input(hwnd, text, auto_enter);
                        feedback.push(format!("Ligne réinitialisée et texte inséré dans le terminal '{}'.", win_title));
                        handled = true;
                    }
                }
                }

                if !handled {
                    if is_address_bar {
                        let browser_hwnd = target_hwnd.filter(|&h| {
                            let title = user_windows.iter().find(|(wh, _)| *wh == h).map(|(_, t)| t.as_str()).unwrap_or("");
                            is_browser_hwnd(h, title)
                        });

                        if let Some(hwnd) = browser_hwnd {
                            if is_window_hung(hwnd) {
                                feedback.push(format!("Navigateur [HWND {:?}] non réactif. Remplacement ignoré.", hwnd.0));
                                continue;
                            }
                            let win_title = user_windows.iter().find(|(h, _)| *h == hwnd).map(|(_, t)| t.as_str()).unwrap_or("Navigateur");
                            println!("[Actions] Remplacement d'url barre d'adresse sur [HWND {:?}] '{}'", hwnd.0, win_title);
                            force_foreground_window(hwnd);
                            unsafe {
                                let _ = ShowWindow(hwnd, SW_MAXIMIZE);
                            }
                            std::thread::sleep(Duration::from_millis(50));
                            navigate_browser_address_bar(hwnd, text);
                            handled = true;
                        } else {
                            println!("[Actions] Aucun navigateur ouvert trouvé pour la barre d'adresse, ouverture avec : {}", text);
                            let spawned = launch_browser_new_window(text);
                            if let Some(h) = spawned {
                                newly_spawned_hwnd = Some(h);
                                active_user_hwnd = Some(h);
                                preferred_target_hwnd = Some(h);
                                user_windows = list_user_windows();
                            }
                            feedback.push(format!("Navigateur ouvert vers '{}'.", text));
                            handled = true;
                        }
                } else if is_generic {
                    let largest_target = if let Some(hwnd) = window.as_deref().and_then(|w| {
                        if !w.trim().is_empty() {
                            find_windows_matching(w, &user_windows, active_user_hwnd).first().copied()
                        } else {
                            None
                        }
                    }) {
                        find_largest_textarea(hwnd).map(|elem| (hwnd, elem))
                    } else {
                        find_largest_textarea_on_screen(&user_windows, preferred_target_hwnd)
                    };

                    if let Some((hwnd, elem)) = largest_target {
                        if is_window_hung(hwnd) {
                            feedback.push(format!("Fenêtre [HWND {:?}] non réactive. Remplacement ignoré.", hwnd.0));
                            continue;
                        }
                        println!("[Actions] Remplacement dans la plus grande zone de texte : [HWND {:?}], area={}, clic en ({}, {})", hwnd.0, elem.area, elem.click_x, elem.click_y);
                        force_foreground_window(hwnd);
                        unsafe {
                            let _ = ShowWindow(hwnd, SW_RESTORE);
                        }
                        std::thread::sleep(Duration::from_millis(80));
                        unsafe { let _ = elem.element.SetFocus(); }
                        click_element(elem.click_x, elem.click_y, Some(&elem.element), elem.pattern.as_ref());
                        std::thread::sleep(Duration::from_millis(50));
                        const VK_CONTROL: VIRTUAL_KEY = VIRTUAL_KEY(0x11);
                        const VK_A: VIRTUAL_KEY = VIRTUAL_KEY(0x41);
                        send_hotkey(&[VK_CONTROL], VK_A);
                        std::thread::sleep(Duration::from_millis(30));
                        clipboard::set_text(text);
                        std::thread::sleep(Duration::from_millis(30));
                        send_paste();
                        println!("[Actions] Texte remplacé avec succès dans la zone de texte : {:?}", text);
                        handled = true;
                    }
                } else if let (Some(hwnd), Some(target_desc)) = (target_hwnd, target_desc_opt) {
                    if is_window_hung(hwnd) {
                        feedback.push(format!("Fenêtre [HWND {:?}] non réactive. Remplacement ignoré.", hwnd.0));
                        continue;
                    }
                    force_foreground_window(hwnd);
                    unsafe {
                        let _ = ShowWindow(hwnd, SW_RESTORE);
                    }
                    std::thread::sleep(Duration::from_millis(70));

                    let elements = list_interactive_elements(hwnd);
                    if let Some(elem) = find_best_input_element(&elements, target_desc) {
                        println!("[Actions] Remplacement du champ : name='{}', id='{}'", elem.name, elem.automation_id);
                        unsafe { let _ = elem.element.SetFocus(); }
                        click_element(elem.click_x, elem.click_y, Some(&elem.element), elem.pattern.as_ref());
                        std::thread::sleep(Duration::from_millis(50));

                        const VK_CONTROL: VIRTUAL_KEY = VIRTUAL_KEY(0x11);
                        const VK_A: VIRTUAL_KEY = VIRTUAL_KEY(0x41);
                        send_hotkey(&[VK_CONTROL], VK_A);
                        std::thread::sleep(Duration::from_millis(30));
                        clipboard::set_text(text);
                        std::thread::sleep(Duration::from_millis(30));
                        send_paste();
                        feedback.push(format!("Texte du champ mis à jour avec succès ({} caractères).", text.len()));
                        handled = true;
                    }
                }
            }
                if !handled {
                    println!("[Actions] Repli de remplacement sur le champ actif");
                    let _ = replace_active_field_text(text);
                }
            }
            AgentAction::OpenBrowser { .. } => {}
            AgentAction::NavigateToUrl { url, target, window, new_window } => {
                let formatted_url = format_url_for_navigation(url);
                let target_mode = target.as_deref()
                    .or(window.as_deref())
                    .map(str::trim)
                    .filter(|s| !s.is_empty());

                let t_lower = target_mode.map(|s| s.to_lowercase()).unwrap_or_default();
                let force_new = new_window.unwrap_or(false)
                    || t_lower == "new_window"
                    || t_lower == "nouvelle_fenetre"
                    || t_lower == "nouvelle_fenêtre"
                    || t_lower == "window";

                let is_current = t_lower == "current_tab"
                    || t_lower == "current_window"
                    || t_lower == "current"
                    || t_lower == "active"
                    || t_lower == "courante"
                    || t_lower == "_self";

                let is_explicit_new_tab = t_lower == "new_tab"
                    || t_lower == "new"
                    || t_lower == "tab"
                    || t_lower == "nouvel_onglet"
                    || t_lower == "onglet"
                    || t_lower == "_blank";

                if force_new {
                    println!("[Actions] Ouverture d'une nouvelle fenêtre de navigateur vers : {}", formatted_url);
                    let spawned = launch_browser_new_window(&formatted_url);
                    if let Some(h) = spawned {
                        newly_spawned_hwnd = Some(h);
                        active_user_hwnd = Some(h);
                        preferred_target_hwnd = Some(h);
                        user_windows = list_user_windows();
                    }
                    feedback.push(format!("Nouvelle fenêtre ouverte vers '{}'.", formatted_url));
                } else if is_current {
                    let target_hwnd = find_best_browser_window(&user_windows, active_user_hwnd);

                    if let Some(hwnd) = target_hwnd {
                        if is_window_hung(hwnd) {
                            feedback.push(format!("Navigateur [HWND {:?}] ne répond pas. Ouverture d'une nouvelle instance.", hwnd.0));
                            let spawned = launch_browser_new_window(&formatted_url);
                            if let Some(h) = spawned {
                                newly_spawned_hwnd = Some(h);
                                active_user_hwnd = Some(h);
                                preferred_target_hwnd = Some(h);
                            }
                        } else {
                            println!("[Actions] Navigation dans l'onglet actif sur [HWND {:?}] vers : {}", hwnd.0, formatted_url);
                            force_foreground_window(hwnd);
                            unsafe {
                                let _ = ShowWindow(hwnd, SW_MAXIMIZE);
                            }
                            navigate_browser_address_bar(hwnd, &formatted_url);
                            active_user_hwnd = Some(hwnd);
                            preferred_target_hwnd = Some(hwnd);
                            feedback.push(format!("Navigation effectuée vers '{}' dans l'onglet actif.", formatted_url));
                        }
                    } else {
                        println!("[Actions] Aucun navigateur actif trouvé, ouverture vers : {}", formatted_url);
                        let spawned = launch_browser_new_window(&formatted_url);
                        if let Some(h) = spawned {
                            newly_spawned_hwnd = Some(h);
                            active_user_hwnd = Some(h);
                            preferred_target_hwnd = Some(h);
                            user_windows = list_user_windows();
                        }
                        feedback.push(format!("Navigateur ouvert vers '{}'.", formatted_url));
                    }
                } else {
                    // Par défaut (sans target, "new_tab", ou si target désigne un navigateur spécifique) : NOUVEL ONGLET
                    let target_hwnd = if is_explicit_new_tab || target_mode.is_none() {
                        find_best_browser_window(&user_windows, active_user_hwnd)
                    } else {
                        let matched = find_windows_matching(target_mode.unwrap(), &user_windows, active_user_hwnd);
                        matched.first().copied().filter(|&h| {
                            let title = user_windows.iter().find(|(wh, _)| *wh == h).map(|(_, t)| t.as_str()).unwrap_or("");
                            is_browser_hwnd(h, title)
                        }).or_else(|| find_best_browser_window(&user_windows, active_user_hwnd))
                    };

                    if let Some(hwnd) = target_hwnd {
                        if is_window_hung(hwnd) {
                            feedback.push(format!("Navigateur [HWND {:?}] ne répond pas. Ouverture d'une nouvelle instance.", hwnd.0));
                            let spawned = launch_browser_new_window(&formatted_url);
                            if let Some(h) = spawned {
                                newly_spawned_hwnd = Some(h);
                                active_user_hwnd = Some(h);
                                preferred_target_hwnd = Some(h);
                            }
                        } else {
                            println!("[Actions] Ouverture d'un nouvel onglet sur [HWND {:?}] vers : {}", hwnd.0, formatted_url);
                            open_browser_new_tab(hwnd, &formatted_url);
                            active_user_hwnd = Some(hwnd);
                            preferred_target_hwnd = Some(hwnd);
                            feedback.push(format!("Nouvel onglet ouvert vers '{}'.", formatted_url));
                        }
                    } else {
                        println!("[Actions] Aucun navigateur existant trouvé, ouverture vers : {}", formatted_url);
                        let spawned = launch_browser_new_window(&formatted_url);
                        if let Some(h) = spawned {
                            newly_spawned_hwnd = Some(h);
                            active_user_hwnd = Some(h);
                            preferred_target_hwnd = Some(h);
                            user_windows = list_user_windows();
                        }
                        feedback.push(format!("Navigateur ouvert dans une nouvelle fenêtre vers '{}'.", formatted_url));
                    }
                }
            }
            AgentAction::ArrangeWindow { title, position } => {
                let targets = find_windows_matching(title, &user_windows, preferred_target_hwnd);
                if let Some(&hwnd) = targets.first() {
                    let pos = position.to_lowercase();
                    let p = pos.trim();
                    let is_right = p == "right" || p == "right_half" || p == "droite" || p == "droit" || p.contains("droit") || p.ends_with("right");
                    let is_left = p == "left" || p == "left_half" || p == "gauche" || p.contains("gauche") || p.ends_with("left");

                    if is_right {
                        let screen_mid_x = wa_x + wa_w / 2;
                        let companion = user_windows
                            .iter()
                            .map(|(h, _)| *h)
                            .find(|&h| {
                                if h == hwnd { return false; }
                                let mut r = RECT::default();
                                if unsafe { GetWindowRect(h, &mut r).is_ok() } {
                                    let center_x = (r.left + r.right) / 2;
                                    center_x < screen_mid_x
                                } else {
                                    false
                                }
                            })
                            .or_else(|| active_user_hwnd.filter(|&h| h != hwnd));

                        if let Some(comp_hwnd) = companion {
                            snap_window_pair(comp_hwnd, hwnd);
                        } else {
                            snap_window_native(hwnd, true);
                        }
                    } else if is_left {
                        let screen_mid_x = wa_x + wa_w / 2;
                        let companion = user_windows
                            .iter()
                            .map(|(h, _)| *h)
                            .find(|&h| {
                                if h == hwnd { return false; }
                                let mut r = RECT::default();
                                if unsafe { GetWindowRect(h, &mut r).is_ok() } {
                                    let center_x = (r.left + r.right) / 2;
                                    center_x >= screen_mid_x
                                } else {
                                    false
                                }
                            })
                            .or_else(|| active_user_hwnd.filter(|&h| h != hwnd));

                        if let Some(comp_hwnd) = companion {
                            snap_window_pair(hwnd, comp_hwnd);
                        } else {
                            snap_window_native(hwnd, false);
                        }
                    } else if p == "top" || p == "top_half" || p == "haut" {
                        apply_window_rect(hwnd, wa_x, wa_y, wa_w, wa_h / 2);
                    } else if p == "bottom" || p == "bottom_half" || p == "bas" {
                        apply_window_rect(hwnd, wa_x, wa_y + wa_h / 2, wa_w, wa_h / 2);
                    } else if p == "top_left" || p == "quarter_top_left" || p == "haut_gauche" {
                        apply_window_rect(hwnd, wa_x, wa_y, wa_w / 2, wa_h / 2);
                    } else if p == "top_right" || p == "quarter_top_right" || p == "haut_droite" {
                        apply_window_rect(hwnd, wa_x + wa_w / 2, wa_y, wa_w / 2, wa_h / 2);
                    } else if p == "bottom_left" || p == "quarter_bottom_left" || p == "bas_gauche" {
                        apply_window_rect(hwnd, wa_x, wa_y + wa_h / 2, wa_w / 2, wa_h / 2);
                    } else if p == "bottom_right" || p == "quarter_bottom_right" || p == "bas_droite" {
                        apply_window_rect(hwnd, wa_x + wa_w / 2, wa_y + wa_h / 2, wa_w / 2, wa_h / 2);
                    } else if p == "left_two_thirds" || p == "deux_tiers_gauche" {
                        apply_window_rect(hwnd, wa_x, wa_y, (wa_w * 2) / 3, wa_h);
                    } else if p == "right_one_third" || p == "un_tiers_droite" {
                        apply_window_rect(hwnd, wa_x + (wa_w * 2) / 3, wa_y, wa_w / 3, wa_h);
                    } else if p == "left_one_third" || p == "un_tiers_gauche" {
                        apply_window_rect(hwnd, wa_x, wa_y, wa_w / 3, wa_h);
                    } else if p == "right_two_thirds" || p == "deux_tiers_droite" {
                        apply_window_rect(hwnd, wa_x + wa_w / 3, wa_y, (wa_w * 2) / 3, wa_h);
                    } else if p == "maximize" || p == "plein_ecran" || p == "agrandir" {
                        force_foreground_window(hwnd);
                        unsafe {
                            let _ = ShowWindow(hwnd, SW_MAXIMIZE);
                        }
                    } else if p == "minimize" || p == "reduire" {
                        unsafe {
                            let _ = ShowWindow(hwnd, SW_MINIMIZE);
                        }
                    } else if p == "center" || p == "centre" {
                        let w = (wa_w * 7) / 10;
                        let h = (wa_h * 8) / 10;
                        apply_window_rect(hwnd, wa_x + (wa_w - w) / 2, wa_y + (wa_h - h) / 2, w, h);
                    } else {
                        apply_window_rect(hwnd, wa_x + wa_w / 2, wa_y, wa_w / 2, wa_h);
                    }
                    feedback.push(format!("Fenêtre '{}' agencée en '{}'.", title, position));
                }
            }
            AgentAction::TileWindows { layout, windows } => {
                let layout_mode = layout.as_deref().unwrap_or("split_horizontal");
                let mut target_hwnds = Vec::new();

                for win_key in windows {
                    let hits = find_windows_matching(win_key, &user_windows, preferred_target_hwnd);
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
                match layout_mode {
                    "grid" | "quad" | "grid_2x2" => {
                        let half_w = wa_w / 2;
                        let half_h = wa_h / 2;
                        let ordered_quads = classify_immersion_quadrants(&target_hwnds, &user_windows);
                        let coords = [
                            (wa_x, wa_y),
                            (wa_x + half_w, wa_y),
                            (wa_x, wa_y + half_h),
                            (wa_x + half_w, wa_y + half_h),
                        ];
                        for (idx, &hwnd) in ordered_quads.iter().flatten().take(4).enumerate() {
                            let (x, y) = coords[idx];
                            apply_window_rect(hwnd, x, y, half_w, half_h);
                        }
                    }
                    "master_stack" | "focus_side" => {
                        if let Some(&first) = target_hwnds.first() {
                            let master_w = (wa_w * 65) / 100;
                            apply_window_rect(first, wa_x, wa_y, master_w, wa_h);
                            let rest = &target_hwnds[1..];
                            let rest_count = rest.len().max(1) as i32;
                            let stack_h = wa_h / rest_count;
                            let stack_w = wa_w - master_w;
                            for (i, &hwnd) in rest.iter().enumerate() {
                                apply_window_rect(
                                    hwnd,
                                    wa_x + master_w,
                                    wa_y + (i as i32 * stack_h),
                                    stack_w,
                                    stack_h,
                                );
                            }
                        }
                    }
                    "split_vertical" => {
                        let h = wa_h / count;
                        for (idx, &hwnd) in target_hwnds.iter().enumerate() {
                            apply_window_rect(hwnd, wa_x, wa_y + (idx as i32 * h), wa_w, h);
                        }
                    }
                    _ => {
                        if target_hwnds.len() == 2 {
                            snap_window_pair(target_hwnds[0], target_hwnds[1]);
                        } else {
                            let w = wa_w / count;
                            for (idx, &hwnd) in target_hwnds.iter().enumerate() {
                                apply_window_rect(hwnd, wa_x + (idx as i32 * w), wa_y, w, wa_h);
                            }
                        }
                    }
                }
                feedback.push(format!("Disposition en mosaïque ({layout_mode}) appliquée sur {} fenêtres.", count));
            }
            AgentAction::MoveWindow { title, x, y, width, height } => {
                let targets = find_windows_matching(title, &user_windows, preferred_target_hwnd);
                if let Some(&hwnd) = targets.first() {
                    apply_window_rect(hwnd, *x, *y, *width, *height);
                }
                feedback.push(format!("Fenêtre '{}' déplacée en ({}, {}) [{}x{}].", title, x, y, width, height));
            }
            AgentAction::AccessibilityShortcut { shortcut } => {
                const VK_LWIN: VIRTUAL_KEY = VIRTUAL_KEY(0x5B);
                const VK_CONTROL: VIRTUAL_KEY = VIRTUAL_KEY(0x11);
                const VK_SHIFT: VIRTUAL_KEY = VIRTUAL_KEY(0x10);
                const VK_MENU: VIRTUAL_KEY = VIRTUAL_KEY(0x12);
                const VK_TAB: VIRTUAL_KEY = VIRTUAL_KEY(0x09);
                const VK_HOME: VIRTUAL_KEY = VIRTUAL_KEY(0x24);
                const VK_UP: VIRTUAL_KEY = VIRTUAL_KEY(0x26);
                const VK_DOWN: VIRTUAL_KEY = VIRTUAL_KEY(0x28);
                const VK_RETURN: VIRTUAL_KEY = VIRTUAL_KEY(0x0D);
                const VK_ESCAPE: VIRTUAL_KEY = VIRTUAL_KEY(0x1B);
                const VK_OEM_PLUS: VIRTUAL_KEY = VIRTUAL_KEY(0xBB);
                const VK_OEM_MINUS: VIRTUAL_KEY = VIRTUAL_KEY(0xBD);
                const VK_OEM_PERIOD: VIRTUAL_KEY = VIRTUAL_KEY(0xBE);
                const VK_LEFT: VIRTUAL_KEY = VIRTUAL_KEY(0x25);
                const VK_RIGHT: VIRTUAL_KEY = VIRTUAL_KEY(0x27);
                const VK_F4: VIRTUAL_KEY = VIRTUAL_KEY(0x73);
                const VK_F5: VIRTUAL_KEY = VIRTUAL_KEY(0x74);
                const VK_D: VIRTUAL_KEY = VIRTUAL_KEY(0x44);
                const VK_L: VIRTUAL_KEY = VIRTUAL_KEY(0x4C);

                match shortcut.as_str() {
                    "magnifier_zoom_in" => send_hotkey(&[VK_LWIN], VK_OEM_PLUS),
                    "magnifier_zoom_out" => send_hotkey(&[VK_LWIN], VK_OEM_MINUS),
                    "magnifier_close" => send_hotkey(&[VK_LWIN], VK_ESCAPE),
                    "snap_left" => send_hotkey(&[VK_LWIN], VK_LEFT),
                    "snap_right" => send_hotkey(&[VK_LWIN], VK_RIGHT),
                    "snap_up" => send_hotkey(&[VK_LWIN], VK_UP),
                    "snap_down" => send_hotkey(&[VK_LWIN], VK_DOWN),
                    "snap_top_half" => send_hotkey(&[VK_LWIN, VK_MENU], VK_UP),
                    "snap_bottom_half" => send_hotkey(&[VK_LWIN, VK_MENU], VK_DOWN),
                    "minimize_others" => send_hotkey(&[VK_LWIN], VK_HOME),
                    "restore_window" => send_hotkey(&[VK_LWIN, VK_SHIFT], VK_DOWN),
                    "narrator_toggle" => send_hotkey(&[VK_LWIN, VK_CONTROL], VK_RETURN),
                    "color_filter_toggle" => send_hotkey(&[VK_LWIN, VK_CONTROL], VIRTUAL_KEY(0x43)),
                    "accessibility_settings" => send_hotkey(&[VK_LWIN], VIRTUAL_KEY(0x55)),
                    "clipboard_history" => send_hotkey(&[VK_LWIN], VIRTUAL_KEY(0x56)),
                    "mute_mic" => send_hotkey(&[VK_LWIN, VK_MENU], VIRTUAL_KEY(0x4B)),
                    "toggle_desktop" => send_hotkey(&[VK_LWIN], VIRTUAL_KEY(0x44)),
                    "snap_layouts" => send_hotkey(&[VK_LWIN], VIRTUAL_KEY(0x5A)),
                    "task_manager" => send_hotkey(&[VK_CONTROL, VK_SHIFT], VK_ESCAPE),
                    "snip_screenshot" => send_hotkey(&[VK_LWIN, VK_SHIFT], VIRTUAL_KEY(0x53)),
                    "action_center" => send_hotkey(&[VK_LWIN], VIRTUAL_KEY(0x41)),
                    "notification_center" => send_hotkey(&[VK_LWIN], VIRTUAL_KEY(0x4E)),
                    "task_view" => send_hotkey(&[VK_LWIN], VK_TAB),
                    "open_search" => send_hotkey(&[VK_LWIN], VIRTUAL_KEY(0x53)),
                    "open_run" => send_hotkey(&[VK_LWIN], VIRTUAL_KEY(0x52)),
                    "open_settings" => send_hotkey(&[VK_LWIN], VIRTUAL_KEY(0x49)),
                    "lock_screen" => send_hotkey(&[VK_LWIN], VIRTUAL_KEY(0x4C)),
                    "emoji_panel" => send_hotkey(&[VK_LWIN], VK_OEM_PERIOD),
                    "minimize_all" => send_hotkey(&[VK_LWIN], VIRTUAL_KEY(0x4D)),
                    "restore_minimized" => send_hotkey(&[VK_LWIN, VK_SHIFT], VIRTUAL_KEY(0x4D)),
                    "new_desktop" => send_hotkey(&[VK_LWIN, VK_CONTROL], VIRTUAL_KEY(0x44)),
                    "next_desktop" => send_hotkey(&[VK_LWIN, VK_CONTROL], VK_RIGHT),
                    "prev_desktop" => send_hotkey(&[VK_LWIN, VK_CONTROL], VK_LEFT),
                    "close_desktop" => send_hotkey(&[VK_LWIN, VK_CONTROL], VK_F4),
                    "move_window_monitor_left" => send_hotkey(&[VK_LWIN, VK_SHIFT], VK_LEFT),
                    "move_window_monitor_right" => send_hotkey(&[VK_LWIN, VK_SHIFT], VK_RIGHT),
                    "voice_typing" | "dictation" => send_hotkey(&[VK_LWIN], VIRTUAL_KEY(0x48)),
                    "file_explorer" => send_hotkey(&[VK_LWIN], VIRTUAL_KEY(0x45)),
                    "quick_link_menu" => send_hotkey(&[VK_LWIN], VIRTUAL_KEY(0x58)),
                    "project_display" => send_hotkey(&[VK_LWIN], VIRTUAL_KEY(0x50)),
                    "cast_display" => send_hotkey(&[VK_LWIN], VIRTUAL_KEY(0x4B)),
                    "screen_recording" => send_hotkey(&[VK_LWIN, VK_SHIFT], VIRTUAL_KEY(0x52)),
                    "select_all" => send_hotkey(&[VK_CONTROL], VIRTUAL_KEY(0x41)),
                    "copy" => send_hotkey(&[VK_CONTROL], VIRTUAL_KEY(0x43)),
                    "undo" => send_hotkey(&[VK_CONTROL], VIRTUAL_KEY(0x5A)),
                    "redo" => send_hotkey(&[VK_CONTROL], VIRTUAL_KEY(0x59)),
                    "find_in_page" => send_hotkey(&[VK_CONTROL], VIRTUAL_KEY(0x46)),
                    "close_tab" => send_hotkey(&[VK_CONTROL], VIRTUAL_KEY(0x57)),
                    "reopen_tab" => send_hotkey(&[VK_CONTROL, VK_SHIFT], VIRTUAL_KEY(0x54)),
                    "refresh_page" => send_hotkey(&[], VK_F5),
                    "bookmarks_bar" | "toggle_bookmarks" | "favoris" => {
                        // Ctrl + Shift + B (affiche/masque la barre de favoris)
                        send_hotkey(&[VK_CONTROL, VK_SHIFT], VIRTUAL_KEY(0x42));
                    }
                    "focus_address_bar" | "address_bar" | "select_address_bar" => {
                        send_hotkey(&[VK_MENU], VK_D);
                        std::thread::sleep(Duration::from_millis(40));
                        send_hotkey(&[VK_CONTROL], VK_L);
                    }
                    "mute_volume" | "volume_mute" | "mute_sound" | "mute_audio" => {
                        send_hotkey(&[], VIRTUAL_KEY(0xAD));
                    }
                    "volume_down" => {
                        for _ in 0..5 {
                            send_hotkey(&[], VIRTUAL_KEY(0xAE));
                            std::thread::sleep(Duration::from_millis(20));
                        }
                    }
                    "volume_up" => {
                        for _ in 0..5 {
                            send_hotkey(&[], VIRTUAL_KEY(0xAF));
                            std::thread::sleep(Duration::from_millis(20));
                        }
                    }
                    "next_field" => send_hotkey(&[], VK_TAB),
                    "previous_field" => send_hotkey(&[VK_SHIFT], VK_TAB),
                    "escape" => send_hotkey(&[], VK_ESCAPE),
                    "enter" | "press_enter" | "return" => send_hotkey(&[], VK_RETURN),
                    other => {
                        if !try_send_key_combination(other) {
                            feedback.push(format!("Raccourci système '{}' non reconnu.", other));
                            continue;
                        }
                    }
                }
                feedback.push(format!("Raccourci système '{}' envoyé.", shortcut));
            }
            AgentAction::SummarizeScreen { window } => {
                println!("[Actions] Analyse de l'écran en cours (fenêtre : {:?})", window);
                let current_summary = summarize_screen_state(window.as_deref(), true);
                let prev_summary = {
                    let mut lock = LAST_SCREEN_SUMMARY.lock().unwrap_or_else(|e| e.into_inner());
                    let prev = lock.clone();
                    *lock = Some(current_summary.clone());
                    prev
                };

                if let Some(prev) = prev_summary {
                    let comparison = format!(
                        "=== Analyse différentielle d'écran ===\n\n[État d'écran précédent] :\n{}\n\n[État d'écran actuel] :\n{}\n\n[Instruction d'analyse comparative] : Compare minutieusement les deux états fournis (fenêtre au premier plan, fenêtres ouvertes ou fermées, contenu des champs de saisie, boutons disponibles). Explique clairement les changements survenus à l'utilisateur et propose la suite d'actions la plus pertinente.",
                        truncate_with_notice(&prev, 1500),
                        truncate_with_notice(&current_summary, 1500)
                    );
                    feedback.push(comparison);
                } else {
                    feedback.push(current_summary);
                }
            }
            AgentAction::ReadEditorContent { window, target } => {
                let target_hwnd = match window.as_deref() {
                    Some(w) if !w.trim().is_empty() => {
                        find_windows_matching(w, &user_windows, active_user_hwnd).first().copied()
                    }
                    _ => preferred_target_hwnd.or(active_user_hwnd),
                };

                if let Some(hwnd) = target_hwnd {
                    if is_window_hung(hwnd) {
                        feedback.push(format!("Fenêtre cible [HWND {:?}] non réactive (sablier). Lecture impossible.", hwnd.0));
                        continue;
                    }

                    let win_title = user_windows
                        .iter()
                        .find(|(h, _)| *h == hwnd)
                        .map(|(_, t)| t.as_str())
                        .unwrap_or("Éditeur");

                    if is_terminal_window(hwnd, win_title) {
                        if let Some(content) = read_terminal_window_content(hwnd, win_title) {
                            let trimmed = content.trim();
                            let char_count = trimmed.chars().count();
                            feedback.push(format!(
                                "[Contenu extrait du terminal '{}' ({} caractères)] :\n```\n{}\n```",
                                win_title,
                                char_count,
                                truncate_with_notice(trimmed, 10000)
                            ));
                            continue;
                        } else {
                            feedback.push(format!("'{}' est un terminal : lecture automatique impossible sans buffer accessible.", win_title));
                            continue;
                        }
                    }

                    let (_pid, exe_name) = get_window_process_info(hwnd);
                    let exe_lower = exe_name.to_lowercase();
                    let title_lower = win_title.to_lowercase();
                    let is_jetbrains = exe_lower.contains("idea")
                        || exe_lower.contains("rustrover")
                        || exe_lower.contains("clion")
                        || exe_lower.contains("pycharm")
                        || exe_lower.contains("rider")
                        || exe_lower.contains("webstorm")
                        || exe_lower.contains("goland")
                        || exe_lower.contains("phpstorm")
                        || exe_lower.contains("rubymine")
                        || title_lower.contains("intellij")
                        || title_lower.contains("rustrover");
                    let is_vscode_or_similar = exe_lower.contains("code")
                        || exe_lower.contains("vscodium")
                        || exe_lower.contains("cursor")
                        || exe_lower.contains("sublime")
                        || exe_lower.contains("zed");

                    force_foreground_window(hwnd);
                    unsafe {
                        let _ = ShowWindow(hwnd, SW_RESTORE);
                    }
                    std::thread::sleep(Duration::from_millis(80));

                    // Résolution de la cible de fichier : explicite ou déduite du titre de la fenêtre
                    let effective_target = target.clone().or_else(|| extract_filename_from_title(win_title));

                    // Si un onglet ou élément précis est ciblé (ex: "main.rs", "config.json")
                    let mut tab_clicked = false;
                    if let Some(ref target_name) = effective_target.as_deref().filter(|s| !s.trim().is_empty()) {
                        let elements = list_interactive_elements(hwnd);
                        // Chercher en priorité absolue un onglet d'éditeur (tab / onglet)
                        let tab_elem = elements.iter().find(|e| {
                            let is_tab_ctrl = e.is_tab
                                || e.localized_type.contains("tab")
                                || e.localized_type.contains("onglet")
                                || e.class_name.contains("tab")
                                || e.automation_id.to_lowercase().contains("tab");
                            if is_tab_ctrl {
                                let name_lower = e.name.to_lowercase();
                                let target_lower = target_name.to_lowercase();
                                name_lower.contains(&target_lower) || target_lower.contains(&name_lower)
                            } else {
                                false
                            }
                        });

                        if let Some(elem) = tab_elem {
                            println!("[Actions] Sélection de l'onglet éditeur '{}' [class={}] avant lecture", elem.name, elem.class_name);
                            unsafe { let _ = elem.element.SetFocus(); }
                            click_element(elem.click_x, elem.click_y, Some(&elem.element), elem.pattern.as_ref());
                            std::thread::sleep(Duration::from_millis(150));
                            // Clic juste sous l'onglet pour focaliser le panneau d'édition de code associé
                            let body_x = elem.click_x;
                            let body_y = elem.rect.bottom + 40;
                            unsafe {
                                let _ = SetCursorPos(body_x, body_y);
                            }
                            click_element(body_x, body_y, None, None);
                            tab_clicked = true;
                        } else if is_jetbrains || is_vscode_or_similar {
                            println!("[Actions] Onglet non trouvé via UIA dans {}, tentative via raccourci de recherche de fichier pour '{}'", win_title, target_name);
                            const VK_CONTROL: VIRTUAL_KEY = VIRTUAL_KEY(0x11);
                            const VK_SHIFT: VIRTUAL_KEY = VIRTUAL_KEY(0x10);
                            const VK_N: VIRTUAL_KEY = VIRTUAL_KEY(0x4E);
                            const VK_P: VIRTUAL_KEY = VIRTUAL_KEY(0x50);
                            const VK_RETURN: VIRTUAL_KEY = VIRTUAL_KEY(0x0D);

                            if is_jetbrains {
                                send_hotkey(&[VK_CONTROL, VK_SHIFT], VK_N);
                            } else {
                                send_hotkey(&[VK_CONTROL], VK_P);
                            }
                            std::thread::sleep(Duration::from_millis(250));
                            clipboard::set_text(target_name);
                            std::thread::sleep(Duration::from_millis(40));
                            send_paste();
                            std::thread::sleep(Duration::from_millis(300));
                            send_hotkey(&[], VK_RETURN);
                            std::thread::sleep(Duration::from_millis(350));
                            tab_clicked = true;
                        } else if let Some(elem) = find_best_element(&elements, target_name) {
                            println!("[Actions] Sélection de l'élément de repli '{}' avant lecture", target_name);
                            unsafe { let _ = elem.element.SetFocus(); }
                            click_element(elem.click_x, elem.click_y, Some(&elem.element), elem.pattern.as_ref());
                            std::thread::sleep(Duration::from_millis(100));
                        }
                    }

                    // Focus sur la zone d'édition principale
                    if !tab_clicked {
                        if let Some(elem) = find_largest_textarea(hwnd) {
                            unsafe { let _ = elem.element.SetFocus(); }
                            click_element(elem.click_x, elem.click_y, Some(&elem.element), elem.pattern.as_ref());
                            std::thread::sleep(Duration::from_millis(60));
                        } else {
                            // Clic au centre de la fenêtre pour donner le focus au canevas Swing/IntelliJ
                            let mut r = RECT::default();
                            if unsafe { GetWindowRect(hwnd, &mut r).is_ok() } {
                                let center_x = r.left + (r.right - r.left) / 2;
                                let center_y = r.top + (r.bottom - r.top) / 2;
                                unsafe { let _ = SetCursorPos(center_x, center_y); }
                                click_element(center_x, center_y, None, None);
                                std::thread::sleep(Duration::from_millis(100));
                            }
                        }
                    } else if !has_focused_textarea(hwnd) {
                        refocus_largest_textarea_or_fallback(hwnd);
                        std::thread::sleep(Duration::from_millis(60));
                    }

                    const VK_CONTROL: VIRTUAL_KEY = VIRTUAL_KEY(0x11);
                    const VK_A: VIRTUAL_KEY = VIRTUAL_KEY(0x41);
                    const VK_C: VIRTUAL_KEY = VIRTUAL_KEY(0x43);
                    const VK_LEFT: VIRTUAL_KEY = VIRTUAL_KEY(0x25);

                    send_hotkey(&[VK_CONTROL], VK_A);
                    std::thread::sleep(Duration::from_millis(70));
                    send_hotkey(&[VK_CONTROL], VK_C);
                    std::thread::sleep(Duration::from_millis(90));
                    // Désélection douce pour laisser l'éditeur dans son état normal
                    send_hotkey(&[], VK_LEFT);

                    // Repli fichier disque si target correspond à un fichier local existant
                    let local_file_content = effective_target.as_deref().and_then(find_file_in_workspace);

                    let mut content_captured = false;
                    if let Some(content) = clipboard::get_text() {
                        let trimmed = content.trim();
                        if !trimmed.is_empty() {
                            let char_count = content.chars().count();
                            println!("[Actions] Contenu lu depuis '{}' ({} caractères)", win_title, char_count);
                            feedback.push(format!(
                                "[Contenu extrait du fichier/éditeur '{}' ({} caractères)] :\n```\n{}\n```",
                                win_title,
                                char_count,
                                truncate_with_notice(&content, 10000)
                            ));
                            content_captured = true;
                        }
                    }

                    if !content_captured {
                        if let Some((path_str, content)) = local_file_content {
                            let char_count = content.chars().count();
                            println!("[Actions] Repli lecture directe depuis le fichier disque '{}' ({} caractères)", path_str, char_count);
                            feedback.push(format!(
                                "[Contenu extrait du fichier disque '{}' ({} caractères)] :\n```\n{}\n```",
                                path_str,
                                char_count,
                                truncate_with_notice(&content, 10000)
                            ));
                        } else {
                            feedback.push(format!("Le document ou champ dans '{}' est vide.", win_title));
                        }
                    }
                } else {
                    feedback.push("Aucune fenêtre d'éditeur trouvée pour lire le fichier.".to_string());
                }
            }
            AgentAction::ActivateImmersion { apps, urls, layout } => {
                IS_IMMERSION_ACTIVE.store(true, Ordering::SeqCst);
                if let Ok(mut hist) = IMMERSION_SCREEN_HISTORY.lock() {
                    hist.clear();
                }
                println!("[Actions] Activation du mode immersion (apps: {:?}, urls: {:?}, layout: {:?})", apps, urls, layout);

                let initial_summary = summarize_screen_state(None, false);
                feedback.push(format!("Analyse pré-immersion :\n{}", truncate_with_notice(&initial_summary, 600)));

                const VK_LWIN: VIRTUAL_KEY = VIRTUAL_KEY(0x5B);
                const VK_CONTROL: VIRTUAL_KEY = VIRTUAL_KEY(0x11);
                const VK_D: VIRTUAL_KEY = VIRTUAL_KEY(0x44);
                send_hotkey(&[VK_LWIN, VK_CONTROL], VK_D);
                std::thread::sleep(Duration::from_millis(500));

                let mut spawned_windows = Vec::new();
                for app in apps {
                    let app_trimmed = app.trim();
                    if app_trimmed.is_empty() {
                        continue;
                    }
                    let before_windows = list_user_windows();
                    let before_hwnds: std::collections::HashSet<isize> = before_windows.iter().map(|(h, _)| h.0 as isize).collect();

                    let launched = if let Some(exe_path) = find_executable_in_path(app_trimmed) {
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
                        let mut cmd = std::process::Command::new(app_trimmed);
                        cmd.creation_flags(CREATE_NEW_CONSOLE);
                        cmd.spawn().is_ok()
                    };

                    if launched {
                        for _ in 0..20 {
                            if is_request_cancelled(request_id) { break; }
                            std::thread::sleep(Duration::from_millis(100));
                            let after_windows = list_user_windows();
                            if let Some((h, _)) = after_windows.iter().find(|(h, _)| !before_hwnds.contains(&(h.0 as isize))) {
                                spawned_windows.push(*h);
                                break;
                            }
                        }
                    }
                }

                for url in urls {
                    let url_trimmed = url.trim();
                    if url_trimmed.is_empty() {
                        continue;
                    }
                    let before_windows = list_user_windows();
                    let before_hwnds: std::collections::HashSet<isize> = before_windows.iter().map(|(h, _)| h.0 as isize).collect();

                    launch_browser_new_window(url_trimmed);

                    for _ in 0..20 {
                        if is_request_cancelled(request_id) { break; }
                        std::thread::sleep(Duration::from_millis(100));
                        let after_windows = list_user_windows();
                        if let Some((h, _)) = after_windows.iter().find(|(h, _)| !before_hwnds.contains(&(h.0 as isize))) {
                            spawned_windows.push(*h);
                            break;
                        }
                    }
                }

                let layout_mode = layout.as_deref().unwrap_or("split_horizontal");
                let current_windows = list_user_windows();
                let mut target_hwnds: Vec<HWND> = spawned_windows;
                for (h, _) in &current_windows {
                    if !target_hwnds.contains(h) {
                        target_hwnds.push(*h);
                    }
                    if target_hwnds.len() >= 4 {
                        break;
                    }
                }

                let count = target_hwnds.len().max(1) as i32;
                if count >= 2 {
                    match layout_mode {
                        "grid" | "quad" | "grid_2x2" => {
                            let half_w = wa_w / 2;
                            let half_h = wa_h / 2;
                            let ordered_quads = classify_immersion_quadrants(&target_hwnds, &current_windows);
                            let coords = [
                                (wa_x, wa_y),
                                (wa_x + half_w, wa_y),
                                (wa_x, wa_y + half_h),
                                (wa_x + half_w, wa_y + half_h),
                            ];
                            for (idx, opt_h) in ordered_quads.iter().enumerate() {
                                if let Some(h) = opt_h {
                                    let (x, y) = coords[idx];
                                    apply_window_rect(*h, x, y, half_w, half_h);
                                }
                            }
                        }
                        "master_stack" | "focus_side" => {
                            if let Some(&first) = target_hwnds.first() {
                                let master_w = (wa_w * 65) / 100;
                                apply_window_rect(first, wa_x, wa_y, master_w, wa_h);
                                let rest = &target_hwnds[1..];
                                let rest_count = rest.len().max(1) as i32;
                                let stack_h = wa_h / rest_count;
                                let stack_w = wa_w - master_w;
                                for (i, &h) in rest.iter().enumerate() {
                                    apply_window_rect(
                                        h,
                                        wa_x + master_w,
                                        wa_y + (i as i32 * stack_h),
                                        stack_w,
                                        stack_h,
                                    );
                                }
                            }
                        }
                        _ => {
                            if target_hwnds.len() == 2 {
                                snap_window_pair(target_hwnds[0], target_hwnds[1]);
                            } else {
                                let w = wa_w / count;
                                for (idx, &h) in target_hwnds.iter().enumerate() {
                                    apply_window_rect(h, wa_x + (idx as i32 * w), wa_y, w, wa_h);
                                }
                            }
                        }
                    }
                }

                std::thread::sleep(Duration::from_millis(200));
                let post_summary = summarize_screen_state(None, false);
                if let Ok(mut hist) = IMMERSION_SCREEN_HISTORY.lock() {
                    hist.push(post_summary.clone());
                }
                feedback.push(format!("Mode immersion actif (bureau virtuel créé, disposition : {}). Nouvel état d'écran :\n{}", layout_mode, truncate_with_notice(&post_summary, 800)));
            }
            AgentAction::DeactivateImmersion => {
                IS_IMMERSION_ACTIVE.store(false, Ordering::SeqCst);
                if let Ok(mut hist) = IMMERSION_SCREEN_HISTORY.lock() {
                    hist.clear();
                }
                println!("[Actions] Désactivation du mode immersion");
                const VK_LWIN: VIRTUAL_KEY = VIRTUAL_KEY(0x5B);
                const VK_CONTROL: VIRTUAL_KEY = VIRTUAL_KEY(0x11);
                const VK_F4: VIRTUAL_KEY = VIRTUAL_KEY(0x73);
                send_hotkey(&[VK_LWIN, VK_CONTROL], VK_F4);
                std::thread::sleep(Duration::from_millis(300));
                let current_summary = summarize_screen_state(None, false);
                feedback.push(format!("Mode immersion désactivé : bureau virtuel fermé. Retour à l'espace initial :\n{}", truncate_with_notice(&current_summary, 600)));
            }
            AgentAction::Dictate { text, window, target, press_enter } => {
                let target_desc_opt = target.as_deref().filter(|s| !s.trim().is_empty());
                let target_hwnd = match window.as_deref() {
                    Some(w) if !w.trim().is_empty() => {
                        find_windows_matching(w, &user_windows, active_user_hwnd).first().copied()
                    }
                    _ => {
                        let ext_h = LAST_EXTERNAL_HWND.lock().ok().and_then(|g| *g).map(|raw| HWND(raw as _));
                        ext_h.filter(|h| !h.0.is_null() && !is_window_hung(*h))
                            .or(active_user_hwnd)
                            .or(preferred_target_hwnd)
                    }
                };

                let should_enter = press_enter.unwrap_or(false);

                if let Some(hwnd) = target_hwnd {
                    if is_window_hung(hwnd) {
                        feedback.push(format!("Fenêtre [HWND {:?}] non réactive. Dictée ignorée.", hwnd.0));
                        continue;
                    }
                    let win_title = user_windows
                        .iter()
                        .find(|(h, _)| *h == hwnd)
                        .map(|(_, t)| t.as_str())
                        .unwrap_or("");

                    restore_external_focus_target(Some(hwnd));
                    release_modifier_keys();
                    std::thread::sleep(Duration::from_millis(40));

                    if is_terminal_window(hwnd, win_title) {
                        send_terminal_input(hwnd, text, should_enter);
                        feedback.push(format!("Texte dicté inséré dans le terminal '{}'.", win_title));
                    } else {
                        if let Some(target_desc) = target_desc_opt {
                            let elements = list_interactive_elements(hwnd);
                            if let Some(elem) = find_best_input_element(&elements, target_desc) {
                                unsafe { let _ = elem.element.SetFocus(); }
                                click_element(elem.click_x, elem.click_y, Some(&elem.element), elem.pattern.as_ref());
                                std::thread::sleep(Duration::from_millis(50));
                            }
                        } else if window.is_some() && !has_focused_textarea(hwnd) {
                            refocus_largest_textarea_or_fallback(hwnd);
                            std::thread::sleep(Duration::from_millis(40));
                        }

                        let text_to_type = if !text.ends_with(' ') && !text.ends_with('\n') {
                            format!("{text} ")
                        } else {
                            text.to_string()
                        };

                        for c in text_to_type.chars() {
                            if c == '\n' || c == '\r' {
                                send_hotkey(&[], VIRTUAL_KEY(0x0D));
                                std::thread::sleep(Duration::from_millis(15));
                            } else {
                                send_char_unicode(c);
                                std::thread::sleep(Duration::from_millis(2));
                            }
                        }
                        if should_enter {
                            std::thread::sleep(Duration::from_millis(30));
                            send_hotkey(&[], VIRTUAL_KEY(0x0D));
                        }
                        feedback.push(format!("Texte dicté inséré directement au clavier ({} caractères).", text.len()));
                    }
                } else {
                    let _ = write_to_browser_or_txt(text);
                    feedback.push(format!("Texte dicté via repli système ({} caractères).", text.len()));
                }
            }
            AgentAction::CursorMove { command } => {
                let target_hwnd = LAST_EXTERNAL_HWND.lock().ok().and_then(|g| *g).map(|raw| HWND(raw as _))
                    .filter(|h| !h.0.is_null() && !is_window_hung(*h))
                    .or(active_user_hwnd)
                    .or(preferred_target_hwnd);

                if let Some(hwnd) = target_hwnd {
                    restore_external_focus_target(Some(hwnd));
                    release_modifier_keys();
                    std::thread::sleep(Duration::from_millis(40));
                }

                let cmd_lower = command.to_lowercase();
                const VK_RETURN: VIRTUAL_KEY = VIRTUAL_KEY(0x0D);
                const VK_LEFT: VIRTUAL_KEY = VIRTUAL_KEY(0x25);
                const VK_UP: VIRTUAL_KEY = VIRTUAL_KEY(0x26);
                const VK_RIGHT: VIRTUAL_KEY = VIRTUAL_KEY(0x27);
                const VK_DOWN: VIRTUAL_KEY = VIRTUAL_KEY(0x28);
                const VK_HOME: VIRTUAL_KEY = VIRTUAL_KEY(0x24);
                const VK_END: VIRTUAL_KEY = VIRTUAL_KEY(0x23);
                const VK_CONTROL: VIRTUAL_KEY = VIRTUAL_KEY(0x11);
                const VK_SHIFT: VIRTUAL_KEY = VIRTUAL_KEY(0x10);
                const VK_BACK: VIRTUAL_KEY = VIRTUAL_KEY(0x08);
                const VK_SPACE: VIRTUAL_KEY = VIRTUAL_KEY(0x20);
                const VK_TAB: VIRTUAL_KEY = VIRTUAL_KEY(0x09);
                const VK_A: VIRTUAL_KEY = VIRTUAL_KEY(0x41);

                match cmd_lower.as_str() {
                    "newline" | "a_la_ligne" | "ligne_suivante" | "enter" => {
                        send_hotkey(&[], VK_RETURN);
                        feedback.push("Curseur : à la ligne.".to_string());
                    }
                    "new_paragraph" | "nouveau_paragraphe" => {
                        send_hotkey(&[], VK_RETURN);
                        std::thread::sleep(Duration::from_millis(25));
                        send_hotkey(&[], VK_RETURN);
                        feedback.push("Curseur : nouveau paragraphe.".to_string());
                    }
                    "left" | "gauche" => {
                        send_hotkey(&[], VK_LEFT);
                        feedback.push("Curseur : vers la gauche.".to_string());
                    }
                    "right" | "droite" => {
                        send_hotkey(&[], VK_RIGHT);
                        feedback.push("Curseur : vers la droite.".to_string());
                    }
                    "up" | "haut" => {
                        send_hotkey(&[], VK_UP);
                        feedback.push("Curseur : vers le haut.".to_string());
                    }
                    "down" | "bas" => {
                        send_hotkey(&[], VK_DOWN);
                        feedback.push("Curseur : vers le bas.".to_string());
                    }
                    "line_start" | "debut_ligne" => {
                        send_hotkey(&[], VK_HOME);
                        feedback.push("Curseur : début de ligne.".to_string());
                    }
                    "line_end" | "fin_ligne" => {
                        send_hotkey(&[], VK_END);
                        feedback.push("Curseur : fin de ligne.".to_string());
                    }
                    "doc_start" | "debut_doc" => {
                        send_hotkey(&[VK_CONTROL], VK_HOME);
                        feedback.push("Curseur : début du document.".to_string());
                    }
                    "doc_end" | "fin_doc" => {
                        send_hotkey(&[VK_CONTROL], VK_END);
                        feedback.push("Curseur : fin du document.".to_string());
                    }
                    "backspace" | "efface" | "supprime" => {
                        send_hotkey(&[], VK_BACK);
                        feedback.push("Effacement caractère précédent.".to_string());
                    }
                    "delete_word" | "efface_mot" => {
                        send_hotkey(&[VK_CONTROL], VK_BACK);
                        feedback.push("Effacement du mot précédent.".to_string());
                    }
                    "delete_line" | "efface_ligne" => {
                        send_hotkey(&[VK_SHIFT], VK_HOME);
                        std::thread::sleep(Duration::from_millis(20));
                        send_hotkey(&[], VK_BACK);
                        feedback.push("Effacement de la ligne.".to_string());
                    }
                    "select_all" | "tout_selectionner" => {
                        send_hotkey(&[VK_CONTROL], VK_A);
                        feedback.push("Tout sélectionner.".to_string());
                    }
                    "space" | "espace" => {
                        send_hotkey(&[], VK_SPACE);
                        feedback.push("Espace inséré.".to_string());
                    }
                    "tab" | "tabulation" => {
                        send_hotkey(&[], VK_TAB);
                        feedback.push("Tabulation.".to_string());
                    }
                    _ => {
                        feedback.push(format!("Commande curseur '{command}' inconnue."));
                    }
                }
            }
            AgentAction::ActivateDictation { window, target: _ } => {
                IS_DICTATION_ACTIVE.store(true, Ordering::SeqCst);
                if let Some(w) = window {
                    let targets = find_windows_matching(w, &user_windows, active_user_hwnd);
                    if let Some(&h) = targets.first() {
                        force_foreground_window(h);
                        unsafe { let _ = ShowWindow(h, SW_RESTORE); }
                    }
                }
                feedback.push("Mode dictée vocale activé.".to_string());
            }
            AgentAction::DeactivateDictation => {
                IS_DICTATION_ACTIVE.store(false, Ordering::SeqCst);
                feedback.push("Mode dictée désactivé.".to_string());
            }
            AgentAction::ToggleDictation { window } => {
                let new_state = !IS_DICTATION_ACTIVE.load(Ordering::SeqCst);
                IS_DICTATION_ACTIVE.store(new_state, Ordering::SeqCst);
                if new_state {
                    if let Some(w) = window {
                        let targets = find_windows_matching(w, &user_windows, active_user_hwnd);
                        if let Some(&h) = targets.first() {
                            force_foreground_window(h);
                            unsafe { let _ = ShowWindow(h, SW_RESTORE); }
                        }
                    }
                    feedback.push("Mode dictée vocale activé.".to_string());
                } else {
                    feedback.push("Mode dictée vocale désactivé.".to_string());
                }
            }
            AgentAction::SelectTextBetween { from, to, window, target } => {
                let target_hwnd = match window.as_deref() {
                    Some(w) if !w.trim().is_empty() => {
                        find_windows_matching(w, &user_windows, active_user_hwnd).first().copied()
                    }
                    _ => active_user_hwnd.or(preferred_target_hwnd),
                };

                if let Some(hwnd) = target_hwnd {
                    if is_window_hung(hwnd) {
                        feedback.push(format!("Fenêtre [HWND {:?}] non réactive. Sélection ignorée.", hwnd.0));
                        continue;
                    }
                    force_foreground_window(hwnd);
                    unsafe { let _ = ShowWindow(hwnd, SW_RESTORE); }
                    std::thread::sleep(Duration::from_millis(60));

                    if let Some(t_desc) = target.as_deref().filter(|s| !s.trim().is_empty()) {
                        let elements = list_interactive_elements(hwnd);
                        if let Some(elem) = find_best_input_element(&elements, t_desc) {
                            unsafe { let _ = elem.element.SetFocus(); }
                            click_element(elem.click_x, elem.click_y, Some(&elem.element), elem.pattern.as_ref());
                            std::thread::sleep(Duration::from_millis(40));
                        }
                    } else if !has_focused_textarea(hwnd) {
                        refocus_largest_textarea_or_fallback(hwnd);
                        std::thread::sleep(Duration::from_millis(40));
                    }

                    // 1. Tenter la sélection via UIA TextPattern si supporté
                    let mut selected_via_uia = false;
                    unsafe {
                        let _ = CoInitializeEx(None, COINIT_MULTITHREADED);
                        if let Ok(uia) = CoCreateInstance::<_, IUIAutomation>(&CUIAutomation, None, CLSCTX_INPROC_SERVER) {
                            if let Ok(focused) = uia.GetFocusedElement() {
                                if let Ok(p_unk) = focused.GetCurrentPattern(UIA_TextPatternId) {
                                    if let Ok(text_pattern) = p_unk.cast::<IUIAutomationTextPattern>() {
                                        if let Ok(doc_range) = text_pattern.DocumentRange() {
                                            let from_bstr = windows::core::BSTR::from(from.as_str());
                                            let to_bstr = windows::core::BSTR::from(to.as_str());
                                            if let Ok(from_range) = doc_range.FindText(&from_bstr, false, false) {
                                                if let Ok(to_range) = doc_range.FindText(&to_bstr, false, false) {
                                                    from_range.MoveEndpointByRange(
                                                        windows::Win32::UI::Accessibility::TextPatternRangeEndpoint_End,
                                                        &to_range,
                                                        windows::Win32::UI::Accessibility::TextPatternRangeEndpoint_End,
                                                    ).ok();
                                                    if from_range.Select().is_ok() {
                                                        selected_via_uia = true;
                                                    }
                                                }
                                            }
                                        }
                                    }
                                }
                            }
                        }
                    }

                    // 2. Repli clavier universel avec recherche (Ctrl+F)
                    if !selected_via_uia {
                        const VK_CONTROL: VIRTUAL_KEY = VIRTUAL_KEY(0x11);
                        const VK_F: VIRTUAL_KEY = VIRTUAL_KEY(0x46);
                        const VK_ESCAPE: VIRTUAL_KEY = VIRTUAL_KEY(0x1B);
                        const VK_RETURN: VIRTUAL_KEY = VIRTUAL_KEY(0x0D);

                        send_hotkey(&[VK_CONTROL], VK_F);
                        std::thread::sleep(Duration::from_millis(150));
                        clipboard::set_text(from);
                        send_paste();
                        std::thread::sleep(Duration::from_millis(80));
                        send_hotkey(&[], VK_RETURN);
                        std::thread::sleep(Duration::from_millis(50));
                        send_hotkey(&[], VK_ESCAPE);
                    }
                    feedback.push(format!("Plage de texte entre \"{from}\" et \"{to}\" ciblée avec succès."));
                }
            }
            AgentAction::EnableAutostart => {
                match set_autostart(true) {
                    Ok(msg) => feedback.push(msg),
                    Err(err) => feedback.push(format!("Erreur lors de l'activation du démarrage auto : {err}")),
                }
            }
            AgentAction::DisableAutostart => {
                match set_autostart(false) {
                    Ok(msg) => feedback.push(msg),
                    Err(err) => feedback.push(format!("Erreur lors de la désactivation du démarrage auto : {err}")),
                }
            }
            AgentAction::CheckAutostart => {
                let enabled = is_autostart_enabled();
                if enabled {
                    feedback.push("Statut : Libertide est configuré pour se lancer au démarrage de Windows.".to_string());
                } else {
                    feedback.push("Statut : Libertide n'est pas configuré au démarrage de Windows.".to_string());
                }
            }
            AgentAction::SetAutostart { enabled } => {
                match set_autostart(*enabled) {
                    Ok(msg) => feedback.push(msg),
                    Err(err) => feedback.push(format!("Erreur configuration démarrage auto : {err}")),
                }
            }
            AgentAction::SetReasoningPasses { passes, reason } => {
                let clamped = (*passes).clamp(1, 15);
                TARGET_REASONING_PASSES.store(clamped, Ordering::SeqCst);
                let reason_desc = reason.as_deref().unwrap_or("ajustement au contexte d'urgence");
                println!("[Actions] Quota de passes ajusté à {} (motif: {})", clamped, reason_desc);
                feedback.push(format!("Budget de raisonnement configuré à {} passes ({reason_desc}).", clamped));
            }
            AgentAction::SchedulePrompt { delay_seconds, prompt } => {
                let dur = Duration::from_secs((*delay_seconds).max(1));
                schedule_prompt_execution(dur, prompt.clone());
                crate::play_earcon(crate::Earcon::Success);
                let friendly = format_duration_friendly(dur);
                println!("[Actions] Commande différée planifiée dans {} : \"{}\"", friendly, prompt);
                feedback.push(format!("Commande différée planifiée dans {friendly} : « {prompt} »."));
            }
            AgentAction::SchedulePrompts { tasks } => {
                let mut count = 0;
                for t in tasks {
                    if !t.prompt.trim().is_empty() {
                        let dur = Duration::from_secs(t.delay_seconds.max(1));
                        schedule_prompt_execution(dur, t.prompt.clone());
                        count += 1;
                    }
                }
                if count > 0 {
                    crate::play_earcon(crate::Earcon::Success);
                    println!("[Actions] {} commande(s) différée(s) planifiée(s).", count);
                    feedback.push(format!("{count} commande(s) différée(s) planifiée(s)."));
                }
            }
            AgentAction::CancelScheduledTasks => {
                let count = cancel_all_scheduled_tasks();
                crate::play_earcon(crate::Earcon::Success);
                if count > 0 {
                    println!("[Actions] {count} commande(s) différée(s) annulée(s).");
                    feedback.push(format!("Toutes les commandes différées ont été annulées ({count} annulée{}).", if count > 1 { "s" } else { "" }));
                } else {
                    println!("[Actions] Aucune commande différée active à annuler.");
                    feedback.push("Aucune commande différée active à annuler.".to_string());
                }
            }
            AgentAction::MuteAudio => {
                // Stoppe immédiatement la lecture multimédia (Spotify, YouTube, navigateurs)
                const VK_MEDIA_STOP: VIRTUAL_KEY = VIRTUAL_KEY(0xB2);
                send_hotkey(&[], VK_MEDIA_STOP);

                // Isole le son en rendant muettes toutes les sessions audio tierces dans le mélangeur Windows
                let muted = unsafe { mute_other_audio_sessions() };
                println!("[Actions] Flux audio tiers coupés ({muted} application(s) mise(s) en sourdine). Voix de Libertide préservée.");
                feedback.push(format!("Flux audio externes mis en sourdine ({muted} application(s)). Voix de Libertide maintenue active."));
            }
            AgentAction::SetVolume { command } => {
                const VK_VOLUME_MUTE: VIRTUAL_KEY = VIRTUAL_KEY(0xAD);
                const VK_VOLUME_DOWN: VIRTUAL_KEY = VIRTUAL_KEY(0xAE);
                const VK_VOLUME_UP: VIRTUAL_KEY = VIRTUAL_KEY(0xAF);

                let cmd_lower = command.to_lowercase();
                match cmd_lower.as_str() {
                    "mute" | "couper" | "silence" | "off" => {
                        send_hotkey(&[], VK_VOLUME_MUTE);
                        println!("[Actions] Son système coupé via VK_VOLUME_MUTE.");
                        feedback.push("Son système coupé (muet).".to_string());
                    }
                    "unmute" | "reactiver" | "on" => {
                        send_hotkey(&[], VK_VOLUME_MUTE);
                        println!("[Actions] Son système réactivé via VK_VOLUME_MUTE.");
                        feedback.push("Son système réactivé.".to_string());
                    }
                    "down" | "baisser" | "diminuer" => {
                        for _ in 0..5 {
                            send_hotkey(&[], VK_VOLUME_DOWN);
                            std::thread::sleep(Duration::from_millis(20));
                        }
                        println!("[Actions] Volume système diminué.");
                        feedback.push("Volume système diminué.".to_string());
                    }
                    "up" | "monter" | "augmenter" => {
                        for _ in 0..5 {
                            send_hotkey(&[], VK_VOLUME_UP);
                            std::thread::sleep(Duration::from_millis(20));
                        }
                        println!("[Actions] Volume système augmenté.");
                        feedback.push("Volume système augmenté.".to_string());
                    }
                    "min" | "minimum" | "zero" => {
                        for _ in 0..50 {
                            send_hotkey(&[], VK_VOLUME_DOWN);
                            std::thread::sleep(Duration::from_millis(10));
                        }
                        println!("[Actions] Volume système mis à zéro.");
                        feedback.push("Volume système mis au minimum.".to_string());
                    }
                    _ => {
                        send_hotkey(&[], VK_VOLUME_MUTE);
                        feedback.push(format!("Commande de volume '{command}' appliquée."));
                    }
                }
            }
            AgentAction::OpenFile { path } => {
                match open_path_with_default_app(path) {
                    Ok(_) => feedback.push(format!("Fichier '{path}' ouvert dans son application par défaut.")),
                    Err(err) => feedback.push(format!("Impossible d'ouvrir le fichier '{path}' : {err}")),
                }
            }
            AgentAction::ShowFloatingCard { title, .. } => {
                feedback.push(format!("Fenêtre volante '{title}' affichée à l'écran."));
            }
            AgentAction::CloseFloatingCard { id } => {
                feedback.push(format!("Fenêtre volante '{id}' fermée."));
            }
        }
    }

    if feedback.is_empty() {
        "Actions système exécutées avec succès.".to_string()
    } else {
        truncate_with_notice(&feedback.join("\n"), 12000)
    }
}

#[cfg(windows)]
unsafe fn mute_other_audio_sessions() -> usize {
    use std::ffi::c_void;
    use windows::core::GUID;

    type HRESULT = i32;
    const S_OK: HRESULT = 0;
    const CLSCTX_ALL: u32 = 23;

    extern "system" {
        fn CoCreateInstance(
            rclsid: *const GUID,
            p_unk_outer: *mut c_void,
            dw_cls_context: u32,
            riid: *const GUID,
            ppv: *mut *mut c_void,
        ) -> HRESULT;
    }

    let _ = CoInitializeEx(None, COINIT_MULTITHREADED);

    let clsid_mm_device_enumerator = GUID::from_u128(0xBCDE0395_E52F_467C_8E3D_C4579291692E);
    let iid_imm_device_enumerator = GUID::from_u128(0xA95664D2_9614_4D35_A746_DE8DB63617E6);
    let iid_audio_session_manager2 = GUID::from_u128(0x77AA99A0_1BD6_484F_8BC7_2C654C9A9B6F);
    let iid_audio_session_control2 = GUID::from_u128(0xBFB7FF88_7239_4FC9_8FA8_880C73E2E7EC);
    let iid_simple_audio_volume = GUID::from_u128(0x87CE5498_68D6_44E5_AC21_AB2709778022);

    let mut enumerator: *mut c_void = std::ptr::null_mut();
    if CoCreateInstance(
        &clsid_mm_device_enumerator,
        std::ptr::null_mut(),
        CLSCTX_ALL,
        &iid_imm_device_enumerator,
        &mut enumerator,
    ) != S_OK || enumerator.is_null() {
        return 0;
    }

    let vtable_enum = *(enumerator as *const *const usize);
    let get_default_endpoint: unsafe extern "system" fn(*mut c_void, i32, i32, *mut *mut c_void) -> HRESULT =
        std::mem::transmute(*vtable_enum.add(4));
    let release_enum: unsafe extern "system" fn(*mut c_void) -> u32 =
        std::mem::transmute(*vtable_enum.add(2));

    let mut device: *mut c_void = std::ptr::null_mut();
    let hr = get_default_endpoint(enumerator, 0, 1, &mut device); // eRender, eMultimedia
    release_enum(enumerator);

    if hr != S_OK || device.is_null() {
        return 0;
    }

    let vtable_dev = *(device as *const *const usize);
    let activate: unsafe extern "system" fn(*mut c_void, *const GUID, u32, *mut c_void, *mut *mut c_void) -> HRESULT =
        std::mem::transmute(*vtable_dev.add(3));
    let release_dev: unsafe extern "system" fn(*mut c_void) -> u32 =
        std::mem::transmute(*vtable_dev.add(2));

    let mut session_mgr: *mut c_void = std::ptr::null_mut();
    let hr = activate(device, &iid_audio_session_manager2, CLSCTX_ALL, std::ptr::null_mut(), &mut session_mgr);
    release_dev(device);

    if hr != S_OK || session_mgr.is_null() {
        return 0;
    }

    let vtable_mgr = *(session_mgr as *const *const usize);
    let get_session_enum: unsafe extern "system" fn(*mut c_void, *mut *mut c_void) -> HRESULT =
        std::mem::transmute(*vtable_mgr.add(5));
    let release_mgr: unsafe extern "system" fn(*mut c_void) -> u32 =
        std::mem::transmute(*vtable_mgr.add(2));

    let mut session_enumerator: *mut c_void = std::ptr::null_mut();
    let hr = get_session_enum(session_mgr, &mut session_enumerator);
    release_mgr(session_mgr);

    if hr != S_OK || session_enumerator.is_null() {
        return 0;
    }

    let vtable_s_enum = *(session_enumerator as *const *const usize);
    let get_count: unsafe extern "system" fn(*mut c_void, *mut i32) -> HRESULT =
        std::mem::transmute(*vtable_s_enum.add(3));
    let get_session: unsafe extern "system" fn(*mut c_void, i32, *mut *mut c_void) -> HRESULT =
        std::mem::transmute(*vtable_s_enum.add(4));
    let release_s_enum: unsafe extern "system" fn(*mut c_void) -> u32 =
        std::mem::transmute(*vtable_s_enum.add(2));

    let mut count = 0i32;
    let _ = get_count(session_enumerator, &mut count);

    let my_pid = std::process::id();
    let mut muted_count = 0usize;

    for i in 0..count {
        let mut session_control: *mut c_void = std::ptr::null_mut();
        if get_session(session_enumerator, i, &mut session_control) != S_OK || session_control.is_null() {
            continue;
        }

        let vtable_sc = *(session_control as *const *const usize);
        let query_interface_sc: unsafe extern "system" fn(*mut c_void, *const GUID, *mut *mut c_void) -> HRESULT =
            std::mem::transmute(*vtable_sc.add(0));
        let release_sc: unsafe extern "system" fn(*mut c_void) -> u32 =
            std::mem::transmute(*vtable_sc.add(2));

        let mut session_pid = 0u32;
        let mut session_control2: *mut c_void = std::ptr::null_mut();
        if query_interface_sc(session_control, &iid_audio_session_control2, &mut session_control2) == S_OK && !session_control2.is_null() {
            let vtable_sc2 = *(session_control2 as *const *const usize);
            let get_pid: unsafe extern "system" fn(*mut c_void, *mut u32) -> HRESULT =
                std::mem::transmute(*vtable_sc2.add(14));
            let release_sc2: unsafe extern "system" fn(*mut c_void) -> u32 =
                std::mem::transmute(*vtable_sc2.add(2));
            let _ = get_pid(session_control2, &mut session_pid);
            release_sc2(session_control2);
        }

        let mut simple_volume: *mut c_void = std::ptr::null_mut();
        if query_interface_sc(session_control, &iid_simple_audio_volume, &mut simple_volume) == S_OK && !simple_volume.is_null() {
            let vtable_vol = *(simple_volume as *const *const usize);
            let set_mute: unsafe extern "system" fn(*mut c_void, i32, *mut c_void) -> HRESULT =
                std::mem::transmute(*vtable_vol.add(5));
            let release_vol: unsafe extern "system" fn(*mut c_void) -> u32 =
                std::mem::transmute(*vtable_vol.add(2));

            if session_pid != my_pid {
                if set_mute(simple_volume, 1, std::ptr::null_mut()) == S_OK {
                    muted_count += 1;
                }
            } else {
                let _ = set_mute(simple_volume, 0, std::ptr::null_mut());
            }
            release_vol(simple_volume);
        }
        release_sc(session_control);
    }

    release_s_enum(session_enumerator);
    muted_count
}

#[cfg(not(windows))]
pub fn execute_system_actions(actions: &[AgentAction], _request_id: u64) -> String {
    let mut feedback = Vec::new();
    for action in actions {
        match action {
            AgentAction::OpenApp { name } => {
                let _ = std::process::Command::new(name).spawn();
                feedback.push(format!("Application '{}' lancée en arrière-plan.", name));
            }
            AgentAction::CloseApp { name } => {
                let _ = std::process::Command::new("pkill").arg("-f").arg(name).spawn();
                feedback.push(format!("Signal de fermeture envoyé à '{}'.", name));
            }
            AgentAction::OpenBrowser { url } => {
                let url_str = format_url_for_navigation(url.as_deref().unwrap_or(""));
                #[cfg(target_os = "macos")]
                let _ = std::process::Command::new("open").arg(&url_str).spawn();
                #[cfg(not(target_os = "macos"))]
                let _ = std::process::Command::new("xdg-open").arg(&url_str).spawn();
                feedback.push(format!("Navigateur ouvert sur '{}'.", url_str));
            }
            AgentAction::FocusAddressBar { .. } => {
                #[cfg(target_os = "macos")]
                let _ = std::process::Command::new("osascript")
                    .args(["-e", "tell application \"System Events\" to keystroke \"l\" using command down"])
                    .status();
                #[cfg(not(target_os = "macos"))]
                let _ = std::process::Command::new("xdotool").args(["key", "ctrl+l"]).status();
                feedback.push("Barre d'adresse sélectionnée (Ctrl+L).".to_string());
            }
            AgentAction::NavigateToUrl { url, .. } => {
                let formatted = format_url_for_navigation(url);
                #[cfg(target_os = "macos")]
                let _ = std::process::Command::new("open").arg(&formatted).spawn();
                #[cfg(not(target_os = "macos"))]
                let _ = std::process::Command::new("xdg-open").arg(&formatted).spawn();
                feedback.push(format!("Navigation vers '{}'.", formatted));
            }
            AgentAction::WriteText { text, press_enter, .. } => {
                let _ = std::process::Command::new("xdotool").args(["key", "ctrl+a"]).status();
                std::thread::sleep(Duration::from_millis(30));
                let _ = std::process::Command::new("xdotool").args(["type", "--delay", "12", text]).status();
                if press_enter.unwrap_or(false) {
                    let _ = std::process::Command::new("xdotool").args(["key", "Return"]).status();
                }
                feedback.push(format!("Texte saisi sous X11 ({} caractères).", text.len()));
            }
            AgentAction::Dictate { text, press_enter, .. } => {
                let _ = std::process::Command::new("xdotool").args(["type", "--delay", "12", text]).status();
                if press_enter.unwrap_or(false) {
                    let _ = std::process::Command::new("xdotool").args(["key", "Return"]).status();
                }
                feedback.push(format!("Texte saisi sous X11 ({} caractères).", text.len()));
            }
            AgentAction::RunCommand { command } => {
                let cmd_clean = command.trim().to_lowercase();
                if cmd_clean.contains('+') || cmd_clean.starts_with("ctrl") || cmd_clean.starts_with("alt") {
                    let _ = std::process::Command::new("xdotool").args(["key", &cmd_clean]).status();
                    feedback.push(format!("Combinaison de touches '{}' envoyée via xdotool.", command));
                    continue;
                }
                let ok = std::process::Command::new("sh").arg("-c").arg(command).spawn().is_ok();
                feedback.push(format!("Commande shell '{}' ({})", command, if ok { "succès" } else { "échec" }));
            }
            AgentAction::AccessibilityShortcut { shortcut } => {
                match shortcut.as_str() {
                    "enter" | "press_enter" => { let _ = std::process::Command::new("xdotool").args(["key", "Return"]).status(); }
                    "escape" => { let _ = std::process::Command::new("xdotool").args(["key", "Escape"]).status(); }
                    "copy" => { let _ = std::process::Command::new("xdotool").args(["key", "ctrl+c"]).status(); }
                    "close_tab" => { let _ = std::process::Command::new("xdotool").args(["key", "ctrl+w"]).status(); }
                    "refresh_page" => { let _ = std::process::Command::new("xdotool").args(["key", "F5"]).status(); }
                    "bookmarks_bar" | "toggle_bookmarks" | "favoris" => {
                        let _ = std::process::Command::new("xdotool").args(["key", "ctrl+shift+b"]).status();
                    }
                    other => {
                        let _ = std::process::Command::new("xdotool").args(["key", other]).status();
                    }
                }
                feedback.push(format!("Raccourci '{}' envoyé via xdotool.", shortcut));
            }
            AgentAction::EnableAutostart => {
                feedback.push(set_autostart(true).unwrap_or_else(|e| e));
            }
            AgentAction::DisableAutostart => {
                feedback.push(set_autostart(false).unwrap_or_else(|e| e));
            }
            AgentAction::CheckAutostart => {
                feedback.push(format!("Autostart XDG actif : {}", is_autostart_enabled()));
            }
            AgentAction::SetReasoningPasses { passes, reason } => {
                let clamped = (*passes).clamp(1, 15);
                TARGET_REASONING_PASSES.store(clamped, Ordering::SeqCst);
                let reason_desc = reason.as_deref().unwrap_or("ajustement au contexte d'urgence");
                feedback.push(format!("Budget de raisonnement configuré à {} passes ({reason_desc}).", clamped));
            }
            AgentAction::SchedulePrompt { delay_seconds, prompt } => {
                let dur = Duration::from_secs((*delay_seconds).max(1));
                schedule_prompt_execution(dur, prompt.clone());
                crate::play_earcon(crate::Earcon::Success);
                let friendly = format_duration_friendly(dur);
                feedback.push(format!("Commande différée planifiée dans {friendly} : « {prompt} »."));
            }
            AgentAction::SchedulePrompts { tasks } => {
                let mut count = 0;
                for t in tasks {
                    if !t.prompt.trim().is_empty() {
                        let dur = Duration::from_secs(t.delay_seconds.max(1));
                        schedule_prompt_execution(dur, t.prompt.clone());
                        count += 1;
                    }
                }
                if count > 0 {
                    crate::play_earcon(crate::Earcon::Success);
                    feedback.push(format!("{count} commande(s) différée(s) planifiée(s)."));
                }
            }
            AgentAction::CancelScheduledTasks => {
                let count = cancel_all_scheduled_tasks();
                crate::play_earcon(crate::Earcon::Success);
                if count > 0 {
                    feedback.push(format!("Toutes les commandes différées ont été annulées ({count} annulée{}).", if count > 1 { "s" } else { "" }));
                } else {
                    feedback.push("Aucune commande différée active à annuler.".to_string());
                }
            }
            AgentAction::MuteAudio => {
                #[cfg(target_os = "macos")]
                {
                    let _ = std::process::Command::new("osascript").args(["-e", "set volume output muted true"]).spawn();
                }
                #[cfg(not(target_os = "macos"))]
                {
                let _ = std::process::Command::new("playerctl").arg("pause").spawn();
                let _ = std::process::Command::new("sh")
                    .arg("-c")
                    .arg("pactl list sink-inputs short | awk '{print $1}' | xargs -r -I{} pactl set-sink-input-mute {} 1")
                    .spawn();
                }
                feedback.push("Flux sonores externes mis en sourdine (voix Libertide préservée).".to_string());
            }
            AgentAction::SetVolume { command } => {
                let cmd_lower = command.to_lowercase();
                match cmd_lower.as_str() {
                    "mute" | "couper" => {
                        #[cfg(target_os = "macos")]
                        let _ = std::process::Command::new("osascript").args(["-e", "set volume output muted true"]).spawn();
                        #[cfg(not(target_os = "macos"))]
                        let _ = std::process::Command::new("pactl")
                            .args(["set-sink-mute", "@DEFAULT_SINK@", "1"])
                            .spawn();
                        feedback.push("Son système coupé.".to_string());
                    }
                    "unmute" => {
                        #[cfg(target_os = "macos")]
                        let _ = std::process::Command::new("osascript").args(["-e", "set volume output muted false"]).spawn();
                        #[cfg(not(target_os = "macos"))]
                        let _ = std::process::Command::new("pactl")
                            .args(["set-sink-mute", "@DEFAULT_SINK@", "0"])
                            .spawn();
                        feedback.push("Son système réactivé.".to_string());
                    }
                    "down" | "baisser" => {
                        #[cfg(target_os = "macos")]
                        let _ = std::process::Command::new("osascript").args(["-e", "set volume output volume ((output volume of (get volume settings)) - 10)"]).spawn();
                        #[cfg(not(target_os = "macos"))]
                        let _ = std::process::Command::new("pactl")
                            .args(["set-sink-volume", "@DEFAULT_SINK@", "-10%"])
                            .spawn();
                        feedback.push("Volume diminué.".to_string());
                    }
                    "up" | "monter" => {
                        #[cfg(target_os = "macos")]
                        let _ = std::process::Command::new("osascript").args(["-e", "set volume output volume ((output volume of (get volume settings)) + 10)"]).spawn();
                        #[cfg(not(target_os = "macos"))]
                        let _ = std::process::Command::new("pactl")
                            .args(["set-sink-volume", "@DEFAULT_SINK@", "+10%"])
                            .spawn();
                        feedback.push("Volume augmenté.".to_string());
                    }
                    _ => {
                        let _ = std::process::Command::new("pactl")
                            .args(["set-sink-mute", "@DEFAULT_SINK@", "toggle"])
                            .spawn();
                        feedback.push("Bascule muet audio système envoyée.".to_string());
                    }
                }
            }
            _ => {}
        }
    }
    if feedback.is_empty() { "Aucune action exécutée.".to_string() } else { feedback.join("\n") }
}