use std::time::Duration;

#[cfg(windows)]
use windows::core::{w, Interface};
#[cfg(windows)]
use windows::Win32::Foundation::{HWND, RECT};
#[cfg(windows)]
use windows::Win32::System::Com::{
    CoCreateInstance, CoInitializeEx, CLSCTX_INPROC_SERVER, COINIT_MULTITHREADED,
};
#[cfg(windows)]
use windows::Win32::UI::Accessibility::{
    CUIAutomation, IUIAutomation, IUIAutomation2, IUIAutomationElement, IUIAutomationElement2,
    IUIAutomationExpandCollapsePattern, IUIAutomationInvokePattern,
    IUIAutomationLegacyIAccessiblePattern, IUIAutomationRangeValuePattern,
    IUIAutomationScrollItemPattern, IUIAutomationScrollPattern,
    IUIAutomationSelectionItemPattern, IUIAutomationSelectionPattern, IUIAutomationTextPattern,
    IUIAutomationTogglePattern, IUIAutomationValuePattern, TreeScope_Descendants,
    UIA_ButtonControlTypeId, UIA_CheckBoxControlTypeId, UIA_ComboBoxControlTypeId,
    UIA_DataGridControlTypeId, UIA_DataItemControlTypeId, UIA_DocumentControlTypeId,
    UIA_EditControlTypeId, UIA_ExpandCollapsePatternId, UIA_HeaderItemControlTypeId,
    UIA_HyperlinkControlTypeId, UIA_ImageControlTypeId, UIA_InvokePatternId,
    UIA_LegacyIAccessiblePatternId, UIA_ListControlTypeId, UIA_ListItemControlTypeId,
    UIA_MenuItemControlTypeId, UIA_ProgressBarControlTypeId, UIA_RadioButtonControlTypeId,
    UIA_RangeValuePatternId, UIA_ScrollBarControlTypeId, UIA_ScrollItemPatternId,
    UIA_ScrollPatternId, UIA_SelectionItemPatternId, UIA_SelectionPatternId,
    UIA_SliderControlTypeId, UIA_SpinnerControlTypeId, UIA_TabControlTypeId,
    UIA_TabItemControlTypeId, UIA_TextPatternId, UIA_TogglePatternId,
    UIA_ToolBarControlTypeId, UIA_TreeItemControlTypeId, UIA_ValuePatternId,
};
#[cfg(windows)]
use windows::Win32::UI::Input::KeyboardAndMouse::{
    SendInput, INPUT, INPUT_0, INPUT_KEYBOARD, INPUT_MOUSE, KEYBDINPUT, KEYBD_EVENT_FLAGS,
    KEYEVENTF_KEYUP, KEYEVENTF_UNICODE, MOUSEEVENTF_LEFTDOWN, MOUSEEVENTF_LEFTUP, MOUSEINPUT,
    VIRTUAL_KEY,
};
#[cfg(not(windows))]
pub use crate::automation::{AsWindowTarget, NumberedElementTarget, VIRTUAL_KEY};
#[cfg(windows)]
use windows::Win32::UI::WindowsAndMessaging::{FindWindowW, GetClassNameW, SetCursorPos};

pub use crate::automation::{
    ActionHistoryManager, UndoOperation, ACTION_HISTORY,
    execute_undo_operation, undo_last_system_action, redo_last_system_action,
};
use crate::automation::{
    focus_browser_address_bar, force_foreground_window, format_url_for_navigation,
    is_browser_chrome_target, restore_if_iconic, send_hotkey, send_paste, set_clipboard_text,
    write_to_browser_or_txt,
};
use crate::pattern::{clean_words, extract_target_propositions, matches_pattern};
use crate::window_util::*;

#[cfg(windows)]
#[derive(Clone, Default)]
pub struct UiaElementInfoNonWindows;

#[cfg(windows)]
#[derive(Clone)]
pub struct UiaElementInfo {
    pub index: usize,
    pub element: IUIAutomationElement,
    pub automation_id: String,
    pub help_text: String,
    pub value_text: String,
    pub name: String,
    pub class_name: String,
    pub primary_class: String,
    pub data_attributes: Vec<(String, String)>,
    pub json_metadata: Option<serde_json::Value>,
    pub localized_type: String,
    pub aria_role: String,
    pub aria_label: String,
    pub aria_description: String,
    pub aria_indications: String,
    pub is_edit_or_textarea: bool,
    pub is_explicit_textarea: bool,
    pub is_input: bool,
    pub is_select: bool,
    pub is_option: bool,
    pub is_media: bool,
    pub is_slider: bool,
    pub is_toggle: bool,
    pub is_button: bool,
    pub is_link: bool,
    pub is_tab: bool,
    pub is_focusable: bool,
    pub is_tabindex: bool,
    pub is_web_content: bool,
    pub is_browser_chrome: bool,
    pub is_offscreen: bool,
    pub click_x: i32,
    pub click_y: i32,
    pub rect: RECT,
    pub has_action_pattern: bool,
    pub pattern: Option<IUIAutomationInvokePattern>,
    pub area: i64,
}

#[cfg(not(windows))]
#[derive(Clone, Default)]
pub struct UiaElementInfo;

