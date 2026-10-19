use std::path::Path;
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
pub fn is_window_hung(hwnd: HWND) -> bool {
    extern "system" {
        fn IsHungAppWindow(hwnd: HWND) -> BOOL;
    }
    if hwnd.0.is_null() {
        return false;
    }
    unsafe { IsHungAppWindow(hwnd).as_bool() }
}

#[cfg(windows)]
pub fn is_terminal_window(hwnd: HWND, title: &str) -> bool {
    let (_pid, exe_name) = get_window_process_info(hwnd);
    let exe_lower = exe_name.to_lowercase();
    let title_lower = title.to_lowercase();

    if exe_lower.contains("putty")
        || exe_lower.contains("kitty")
        || exe_lower.contains("cmd")
        || exe_lower.contains("powershell")
        || exe_lower.contains("pwsh")
        || exe_lower.contains("windowsterminal")
        || exe_lower.contains("wt")
        || exe_lower.contains("bash")
        || exe_lower.contains("mintty")
        || exe_lower.contains("conemu")
        || exe_lower.contains("alacritty")
        || exe_lower.contains("wezterm")
    {
        return true;
    }

    if title_lower.contains("putty")
        || title_lower.contains("terminal")
        || title_lower.contains("invite de commandes")
        || title_lower.contains("powershell")
        || title_lower.contains("bash")
    {
        return true;
    }

    unsafe {
        let mut class_buf = [0u16; 256];
        let len = GetClassNameW(hwnd, &mut class_buf);
        if len > 0 {
            let class_name = String::from_utf16_lossy(&class_buf[..len as usize]).to_lowercase();
            if class_name.contains("putty")
                || class_name.contains("consolewindowclass")
                || class_name.contains("mintty")
                || class_name.contains("virtualconsoleclass")
            {
                return true;
            }
        }
    }
    false
}

#[cfg(windows)]
pub fn is_windows_terminal(hwnd: HWND) -> bool {
    let (_pid, exe) = get_window_process_info(hwnd);
    let exe_lower = exe.to_lowercase();
    if exe_lower.contains("windowsterminal") || exe_lower == "wt.exe" {
        return true;
    }
    unsafe {
        let mut class_buf = [0u16; 256];
        let len = GetClassNameW(hwnd, &mut class_buf);
        if len > 0 {
            let class_name = String::from_utf16_lossy(&class_buf[..len as usize]).to_lowercase();
            if class_name.contains("cascadia") {
                return true;
            }
        }
    }
    false
}

#[cfg(not(windows))]
pub fn is_windows_terminal(_hwnd: ()) -> bool {
    false
}

#[cfg(windows)]
pub fn is_putty_window(hwnd: HWND) -> bool {
    unsafe {
        let mut class_buf = [0u16; 256];
        let len = GetClassNameW(hwnd, &mut class_buf);
        if len > 0 {
            let class_name = String::from_utf16_lossy(&class_buf[..len as usize]).to_lowercase();
            if class_name.contains("putty") {
                return true;
            }
        }
    }
    let (_pid, exe) = get_window_process_info(hwnd);
    let exe_lower = exe.to_lowercase();
    exe_lower.contains("putty") || exe_lower.contains("kitty")
}

#[cfg(not(windows))]
pub fn is_putty_window(_hwnd: ()) -> bool {
    false
}

