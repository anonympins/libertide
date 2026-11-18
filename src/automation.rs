use std::sync::mpsc::{Receiver, Sender};
use std::sync::Mutex;
use std::sync::atomic::{AtomicBool, AtomicU32, AtomicU64, AtomicUsize, Ordering};
use std::time::{Duration, Instant};

pub use crate::scheduler::*;
pub use crate::autostart::*;
pub use crate::clipboard::*;
#[cfg(windows)]
use crate::get_window_title;

pub use crate::action_history::*;
use crate::fs_util::{
    create_temp_note_file, find_executable_in_path, find_file_in_workspace,
    open_path_with_default_app,
};

use crate::keyboard_hook::{
    SWITCH_HOOK_ENABLED, IS_EXTERNAL_TEXT_INPUT_FOCUSED, SWITCH_KEY_QUEUE,SWITCH_LONG_PRESS_MS
};
#[cfg(not(windows))]
use crate::ocr::HWND;
#[cfg(not(windows))]
use crate::screen_summary::summarize_screen_state;
use crate::types::{AgentAction, QuickSuggestionItem};
pub use crate::types::{NavigationZone, NumberedElementTarget};
use crate::window_util::*;

use crate::ui::*;

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
    CUIAutomation, IUIAutomation, IUIAutomationElement,
    IUIAutomationTextPattern,
    IUIAutomationValuePattern, TreeScope_Descendants, UIA_DocumentControlTypeId,
    UIA_EditControlTypeId,
    UIA_TextPatternId,
    UIA_ValuePatternId,
};
#[cfg(windows)]
pub(crate) use windows::Win32::UI::Input::KeyboardAndMouse::{
    keybd_event, SetFocus,
    KEYBD_EVENT_FLAGS, KEYEVENTF_EXTENDEDKEY, KEYEVENTF_KEYUP, VIRTUAL_KEY,
};
#[cfg(windows)]
use windows::Win32::UI::WindowsAndMessaging::{
    AllowSetForegroundWindow,
    DispatchMessageW, FindWindowW, GetForegroundWindow, GetMessageW, GetWindowRect,
    GetWindowTextW, GetWindowThreadProcessId, IsIconic, IsWindowVisible, MSG, PostThreadMessageW,
    SetWindowsHookExW, TranslateMessage, UnhookWindowsHookEx, WH_KEYBOARD_LL, WH_MOUSE_LL, HHOOK, WM_QUIT,
    GetGUIThreadInfo, GUITHREADINFO,
    PostMessageW, SendMessageW, SetForegroundWindow, ShowWindow,
    SW_RESTORE, WM_CLOSE,
};

#[cfg(windows)]
const CREATE_NEW_CONSOLE: u32 = 0x00000010;
#[cfg(windows)]
const CREATE_NO_WINDOW: u32 = 0x08000000;

#[cfg(target_os = "macos")]
#[link(name = "ApplicationServices", kind = "framework")]
#[link(name = "CoreGraphics", kind = "framework")]
extern "C" {
    fn CGEventCreateKeyboardEvent(source: *mut std::ffi::c_void, virtual_key: u16, key_down: bool) -> *mut std::ffi::c_void;
    fn CGEventSetFlags(event: *mut std::ffi::c_void, flags: u64);
    fn CGEventPost(tap: u32, event: *mut std::ffi::c_void);
    fn CFRelease(cf: *mut std::ffi::c_void);

    fn AXUIElementCreateSystemWide() -> *mut std::ffi::c_void;
    fn AXUIElementCopyAttributeValue(
        element: *mut std::ffi::c_void,
        attribute: *const std::ffi::c_void,
        value: *mut *mut std::ffi::c_void,
    ) -> i32;
    fn AXValueGetValue(value: *mut std::ffi::c_void, the_type: u32, value_ptr: *mut std::ffi::c_void) -> bool;

    fn CFStringCreateWithCString(alloc: *mut std::ffi::c_void, c_str: *const std::os::raw::c_char, encoding: u32) -> *mut std::ffi::c_void;
    fn CFStringGetLength(the_string: *mut std::ffi::c_void) -> isize;
    fn CFStringGetCString(the_string: *mut std::ffi::c_void, buffer: *mut std::os::raw::c_char, buffer_size: isize, encoding: u32) -> bool;
    fn CFGetTypeID(cf: *mut std::ffi::c_void) -> usize;
    fn CFStringGetTypeID() -> usize;
}

#[derive(Clone, Debug)]
pub struct SwitchWindowTarget {
    pub id: isize,
    pub title: String,
    pub click_x: i32,
    pub click_y: i32,
    pub rect: [i32; 4],
}

#[cfg(windows)]
pub fn collect_switch_candidate_windows() -> Vec<SwitchWindowTarget> {
    let user_windows = list_user_windows();
    let overlay_hwnd = unsafe {
        FindWindowW(None, w!("Libertide overlay")).unwrap_or(HWND(std::ptr::null_mut()))
    };
    let current_pid = std::process::id();

    let mut candidates = Vec::new();

    for (hwnd, title) in user_windows {
        if hwnd.0.is_null() || hwnd == overlay_hwnd {
            continue;
        }
        let (pid, exe_name) = get_window_process_info(hwnd);
        if pid == current_pid || is_window_hung(hwnd) {
            continue;
        }

        let is_iconic = unsafe { IsIconic(hwnd).as_bool() };
        let is_visible = unsafe { IsWindowVisible(hwnd).as_bool() };

        if !is_visible || (!is_iconic && is_window_cloaked(hwnd)) {
            continue;
        }

        let t_clean = title.trim();
        let t_lower = t_clean.to_lowercase();
        let exe_lower = exe_name.to_lowercase();

        // Filtrage drastique des fenêtres parasites et widgets
        let is_parasite = t_clean.is_empty()
            || t_lower == "program manager"
            || t_lower.contains("volet")
            || t_lower.contains("widget")
            || t_lower.contains("windows input experience")
            || t_lower.contains("shellexperiencehost")
            || t_lower.contains("taskbar")
            || t_lower.contains("barre des tâches")
            || (t_lower == "paramètres" && exe_lower.contains("applicationframehost") && t_clean.len() <= 2)
            || exe_lower.contains("widget")
            || exe_lower.contains("searchapp")
            || exe_lower.contains("startmenuexperiencehost");

        if is_parasite {
            continue;
        }

        let rect = if is_iconic {
            // Récupérer la géométrie normale mémorisée de la fenêtre minimisée
            let mut wp = windows::Win32::UI::WindowsAndMessaging::WINDOWPLACEMENT::default();
            wp.length = std::mem::size_of::<windows::Win32::UI::WindowsAndMessaging::WINDOWPLACEMENT>() as u32;
            if unsafe { windows::Win32::UI::WindowsAndMessaging::GetWindowPlacement(hwnd, &mut wp).is_ok() } {
                let r = wp.rcNormalPosition;
                let w = r.right - r.left;
                let h = r.bottom - r.top;
                if w >= 120 && h >= 80 {
                    [r.left, r.top, r.right, r.bottom]
                } else {
                    let wa = get_desktop_work_area();
                    [wa.left, wa.top, wa.right, wa.bottom]
                }
            } else {
                let wa = get_desktop_work_area();
                [wa.left, wa.top, wa.right, wa.bottom]
            }
        } else {
            let mut r = RECT::default();
            let rect_ok = unsafe { GetWindowRect(hwnd, &mut r).is_ok() };
            if !rect_ok {
                continue;
            }
            let width = r.right - r.left;
            let height = r.bottom - r.top;
            if width < 120 || height < 80 {
                continue;
            }
            [r.left, r.top, r.right, r.bottom]
        };

        let click_x = rect[0] + ((rect[2] - rect[0]) / 2);
        let click_y = (rect[1] + 30).min(rect[3] - 10);

        let friendly_title = resolve_human_friendly_window_title(t_clean, &exe_name);
        let display_title = if is_iconic {
            format!("{friendly_title} (réduite)")
        } else {
            friendly_title
        };

        candidates.push(SwitchWindowTarget {
            id: hwnd.0 as isize,
            title: display_title,
            click_x,
            click_y,
            rect,
        });
    }

    candidates
}

#[cfg(not(windows))]
#[cfg(target_os = "macos")]
pub fn collect_switch_candidate_windows() -> Vec<SwitchWindowTarget> {
    const JXA_WINS: &str = r#"(() => {
        try {
            const se = Application("System Events");
            const procs = se.applicationProcesses.whose({ backgroundOnly: false, visible: true });
            let list = [];
            for (let i = 0; i < procs.length; i++) {
                const p = procs[i];
                const pName = p.name();
                const wins = p.windows();
                for (let j = 0; j < wins.length; j++) {
                    const w = wins[j];
                    const title = w.name() || pName;
                    if (!title || title.indexOf("Libertide") !== -1) continue;
                    let pos = [100, 100];
                    let sz = [800, 600];
                    try { pos = w.position(); } catch(e) {}
                    try { sz = w.size(); } catch(e) {}
                    list.push({
                        id: pName + "_" + j,
                        title: pName + " - " + title,
                        x: Math.round(pos[0]),
                        y: Math.round(pos[1]),
                        width: Math.round(sz[0]),
                        height: Math.round(sz[1])
                    });
                }
            }
            return JSON.stringify(list);
        } catch(e) { return "[]"; }
    })()"#;

    #[derive(serde::Deserialize)]
    struct MacWinRaw {
        id: String,
        title: String,
        x: i32,
        y: i32,
        width: i32,
        height: i32,
    }

    let Ok(output) = std::process::Command::new("osascript")
        .args(["-l", "JavaScript", "-e", JXA_WINS])
        .output()
    else {
        return Vec::new();
    };

    let items: Vec<MacWinRaw> = serde_json::from_slice(&output.stdout).unwrap_or_default();
    items
        .into_iter()
        .filter(|w| w.width >= 100 && w.height >= 60)
        .map(|w| {
            let hash = w.id.as_bytes().iter().fold(0isize, |acc, &b| acc.wrapping_mul(31).wrapping_add(b as isize));
            SwitchWindowTarget {
                id: hash.abs().max(1),
                title: w.title,
                click_x: w.x + (w.width / 2),
                click_y: (w.y + 30).min(w.y + w.height - 10),
                rect: [w.x, w.y, w.x + w.width, w.y + w.height],
            }
        })
        .collect()
}

