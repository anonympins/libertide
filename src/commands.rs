use crate::Ordering;
use std::time::Duration;

#[cfg(windows)]
use std::os::windows::process::CommandExt;
#[cfg(windows)]
use windows::core::{w, Interface};
#[cfg(windows)]
use windows::Win32::Foundation::{HWND, LPARAM, RECT, WPARAM};
#[cfg(windows)]
use windows::Win32::UI::Input::KeyboardAndMouse::{
    mouse_event, MOUSEEVENTF_WHEEL, VIRTUAL_KEY,
};
#[cfg(windows)]
use windows::Win32::UI::WindowsAndMessaging::{
    FindWindowW, GetForegroundWindow, GetWindowRect, IsIconic, PostMessageW,
    SetCursorPos, ShowWindow, SW_MAXIMIZE, SW_MINIMIZE, WM_CLOSE,
};

use crate::ui::*;
use crate::automation::*;
use crate::autostart::*;
use crate::fs_util::*;
use crate::scheduler::*;
use crate::types::{AgentAction, QuickSuggestionItem};
use crate::window_util::*;

#[cfg(windows)]
const CREATE_NEW_CONSOLE: u32 = 0x00000010;

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
                restore_if_iconic(hwnd);
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
                        restore_if_iconic(*hwnd);
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
            AgentAction::Undo => {
                if let Some(msg) = undo_last_system_action() {
                    feedback.push(msg);
                } else {
                    feedback.push("Aucune action à annuler dans l'historique.".to_string());
                }
            }
            AgentAction::Redo => {
                if let Some(msg) = redo_last_system_action() {
                    feedback.push(msg);
                } else {
                    feedback.push("Aucune action à rétablir dans l'historique.".to_string());
                }
            }
            AgentAction::NavigateBack => {
                let target_hwnd = find_best_browser_window(&user_windows, active_user_hwnd);
                if let Some(hwnd) = target_hwnd {
                    force_foreground_window(hwnd);
                    std::thread::sleep(Duration::from_millis(40));
                }
                const VK_MENU: VIRTUAL_KEY = VIRTUAL_KEY(0x12);
                const VK_LEFT: VIRTUAL_KEY = VIRTUAL_KEY(0x25);
                send_hotkey(&[VK_MENU], VK_LEFT);
                if let Ok(mut hist) = ACTION_HISTORY.lock() {
                    hist.push_undo(UndoOperation::NavigateForward {
                        target_hwnd: target_hwnd.map(|h| h.0 as isize),
                    });
                }
                feedback.push("Navigation arrière (page précédente) effectuée.".to_string());
            }
            AgentAction::NavigateForward => {
                let target_hwnd = find_best_browser_window(&user_windows, active_user_hwnd);
                if let Some(hwnd) = target_hwnd {
                    force_foreground_window(hwnd);
                    std::thread::sleep(Duration::from_millis(40));
                }
                const VK_MENU: VIRTUAL_KEY = VIRTUAL_KEY(0x12);
                const VK_RIGHT: VIRTUAL_KEY = VIRTUAL_KEY(0x27);
                send_hotkey(&[VK_MENU], VK_RIGHT);
                if let Ok(mut hist) = ACTION_HISTORY.lock() {
                    hist.push_undo(UndoOperation::NavigateBack {
                        target_hwnd: target_hwnd.map(|h| h.0 as isize),
                    });
                }
                feedback.push("Navigation avant (page suivante) effectuée.".to_string());
            }
            AgentAction::OpenSearch => {
                const VK_LWIN: VIRTUAL_KEY = VIRTUAL_KEY(0x5B);
                const VK_S: VIRTUAL_KEY = VIRTUAL_KEY(0x53);
                send_hotkey(&[VK_LWIN], VK_S);
                println!("[Actions] Recherche Windows locale ouverte via Win+S.");
                feedback.push("Recherche Windows locale ouverte (Win+S).".to_string());
            }
            AgentAction::OpenSearchBrowser => {
                let target_hwnd = find_best_browser_window(&user_windows, active_user_hwnd);
                if let Some(hwnd) = target_hwnd {
                    if is_window_hung(hwnd) {
                        feedback.push(format!("Navigateur [HWND {:?}] non réactif. Focus barre d'adresse ignoré.", hwnd.0));
                        continue;
                    }
                    force_foreground_window(hwnd);
                    restore_if_iconic(hwnd);
                    std::thread::sleep(Duration::from_millis(60));
                    focus_browser_address_bar(hwnd);
                    active_user_hwnd = Some(hwnd);
                    preferred_target_hwnd = Some(hwnd);
                    println!("[Actions] Recherche navigateur / omnibox focalisée sur [HWND {:?}].", hwnd.0);
                    feedback.push("Barre de recherche web du navigateur sélectionnée.".to_string());
                } else {
                    println!("[Actions] Aucun navigateur ouvert pour open_search_browser, lancement du navigateur.");
                    let spawned = launch_browser_new_window("https://www.google.com");
                    if let Some(h) = spawned {
                        newly_spawned_hwnd = Some(h);
                        active_user_hwnd = Some(h);
                        preferred_target_hwnd = Some(h);
                        user_windows = list_user_windows();
                    }
                    feedback.push("Navigateur ouvert pour recherche web.".to_string());
                }
            }
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
                        restore_if_iconic(hwnd);
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
                    restore_if_iconic(hwnd);
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
                    force_foreground_window(hwnd);
                    restore_if_iconic(hwnd);
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
                    force_foreground_window(hwnd);
                    restore_if_iconic(hwnd);
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
                        if let Ok(mut hist) = ACTION_HISTORY.lock() {
                            hist.push_undo(UndoOperation::ScrollOpposite {
                                direction: dir_clean.clone(),
                                amount: steps,
                                target_hwnd: Some(hwnd.0 as isize),
                            });
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
                    force_foreground_window(hwnd);
                    restore_if_iconic(hwnd);
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
                    force_foreground_window(hwnd);
                    restore_if_iconic(hwnd);
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
                        restore_if_iconic(hwnd);
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
                            restore_if_iconic(*new_hwnd);
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
                            restore_if_iconic(hwnd);
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
                            restore_if_iconic(hwnd);
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
                    } else if let Some(target_desc) = target_desc_opt {
                        if let Some(hwnd) = target_hwnd {
                            if is_window_hung(hwnd) {
                                feedback.push(format!("Fenêtre [HWND {:?}] non réactive. Écriture ignorée.", hwnd.0));
                                continue;
                            }
                            force_foreground_window(hwnd);
                            restore_if_iconic(hwnd);
                            std::thread::sleep(Duration::from_millis(70));

                            let elements = list_interactive_elements(hwnd);
                            if let Some(elem) = find_best_input_element(&elements, target_desc) {
                                println!("[Actions] Champ interactable validé : name='{}', id='{}', type='{}'", elem.name, elem.automation_id, elem.localized_type);
                                unsafe { let _ = elem.element.SetFocus(); }
                                click_element(elem.click_x, elem.click_y, Some(&elem.element), elem.pattern.as_ref());
                                std::thread::sleep(Duration::from_millis(50));

                                send_hotkey(&[VK_CONTROL], VK_A);
                                std::thread::sleep(Duration::from_millis(30));

                                set_clipboard_text(text);
                                std::thread::sleep(Duration::from_millis(30));
                                send_paste();
                                if should_enter {
                                    std::thread::sleep(Duration::from_millis(60));
                                    send_hotkey(&[], VK_RETURN);
                                }
                                feedback.push(format!("Texte inséré dans le champ validé '{}'.", elem.name));
                                handled = true;
                            } else {
                                println!("[Actions] Rejet : aucun champ interactable correspondant à '{}'", target_desc);
                                feedback.push(format!("Ciblage rejeté : le champ interactable '{}' est introuvable parmi les éléments réels de l'écran. Aucune cible arbitraire n'a été sélectionnée.", target_desc));
                                handled = true;
                            }
                        }
                    } else if let Some(hwnd) = target_hwnd.or(active_user_hwnd).or(preferred_target_hwnd) {
                        if is_window_hung(hwnd) {
                            feedback.push(format!("Fenêtre [HWND {:?}] non réactive. Écriture ignorée.", hwnd.0));
                            continue;
                        }
                        let win_title = user_windows.iter().find(|(wh, _)| *wh == hwnd).map(|(_, t)| t.as_str()).unwrap_or("");
                        let is_browser = is_browser_hwnd(hwnd, win_title);

                        force_foreground_window(hwnd);
                        restore_if_iconic(hwnd);
                        std::thread::sleep(Duration::from_millis(60));

                        // Si c'est un navigateur et qu'aucun champ texte web n'a le focus, on cible l'omnibar
                        if is_browser && !has_focused_textarea(hwnd) {
                            navigate_browser_address_bar(hwnd, text);
                            feedback.push(format!("Texte saisi dans la barre d'adresse du navigateur : '{}'.", text));
                            handled = true;
                        } else {
                            // Saisie directe dans le champ déjà focalisé au premier plan
                            restore_external_focus_target(Some(hwnd));
                            release_modifier_keys();
                            std::thread::sleep(Duration::from_millis(40));

                            set_clipboard_text(text);
                            std::thread::sleep(Duration::from_millis(30));
                            send_paste();
                            if should_enter {
                                std::thread::sleep(Duration::from_millis(60));
                                send_hotkey(&[], VK_RETURN);
                            }
                            feedback.push(format!("Texte inséré dans le champ actif au premier plan ({} caractères).", text.len()));
                            handled = true;
                        }
                    } else {
                        let _ = write_to_browser_or_txt(text);
                        feedback.push(format!("Texte écrit via fichier temporaire ou repli système ({} caractères).", text.len()));
                        handled = true;
                    }
                }

                if !handled {
                    if let Ok(mut hist) = ACTION_HISTORY.lock() {
                        hist.push_undo(UndoOperation::RevertText {
                            target_hwnd: target_hwnd.or(active_user_hwnd).or(preferred_target_hwnd).map(|h| h.0 as isize),
                        });
                    }
                    feedback.push("Action non exécutée : champ cible non validé.".to_string());
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
                            restore_if_iconic(hwnd);
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
                            restore_if_iconic(hwnd);
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
                            restore_if_iconic(hwnd);
                            std::thread::sleep(Duration::from_millis(80));
                            unsafe { let _ = elem.element.SetFocus(); }
                            click_element(elem.click_x, elem.click_y, Some(&elem.element), elem.pattern.as_ref());
                            std::thread::sleep(Duration::from_millis(50));
                            const VK_CONTROL: VIRTUAL_KEY = VIRTUAL_KEY(0x11);
                            const VK_A: VIRTUAL_KEY = VIRTUAL_KEY(0x41);
                            send_hotkey(&[VK_CONTROL], VK_A);
                            std::thread::sleep(Duration::from_millis(30));
                            set_clipboard_text(text);
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
                        restore_if_iconic(hwnd);
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
                            set_clipboard_text(text);
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
                if let Ok(mut hist) = ACTION_HISTORY.lock() {
                    hist.push_undo(UndoOperation::RevertText {
                        target_hwnd: target_hwnd.or(active_user_hwnd).or(preferred_target_hwnd).map(|h| h.0 as isize),
                    });
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
                    if let Ok(mut hist) = ACTION_HISTORY.lock() {
                        hist.push_undo(UndoOperation::CloseTab {
                            target_hwnd: active_user_hwnd.map(|h| h.0 as isize),
                        });
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
                            restore_if_iconic(hwnd);
                            navigate_browser_address_bar(hwnd, &formatted_url);
                            active_user_hwnd = Some(hwnd);
                            preferred_target_hwnd = Some(hwnd);
                            if let Ok(mut hist) = ACTION_HISTORY.lock() {
                                hist.push_undo(UndoOperation::NavigateBack {
                                    target_hwnd: Some(hwnd.0 as isize),
                                });
                            }
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
                        if let Ok(mut hist) = ACTION_HISTORY.lock() {
                            hist.push_undo(UndoOperation::CloseTab {
                                target_hwnd: active_user_hwnd.map(|h| h.0 as isize),
                            });
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
                            if let Ok(mut hist) = ACTION_HISTORY.lock() {
                                hist.push_undo(UndoOperation::CloseTab {
                                    target_hwnd: Some(hwnd.0 as isize),
                                });
                            }
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
                        if let Ok(mut hist) = ACTION_HISTORY.lock() {
                            hist.push_undo(UndoOperation::CloseTab {
                                target_hwnd: active_user_hwnd.map(|h| h.0 as isize),
                            });
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
                const VK_A: VIRTUAL_KEY = VIRTUAL_KEY(0x41);

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
                    "browser_back" | "navigate_back" | "back" => {
                        const VK_LEFT: VIRTUAL_KEY = VIRTUAL_KEY(0x25);
                        send_hotkey(&[VK_MENU], VK_LEFT);
                    }
                    "browser_forward" | "navigate_forward" | "forward" => {
                        const VK_RIGHT: VIRTUAL_KEY = VIRTUAL_KEY(0x27);
                        send_hotkey(&[VK_MENU], VK_RIGHT);
                    }
                    "mute_mic" => send_hotkey(&[VK_LWIN, VK_MENU], VIRTUAL_KEY(0x4B)),
                    "toggle_desktop" => send_hotkey(&[VK_LWIN], VIRTUAL_KEY(0x44)),
                    "snap_layouts" => send_hotkey(&[VK_LWIN], VIRTUAL_KEY(0x5A)),
                    "task_manager" => send_hotkey(&[VK_CONTROL, VK_SHIFT], VK_ESCAPE),
                    "snip_screenshot" => send_hotkey(&[VK_LWIN, VK_SHIFT], VIRTUAL_KEY(0x53)),
                    "action_center" => send_hotkey(&[VK_LWIN], VIRTUAL_KEY(0x41)),
                    "notification_center" => send_hotkey(&[VK_LWIN], VIRTUAL_KEY(0x4E)),
                    "task_view" => send_hotkey(&[VK_LWIN], VK_TAB),
                    "open_search" => send_hotkey(&[VK_LWIN], VIRTUAL_KEY(0x53)),
                    "open_search_browser" | "browser_search" | "search_browser" => {
                        let target_hwnd = find_best_browser_window(&user_windows, active_user_hwnd);
                        if let Some(hwnd) = target_hwnd {
                            force_foreground_window(hwnd);
                            unsafe {
                                let _ = ShowWindow(hwnd, SW_MAXIMIZE);
                            }
                            std::thread::sleep(Duration::from_millis(50));
                            focus_browser_address_bar(hwnd);
                        } else {
                            let spawned = launch_browser_new_window("https://www.google.com");
                            if let Some(h) = spawned {
                                active_user_hwnd = Some(h);
                                preferred_target_hwnd = Some(h);
                                user_windows = list_user_windows();
                            }
                        }
                    }
                    "open_run" => send_hotkey(&[VK_LWIN], VIRTUAL_KEY(0x52)),
                    "open_settings" => send_hotkey(&[VK_LWIN], VIRTUAL_KEY(0x49)),
                    "sound_settings" | "audio_settings" | "volume_mixer" => {
                        // Win + A ouvre le panneau latéral rapide avec curseur de volume et sélection du périphérique audio
                        send_hotkey(&[VK_LWIN], VK_A);
                    }
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
                    "focus_address_bar" | "address_bar" | "select_address_bar" | "omnibox" | "omnibar" => {
                        let user_windows = list_user_windows();
                        if let Some(hwnd) = find_best_browser_window(&user_windows, active_user_hwnd) {
                            focus_browser_address_bar(hwnd);
                        } else {
                            send_hotkey(&[VK_MENU], VK_D);
                            std::thread::sleep(Duration::from_millis(40));
                            send_hotkey(&[VK_CONTROL], VK_L);
                        }
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
                    restore_if_iconic(hwnd);
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
                            set_clipboard_text(target_name);
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
                    if let Some(content) = get_plain_clipboard_text() {
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
                    restore_if_iconic(hwnd);
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
                if let Ok(mut hist) = ACTION_HISTORY.lock() {
                    hist.push_undo(UndoOperation::DeactivateDictation);
                }
                if let Some(w) = window {
                    let targets = find_windows_matching(w, &user_windows, active_user_hwnd);
                    if let Some(&h) = targets.first() {
                        force_foreground_window(h);
                        restore_if_iconic(h);
                    }
                }
                feedback.push("Mode dictée vocale activé.".to_string());
            }
            AgentAction::DeactivateDictation => {
                IS_DICTATION_ACTIVE.store(false, Ordering::SeqCst);
                if let Ok(mut hist) = ACTION_HISTORY.lock() {
                    hist.push_undo(UndoOperation::ActivateDictation);
                }
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
                            restore_if_iconic(h);
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
                    restore_if_iconic(hwnd);
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
                        let _ = windows::Win32::System::Com::CoInitializeEx(None, windows::Win32::System::Com::COINIT_MULTITHREADED);
                        if let Ok(uia) = windows::Win32::System::Com::CoCreateInstance::<_, windows::Win32::UI::Accessibility::IUIAutomation>(&windows::Win32::UI::Accessibility::CUIAutomation, None, windows::Win32::System::Com::CLSCTX_INPROC_SERVER) {
                            if let Ok(focused) = uia.GetFocusedElement() {
                                if let Ok(p_unk) = focused.GetCurrentPattern(windows::Win32::UI::Accessibility::UIA_TextPatternId) {
                                    if let Ok(text_pattern) = p_unk.cast::<windows::Win32::UI::Accessibility::IUIAutomationTextPattern>() {
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
                        set_clipboard_text(from);
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
                if let Ok(mut hist) = ACTION_HISTORY.lock() {
                    hist.push_undo(UndoOperation::UnmuteAudio);
                }
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
            AgentAction::ShowFloatingCard { id, title, .. } => {
                if let Some(ref cid) = id {
                    if let Ok(mut hist) = ACTION_HISTORY.lock() {
                        hist.push_undo(UndoOperation::CloseFloatingCard(cid.clone()));
                    }
                }
                feedback.push(format!("Fenêtre volante '{title}' affichée à l'écran."));
            }
            AgentAction::CloseFloatingCard { id } => {
                let was_just_shown = actions.iter().any(|a| matches!(a, AgentAction::ShowFloatingCard { id: Some(sid), .. } if sid == id));
                if was_just_shown {
                    println!("[Actions] Annulation de close_floating_card('{id}') car la carte a été mise à jour dans la même passe.");
                    continue;
                }
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

#[cfg(not(windows))]
pub fn execute_system_actions(actions: &[AgentAction], _request_id: u64) -> String {
    let mut feedback = Vec::new();
    for action in actions {
        match action {
            AgentAction::Undo => {
                if let Some(msg) = undo_last_system_action() {
                    feedback.push(msg);
                } else {
                    feedback.push("Aucune action à annuler dans l'historique.".to_string());
                }
            }
            AgentAction::Redo => {
                if let Some(msg) = redo_last_system_action() {
                    feedback.push(msg);
                } else {
                    feedback.push("Aucune action à rétablir dans l'historique.".to_string());
                }
            }
            AgentAction::OpenApp { name } => {
                let _ = std::process::Command::new(name).spawn();
                feedback.push(format!("Application '{}' lancée en arrière-plan.", name));
            }
            AgentAction::NavigateBack => {
                #[cfg(target_os = "macos")]
                let _ = std::process::Command::new("osascript").args(["-e", "tell application \"System Events\" to keystroke \"[\" using command down"]).status();
                #[cfg(not(target_os = "macos"))]
                let _ = std::process::Command::new("xdotool").args(["key", "alt+Left"]).status();
                feedback.push("Navigation arrière (page précédente) effectuée.".to_string());
            }
            AgentAction::NavigateForward => {
                #[cfg(target_os = "macos")]
                let _ = std::process::Command::new("osascript").args(["-e", "tell application \"System Events\" to keystroke \"]\" using command down"]).status();
                #[cfg(not(target_os = "macos"))]
                let _ = std::process::Command::new("xdotool").args(["key", "alt+Right"]).status();
                feedback.push("Navigation avant (page suivante) effectuée.".to_string());
            }
            AgentAction::OpenSearch => {
                let _ = std::process::Command::new("xdotool").args(["key", "Super"]).status();
                feedback.push("Recherche système locale ouverte.".to_string());
            }
            AgentAction::OpenSearchBrowser => {
                #[cfg(target_os = "macos")]
                let _ = std::process::Command::new("osascript")
                    .args(["-e", "tell application \"System Events\" to keystroke \"l\" using command down"])
                    .status();
                #[cfg(not(target_os = "macos"))]
                {
                    let user_windows = list_user_windows();
                    if let Some(win_id) = find_best_browser_window(&user_windows, None) {
                        let _ = std::process::Command::new("wmctrl").args(["-i", "-a", &win_id]).status();
                        std::thread::sleep(Duration::from_millis(50));
                    }
                    let _ = std::process::Command::new("xdotool").args(["key", "ctrl+l"]).status();
                }
                feedback.push("Barre d'adresse / recherche navigateur sélectionnée.".to_string());
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
            AgentAction::ReplaceFieldText { text, .. } => {
                set_clipboard_text(text);
                #[cfg(target_os = "macos")]
                {
                    let _ = std::process::Command::new("osascript")
                        .args(["-e", "tell application \"System Events\" to keystroke \"a\" using command down\ntell application \"System Events\" to keystroke \"v\" using command down"])
                        .status();
                }
                #[cfg(all(not(windows), not(target_os = "macos")))]
                {
                    let _ = std::process::Command::new("xdotool").args(["key", "ctrl+a", "ctrl+v"]).status();
                }
                feedback.push(format!("Texte du champ mis à jour ({} caractères).", text.len()));
            }
            AgentAction::ClearText { .. } => {
                #[cfg(target_os = "macos")]
                {
                    let _ = std::process::Command::new("osascript")
                        .args(["-e", "tell application \"System Events\" to keystroke \"a\" using command down\ntell application \"System Events\" to key code 51"])
                        .status();
                }
                #[cfg(all(not(windows), not(target_os = "macos")))]
                {
                    let _ = std::process::Command::new("xdotool").args(["key", "ctrl+a", "BackSpace"]).status();
                }
                feedback.push("Champ de texte réinitialisé.".to_string());
            }
            AgentAction::Scroll { direction, amount, .. } => {
                let dir = direction.trim().to_lowercase();
                let steps = amount.unwrap_or(1).clamp(1, 10);
                #[cfg(target_os = "macos")]
                {
                    let key_code = if dir == "up" || dir == "haut" { "116" } else { "121" };
                    for _ in 0..steps {
                        let script = format!("tell application \"System Events\" to key code {key_code}");
                        let _ = std::process::Command::new("osascript").args(["-e", &script]).status();
                    }
                }
                #[cfg(all(not(windows), not(target_os = "macos")))]
                {
                    let btn = if dir == "up" || dir == "haut" { "4" } else { "5" };
                    for _ in 0..steps {
                        let _ = std::process::Command::new("xdotool").args(["click", btn]).status();
                    }
                }
                feedback.push(format!("Défilement ({dir}, {steps} pas) effectué."));
            }
            AgentAction::FocusWindow { title, pid } => {
                #[cfg(target_os = "macos")]
                {
                    if let Some(t) = title.as_deref().filter(|s| !s.trim().is_empty()) {
                        let script = format!("tell application \"System Events\" to set frontmost of (first process whose name contains \"{t}\") to true");
                        let _ = std::process::Command::new("osascript").args(["-e", &script]).status();
                        feedback.push(format!("Fenêtre '{t}' passée au premier plan."));
                    }
                }
                #[cfg(all(not(windows), not(target_os = "macos")))]
                {
                    if let Some(t) = title.as_deref().filter(|s| !s.trim().is_empty()) {
                        let _ = std::process::Command::new("wmctrl").args(["-a", t]).status();
                        feedback.push(format!("Fenêtre '{t}' passée au premier plan."));
                    } else if let Some(p) = pid {
                        let _ = std::process::Command::new("wmctrl").args(["-p", "-a", &p.to_string()]).status();
                        feedback.push(format!("Fenêtre PID {} passée au premier plan.", p));
                    }
                }
            }
            AgentAction::KillProcess { pid, name } => {
                if let Some(p) = pid {
                    let _ = std::process::Command::new("kill").arg("-9").arg(p.to_string()).status();
                    feedback.push(format!("Processus PID {} arrêté.", p));
                } else if let Some(n) = name.as_deref().filter(|s| !s.trim().is_empty()) {
                    let _ = std::process::Command::new("pkill").arg("-9").arg(n).status();
                    feedback.push(format!("Processus '{}' arrêté.", n));
                }
            }
            AgentAction::CursorMove { command } => {
                let cmd_lower = command.to_lowercase();
                match cmd_lower.as_str() {
                    "newline" | "a_la_ligne" | "enter" => {
                        #[cfg(target_os = "macos")]
                        let _ = std::process::Command::new("osascript").args(["-e", "tell application \"System Events\" to key code 36"]).status();
                        #[cfg(all(not(windows), not(target_os = "macos")))]
                        let _ = std::process::Command::new("xdotool").args(["key", "Return"]).status();
                        feedback.push("Curseur : à la ligne.".to_string());
                    }
                    "left" | "gauche" => {
                        #[cfg(target_os = "macos")]
                        let _ = std::process::Command::new("osascript").args(["-e", "tell application \"System Events\" to key code 123"]).status();
                        #[cfg(all(not(windows), not(target_os = "macos")))]
                        let _ = std::process::Command::new("xdotool").args(["key", "Left"]).status();
                        feedback.push("Curseur : vers la gauche.".to_string());
                    }
                    "right" | "droite" => {
                        #[cfg(target_os = "macos")]
                        let _ = std::process::Command::new("osascript").args(["-e", "tell application \"System Events\" to key code 124"]).status();
                        #[cfg(all(not(windows), not(target_os = "macos")))]
                        let _ = std::process::Command::new("xdotool").args(["key", "Right"]).status();
                        feedback.push("Curseur : vers la droite.".to_string());
                    }
                    _ => {
                        feedback.push(format!("Commande curseur '{command}' appliquée."));
                    }
                }
            }
            AgentAction::ActivateDictation { .. } => {
                IS_DICTATION_ACTIVE.store(true, Ordering::SeqCst);
                feedback.push("Mode dictée vocale activé.".to_string());
            }
            AgentAction::DeactivateDictation => {
                IS_DICTATION_ACTIVE.store(false, Ordering::SeqCst);
                feedback.push("Mode dictée désactivé.".to_string());
            }
            AgentAction::ToggleDictation { .. } => {
                let new_state = !IS_DICTATION_ACTIVE.load(Ordering::SeqCst);
                IS_DICTATION_ACTIVE.store(new_state, Ordering::SeqCst);
                feedback.push(if new_state { "Mode dictée vocale activé.".to_string() } else { "Mode dictée vocale désactivé.".to_string() });
            }
            AgentAction::SummarizeScreen { window } => {
                let summary = summarize_screen_state(window.as_deref(), false);
                feedback.push(summary);
            }
            AgentAction::ReadEditorContent { target, .. } => {
                let local_file = target.as_deref().and_then(find_file_in_workspace);
                if let Some((path_str, content)) = local_file {
                    feedback.push(format!(
                        "[Contenu extrait du fichier disque '{}' ({} caractères)] :\n```\n{}\n```",
                        path_str,
                        content.chars().count(),
                        truncate_with_notice(&content, 10000)
                    ));
                } else if let Some(clip) = get_plain_clipboard_text() {
                    feedback.push(format!("[Presse-papiers] :\n```\n{}\n```", truncate_with_notice(&clip, 10000)));
                } else {
                    feedback.push("Lecture du fichier : aucun contenu disponible.".to_string());
                }
            }
            AgentAction::MoveWindow { title, x, y, width, height } => {
                apply_window_rect(title, *x, *y, *width, *height);
                feedback.push(format!("Fenêtre '{title}' déplacée en ({}, {}) [{}x{}].", x, y, width, height));
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
            AgentAction::ClickElement { target_name, .. } => {
                let elements = list_interactive_elements("");
                if let Some(elem) = find_best_element(&elements, target_name) {
                    click_element(elem.click_x, elem.click_y, None, None);
                    feedback.push(format!("Clic effectué sur l'élément '{target_name}'."));
                } else {
                    feedback.push(format!("Élément '{target_name}' introuvable à l'écran."));
                }
            }
            AgentAction::ClickButton { button_name, .. } => {
                let elements = list_interactive_elements("");
                if let Some(elem) = find_best_button_element(&elements, button_name) {
                    click_element(elem.click_x, elem.click_y, None, None);
                    feedback.push(format!("Clic effectué sur le bouton '{button_name}'."));
                } else {
                    feedback.push(format!("Bouton '{button_name}' introuvable à l'écran."));
                }
            }
            AgentAction::FocusElement { target_name, .. } => {
                let elements = list_interactive_elements("");
                if let Some(elem) = find_best_input_element(&elements, target_name).or_else(|| find_best_element(&elements, target_name)) {
                    click_element(elem.click_x, elem.click_y, None, None);
                    feedback.push(format!("Focus positionné sur '{target_name}'."));
                } else {
                    feedback.push(format!("Focus demandé sur '{target_name}'."));
                }
            }
            AgentAction::SetAutostart { enabled } => {
                feedback.push(set_autostart(*enabled).unwrap_or_else(|e| e));
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
                    "open_search" => { let _ = std::process::Command::new("xdotool").args(["key", "Super"]).status(); }
                    "open_search_browser" | "browser_search" => {
                        #[cfg(target_os = "macos")]
                        let _ = std::process::Command::new("osascript")
                            .args(["-e", "tell application \"System Events\" to keystroke \"l\" using command down"])
                            .status();
                        #[cfg(not(target_os = "macos"))]
                        let _ = std::process::Command::new("xdotool").args(["key", "ctrl+l"]).status();
                    }
                    "escape" => { let _ = std::process::Command::new("xdotool").args(["key", "Escape"]).status(); }
                    "copy" => { let _ = std::process::Command::new("xdotool").args(["key", "ctrl+c"]).status(); }
                    "close_tab" => { let _ = std::process::Command::new("xdotool").args(["key", "ctrl+w"]).status(); }
                    "refresh_page" => { let _ = std::process::Command::new("xdotool").args(["key", "F5"]).status(); }
                    "bookmarks_bar" | "toggle_bookmarks" | "favoris" => {
                        let _ = std::process::Command::new("xdotool").args(["key", "ctrl+shift+b"]).status();
                    }
                    "browser_back" | "navigate_back" | "back" => {
                        #[cfg(target_os = "macos")]
                        let _ = std::process::Command::new("osascript").args(["-e", "tell application \"System Events\" to keystroke \"[\" using command down"]).status();
                        #[cfg(not(target_os = "macos"))]
                        let _ = std::process::Command::new("xdotool").args(["key", "alt+Left"]).status();
                    }
                    "browser_forward" | "navigate_forward" | "forward" => {
                        #[cfg(target_os = "macos")]
                        let _ = std::process::Command::new("osascript").args(["-e", "tell application \"System Events\" to keystroke \"]\" using command down"]).status();
                        #[cfg(not(target_os = "macos"))]
                        let _ = std::process::Command::new("xdotool").args(["key", "alt+Right"]).status();
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