/// Extrait le nom de fichier actif depuis le titre d'une fenêtre d'éditeur (ex: "projet – main.rs" -> "main.rs").
pub fn extract_filename_from_title(title: &str) -> Option<String> {
    const KNOWN_EXTS: &[&str] = &[
        "rs", "toml", "json", "md", "txt", "js", "ts", "tsx", "jsx", "py", "java", "c", "cpp",
        "h", "hpp", "cs", "go", "html", "css", "scss", "yaml", "yml", "sh", "bat", "cmd", "ps1",
        "sql", "env", "lock", "properties", "gradle", "xml", "kt", "kts", "dart", "php", "rb", "lua",
    ];

    let segments: Vec<&str> = title.split(|c: char| c == '–' || c == '—' || c == '-' || c == '|' || c == ':').collect();

    for seg in segments.iter().rev() {
        let clean = seg.trim().trim_start_matches(['●', '*', '•']).trim();
        let candidate_path = Path::new(clean);
        if let Some(ext) = candidate_path.extension().and_then(|e| e.to_str()) {
            if KNOWN_EXTS.iter().any(|k| ext.eq_ignore_ascii_case(k)) {
                let fname = candidate_path.file_name().and_then(|n| n.to_str()).unwrap_or(clean);
                return Some(fname.to_string());
            }
        }

        for word in clean.split_whitespace() {
            let w_clean = word.trim_matches(|c: char| c == '[' || c == ']' || c == '(' || c == ')' || c == '"' || c == '\'');
            let p = Path::new(w_clean);
            if let Some(ext) = p.extension().and_then(|e| e.to_str()) {
                if KNOWN_EXTS.iter().any(|k| ext.eq_ignore_ascii_case(k)) {
                    let fname = p.file_name().and_then(|n| n.to_str()).unwrap_or(w_clean);
                    return Some(fname.to_string());
                }
            }
        }
    }
    None
}

/// Vérifie si l'exécutable correspond à un véritable navigateur Web
pub fn is_browser_executable(exe: &str) -> bool {
    let lower = exe.to_lowercase();
    let name = lower.strip_suffix(".exe").unwrap_or(&lower);
    matches!(
        name,
        "chrome"
            | "google-chrome"
            | "google-chrome-stable"
            | "msedge"
            | "firefox"
            | "brave"
            | "brave-browser"
            | "opera"
            | "opera_gx"
            | "vivaldi"
            | "arc"
            | "chromium"
            | "waterfox"
            | "librewolf"
            | "zen"
            | "thorium"
            | "floorp"
            | "ladybird"
    )
}

#[cfg(windows)]
pub fn is_editor_window(_hwnd: HWND, title: &str, exe_name: &str) -> bool {
    if is_browser_executable(exe_name) {
        return false;
    }
    let exe_lower = exe_name.to_lowercase();
    let title_lower = title.to_lowercase();

    exe_lower.contains("idea")
        || exe_lower.contains("rustrover")
        || exe_lower.contains("clion")
        || exe_lower.contains("pycharm")
        || exe_lower.contains("rider")
        || exe_lower.contains("webstorm")
        || exe_lower.contains("goland")
        || exe_lower.contains("phpstorm")
        || exe_lower.contains("rubymine")
        || exe_lower.contains("code")
        || exe_lower.contains("vscodium")
        || exe_lower.contains("cursor")
        || exe_lower.contains("sublime")
        || exe_lower.contains("zed")
        || exe_lower.contains("notepad")
        || exe_lower.contains("devenv")
        || title_lower.contains("intellij")
        || title_lower.contains("rustrover")
        || title_lower.contains("visual studio")
        || title_lower.contains("sublime text")
        || title_lower.contains("bloc-notes")
        || title_lower.contains("notepad")
        || extract_filename_from_title(title).is_some()
}

#[cfg(not(windows))]
pub fn is_editor_window(_hwnd: (), title: &str, exe_name: &str) -> bool {
    let exe_lower = exe_name.to_lowercase();
    let title_lower = title.to_lowercase();
    exe_lower.contains("code") || title_lower.contains("code") || extract_filename_from_title(title).is_some()
}

#[cfg(not(windows))]
pub fn is_terminal_window(_id: &str, title: &str) -> bool {
    let t = title.to_lowercase();
    t.contains("terminal")
        || t.contains("konsole")
        || t.contains("alacritty")
        || t.contains("kitty")
        || t.contains("wezterm")
        || t.contains("xterm")
        || t.contains("bash")
        || t.contains("zsh")
}

#[cfg(not(windows))]
pub fn is_browser_window(title: &str) -> bool {
    let t = title.to_lowercase();
    t.contains("firefox")
        || t.contains("chrome")
        || t.contains("chromium")
        || t.contains("brave")
        || t.contains("edge")
        || t.contains("opera")
        || t.contains("vivaldi")
}