#[cfg(all(not(windows), not(target_os = "macos")))]
pub fn collect_switch_candidate_windows() -> Vec<SwitchWindowTarget> {
    let output = std::process::Command::new("wmctrl")
        .args(["-l", "-G"])
        .output();

    let mut candidates = Vec::new();
    if let Ok(out) = output {
        let text = String::from_utf8_lossy(&out.stdout);
        for line in text.lines() {
            let parts: Vec<&str> = line.split_whitespace().collect();
            if parts.len() >= 8 {
                let win_id = parts[0];
                let title = parts[7..].join(" ");
                let t_clean = title.trim();
                if t_clean.is_empty()
                    || t_clean.contains("Libertide")
                    || t_clean.eq_ignore_ascii_case("desktop")
                {
                    continue;
                }
                let x = parts[2].parse::<i32>().unwrap_or(0);
                let y = parts[3].parse::<i32>().unwrap_or(0);
                let w = parts[4].parse::<i32>().unwrap_or(0);
                let h = parts[5].parse::<i32>().unwrap_or(0);
                if w < 100 || h < 60 {
                    continue;
                }
                let rect = [x, y, x + w, y + h];

                let hash = win_id.as_bytes().iter().fold(0isize, |acc, &b| acc.wrapping_mul(31).wrapping_add(b as isize));
                candidates.push(SwitchWindowTarget {
                    id: hash.abs().max(1),
                    title: t_clean.to_string(),
                    click_x: x + (w / 2),
                    click_y: (y + 30).min(y + h - 10),
                    rect,
                });
            }
        }
    }
    candidates
}

/// Détermine l'index de la fenêtre sous le point de l'écran donné
pub fn find_switch_window_at_point(windows: &[SwitchWindowTarget], x: i32, y: i32) -> Option<usize> {
    #[cfg(windows)]
    {
        let overlay_hwnd = unsafe {
            windows::Win32::UI::WindowsAndMessaging::FindWindowW(None, windows::core::w!("Libertide overlay")).unwrap_or(HWND(std::ptr::null_mut()))
        };
        if !overlay_hwnd.0.is_null() {
            let mut r = windows::Win32::Foundation::RECT::default();
            if unsafe { windows::Win32::UI::WindowsAndMessaging::GetWindowRect(overlay_hwnd, &mut r).is_ok() } {
                if x >= r.left && x <= r.right && y >= r.top && y <= r.bottom {
                    return None;
                }
            }
        }

        let pt = windows::Win32::Foundation::POINT { x, y };
        let child = unsafe { windows::Win32::UI::WindowsAndMessaging::WindowFromPoint(pt) };
        if !child.is_invalid() {
            let root = unsafe {
                windows::Win32::UI::WindowsAndMessaging::GetAncestor(
                    child,
                    windows::Win32::UI::WindowsAndMessaging::GA_ROOT,
                )
            };
            let top_hwnd = if !root.is_invalid() { root } else { child };
            if !overlay_hwnd.0.is_null() && (top_hwnd == overlay_hwnd || child == overlay_hwnd) {
                return None;
            }
            if let Some(pos) = windows.iter().position(|w| w.id == top_hwnd.0 as isize) {
                return Some(pos);
            }
        }
    }
    windows.iter().position(|w| {
        x >= w.rect[0] && x <= w.rect[2] && y >= w.rect[1] && y <= w.rect[3]
    })
}

#[cfg(windows)]
fn score_element_importance(e: &UiaElementInfo) -> i32 {
    let mut score = 0;
    if e.is_edit_or_textarea { score += 100; }
    else if e.is_button { score += 90; }
    else if e.is_link { score += 80; }
    else if e.is_tab { score += 70; }
    else if e.is_option { score += 60; }
    else if e.is_toggle { score += 50; }
    else if e.is_slider { score += 40; }
    else if e.is_media { score += 30; }
    else if e.is_tabindex { score += 20; }
    else if e.is_focusable { score += 10; }
    else if e.is_label { score += 1; }
    score
}

#[cfg(windows)]
pub fn collect_numbered_targets_for_window(hwnd: HWND) -> Vec<NumberedElementTarget> {
    if hwnd.0.is_null() || is_window_hung(hwnd) {
        return Vec::new();
    }
    // Stabilisation DWM et garantie de premier plan avant la capture et l'inspection UIA
    force_foreground_window(hwnd);
    restore_if_iconic(hwnd);
    std::thread::sleep(Duration::from_millis(80));

    let (_pid, exe_name) = get_window_process_info(hwnd);
    let mut win_title_buf = [0u16; 512];
    let t_len = unsafe { GetWindowTextW(hwnd, &mut win_title_buf) };
    let raw_title = if t_len > 0 {
        String::from_utf16_lossy(&win_title_buf[..t_len as usize])
    } else {
        String::new()
    };
    let win_title = resolve_human_friendly_window_title(&raw_title, &exe_name);

    let elements = list_interactive_elements(hwnd);
    let potential_targets: Vec<UiaElementInfo> = elements
        .into_iter()
        .filter(|e| {
            if e.is_offscreen {
                return false;
            }
            let n_low = e.name.trim().to_lowercase();
            let loc_low = e.localized_type.trim().to_lowercase();
            if n_low == "groupe" || n_low == "group" || loc_low == "groupe" || loc_low == "group" {
                return false;
            }
            if n_low == "séparateur" || n_low == "separateur" || n_low == "separator"
                || loc_low.contains("séparateur") || loc_low.contains("separateur") || loc_low.contains("separator")
            {
                return false;
            }
            e.is_button
                || e.is_link
                || e.is_tab
                || e.is_edit_or_textarea
                || e.is_tabindex
                || e.is_media
                || e.is_slider
                || e.is_toggle
                || e.is_option
                || e.is_label
                || e.has_action_pattern
                || !e.name.is_empty()
        })
        .collect();

    let mut final_targets_info: Vec<UiaElementInfo> = Vec::new();
    const BADGE_W: i32 = 38;
    const BADGE_H: i32 = 30;

    for elem in potential_targets {
        let badge_x = elem.rect.left;
        let badge_y = elem.rect.top - BADGE_H / 2;

        let mut overlapping_index = None;
        for (i, final_elem) in final_targets_info.iter().enumerate() {
            let final_badge_x = final_elem.rect.left;
            let final_badge_y = final_elem.rect.top - BADGE_H / 2;

            // AABB collision check for badges
            if badge_x < final_badge_x + BADGE_W &&
               badge_x + BADGE_W > final_badge_x &&
               badge_y < final_badge_y + BADGE_H &&
               badge_y + BADGE_H > final_badge_y
            {
                overlapping_index = Some(i);
                break;
            }
        }

        if let Some(idx) = overlapping_index {
            let existing_score = score_element_importance(&final_targets_info[idx]);
            let new_score = score_element_importance(&elem);
            if new_score > existing_score {
                final_targets_info[idx] = elem;
            } else if new_score == existing_score {
                if elem.area > final_targets_info[idx].area {
                    final_targets_info[idx] = elem;
                }
            }
        } else {
            final_targets_info.push(elem);
        }
    }

    let mut targets: Vec<NumberedElementTarget> = final_targets_info.into_iter()
        .take(300)
        .enumerate()
        .map(|(i, e)| {
            let display_name = if !e.name.is_empty() {
                e.name.clone()
            } else if !e.aria_label.is_empty() {
                e.aria_label.clone()
            } else if !e.value_text.is_empty() {
                e.value_text.clone()
            } else if !e.localized_type.is_empty() && !e.localized_type.eq_ignore_ascii_case("groupe") && !e.localized_type.eq_ignore_ascii_case("group") && !e.localized_type.eq_ignore_ascii_case("séparateur") {
                e.localized_type.clone()
            } else if e.is_button {
                "Bouton".to_string()
            } else if e.is_link {
                "Lien".to_string()
            } else if e.is_edit_or_textarea {
                "Champ de saisie".to_string()
            } else if e.is_tab {
                "Onglet".to_string()
            } else if e.is_label {
                "Texte".to_string()
            } else {
                format!("Élément {}", i + 1)
            };

            let role = if e.is_button {
                "bouton".to_string()
            } else if e.is_link {
                "lien".to_string()
            } else if e.is_tab {
                "onglet".to_string()
            } else if e.is_explicit_textarea {
                "zone de texte".to_string()
            } else if e.is_input {
                "champ de saisie".to_string()
            } else if e.is_select {
                "liste déroulante".to_string()
            } else if e.is_slider {
                "curseur".to_string()
            } else if e.is_toggle {
                "case à cocher".to_string()
            } else if e.is_media {
                "lecteur multimédia".to_string()
            } else if e.is_label {
                "étiquette".to_string()
            } else if !e.localized_type.is_empty() {
                e.localized_type.clone()
            } else {
                "contrôle".to_string()
            };

            let val_summary = if !e.value_text.is_empty() {
                e.value_text.clone()
            } else if !e.aria_description.is_empty() {
                e.aria_description.clone()
            } else if !e.help_text.is_empty() {
                e.help_text.clone()
            } else {
                String::new()
            };

            let mut shortcut = String::new();
            unsafe {
                if let Ok(acc) = e.element.CurrentAcceleratorKey() {
                    let s = acc.to_string();
                    if !s.trim().is_empty() {
                        shortcut = s.trim().to_string();
                    }
                }
            }
            if shortcut.is_empty() {
                if let Some(pos) = e.aria_indications.find("raccourci: ") {
                    let rest = &e.aria_indications[pos + 11..];
                    let end = rest.find(',').unwrap_or(rest.len());
                    shortcut = rest[..end].trim().to_string();
                }
            }

            NumberedElementTarget {
                index: i + 1,
                role,
                shortcut,
                click_x: e.click_x,
                click_y: e.click_y,
                rect: [e.rect.left, e.rect.top, e.rect.right, e.rect.bottom],
                name: display_name,
                value: val_summary,
                window_id: Some(hwnd.0 as isize),
            }
        })
        .collect();

    let mut wr = RECT::default();
    let has_rect = unsafe { GetWindowRect(hwnd, &mut wr).is_ok() };

    // Injection de sécurité pour IntelliJ / IDEs : si aucune zone de texte n'est listée
    let is_editor = is_editor_window(hwnd, &win_title, &exe_name);
    let has_textarea = targets.iter().any(|t| t.role == "zone de texte" || t.role == "champ de saisie");
    if is_editor && !has_textarea && has_rect {
        {
            let w = wr.right - wr.left;
            let h = wr.bottom - wr.top;
            let filename = extract_filename_from_title(&win_title);
            let display = filename.map(|f| format!("Éditeur ({f})")).unwrap_or_else(|| "Éditeur de code".to_string());
            targets.insert(0, NumberedElementTarget {
                index: 0,
                role: "zone de texte".to_string(),
                shortcut: "Ctrl+A".to_string(),
                click_x: wr.left + w / 2,
                click_y: wr.top + (h * 55 / 100),
                rect: [wr.left + 50, wr.top + 80, wr.right - 50, wr.bottom - 50],
                name: display,
                value: String::new(),
                window_id: Some(hwnd.0 as isize),
            });
            for (i, t) in targets.iter_mut().enumerate() {
                t.index = i + 1;
            }
        }
    }

    if has_rect {
        let window_rect = [wr.left, wr.top, wr.right, wr.bottom];
        crate::ocr::enrich_targets_with_ocr(hwnd, window_rect, &mut targets);
        for (i, t) in targets.iter_mut().enumerate() {
            t.index = i + 1;
        }
    }

    targets
}

