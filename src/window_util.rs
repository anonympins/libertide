#[cfg(windows)]
use std::time::Duration;
#[cfg(windows)]
use windows::core::w;
#[cfg(windows)]
use windows::Win32::Foundation::{BOOL, HWND, LPARAM, RECT};
#[cfg(windows)]
use windows::Win32::UI::Input::KeyboardAndMouse::VIRTUAL_KEY;
#[cfg(windows)]
use windows::Win32::UI::WindowsAndMessaging::{
    EnumWindows, FindWindowW, GetClassNameW, GetForegroundWindow, GetSystemMetrics,
    GetWindowRect, GetWindowTextLengthW, GetWindowTextW, IsIconic, IsWindowVisible,
    SetForegroundWindow, SetWindowPos, ShowWindow, SM_CXSCREEN, SM_CYSCREEN,
    SWP_NOZORDER, SWP_SHOWWINDOW, SW_MAXIMIZE, SW_RESTORE,
};

use crate::pattern::{extract_target_propositions, matches_pattern};
#[cfg(windows)]
use crate::send_hotkey;

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
pub fn list_user_windows() -> Vec<(HWND, String)> {
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
pub fn get_desktop_work_area() -> RECT {
    extern "system" {
        fn SystemParametersInfoW(uiAction: u32, uiParam: u32, pvParam: *mut std::ffi::c_void, fWinIni: u32) -> BOOL;
    }
    const SPI_GETWORKAREA: u32 = 0x0030;
    let mut rect = RECT::default();
    unsafe {
        if SystemParametersInfoW(SPI_GETWORKAREA, 0, &mut rect as *mut _ as *mut std::ffi::c_void, 0).as_bool()
            && rect.right > rect.left
            && rect.bottom > rect.top
        {
            return rect;
        }
    }
    let screen_w = unsafe { GetSystemMetrics(SM_CXSCREEN).max(800) };
    let screen_h = unsafe { (GetSystemMetrics(SM_CYSCREEN) - 48).max(600) };
    RECT { left: 0, top: 0, right: screen_w, bottom: screen_h }
}

#[cfg(windows)]
pub fn get_window_process_info(hwnd: HWND) -> (u32, String) {
    extern "system" {
        fn GetWindowThreadProcessId(hwnd: HWND, lpdwprocessid: *mut u32) -> u32;
        fn OpenProcess(dwdesiredaccess: u32, binherithandle: i32, dwprocessid: u32) -> *mut std::ffi::c_void;
        fn QueryFullProcessImageNameW(hprocess: *mut std::ffi::c_void, dwflags: u32, lpexename: *mut u16, lpsize: *mut u32) -> i32;
        fn CloseHandle(hobject: *mut std::ffi::c_void) -> i32;
    }
    let mut pid: u32 = 0;
    unsafe {
        GetWindowThreadProcessId(hwnd, &mut pid);
    }
    if pid == 0 {
        return (0, "inconnu".to_string());
    }

    const PROCESS_QUERY_LIMITED_INFORMATION: u32 = 0x1000;
    let h_proc = unsafe { OpenProcess(PROCESS_QUERY_LIMITED_INFORMATION, 0, pid) };
    if h_proc.is_null() {
        return (pid, "inconnu".to_string());
    }

    let mut path_buf = [0u16; 1024];
    let mut size = path_buf.len() as u32;
    let ok = unsafe { QueryFullProcessImageNameW(h_proc, 0, path_buf.as_mut_ptr(), &mut size) };
    unsafe { CloseHandle(h_proc); }

    if ok != 0 && size > 0 {
        let full_path = String::from_utf16_lossy(&path_buf[..size as usize]);
        let exe_name = std::path::Path::new(&full_path)
            .file_name()
            .and_then(|s| s.to_str())
            .unwrap_or(&full_path)
            .to_string();
        (pid, exe_name)
    } else {
        (pid, "inconnu".to_string())
    }
}

#[cfg(windows)]
pub fn find_largest_and_most_centered_window(user_windows: &[(HWND, String)]) -> Option<HWND> {
    if user_windows.is_empty() {
        return None;
    }

    let wa = get_desktop_work_area();
    let sw = (wa.right - wa.left).max(1) as f32;
    let sh = (wa.bottom - wa.top).max(1) as f32;
    let screen_cx = (wa.left + wa.right) as f32 / 2.0;
    let screen_cy = (wa.top + wa.bottom) as f32 / 2.0;
    let max_dist = (sw * sw + sh * sh).sqrt() / 2.0;

    let mut best_hwnd = None;
    let mut best_score = -1.0f32;

    for (z_idx, &(hwnd, _)) in user_windows.iter().enumerate() {
        unsafe {
            if IsIconic(hwnd).as_bool() {
                continue;
            }
            let mut r = RECT::default();
            if GetWindowRect(hwnd, &mut r).is_ok() {
                let w = (r.right - r.left).max(0) as f32;
                let h = (r.bottom - r.top).max(0) as f32;
                let area = w * h;
                if area <= 100.0 {
                    continue;
                }

                let win_cx = (r.left + r.right) as f32 / 2.0;
                let win_cy = (r.top + r.bottom) as f32 / 2.0;
                let dx = win_cx - screen_cx;
                let dy = win_cy - screen_cy;
                let dist = (dx * dx + dy * dy).sqrt();

                let center_factor = (1.0 - (dist / max_dist.max(1.0)).min(1.0)).max(0.05);
                let z_factor = 1.0 / (1.0 + 0.35 * (z_idx as f32));
                let score = area * center_factor * z_factor;

                if score > best_score {
                    best_score = score;
                    best_hwnd = Some(hwnd);
                }
            }
        }
    }

    best_hwnd.or_else(|| user_windows.first().map(|(h, _)| *h))
}

#[cfg(windows)]
pub fn get_active_or_best_window(user_windows: &[(HWND, String)]) -> Option<HWND> {
    let overlay_hwnd = unsafe {
        FindWindowW(None, w!("Libertide overlay")).unwrap_or(HWND(std::ptr::null_mut()))
    };
    let fg = unsafe { GetForegroundWindow() };
    if !fg.0.is_null() && fg != overlay_hwnd && user_windows.iter().any(|(h, _)| *h == fg) {
        Some(fg)
    } else {
        find_largest_and_most_centered_window(user_windows)
    }
}

#[cfg(windows)]
pub fn strip_spaces_and_symbols(s: &str) -> String {
    s.to_lowercase().chars().filter(|c| c.is_alphanumeric()).collect()
}

#[cfg(windows)]
pub fn window_matches_keyword(hwnd: HWND, title: &str, kw: &str) -> bool {
    let propositions = extract_target_propositions(kw);
    let t_lower = title.to_lowercase();
    let t_norm = strip_spaces_and_symbols(title);

    for prop in &propositions {
        let p_clean = prop.trim();
        let p_lower = p_clean.to_lowercase();
        let p_stem = std::path::Path::new(p_clean)
            .file_stem()
            .and_then(|s| s.to_str())
            .unwrap_or(p_clean);
        let p_norm = strip_spaces_and_symbols(p_stem);

        let is_generic_browser = p_lower == "browser" || p_lower == "navigateur" || p_lower == "web" || p_lower == "internet";
        if is_generic_browser {
            if is_browser_hwnd(hwnd, title)
                || t_lower.contains("chrome")
                || t_lower.contains("edge")
                || t_lower.contains("firefox")
                || t_lower.contains("brave")
            {
                return true;
            }
            continue;
        }

        if p_lower == "google" || p_lower == "chrome" || p_lower == "google chrome" {
            if t_lower.contains("chrome") || t_lower.contains("google") {
                return true;
            }
            continue;
        }

        if matches_pattern(p_clean, title)
            || t_lower.contains(&p_lower)
            || (!p_norm.is_empty() && (t_norm.contains(&p_norm) || (t_norm.len() >= 3 && p_norm.contains(&t_norm))))
        {
            return true;
        }

        unsafe {
            let mut class_buf = [0u16; 256];
            let len = GetClassNameW(hwnd, &mut class_buf);
            if len > 0 {
                let class_name = String::from_utf16_lossy(&class_buf[..len as usize]);
                let c_norm = strip_spaces_and_symbols(&class_name);
                if matches_pattern(p_clean, &class_name)
                    || class_name.to_lowercase().contains(&p_lower)
                    || (!p_norm.is_empty() && (c_norm.contains(&p_norm) || (c_norm.len() >= 3 && p_norm.contains(&c_norm))))
                {
                    return true;
                }
            }
        }
    }

    false
}

#[cfg(windows)]
pub fn find_windows_matching(
    keyword: &str,
    user_windows: &[(HWND, String)],
    preferred_hwnd: Option<HWND>,
) -> Vec<HWND> {
    let kw = keyword.trim().to_lowercase();
    let is_active_kw = kw.is_empty()
        || kw == "cette"
        || kw == "cette fenetre"
        || kw == "cette fenêtre"
        || kw == "active"
        || kw == "courante"
        || kw == "en cours"
        || kw == "actuelle"
        || kw == "premier plan";

    if is_active_kw {
        if let Some(pref) = preferred_hwnd {
            return vec![pref];
        }
        if let Some(best) = find_largest_and_most_centered_window(user_windows) {
            return vec![best];
        }
        return Vec::new();
    }
    let mut matches = Vec::new();

    if let Some(pref) = preferred_hwnd {
        if let Some((_, title)) = user_windows.iter().find(|(h, _)| *h == pref) {
            if window_matches_keyword(pref, title, &kw) {
                matches.push(pref);
            }
        }
    }

    for &(hwnd, ref title) in user_windows {
        if Some(hwnd) == preferred_hwnd && matches.contains(&hwnd) {
            continue;
        }
        if window_matches_keyword(hwnd, title, &kw) && !matches.contains(&hwnd) {
            matches.push(hwnd);
        }
    }
    matches
}

#[cfg(windows)]
pub fn is_browser_hwnd(hwnd: HWND, title: &str) -> bool {
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
pub fn apply_window_rect(hwnd: HWND, x: i32, y: i32, width: i32, height: i32) {
    unsafe {
        let _ = ShowWindow(hwnd, SW_RESTORE);
        let _ = SetForegroundWindow(hwnd);
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
pub fn snap_window_pair(left_hwnd: HWND, right_hwnd: HWND) {
    const VK_LWIN: VIRTUAL_KEY = VIRTUAL_KEY(0x5B);
    const VK_LEFT: VIRTUAL_KEY = VIRTUAL_KEY(0x25);
    const VK_RIGHT: VIRTUAL_KEY = VIRTUAL_KEY(0x27);
    const VK_ESCAPE: VIRTUAL_KEY = VIRTUAL_KEY(0x1B);

    unsafe {
        let _ = ShowWindow(left_hwnd, SW_RESTORE);
        let _ = SetForegroundWindow(left_hwnd);
    }
    std::thread::sleep(Duration::from_millis(100));
    send_hotkey(&[VK_LWIN], VK_LEFT);
    std::thread::sleep(Duration::from_millis(120));
    send_hotkey(&[], VK_ESCAPE);
    std::thread::sleep(Duration::from_millis(60));

    unsafe {
        let _ = ShowWindow(right_hwnd, SW_RESTORE);
        let _ = SetForegroundWindow(right_hwnd);
    }
    std::thread::sleep(Duration::from_millis(100));
    send_hotkey(&[VK_LWIN], VK_RIGHT);
    std::thread::sleep(Duration::from_millis(120));
    send_hotkey(&[], VK_ESCAPE);
}

#[cfg(windows)]
pub fn snap_window_native(hwnd: HWND, is_right: bool) {
    const VK_LWIN: VIRTUAL_KEY = VIRTUAL_KEY(0x5B);
    const VK_LEFT: VIRTUAL_KEY = VIRTUAL_KEY(0x25);
    const VK_RIGHT: VIRTUAL_KEY = VIRTUAL_KEY(0x27);
    const VK_ESCAPE: VIRTUAL_KEY = VIRTUAL_KEY(0x1B);

    unsafe {
        let _ = ShowWindow(hwnd, SW_RESTORE);
        let _ = SetForegroundWindow(hwnd);
    }
    std::thread::sleep(Duration::from_millis(100));
    let key = if is_right { VK_RIGHT } else { VK_LEFT };
    send_hotkey(&[VK_LWIN], key);
    std::thread::sleep(Duration::from_millis(120));
    send_hotkey(&[], VK_ESCAPE);
}

#[cfg(windows)]
pub fn classify_immersion_quadrants(hwnds: &[HWND], user_windows: &[(HWND, String)]) -> [Option<HWND>; 4] {
    let mut slots: [Option<HWND>; 4] = [None; 4];
    let mut unassigned: Vec<HWND> = Vec::new();

    for &hwnd in hwnds {
        let (_pid, exe_name) = get_window_process_info(hwnd);
        let title = user_windows
            .iter()
            .find(|(h, _)| *h == hwnd)
            .map(|(_, t)| t.as_str())
            .unwrap_or("")
            .to_lowercase();
        let exe_lower = exe_name.to_lowercase();

        let is_terminal = exe_lower.contains("cmd")
            || exe_lower.contains("powershell")
            || exe_lower.contains("windowsterminal")
            || exe_lower.contains("wt")
            || exe_lower.contains("bash")
            || exe_lower.contains("mintty")
            || title.contains("terminal")
            || title.contains("invite de commandes")
            || title.contains("powershell");

        let is_music = exe_lower.contains("spotify")
            || exe_lower.contains("music")
            || title.contains("spotify")
            || title.contains("music")
            || title.contains("lofi")
            || title.contains("lo-fi")
            || title.contains("ambient")
            || title.contains("youtube")
            || title.contains("soundcloud")
            || title.contains("deezer");

        let is_ide = !is_terminal
            && (exe_lower.contains("code")
                || exe_lower.contains("devenv")
                || exe_lower.contains("idea")
                || exe_lower.contains("clion")
                || exe_lower.contains("pycharm")
                || exe_lower.contains("rustrover")
                || exe_lower.contains("sublime")
                || exe_lower.contains("notepad")
                || exe_lower.contains("zed"));

        let is_browser = is_browser_hwnd(hwnd, &title);

        if is_music && slots[3].is_none() {
            slots[3] = Some(hwnd);
        } else if is_ide && slots[0].is_none() {
            slots[0] = Some(hwnd);
        } else if is_terminal && slots[2].is_none() {
            slots[2] = Some(hwnd);
        } else if is_browser && slots[1].is_none() {
            slots[1] = Some(hwnd);
        } else {
            unassigned.push(hwnd);
        }
    }

    for slot_idx in [0, 1, 2, 3] {
        if slots[slot_idx].is_none() && !unassigned.is_empty() {
            slots[slot_idx] = Some(unassigned.remove(0));
        }
    }
    slots
}

#[cfg(windows)]
pub fn launch_browser_new_window(url: &str) -> Option<HWND> {
    let initial_hwnds: std::collections::HashSet<isize> = list_user_windows()
        .into_iter()
        .map(|(h, _)| h.0 as isize)
        .collect();

    let chrome_paths = [
        r"C:\Program Files\Google\Chrome\Application\chrome.exe",
        r"C:\Program Files (x86)\Google\Chrome\Application\chrome.exe",
        r"C:\Program Files\BraveSoftware\Brave-Browser\Application\brave.exe",
    ];
    let edge_paths = [
        r"C:\Program Files (x86)\Microsoft\Edge\Application\msedge.exe",
        r"C:\Program Files\Microsoft\Edge\Application\msedge.exe",
    ];

    let mut spawned = false;
    for path in chrome_paths.iter().chain(edge_paths.iter()) {
        if std::path::Path::new(path).exists() {
            if std::process::Command::new(path)
                .args(["--force-renderer-accessibility", "--start-maximized", "--new-window", url])
                .spawn()
                .is_ok()
            {
                spawned = true;
                break;
            }
        }
    }

    if !spawned && std::process::Command::new("chrome")
        .args(["--force-renderer-accessibility", "--start-maximized", "--new-window", url])
        .spawn()
        .is_ok()
    {
        spawned = true;
    }

    if !spawned && std::process::Command::new("msedge")
        .args(["--force-renderer-accessibility", "--start-maximized", "--new-window", url])
        .spawn()
        .is_ok()
    {
        spawned = true;
    }

    if !spawned {
        let _ = std::process::Command::new("cmd")
            .args(["/C", "start", "", "/MAX", "chrome", "--start-maximized", "--new-window", url])
            .spawn();
        spawned = true;
    }

    if spawned {
        for _ in 0..25 {
            std::thread::sleep(Duration::from_millis(100));
            let current_windows = list_user_windows();
            for (hwnd, title) in &current_windows {
                if !initial_hwnds.contains(&(hwnd.0 as isize)) {
                    let t = title.to_lowercase();
                    if is_browser_hwnd(*hwnd, title)
                        || t.contains("chrome")
                        || t.contains("edge")
                        || t.contains("brave")
                        || !t.is_empty()
                    {
                        unsafe {
                            let _ = ShowWindow(*hwnd, SW_MAXIMIZE);
                            let _ = SetForegroundWindow(*hwnd);
                        }
                        return Some(*hwnd);
                    }
                }
            }
        }
    }
    None
}