#[cfg(not(windows))]
pub fn is_window_hung(_id: &str) -> bool {
    false
}

#[cfg(windows)]
pub fn is_toast_notification_window(hwnd: HWND, class_name: &str, title: &str) -> bool {
    let c_lower = class_name.to_lowercase();
    let t_lower = title.to_lowercase();

    if t_lower.contains("notification") || t_lower.contains("toast") {
        return true;
    }

    if c_lower.contains("toast") {
        return true;
    }

    if c_lower == "windows.ui.core.corewindow"
        || c_lower.contains("xamlexplorerhostislandwindow")
        || c_lower.contains("toplevelwindowforoverflowxamlisland")
    {
        let (_pid, exe) = get_window_process_info(hwnd);
        let exe_lower = exe.to_lowercase();
        if exe_lower.contains("shellexperiencehost") || exe_lower.contains("explorer") {
            let mut r = RECT::default();
            if unsafe { GetWindowRect(hwnd, &mut r).is_ok() } {
                let w = r.right - r.left;
                let h = r.bottom - r.top;
                let wa = get_desktop_work_area();
                // Toasts situés dans le coin inférieur droit de l'écran
                if w >= 150 && w <= 700 && h >= 40 && h <= 600 {
                    if r.right >= wa.right - 100 && r.bottom >= wa.bottom - 650 && r.left >= wa.left + (wa.right - wa.left) / 3 {
                        return true;
                    }
                }
            }
        }
    }
    false
}

#[cfg(not(windows))]
pub fn is_toast_notification_window(_hwnd: (), _class_name: &str, _title: &str) -> bool {
    false
}

#[cfg(windows)]
unsafe extern "system" fn enum_windows_callback(hwnd: HWND, lparam: LPARAM) -> BOOL {
    let list = &mut *(lparam.0 as *mut Vec<(HWND, String)>);
    if IsWindowVisible(hwnd).as_bool() {
        let length = GetWindowTextLengthW(hwnd);
        let title = if length > 0 {
            let mut buf = vec![0u16; (length + 1) as usize];
            let copied = GetWindowTextW(hwnd, &mut buf);
            if copied > 0 {
                String::from_utf16_lossy(&buf[..copied as usize])
            } else {
                String::new()
            }
        } else {
            String::new()
        };

        let mut class_buf = [0u16; 256];
        let class_len = GetClassNameW(hwnd, &mut class_buf);
        let class_name = if class_len > 0 {
            String::from_utf16_lossy(&class_buf[..class_len as usize])
        } else {
            String::new()
        };

        if is_toast_notification_window(hwnd, &class_name, &title) {
            let display_title = if title.trim().is_empty() {
                "Notification Windows".to_string()
            } else {
                title
            };
            list.push((hwnd, display_title));
        } else if !title.trim().is_empty() {
            list.push((hwnd, title));
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
        })
        .collect()
}

#[cfg(not(windows))]
pub fn list_user_windows() -> Vec<(String, String)> {
    // Utilisation de wmctrl pour lister les fenêtres X11 gérées par le Window Manager EWMH
    let output = std::process::Command::new("wmctrl")
        .args(["-l"])
        .output();

    let mut list = Vec::new();
    if let Ok(out) = output {
        let text = String::from_utf8_lossy(&out.stdout);
        for line in text.lines() {
            let parts: Vec<&str> = line.split_whitespace().collect();
            if parts.len() >= 4 {
                let win_id = parts[0].to_string();
                let title = parts[3..].join(" ");
                let t_clean = title.trim();
                if !t_clean.is_empty()
                    && !t_clean.contains("Libertide")
                    && !t_clean.eq_ignore_ascii_case("desktop")
                {
                    list.push((win_id, t_clean.to_string()));
                }
            }
        }
    }
    list
}