/// Réconcilie les nouvelles cibles détectées avec les anciennes afin de conserver les mêmes numéros
pub fn reconcile_numbered_targets(
    new_targets: &mut [NumberedElementTarget],
    previous_targets: &[NumberedElementTarget],
) {
    if previous_targets.is_empty() || new_targets.is_empty() {
        return;
    }

    // Si les cibles proviennent de fenêtres différentes, ne pas réconcilier
    let prev_win = previous_targets.iter().find_map(|t| t.window_id);
    let new_win = new_targets.iter().find_map(|t| t.window_id);
    if let (Some(pw), Some(nw)) = (prev_win, new_win) {
        if pw != nw {
            return;
        }
    }

    let mut used_indices = std::collections::HashSet::new();
    let mut matched_indices = vec![None; new_targets.len()];

    // Passe 1A : correspondance stricte sur nom exact, même rôle et proximité physique (<= 15 px)
    for (new_idx, new_t) in new_targets.iter().enumerate() {
        if matched_indices[new_idx].is_some() {
            continue;
        }
        if let Some(old_t) = previous_targets.iter().find(|ot| {
            !used_indices.contains(&ot.index)
                && (ot.click_x - new_t.click_x).abs() <= 15
                && (ot.click_y - new_t.click_y).abs() <= 15
                && ot.role == new_t.role
                && (ot.name.is_empty() || ot.name.eq_ignore_ascii_case(&new_t.name))
        }) {
            used_indices.insert(old_t.index);
            matched_indices[new_idx] = Some(old_t.index);
        }
    }

    // Passe 1B : correspondance par nom exact et rôle même si légèrement déplacé (tolérance <= 35 px)
    for (new_idx, new_t) in new_targets.iter().enumerate() {
        if matched_indices[new_idx].is_some() {
            continue;
        }
        if !new_t.name.trim().is_empty() {
            if let Some(old_t) = previous_targets.iter().find(|ot| {
                !used_indices.contains(&ot.index)
                    && ot.role == new_t.role
                    && ot.name.eq_ignore_ascii_case(&new_t.name)
                    && (ot.click_x - new_t.click_x).abs() <= 35
                    && (ot.click_y - new_t.click_y).abs() <= 35
            }) {
                used_indices.insert(old_t.index);
                matched_indices[new_idx] = Some(old_t.index);
            }
        }
    }

    // Passe 2 : correspondance sémantique sur nom et rôle (ex: fenêtre ou liste décalée/scrollée <= 120 px)
    for (new_idx, new_t) in new_targets.iter().enumerate() {
        if matched_indices[new_idx].is_some() {
            continue;
        }
        if !new_t.name.trim().is_empty() {
            if let Some(old_t) = previous_targets.iter().find(|ot| {
                !used_indices.contains(&ot.index)
                    && ot.name.eq_ignore_ascii_case(&new_t.name)
                    && ot.role == new_t.role
                    && (ot.click_y - new_t.click_y).abs() <= 120
            }) {
                used_indices.insert(old_t.index);
                matched_indices[new_idx] = Some(old_t.index);
            }
        }
    }

    // Passe 3 : stricte proximité spatiale (<= 8 px) pour icônes ou boutons sans libellé
    for (new_idx, new_t) in new_targets.iter().enumerate() {
        if matched_indices[new_idx].is_some() {
            continue;
        }
        if let Some(old_t) = previous_targets.iter().find(|ot| {
            !used_indices.contains(&ot.index)
                && (ot.click_x - new_t.click_x).abs() <= 8
                && (ot.click_y - new_t.click_y).abs() <= 8
                && ot.role == new_t.role
        }) {
            used_indices.insert(old_t.index);
            matched_indices[new_idx] = Some(old_t.index);
        }
    }

    // Attribution des numéros conservés et allocation séquentielle des nouveaux badges
    let mut next_avail = 1;
    for (new_idx, opt_index) in matched_indices.iter().enumerate() {
        if let Some(idx) = opt_index {
            new_targets[new_idx].index = *idx;
        } else {
            while used_indices.contains(&next_avail) {
                next_avail += 1;
            }
            new_targets[new_idx].index = next_avail;
            used_indices.insert(next_avail);
            next_avail += 1;
        }
    }

    new_targets.sort_by_key(|t| t.index);
}

#[derive(Clone, Debug)]
pub struct TargetsScanRequest {
    pub hwnd: isize,
    pub win_rect: Option<[i32; 4]>,
    pub previous: Vec<NumberedElementTarget>,
    pub sequence: u64,
}

#[derive(Clone, Debug)]
pub struct TargetsScanResult {
    pub hwnd: isize,
    pub win_rect: Option<[i32; 4]>,
    pub targets: Vec<NumberedElementTarget>,
    pub sequence: u64,
}

pub fn spawn_targets_worker(
    request_rx: Receiver<TargetsScanRequest>,
    result_tx: Sender<TargetsScanResult>,
) {
    std::thread::Builder::new()
        .name("targets_scan_worker".to_string())
        .spawn(move || {
            #[cfg(windows)]
            unsafe {
                let _ = windows::Win32::System::Com::CoInitializeEx(
                    None,
                    windows::Win32::System::Com::COINIT_MULTITHREADED,
                );
            }

            while let Ok(mut req) = request_rx.recv() {
                // Si plusieurs requêtes ont été empilées, consommer pour ne traiter que la dernière
                while let Ok(newer) = request_rx.try_recv() {
                    req = newer;
                }

                #[cfg(windows)]
                let mut targets = {
                    let hwnd = HWND(req.hwnd as _);
                    if !hwnd.0.is_null() && !is_window_hung(hwnd) {
                        collect_numbered_targets_for_window(hwnd)
                    } else {
                        Vec::new()
                    }
                };
                #[cfg(not(windows))]
                let mut targets = collect_numbered_targets_for_active_window();

                reconcile_numbered_targets(&mut targets, &req.previous);

                // 1. Envoi et affichage immédiat des cibles OCR + UIA
                let _ = result_tx.send(TargetsScanResult {
                    hwnd: req.hwnd,
                    win_rect: req.win_rect,
                    targets: targets.clone(),
                    sequence: req.sequence,
                });

                // 2. En parallèle, Qwen2-VL détecte les icônes et canvas non textuels et fusionne ses badges
                if crate::local_llm::is_vlm_grounding_enabled() || crate::local_llm::is_vlm_ready() {
                    let result_tx_vlm = result_tx.clone();
                    let hwnd_val = req.hwnd;
                    let win_rect_val = req.win_rect;
                    let sequence_val = req.sequence;
                    let mut base_targets = targets;

                    std::thread::Builder::new()
                        .name("vlm_grounding_async".to_string())
                        .spawn(move || {
                            #[cfg(windows)]
                            let capture_opt = {
                                let hwnd = HWND(hwnd_val as _);
                                if let Some(r) = win_rect_val {
                                    crate::ocr::capture_window_rgba(hwnd, r)
                                } else {
                                    let mut r = RECT::default();
                                    if unsafe { GetWindowRect(hwnd, &mut r).is_ok() } {
                                        crate::ocr::capture_window_rgba(hwnd, [r.left, r.top, r.right, r.bottom])
                                    } else {
                                        None
                                    }
                                }
                            };
                            #[cfg(not(windows))]
                            let capture_opt = {
                                let rect = win_rect_val.or_else(get_active_window_rect);
                                rect.and_then(|r| crate::ocr::capture_window_rgba(HWND::NULL, r))
                            };

                            if let Some((pixels, w, h)) = capture_opt {
                                let win_rect = win_rect_val.unwrap_or([0, 0, w as i32, h as i32]);
                                let vlm_targets = crate::ocr::detect_vlm_grounding_targets(&pixels, w, h, win_rect, &base_targets);
                                if !vlm_targets.is_empty() {
                                    for mut t in vlm_targets {
                                        let next_index = base_targets.len() + 1;
                                        t.index = next_index;
                                        t.value = crate::ocr::visual_zone_color(next_index).to_string();
                                        t.window_id = Some(hwnd_val);
                                        base_targets.push(t);
                                    }
                                    let _ = result_tx_vlm.send(TargetsScanResult {
                                        hwnd: hwnd_val,
                                        win_rect: win_rect_val,
                                        targets: base_targets,
                                        sequence: sequence_val,
                                    });
                                }
                            }
                        })
                        .ok();
                }
            }
        })
        .expect("Échec du lancement du worker de ciblage des numéros");
}

/// Déclenche l'affichage immédiat des badges OCR dès l'arrivée sur la fenêtre,
/// puis enrichit l'affichage en tâche de fond avec Qwen VLM.
#[cfg(windows)]
pub fn trigger_window_scan_async(hwnd: HWND) {
    if hwnd.0.is_null() || is_window_hung(hwnd) {
        return;
    }

    let hwnd_raw = hwnd.0 as isize;
    let epoch = SCAN_EPOCH.fetch_add(1, Ordering::SeqCst) + 1;

    std::thread::Builder::new()
        .name("fast_window_ocr_scan".to_string())
        .spawn(move || {
            std::thread::sleep(Duration::from_millis(40));
            let hwnd = HWND(hwnd_raw as _);
            if SCAN_EPOCH.load(Ordering::SeqCst) != epoch {
                return;
            }

            // 1. Passe immédiate : UIA + OCR local direct
            let targets = collect_numbered_targets_for_window(hwnd);
            if SCAN_EPOCH.load(Ordering::SeqCst) != epoch {
                return;
            }

            if !targets.is_empty() {
                if SWITCH_HOOK_ENABLED.load(Ordering::Relaxed) {
                    draw_screen_numbered_badges(&targets, 1.0, None);
                    trigger_repaint();
                }
            }

            // 2. Passe en arrière-plan : Qwen VLM pour les zones et canvas
            if crate::local_llm::is_vlm_grounding_enabled() || crate::local_llm::is_vlm_ready() {
                let hwnd_val = hwnd_raw;
                let mut base_targets = targets;

                std::thread::Builder::new()
                    .name("vlm_grounding_async_window".to_string())
                    .spawn(move || {
                        let hwnd = HWND(hwnd_val as _);
                        let mut r = RECT::default();
                        let rect = if unsafe { GetWindowRect(hwnd, &mut r).is_ok() } {
                            [r.left, r.top, r.right, r.bottom]
                        } else {
                            return;
                        };

                        if let Some((pixels, w, h)) = crate::ocr::capture_window_rgba(hwnd, rect) {
                            if SCAN_EPOCH.load(Ordering::SeqCst) != epoch {
                                return;
                            }

                            let vlm_targets = crate::ocr::detect_vlm_grounding_targets(&pixels, w, h, rect, &base_targets);
                            if !vlm_targets.is_empty() && SCAN_EPOCH.load(Ordering::SeqCst) == epoch {
                                for mut t in vlm_targets {
                                    let next_index = base_targets.len() + 1;
                                    t.index = next_index;
                                    t.value = crate::ocr::visual_zone_color(next_index).to_string();
                                    t.window_id = Some(hwnd_val);
                                    base_targets.push(t);
                                }
                                if SWITCH_HOOK_ENABLED.load(Ordering::Relaxed) {
                                    draw_screen_numbered_badges(&base_targets, 1.0, None);
                                    trigger_repaint();
                                }
                            }
                        }
                    })
                    .ok();
            }
        })
        .ok();
}

