use std::sync::mpsc::{Receiver, Sender};
use std::sync::Mutex;
use std::sync::atomic::{AtomicBool, AtomicU64, AtomicUsize, Ordering};
use std::time::{Duration, Instant};

pub use crate::scheduler::*;
pub use crate::autostart::*;

use crate::fs_util::{
    create_temp_note_file, find_executable_in_path, find_file_in_workspace,
    open_path_with_default_app,
};
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
    CUIAutomation, IUIAutomation, IUIAutomationElement, IUIAutomationScrollPattern, IUIAutomationSelectionItemPattern,
    IUIAutomationTextPattern,
    IUIAutomationValuePattern, TreeScope_Descendants, UIA_DocumentControlTypeId,
    UIA_EditControlTypeId, UIA_ExpandCollapsePatternId, UIA_GroupControlTypeId,
    UIA_ScrollPatternId,
    UIA_TextPatternId,
    UIA_ValuePatternId, UIA_SeparatorControlTypeId,
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
    FindWindowW, GetForegroundWindow, GetWindowRect, GetWindowTextW, GetWindowThreadProcessId, IsIconic,
    GetGUIThreadInfo, GUITHREADINFO,
    PostMessageW, SendMessageW, SetForegroundWindow, ShowWindow,
    SW_RESTORE, WM_CLOSE,
};

#[cfg(windows)]
const CREATE_NEW_CONSOLE: u32 = 0x00000010;
#[cfg(windows)]
const CREATE_NO_WINDOW: u32 = 0x08000000;

