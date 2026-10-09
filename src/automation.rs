use std::sync::Mutex;
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::Duration;

use crate::fs_util::{create_temp_note_file, find_executable_in_path};
use crate::pattern::{clean_words, extract_target_propositions, matches_pattern};
use crate::types::{AgentAction, QuickSuggestionItem};
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
    CUIAutomation, IUIAutomation, IUIAutomationElement, IUIAutomationElement2, IUIAutomationInvokePattern,
    IUIAutomationScrollItemPattern, IUIAutomationTextPattern, IUIAutomationValuePattern,
    TreeScope_Descendants, UIA_ButtonControlTypeId, UIA_DocumentControlTypeId, UIA_EditControlTypeId,
    UIA_HyperlinkControlTypeId, UIA_InvokePatternId, UIA_ScrollItemPatternId, UIA_TextPatternId, UIA_ValuePatternId,
};
#[cfg(windows)]
use windows::Win32::UI::Input::KeyboardAndMouse::{
    mouse_event, SendInput, INPUT, INPUT_0, INPUT_KEYBOARD, INPUT_MOUSE, KEYBDINPUT,
    KEYBD_EVENT_FLAGS, KEYEVENTF_KEYUP, KEYEVENTF_UNICODE, MOUSEEVENTF_LEFTDOWN, MOUSEEVENTF_WHEEL,
    MOUSEEVENTF_LEFTUP, MOUSEINPUT, VIRTUAL_KEY,
};
#[cfg(windows)]
use windows::Win32::UI::WindowsAndMessaging::{
    FindWindowW, GetForegroundWindow, GetWindowRect, IsIconic,
    PostMessageW, SetCursorPos, SetForegroundWindow, ShowWindow,
    SW_MAXIMIZE, SW_MINIMIZE, SW_RESTORE, WM_CLOSE,
};

#[cfg(windows)]
const CREATE_NEW_CONSOLE: u32 = 0x00000010;

#[cfg(windows)]
static LAST_TXT_HWND: Mutex<Option<isize>> = Mutex::new(None);

pub static LAST_SCREEN_SUMMARY: Mutex<Option<String>> = Mutex::new(None);
pub static IS_IMMERSION_ACTIVE: AtomicBool = AtomicBool::new(false);
pub static IMMERSION_SCREEN_HISTORY: Mutex<Vec<String>> = Mutex::new(Vec::new());

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
pub fn force_foreground_window(hwnd: HWND) {
    extern "system" {
        fn GetCurrentThreadId() -> u32;
        fn GetWindowThreadProcessId(hwnd: HWND, lpdwprocessid: *mut u32) -> u32;
        fn AttachThreadInput(idattach: u32, idattachto: u32, fattach: i32) -> i32;
        fn BringWindowToTop(hwnd: HWND) -> i32;
    }
    unsafe {
        if IsIconic(hwnd).as_bool() {
            let _ = ShowWindow(hwnd, SW_RESTORE);
        }
        let fg_hwnd = GetForegroundWindow();
        let current_thread_id = GetCurrentThreadId();
        let target_thread_id = GetWindowThreadProcessId(hwnd, std::ptr::null_mut());
        let fg_thread_id = if !fg_hwnd.0.is_null() {
            GetWindowThreadProcessId(fg_hwnd, std::ptr::null_mut())
        } else {
            0
        };

        // Déblocage de la restriction de premier plan Windows
        const VK_MENU: VIRTUAL_KEY = VIRTUAL_KEY(0x12);
        send_key_event(VK_MENU, KEYBD_EVENT_FLAGS(0));
        send_key_event(VK_MENU, KEYEVENTF_KEYUP);

        if current_thread_id != target_thread_id && target_thread_id != 0 {
            let _ = AttachThreadInput(current_thread_id, target_thread_id, 1);
            if fg_thread_id != 0 && fg_thread_id != target_thread_id {
                let _ = AttachThreadInput(fg_thread_id, target_thread_id, 1);
            }
            let _ = BringWindowToTop(hwnd);
            let _ = SetForegroundWindow(hwnd);
            if fg_thread_id != 0 && fg_thread_id != target_thread_id {
                let _ = AttachThreadInput(fg_thread_id, target_thread_id, 0);
            }
            let _ = AttachThreadInput(current_thread_id, target_thread_id, 0);
        } else {
            let _ = BringWindowToTop(hwnd);
            let _ = SetForegroundWindow(hwnd);
        }
    }
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
    t == "url"
        || t == "l'url"
        || t == "adresse"
        || t == "l'adresse"
        || t == "barre d'adresse"
        || t == "barre d adresse"
        || t == "barre dadresse"
        || t == "barre d'url"
        || t == "barre url"
        || t == "omnibox"
        || t == "address"
        || t == "address bar"
        || t.contains("barre d'adresse")
        || t.contains("barre d adresse")
        || t.contains("adresse web")
}