#[cfg(not(windows))]
pub fn trigger_window_scan_async_target(_target: &str) {
    let epoch = SCAN_EPOCH.fetch_add(1, Ordering::SeqCst) + 1;
    std::thread::Builder::new()
        .name("fast_window_ocr_scan_nonwin".to_string())
        .spawn(move || {
            std::thread::sleep(Duration::from_millis(40));
            if SCAN_EPOCH.load(Ordering::SeqCst) != epoch {
                return;
            }
            let targets = collect_numbered_targets_for_active_window();
            if SCAN_EPOCH.load(Ordering::SeqCst) != epoch {
                return;
            }
            if !targets.is_empty() {
                if SWITCH_HOOK_ENABLED.load(Ordering::Relaxed) {
                    draw_screen_numbered_badges(&targets, 1.0, None);
                    trigger_repaint();
                }
            }

            // Passe en arrière-plan : Qwen VLM pour les icônes et canvas
            if crate::local_llm::is_vlm_grounding_enabled() || crate::local_llm::is_vlm_ready() {
                let mut base_targets = targets;
                std::thread::Builder::new()
                    .name("vlm_grounding_async_window_nonwin".to_string())
                    .spawn(move || {
                        let rect = get_active_window_rect().unwrap_or([0, 0, 1920, 1080]);
                        if let Some((pixels, w, h)) = crate::ocr::capture_window_rgba(HWND::NULL, rect) {
                            if SCAN_EPOCH.load(Ordering::SeqCst) != epoch {
                                return;
                            }
                            let vlm_targets = crate::ocr::detect_vlm_grounding_targets(&pixels, w, h, rect, &base_targets);
                            if !vlm_targets.is_empty() && SCAN_EPOCH.load(Ordering::SeqCst) == epoch {
                                for mut t in vlm_targets {
                                    let next_index = base_targets.len() + 1;
                                    t.index = next_index;
                                    t.value = crate::ocr::visual_zone_color(next_index).to_string();
                                    base_targets.push(t);
                                }
                                if SWITCH_HOOK_ENABLED.load(Ordering::Relaxed) {
                                    draw_screen_numbered_badges(&base_targets, 1.0, None);
                                    trigger_repaint();
                                }
                            }
                        }
                    })
                    .ok();
            }
        })
        .ok();
}

#[cfg(not(windows))]
pub fn collect_numbered_targets_for_window(_target: impl AsWindowTarget) -> Vec<NumberedElementTarget> {
    collect_numbered_targets_for_active_window()
}

#[cfg(windows)]
pub fn collect_numbered_targets_for_active_window() -> Vec<NumberedElementTarget> {
    let fg = unsafe { GetForegroundWindow() };
    let current_pid = std::process::id();
    let mut fg_pid = 0u32;
    let _ = unsafe { GetWindowThreadProcessId(fg, Some(&mut fg_pid)) };

    let target_hwnd = if !fg.0.is_null() && fg_pid != current_pid && !is_window_hung(fg) {
        Some(fg)
    } else {
        LAST_EXTERNAL_HWND.lock().ok().and_then(|g| *g).map(|raw| HWND(raw as _)).filter(|&h| !h.0.is_null() && !is_window_hung(h))
    };

    let target = target_hwnd.or_else(|| {
        let user_windows = list_user_windows();
        get_active_or_best_window(&user_windows)
    });

    if let Some(active_hwnd) = target {
        collect_numbered_targets_for_window(active_hwnd)
    } else {
        Vec::new()
    }
}

#[cfg(windows)]
pub fn collect_numbered_targets_for_active_window_reconciled(
    previous: &[NumberedElementTarget],
) -> Vec<NumberedElementTarget> {
    let mut targets = collect_numbered_targets_for_active_window();
    reconcile_numbered_targets(&mut targets, previous);
    targets
}

#[cfg(not(windows))]
pub fn collect_numbered_targets_for_active_window_reconciled(
    previous: &[NumberedElementTarget],
) -> Vec<NumberedElementTarget> {
    let mut targets = collect_numbered_targets_for_active_window();
    reconcile_numbered_targets(&mut targets, previous);
    targets
}

#[cfg(target_os = "macos")]
#[derive(serde::Deserialize, Debug, Clone)]
pub struct MacosAxElementRaw {
    pub name: String,
    pub role: String,
    pub x: i32,
    pub y: i32,
    pub width: i32,
    pub height: i32,
    #[serde(default)]
    pub value: String,
}

#[cfg(target_os = "macos")]
pub fn query_macos_ax_elements() -> Vec<MacosAxElementRaw> {
    const JXA_SCRIPT: &str = r#"(() => {
    try {
        const se = Application("System Events");
        const procs = se.applicationProcesses.whose({ frontmost: true });
        if (!procs.length) return "[]";
        const app = procs[0];
        let win = null;
        try {
            const wins = app.windows();
            if (wins.length > 0) win = wins[0];
        } catch(e) {}
        if (!win) return "[]";

        const targetRoles = [
            "AXButton", "AXTextField", "AXTextArea", "AXPopUpButton",
            "AXCheckBox", "AXRadioButton", "AXLink", "AXTab", "AXMenuItem", "AXComboBox", "AXStaticText"
        ];

        let elements = [];
        function walk(elem, depth) {
            if (depth > 6 || elements.length >= 60) return;
            try {
                let role = "";
                try { role = elem.role(); } catch(e) {}
                if (targetRoles.indexOf(role) !== -1) {
                    let pos = [0, 0];
                    let sz = [0, 0];
                    try { pos = elem.position(); } catch(e) {}
                    try { sz = elem.size(); } catch(e) {}
                    let name = "";
                    try { name = elem.title(); } catch(e) {}
                    if (!name) { try { name = elem.name(); } catch(e) {} }
                    if (!name) { try { name = elem.description(); } catch(e) {} }
                    let val = "";
                    try { val = elem.value(); } catch(e) {}
                    if (sz && sz[0] > 4 && sz[1] > 4) {
                        elements.push({
                            name: String(name || "").trim(),
                            role: role,
                            x: Math.round(pos[0]),
                            y: Math.round(pos[1]),
                            width: Math.round(sz[0]),
                            height: Math.round(sz[1]),
                            value: String(val || "").trim()
                        });
                    }
                }
                let children = [];
                try { children = elem.uiElements(); } catch(e) {}
                for (let i = 0; i < children.length; i++) {
                    walk(children[i], depth + 1);
                    if (elements.length >= 60) break;
                }
            } catch(e) {}
        }
        walk(win, 0);
        return JSON.stringify(elements);
    } catch(err) {
        return "[]";
    }
})()"#;

    let Ok(output) = std::process::Command::new("osascript")
        .args(["-l", "JavaScript", "-e", JXA_SCRIPT])
        .output()
    else {
        return Vec::new();
    };

    if !output.status.success() {
        return Vec::new();
    }
    let stdout = String::from_utf8_lossy(&output.stdout);
    serde_json::from_str(&stdout).unwrap_or_default()
}

#[cfg(target_os = "macos")]
pub fn collect_numbered_targets_for_active_window() -> Vec<NumberedElementTarget> {
    let items = query_macos_ax_elements();
    let mut targets: Vec<NumberedElementTarget> = items
        .into_iter()
        .take(300)
        .enumerate()
        .map(|(i, e)| NumberedElementTarget {
            index: i + 1,
            click_x: e.x + (e.width / 2),
            click_y: e.y + (e.height / 2),
            rect: [e.x, e.y, e.x + e.width, e.y + e.height],
            name: if !e.name.is_empty() {
                e.name.clone()
            } else if !e.value.is_empty() {
                e.value.clone()
            } else {
                format!("[{}]", e.role.strip_prefix("AX").unwrap_or(&e.role))
            },
            role: match e.role.strip_prefix("AX").unwrap_or(&e.role) {
                "StaticText" => "étiquette".to_string(),
                "TextField" => "champ de saisie".to_string(),
                "TextArea" => "zone de texte".to_string(),
                "Button" => "bouton".to_string(),
                "Link" => "lien".to_string(),
                "Tab" => "onglet".to_string(),
                other => other.to_lowercase(),
            },
            shortcut: String::new(),
            value: e.value,
            window_id: None,
        })
        .collect();

    if let Some(rect) = get_active_window_rect() {
        crate::ocr::enrich_targets_with_ocr(HWND::NULL, rect, &mut targets);
        for (i, t) in targets.iter_mut().enumerate() {
            t.index = i + 1;
        }
    }
    targets
}

#[cfg(all(not(windows), not(target_os = "macos")))]
#[derive(serde::Deserialize, Debug, Clone)]
pub struct AtspiElementRaw {
    pub name: String,
    pub role: String,
    pub x: i32,
    pub y: i32,
    pub width: i32,
    pub height: i32,
    #[serde(default)]
    pub value: String,
}