#[cfg(not(windows))]
#[derive(Clone, Default)]
pub struct UiaElementInfo;

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

        unsafe {
            if IsIconic(hwnd).as_bool() {
                continue;
            }
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

        let click_x = r.left + (width / 2);
        let click_y = (r.top + 30).min(r.bottom - 10);

        candidates.push(SwitchWindowTarget {
            id: hwnd.0 as isize,
            title: t_clean.to_string(),
            click_x,
            click_y,
            rect: [r.left, r.top, r.right, r.bottom],
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
                let hash = win_id.as_bytes().iter().fold(0isize, |acc, &b| acc.wrapping_mul(31).wrapping_add(b as isize));
                candidates.push(SwitchWindowTarget {
                    id: hash.abs().max(1),
                    title: t_clean.to_string(),
                    click_x: x + (w / 2),
                    click_y: (y + 30).min(y + h - 10),
                    rect: [x, y, x + w, y + h],
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
    let (_pid, exe_name) = get_window_process_info(hwnd);
    let mut win_title_buf = [0u16; 512];
    let t_len = unsafe { GetWindowTextW(hwnd, &mut win_title_buf) };
    let win_title = if t_len > 0 {
        String::from_utf16_lossy(&win_title_buf[..t_len as usize])
    } else {
        String::new()
    };

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

                let _ = result_tx.send(TargetsScanResult {
                    hwnd: req.hwnd,
                    win_rect: req.win_rect,
                    targets,
                    sequence: req.sequence,
                });
            }
        })
        .expect("Échec du lancement du worker de ciblage des numéros");
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
struct MacosAxElementRaw {
    name: String,
    role: String,
    x: i32,
    y: i32,
    width: i32,
    height: i32,
    #[serde(default)]
    value: String,
}

#[cfg(target_os = "macos")]
fn query_macos_ax_elements() -> Vec<MacosAxElementRaw> {
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
    items
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
        .collect()
}

#[cfg(all(not(windows), not(target_os = "macos")))]
#[derive(serde::Deserialize, Debug, Clone)]
struct AtspiElementRaw {
    name: String,
    role: String,
    x: i32,
    y: i32,
    width: i32,
    height: i32,
    #[serde(default)]
    value: String,
}

#[cfg(all(not(windows), not(target_os = "macos")))]
fn query_atspi_elements() -> Vec<AtspiElementRaw> {
    // Script Python embarqué utilisant le binding standard Atspi (GObject introspection)
    // présent nativement sur la quasi-totalité des distributions Linux avec environnement de bureau.
    const ATSPI_SCRIPT: &str = r#"
import json, sys
try:
    import gi
    gi.require_version('Atspi', '2.0')
    from gi.repository import Atspi
except Exception:
    sys.exit(0)

elements = []
desktop = Atspi.get_desktop(0)
if not desktop:
    sys.exit(0)

def walk(accessible, depth=0):
    if depth > 7 or len(elements) >= 120:
        return
    try:
        role = accessible.get_role_name()
        state = accessible.get_state_set()
        is_showing = state.contains(Atspi.StateType.SHOWING) if state else True
        if is_showing and role in ("push-button", "link", "entry", "password-text", "page-tab", "check-box", "radio-button", "combo-box", "label", "text"):
            comp = accessible.get_component_iface()
            if comp:
                bbox = comp.get_extents(Atspi.CoordType.SCREEN)
                name = accessible.get_name() or ""
                val_text = ""
                text_iface = accessible.get_text_iface()
                if text_iface:
                    val_text = text_iface.get_text(0, -1) or ""
                if bbox.width > 5 and bbox.height > 5:
                    elements.append({
                        "name": name.strip(),
                        "role": role,
                        "x": bbox.x,
                        "y": bbox.y,
                        "width": bbox.width,
                        "height": bbox.height,
                        "value": val_text.strip()
                    })
        for i in range(accessible.get_child_count()):
            child = accessible.get_child_at_index(i)
            if child:
                walk(child, depth + 1)
    except Exception:
        pass

walk(desktop)
print(json.dumps(elements))
"#;

    let Ok(output) = std::process::Command::new("python3")
        .args(["-c", ATSPI_SCRIPT])
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

#[cfg(all(not(windows), not(target_os = "macos")))]
pub fn collect_numbered_targets_for_active_window() -> Vec<NumberedElementTarget> {
    let items = query_atspi_elements();
    items
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
        .collect()
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

    let base_speech = if clean_role.eq_ignore_ascii_case("lecture textuelle") {
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

pub static SWITCH_HOOK_ENABLED: AtomicBool = AtomicBool::new(false);
pub static SWITCH_KEY_QUEUE: Mutex<Vec<crate::types::SwitchKeyEvent>> = Mutex::new(Vec::new());
pub static NUMBER_KEY_QUEUE: Mutex<Vec<char>> = Mutex::new(Vec::new());

#[cfg(windows)]
pub mod accessibility_keys {
    use windows::Win32::UI::Input::KeyboardAndMouse::VIRTUAL_KEY;

    pub const VK_SWITCH_PRIMARY: VIRTUAL_KEY = VIRTUAL_KEY(0x20);   // Espace (Contacteur 1 standard / validation)
    pub const VK_SWITCH_ENTER: VIRTUAL_KEY = VIRTUAL_KEY(0x0D);     // Entrée (Contacteur 2 standard / sélection)
    pub const VK_SWITCH_F7: VIRTUAL_KEY = VIRTUAL_KEY(0x76);        // F7 (Contacteur profil comm) -> Toggle Survol / Dwell
    pub const VK_SWITCH_F8: VIRTUAL_KEY = VIRTUAL_KEY(0x77);        // F8 (Contacteur profil comm) -> Toggle Dictée
    pub const VK_SWITCH_F9: VIRTUAL_KEY = VIRTUAL_KEY(0x78);        // F9 -> Toggle Voix / TTS
    pub const VK_SWITCH_F4: VIRTUAL_KEY = VIRTUAL_KEY(0x73);        // F4 -> Effacer / Trash
    pub const VK_PAUSE_BREAK: VIRTUAL_KEY = VIRTUAL_KEY(0x13);      // Touche Pause -> Toggle Dictée
    pub const VK_XBUTTON1: VIRTUAL_KEY = VIRTUAL_KEY(0x05);         // Trackball bouton 4 (Souris 4)
    pub const VK_XBUTTON2: VIRTUAL_KEY = VIRTUAL_KEY(0x06);         // Trackball bouton 5 (Souris 5)
    pub const VK_MBUTTON: VIRTUAL_KEY = VIRTUAL_KEY(0x04);          // Trackball bouton molette
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
static KEYBOARD_HOOK_HANDLE: Mutex<Option<isize>> = Mutex::new(None);

#[cfg(windows)]
unsafe extern "system" fn switch_keyboard_hook_proc(
    code: i32,
    wparam: WPARAM,
    lparam: LPARAM,
) -> windows::Win32::Foundation::LRESULT {
    use windows::Win32::UI::WindowsAndMessaging::{
        CallNextHookEx, GetForegroundWindow, GetWindowThreadProcessId, KBDLLHOOKSTRUCT, WM_KEYDOWN,
        WM_KEYUP, WM_SYSKEYDOWN, WM_SYSKEYUP,
    };
    use crate::types::SwitchKeyEvent;

    if code >= 0 {
        let fg = GetForegroundWindow();
        let mut fg_pid = 0u32;
        let _ = GetWindowThreadProcessId(fg, Some(&mut fg_pid));
        let is_libertide_fg = fg_pid == std::process::id();
        let kbd = *(lparam.0 as *const KBDLLHOOKSTRUCT);

        // Si l'événement a été injecté par un logiciel tiers (ex: PowerToys, AutoHotkey),
        // ne pas le bloquer pour lui permettre d'atteindre la fenêtre cible.
        const LLKHF_INJECTED: u32 = 0x00000010;
        let is_injected = (kbd.flags.0 & LLKHF_INJECTED) != 0;
        if is_injected {
            return CallNextHookEx(None, code, wparam, lparam);
        }

        if !is_libertide_fg {
        let vk = kbd.vkCode;
        let is_down = wparam.0 == WM_KEYDOWN as usize || wparam.0 == WM_SYSKEYDOWN as usize;
        let is_up = wparam.0 == WM_KEYUP as usize || wparam.0 == WM_SYSKEYUP as usize;

        let is_shift = (windows::Win32::UI::Input::KeyboardAndMouse::GetAsyncKeyState(0x10) as u16 & 0x8000) != 0;
        let is_ctrl = (windows::Win32::UI::Input::KeyboardAndMouse::GetAsyncKeyState(0x11) as u16 & 0x8000) != 0;
        let is_alt = (windows::Win32::UI::Input::KeyboardAndMouse::GetAsyncKeyState(0x12) as u16 & 0x8000) != 0;
        let is_switch_hook_on = SWITCH_HOOK_ENABLED.load(Ordering::Relaxed);

        // Interception globale du pavé numérique (0x60..=0x69) et des touches chiffres (0x30..=0x39) en mode survol
        if is_switch_hook_on && !is_ctrl && !is_alt {
            let digit_char = match vk {
                0x60..=0x69 => Some((b'0' + (vk as u8 - 0x60)) as char),
                0x30..=0x39 => Some((b'0' + (vk as u8 - 0x30)) as char),
                _ => None,
            };

            if let Some(digit) = digit_char {
                let vk_idx = (vk as usize) & 0xFF;
                if is_down {
                    let mut already_down = false;
                    if let Ok(mut keys) = HOOK_KEYS_DOWN.lock() {
                        already_down = keys[vk_idx];
                        keys[vk_idx] = true;
                    }
                    if !already_down {
                        if let Ok(mut nq) = NUMBER_KEY_QUEUE.lock() {
                            nq.push(digit);
                        }
                    }
                } else if is_up {
                    if let Ok(mut keys) = HOOK_KEYS_DOWN.lock() {
                        keys[vk_idx] = false;
                    }
                }
                if is_down || is_up {
                    return windows::Win32::Foundation::LRESULT(1);
                }
            }
        }

        let switch_event = match vk {
            0x76 => Some(SwitchKeyEvent::ToggleSwitch),             // F7 -> Toggle Survol
            0x77 | 0x13 => Some(SwitchKeyEvent::ToggleDictation),  // F8 ou Pause -> Toggle Dictée
            0x78 => Some(SwitchKeyEvent::ToggleVoice),              // F9 -> Toggle Voix
            0x73 => Some(SwitchKeyEvent::Clear),                    // F4 -> Effacer / Trash
            0x09 if is_switch_hook_on => Some(if is_shift { SwitchKeyEvent::Prev } else { SwitchKeyEvent::Next }), // Tab / Shift+Tab
            // Précédent : Flèche Gauche (0x25), Flèche Haut (0x26)
            0x25 | 0x26 if is_switch_hook_on => Some(SwitchKeyEvent::Prev),
            // Suivant : Flèche Droite (0x27), Flèche Bas (0x28)
            0x27 | 0x28 if is_switch_hook_on => Some(SwitchKeyEvent::Next),
            // Sélection : Entrée, Espace
            0x0D | 0x20 if is_switch_hook_on => Some(SwitchKeyEvent::Select),
            // Retour / Sortie : Backspace (Retour arrière)
            0x08 if is_switch_hook_on => Some(SwitchKeyEvent::Back),
            0x1B if is_switch_hook_on => Some(SwitchKeyEvent::Escape),
            _ => None,
        };

        if let Some(event) = switch_event {
                let vk_idx = (vk as usize) & 0xFF;
            if is_down {
                    let mut already_down = false;
                    if let Ok(mut keys) = HOOK_KEYS_DOWN.lock() {
                        already_down = keys[vk_idx];
                        keys[vk_idx] = true;
                    }
                    if !already_down {
                        if let Ok(mut times) = HOOK_KEYS_DOWN_TIME.lock() {
                            times[vk_idx] = Some(Instant::now());
                        }
                        // Pour les touches directes (Next, Prev, Back, Escape, Fonctions)
                        if event != SwitchKeyEvent::Select {
                            if let Ok(mut q) = SWITCH_KEY_QUEUE.lock() {
                                q.push(event);
                            }
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

                    // Détection appui court vs appui long sur le contacteur de validation
                    if event == SwitchKeyEvent::Select {
                        let is_long_press = press_duration.map_or(false, |d| d >= Duration::from_millis(1100));
                        if let Ok(mut q) = SWITCH_KEY_QUEUE.lock() {
                            if is_long_press {
                                // Appui long = Retour / Sortie (indispensable pour 1 contacteur unique !)
                                q.push(SwitchKeyEvent::Back);
                            } else {
                                // Appui court = Validation / Sélection
                                q.push(SwitchKeyEvent::Select);
                            }
                        }
                }
            }
            // Bloque et avale l'événement clavier : il n'atteindra jamais la fenêtre cible
            if is_down || is_up {
                return windows::Win32::Foundation::LRESULT(1);
            }
        }
        }
    }

    CallNextHookEx(None, code, wparam, lparam)
}

#[cfg(windows)]
pub fn install_switch_keyboard_hook() {
    use windows::Win32::UI::WindowsAndMessaging::{SetWindowsHookExW, WH_KEYBOARD_LL};
    let mut guard = KEYBOARD_HOOK_HANDLE.lock().unwrap();
    if guard.is_none() {
        unsafe {
            if let Ok(hook) = SetWindowsHookExW(
                WH_KEYBOARD_LL,
                Some(switch_keyboard_hook_proc),
                windows::Win32::Foundation::HINSTANCE(std::ptr::null_mut()),
                0,
            ) {
                *guard = Some(hook.0 as isize);
            }
        }
    }
}

#[cfg(windows)]
pub fn uninstall_switch_keyboard_hook() {
    use windows::Win32::UI::WindowsAndMessaging::{UnhookWindowsHookEx, HHOOK};
    let mut guard = KEYBOARD_HOOK_HANDLE.lock().unwrap();
    if let Some(raw) = guard.take() {
        unsafe {
            let _ = UnhookWindowsHookEx(HHOOK(raw as _));
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
    const SCRIPT: &str = r#"tell application "System Events"
        try
            set frontApp to first application process whose frontmost is true
            set focusedElem to value of attribute "AXFocusedUIElement" of frontApp
            if focusedElem is not missing value then
                set p to position of focusedElem
                set s to size of focusedElem
                return (item 1 of p as text) & "," & (item 2 of p as text) & "," & ((item 1 of p) + (item 1 of s) as text) & "," & ((item 2 of p) + (item 2 of s) as text)
            end if
        end try
    end tell"#;

    if let Ok(output) = std::process::Command::new("osascript").args(["-e", SCRIPT]).output() {
        let out_str = String::from_utf8_lossy(&output.stdout).trim().to_string();
        let parts: Vec<i32> = out_str.split(',').filter_map(|s| s.parse().ok()).collect();
        if parts.len() == 4 {
            if let Ok(mut rlock) = LAST_FOCUSED_RECT.lock() {
                *rlock = Some([parts[0], parts[1], parts[2], parts[3]]);
            }
        }
    }
}

#[cfg(all(not(windows), not(target_os = "macos")))]
pub fn update_last_external_focus() {
    // Intégration via AT-SPI pour récupérer le composant ayant State.FOCUSED
    // et mettre à jour LAST_FOCUSED_RECT avec bbox.x, bbox.y, bbox.x + bbox.width, bbox.y + bbox.height
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

    pub fn get_plain_text() -> Option<String> {
        get_text()
    }

    pub fn get_advanced_text() -> Option<String> {
        if let Some(text) = get_text() {
            let lines: Vec<&str> = text.lines().collect();
            // Heuristic: if all lines are valid, absolute paths, treat as file list.
            if !lines.is_empty() && lines.iter().all(|l| l.starts_with('/')) {
                let mut spans = Vec::new();
                for line in lines {
                    let path = line.trim();
                    if path.is_empty() { continue; }
                    let p = std::path::Path::new(path);
                    if p.exists() {
                        let fname = p.file_name().and_then(|n| n.to_str()).unwrap_or(path);
                        let ext = p.extension().and_then(|e| e.to_str()).unwrap_or("").to_lowercase();
                        let is_img = matches!(ext.as_str(), "png" | "jpg" | "jpeg" | "gif" | "webp" | "svg");
                        let tag = if is_img {
                            format!("[link=\"action:Ouvrir l'image : {path}\"]🖼️ {fname}[/link]")
                        } else {
                            format!("[link=\"action:Ouvrir le fichier : {path}\"]📄 {fname}[/link]")
                        };
                        spans.push(tag);
                    }
                }
                if !spans.is_empty() {
                    return Some(spans.join(" "));
                }
            }
            return Some(text);
        }
        None
    }
}

#[cfg(all(not(windows), not(target_os = "macos")))]
mod clipboard {
    use std::io::Write;
    use super::html_to_advanced_markdown;

    fn copy_via_wl(text: &str, mime: &str) -> bool {
        let mut child = match std::process::Command::new("wl-copy")
            .args(["-t", mime])
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

    fn copy_via_xclip(text: &str, mime: &str) -> bool {
        let mut child = match std::process::Command::new("xclip")
            .args(["-selection", "clipboard", "-t", mime])
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

    pub fn set_text(text: &str) -> bool {
        let html_payload = super::convert_markdown_and_links_to_html(text);
        let has_links = text.contains("[link") || text.contains("](");
        let is_wayland = super::is_wayland_session();

        let ok = if is_wayland {
            copy_via_wl(text, "text/plain;charset=utf-8") || copy_via_xclip(text, "text/plain;charset=utf-8")
        } else {
            copy_via_xclip(text, "text/plain;charset=utf-8") || copy_via_wl(text, "text/plain;charset=utf-8")
        };

        if has_links {
            if is_wayland {
                if !copy_via_wl(&html_payload, "text/html") {
                    let _ = copy_via_xclip(&html_payload, "text/html");
                }
            } else if !copy_via_xclip(&html_payload, "text/html") {
                let _ = copy_via_wl(&html_payload, "text/html");
            }
        }
        ok
    }

    fn paste_format(mime: &str) -> Option<String> {
        let try_wl = || -> Option<String> {
            let output = std::process::Command::new("wl-paste")
                .args(["-t", mime])
                .output()
                .ok()?;
            if output.status.success() {
                let s = String::from_utf8_lossy(&output.stdout).to_string();
                if !s.is_empty() { return Some(s); }
            }
            None
        };

        let try_xclip = || -> Option<String> {
            let output = std::process::Command::new("xclip")
                .args(["-selection", "clipboard", "-t", mime, "-o"])
                .output()
                .ok()?;
            if output.status.success() {
                let s = String::from_utf8_lossy(&output.stdout).to_string();
                if !s.is_empty() { return Some(s); }
            }
            None
        };

        if super::is_wayland_session() {
            try_wl().or_else(try_xclip)
        } else {
            try_xclip().or_else(try_wl)
        }
    }

    pub fn get_advanced_text() -> Option<String> {
        // 1. Détection des fichiers (text/uri-list)
        if let Some(uris) = paste_format("text/uri-list") {
            let mut spans = Vec::new();
            for line in uris.lines() {
                let trimmed = line.trim();
                if let Some(path) = trimmed.strip_prefix("file://") {
                    let p = std::path::Path::new(path);
                    let fname = p.file_name().and_then(|n| n.to_str()).unwrap_or(path);
                    let ext = p.extension().and_then(|e| e.to_str()).unwrap_or("").to_lowercase();
                    let is_img = matches!(ext.as_str(), "png" | "jpg" | "jpeg" | "gif" | "webp" | "svg");
                    let tag = if is_img {
                        format!("[link=\"action:Ouvrir l'image : {path}\"]🖼️ {fname}[/link]")
                    } else {
                        format!("[link=\"action:Ouvrir le fichier : {path}\"]📄 {fname}[/link]")
                    };
                    spans.push(tag);
                }
            }
            if !spans.is_empty() {
                return Some(spans.join(" "));
            }
        }

        // 2. Détection du format HTML avec liens
        if let Some(html) = paste_format("text/html") {
            let md = html_to_advanced_markdown(&html);
            if !md.trim().is_empty() {
                return Some(md);
            }
        }

        get_plain_text()
    }

    pub fn get_text() -> Option<String> {
        get_plain_text()
    }

    pub fn get_plain_text() -> Option<String> {
        let try_wl = || -> Option<String> {
            let output = std::process::Command::new("wl-paste")
                .args(["-t", "text/plain;charset=utf-8"])
                .output()
                .or_else(|_| std::process::Command::new("wl-paste").arg("--no-newline").output())
                .ok()?;
            if output.status.success() {
                let s = String::from_utf8_lossy(&output.stdout).to_string();
                if !s.is_empty() { return Some(s); }
            }
            None
        };

        let try_xclip = || -> Option<String> {
            let output = std::process::Command::new("xclip")
                .args(["-selection", "clipboard", "-o"])
                .output()
                .ok()?;
            if output.status.success() {
                let s = String::from_utf8_lossy(&output.stdout).to_string();
                if !s.is_empty() { return Some(s); }
            }
            None
        };

        if super::is_wayland_session() {
            try_wl().or_else(try_xclip)
        } else {
            try_xclip().or_else(try_wl)
        }
    }
}

pub fn get_clipboard_text() -> Option<String> {
    clipboard::get_advanced_text().or_else(clipboard::get_plain_text)
}

pub fn get_plain_clipboard_text() -> Option<String> {
    clipboard::get_plain_text()
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
        fn GlobalSize(hmem: *mut c_void) -> usize;
        fn GlobalLock(hmem: *mut c_void) -> *mut c_void;
        fn GlobalUnlock(hmem: *mut c_void) -> i32;
        fn RegisterClipboardFormatW(lpszformat: *const u16) -> u32;
    }

    #[link(name = "shell32")]
    extern "system" {
        fn DragQueryFileW(hdrop: *mut c_void, ifile: u32, lpfile: *mut u16, cch: u32) -> u32;
    }

    const CF_UNICODETEXT: u32 = 13;
    const CF_HDROP: u32 = 15;
    const GMEM_MOVEABLE: u32 = 0x0002;

    fn cf_html_format_id() -> u32 {
        let format_name: Vec<u16> = "HTML Format\0".encode_utf16().collect();
        unsafe { RegisterClipboardFormatW(format_name.as_ptr()) }
    }

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

            // Si le texte contient des liens, générer également le format MIME HTML (CF_HTML)
            if text.contains("[link") || text.contains("](") {
                let cf_html = cf_html_format_id();
                if cf_html != 0 {
                    let html_body = super::convert_markdown_and_links_to_html(text);
                    let cf_html_payload = build_cf_html_buffer(&html_body);
                    let h_mem_html = GlobalAlloc(GMEM_MOVEABLE, cf_html_payload.len());
                    if !h_mem_html.is_null() {
                        let p_html = GlobalLock(h_mem_html) as *mut u8;
                        if !p_html.is_null() {
                            std::ptr::copy_nonoverlapping(cf_html_payload.as_ptr(), p_html, cf_html_payload.len());
                            GlobalUnlock(h_mem_html);
                            SetClipboardData(cf_html, h_mem_html);
                        }
                    }
                }
            }
            CloseClipboard();
        }
        true
    }

    fn build_cf_html_buffer(fragment: &str) -> Vec<u8> {
        let start_html = 105;
        let start_fragment = 139;
        let end_fragment = start_fragment + fragment.as_bytes().len();
        let footer = "<!--EndFragment-->\r\n</body></html>\0";
        let total_len = end_fragment + footer.len() - 1;

        let header = format!(
            "Version:0.9\r\nStartHTML:{:010}\r\nEndHTML:{:010}\r\nStartFragment:{:010}\r\nEndFragment:{:010}\r\n<html><body>\r\n<!--StartFragment-->",
            start_html,
            total_len,
            start_fragment,
            end_fragment
        );

        let mut buf = Vec::with_capacity(header.len() + fragment.len() + footer.len());
        buf.extend_from_slice(header.as_bytes());
        buf.extend_from_slice(fragment.as_bytes());
        buf.extend_from_slice(footer.as_bytes());
        buf
    }

    pub fn get_advanced_text() -> Option<String> {
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

            // 1. Détection de fichiers copiés depuis l'Explorateur Windows (CF_HDROP)
            if IsClipboardFormatAvailable(CF_HDROP) != 0 {
                let h_drop = GetClipboardData(CF_HDROP);
                if !h_drop.is_null() {
                    let count = DragQueryFileW(h_drop, 0xFFFFFFFF, std::ptr::null_mut(), 0);
                    if count > 0 {
                        let mut file_spans = Vec::new();
                        for i in 0..count {
                            let len = DragQueryFileW(h_drop, i, std::ptr::null_mut(), 0);
                            if len > 0 {
                                let mut buf = vec![0u16; (len + 1) as usize];
                                let copied = DragQueryFileW(h_drop, i, buf.as_mut_ptr(), len + 1);
                                if copied > 0 {
                                    let path = String::from_utf16_lossy(&buf[..copied as usize]);
                                    let p = std::path::Path::new(&path);
                                    let fname = p.file_name().and_then(|n| n.to_str()).unwrap_or(&path);
                                    let ext = p.extension().and_then(|e| e.to_str()).unwrap_or("").to_lowercase();

                                    let is_img = matches!(ext.as_str(), "png" | "jpg" | "jpeg" | "gif" | "webp" | "bmp" | "svg");
                                    let is_audio = matches!(ext.as_str(), "mp3" | "wav" | "ogg" | "flac" | "m4a" | "aac");
                                    let is_video = matches!(ext.as_str(), "mp4" | "mkv" | "avi" | "mov" | "webm" | "wmv");

                                    let tag = if is_img {
                                        format!("[link=\"action:Ouvrir l'image : {path}\"]🖼️ {fname}[/link]")
                                    } else if is_audio {
                                        format!("[link=\"action:Lancer l'audio : {path}\"]🎵 {fname}[/link]")
                                    } else if is_video {
                                        format!("[link=\"action:Lancer la vidéo : {path}\"]▶ {fname}[/link]")
                                    } else {
                                        format!("[link=\"action:Ouvrir le fichier : {path}\"]📄 {fname}[/link]")
                                    };
                                    file_spans.push(tag);
                                }
                            }
                        }
                        let _ = CloseClipboard();
                        if !file_spans.is_empty() {
                            return Some(file_spans.join(" "));
                        }
                        return None;
                    }
                }
            }

            // 2. Détection du format HTML enrichi avec liens (CF_HTML)
            let cf_html = cf_html_format_id();
            if cf_html != 0 && IsClipboardFormatAvailable(cf_html) != 0 {
                let h_mem = GetClipboardData(cf_html);
                if !h_mem.is_null() {
                    let p_data = GlobalLock(h_mem) as *const u8;
                    if !p_data.is_null() {
                        let size = GlobalSize(h_mem);
                        let mut len = 0;
                        while len < size && *p_data.add(len) != 0 {
                            len += 1;
                        }
                        let slice = std::slice::from_raw_parts(p_data, len);
                        let raw_html = String::from_utf8_lossy(slice);
                        GlobalUnlock(h_mem);

                        let md = super::parse_cf_html_to_markdown(&raw_html);
                        if !md.trim().is_empty() {
                            let _ = CloseClipboard();
                            return Some(md);
                        }
                    }
                }
            }

            // 3. Repli standard sur CF_UNICODETEXT
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

    pub fn get_plain_text() -> Option<String> {
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

    pub fn get_text() -> Option<String> {
        get_plain_text()
    }
}

pub fn html_to_advanced_markdown(html: &str) -> String {
    let mut result = String::with_capacity(html.len());
    let mut i = 0;
    let bytes = html.as_bytes();
    let len = bytes.len();

    let mut current_link_href: Option<String> = None;
    let mut current_link_text = String::new();
    let mut has_links = false;

    while i < len {
        if bytes[i] == b'<' {
            if let Some(tag_end) = html[i..].find('>') {
                let tag_content = html[i + 1..i + tag_end].trim();
                let is_closing = tag_content.starts_with('/');
                let tag_body = if is_closing { tag_content[1..].trim() } else { tag_content };
                let tag_name = tag_body.split_whitespace().next().unwrap_or("").to_lowercase();
                let tag_name = tag_name.trim_end_matches('/');

                if tag_name == "a" {
                    if is_closing {
                        if let Some(href) = current_link_href.take() {
                            let label = current_link_text.trim();
                            if !label.is_empty() {
                                result.push_str(&format!("[link=\"{href}\"]{label}[/link]"));
                                has_links = true;
                            } else if !href.is_empty() {
                                result.push_str(&format!("[link=\"{href}\"]{href}[/link]"));
                                has_links = true;
                            }
                            current_link_text.clear();
                        }
                    } else if let Some(href) = crate::html_util::extract_attribute(tag_body, "href") {
                        let clean_href = crate::html_util::decode_html_entities(href.trim());
                        current_link_href = Some(clean_href);
                        current_link_text.clear();
                    }
                } else if tag_name == "br" || tag_name == "p" || tag_name == "div" || tag_name == "tr" || tag_name == "li" {
                    result.push(' ');
                } else if tag_name == "b" || tag_name == "strong" {
                    result.push_str("**");
                } else if tag_name == "code" {
                    result.push('`');
                }

                i += tag_end + 1;
                continue;
            }
        }

        if let Some(ch) = html[i..].chars().next() {
            if current_link_href.is_some() {
                current_link_text.push(ch);
            } else {
                result.push(ch);
            }
            i += ch.len_utf8();
        } else {
            break;
        }
    }

    if let Some(href) = current_link_href.take() {
        let label = current_link_text.trim();
        if !label.is_empty() {
            result.push_str(&format!("[link=\"{href}\"]{label}[/link]"));
            has_links = true;
        }
    }

    if has_links {
        let decoded = crate::html_util::decode_html_entities(&result);
        decoded.split_whitespace().collect::<Vec<_>>().join(" ")
    } else {
        String::new()
    }
}

pub fn parse_cf_html_to_markdown(raw_html: &str) -> String {
    let fragment = if let (Some(s), Some(e)) = (raw_html.find("<!--StartFragment-->"), raw_html.find("<!--EndFragment-->")) {
        if s < e {
            &raw_html[s + "<!--StartFragment-->".len()..e]
        } else {
            raw_html
        }
    } else {
        raw_html
    };

    html_to_advanced_markdown(fragment)
}

pub fn convert_markdown_and_links_to_html(text: &str) -> String {
    let mut out = String::with_capacity(text.len() * 2);
    out.push_str("<div style=\"font-family:sans-serif;\">\r\n");
    for line in text.lines() {
        let clean = crate::floating_window::render_markdown_line_to_html(line);
        out.push_str(&clean);
        out.push_str("<br/>\r\n");
    }
    out.push_str("</div>");
    out
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

    let old_clip = clipboard::get_plain_text();
    unsafe {
        SendMessageW(hwnd, WM_SYSCOMMAND, WPARAM(IDM_COPYALL), LPARAM(0));
    }
    std::thread::sleep(Duration::from_millis(60));

    let new_clip = clipboard::get_plain_text();
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
        let mut target_pid = 0u32;
        let target_thread_id = GetWindowThreadProcessId(hwnd, Some(&mut target_pid));
        let fg_thread_id = if !fg_hwnd.0.is_null() {
            GetWindowThreadProcessId(fg_hwnd, None)
        } else {
            0
        };

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
        let _ = std::process::Command::new("osascript")
            .args(["-e", "tell application \"System Events\" to keystroke \"v\" using command down"])
            .status();
    }
    #[cfg(all(not(windows), not(target_os = "macos")))]
    {
        const VK_CONTROL: VIRTUAL_KEY = VIRTUAL_KEY(0x11);
        const VK_V: VIRTUAL_KEY = VIRTUAL_KEY(0x56);
        send_hotkey(&[VK_CONTROL], VK_V);
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

#[cfg(not(windows))]
pub fn send_hotkey(modifiers: &[VIRTUAL_KEY], key: VIRTUAL_KEY) {
    let mut mod_strs = Vec::new();
    for m in modifiers {
        match m.0 {
            0x11 => mod_strs.push("ctrl"),
            0x10 => mod_strs.push("shift"),
            0x12 => mod_strs.push("alt"),
            0x5B | 0x5C => mod_strs.push("super"),
            _ => {}
        }
    }
    let key_str = match key.0 {
        0x0D => "Return",
        0x1B => "Escape",
        0x09 => "Tab",
        0x20 => "space",
        0x08 => "BackSpace",
        0x25 => "Left",
        0x26 => "Up",
        0x27 => "Right",
        0x28 => "Down",
        0x21 => "Page_Up",
        0x22 => "Page_Down",
        0x24 => "Home",
        0x23 => "End",
        0x54 => "t",
        0x41 => "a",
        0x43 => "c",
        0x56 => "v",
        0x57 => "w",
        0x53 => "s",
        0x5A => "z",
        0x4C => "l",
        0x44 => "d",
        0x74 => "F5",
        c if (0x41..=0x5A).contains(&c) => {
            match (b'a' + (c as u8 - 0x41)) as char {
                'a' => "a", 'b' => "b", 'c' => "c", 'd' => "d", 'e' => "e", 'f' => "f",
                'g' => "g", 'h' => "h", 'i' => "i", 'j' => "j", 'k' => "k", 'l' => "l",
                'm' => "m", 'n' => "n", 'o' => "o", 'p' => "p", 'q' => "q", 'r' => "r",
                's' => "s", 't' => "t", 'u' => "u", 'v' => "v", 'w' => "w", 'x' => "x",
                'y' => "y", 'z' => "z", _ => "",
            }
        }
        _ => "",
    };

    if key_str.is_empty() && mod_strs.is_empty() {
        return;
    }

    #[cfg(target_os = "macos")]
    {
        let using_clause = if !mod_strs.is_empty() {
            let mac_mods: Vec<&str> = mod_strs.iter().map(|&m| match m {
                "ctrl" => "command down",
                "shift" => "shift down",
                "alt" => "option down",
                "super" => "control down",
                _ => "",
            }).filter(|s| !s.is_empty()).collect();
            if !mac_mods.is_empty() {
                format!(" using {{{}}}", mac_mods.join(", "))
            } else {
                String::new()
            }
        } else {
            String::new()
        };

        let script = if key_str == "Return" {
            format!("tell application \"System Events\" to key code 36{using_clause}")
        } else if key_str == "Tab" {
            format!("tell application \"System Events\" to key code 48{using_clause}")
        } else if key_str == "Escape" {
            format!("tell application \"System Events\" to key code 53{using_clause}")
        } else if key_str == "BackSpace" {
            format!("tell application \"System Events\" to key code 51{using_clause}")
        } else if key_str == "space" {
            format!("tell application \"System Events\" to key code 49{using_clause}")
        } else if key_str == "Left" {
            format!("tell application \"System Events\" to key code 123{using_clause}")
        } else if key_str == "Right" {
            format!("tell application \"System Events\" to key code 124{using_clause}")
        } else {
            format!("tell application \"System Events\" to keystroke \"{key_str}\"{using_clause}")
        };
        let _ = std::process::Command::new("osascript").args(["-e", &script]).status();
    }

    #[cfg(all(not(windows), not(target_os = "macos")))]
    {
        let try_wtype = || -> bool {
            let mut args = Vec::new();
            for &m in &mod_strs {
                let wm = match m {
                    "ctrl" => "ctrl",
                    "shift" => "shift",
                    "alt" => "alt",
                    "super" => "logo",
                    other => other,
                };
                args.push("-M");
                args.push(wm);
            }
            if !key_str.is_empty() {
                args.push("-k");
                args.push(key_str);
            }
            for &m in mod_strs.iter().rev() {
                let wm = match m {
                    "ctrl" => "ctrl",
                    "shift" => "shift",
                    "alt" => "alt",
                    "super" => "logo",
                    other => other,
                };
                args.push("-m");
                args.push(wm);
            }
            std::process::Command::new("wtype").args(&args).status().map_or(false, |s| s.success())
        };

        let combo = if mod_strs.is_empty() {
            key_str.to_string()
        } else {
            format!("{}+{}", mod_strs.join("+"), key_str)
        };

        let try_ydotool = || -> bool {
            std::process::Command::new("ydotool")
                .args(["key", &combo])
                .status()
                .map_or(false, |s| s.success())
        };

        let try_xdotool = || -> bool {
            std::process::Command::new("xdotool")
                .args(["key", "--clearmodifiers", &combo])
                .status()
                .map_or(false, |s| s.success())
        };

        if is_wayland_session() {
            if try_wtype() || try_ydotool() || try_xdotool() {
                return;
            }
        } else if try_xdotool() || try_wtype() || try_ydotool() {
            return;
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

#[cfg(target_os = "macos")]
pub fn get_active_field_content() -> Option<String> {
    const SCRIPT: &str = r#"tell application "System Events"
        try
            set frontApp to first application process whose frontmost is true
            set focusedElem to value of attribute "AXFocusedUIElement" of frontApp
            if focusedElem is not missing value then
                try
                    set v to value of focusedElem
                    if v is not missing value and (v as text) is not "" then return v as text
                end try
                try
                    set v to value of attribute "AXValue" of focusedElem
                    if v is not missing value and (v as text) is not "" then return v as text
                end try
                try
                    set v to value of attribute "AXSelectedText" of focusedElem
                    if v is not missing value and (v as text) is not "" then return v as text
                end try
            end if
        end try
    end tell"#;

    let output = std::process::Command::new("osascript")
        .args(["-e", SCRIPT])
        .output()
        .ok()?;

    if output.status.success() {
        let text = String::from_utf8_lossy(&output.stdout).trim().to_string();
        if !text.is_empty() {
            return Some(text);
        }
    }
    None
}

#[cfg(all(not(windows), not(target_os = "macos")))]
pub fn get_active_field_content() -> Option<String> {
    const ATSPI_FOCUSED_SCRIPT: &str = r#"
import sys
try:
    import gi
    gi.require_version('Atspi', '2.0')
    from gi.repository import Atspi
except Exception:
    sys.exit(0)

desktop = Atspi.get_desktop(0)
def find_focused(accessible, depth=0):
    if depth > 8: return None
    try:
        state = accessible.get_state_set()
        if state and state.contains(Atspi.StateType.FOCUSED):
            t = accessible.get_text_iface()
            if t: return t.get_text(0, -1)
        for i in range(accessible.get_child_count()):
            res = find_focused(accessible.get_child_at_index(i), depth + 1)
            if res is not None: return res
    except Exception:
        pass
    return None

txt = find_focused(desktop)
if txt: sys.stdout.write(txt)
"#;
    if let Ok(output) = std::process::Command::new("python3").args(["-c", ATSPI_FOCUSED_SCRIPT]).output() {
        let res = String::from_utf8_lossy(&output.stdout).trim().to_string();
        if !res.is_empty() { return Some(res); }
    }
    None
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

pub fn compute_screen_diff(old_summary: &str, new_summary: &str) -> String {
    let clean_old = strip_diff_section(old_summary);
    let clean_new = strip_diff_section(new_summary);

    let is_ignored_diff_meta = |l: &str| -> bool {
        let t = l.trim();
        t.starts_with("===")
            || t.starts_with("---")
            || t.starts_with("####")
            || t.starts_with("Espace de travail")
            || t.starts_with("[Consigne prioritaire]")
            || t.starts_with("[Note d'intention")
            || t.starts_with("Fenêtre au premier plan")
            || t.starts_with("- ~~")
            || t.starts_with("- **")
    };

    let old_lines: Vec<String> = clean_old
        .lines()
        .filter(|l| !is_ignored_diff_meta(l))
        .map(clean_diff_line)
        .filter(|l| !l.is_empty())
        .collect();

    let new_lines: Vec<String> = clean_new
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

    if removed.len() > 10 {
        let count = removed.len();
        removed.truncate(6);
        removed.push(format!("... et {count} éléments de la vue précédente masqués"));
    }
    if added.len() > 25 {
        let count = added.len();
        added.truncate(20);
        added.push(format!("... et {count} autres nouveaux éléments"));
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
pub fn summarize_screen_state(target_window: Option<&str>, _autoscroll: bool) -> String {
    let work_area = get_desktop_work_area();
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

        // A. Champs de formulaire, barres de recherche et sélecteurs (input, select, textarea)
        let form_fields: Vec<&UiaElementInfo> = elements.iter()
            .filter(|e| (!has_web_doc || e.is_web_content) && (e.is_edit_or_textarea || e.is_input || e.is_select))
            .collect();
        if !form_fields.is_empty() {
            let prefix = if has_web_doc { "  [Page web - champs interactables (input / select / textarea)] :\n" } else { "  [Champs interactables (input / select / textarea)] :\n" };
            active_window_content.push_str(prefix);
            for edit in form_fields.iter().take(15) {
                let field_type_tag = if edit.is_select {
                    "[select/combobox]"
                } else if edit.is_explicit_textarea {
                    "[textarea]"
                } else {
                    "[input]"
                };
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
                active_window_content.push_str(&format!("  • {} {}{}\n", field_type_tag, label, val));
            }
        }

        // A bis. Suggestions dynamiques, popups et options de listes déroulantes ouvertes
        let option_fields: Vec<&UiaElementInfo> = elements.iter()
            .filter(|e| (!has_web_doc || e.is_web_content) && e.is_option && !e.name.is_empty())
            .collect();
        if !option_fields.is_empty() {
            let prefix = if has_web_doc {
                "  [Page web - suggestions / options de sélection (popup)] :\n"
            } else {
                "  [Suggestions / options de sélection (popup)] :\n"
            };
            active_window_content.push_str(prefix);
            for opt in option_fields.iter().take(20) {
                let mut label = opt.name.clone();
                if !opt.aria_description.is_empty() {
                    label.push_str(&format!(" ({})", opt.aria_description));
                }
                active_window_content.push_str(&format!("  • [option/suggestion] {}\n", label));
            }
        }

        // A ter. Lecteurs vidéo et audio interactables
        let media_fields: Vec<&UiaElementInfo> = elements.iter()
            .filter(|e| (!has_web_doc || e.is_web_content) && e.is_media)
            .collect();
        if !media_fields.is_empty() {
            let prefix = if has_web_doc {
                "  [Page web - lecteurs vidéo / audio interactables (cliquer pour jouer)] :\n"
            } else {
                "  [Lecteurs vidéo / audio interactables (cliquer pour jouer)] :\n"
            };
            active_window_content.push_str(prefix);
            for m in media_fields.iter().take(10) {
                let tag = if m.aria_role == "audio" || m.name.to_lowercase().contains("audio") || m.localized_type.contains("audio") {
                    "[audio]"
                } else {
                    "[video]"
                };
                let mut label = if !m.name.is_empty() {
                    m.name.clone()
                } else if !m.primary_class.is_empty() {
                    format!(".{}", m.primary_class)
                } else {
                    "Média".to_string()
                };
                if !m.aria_indications.is_empty() {
                    label.push_str(&format!(" ({})", m.aria_indications));
                }
                active_window_content.push_str(&format!("  • {} {}\n", tag, label));
            }
        }

        // A quater. Curseurs et sliders OS (ex: réglage du volume dans Paramètres)
        let sliders: Vec<&UiaElementInfo> = elements.iter()
            .filter(|e| (!has_web_doc || e.is_web_content) && e.is_slider && !e.name.is_empty())
            .collect();
        if !sliders.is_empty() {
            active_window_content.push_str("  [Curseurs / Barres de réglage (Volume / Sliders)] :\n");
            for s in sliders.iter().take(10) {
                let val = if !s.value_text.is_empty() {
                    format!(" (niveau actuel : {})", s.value_text)
                } else {
                    String::new()
                };
                active_window_content.push_str(&format!("  • [slider] {}{}\n", s.name, val));
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

        // B bis. Bannières et popups d'alerte / notification web / consentement
        let notification_banners: Vec<String> = elements
            .iter()
            .filter(|e| {
                (!has_web_doc || e.is_web_content)
                    && !e.is_edit_or_textarea
                    && !e.is_button
                    && !e.name.is_empty()
                    && e.name.len() >= 6
                    && (e.name.to_lowercase().contains("notification")
                        || e.name.to_lowercase().contains("ne manquez rien")
                        || e.name.to_lowercase().contains("activez les notifications")
                        || e.name.to_lowercase().contains("autoriser les notifications")
                        || e.name.to_lowercase().contains("abonnez-vous à l'actualité")
                        || e.aria_role == "alert"
                        || e.aria_role == "alertdialog")
            })
            .map(|e| e.name.clone())
            .collect();
        if !notification_banners.is_empty() {
            let mut unique_banners = notification_banners;
            unique_banners.dedup();
            let display_banners: Vec<String> = unique_banners.into_iter().take(3).collect();
            active_window_content.push_str(&format!(
                "  [Page web - bannières / popups de notification ou abonnement à traiter (Activer / Fermer)] : {}\n",
                display_banners.join(" | ")
            ));
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
                        || e.name.to_lowercase().contains("notification")
                        || e.name.to_lowercase().contains("actualité")
                        || e.name.to_lowercase().contains("actualite")
                        || e.name.to_lowercase().contains("activez")
                        || e.name.to_lowercase().contains("abonnez")
                        || e.name.to_lowercase().contains("ne manquez")
                        || e.name.to_lowercase().contains("autoriser")
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
                        || e.is_media
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

#[cfg(target_os = "macos")]
pub fn summarize_screen_state(target_window: Option<&str>, _autoscroll: bool) -> String {
    let user_windows = list_user_windows();
    let active_win = get_active_macos_window();
    let prev_screen_snapshot = LAST_SCREEN_SUMMARY.lock().ok().and_then(|g| g.clone());

    let mut out = String::new();
    out.push_str("=== Rapport d'analyse de l'écran (macOS WindowServer / Cocoa) ===\n");

    let active_title = active_win.as_deref().unwrap_or("Fenêtre active");
    out.push_str(&format!(
        "Fenêtre au premier plan (contexte actif) : \"{}\"\n\
         [Note d'intention contextuelle] : Cette fenêtre est le contexte direct de l'utilisateur. Toute instruction relative à une question, recherche, météo, achat ou formulaire doit être résolue prioritairement dans cette fenêtre active.\n\n",
        active_title
    ));

    if let Some(content) = get_active_field_content() {
        let safe_content = truncate_with_notice(content.trim(), 1200);
        out.push_str(&format!(
            "  [Champ de saisie focalisé (AXFocusedUIElement)] :\n\"\"\"\n{}\n\"\"\"\n\n",
            safe_content
        ));
    }

    out.push_str("=== Fenêtres macOS détectées ===\n");
    for (_id, title) in &user_windows {
        let is_term = is_terminal_window("", title);
        let is_browser = is_browser_window(title);
        let kind = if is_term {
            " [Terminal]"
        } else if is_browser {
            " [Navigateur web]"
        } else {
            ""
        };
        out.push_str(&format!("- Fenêtre \"{}\"{}\n", title, kind));
    }

    let ax_elements = query_macos_ax_elements();
    if !ax_elements.is_empty() {
        out.push_str("\n=== Éléments interactifs accessibles (macOS Accessibility) ===\n");

        let text_fields: Vec<&MacosAxElementRaw> = ax_elements.iter()
            .filter(|e| e.role == "AXTextField" || e.role == "AXTextArea" || e.role == "AXComboBox")
            .collect();
        if !text_fields.is_empty() {
            out.push_str("  [Champs de saisie (input / textarea)] :\n");
            for tf in text_fields.iter().take(15) {
                let val_str = if !tf.value.is_empty() {
                    format!(" = \"{}\"", truncate_with_notice(&tf.value, 60))
                } else {
                    String::new()
                };
                let label = if !tf.name.is_empty() { &tf.name } else { "Champ" };
                out.push_str(&format!("  • [{}] \"{}\"{}\n", tf.role.strip_prefix("AX").unwrap_or(&tf.role), label, val_str));
            }
        }

        let buttons: Vec<&MacosAxElementRaw> = ax_elements.iter()
            .filter(|e| e.role == "AXButton" || e.role == "AXPopUpButton" || e.role == "AXCheckBox" || e.role == "AXRadioButton")
            .collect();
        if !buttons.is_empty() {
            let names: Vec<String> = buttons.iter()
                .filter(|b| !b.name.is_empty())
                .map(|b| b.name.clone())
                .take(30)
                .collect();
            if !names.is_empty() {
                out.push_str(&format!("  [Boutons / contrôles cliquables] : {}\n", names.join(", ")));
            }
        }

        let links: Vec<&MacosAxElementRaw> = ax_elements.iter()
            .filter(|e| e.role == "AXLink")
            .collect();
        if !links.is_empty() {
            let link_names: Vec<String> = links.iter()
                .filter(|l| !l.name.is_empty())
                .map(|l| l.name.clone())
                .take(30)
                .collect();
            if !link_names.is_empty() {
                out.push_str(&format!("  [Liens / résultats cliquables] : {}\n", link_names.join(" | ")));
            }
        }
    }

    if let Some(clip) = clipboard::get_text() {
        let clip_trim = clip.trim();
        if !clip_trim.is_empty() {
            out.push_str(&format!("\n[Contenu presse-papiers macOS (Clipboard)] :\n\"\"\"\n{}\n\"\"\"\n", truncate_with_notice(clip_trim, 600)));
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

#[cfg(all(not(windows), not(target_os = "macos")))]
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

    // Injection des éléments interactifs AT-SPI découverts
    let atspi_elements = query_atspi_elements();
    if !atspi_elements.is_empty() {
        out.push_str("\n=== Éléments accessibles détectés (AT-SPI Linux) ===\n");
        for elem in atspi_elements.iter().take(30) {
            let val = if !elem.value.is_empty() {
                format!(" = \"{}\"", truncate_with_notice(&elem.value, 60))
            } else {
                String::new()
            };
            let label = if !elem.name.is_empty() { &elem.name } else { "(sans nom)" };
            out.push_str(&format!("  • [{}] \"{}\" [pos: {},{}]{}\n", elem.role, label, elem.x, elem.y, val));
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

#[derive(Default)]
pub struct ActionHistoryManager {
    pub undo_stack: Vec<UndoOperation>,
    pub redo_stack: Vec<UndoOperation>,
}

impl ActionHistoryManager {
    pub fn push_undo(&mut self, op: UndoOperation) {
        self.undo_stack.push(op);
        self.redo_stack.clear();
    }

    pub fn pop_undo(&mut self) -> Option<UndoOperation> {
        self.undo_stack.pop()
    }

    pub fn pop_redo(&mut self) -> Option<UndoOperation> {
        self.redo_stack.pop()
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
            #[cfg(not(windows))]
            {
                let _ = raw_hwnd;
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
            #[cfg(windows)]
            {
                const VK_CONTROL: VIRTUAL_KEY = VIRTUAL_KEY(0x11);
                const VK_W: VIRTUAL_KEY = VIRTUAL_KEY(0x57);
                send_hotkey(&[VK_CONTROL], VK_W);
            }
            #[cfg(target_os = "macos")]
            {
                let _ = std::process::Command::new("osascript")
                    .args(["-e", "tell application \"System Events\" to keystroke \"w\" using command down"])
                    .status();
            }
            #[cfg(all(not(windows), not(target_os = "macos")))]
            {
                let _ = std::process::Command::new("xdotool").args(["key", "ctrl+w"]).status();
            }
            (
                "Onglet ouvert fermé (Ctrl+W).".to_string(),
                Some(UndoOperation::ReopenTab { target_hwnd }),
            )
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
            #[cfg(windows)]
            {
                const VK_CONTROL: VIRTUAL_KEY = VIRTUAL_KEY(0x11);
                const VK_SHIFT: VIRTUAL_KEY = VIRTUAL_KEY(0x10);
                const VK_T: VIRTUAL_KEY = VIRTUAL_KEY(0x54);
                send_hotkey(&[VK_CONTROL, VK_SHIFT], VK_T);
            }
            #[cfg(target_os = "macos")]
            {
                let _ = std::process::Command::new("osascript")
                    .args(["-e", "tell application \"System Events\" to keystroke \"t\" using {command down, shift down}"])
                    .status();
            }
            #[cfg(all(not(windows), not(target_os = "macos")))]
            {
                let _ = std::process::Command::new("xdotool").args(["key", "ctrl+shift+t"]).status();
            }
            (
                "Onglet rouvert (Ctrl+Shift+T).".to_string(),
                Some(UndoOperation::CloseTab { target_hwnd }),
            )
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
            #[cfg(windows)]
            {
                const VK_MENU: VIRTUAL_KEY = VIRTUAL_KEY(0x12);
                const VK_LEFT: VIRTUAL_KEY = VIRTUAL_KEY(0x25);
                send_hotkey(&[VK_MENU], VK_LEFT);
            }
            #[cfg(target_os = "macos")]
            {
                let _ = std::process::Command::new("osascript")
                    .args(["-e", "tell application \"System Events\" to keystroke \"[\" using command down"])
                    .status();
            }
            #[cfg(all(not(windows), not(target_os = "macos")))]
            {
                let _ = std::process::Command::new("xdotool").args(["key", "alt+Left"]).status();
            }
            (
                "Retour arrière dans l'historique du navigateur.".to_string(),
                Some(UndoOperation::NavigateForward { target_hwnd }),
            )
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
            #[cfg(windows)]
            {
                const VK_MENU: VIRTUAL_KEY = VIRTUAL_KEY(0x12);
                const VK_RIGHT: VIRTUAL_KEY = VIRTUAL_KEY(0x27);
                send_hotkey(&[VK_MENU], VK_RIGHT);
            }
            #[cfg(target_os = "macos")]
            {
                let _ = std::process::Command::new("osascript")
                    .args(["-e", "tell application \"System Events\" to keystroke \"]\" using command down"])
                    .status();
            }
            #[cfg(all(not(windows), not(target_os = "macos")))]
            {
                let _ = std::process::Command::new("xdotool").args(["key", "alt+Right"]).status();
            }
            (
                "Navigation avant dans l'historique du navigateur.".to_string(),
                Some(UndoOperation::NavigateBack { target_hwnd }),
            )
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
            #[cfg(windows)]
            {
                const VK_CONTROL: VIRTUAL_KEY = VIRTUAL_KEY(0x11);
                const VK_Z: VIRTUAL_KEY = VIRTUAL_KEY(0x5A);
                send_hotkey(&[VK_CONTROL], VK_Z);
            }
            #[cfg(target_os = "macos")]
            {
                let _ = std::process::Command::new("osascript")
                    .args(["-e", "tell application \"System Events\" to keystroke \"z\" using command down"])
                    .status();
            }
            #[cfg(all(not(windows), not(target_os = "macos")))]
            {
                let _ = std::process::Command::new("xdotool").args(["key", "ctrl+z"]).status();
            }
            (
                "Texte annulé (Ctrl+Z).".to_string(),
                Some(UndoOperation::RedoText { target_hwnd }),
            )
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
            #[cfg(windows)]
            {
                const VK_CONTROL: VIRTUAL_KEY = VIRTUAL_KEY(0x11);
                const VK_Y: VIRTUAL_KEY = VIRTUAL_KEY(0x59);
                send_hotkey(&[VK_CONTROL], VK_Y);
            }
            #[cfg(target_os = "macos")]
            {
                let _ = std::process::Command::new("osascript")
                    .args(["-e", "tell application \"System Events\" to keystroke \"z\" using {command down, shift down}"])
                    .status();
            }
            #[cfg(all(not(windows), not(target_os = "macos")))]
            {
                let _ = std::process::Command::new("xdotool").args(["key", "ctrl+y"]).status();
            }
            (
                "Texte rétabli (Ctrl+Y).".to_string(),
                Some(UndoOperation::RevertText { target_hwnd }),
            )
        }
        UndoOperation::ScrollOpposite { direction, amount, target_hwnd } => {
            let opp_dir = if direction == "down" { "up" } else { "down" };
            #[cfg(windows)]
            {
                let _ = execute_system_actions(&[AgentAction::Scroll {
                    direction: opp_dir.to_string(),
                    amount: Some(amount),
                    window: None,
                }], 0);
            }
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

pub use crate::commands::execute_system_actions;
#[cfg(windows)]
fn name_words_count(s: &str) -> usize {
    s.split_whitespace().count()
}