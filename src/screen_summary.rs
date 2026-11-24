use std::sync::Mutex;

use crate::automation::{
    read_terminal_window_content, truncate_with_notice, LAST_EXTERNAL_HWND,
};
use crate::fs_util::find_file_in_workspace;
#[cfg(target_os = "macos")]
use crate::automation::{get_active_field_content, query_macos_ax_elements, MacosAxElementRaw};
#[cfg(all(not(windows), not(target_os = "macos")))]
use crate::automation::{query_atspi_elements, AtspiElementRaw};
use crate::types::QuickSuggestionItem;
use crate::window_util::*;

#[cfg(windows)]
use windows::Win32::Foundation::{HWND, RECT};
#[cfg(windows)]
use windows::Win32::System::Com::{
    CoCreateInstance, CoInitializeEx, CLSCTX_INPROC_SERVER, COINIT_MULTITHREADED,
};
#[cfg(windows)]
use windows::Win32::UI::Accessibility::*;
#[cfg(windows)]
use windows::Win32::UI::WindowsAndMessaging::*;
#[cfg(windows)]
use crate::ui::*;
#[cfg(windows)]
use crate::get_window_title;

#[cfg(not(windows))]
use crate::automation::AsWindowTarget;

pub static LAST_SCREEN_SUMMARY: Mutex<Option<String>> = Mutex::new(None);

pub fn clean_diff_line(line: &str) -> String {
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

pub fn strip_diff_section(text: &str) -> String {
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

            if lines.len() <= 2 {
                if let Ok(cond) = unsafe { uia.ControlViewCondition() } {
                    if let Ok(elements) = unsafe { win_elem.FindAll(TreeScope_Descendants, &cond) } {
                        let count = unsafe { elements.Length().unwrap_or(0).min(120) };
                        for i in 0..count {
                            if let Ok(elem) = unsafe { elements.GetElement(i) } {
                                let ctype = unsafe { elem.CurrentControlType().unwrap_or_default() };
                                let is_text_type = ctype == UIA_DocumentControlTypeId
                                    || ctype == UIA_EditControlTypeId
                                    || ctype.0 == 50020;

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

#[cfg(windows)]
pub fn summarize_screen_state(target_window: Option<&str>, _autoscroll: bool) -> String {
    let work_area = get_desktop_work_area();
    let wa_w = work_area.right - work_area.left;
    let wa_h = work_area.bottom - work_area.top;

    let user_windows = list_user_windows();
    let raw_fg = unsafe { GetForegroundWindow() };
    let current_pid = std::process::id();
    let (raw_fg_pid, _) = get_window_process_info(raw_fg);
    let is_libertide_or_unknown = raw_fg.0.is_null()
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
            .unwrap_or(HWND(std::ptr::null_mut()))
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

    if inspect_hwnds.is_empty() && !active_hwnd.0.is_null() {
        let (pid, _) = get_window_process_info(active_hwnd);
        if pid != current_pid {
            inspect_hwnds.push(active_hwnd);
        }
    }
    for (h, _) in &user_windows {
        let (h_pid, _) = get_window_process_info(*h);
        if h_pid != current_pid && !inspect_hwnds.contains(h) && inspect_hwnds.len() < 3 {
            inspect_hwnds.push(*h);
        }
    }

    for (h, title) in &user_windows {
        if title.to_lowercase().contains("notification") || title.to_lowercase().contains("toast") {
            if !inspect_hwnds.contains(h) {
                inspect_hwnds.push(*h);
            }
        }
    }

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

        unsafe {
            if let Ok(uia) = CoCreateInstance::<_, IUIAutomation>(&CUIAutomation, None, CLSCTX_INPROC_SERVER) {
                let viewports_text = extract_scrollable_viewports_content(&uia, *hwnd);
                if !viewports_text.trim().is_empty() {
                    active_window_content.push_str(&format!("\n=== Viewports et zones défilables (Scrollbar max / conteneurs actifs) ===\n{}", viewports_text));
                }
            }
        }

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

    if !web_changes_section.is_empty() {
        out.push_str(&web_changes_section);
    }
    out.push_str(&active_window_content);
    out.push_str(&other_windows_content);

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

    if let Some(clip) = crate::clipboard::get_clipboard_text() {
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

        let is_target = target_window.map_or(false, |tw| title.to_lowercase().contains(&tw.to_lowercase()));
        let is_current = active_id.as_deref().map_or(false, |act| win_id.contains(act) || act.contains(win_id));

        if is_target || is_current {
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

    if let Some(clip) = crate::clipboard::get_clipboard_text() {
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