#[cfg(all(not(windows), not(target_os = "macos")))]
pub fn query_atspi_elements() -> Vec<AtspiElementRaw> {
    // Résolution de l'adresse du bus d'accessibilité (org.a11y.Bus)
    let bus_addr = std::env::var("AT_SPI_BUS_ADDRESS").ok().filter(|s| !s.trim().is_empty()).or_else(|| {
        let session = zbus::blocking::Connection::session().ok()?;
        let reply: Result<String, _> = session.call_method(
            Some("org.a11y.Bus"),
            "/org/a11y/bus",
            Some("org.a11y.Bus"),
            "GetAddress",
            &(),
        ).and_then(|r| r.body().deserialize());
        reply.ok().filter(|s| !s.trim().is_empty())
    });

    let Some(addr) = bus_addr else {
        return Vec::new();
    };

    let Ok(conn) = zbus::blocking::connection::Builder::address(addr.as_str())
        .and_then(|b| b.build())
    else {
        return Vec::new();
    };

    let mut elements = Vec::new();

    fn walk_node(
        conn: &zbus::blocking::Connection,
        dest: &str,
        path: &str,
        depth: usize,
        elements: &mut Vec<AtspiElementRaw>,
    ) {
        if depth > 7 || elements.len() >= 120 {
            return;
        }

        // Récupération du rôle
        let role_res: Result<String, _> = conn.call_method(
            Some(dest),
            path,
            Some("org.a11y.atspi.Accessible"),
            "GetRoleName",
            &(),
        ).and_then(|r| r.body().deserialize());

        let role = role_res.unwrap_or_default();
        let is_target_role = matches!(
            role.as_str(),
            "push-button" | "link" | "entry" | "password-text" | "page-tab"
                | "check-box" | "radio-button" | "combo-box" | "label" | "text"
        );

        if is_target_role {
            // ATSPI_COORD_TYPE_SCREEN = 0
            let extents_res: Result<(i32, i32, i32, i32), _> = conn.call_method(
                Some(dest),
                path,
                Some("org.a11y.atspi.Component"),
                "GetExtents",
                &(0u32),
            ).and_then(|r| r.body().deserialize());

            if let Ok((x, y, width, height)) = extents_res {
                if width > 5 && height > 5 {
                    let name_res: Result<String, _> = conn.call_method(
                        Some(dest),
                        path,
                        Some("org.a11y.atspi.Accessible"),
                        "getName",
                        &(),
                    ).and_then(|r| r.body().deserialize())
                    .or_else(|_| {
                        conn.call_method(
                            Some(dest),
                            path,
                            Some("org.freedesktop.DBus.Properties"),
                            "Get",
                            &("org.a11y.atspi.Accessible", "Name"),
                        ).and_then(|r| {
                            let val: zbus::zvariant::OwnedValue = r.body().deserialize()?;
                            zbus::zvariant::Value::from(val).downcast::<String>().map_err(zbus::Error::Variant)
                        })
                    });

                    let mut val_text = String::new();
                    let text_res: Result<String, _> = conn.call_method(
                        Some(dest),
                        path,
                        Some("org.a11y.atspi.Text"),
                        "GetText",
                        &(0i32, -1i32),
                    ).and_then(|r| r.body().deserialize());
                    if let Ok(txt) = text_res {
                        val_text = txt.trim().to_string();
                    }

                    elements.push(AtspiElementRaw {
                        name: name_res.unwrap_or_default().trim().to_string(),
                        role,
                        x,
                        y,
                        width,
                        height,
                        value: val_text,
                    });
                }
            }
        }

        // Parcours récursif des enfants
        let children_res: Result<Vec<(String, zbus::zvariant::OwnedObjectPath)>, _> = conn.call_method(
            Some(dest),
            path,
            Some("org.a11y.atspi.Accessible"),
            "GetChildren",
            &(),
        ).and_then(|r| r.body().deserialize());

        if let Ok(children) = children_res {
            for (child_dest, child_path) in children {
                walk_node(conn, &child_dest, child_path.as_str(), depth + 1, elements);
                if elements.len() >= 120 {
                    break;
                }
            }
        }
    }

    walk_node(
        &conn,
        "org.a11y.atspi.Registry",
        "/org/a11y/atspi/accessible/root",
        0,
        &mut elements,
    );

    elements
}

#[cfg(all(not(windows), not(target_os = "macos")))]
pub fn collect_numbered_targets_for_active_window() -> Vec<NumberedElementTarget> {
    let items = query_atspi_elements();
    let mut targets: Vec<NumberedElementTarget> = items
        .into_iter()
        .take(300)
        .enumerate()
        .map(|(i, e)| NumberedElementTarget {
            index: i + 1,
            click_x: e.x + (e.width / 2),
            click_y: e.y + (e.height / 2),
            rect: [e.x, e.y, e.x + e.width, e.y + e.height],
            name: if !e.name.is_empty() {
                e.name.clone()
            } else if !e.value.is_empty() {
                e.value.clone()
            } else {
                format!("[{}]", e.role)
            },
            role: match e.role.as_str() {
                "label" | "text" => "étiquette".to_string(),
                "entry" | "password-text" => "champ de saisie".to_string(),
                "push-button" => "bouton".to_string(),
                "link" => "lien".to_string(),
                "page-tab" => "onglet".to_string(),
                other => other.to_string(),
            },
            shortcut: String::new(),
            value: e.value,
            window_id: None,
        })
        .collect();

    if let Some(rect) = get_active_window_rect() {
        crate::ocr::enrich_targets_with_ocr(HWND::NULL, rect, &mut targets);
        for (i, t) in targets.iter_mut().enumerate() {
            t.index = i + 1;
        }
    }
    targets
}

#[cfg(not(windows))]
pub trait AsWindowTarget {
    fn to_window_target(&self) -> String;
}

#[cfg(not(windows))]
impl AsWindowTarget for &str {
    fn to_window_target(&self) -> String {
        self.to_string()
    }
}

#[cfg(not(windows))]
impl AsWindowTarget for String {
    fn to_window_target(&self) -> String {
        self.clone()
    }
}

#[cfg(not(windows))]
impl AsWindowTarget for &String {
    fn to_window_target(&self) -> String {
        (*self).clone()
    }
}

#[cfg(not(windows))]
impl AsWindowTarget for () {
    fn to_window_target(&self) -> String {
        String::new()
    }
}

#[cfg(not(windows))]
impl AsWindowTarget for crate::ocr::HWND {
    fn to_window_target(&self) -> String {
        String::new()
    }
}

#[cfg(not(windows))]
impl AsWindowTarget for isize {
    fn to_window_target(&self) -> String {
        self.to_string()
    }
}

#[cfg(not(windows))]
#[derive(Clone, Copy, Debug, PartialEq, Eq, Default)]
pub struct VIRTUAL_KEY(pub u16);

#[cfg(all(not(windows), not(target_os = "macos")))]
pub fn is_wayland_session() -> bool {
    std::env::var("WAYLAND_DISPLAY").map(|v| !v.trim().is_empty()).unwrap_or(false)
        || std::env::var("XDG_SESSION_TYPE").map(|v| v.trim().eq_ignore_ascii_case("wayland")).unwrap_or(false)
}

pub fn format_plural_control_type(targets: &[NumberedElementTarget]) -> String {
    let count = targets.len();
    if count == 0 {
        return "contrôles".to_string();
    }
    let btn_count = targets.iter().filter(|t| t.role == "bouton").count();
    let link_count = targets.iter().filter(|t| t.role == "lien").count();
    let tab_count = targets.iter().filter(|t| t.role == "onglet").count();
    let input_count = targets.iter().filter(|t| t.role == "champ de saisie" || t.role == "zone de texte").count();
    let label_count = targets.iter().filter(|t| t.role == "étiquette" || t.role == "label").count();

    if btn_count * 2 >= count {
        format!("{count} bouton{}", if count > 1 { "s" } else { "" })
    } else if link_count * 2 >= count {
        format!("{count} lien{}", if count > 1 { "s" } else { "" })
    } else if tab_count * 2 >= count {
        format!("{count} onglet{}", if count > 1 { "s" } else { "" })
    } else if input_count * 2 >= count {
        format!("{count} champ{}", if count > 1 { "s" } else { "" })
    } else if label_count * 2 >= count {
        format!("{count} étiquette{}", if count > 1 { "s" } else { "" })
    } else {
        format!("{count} élément{}", if count > 1 { "s" } else { "" })
    }
}

pub fn inject_text_reader_target(zone: &mut NavigationZone) {
    if zone.targets.iter().any(|t| t.role == "lecture textuelle") {
        return;
    }

    let click_x = (zone.rect[0] + zone.rect[2]) / 2;
    let click_y = zone.rect[1] + 16;
    let reader_target = NumberedElementTarget {
        index: 1,
        click_x,
        click_y,
        rect: [zone.rect[0], zone.rect[1], (zone.rect[0] + 200).min(zone.rect[2]), (zone.rect[1] + 32).min(zone.rect[3])],
        name: format!("📖 Lecture / Extraction texte ({})", zone.title),
        role: "lecture textuelle".to_string(),
        shortcut: "Entrée".to_string(),
        value: String::new(),
        window_id: zone.targets.first().and_then(|t| t.window_id),
    };

    zone.targets.insert(0, reader_target);
    for (i, t) in zone.targets.iter_mut().enumerate() {
        t.index = i + 1;
    }
}

/// Active et clique sur une cible numérotée (refocus de la fenêtre hôte + positionnement physique du curseur + clic)
pub fn activate_numbered_target(target: &NumberedElementTarget) -> bool {
    #[cfg(windows)]
    {
        if let Some(win_id) = target.window_id {
            let hwnd = HWND(win_id as _);
            if !hwnd.0.is_null() && !is_window_hung(hwnd) {
                force_foreground_window(hwnd);
                restore_if_iconic(hwnd);
                std::thread::sleep(Duration::from_millis(40));
            }
        }
        unsafe {
            let _ = windows::Win32::UI::WindowsAndMessaging::SetCursorPos(target.click_x, target.click_y);
        }
        std::thread::sleep(Duration::from_millis(20));
        click_element(target.click_x, target.click_y, None, None);
        true
    }
    #[cfg(not(windows))]
    {
        set_global_cursor_pos(target.click_x, target.click_y);
        crate::ui::click_element(target.click_x, target.click_y, None, None);
        true
    }
}

pub fn click_numbered_target(target: &NumberedElementTarget) -> bool {
    activate_numbered_target(target)
}

pub fn build_targeted_hover_speech(
    _macro_zone_title: Option<&str>,
    _zone_targets: &[NumberedElementTarget],
    target_index: usize,
    target_name: &str,
    target_role: &str,
    _target_shortcut: &str,
    _is_same_zone_as_last: bool,
) -> String {
    let clean_name = target_name.trim();
    let clean_role = target_role.trim();

    let base_speech = if clean_role.eq_ignore_ascii_case("fenêtre") || clean_role.eq_ignore_ascii_case("window") {
        if is_human_friendly_window_title(clean_name) {
            format!("Fenêtre : {clean_name}")
        } else {
            "Fenêtre".to_string()
        }
    } else if clean_role.eq_ignore_ascii_case("lecture textuelle") {
        if !clean_name.is_empty() {
            clean_name.to_string()
        } else {
            "Lecture textuelle".to_string()
        }
    } else if clean_role.eq_ignore_ascii_case("visuel (ocr)") || clean_role.eq_ignore_ascii_case("ocr") {
        if !clean_name.is_empty() {
            format!("Texte : {clean_name}")
        } else {
            "Zone visuelle".to_string()
        }
    } else if clean_role.eq_ignore_ascii_case("étiquette") || clean_role.eq_ignore_ascii_case("label") {
        if !clean_name.is_empty() {
            format!("Texte : {clean_name}")
        } else {
            "Texte".to_string()
        }
    } else if clean_role.eq_ignore_ascii_case("champ de saisie") || clean_role.eq_ignore_ascii_case("zone de texte") {
        if !clean_name.is_empty() {
            format!("{clean_role} : {clean_name}")
        } else {
            clean_role.to_string()
        }
    } else if !clean_name.is_empty() {
        clean_name.to_string()
    } else if !clean_role.is_empty() {
        clean_role.to_string()
    } else {
        "Contrôle".to_string()
    };

    format!("{} : {}", target_index, base_speech)
}

#[cfg(not(windows))]
pub fn force_foreground_window(target: impl AsWindowTarget) -> bool {
    let t = target.to_window_target();
    #[cfg(target_os = "macos")]
    {
        if !t.is_empty() {
            let script = format!(
                "tell application \"System Events\" to set frontmost of (first process whose name contains \"{t}\") to true"
            );
            return std::process::Command::new("osascript")
                .args(["-e", &script])
                .status()
                .map_or(false, |s| s.success());
        }
        true
    }
    #[cfg(all(not(windows), not(target_os = "macos")))]
    {
        if !t.is_empty() {
            if t.starts_with("0x") || t.starts_with("0X") {
                let status = std::process::Command::new("wmctrl")
                    .args(["-i", "-a", &t])
                    .status();
                if status.map_or(false, |s| s.success()) {
                    return true;
                }
            }
            let status = std::process::Command::new("wmctrl")
                .args(["-a", &t])
                .status();
            if status.map_or(false, |s| s.success()) {
                return true;
            }
        }
        let status = std::process::Command::new("xdotool")
            .args(["getactivewindow", "windowactivate"])
            .status();
        status.map_or(false, |s| s.success())
    }
}