pub fn is_browser_chrome_target(target: &str) -> bool {
    let t = target.trim().to_lowercase();
    t.starts_with("[navigateur]")
        || t.starts_with("navigateur:")
        || t.starts_with("[browser]")
        || t.starts_with("browser:")
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
    if trimmed.is_empty() || trimmed.starts_with("about:") {
        return "https://www.google.com".to_string();
    }
    if !trimmed.starts_with("http://") && !trimmed.starts_with("https://") {
        if trimmed.contains('.') && !trimmed.contains(' ') {
            format!("https://{trimmed}")
        } else {
            format!("https://www.google.com/search?q={}", urlencoding_simple(trimmed))
        }
    } else {
        trimmed.to_string()
    }
}

#[cfg(windows)]
fn navigate_browser_address_bar(hwnd: HWND, url: &str) {
    let formatted_url = format_url_for_navigation(url);
    force_foreground_window(hwnd);
    unsafe {
        let _ = ShowWindow(hwnd, SW_MAXIMIZE);
    }
    std::thread::sleep(Duration::from_millis(150));

    const VK_CONTROL: VIRTUAL_KEY = VIRTUAL_KEY(0x11);
    const VK_L: VIRTUAL_KEY = VIRTUAL_KEY(0x4C);
    const VK_MENU: VIRTUAL_KEY = VIRTUAL_KEY(0x12);
    const VK_D: VIRTUAL_KEY = VIRTUAL_KEY(0x44);
    const VK_RETURN: VIRTUAL_KEY = VIRTUAL_KEY(0x0D);

    // 1. Clic direct sur la barre d'adresse si elle est repérée dans l'arbre UIA
    let elements = list_interactive_elements(hwnd);
    let address_elem = elements.iter().find(|e| {
        e.is_browser_chrome
            && (e.class_name.contains("omnibox")
                || e.automation_id.to_lowercase().contains("omnibox")
                || e.localized_type.contains("omnibox")
                || e.name.to_lowercase().contains("adresse")
                || e.name.to_lowercase().contains("address"))
    });

    if let Some(elem) = address_elem {
        unsafe { let _ = elem.element.SetFocus(); }
        click_element(elem.click_x, elem.click_y, Some(&elem.element), elem.pattern.as_ref());
        std::thread::sleep(Duration::from_millis(80));
    }

    // 2. Raccourcis universels de sélection de la barre d'adresse (Alt+D puis Ctrl+L)
    send_hotkey(&[VK_MENU], VK_D);
    std::thread::sleep(Duration::from_millis(80));
    send_hotkey(&[VK_CONTROL], VK_L);
    std::thread::sleep(Duration::from_millis(100));

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
#[derive(Clone)]
struct UiaElementInfo {
    element: IUIAutomationElement,
    automation_id: String,
    help_text: String,
    value_text: String,
    name: String,
    class_name: String,
    localized_type: String,
    aria_role: String,
    is_edit_or_textarea: bool,
    is_explicit_textarea: bool,
    is_input: bool,
    is_button: bool,
    is_link: bool,
    is_focusable: bool,
    is_web_content: bool,
    is_browser_chrome: bool,
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

        let count = elements.Length().unwrap_or(0).clamp(0, 1200);

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
                    let mut name = raw_name.trim().to_string();
                    let automation_id = item.CurrentAutomationId().map(|b| b.to_string()).unwrap_or_default();
                    let help_text = item.CurrentHelpText().map(|b| b.to_string()).unwrap_or_default().trim().to_string();
                    let item_type = item.CurrentItemType().map(|b| b.to_string()).unwrap_or_default().to_lowercase();
                    let item_status = item.CurrentItemStatus().map(|b| b.to_string()).unwrap_or_default().to_lowercase();

                    let (aria_role, aria_properties) = if let Ok(e2) = item.cast::<IUIAutomationElement2>() {
                        let r = unsafe { e2.CurrentAriaRole().ok() }
                            .map(|b| b.to_string())
                            .unwrap_or_default()
                            .to_lowercase();
                        let p = unsafe { e2.CurrentAriaProperties().ok() }
                            .map(|b| b.to_string())
                            .unwrap_or_default()
                            .to_lowercase();
                        (r, p)
                    } else {
                        (String::new(), String::new())
                    };

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
                        .and_then(|vp| unsafe { vp.CurrentValue().ok() })
                        .map(|b| b.to_string())
                        .unwrap_or_default()
                        .trim().to_string();
                    let is_focusable = item.CurrentIsKeyboardFocusable().map(|b| b.as_bool()).unwrap_or(false);

                    let is_explicit_textarea = class_name.contains("textarea")
                        || loc_type.contains("textarea")
                        || loc_type.contains("zone de texte")
                        || (is_edit_type && height >= 35);

                    let is_input = !is_explicit_textarea && (is_edit_type
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

                    let is_button = ctype == UIA_ButtonControlTypeId
                        || loc_type.contains("bouton")
                        || loc_type.contains("button")
                        || is_role_button
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

                    if is_browser_chrome && !name.is_empty() && !name.starts_with("[Navigateur]") {
                        name = format!("[Navigateur] {name}");
                    }

                    if name.is_empty() && (is_link || is_button) {
                        if let Some(txt) = extract_element_text(&item) {
                            name = txt.trim().to_string();
                        }
                    }

                    if name.is_empty() && automation_id.is_empty() && help_text.is_empty() && value_text.is_empty() && !is_edit_or_textarea && !is_button && !is_link && !is_focusable {
                        continue;
                    }

                    let click_x = rect.left + width / 2;
                    let click_y = rect.top + height / 2;
                    let pattern = item.GetCurrentPattern(UIA_InvokePatternId)
                        .ok()
                        .and_then(|p| p.cast::<IUIAutomationInvokePattern>().ok());
                    let area = (width as i64) * (height as i64);
                    results.push(UiaElementInfo {
                        element: item,
                        automation_id,
                        help_text,
                        value_text,
                        name,
                        class_name,
                        localized_type: loc_type,
                        aria_role,
                        is_edit_or_textarea,
                        is_explicit_textarea,
                        is_input,
                        is_button,
                        is_link,
                        is_focusable,
                        is_web_content,
                        is_browser_chrome,
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
fn has_focused_textarea(hwnd: HWND) -> bool {
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
        std::thread::sleep(Duration::from_millis(100));
        elements = list_interactive_elements(hwnd);
    }
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
        std::thread::sleep(Duration::from_millis(100));
        elements = list_interactive_elements(hwnd);
    }
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
    let mut best_explicit: Option<(HWND, UiaElementInfo)> = None;
    let mut best_fallback: Option<(HWND, UiaElementInfo)> = None;

    let mut ordered: Vec<HWND> = Vec::new();
    if let Some(pref) = preferred_hwnd {
        ordered.push(pref);
    }
    for &(h, _) in user_windows {
        if !ordered.contains(&h) {
            ordered.push(h);
        }
    }

    for hwnd in ordered.into_iter().take(4) {
        let elements = list_interactive_elements(hwnd);
        let has_web = elements.iter().any(|e| e.is_web_content);
        for elem in elements {
            if has_web && elem.is_browser_chrome {
                continue;
            }
            if elem.is_explicit_textarea {
                let replace = match &best_explicit {
                    Some((_, cur)) => elem.area > cur.area,
                    None => true,
                };
                if replace {
                    best_explicit = Some((hwnd, elem));
                }
            } else if elem.is_edit_or_textarea || elem.is_input {
                let replace = match &best_fallback {
                    Some((_, cur)) => elem.area > cur.area,
                    None => true,
                };
                if replace {
                    best_fallback = Some((hwnd, elem));
                }
            }
        }
    }

    best_explicit.or(best_fallback)
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
    std::process::Command::new("cmd")
        .args(["/C", trimmed])
        .creation_flags(CREATE_NEW_CONSOLE)
        .spawn()
        .is_ok()
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
        let uia: Result<IUIAutomation, _> = CoCreateInstance(&CUIAutomation, None, CLSCTX_INPROC_SERVER);
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
fn score_single_proposition(elem: &UiaElementInfo, prop: &str) -> Option<f32> {
    let prop_clean = prop.trim();
    let prop_lower = prop_clean.to_lowercase();
    let name_trim = elem.name.trim();
    let name_lower = name_trim.to_lowercase();

    if !name_lower.is_empty() && name_lower == prop_lower {
        return Some(350.0);
    }

    let mut best_score: Option<f32> = None;

    let mut regex_score = 0.0f32;
    if matches_pattern(prop, &elem.name) {
        regex_score = regex_score.max(180.0);
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

    let name_words = clean_words(&name_lower);
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
        let s_help = check_attr(&help_lower, &help_words, 3.0);
        let s_id = check_attr(&id_lower, &id_words, 2.5);
        let s_val = check_attr(&val_lower, &val_words, 2.0);
        let s_type = check_attr(&type_lower, &type_words, 1.8);
        let s_class = check_attr(&class_lower, &class_words, 1.2);
        let s_aria = check_attr(&aria_lower, &aria_words, 2.2);

        let best_match = s_name.max(s_help).max(s_id).max(s_val).max(s_type).max(s_class).max(s_aria);
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
            if has_web_content {
                if is_chrome_target && !elem.is_browser_chrome {
                    return None;
                }
                if !is_chrome_target && elem.is_browser_chrome {
                    return None;
                }
            }

            let word_count = elem.name.split_whitespace().count();
            if is_target_short && (word_count > 12 || elem.name.len() > 90) {
                return None;
            }

            score_element(elem, target).map(|score| {
                let mut final_score = score;
                if elem.is_link {
                    final_score += 40.0;
                } else if elem.pattern.is_some() {
                    final_score += 20.0;
                }
                if elem.is_web_content {
                    final_score += 35.0;
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
            if word_count > 10 || elem.name.len() > 80 {
                return None;
            }
            if has_web_content {
                if is_chrome_target && !elem.is_browser_chrome {
                    return None;
                }
                if !is_chrome_target && elem.is_browser_chrome {
                    return None;
                }
            }

            score_element(elem, target).map(|score| {
                let mut final_score = score;
                if elem.is_button {
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
            if has_web_content {
                if is_chrome_target && !elem.is_browser_chrome {
                    return None;
                }
                if !is_chrome_target && elem.is_browser_chrome {
                    return None;
                }
            }

            score_element(elem, target).map(|score| {
                let mut final_score = score;
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

pub fn compute_screen_diff(old_summary: &str, new_summary: &str) -> String {
    let old_lines: Vec<&str> = old_summary.lines().map(|l| l.trim()).filter(|l| !l.is_empty()).collect();
    let new_lines: Vec<&str> = new_summary.lines().map(|l| l.trim()).filter(|l| !l.is_empty()).collect();

    let mut added = Vec::new();
    let mut removed = Vec::new();

    for &line in &new_lines {
        if line.starts_with("===") || line.starts_with("Espace de travail") {
            continue;
        }
        if !old_lines.contains(&line) {
            added.push(line);
        }
    }

    for &line in &old_lines {
        if line.starts_with("===") || line.starts_with("Espace de travail") {
            continue;
        }
        if !new_lines.contains(&line) {
            removed.push(line);
        }
    }

    if added.is_empty() && removed.is_empty() {
        return String::new();
    }

    let mut diff = String::new();
    if !removed.is_empty() {
        diff.push_str("[Éléments disparus ou modifiés] :\n");
        for r in removed {
            diff.push_str(&format!("- {}\n", r));
        }
    }
    if !added.is_empty() {
        diff.push_str("[Nouveaux éléments ou nouveaux états] :\n");
        for a in added {
            diff.push_str(&format!("+ {}\n", a));
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
        "précédent", "suivant", "next", "previous", "en savoir plus", "lire la suite",
        "plus", "voir plus", "connexion", "se connecter", "s'inscrire", "sign in",
        "login", "accueil", "home", "menu", "conditions d'utilisation", "confidentialité",
        "cookies", "aide", "help", "contact", "retour", "partager", "haut de page", "top",
        "fermer", "close", "annuler", "cancel", "ok", "oui", "non",
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
pub fn summarize_screen_state(target_window: Option<&str>, autoscroll: bool) -> String {
    let work_area = get_desktop_work_area();
    let wa_x = work_area.left;
    let wa_y = work_area.top;
    let wa_w = work_area.right - work_area.left;
    let wa_h = work_area.bottom - work_area.top;

    let user_windows = list_user_windows();
    let fg = unsafe { GetForegroundWindow() };

    let mut out = String::new();
    out.push_str(&format!(
        "=== Rapport d'analyse de l'écran ===\nEspace de travail : {}x{}\n",
        wa_w, wa_h
    ));

    let fg_title = user_windows
        .iter()
        .find(|(h, _)| *h == fg)
        .map(|(_, t)| t.as_str())
        .unwrap_or("Inconnue ou overlay");
    out.push_str(&format!("Fenêtre au premier plan : \"{}\"\n\n", fg_title));

    out.push_str("Fenêtres et processus actifs :\n");
    let mut inspect_hwnds = Vec::new();

    if let Some(target) = target_window.filter(|t| !t.trim().is_empty()) {
        let matched = find_windows_matching(target, &user_windows, Some(fg));
        if let Some(&h) = matched.first() {
            inspect_hwnds.push(h);
        }
    }

    if inspect_hwnds.is_empty() {
        if let Some(best) = get_active_or_best_window(&user_windows) {
            inspect_hwnds.push(best);
        }
        for (h, _) in &user_windows {
            if !inspect_hwnds.contains(h) && inspect_hwnds.len() < 3 {
                inspect_hwnds.push(*h);
            }
        }
    }

    for (hwnd, title) in &user_windows {
        let (pid, exe_name) = get_window_process_info(*hwnd);
        let mut r = RECT::default();
        let rect_str = if unsafe { GetWindowRect(*hwnd, &mut r).is_ok() } {
            let w = r.right - r.left;
            let h = r.bottom - r.top;
            format!("pos: ({}, {}), taille: {}x{}", r.left, r.top, w, h)
        } else {
            "position inconnue".to_string()
        };

        let is_minimized = unsafe { IsIconic(*hwnd).as_bool() };
        let state = if is_minimized {
            "réduite"
        } else if *hwnd == fg {
            "active/premier plan"
        } else {
            "visible"
        };

        out.push_str(&format!(
            "- \"{}\" [{}] ({}) | PID: {}, Exe: \"{}\" | Accéder: focus_window(title: \"{}\") | Tuer: kill_process(pid: {}, name: \"{}\")\n",
            title, state, rect_str, pid, exe_name, title, pid, exe_name
        ));
    }

    out.push_str("\n=== Contenu détaillé des fenêtres principales ===\n");
    for (win_idx, hwnd) in inspect_hwnds.iter().enumerate() {
        let win_title = user_windows
            .iter()
            .find(|(h, _)| *h == *hwnd)
            .map(|(_, t)| t.as_str())
            .unwrap_or("Fenêtre");

        out.push_str(&format!("\n--- Fenêtre : \"{}\" ---\n", win_title));
        let initial_elements = list_interactive_elements(*hwnd);
        let is_browser = is_browser_hwnd(*hwnd, win_title);
        let has_scrollbar = initial_elements.iter().any(|e| {
            e.class_name.contains("scrollbar")
                || e.localized_type.contains("scrollbar")
                || e.localized_type.contains("défilement")
        });

        let is_primary = win_idx == 0;
        let mut elements = initial_elements;
        let mut chunks_count = 1usize;

        if is_primary && autoscroll && (has_scrollbar || is_browser) {
            force_foreground_window(*hwnd);
            unsafe {
                let show_mode = if is_browser { SW_MAXIMIZE } else { SW_RESTORE };
                let _ = ShowWindow(*hwnd, show_mode);
            }
            std::thread::sleep(Duration::from_millis(50));

            let mut r = RECT::default();
            let (cx, cy) = if unsafe { GetWindowRect(*hwnd, &mut r).is_ok() } {
                let w = (r.right - r.left).max(10);
                let h = (r.bottom - r.top).max(10);
                (r.left + w / 2, r.top + h / 2)
            } else {
                (wa_x + wa_w / 2, wa_y + wa_h / 2)
            };
            unsafe { let _ = SetCursorPos(cx, cy); }
            std::thread::sleep(Duration::from_millis(30));

            let mut prev_sig: Vec<String> = elements.iter().map(|e| format!("{}:{}:{}", e.name, e.localized_type, e.automation_id)).collect();
            const VK_NEXT: VIRTUAL_KEY = VIRTUAL_KEY(0x22);
            const VK_CONTROL: VIRTUAL_KEY = VIRTUAL_KEY(0x11);
            const VK_HOME: VIRTUAL_KEY = VIRTUAL_KEY(0x24);

            for _chunk in 2..=10 {
                unsafe { mouse_event(MOUSEEVENTF_WHEEL, 0, 0, -480, 0); }
                std::thread::sleep(Duration::from_millis(30));
                send_hotkey(&[], VK_NEXT);
                std::thread::sleep(Duration::from_millis(90));

                let next_chunk = list_interactive_elements(*hwnd);
                if next_chunk.is_empty() {
                    break;
                }
                let next_sig: Vec<String> = next_chunk.iter().map(|e| format!("{}:{}:{}", e.name, e.localized_type, e.automation_id)).collect();
                if next_sig == prev_sig {
                    break;
                }
                chunks_count += 1;
                prev_sig = next_sig;

                for item in next_chunk {
                    let exists = elements.iter().any(|e| {
                        (!e.name.is_empty() && e.name == item.name && e.localized_type == item.localized_type)
                            || (!e.automation_id.is_empty() && e.automation_id == item.automation_id)
                    });
                    if !exists {
                        elements.push(item);
                    }
                }
            }
            send_hotkey(&[VK_CONTROL], VK_HOME);
            std::thread::sleep(Duration::from_millis(60));
        }

        if elements.is_empty() {
            out.push_str("  (Aucun élément UI interactif accessible)\n");
            continue;
        }

        let has_web_doc = elements.iter().any(|e| e.is_web_content);

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
                .take(5)
                .collect();
            if !chrome_controls.is_empty() {
                out.push_str(&format!("  [Interface navigateur] : {}\n", chrome_controls.join(" | ")));
            }
        }

        let edits: Vec<&UiaElementInfo> = elements.iter()
            .filter(|e| (!has_web_doc || e.is_web_content) && (e.is_edit_or_textarea || e.is_input))
            .collect();
        if !edits.is_empty() {
            let prefix = if has_web_doc { "  [Page web - champs de saisie] :\n" } else { "  [Champs de texte / saisie] :\n" };
            out.push_str(prefix);
            for edit in edits.iter().take(5) {
                let label = if !edit.name.is_empty() { &edit.name } else { "Champ" };
                let val = if !edit.value_text.is_empty() {
                    format!(" = \"{}\"", edit.value_text.chars().take(100).collect::<String>())
                } else {
                    String::new()
                };
                out.push_str(&format!("  • {}{}\n", label, val));
            }
        }

        let mut tabs: Vec<String> = elements
            .iter()
            .filter(|e| !e.name.is_empty() && (e.localized_type.contains("onglet") || e.localized_type.contains("tab")))
            .map(|e| e.name.clone())
            .collect();
        tabs.dedup();
        if !tabs.is_empty() {
            let display_tabs: Vec<String> = tabs.into_iter().take(8).collect();
            out.push_str(&format!("  [Onglets / pages ouvertes] : {}\n", display_tabs.join(" | ")));
        }

        let raw_links: Vec<String> = elements
            .iter()
            .filter(|e| {
                !e.is_edit_or_textarea
                    && !e.name.is_empty()
                    && (e.is_link
                        || e.localized_type.contains("lien")
                        || e.localized_type.contains("link")
                        || e.localized_type.contains("hyperlink")
                        || e.aria_role.contains("link")
                        || e.aria_role.contains("lien"))
            })
            .map(|e| e.name.clone())
            .collect();

        let mut seen_links = std::collections::HashSet::new();
        let mut scored_links: Vec<(f32, String)> = raw_links
            .into_iter()
            .filter(|l| !is_poor_or_redundant_link(l))
            .filter(|l| {
                let lower = l.trim().to_lowercase();
                if seen_links.contains(&lower) {
                    false
                } else {
                    seen_links.insert(lower);
                    true
                }
            })
            .map(|l| (score_enriched_link(&l), l))
            .collect();

        scored_links.sort_by(|a, b| b.0.partial_cmp(&a.0).unwrap_or(std::cmp::Ordering::Equal));
        let display_links: Vec<String> = scored_links.into_iter().take(25).map(|(_, l)| l).collect();

        if !display_links.is_empty() {
            let prefix = if has_web_doc { "  [Page web - liens cliquables] : " } else { "  [Liens / résultats cliquables] : " };
            out.push_str(&format!("{}{}\n", prefix, display_links.join(" | ")));
        }

        let mut text_items: Vec<String> = elements
            .iter()
            .filter(|e| {
                !e.is_edit_or_textarea
                    && !e.name.is_empty()
                    && e.name.len() >= 3
                    && !e.is_link
                    && !e.aria_role.contains("link")
                    && !e.localized_type.contains("lien")
                    && !e.localized_type.contains("link")
                    && (e.localized_type.contains("texte")
                        || e.localized_type.contains("text")
                        || e.localized_type.contains("en-tête")
                        || e.localized_type.contains("heading")
                        || e.name.contains('€')
                        || e.name.contains('$')
                        || e.name.to_lowercase().contains("eur"))
            })
            .map(|e| e.name.clone())
            .collect();
        text_items.dedup();
        if !text_items.is_empty() {
            let display_texts: Vec<String> = text_items.into_iter().take(10).collect();
            out.push_str(&format!("  [Textes / prix observés] : {}\n", display_texts.join(" | ")));
        }

        let mut buttons: Vec<String> = elements
            .iter()
            .filter(|e| {
                (!has_web_doc || e.is_web_content)
                    && !e.is_edit_or_textarea
                    && (!e.name.is_empty() || !e.help_text.is_empty() || !e.automation_id.is_empty())
                    && (e.is_button
                        || e.pattern.is_some()
                        || e.localized_type.contains("bouton")
                        || e.localized_type.contains("button")
                        || e.automation_id.to_lowercase().contains("close")
                        || e.name.to_lowercase().contains("fermer")
                        || e.name.to_lowercase().contains("close"))
                    && e.name.len() <= 80
                    && e.name.split_whitespace().count() <= 10
            })
            .map(|e| {
                if !e.name.is_empty() {
                    e.name.clone()
                } else if !e.help_text.is_empty() {
                    e.help_text.clone()
                } else {
                    e.automation_id.clone()
                }
            })
            .collect();
        buttons.dedup();
        if !buttons.is_empty() {
            let display_btns: Vec<String> = buttons.into_iter().take(25).collect();
            let prefix = if has_web_doc { "  [Page web - boutons cliquables] : " } else { "  [Boutons / contrôles cliquables] : " };
            out.push_str(&format!("{}{}\n", prefix, display_btns.join(", ")));
        }

        if chunks_count > 1 {
            out.push_str(&format!("  [Autoscroll et chunks de viewport] : {} viewports explorés (limite max 10), contenu agrégé.\n", chunks_count));
        } else if has_scrollbar {
            out.push_str("  [Défilement possible] : Une barre de défilement est présente. Utilise l'action 'scroll' ('down'/'up') pour révéler la suite de la page.\n");
        }
    }

    truncate_with_notice(&out, 4500)
}

#[cfg(not(windows))]
pub fn summarize_screen_state(_target_window: Option<&str>, _autoscroll: bool) -> String {
    "[Analyse de l'écran] Environnement non-Windows (simulation).".to_string()
}

#[cfg(windows)]
pub fn execute_system_actions(actions: &[AgentAction]) -> String {
    if actions.is_empty() {
        println!("[Actions] Aucune action système à exécuter.");
        return "Aucune action système à exécuter.".to_string();
    }

    let mut feedback = Vec::new();
    println!("[Actions] Exécution de {} action(s) système...", actions.len());

    let mut newly_spawned_hwnd: Option<HWND> = None;

    for action in actions {
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
        if let AgentAction::OpenBrowser { url } = action {
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

    let user_windows = list_user_windows();
    let active_user_hwnd = get_active_or_best_window(&user_windows);
    let preferred_target_hwnd = newly_spawned_hwnd.or(active_user_hwnd);

    for (idx, action) in actions.iter().enumerate() {
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
                    force_foreground_window(hwnd);
                    unsafe {
                        let _ = ShowWindow(hwnd, SW_RESTORE);
                    }
                    feedback.push(format!("Fenêtre passée au premier plan [HWND {:?}].", hwnd.0));
                } else {
                    feedback.push("Fenêtre introuvable pour focus.".to_string());
                }
            }
            AgentAction::ClickElement { window, target_name } => {
                let target_hwnd = match window.as_deref() {
                    Some(w) if !w.trim().is_empty() => {
                        find_windows_matching(w, &user_windows, active_user_hwnd).first().copied()
                    }
                    _ => active_user_hwnd,
                };

                if let Some(hwnd) = target_hwnd {
                    println!("[Actions] Clic sur l'élément [HWND {:?}] pour '{}'", hwnd.0, target_name);
                    let is_browser = user_windows.iter().find(|(h, t)| *h == hwnd).map_or(false, |(h, t)| is_browser_hwnd(*h, t));
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
                        println!("[Actions] Élément/lien trouvé : name='{}', id='{}', type='{}', clic en ({}, {})", elem.name, elem.automation_id, elem.localized_type, elem.click_x, elem.click_y);
                        unsafe { let _ = elem.element.SetFocus(); }
                        click_element(elem.click_x, elem.click_y, Some(&elem.element), elem.pattern.as_ref());
                        feedback.push(format!("Clic effectué sur l'élément '{}'.", target_name));
                    } else {
                        println!("[Actions] Aucun élément/lien correspondant trouvé pour '{}'", target_name);
                        feedback.push(format!("Élément ou lien '{}' introuvable à l'écran.", target_name));
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
                    let win_title = user_windows
                        .iter()
                        .find(|(h, _)| *h == hwnd)
                        .map(|(_, t)| t.as_str())
                        .unwrap_or("");
                    let is_browser = user_windows.iter().find(|(h, t)| *h == hwnd).map_or(false, |(h, t)| is_browser_hwnd(*h, t));
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
                let target_hwnd = match window.as_deref() {
                    Some(w) if !w.trim().is_empty() => {
                        find_windows_matching(w, &user_windows, active_user_hwnd).first().copied()
                    }
                    _ => active_user_hwnd,
                };

                if let Some(hwnd) = target_hwnd {
                    println!("[Actions] Clic sur le bouton [HWND {:?}] pour '{}'", hwnd.0, button_name);
                    let is_browser = user_windows.iter().find(|(h, t)| *h == hwnd).map_or(false, |(h, t)| is_browser_hwnd(*h, t));
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
                        println!("[Actions] Aucun bouton correspondant trouvé pour '{}'", button_name);
                        feedback.push(format!("Bouton '{}' introuvable à l'écran.", button_name));
                    }
                }
            }
            AgentAction::FocusElement { window, target_name } => {
                let target_hwnd = match window.as_deref() {
                    Some(w) if !w.trim().is_empty() => {
                        find_windows_matching(w, &user_windows, active_user_hwnd).first().copied()
                    }
                    _ => active_user_hwnd,
                };

                if let Some(hwnd) = target_hwnd {
                    println!("[Actions] Focus sur l'élément [HWND {:?}] pour '{}'", hwnd.0, target_name);
                    let is_browser = user_windows.iter().find(|(h, t)| *h == hwnd).map_or(false, |(h, t)| is_browser_hwnd(*h, t));
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
                let ok = run_system_command(command);
                let status = if ok { "exécutée avec succès" } else { "échec d'exécution" };
                feedback.push(format!("Commande système '{}' ({status}).", command));
            }
            AgentAction::WriteText { text, target, window } => {
                let target_desc_opt = target.as_deref().filter(|s| !s.trim().is_empty());
                let is_address_bar = target_desc_opt.map_or(false, is_address_bar_target);
                let is_generic = target_desc_opt.map_or(true, is_generic_textarea_target);

                let target_hwnd = match window.as_deref() {
                    Some(w) if !w.trim().is_empty() => {
                        find_windows_matching(w, &user_windows, active_user_hwnd).first().copied()
                    }
                    _ => {
                        if is_address_bar {
                            user_windows.iter().find(|(h, t)| is_browser_hwnd(*h, t)).map(|(h, _)| *h).or(active_user_hwnd)
                        } else {
                            active_user_hwnd
                        }
                    }
                };

                let mut handled = false;

                if is_address_bar {
                    if let Some(hwnd) = target_hwnd {
                        let win_title = user_windows.iter().find(|(h, _)| *h == hwnd).map(|(_, t)| t.as_str()).unwrap_or("Navigateur");
                        let is_browser = is_browser_hwnd(hwnd, win_title);
                        println!("[Actions] Ciblage barre d'adresse sur [HWND {:?}] '{}'", hwnd.0, win_title);
                        force_foreground_window(hwnd);
                        unsafe {
                            let _ = ShowWindow(hwnd, if is_browser { SW_MAXIMIZE } else { SW_RESTORE });
                        }
                        std::thread::sleep(Duration::from_millis(50));
                        navigate_browser_address_bar(hwnd, text);
                        handled = true;
                    } else {
                        println!("[Actions] Aucun navigateur ouvert trouvé, ouverture d'une nouvelle fenêtre pour : {}", text);
                        launch_browser_new_window(text);
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
                        println!("[Actions] Sélection de la plus grande zone de texte à l'écran : [HWND {:?}] name='{}', id='{}', area={}, clic en ({}, {})", hwnd.0, elem.name, elem.automation_id, elem.area, elem.click_x, elem.click_y);
                        force_foreground_window(hwnd);
                        unsafe {
                            let _ = ShowWindow(hwnd, SW_RESTORE);
                        }
                        std::thread::sleep(Duration::from_millis(80));
                        unsafe { let _ = elem.element.SetFocus(); }
                        click_element(elem.click_x, elem.click_y, Some(&elem.element), elem.pattern.as_ref());
                        std::thread::sleep(Duration::from_millis(50));
                        clipboard::set_text(text);
                        std::thread::sleep(Duration::from_millis(30));
                        send_paste();
                        println!("[Actions] Texte inséré avec succès dans la plus grande zone de texte : {:?}", text);
                        handled = true;
                    }
                } else if let (Some(hwnd), Some(target_desc)) = (target_hwnd, target_desc_opt) {
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

                        clipboard::set_text(text);
                        std::thread::sleep(Duration::from_millis(30));
                        send_paste();
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
                            clipboard::set_text(text);
                            std::thread::sleep(Duration::from_millis(30));
                            send_paste();
                            feedback.push(format!("Texte inséré dans la zone principale ({} caractères).", text.len()));
                            handled = true;
                        }
                    }
                }

                if !handled {
                    println!("[Actions] Repli d'écriture dans le document ou fenêtre de repli");
                    let _ = write_to_browser_or_txt(text);
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
                            user_windows.iter().find(|(h, t)| is_browser_hwnd(*h, t)).map(|(h, _)| *h).or(active_user_hwnd)
                        } else {
                            active_user_hwnd
                        }
                    }
                };

                let mut handled = false;
                if is_address_bar {
                    if let Some(hwnd) = target_hwnd {
                        let win_title = user_windows.iter().find(|(h, _)| *h == hwnd).map(|(_, t)| t.as_str()).unwrap_or("Navigateur");
                        let is_browser = is_browser_hwnd(hwnd, win_title);
                        println!("[Actions] Remplacement d'url barre d'adresse sur [HWND {:?}] '{}'", hwnd.0, win_title);
                        force_foreground_window(hwnd);
                        unsafe {
                            let _ = ShowWindow(hwnd, if is_browser { SW_MAXIMIZE } else { SW_RESTORE });
                        }
                        std::thread::sleep(Duration::from_millis(50));
                        navigate_browser_address_bar(hwnd, text);
                        handled = true;
                    } else {
                        println!("[Actions] Aucun navigateur ouvert trouvé, ouverture avec : {}", text);
                        launch_browser_new_window(text);
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
                if !handled {
                    println!("[Actions] Repli de remplacement sur le champ actif");
                    let _ = replace_active_field_text(text);
                }
            }
            AgentAction::OpenBrowser { .. } => {}
            AgentAction::NavigateToUrl { url, window } => {
                let target = format_url_for_navigation(url);
                let target_hwnd = match window.as_deref() {
                    Some(w) if !w.trim().is_empty() => {
                        find_windows_matching(w, &user_windows, active_user_hwnd).first().copied()
                    }
                    _ => {
                        user_windows
                            .iter()
                            .find(|(h, t)| is_browser_hwnd(*h, t))
                            .map(|(h, _)| *h)
                            .or(active_user_hwnd)
                    }
                };

                let existing_browser = target_hwnd
                    .filter(|h| user_windows.iter().any(|(wh, t)| *wh == *h && is_browser_hwnd(*wh, t)))
                    .or_else(|| user_windows.iter().find(|(h, t)| is_browser_hwnd(*h, t)).map(|(h, _)| *h));

                if let Some(hwnd) = existing_browser {
                    println!("[Actions] Navigateur ouvert trouvé [HWND {:?}], navigation via barre d'adresse vers : {}", hwnd.0, target);
                    force_foreground_window(hwnd);
                    unsafe {
                        let _ = ShowWindow(hwnd, SW_MAXIMIZE);
                    }
                    navigate_browser_address_bar(hwnd, &target);
                } else {
                    println!("[Actions] Aucun navigateur ouvert trouvé, ouverture d'une nouvelle fenêtre vers : {}", target);
                    launch_browser_new_window(&target);
                }
                feedback.push(format!("Navigation effectuée vers '{}'.", target));
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
                    "next_field" => send_hotkey(&[], VK_TAB),
                    "previous_field" => send_hotkey(&[VK_SHIFT], VK_TAB),
                    "escape" => send_hotkey(&[], VK_ESCAPE),
                    _ => {}
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
        }
    }

    if feedback.is_empty() {
        "Actions système exécutées avec succès.".to_string()
    } else {
        truncate_with_notice(&feedback.join("\n"), 2500)
    }
}

#[cfg(not(windows))]
pub fn execute_system_actions(_actions: &[AgentAction]) -> String {
    "Actions simulées (environnement non-Windows).".to_string()
}