#[cfg(not(windows))]
pub fn get_active_x11_window() -> Option<String> {
    let output = std::process::Command::new("xdotool")
        .arg("getactivewindow")
        .output()
        .ok()?;
    let id = String::from_utf8_lossy(&output.stdout).trim().to_string();
    if id.is_empty() {
        None
    } else {
        Some(id)
    }
}

#[cfg(not(windows))]
pub fn get_x11_window_properties(win_id: &str) -> (String, String) {
    // Récupère WM_CLASS et _NET_WM_PID via xprop
    let output = std::process::Command::new("xprop")
        .args(["-id", win_id, "WM_CLASS", "_NET_WM_PID"])
        .output();

    let mut class_name = String::new();
    let mut pid_str = String::new();

    if let Ok(out) = output {
        let text = String::from_utf8_lossy(&out.stdout);
        for line in text.lines() {
            if line.contains("WM_CLASS") {
                if let Some((_, val)) = line.split_once('=') {
                    class_name = val.replace('"', "").trim().to_string();
                }
            } else if line.contains("_NET_WM_PID") {
                if let Some((_, val)) = line.split_once('=') {
                    pid_str = val.trim().to_string();
                }
            }
        }
    }
    (class_name, pid_str)
}

#[cfg(not(windows))]
pub fn get_active_or_best_window(user_windows: &[(String, String)]) -> Option<String> {
    if let Some(active) = get_active_x11_window() {
        // Normaliser l'ID décimal / hexadécimal si nécessaire
        if let Ok(dec_id) = active.parse::<u64>() {
            let hex_id = format!("0x{:08x}", dec_id);
            if let Some((id, _)) = user_windows.iter().find(|(w_id, _)| w_id.eq_ignore_ascii_case(&hex_id) || w_id.eq_ignore_ascii_case(&active)) {
                return Some(id.clone());
            }
        }
        return Some(active);
    }
    user_windows.first().map(|(id, _)| id.clone())
}

#[cfg(not(windows))]
pub fn find_windows_matching(
    keyword: &str,
    user_windows: &[(String, String)],
    _preferred_id: Option<&str>,
) -> Vec<String> {
    let kw = keyword.trim().to_lowercase();
    user_windows
        .iter()
        .filter(|(_, title)| kw.is_empty() || title.to_lowercase().contains(&kw))
        .map(|(id, _)| id.clone())
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
    let current_pid = std::process::id();
    let (fg_pid, _) = get_window_process_info(fg);
    if !fg.0.is_null()
        && fg != overlay_hwnd
        && fg_pid != current_pid
        && !is_window_hung(fg)
        && user_windows.iter().any(|(h, _)| *h == fg)
    {
        Some(fg)
    } else {
        crate::automation::LAST_EXTERNAL_HWND
            .lock()
            .ok()
            .and_then(|g| *g)
            .map(|raw| HWND(raw as _))
            .filter(|&h| user_windows.iter().any(|(w, _)| *w == h) && !is_window_hung(h))
            .or_else(|| {
                user_windows
                    .iter()
                    .find(|(h, t)| is_browser_hwnd(*h, t))
                    .map(|(h, _)| *h)
            })
            .or_else(|| find_largest_and_most_centered_window(user_windows))
    }
}

#[cfg(windows)]
pub fn find_best_browser_window(
    user_windows: &[(HWND, String)],
    preferred_hwnd: Option<HWND>,
) -> Option<HWND> {
    if let Some(pref) = preferred_hwnd {
        if let Some((_, title)) = user_windows.iter().find(|(h, _)| *h == pref) {
            if is_browser_hwnd(pref, title) && !is_window_hung(pref) {
                return Some(pref);
            }
        }
    }

    if let Some(last_ext) = crate::automation::LAST_EXTERNAL_HWND
        .lock()
        .ok()
        .and_then(|g| *g)
        .map(|raw| HWND(raw as _))
    {
        if Some(last_ext) != preferred_hwnd {
            if let Some((_, title)) = user_windows.iter().find(|(h, _)| *h == last_ext) {
                if is_browser_hwnd(last_ext, title) && !is_window_hung(last_ext) {
                    return Some(last_ext);
                }
            }
        }
    }

    user_windows
        .iter()
        .find(|(h, t)| is_browser_hwnd(*h, t) && !is_window_hung(*h))
        .map(|(h, _)| *h)
}