#[cfg(not(windows))]
pub fn focus_browser_address_bar(target: impl AsWindowTarget) {
    let t = target.to_window_target();
    if !t.is_empty() {
        force_foreground_window(&t);
        std::thread::sleep(Duration::from_millis(50));
    }
    const VK_CONTROL: VIRTUAL_KEY = VIRTUAL_KEY(0x11);
    const VK_L: VIRTUAL_KEY = VIRTUAL_KEY(0x4C);
    send_hotkey(&[VK_CONTROL], VK_L);
}

#[cfg(windows)]
pub fn restore_if_iconic(hwnd: HWND) {
    if !hwnd.0.is_null() && !is_window_hung(hwnd) {
        unsafe {
            if IsIconic(hwnd).as_bool() {
                let _ = ShowWindow(hwnd, SW_RESTORE);
            }
        }
    }
}

#[cfg(not(windows))]
pub fn restore_if_iconic(target: impl AsWindowTarget) {
    let t = target.to_window_target();
    if !t.is_empty() {
        #[cfg(all(not(windows), not(target_os = "macos")))]
        {
            if t.starts_with("0x") || t.starts_with("0X") {
                let _ = std::process::Command::new("wmctrl")
                    .args(["-i", "-R", &t])
                    .status();
            } else {
                let _ = std::process::Command::new("wmctrl")
                    .args(["-R", &t])
                    .status();
            }
        }
    }
}

#[cfg(windows)]
static LAST_TXT_HWND: Mutex<Option<isize>> = Mutex::new(None);
pub static LAST_EXTERNAL_HWND: Mutex<Option<isize>> = Mutex::new(None);
pub static LAST_FOCUSED_CHILD_HWND: Mutex<Option<isize>> = Mutex::new(None);
pub static LAST_FOCUSED_RECT: Mutex<Option<[i32; 4]>> = Mutex::new(None);

pub static CURRENT_REQUEST_ID: AtomicU64 = AtomicU64::new(0);
pub static SCAN_EPOCH: AtomicU64 = AtomicU64::new(0);
pub static IS_EMERGENCY_STOPPED: AtomicBool = AtomicBool::new(false);
pub static AGENT_BUSY: AtomicBool = AtomicBool::new(false);
pub static TARGET_REASONING_PASSES: AtomicUsize = AtomicUsize::new(5);
pub fn is_request_cancelled(request_id: u64) -> bool {
    CURRENT_REQUEST_ID.load(Ordering::SeqCst) != request_id || IS_EMERGENCY_STOPPED.load(Ordering::SeqCst)
}

#[cfg(windows)]
static LAST_FOCUS_UPDATE: Mutex<Option<Instant>> = Mutex::new(None);

pub static IS_IMMERSION_ACTIVE: AtomicBool = AtomicBool::new(false);
pub static IS_DICTATION_ACTIVE: AtomicBool = AtomicBool::new(false);
pub static IS_CHAT_TAB_ACTIVE: AtomicBool = AtomicBool::new(false);
pub static IMMERSION_SCREEN_HISTORY: Mutex<Vec<String>> = Mutex::new(Vec::new());
pub const LIBERTIDE_EXTRA_INFO: usize = 0x4C494254; // 'LIBT'
pub static IS_USER_TYPING: AtomicBool = AtomicBool::new(false);

type RepaintCallback = Box<dyn Fn() + Send + Sync>;
static REPAINT_CALLBACK: Mutex<Option<RepaintCallback>> = Mutex::new(None);

pub fn set_repaint_callback<F: Fn() + Send + Sync + 'static>(f: F) {
    if let Ok(mut lock) = REPAINT_CALLBACK.lock() {
        *lock = Some(Box::new(f));
    }
}

pub fn trigger_repaint() {
    if let Ok(lock) = REPAINT_CALLBACK.lock() {
        if let Some(ref cb) = *lock {
            cb();
        }
    }
}

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

        // Détection universelle (Win32 + UIA) pour les navigateurs (Chrome/Edge/Firefox) et éditeurs
        let is_text_focused = if crate::ui::has_focused_text_input(fg) {
            true
        } else {
            false
        };
        IS_EXTERNAL_TEXT_INPUT_FOCUSED.store(is_text_focused, Ordering::Relaxed);

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

#[cfg(target_os = "macos")]
pub fn update_last_external_focus() {
    unsafe {
        let system_wide = AXUIElementCreateSystemWide();
        if system_wide.is_null() {
            return;
        }
        let k_focused = CFStringCreateWithCString(std::ptr::null_mut(), b"AXFocusedUIElement\0".as_ptr() as _, 0x08000100 /* kCFStringEncodingUTF8 */);
        let mut focused_elem: *mut std::ffi::c_void = std::ptr::null_mut();
        let err = AXUIElementCopyAttributeValue(system_wide, k_focused, &mut focused_elem);
        CFRelease(k_focused);
        CFRelease(system_wide);

        if err == 0 && !focused_elem.is_null() {
            let mut rect = [0i32; 4];
            let mut has_pos = false;
            let mut has_size = false;

            // AXPosition
            let k_pos = CFStringCreateWithCString(std::ptr::null_mut(), b"AXPosition\0".as_ptr() as _, 0x08000100);
            let mut pos_val: *mut std::ffi::c_void = std::ptr::null_mut();
            if AXUIElementCopyAttributeValue(focused_elem, k_pos, &mut pos_val) == 0 && !pos_val.is_null() {
                let mut pt = [0.0f64; 2];
                if AXValueGetValue(pos_val, 1 /* kAXValueCGPointType */, pt.as_mut_ptr() as _) {
                    rect[0] = pt[0] as i32;
                    rect[1] = pt[1] as i32;
                    has_pos = true;
                }
                CFRelease(pos_val);
            }
            CFRelease(k_pos);

            // AXSize
            let k_sz = CFStringCreateWithCString(std::ptr::null_mut(), b"AXSize\0".as_ptr() as _, 0x08000100);
            let mut sz_val: *mut std::ffi::c_void = std::ptr::null_mut();
            if AXUIElementCopyAttributeValue(focused_elem, k_sz, &mut sz_val) == 0 && !sz_val.is_null() {
                let mut sz = [0.0f64; 2];
                if AXValueGetValue(sz_val, 2 /* kAXValueCGSizeType */, sz.as_mut_ptr() as _) {
                    rect[2] = rect[0] + sz[0] as i32;
                    rect[3] = rect[1] + sz[1] as i32;
                    has_size = true;
                }
                CFRelease(sz_val);
            }
            CFRelease(k_sz);
            CFRelease(focused_elem);

            if has_pos && has_size {
                if let Ok(mut rlock) = LAST_FOCUSED_RECT.lock() {
                    *rlock = Some(rect);
                }
            }
        }
    }
}

#[cfg(all(not(windows), not(target_os = "macos")))]
pub fn update_last_external_focus() {
    let elements = query_atspi_elements();
    if let Some(focused) = elements.into_iter().find(|e| !e.value.is_empty() || e.role.contains("text") || e.role.contains("entry")) {
        if let Ok(mut rlock) = LAST_FOCUSED_RECT.lock() {
            *rlock = Some([focused.x, focused.y, focused.x + focused.width, focused.y + focused.height]);
        }
    }
}

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
            let fg = unsafe { GetForegroundWindow() };
            if fg == hwnd {
                return true;
            }

            let (_pid, exe_name) = get_window_process_info(hwnd);
            let is_browser = is_browser_executable(&exe_name);

            if is_browser {
                focus_browser_address_bar(hwnd);
                return true;
            }

            force_foreground_window(hwnd);
            std::thread::sleep(Duration::from_millis(80));

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

    let old_clip = get_plain_text();
    unsafe {
        SendMessageW(hwnd, WM_SYSCOMMAND, WPARAM(IDM_COPYALL), LPARAM(0));
    }
    std::thread::sleep(Duration::from_millis(60));

    let new_clip = get_plain_text();
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

        let _ = windows::Win32::UI::WindowsAndMessaging::LockSetForegroundWindow(
            windows::Win32::UI::WindowsAndMessaging::LSFW_UNLOCK,
        );
        const ASFW_ANY: u32 = 0xFFFFFFFF;
        let _ = AllowSetForegroundWindow(ASFW_ANY);

        let current_thread_id = GetCurrentThreadId();
        let fg_thread_id = if !fg_hwnd.0.is_null() {
            GetWindowThreadProcessId(fg_hwnd, None)
        } else {
            0
        };

        if fg_thread_id != 0 && current_thread_id != fg_thread_id {
            let _ = AttachThreadInput(current_thread_id, fg_thread_id, 1);
            let _ = BringWindowToTop(hwnd);
            let _ = SetForegroundWindow(hwnd);
            let _ = AttachThreadInput(current_thread_id, fg_thread_id, 0);
        } else {
            let _ = BringWindowToTop(hwnd);
            let _ = SetForegroundWindow(hwnd);
        }
    }
    true
}

#[cfg(windows)]
fn send_key_event(vk: VIRTUAL_KEY, flags: KEYBD_EVENT_FLAGS) {
    let mut flags = flags;
    let is_extended = matches!(
        vk.0,
        0xAD..=0xB3 // VK_VOLUME_MUTE, VK_VOLUME_DOWN, VK_VOLUME_UP, VK_MEDIA_*
        | 0xB4..=0xB7 // VK_LAUNCH_MAIL, VK_LAUNCH_MEDIA_SELECT, etc.
        | 0x21..=0x28 // VK_PRIOR, VK_NEXT, VK_END, VK_HOME, VK_LEFT, VK_UP, VK_RIGHT, VK_DOWN
        | 0x2D | 0x2E // VK_INSERT, VK_DELETE
        | 0x5B..=0x5D // VK_LWIN, VK_RWIN, VK_APPS
        | 0x6F        // VK_DIVIDE
    );
    if is_extended {
        flags |= KEYEVENTF_EXTENDEDKEY;
    }
    unsafe {
        keybd_event(vk.0 as u8, 0, flags, LIBERTIDE_EXTRA_INFO);
    }
}