#[cfg(windows)]
pub fn click_element(
    x: i32,
    y: i32,
    element: Option<&IUIAutomationElement>,
    invoke_pattern: Option<&IUIAutomationInvokePattern>,
) {
    let mut click_x = x;
    let mut click_y = y;
    let work_area = get_desktop_work_area();

    // 1. Détection hors-écran & défilement automatique
    if let Some(elem) = element {
        unsafe {
            let is_offscreen = elem.CurrentIsOffscreen().map(|b| b.as_bool()).unwrap_or(false);
            let needs_scroll = is_offscreen
                || click_x < work_area.left
                || click_x >= work_area.right
                || click_y < work_area.top
                || click_y >= work_area.bottom;

            if needs_scroll {
                if let Ok(pattern_unk) = elem.GetCurrentPattern(UIA_ScrollItemPatternId) {
                    if let Ok(scroll_pattern) = pattern_unk.cast::<IUIAutomationScrollItemPattern>() {
                        let _ = scroll_pattern.ScrollIntoView();
                        std::thread::sleep(Duration::from_millis(60));
                    }
                }
            }

            if let Ok(rect) = elem.CurrentBoundingRectangle() {
                let width = rect.right - rect.left;
                let height = rect.bottom - rect.top;
                if width > 0 && height > 0 {
                    click_x = rect.left + (width / 2);
                    click_y = rect.top + (height / 2);
                }
            }

            let _ = elem.SetFocus();
            std::thread::sleep(Duration::from_millis(30));
        }
    }

    // 2. Priorisation des patterns programmatiques UIA
    let mut programmatic_success = false;

    if let Some(pattern) = invoke_pattern {
        unsafe {
            if pattern.Invoke().is_ok() {
                programmatic_success = true;
            }
        }
        std::thread::sleep(Duration::from_millis(25));
    } else if let Some(elem) = element {
        unsafe {
            if let Ok(unk) = elem.GetCurrentPattern(UIA_TogglePatternId) {
                if let Ok(tp) = unk.cast::<IUIAutomationTogglePattern>() {
                    if tp.Toggle().is_ok() {
                        programmatic_success = true;
                    }
                }
            }
            if !programmatic_success {
                if let Ok(unk) = elem.GetCurrentPattern(UIA_SelectionItemPatternId) {
                    if let Ok(sip) = unk.cast::<IUIAutomationSelectionItemPattern>() {
                        if sip.Select().is_ok() {
                            programmatic_success = true;
                        }
                    }
                }
            }
            if !programmatic_success {
                if let Ok(unk) = elem.GetCurrentPattern(UIA_ExpandCollapsePatternId) {
                    if let Ok(ecp) = unk.cast::<IUIAutomationExpandCollapsePattern>() {
                        if let Ok(state) = ecp.CurrentExpandCollapseState() {
                            if state.0 == 0 {
                                if ecp.Expand().is_ok() {
                                    programmatic_success = true;
                                }
                            } else if ecp.Collapse().is_ok() {
                                programmatic_success = true;
                            }
                        }
                    }
                }
            }
            if !programmatic_success {
                if let Ok(unk) = elem.GetCurrentPattern(UIA_LegacyIAccessiblePatternId) {
                    if let Ok(legacy) = unk.cast::<IUIAutomationLegacyIAccessiblePattern>() {
                        if legacy.DoDefaultAction().is_ok() {
                            programmatic_success = true;
                        }
                    }
                }
            }
        }
        std::thread::sleep(Duration::from_millis(25));
    }

    if programmatic_success {
        return;
    }

    // 3. Clic souris physique avec clampage géométrique strict
    let safe_x = click_x.clamp(work_area.left + 2, (work_area.right - 2).max(work_area.left + 2));
    let safe_y = click_y.clamp(work_area.top + 2, (work_area.bottom - 2).max(work_area.top + 2));

    unsafe {
        let _ = SetCursorPos(safe_x, safe_y);
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

#[cfg(not(windows))]
pub fn click_element(_x: i32, _y: i32, _element: Option<&()>, _invoke_pattern: Option<&()>) {}

#[cfg(windows)]
pub fn list_interactive_elements(hwnd: HWND) -> Vec<UiaElementInfo> {
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

        let count = elements.Length().unwrap_or(0).clamp(0, 450);

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
                    if is_offscreen && !is_tab_candidate && !is_toast_window && !is_in_web {
                        continue;
                    }
                    let is_edit_type = ctype == UIA_EditControlTypeId || ctype == UIA_DocumentControlTypeId;
                    let help_text = item.CurrentHelpText().map(|b| b.to_string()).unwrap_or_default().trim().to_string();
                    let item_type = item.CurrentItemType().map(|b| b.to_string()).unwrap_or_default().to_lowercase();
                    let item_status_raw = item.CurrentItemStatus().map(|b| b.to_string()).unwrap_or_default().trim().to_string();
                    let item_status = item_status_raw.to_lowercase();

                    let primary_class = class_name.split_whitespace().next().unwrap_or("").to_string();

                    let (aria_role, aria_properties) = if let Ok(e2) = item.cast::<IUIAutomationElement2>() {
                        let r = e2.CurrentAriaRole().ok().map(|b| b.to_string()).unwrap_or_default();
                        let p = e2.CurrentAriaProperties().ok().map(|b| b.to_string()).unwrap_or_default();
                        (r, p)
                    } else {
                        (String::new(), String::new())
                    };

                    let (data_attrs, json_metadata) = parse_data_attributes_and_json(&aria_properties, &item_status_raw, &help_text);
                    let (aria_label_opt, aria_desc_opt, indications_vec) = extract_aria_details(&aria_properties);
                    let aria_label = aria_label_opt.unwrap_or_default();
                    let aria_description = aria_desc_opt.unwrap_or_default();
                    let mut aria_indications = indications_vec.join(", ");

                    let is_root_web_area = class_name.contains("rootwebarea")
                        || loc_type.contains("rootwebarea")
                        || automation_id.to_lowercase().contains("rootwebarea");
                    if is_root_web_area {
                        continue;
                    }

                    let mut value_text = item
                        .GetCurrentPattern(UIA_ValuePatternId)
                        .ok()
                        .and_then(|p| p.cast::<IUIAutomationValuePattern>().ok())
                        .and_then(|vp| vp.CurrentValue().ok())
                        .map(|b| b.to_string())
                        .unwrap_or_default()
                        .trim().to_string();

                    if value_text.is_empty() {
                        if let Ok(p) = item.GetCurrentPattern(UIA_RangeValuePatternId) {
                            if let Ok(rvp) = p.cast::<IUIAutomationRangeValuePattern>() {
                                if let Ok(val) = rvp.CurrentValue() {
                                    value_text = format!("{:.0}", val);
                                }
                            }
                        }
                    }

                    if value_text.is_empty() {
                        if let Ok(p) = item.GetCurrentPattern(UIA_LegacyIAccessiblePatternId) {
                            if let Ok(legacy) = p.cast::<IUIAutomationLegacyIAccessiblePattern>() {
                                if let Ok(val) = legacy.CurrentValue() {
                                    let s = val.to_string();
                                    if !s.trim().is_empty() {
                                        value_text = s.trim().to_string();
                                    }
                                }
                            }
                        }
                    }

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

                    let is_select = ctype == UIA_ComboBoxControlTypeId
                        || ctype == UIA_ListControlTypeId
                        || ctype == UIA_DataGridControlTypeId
                        || loc_type.contains("liste déroulante")
                        || loc_type.contains("combobox")
                        || loc_type.contains("select")
                        || loc_type.contains("menu déroulant")
                        || aria_role.to_lowercase().contains("combobox")
                        || aria_role.to_lowercase().contains("listbox")
                        || aria_role.to_lowercase().contains("select")
                        || class_name.contains("select")
                        || class_name.contains("combobox")
                        || automation_id.to_lowercase().contains("select");

                    let is_slider = ctype == UIA_SliderControlTypeId
                        || ctype == UIA_ProgressBarControlTypeId
                        || ctype == UIA_SpinnerControlTypeId
                        || loc_type.contains("curseur")
                        || loc_type.contains("slider")
                        || class_name.contains("slider");

                    let is_toggle = ctype == UIA_CheckBoxControlTypeId
                        || ctype == UIA_RadioButtonControlTypeId
                        || loc_type.contains("case à cocher")
                        || loc_type.contains("bascule")
                        || loc_type.contains("toggle")
                        || loc_type.contains("commutateur");

                    let is_option = ctype == UIA_ListItemControlTypeId
                        || ctype == UIA_TreeItemControlTypeId
                        || ctype == UIA_MenuItemControlTypeId
                        || ctype == UIA_DataItemControlTypeId
                        || aria_role == "option"
                        || aria_role == "listitem"
                        || aria_role == "menuitem"
                        || loc_type.contains("option")
                        || loc_type.contains("élément de liste")
                        || loc_type.contains("suggestion")
                        || class_name.contains("suggestion")
                        || class_name.contains("dropdown")
                        || class_name.contains("popup")
                        || automation_id.to_lowercase().contains("option");

                    let is_media = aria_role == "video"
                        || aria_role == "audio"
                        || aria_role == "media"
                        || loc_type.contains("vidéo")
                        || loc_type.contains("video")
                        || loc_type.contains("audio")
                        || loc_type.contains("lecteur")
                        || loc_type.contains("player")
                        || class_name.contains("video-stream")
                        || class_name.contains("html5-main-video")
                        || class_name.contains("html5-video-player")
                        || class_name.contains("jw-video")
                        || class_name.contains("vjs-tech")
                        || (class_name.contains("video") && !class_name.contains("vide"))
                        || class_name.contains("audio")
                        || automation_id.to_lowercase().contains("video")
                        || automation_id.to_lowercase().contains("audio")
                        || automation_id.to_lowercase().contains("player");

                    let is_edit_or_textarea = is_explicit_textarea || is_input || is_select;

                    let is_role_button = aria_role == "button"
                        || aria_role.contains("button")
                        || aria_role.contains("bouton")
                        || aria_role.contains("switch")
                        || aria_role.contains("checkbox")
                        || aria_properties.contains("role=button");

                    let has_button_data = data_attrs.iter().any(|(k, v)| {
                        (k.contains("role") && v == "button")
                            || (k.contains("action") && (v.contains("click") || v.contains("submit") || v.contains("btn")))
                            || (k.contains("test") && (v.contains("btn") || v.contains("button")))
                    });

                    let is_close_or_cross = name == "×"
                        || name == "✕"
                        || name == "⨯"
                        || name == "x"
                        || name == "X"
                        || class_name.contains("close")
                        || class_name.contains("dismiss")
                        || class_name.contains("cross")
                        || automation_id.to_lowercase().contains("close")
                        || automation_id.to_lowercase().contains("dismiss")
                        || aria_label.to_lowercase().contains("fermer")
                        || aria_label.to_lowercase().contains("close");

                    let is_button = ctype == UIA_ButtonControlTypeId
                        || ctype == UIA_MenuItemControlTypeId
                        || ctype == UIA_HeaderItemControlTypeId
                        || ctype == UIA_ImageControlTypeId && (!name.is_empty() || !aria_label.is_empty())
                        || ctype == UIA_ToolBarControlTypeId
                        || loc_type.contains("bouton")
                        || loc_type.contains("button")
                        || is_role_button
                        || has_button_data
                        || is_close_or_cross
                        || is_media
                        || is_slider
                        || is_toggle
                        || class_name.contains("button")
                        || class_name.contains("btn")
                        || automation_id.to_lowercase().contains("button")
                        || automation_id.to_lowercase().contains("btn");

                    if is_close_or_cross && (name.is_empty() || name == "×" || name == "✕" || name == "⨯" || name.eq_ignore_ascii_case("x")) {
                        name = "Fermer (croix ✕)".to_string();
                    } else if is_media && name.is_empty() {
                        let is_aud = aria_role == "audio" || loc_type.contains("audio") || class_name.contains("audio");
                        name = if is_aud {
                            "Lecteur audio (cliquer pour jouer)".to_string()
                        } else {
                            "Lecteur vidéo (cliquer pour jouer)".to_string()
                        };
                    }

                    let is_role_link = aria_role == "link"
                        || aria_role.contains("link")
                        || aria_role.contains("lien")
                        || aria_properties.contains("role=link")
                        || item_type.contains("link")
                        || item_type.contains("lien")
                        || item_status.contains("link")
                        || class_name.contains("role-link")
                        || (class_name.contains("link") && !is_edit_type);

                    let is_link = ctype == UIA_HyperlinkControlTypeId
                        || loc_type.contains("lien")
                        || loc_type.contains("link")
                        || loc_type.contains("hyperlink")
                        || is_role_link;

                    let mut is_tab = is_tab_candidate;

                    let (is_web_content, is_browser_chrome) = match doc_rect {
                        Some(dr) => {
                            if rect.bottom <= (dr.top + 5) { (false, true) } else { (true, false) }
                        }
                        None => (false, false),
                    };

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
                        && !aria_properties.contains("tabindex=-1"))
                        || (is_web_content && (is_focusable || is_option || is_media) && !is_edit_or_textarea && !is_link && !is_button);
                    let is_tabindex = has_tabindex;

                    if is_browser_chrome && !name.is_empty() && !name.starts_with("[Navigateur]") {
                        name = format!("[Navigateur] {name}");
                    }

                    const MAX_INTERACTIVE_TEXT_LEN: usize = 110;
                    if (name.is_empty() || is_tabindex) && (is_link || is_button || is_tab || is_tabindex || is_focusable) {
                        if let Some(txt) = extract_inner_text(&uia, &item, MAX_INTERACTIVE_TEXT_LEN) {
                            if name.is_empty() {
                                name = txt;
                            } else if is_tabindex && name != txt && !name.contains(&txt) {
                                let combined = format!("{name} {txt}");
                                name = combined.chars().take(MAX_INTERACTIVE_TEXT_LEN).collect();
                            }
                        }
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
                        && !is_edit_or_textarea && !is_button && !is_link && !is_tab && !is_focusable && !is_tabindex && !is_media && !is_slider && !is_toggle {
                        continue;
                    }

                    let click_x = rect.left + width / 2;
                    let click_y = rect.top + height / 2;
                    let pattern = item.GetCurrentPattern(UIA_InvokePatternId)
                        .ok()
                        .and_then(|p| p.cast::<IUIAutomationInvokePattern>().ok());
                    let has_toggle = item.GetCurrentPattern(UIA_TogglePatternId).is_ok();
                    let has_sel_item = item.GetCurrentPattern(UIA_SelectionItemPatternId).is_ok();
                    let has_expand = item.GetCurrentPattern(UIA_ExpandCollapsePatternId).is_ok();
                    let has_legacy = item.GetCurrentPattern(UIA_LegacyIAccessiblePatternId).is_ok();
                    let has_action_pattern = pattern.is_some() || has_toggle || has_sel_item || has_expand || has_legacy;
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
                        is_select,
                        is_option,
                        is_media,
                        is_slider,
                        is_toggle,
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
                        has_action_pattern,
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

#[cfg(not(windows))]
pub fn list_interactive_elements(_hwnd: impl AsWindowTarget) -> Vec<UiaElementInfo> {
    Vec::new()
}

#[cfg(windows)]
pub fn has_focused_textarea(_hwnd: HWND) -> bool {
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

#[cfg(not(windows))]
pub fn has_focused_textarea(_hwnd: impl AsWindowTarget) -> bool {
    false
}

#[cfg(windows)]
pub fn refocus_largest_textarea_or_fallback(hwnd: HWND) -> bool {
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

#[cfg(not(windows))]
pub fn refocus_largest_textarea_or_fallback(_hwnd: impl AsWindowTarget) -> bool {
    false
}

#[cfg(windows)]
pub fn find_best_input_element<'a>(elements: &'a [UiaElementInfo], target: &str) -> Option<&'a UiaElementInfo> {
    let is_chrome_target = is_browser_chrome_target(target);
    let has_web_content = elements.iter().any(|e| e.is_web_content);
    const MIN_CONFIDENCE_SCORE: f32 = 30.0;
    let mut scored: Vec<(f32, &'a UiaElementInfo)> = elements
        .iter()
        .filter_map(|elem| {
            let mut chrome_bias = 0.0f32;
            if has_web_content {
                if is_chrome_target {
                    if elem.is_browser_chrome { chrome_bias += 50.0; } else { chrome_bias -= 40.0; }
                } else if elem.is_browser_chrome {
                    chrome_bias -= 45.0;
                }
            }

            score_element(elem, target).map(|score| {
                let mut final_score = score + chrome_bias;
                if elem.is_edit_or_textarea || elem.is_input || elem.is_select {
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

    scored.retain(|(s, _)| *s >= MIN_CONFIDENCE_SCORE);
    scored.sort_by(|(score_a, elem_a), (score_b, elem_b)| {
        if (score_a - score_b).abs() < 0.5 {
            elem_b.area.cmp(&elem_a.area)
        } else {
            score_b.partial_cmp(score_a).unwrap_or(std::cmp::Ordering::Equal)
        }
    });

    scored.first().map(|(_, elem)| *elem)
}

#[cfg(not(windows))]
pub fn find_best_input_element<'a>(_elements: &'a [UiaElementInfo], _target: &str) -> Option<&'a UiaElementInfo> {
    None
}

#[cfg(windows)]
pub fn open_browser_new_tab(hwnd: HWND, url: &str) {
    let formatted_url = format_url_for_navigation(url);
    force_foreground_window(hwnd);
    restore_if_iconic(hwnd);
    std::thread::sleep(Duration::from_millis(100));

    const VK_CONTROL: VIRTUAL_KEY = VIRTUAL_KEY(0x11);
    const VK_T: VIRTUAL_KEY = VIRTUAL_KEY(0x54);
    const VK_RETURN: VIRTUAL_KEY = VIRTUAL_KEY(0x0D);

    send_hotkey(&[VK_CONTROL], VK_T);
    std::thread::sleep(Duration::from_millis(150));

    set_clipboard_text(&formatted_url);
    std::thread::sleep(Duration::from_millis(50));
    send_paste();
    std::thread::sleep(Duration::from_millis(120));
    send_hotkey(&[], VK_RETURN);
    std::thread::sleep(Duration::from_millis(80));
}

#[cfg(not(windows))]
pub fn open_browser_new_tab(_hwnd: impl AsWindowTarget, url: &str) {
    let formatted_url = format_url_for_navigation(url);
    #[cfg(target_os = "macos")]
    let _ = std::process::Command::new("open").arg(&formatted_url).spawn();
    #[cfg(all(not(windows), not(target_os = "macos")))]
    let _ = std::process::Command::new("xdg-open").arg(&formatted_url).spawn();
}

#[cfg(windows)]
pub fn replace_active_field_text(new_text: &str) -> String {
    unsafe {
        let _ = CoInitializeEx(None, COINIT_MULTITHREADED);
        let _uia: Result<IUIAutomation, _> = CoCreateInstance(&CUIAutomation, None, CLSCTX_INPROC_SERVER);
        let user_windows = list_user_windows();
        let target_hwnd = get_active_or_best_window(&user_windows);

        if let Some(hwnd) = target_hwnd {
            force_foreground_window(hwnd);
            restore_if_iconic(hwnd);
            std::thread::sleep(Duration::from_millis(80));

            ensure_window_textarea_focus(hwnd);
            std::thread::sleep(Duration::from_millis(50));

            const VK_CONTROL: VIRTUAL_KEY = VIRTUAL_KEY(0x11);
            const VK_A: VIRTUAL_KEY = VIRTUAL_KEY(0x41);
            send_hotkey(&[VK_CONTROL], VK_A);
            std::thread::sleep(Duration::from_millis(40));

            set_clipboard_text(new_text);
            std::thread::sleep(Duration::from_millis(40));
            send_paste();

            return "Texte mis à jour dans le champ actif.".to_string();
        }
    }

    write_to_browser_or_txt(new_text)
}

#[cfg(not(windows))]
pub fn replace_active_field_text(text: &str) -> String {
    set_clipboard_text(text);
    send_paste();
    "Texte mis à jour dans le champ actif.".to_string()
}

#[cfg(windows)]
pub fn clear_window_text(hwnd: HWND) {
    force_foreground_window(hwnd);
    restore_if_iconic(hwnd);
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

#[cfg(not(windows))]
pub fn clear_window_text(target: impl AsWindowTarget) {
    force_foreground_window(target);
    #[cfg(target_os = "macos")]
    {
        let _ = std::process::Command::new("osascript")
            .args(["-e", "tell application \"System Events\" to keystroke \"a\" using command down\ntell application \"System Events\" to key code 51"])
            .status();
    }
    #[cfg(all(not(windows), not(target_os = "macos")))]
    {
        let _ = std::process::Command::new("xdotool")
            .args(["key", "--clearmodifiers", "ctrl+a", "BackSpace"])
            .status();
    }
}

#[cfg(windows)]
pub fn navigate_browser_address_bar(hwnd: HWND, url: &str) {
    let formatted_url = format_url_for_navigation(url);
    focus_browser_address_bar(hwnd);

    const VK_RETURN: VIRTUAL_KEY = VIRTUAL_KEY(0x0D);

    set_clipboard_text(&formatted_url);
    std::thread::sleep(Duration::from_millis(60));
    send_paste();
    std::thread::sleep(Duration::from_millis(180));

    send_hotkey(&[], VK_RETURN);
    std::thread::sleep(Duration::from_millis(100));
    println!("[Actions] Navigation vers {} effectuée avec succès via barre d'adresse", formatted_url);
}

#[cfg(not(windows))]
pub fn navigate_browser_address_bar(target: impl AsWindowTarget, url: &str) {
    let formatted_url = format_url_for_navigation(url);
    focus_browser_address_bar(target);
    std::thread::sleep(Duration::from_millis(50));
    set_clipboard_text(&formatted_url);
    std::thread::sleep(Duration::from_millis(50));
    send_paste();
    std::thread::sleep(Duration::from_millis(100));
    const VK_RETURN: VIRTUAL_KEY = VIRTUAL_KEY(0x0D);
    send_hotkey(&[], VK_RETURN);
    println!("[Actions] Navigation vers {} effectuée avec succès via barre d'adresse", formatted_url);
}

#[cfg(windows)]
pub fn send_char_unicode(c: char) {
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

#[cfg(not(windows))]
pub fn send_char_unicode(c: char) {
    #[cfg(target_os = "macos")]
    {
        let s = c.to_string();
        let escaped = s.replace('\\', "\\\\").replace('"', "\\\"");
        let script = format!("tell application \"System Events\" to keystroke \"{escaped}\"");
        let _ = std::process::Command::new("osascript").args(["-e", &script]).status();
    }
    #[cfg(all(not(windows), not(target_os = "macos")))]
    {
        let _ = std::process::Command::new("xdotool").args(["type", "--delay", "5", &c.to_string()]).status();
    }
}

#[cfg(windows)]
pub fn send_terminal_input(hwnd: HWND, text: &str, auto_enter: bool) {
    const VK_RETURN: VIRTUAL_KEY = VIRTUAL_KEY(0x0D);
    const VK_SHIFT: VIRTUAL_KEY = VIRTUAL_KEY(0x10);
    const VK_CONTROL: VIRTUAL_KEY = VIRTUAL_KEY(0x11);
    const VK_INSERT: VIRTUAL_KEY = VIRTUAL_KEY(0x2D);
    const VK_V: VIRTUAL_KEY = VIRTUAL_KEY(0x56);

    let trimmed = text.trim_end_matches(['\r', '\n']);
    let has_trailing_newline = text.ends_with('\n') || text.ends_with('\r');
    let should_press_enter = auto_enter || has_trailing_newline;

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
        set_clipboard_text(trimmed);
        std::thread::sleep(Duration::from_millis(40));

        if is_windows_terminal(hwnd) {
            send_hotkey(&[VK_CONTROL, VK_SHIFT], VK_V);
        } else if is_putty_window(hwnd) {
            send_hotkey(&[VK_SHIFT], VK_INSERT);
        } else {
            send_hotkey(&[VK_SHIFT], VK_INSERT);
        }
        std::thread::sleep(Duration::from_millis(50));
    }

    if should_press_enter {
        std::thread::sleep(Duration::from_millis(40));
        send_hotkey(&[], VK_RETURN);
    }
}

#[cfg(not(windows))]
pub fn send_terminal_input(_hwnd: impl AsWindowTarget, text: &str, auto_enter: bool) {
    const VK_RETURN: VIRTUAL_KEY = VIRTUAL_KEY(0x0D);
    let trimmed = text.trim_end_matches(['\r', '\n']);
    let has_trailing_newline = text.ends_with('\n') || text.ends_with('\r');
    let should_press_enter = auto_enter || has_trailing_newline;

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
        set_clipboard_text(trimmed);
        std::thread::sleep(Duration::from_millis(40));
        send_paste();
        std::thread::sleep(Duration::from_millis(50));
    }

    if should_press_enter {
        std::thread::sleep(Duration::from_millis(40));
        send_hotkey(&[], VK_RETURN);
    }
}

#[cfg(windows)]
pub unsafe fn mute_other_audio_sessions() -> usize {
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
    let hr = get_default_endpoint(enumerator, 0, 1, &mut device);
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
pub unsafe fn mute_other_audio_sessions() -> usize {
    0
}
// --- Fonctions et helpers internes de calcul UIA ---

#[cfg(windows)]
pub fn find_largest_textarea(hwnd: HWND) -> Option<UiaElementInfo> {
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

#[cfg(not(windows))]
pub fn find_largest_textarea(_hwnd: impl AsWindowTarget) -> Option<UiaElementInfo> {
    None
}

#[cfg(windows)]
pub fn find_largest_textarea_on_screen(
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

#[cfg(not(windows))]
pub fn find_largest_textarea_on_screen(
    _user_windows: &[(impl AsWindowTarget, String)],
    _preferred_hwnd: Option<impl AsWindowTarget>,
) -> Option<(String, UiaElementInfo)> {
    None
}

#[cfg(windows)]
pub fn ensure_window_textarea_focus(hwnd: HWND) {
    if !has_focused_textarea(hwnd) {
        refocus_largest_textarea_or_fallback(hwnd);
    }
}

#[cfg(not(windows))]
pub fn ensure_window_textarea_focus(_hwnd: impl AsWindowTarget) {}

#[cfg(windows)]
pub fn find_best_element<'a>(elements: &'a [UiaElementInfo], target: &str) -> Option<&'a UiaElementInfo> {
    let target_words = clean_words(target);
    let is_target_short = target_words.len() <= 5 && target.len() <= 40;
    let is_chrome_target = is_browser_chrome_target(target);
    let has_web_content = elements.iter().any(|e| e.is_web_content);
    const MIN_CONFIDENCE_SCORE: f32 = 35.0;

    let mut scored: Vec<(f32, &'a UiaElementInfo)> = elements
        .iter()
        .filter_map(|elem| {
            let mut chrome_bias = 0.0f32;
            if has_web_content {
                if is_chrome_target {
                    if elem.is_browser_chrome { chrome_bias += 50.0; } else { chrome_bias -= 40.0; }
                } else if elem.is_browser_chrome {
                    chrome_bias -= 45.0;
                }
            }

            let word_count = elem.name.split_whitespace().count();
            if !elem.is_media && is_target_short && (word_count > 12 || elem.name.len() > 90) {
                return None;
            }

            score_element(elem, target).map(|score| {
                let mut final_score = score + chrome_bias;
                if elem.is_tab { final_score += 80.0; }
                else if elem.is_media { final_score += 75.0; }
                else if elem.is_option { final_score += 65.0; }
                else if elem.is_link { final_score += 40.0; }
                else if elem.is_tabindex { final_score += 35.0; }
                else if elem.has_action_pattern { final_score += 20.0; }
                if elem.is_web_content { final_score += 35.0; }
                if !elem.is_offscreen { final_score += 20.0; }
                (final_score, elem)
            })
        })
        .collect();

    scored.retain(|(s, _)| *s >= MIN_CONFIDENCE_SCORE);
    scored.sort_by(|(score_a, elem_a), (score_b, elem_b)| {
        if (score_a - score_b).abs() < 0.5 {
            elem_a.area.cmp(&elem_b.area)
        } else {
            score_b.partial_cmp(score_a).unwrap_or(std::cmp::Ordering::Equal)
        }
    });

    scored.first().map(|(_, elem)| *elem)
}

#[cfg(not(windows))]
pub fn find_best_element<'a>(_elements: &'a [UiaElementInfo], _target: &str) -> Option<&'a UiaElementInfo> {
    None
}

#[cfg(windows)]
pub fn find_best_button_element<'a>(elements: &'a [UiaElementInfo], target: &str) -> Option<&'a UiaElementInfo> {
    let is_chrome_target = is_browser_chrome_target(target);
    let has_web_content = elements.iter().any(|e| e.is_web_content);
    const MIN_CONFIDENCE_SCORE: f32 = 40.0;
    let mut scored: Vec<(f32, &'a UiaElementInfo)> = elements
        .iter()
        .filter_map(|elem| {
            let word_count = elem.name.split_whitespace().count();
            if !elem.is_media && (word_count > 15 || elem.name.len() > 120) {
                return None;
            }
            let mut chrome_bias = 0.0f32;
            if has_web_content {
                if is_chrome_target {
                    if elem.is_browser_chrome { chrome_bias += 50.0; } else { chrome_bias -= 40.0; }
                } else if elem.is_browser_chrome {
                    chrome_bias -= 45.0;
                }
            }

            score_element(elem, target).map(|score| {
                let mut final_score = score + chrome_bias;
                if elem.is_button || elem.is_tabindex || elem.is_media {
                    final_score += 100.0;
                } else if elem.has_action_pattern {
                    final_score += 40.0;
                } else {
                    final_score -= 60.0;
                }
                if elem.is_web_content { final_score += 40.0; }
                if elem.area > 150_000 && !elem.is_media { final_score -= 100.0; }
                (final_score, elem)
            })
        })
        .collect();

    scored.retain(|(s, _)| *s >= MIN_CONFIDENCE_SCORE);
    scored.sort_by(|(score_a, elem_a), (score_b, elem_b)| {
        if (score_a - score_b).abs() < 0.5 {
            elem_a.area.cmp(&elem_b.area)
        } else {
            score_b.partial_cmp(score_a).unwrap_or(std::cmp::Ordering::Equal)
        }
    });

    scored.first().map(|(_, elem)| *elem)
}

#[cfg(not(windows))]
pub fn find_best_button_element<'a>(_elements: &'a [UiaElementInfo], _target: &str) -> Option<&'a UiaElementInfo> {
    None
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
pub fn extract_inner_text(uia: &IUIAutomation, element: &IUIAutomationElement, max_chars: usize) -> Option<String> {
    unsafe {
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

        if let Ok(name_bstr) = element.CurrentName() {
            let s = name_bstr.to_string();
            let trimmed = s.trim();
            if !trimmed.is_empty() {
                return Some(trimmed.chars().take(max_chars).collect());
            }
        }

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
                                if !collected.is_empty() { collected.push(' '); }
                                collected.push_str(ctrim);
                                if collected.chars().count() >= max_chars { break; }
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
pub fn extract_web_document_text(element: &IUIAutomationElement, max_chars: usize) -> Option<String> {
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
pub fn extract_scrollable_viewports_content(uia: &IUIAutomation, hwnd: HWND) -> String {
    let mut output = String::new();
    unsafe {
        let Ok(window_element) = uia.ElementFromHandle(hwnd) else { return output; };
        let Ok(cond) = uia.ControlViewCondition() else { return output; };
        let Ok(elements) = window_element.FindAll(TreeScope_Descendants, &cond) else { return output; };

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
fn is_ecommerce_action_term(term: &str) -> bool {
    let t = term.to_lowercase();
    t.contains("commande") || t.contains("panier") || t.contains("cart") || t.contains("checkout")
        || t.contains("valider") || t.contains("payer") || t.contains("commander")
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
            return if (p.contains("valide") || p.contains("commande"))
                && (t.contains("panier") || t.contains("commander") || t.contains("checkout") || t.contains("valider"))
            {
                75.0
            } else {
                45.0
            };
        }
    }
    0.0
}

#[cfg(windows)]
fn score_single_proposition(elem: &UiaElementInfo, prop: &str) -> Option<f32> {
    let prop_clean = prop.trim();
    let prop_lower = prop_clean.to_lowercase();

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

    if prop_clean.starts_with('@') && !prop_clean.starts_with("[@") {
        let tag = prop_clean.trim_start_matches('@');
        let num_str = tag
            .strip_prefix("btn:")
            .or_else(|| tag.strip_prefix("link:"))
            .or_else(|| tag.strip_prefix("input:"))
            .or_else(|| tag.strip_prefix("textarea:"))
            .or_else(|| tag.strip_prefix("select:"))
            .or_else(|| tag.strip_prefix("option:"))
            .or_else(|| tag.strip_prefix("item:"))
            .or_else(|| tag.strip_prefix("menu:"))
            .or_else(|| tag.strip_prefix("tree:"))
            .or_else(|| tag.strip_prefix("video:"))
            .or_else(|| tag.strip_prefix("audio:"))
            .or_else(|| tag.strip_prefix("media:"))
            .unwrap_or(tag);

        if let Ok(target_idx) = num_str.trim().parse::<usize>() {
            if elem.index == target_idx {
                return Some(500.0);
            }
        }
    }

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
        return Some(600.0);
    }

    if name_lower.starts_with(&prop_lower) || aria_label_lower.starts_with(&prop_lower) {
        let leftover_words = name_words_count(&name_lower).saturating_sub(name_words_count(&prop_lower));
        let penalty = (leftover_words as f32) * 15.0;
        return Some((500.0 - penalty).max(300.0));
    }

    if elem.is_media {
        let is_media_query = prop_lower.contains("video")
            || prop_lower.contains("vidéo")
            || prop_lower.contains("audio")
            || prop_lower.contains("lecteur")
            || prop_lower.contains("player")
            || prop_lower.contains("play")
            || prop_lower.contains("lecture")
            || prop_lower.contains("jouer")
            || prop_lower.contains("lancer")
            || prop_lower.contains("regarder")
            || prop_lower.contains("ecouter")
            || prop_lower.contains("écouter");
        if is_media_query {
            return Some(540.0);
        }
    }

    let is_elem_cross = elem.name.contains("croix")
        || elem.name.contains('✕')
        || elem.name.contains('×')
        || elem.name.eq_ignore_ascii_case("fermer")
        || elem.name.eq_ignore_ascii_case("close")
        || elem.class_name.contains("close")
        || elem.automation_id.to_lowercase().contains("close");
    let is_prop_cross = prop_lower.contains("croix")
        || prop_lower == "fermer"
        || prop_lower == "close"
        || prop_lower == "x"
        || prop_lower.contains("fermer la notification")
        || prop_lower.contains("fermer le bandeau");
    if is_elem_cross && is_prop_cross {
        return Some(550.0);
    }

    let ecom_bonus = ecommerce_match_bonus(prop_clean, &elem.name)
        .max(ecommerce_match_bonus(prop_clean, &elem.aria_label));
    let mut best_score: Option<f32> = None;

    let mut regex_score = 0.0f32;
    if matches_pattern(prop, &elem.name) { regex_score = regex_score.max(180.0); }
    if !elem.aria_label.is_empty() && matches_pattern(prop, &elem.aria_label) { regex_score = regex_score.max(190.0); }
    if !elem.aria_description.is_empty() && matches_pattern(prop, &elem.aria_description) { regex_score = regex_score.max(150.0); }
    if !elem.aria_indications.is_empty() && matches_pattern(prop, &elem.aria_indications) { regex_score = regex_score.max(130.0); }
    if matches_pattern(prop, &elem.help_text) { regex_score = regex_score.max(160.0); }
    if matches_pattern(prop, &elem.automation_id) { regex_score = regex_score.max(150.0); }
    if matches_pattern(prop, &elem.value_text) { regex_score = regex_score.max(140.0); }
    if matches_pattern(prop, &elem.localized_type) { regex_score = regex_score.max(110.0); }
    if matches_pattern(prop, &elem.aria_role) { regex_score = regex_score.max(120.0); }
    if matches_pattern(prop, &elem.class_name) { regex_score = regex_score.max(90.0); }
    if !elem.primary_class.is_empty() && matches_pattern(prop, &elem.primary_class) { regex_score = regex_score.max(140.0); }
    for (k, v) in &elem.data_attributes {
        if matches_pattern(prop, v) { regex_score = regex_score.max(170.0); }
        else if matches_pattern(prop, k) { regex_score = regex_score.max(120.0); }
    }

    if regex_score > 0.0 {
        regex_score += ecom_bonus;
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

    if q_words.len() >= 3 && matched_words_count < 2 {
        return None;
    }

    if matched_words_count == q_words.len() {
        total_score += 50.0 * (q_words.len() as f32);
        if !name_words.is_empty() && name_words.len() == q_words.len() {
            total_score += 80.0;
        }
    } else {
        total_score *= (matched_words_count as f32) / (q_words.len() as f32);
    }

    let mut unexpected_words_penalty = 0.0f32;
    for nw in &name_words {
        if !q_words.iter().any(|qw| qw == nw || nw.contains(qw) || qw.contains(nw)) {
            unexpected_words_penalty += 35.0;
        }
    }
    total_score = (total_score - unexpected_words_penalty).max(1.0);

    if !name_words.is_empty() && name_words.len() > q_words.len() * 4 {
        let excess = (name_words.len() - q_words.len() * 4) as f32;
        total_score = (total_score - excess * 2.0).max(5.0);
    }

    if total_score >= 15.0 {
        total_score += ecom_bonus;
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

pub fn is_poor_or_redundant_link(text: &str) -> bool {
    let lower = text.trim().to_lowercase();
    lower.len() < 2
}

#[cfg(windows)]
pub fn score_enriched_link(text: &str) -> f32 {
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

    if clean.contains("[aria:") { score += 25.0; }
    if clean.contains("[desc:") { score += 15.0; }
    if clean.contains("[data:") || clean.contains("testid:") { score += 20.0; }

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
pub fn score_enriched_button(text: &str) -> f32 {
    let clean = text.trim();
    let lower = clean.to_lowercase();
    let mut score = 10.0f32;

    const HIGH_VALUE_BUTTONS: &[&str] = &[
        "panier", "cart", "commander", "commande", "valider", "passer la commande",
        "voir le panier", "payer", "checkout", "finaliser", "confirmer", "continuer",
        "suivant", "ajouter", "soumettre", "submit", "acheter", "terminer", "accepter",
        "activer", "autoriser", "fermer", "plus tard", "refuser", "bloquer", "croix", "close", "dismiss",
        "non merci", "ne plus afficher", "ignorer", "pas maintenant",
        "play", "lecture", "jouer", "pause", "vidéo", "video", "audio", "écouter", "regarder",
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
    if clean.contains("[aria:") { score += 15.0; }
    if clean.contains("[desc:") { score += 15.0; }
    if clean.contains("[data:") || clean.contains("testid:") || clean.contains("action:") { score += 20.0; }

    let words = clean.split_whitespace().count();
    if (1..=4).contains(&words) {
        score += 10.0;
    } else if words > 8 {
        score -= 15.0;
    }

    score
}

#[cfg(not(windows))]
pub fn score_enriched_link(_text: &str) -> f32 {
    10.0
}

#[cfg(not(windows))]
pub fn score_enriched_button(_text: &str) -> f32 {
    10.0
}

#[cfg(windows)]
fn name_words_count(s: &str) -> usize {
    s.split_whitespace().count()
}