#[cfg(not(windows))]
pub fn find_best_browser_window(
    user_windows: &[(String, String)],
    preferred_id: Option<&str>,
) -> Option<String> {
    if let Some(pref) = preferred_id {
        if let Some((id, title)) = user_windows.iter().find(|(id, _)| id == pref) {
            if is_browser_window(title) {
                return Some(id.clone());
            }
        }
    }
    user_windows
        .iter()
        .find(|(_, title)| is_browser_window(title))
        .map(|(id, _)| id.clone())
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

        if p_lower == "paramètres" || p_lower == "parametres" || p_lower == "settings" || p_lower == "reglages" || p_lower == "réglages" {
            let (_pid, exe) = get_window_process_info(hwnd);
            let exe_lower = exe.to_lowercase();
            if exe_lower.contains("systemsettings") || exe_lower.contains("settings") {
                return true;
            }
            if t_lower == "paramètres" || t_lower == "settings" || t_lower.starts_with("paramètres") || t_lower.starts_with("settings") {
                return true;
            }
        }

        let is_generic_browser = p_lower == "browser" || p_lower == "navigateur" || p_lower == "web" || p_lower == "internet";
        if is_generic_browser {
            if is_browser_hwnd(hwnd, title) {
                return true;
            }
            continue;
        }

        if p_lower == "google" || p_lower == "chrome" || p_lower == "google chrome" {
            let (_pid, exe) = get_window_process_info(hwnd);
            if exe.to_lowercase().contains("chrome") {
                return true;
            }
            if is_browser_hwnd(hwnd, title) && (t_lower.contains("chrome") || t_lower.contains("google")) {
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
    let (_pid, exe_name) = get_window_process_info(hwnd);
    if is_browser_executable(&exe_name) {
        return true;
    }

    // Exclusion explicite des applications Electron / non-navigateurs
    let exe_lower = exe_name.to_lowercase();
    if is_editor_window(hwnd, title, &exe_name)
        || is_terminal_window(hwnd, title)
        || exe_lower.contains("discord")
        || exe_lower.contains("slack")
        || exe_lower.contains("spotify")
        || exe_lower.contains("steam")
        || exe_lower.contains("obsidian")
        || exe_lower.contains("notion")
        || exe_lower.contains("postman")
        || exe_lower.contains("teams")
        || exe_lower.contains("thunderbird")
        || exe_lower.contains("explorer")
    {
        return false;
    }

    let t = title.to_lowercase();
    if t.ends_with(" - google chrome")
        || t.ends_with(" - microsoft edge")
        || t.ends_with(" - mozilla firefox")
        || t.ends_with(" - brave")
        || t.ends_with(" - opera")
        || t.ends_with(" - vivaldi")
        || t.ends_with(" - arc")
        || t.ends_with(" - waterfox")
        || t.ends_with(" - librewolf")
        || t.ends_with(" - zen browser")
    {
        return true;
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

#[cfg(not(windows))]
pub fn apply_window_rect(win_id: &str, x: i32, y: i32, width: i32, height: i32) {
    // wmctrl -i -r <WIN_ID> -e 0,X,Y,W,H
    let geom = format!("0,{},{},{},{}", x.max(0), y.max(0), width.max(100), height.max(100));
    let _ = std::process::Command::new("wmctrl")
        .args(["-i", "-r", win_id, "-e", &geom])
        .status();
}

#[cfg(not(windows))]
pub fn launch_browser_new_window(url: &str) -> Option<String> {
    let browsers = ["google-chrome", "chromium", "firefox", "brave-browser", "xdg-open"];
    for b in &browsers {
        if std::process::Command::new(b)
            .arg(url)
            .spawn()
            .is_ok()
        {
            std::thread::sleep(std::time::Duration::from_millis(300));
            return get_active_x11_window();
        }
    }
    None
}