#[cfg(windows)]
pub fn send_paste() {
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

#[cfg(not(windows))]
pub fn send_paste() {
    #[cfg(target_os = "macos")]
    {
        const VK_CONTROL: VIRTUAL_KEY = VIRTUAL_KEY(0x11);
        const VK_V: VIRTUAL_KEY = VIRTUAL_KEY(0x56);
        send_hotkey(&[VK_CONTROL], VK_V);
    }
    #[cfg(all(not(windows), not(target_os = "macos")))]
    {
        const VK_CONTROL: VIRTUAL_KEY = VIRTUAL_KEY(0x11);
        const VK_V: VIRTUAL_KEY = VIRTUAL_KEY(0x56);
        send_hotkey(&[VK_CONTROL], VK_V);
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

#[cfg(not(windows))]
pub fn send_hotkey(modifiers: &[VIRTUAL_KEY], key: VIRTUAL_KEY) {
    #[cfg(target_os = "macos")]
    {
        let mac_keycode: u16 = match key.0 {
            0x0D => 36,  // Return
            0x09 => 48,  // Tab
            0x20 => 49,  // Space
            0x08 => 51,  // Delete/Backspace
            0x1B => 53,  // Escape
            0x25 => 123, // Left
            0x27 => 124, // Right
            0x28 => 125, // Down
            0x26 => 126, // Up
            0x41 => 0,   // A
            0x53 => 1,   // S
            0x44 => 2,   // D
            0x46 => 3,   // F
            0x48 => 4,   // H
            0x47 => 5,   // G
            0x5A => 6,   // Z
            0x58 => 7,   // X
            0x43 => 8,   // C
            0x56 => 9,   // V
            0x42 => 11,  // B
            0x51 => 12,  // Q
            0x57 => 13,  // W
            0x45 => 14,  // E
            0x52 => 15,  // R
            0x59 => 16,  // Y
            0x54 => 17,  // T
            0x31 => 18,  // 1
            0x32 => 19,  // 2
            0x33 => 20,  // 3
            0x34 => 21,  // 4
            0x36 => 22,  // 6
            0x35 => 23,  // 5
            0x39 => 25,  // 9
            0x37 => 26,  // 7
            0x38 => 28,  // 8
            0x30 => 29,  // 0
            0x4F => 31,  // O
            0x55 => 32,  // U
            0x49 => 34,  // I
            0x50 => 35,  // P
            0x4C => 37,  // L
            0x4A => 38,  // J
            0x4B => 40,  // K
            0x4E => 45,  // N
            0x4D => 46,  // M
            0x74 => 96,  // F5
            _ => 0,
        };

        let mut flags: u64 = 0;
        for m in modifiers {
            match m.0 {
                0x11 => flags |= 0x00100000, // kCGEventFlagMaskCommand (Cmd remplace Ctrl standard)
                0x10 => flags |= 0x00020000, // kCGEventFlagMaskShift
                0x12 => flags |= 0x00080000, // kCGEventFlagMaskAlternate (Option)
                0x5B | 0x5C => flags |= 0x00040000, // kCGEventFlagMaskControl
                _ => {}
            }
        }

        unsafe {
            let ev_down = CGEventCreateKeyboardEvent(std::ptr::null_mut(), mac_keycode, true);
            if !ev_down.is_null() {
                CGEventSetFlags(ev_down, flags);
                CGEventPost(0 /* kCGHIDEventTap */, ev_down);
                CFRelease(ev_down);
            }
            std::thread::sleep(Duration::from_millis(20));
            let ev_up = CGEventCreateKeyboardEvent(std::ptr::null_mut(), mac_keycode, false);
            if !ev_up.is_null() {
                CGEventSetFlags(ev_up, flags);
                CGEventPost(0 /* kCGHIDEventTap */, ev_up);
                CFRelease(ev_up);
            }
        }
    }

    #[cfg(all(not(windows), not(target_os = "macos")))]
    {
        let rdev_key_opt = match key.0 {
            0x0D => Some(rdev::Key::Return),
            0x1B => Some(rdev::Key::Escape),
            0x09 => Some(rdev::Key::Tab),
            0x20 => Some(rdev::Key::Space),
            0x08 => Some(rdev::Key::Backspace),
            0x25 => Some(rdev::Key::LeftArrow),
            0x26 => Some(rdev::Key::UpArrow),
            0x27 => Some(rdev::Key::RightArrow),
            0x28 => Some(rdev::Key::DownArrow),
            0x21 => Some(rdev::Key::PageUp),
            0x22 => Some(rdev::Key::PageDown),
            0x24 => Some(rdev::Key::Home),
            0x23 => Some(rdev::Key::End),
            0x41 => Some(rdev::Key::KeyA),
            0x43 => Some(rdev::Key::KeyC),
            0x44 => Some(rdev::Key::KeyD),
            0x4C => Some(rdev::Key::KeyL),
            0x53 => Some(rdev::Key::KeyS),
            0x54 => Some(rdev::Key::KeyT),
            0x56 => Some(rdev::Key::KeyV),
            0x57 => Some(rdev::Key::KeyW),
            0x59 => Some(rdev::Key::KeyY),
            0x5A => Some(rdev::Key::KeyZ),
            0x74 => Some(rdev::Key::F5),
            _ => None,
        };

        let rdev_mods: Vec<rdev::Key> = modifiers.iter().filter_map(|m| match m.0 {
            0x11 => Some(rdev::Key::ControlLeft),
            0x10 => Some(rdev::Key::ShiftLeft),
            0x12 => Some(rdev::Key::Alt),
            0x5B | 0x5C => Some(rdev::Key::MetaLeft),
            _ => None,
        }).collect();

        for &m in &rdev_mods {
            let _ = rdev::simulate(&rdev::EventType::KeyPress(m));
        }
        if let Some(k) = rdev_key_opt {
            let _ = rdev::simulate(&rdev::EventType::KeyPress(k));
            std::thread::sleep(Duration::from_millis(20));
            let _ = rdev::simulate(&rdev::EventType::KeyRelease(k));
        }
        for &m in rdev_mods.iter().rev() {
            let _ = rdev::simulate(&rdev::EventType::KeyRelease(m));
        }
    }
}

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
            "volume_mute" | "volumemute" | "mute" => key = Some(VIRTUAL_KEY(0xAD)),
            "volume_down" | "volumedown" | "voldown" => key = Some(VIRTUAL_KEY(0xAE)),
            "volume_up" | "volumeup" | "volup" => key = Some(VIRTUAL_KEY(0xAF)),
            "media_next" | "medianext" => key = Some(VIRTUAL_KEY(0xB0)),
            "media_prev" | "mediaprev" => key = Some(VIRTUAL_KEY(0xB1)),
            "media_stop" | "mediastop" => key = Some(VIRTUAL_KEY(0xB2)),
            "media_play_pause" | "playpause" => key = Some(VIRTUAL_KEY(0xB3)),
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

pub fn urldecoding_simple(input: &str) -> String {
    let mut out = Vec::new();
    let bytes = input.as_bytes();
    let mut i = 0;
    while i < bytes.len() {
        if bytes[i] == b'+' {
            out.push(b' ');
            i += 1;
        } else if bytes[i] == b'%' && i + 2 < bytes.len() {
            if let Ok(byte) = u8::from_str_radix(std::str::from_utf8(&bytes[i + 1..i + 3]).unwrap_or(""), 16) {
                out.push(byte);
                i += 3;
            } else {
                out.push(bytes[i]);
                i += 1;
            }
        } else {
            out.push(bytes[i]);
            i += 1;
        }
    }
    String::from_utf8_lossy(&out).to_string()
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
    let t = target.trim().to_lowercase().replace('_', " ").replace('-', " ");
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
        || clean.contains("barre de recherche")
        || clean.contains("search bar")
        || clean.contains("searchbar")
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

pub fn is_window_control_target(target: &str) -> bool {
    let t = target.trim().to_lowercase();
    t.contains("fermer la fenêtre")
        || t.contains("fermer la fenetre")
        || t.contains("fermer (croix")
        || t.contains("close window")
        || t.contains("agrandir la fenêtre")
        || t.contains("agrandir la fenetre")
        || t.contains("agrandir / restaurer")
        || t.contains("réduire la fenêtre")
        || t.contains("reduire la fenetre")
        || t.contains("restaurer la fenêtre")
        || t.contains("restaurer la fenetre")
        || t.contains("menu système")
        || t.contains("contrôle fenêtre")
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

fn clean_search_url_if_present(url: &str) -> Option<String> {
    let lower = url.to_lowercase();
    let (prefix, query_key) = if lower.contains("google.") && lower.contains("/search?") {
        ("https://www.google.com/search?q=", "q=")
    } else if lower.contains("youtube.com/results?") {
        ("https://www.youtube.com/results?search_query=", "search_query=")
    } else if lower.contains("bing.com/search?") {
        ("https://www.bing.com/search?q=", "q=")
    } else if lower.contains("duckduckgo.com/") && lower.contains("q=") {
        ("https://duckduckgo.com/?q=", "q=")
    } else if lower.contains("arte.tv") && lower.contains("/search/?q=") {
        ("https://www.arte.tv/fr/search/?q=", "q=")
    } else if lower.contains("twitch.tv/search?") {
        ("https://www.twitch.tv/search?term=", "term=")
    } else {
        return None;
    };

    if let Some(pos) = lower.find(query_key) {
        let after_key = &url[pos + query_key.len()..];
        let raw_query = if let Some(amp_pos) = after_key.find('&') {
            &after_key[..amp_pos]
        } else {
            after_key
        };
        let decoded = urldecoding_simple(raw_query);
        let cleaned = crate::intent::clean_search_query_intent(&decoded);
        if !cleaned.is_empty() && cleaned != decoded {
            let reencoded = urlencoding_simple(&cleaned);
            return Some(format!("{prefix}{reencoded}"));
        }
    }
    None
}

pub fn format_url_for_navigation(raw_url: &str) -> String {
    let trimmed = raw_url.trim();
    if trimmed.is_empty() {
        return "https://www.google.com".to_string();
    }

    // Si c'est déjà une URL de recherche (Google, YouTube, Bing...), on assainit la requête
    if let Some(cleaned_search_url) = clean_search_url_if_present(trimmed) {
        return cleaned_search_url;
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
        // 5. Repli : terme de recherche vers Google avec extraction de l'intention réelle
        let intent_query = crate::intent::clean_search_query_intent(trimmed);
        format!("https://www.google.com/search?q={}", urlencoding_simple(&intent_query))
    }
}

/// Ouvre de manière intelligente et asynchrone une URL dans le navigateur
/// (amène la fenêtre au premier plan et ouvre un nouvel onglet si le navigateur existe déjà,
/// sinon lance le navigateur avec l'URL ou appelle le navigateur par défaut du système).
pub fn open_browser_smart(url: &str) {
    let url = url.to_string();
    std::thread::spawn(move || {
        let formatted = format_url_for_navigation(&url);
        #[cfg(windows)]
        {
            let user_windows = list_user_windows();
            if let Some(hwnd) = find_best_browser_window(&user_windows, None) {
                if !is_window_hung(hwnd) {
                    force_foreground_window(hwnd);
                    restore_if_iconic(hwnd);
                    open_browser_new_tab(hwnd, &formatted);
                    return;
                }
            }
            if launch_browser_new_window(&formatted).is_none() {
                let _ = open_path_with_default_app(&formatted);
            }
        }
        #[cfg(target_os = "macos")]
        {
            let user_windows = list_user_windows();
            if let Some(target) = find_best_browser_window(&user_windows, None) {
                force_foreground_window(&target);
                open_browser_new_tab(&target, &formatted);
            } else {
                let _ = open_path_with_default_app(&formatted);
            }
        }
        #[cfg(all(not(windows), not(target_os = "macos")))]
        {
            let user_windows = list_user_windows();
            if let Some(target) = find_best_browser_window(&user_windows, None) {
                force_foreground_window(&target);
                open_browser_new_tab(&target, &formatted);
            } else {
                let _ = open_path_with_default_app(&formatted);
            }
        }
    });
}

#[cfg(windows)]
pub fn focus_browser_address_bar(hwnd: HWND) {
    force_foreground_window(hwnd);
    restore_if_iconic(hwnd);
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
pub fn run_system_command(command: &str) -> bool {
    let trimmed = command.trim();
    if trimmed.is_empty() {
        return false;
    }

    let lower = trimmed.to_lowercase();

    // Interception des tentatives d'ouverture des paramètres son ou réglages Windows via console
    if lower.contains("ms-settings:sound") || lower.contains("mmsys.cpl") {
        const VK_LWIN: VIRTUAL_KEY = VIRTUAL_KEY(0x5B);
        const VK_A: VIRTUAL_KEY = VIRTUAL_KEY(0x41);
        send_hotkey(&[VK_LWIN], VK_A);
        return true;
    }
    if lower.contains("ms-settings:") {
        const VK_LWIN: VIRTUAL_KEY = VIRTUAL_KEY(0x5B);
        const VK_I: VIRTUAL_KEY = VIRTUAL_KEY(0x49);
        send_hotkey(&[VK_LWIN], VK_I);
        return true;
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

    // Si la commande commence par explorer ou explorer.exe, la lancer avec son chemin complet sans console parasite
    if cur.to_lowercase().starts_with("explorer.exe") || cur.to_lowercase().starts_with("explorer ") {
        let _ = open_path_with_default_app(cur.strip_prefix("explorer.exe").or_else(|| cur.strip_prefix("explorer")).unwrap_or(cur).trim());
        return true;
    }

    // 3. Maintien impératif de la console ouverte via /K au lieu de /C
    // afin d'afficher la sortie sans fermer la fenêtre à la fin de l'exécution.
    process.raw_arg(format!("/K {cur}"));

    process.spawn().is_ok()
}

#[cfg(not(windows))]
pub fn run_system_command(command: &str) -> bool {
    let trimmed = command.trim();
    if trimmed.is_empty() {
        return false;
    }
    std::process::Command::new("sh")
        .arg("-c")
        .arg(trimmed)
        .spawn()
        .is_ok()
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
            restore_if_iconic(target_hwnd);
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
        restore_if_iconic(target_hwnd);
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

        set_text(text);
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
        let current_pid = std::process::id();

        if let Ok(focused) = uia.GetFocusedElement() {
            let native_hwnd = focused.CurrentNativeWindowHandle().ok().map(|h| HWND(h.0 as _));
            let is_overlay = native_hwnd.map_or(false, |h| {
                if h == overlay_hwnd {
                    return true;
                }
                let (focused_pid, _) = get_window_process_info(h);
                focused_pid == current_pid
            });

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

#[cfg(target_os = "macos")]
pub fn get_active_field_content() -> Option<String> {
    unsafe {
        let system_wide = AXUIElementCreateSystemWide();
        if system_wide.is_null() {
            return None;
        }
        let k_focused = CFStringCreateWithCString(std::ptr::null_mut(), b"AXFocusedUIElement\0".as_ptr() as _, 0x08000100);
        let mut focused_elem: *mut std::ffi::c_void = std::ptr::null_mut();
        let err = AXUIElementCopyAttributeValue(system_wide, k_focused, &mut focused_elem);
        CFRelease(k_focused);
        CFRelease(system_wide);

        if err == 0 && !focused_elem.is_null() {
            let k_val = CFStringCreateWithCString(std::ptr::null_mut(), b"AXValue\0".as_ptr() as _, 0x08000100);
            let mut val_ref: *mut std::ffi::c_void = std::ptr::null_mut();
            let mut result_text = None;
            if AXUIElementCopyAttributeValue(focused_elem, k_val, &mut val_ref) == 0 && !val_ref.is_null() {
                if CFGetTypeID(val_ref) == CFStringGetTypeID() {
                    let len = CFStringGetLength(val_ref);
                    let mut buf = vec![0u8; (len * 4 + 1) as usize];
                    if CFStringGetCString(val_ref, buf.as_mut_ptr() as _, buf.len() as isize, 0x08000100) {
                        let s = std::ffi::CStr::from_ptr(buf.as_ptr() as _).to_string_lossy().trim().to_string();
                        if !s.is_empty() {
                            result_text = Some(s);
                        }
                    }
                }
                CFRelease(val_ref);
            }
            CFRelease(k_val);
            CFRelease(focused_elem);
            return result_text;
        }
        None
    }
}

#[cfg(all(not(windows), not(target_os = "macos")))]
pub fn get_active_field_content() -> Option<String> {
    let elements = query_atspi_elements();
    elements.into_iter().find(|e| !e.value.is_empty()).map(|e| e.value)
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

    if is_forced || find_executable_in_path(first_token).is_some() {
        if run_system_command(cmd_str) {
            return Some(format!("Commande exécutée : {cmd_str}"));
        }
    }

    None
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

fn strip_diff_section(text: &str) -> String {
    let mut result = String::with_capacity(text.len());
    let mut in_diff_section = false;
    for line in text.lines() {
        let trimmed = line.trim();
        if trimmed.starts_with("=== INFORMATIONS WEB MODIFIÉES") || trimmed.starts_with("=== CHANGEMENTS RÉCENTS") {
            in_diff_section = true;
            continue;
        }
        if in_diff_section && (trimmed.starts_with("=== ") || trimmed.starts_with("--- Fenêtre")) {
            in_diff_section = false;
        }
        if !in_diff_section {
            result.push_str(line);
            result.push('\n');
        }
    }
    result
}

/// Extrait exclusivement le contenu textuel et informatif de la fenêtre cible
/// en éliminant tous les contrôles interactifs (boutons, combobox, onglets, coordonnées...).
#[cfg(windows)]
pub fn extract_window_text_only(hwnd: HWND) -> String {
    if hwnd.0.is_null() || is_window_hung(hwnd) {
        return String::new();
    }

    let (pid, _) = get_window_process_info(hwnd);
    if pid == std::process::id() {
        return String::new();
    }

    let title = get_window_title(hwnd);
    if title.to_lowercase().contains("libertide") {
        return String::new();
    }
    let mut lines = Vec::new();

    if !title.is_empty() {
        lines.push(format!("[Fenêtre active : \"{}\"]", title));
    }

    // 1. Si c'est un terminal, extraire directement la sortie console
    if is_terminal_window(hwnd, &title) {
        if let Some(term_text) = read_terminal_window_content(hwnd, &title) {
            let clean = term_text.trim();
            if !clean.is_empty() {
                lines.push(clean.to_string());
                return truncate_with_notice(&lines.join("\n\n"), 10000);
            }
        }
    }

    let _ = unsafe { CoInitializeEx(None, COINIT_MULTITHREADED) };
    let mut seen = std::collections::HashSet::new();

    if let Ok(uia) = unsafe { CoCreateInstance::<_, IUIAutomation>(&CUIAutomation, None, CLSCTX_INPROC_SERVER) } {
        if let Ok(win_elem) = unsafe { uia.ElementFromHandle(hwnd) } {
            // 2. Si c'est un navigateur, extraire en priorité le texte du document web
            if is_browser_hwnd(hwnd, &title) {
                if let Ok(cond_doc) = unsafe {
                    uia.CreatePropertyCondition(
                        windows::Win32::UI::Accessibility::UIA_ControlTypePropertyId,
                        &windows::core::VARIANT::from(UIA_DocumentControlTypeId.0),
                    )
                } {
                    if let Ok(doc_elements) = unsafe { win_elem.FindAll(TreeScope_Descendants, &cond_doc) } {
                        let doc_count = unsafe { doc_elements.Length().unwrap_or(0).min(6) };
                        let mut best_text = String::new();
                        for di in 0..doc_count {
                            if let Ok(doc_elem) = unsafe { doc_elements.GetElement(di) } {
                                if let Some(doc_text) = extract_web_document_text(&doc_elem, 15000) {
                                    if doc_text.len() > best_text.len() {
                                        best_text = doc_text;
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
                            for l in clean_text.lines() {
                                let trimmed = l.trim();
                                if trimmed.len() >= 2 && !seen.contains(trimmed) {
                                    seen.insert(trimmed.to_string());
                                    lines.push(trimmed.to_string());
                                }
                            }
                        }
                    }
                }
            }

            // 3. Parcourir les nœuds textuels descendants (text, document, edit, labels)
            if lines.len() <= 2 {
                if let Ok(cond) = unsafe { uia.ControlViewCondition() } {
                    if let Ok(elements) = unsafe { win_elem.FindAll(TreeScope_Descendants, &cond) } {
                        let count = unsafe { elements.Length().unwrap_or(0).min(120) };
                        for i in 0..count {
                            if let Ok(elem) = unsafe { elements.GetElement(i) } {
                                let ctype = unsafe { elem.CurrentControlType().unwrap_or_default() };
                                let is_text_type = ctype == UIA_DocumentControlTypeId
                                    || ctype == UIA_EditControlTypeId
                                    || ctype.0 == 50020; // UIA_TextControlTypeId

                                let class_name = unsafe { elem.CurrentClassName().map(|b| b.to_string().to_lowercase()).unwrap_or_default() };
                                let is_static_or_label = class_name.contains("static") || class_name.contains("text") || class_name.contains("label");

                                if is_text_type || is_static_or_label {
                                    if let Some(txt) = extract_inner_text(&uia, &elem, 800) {
                                        let clean = txt.trim();
                                        if clean.len() >= 3 && !seen.contains(clean) && !clean.starts_with("http") {
                                            seen.insert(clean.to_string());
                                            lines.push(clean.to_string());
                                        }
                                    }
                                }
                            }
                        }
                    }
                }
            }
        }
    }

    // 4. Repli OCR si très peu de texte natif extrait (Canvas, Java Swing, app legacy)
    if lines.len() <= 2 {
        let mut r = RECT::default();
        if unsafe { GetWindowRect(hwnd, &mut r).is_ok() } {
            let zone_rect = [r.left, r.top, r.right, r.bottom];
            let ocr_text = extract_text_from_zone(hwnd, zone_rect, &[]);
            let clean_ocr = ocr_text.trim();
            if !clean_ocr.is_empty() {
                lines.push(clean_ocr.to_string());
            }
        }
    }

    truncate_with_notice(&lines.join("\n"), 10000)
}

#[cfg(not(windows))]
pub fn extract_window_text_only(_target: impl AsWindowTarget) -> String {
    summarize_screen_state(None, false)
}
pub use crate::commands::execute_system_actions;