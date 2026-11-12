use std::sync::atomic::Ordering;
use std::time::{Duration, Instant};

use crate::automation::*;
use crate::overlay::*;
use crate::types::*;
use crate::ui;
use crate::window_util::*;
use crate::OverlayApp;

impl OverlayApp {
    pub const BADGES_CHUNK_SIZE: usize = 150;

    pub fn narrate_numbered_target(&mut self, target: &NumberedElementTarget) {
        self.narrate_numbered_target_with_prefix(None, target);
    }

    pub fn narrate_numbered_target_with_prefix(&mut self, prefix: Option<&str>, target: &NumberedElementTarget) {
        #[cfg(windows)]
        {
            if let Some(wid) = target.window_id {
                let hwnd = windows::Win32::Foundation::HWND(wid as _);
                if !hwnd.0.is_null() && !is_window_hung(hwnd) {
                    force_foreground_window(hwnd);
                    restore_if_iconic(hwnd);
                }
            }
            focus_element_target(target);
            set_global_cursor_pos(target.click_x, target.click_y);
        }
        #[cfg(not(windows))]
        {
            focus_element_target(target);
            set_global_cursor_pos(target.click_x, target.click_y);
        }
        self.hover_last_cursor_pos = Some((target.click_x, target.click_y));
        self.hover_cursor_still_since = Instant::now();

        let role_label = match target.role.as_str() {
            "bouton" | "button" => t!("roles.button").to_string(),
            "lien" | "link" => t!("roles.link").to_string(),
            "champ de saisie" | "input" => t!("roles.input").to_string(),
            "zone de texte" | "textarea" => t!("roles.textarea").to_string(),
            "onglet" | "tab" => t!("roles.tab").to_string(),
            "lecture textuelle" => t!("roles.text_reader").to_string(),
            "étiquette" | "label" => t!("roles.label").to_string(),
            "visuel (ocr)" | "ocr" => t!("roles.ocr").to_string(),
            "case à cocher" | "checkbox" => t!("roles.checkbox").to_string(),
            "liste déroulante" | "select" | "combobox" => t!("roles.combobox").to_string(),
            "curseur" | "slider" => t!("roles.slider").to_string(),
            "lecteur multimédia" | "media" => t!("roles.media").to_string(),
            s if s.contains("qwen") || s.contains("vlm") => "icône ou élément visuel détecté".to_string(),
            other => other.to_string(),
        };

        let target_desc = if target.name.trim().is_empty() {
            t!("commands.target_desc_role", index = target.index, role = role_label).to_string()
        } else if target.value.trim().is_empty() {
            t!("commands.target_desc_name_role", index = target.index, name = target.name.trim(), role = role_label).to_string()
        } else {
            t!("commands.target_desc_full", index = target.index, name = target.name.trim(), role = role_label, value = target.value.trim()).to_string()
        };

        let speech = if let Some(p) = prefix {
            format!("{p} {target_desc}")
        } else {
            target_desc
        };

        let _ = self.tts_sender.send(TtsCommand::Stop);
        let _ = self.tts_sender.send(TtsCommand::Speak(speech));
    }

    pub fn sync_switch_access_overlays(&self) {
        if self.switch_access_mode {
            match self.switch_step {
                SwitchAccessStep::Windows => {
                    if !self.switch_zones.is_empty() {
                        let zone_infos: Vec<ZoneOverlayInfo> = self.switch_zones.iter().enumerate().map(|(i, z)| {
                            ZoneOverlayInfo {
                                index: i + 1,
                                title: z.title.clone(),
                                rect: z.rect,
                                is_focused: i == 0,
                            }
                        }).collect();
                        let focused_target_idx = self.numbered_targets.first().map(|t| t.index);
                        draw_screen_zones_and_targets(&zone_infos, &self.numbered_targets, self.zoom_factor, focused_target_idx);
                    } else if !self.numbered_targets.is_empty() {
                        let focused_idx = self.numbered_targets.first().map(|t| t.index);
                        draw_screen_numbered_badges(&self.numbered_targets, self.zoom_factor, focused_idx);
                    }
                }
                SwitchAccessStep::Zones => {
                    if !self.switch_zones.is_empty() {
                        let num_zones = self.switch_zones.len();
                        let zone_infos: Vec<ZoneOverlayInfo> = self.switch_zones.iter().enumerate().map(|(i, z)| {
                            ZoneOverlayInfo {
                                index: i + 1,
                                title: z.title.clone(),
                                rect: z.rect,
                                is_focused: (self.switch_focus_index % num_zones) == i,
                            }
                        }).collect();
                        let focused_target_idx = self.numbered_targets.first().map(|t| t.index);
                        draw_screen_zones_and_targets(&zone_infos, &self.numbered_targets, self.zoom_factor, focused_target_idx);
                    }
                }
                SwitchAccessStep::Elements => {
                    let focused_target = self.numbered_targets.get(self.switch_focus_index % self.numbered_targets.len().max(1));
                    let focused_idx = focused_target.map(|t| t.index);
                    let active_zone_opt = self.switch_active_zone_index.and_then(|idx| self.switch_zones.get(idx));
                    let chunk_start = self.numbered_chunk_offset.min(self.numbered_targets.len());
                    let chunk_end = (chunk_start + 50).min(self.numbered_targets.len());
                    let visible_targets = if self.numbered_targets.len() > 50 {
                        &self.numbered_targets[chunk_start..chunk_end]
                    } else {
                        &self.numbered_targets[..]
                    };

                    if let Some(zone) = active_zone_opt {
                        let zone_info = vec![ZoneOverlayInfo {
                            index: self.switch_active_zone_index.unwrap_or(0) + 1,
                            title: zone.title.clone(),
                            rect: zone.rect,
                            is_focused: true,
                        }];
                        draw_screen_zones_and_targets(&zone_info, visible_targets, self.zoom_factor, focused_idx);
                    } else if !visible_targets.is_empty() {
                        draw_screen_numbered_badges(visible_targets, self.zoom_factor, focused_idx);
                    }

                    #[cfg(windows)]
                    if let Some(target) = focused_target {
                        draw_screen_control_border(target.rect, target.role == "lecture textuelle");
                    }
                }
            }
        }
    }

    pub fn click_numbered_target_by_index(&mut self, idx: usize) {
        self.stop_speaking();

        // 1. Chercher en priorité absolue la cible directement parmi les cibles affichées
        let mut found_target = self.numbered_targets.iter().find(|t| t.index == idx).cloned();

        // 2. Si non trouvée dans la zone active courante, chercher dans l'ensemble des zones de la fenêtre
        if found_target.is_none() {
            for (z_idx, zone) in self.switch_zones.iter().enumerate() {
                if let Some(t) = zone.targets.iter().find(|t| t.index == idx) {
                    found_target = Some(t.clone());
                    self.switch_active_zone_index = Some(z_idx);
                    break;
                }
            }
        }

        // 3. Repli : si AUCUNE cible ne porte ce numéro et qu'on est en sélection de zone,
        // alors seulement le numéro sert à basculer vers la zone correspondante
        if found_target.is_none() && self.switch_access_mode && self.switch_step == SwitchAccessStep::Zones {
            if idx >= 1 && idx <= self.switch_zones.len() {
                self.select_switch_zone(idx - 1);
                return;
            }
        }

        if let Some(target) = found_target {
            if self.switch_step == SwitchAccessStep::Zones {
                self.switch_step = SwitchAccessStep::Elements;
            }
            if target.role == "fenêtre" {
                if let Some(wid) = target.window_id {
                    self.switch_active_window_id = Some(wid);
                    self.transition_to_zones_for_window(wid, &target.name);
                    return;
                }
            }

            #[cfg(windows)]
            {
                clear_screen_numbered_badges();
                if let Some(wid) = target.window_id {
                    let hwnd = windows::Win32::Foundation::HWND(wid as _);
                    if !hwnd.0.is_null() && !is_window_hung(hwnd) {
                        force_foreground_window(hwnd);
                        restore_if_iconic(hwnd);
                    }
                }
                let pt = windows::Win32::Foundation::POINT { x: target.click_x, y: target.click_y };
                unsafe {
                    let target_child = windows::Win32::UI::WindowsAndMessaging::WindowFromPoint(pt);
                    if !target_child.is_invalid() {
                        let root = windows::Win32::UI::WindowsAndMessaging::GetAncestor(
                            target_child,
                            windows::Win32::UI::WindowsAndMessaging::GA_ROOT,
                        );
                        let top_hwnd = if !root.is_invalid() { root } else { target_child };
                        force_foreground_window(top_hwnd);
                        restore_if_iconic(top_hwnd);
                    }
                }
                let mut uia_elem = None;
                let _ = unsafe { windows::Win32::System::Com::CoInitializeEx(None, windows::Win32::System::Com::COINIT_MULTITHREADED) };
                if let Ok(automation) = unsafe {
                    windows::Win32::System::Com::CoCreateInstance::<_, windows::Win32::UI::Accessibility::IUIAutomation>(
                        &windows::Win32::UI::Accessibility::CUIAutomation,
                        None,
                        windows::Win32::System::Com::CLSCTX_INPROC_SERVER,
                    )
                } {
                    if let Ok(elem) = unsafe { automation.ElementFromPoint(pt) } {
                        uia_elem = Some(elem);
                    }
                }
                ui::click_element(target.click_x, target.click_y, uia_elem.as_ref(), None);
            }
            #[cfg(not(windows))]
            {
                clear_screen_numbered_badges();
                ui::click_element(target.click_x, target.click_y, None, None);
            }
            if target.role == "champ de saisie" || target.role == "zone de texte" {
                crate::automation::IS_EXTERNAL_TEXT_INPUT_FOCUSED.store(true, Ordering::SeqCst);
                self.dictation_mode = true;
                IS_DICTATION_ACTIVE.store(true, Ordering::SeqCst);
                self.continuous_mode = true;
                if let Ok(mut guard) = LAST_FOCUSED_RECT.lock() {
                    *guard = Some(target.rect);
                }
            }
            play_earcon(Earcon::Success);
            let target_desc = if !target.name.is_empty() {
                t!("commands.click_target_named", index = idx, name = target.name).to_string()
            } else {
                t!("commands.click_target_num", index = idx).to_string()
            };
            self.chat_history.push(ChatEntry {
                role: ChatRole::Agent,
                text: target_desc.clone(),
                timestamp: current_time_str(),
                quick_suggestions: vec![QuickSuggestionItem::Text(t!("commands.close_badges").to_string())],
                screen_size_kb: None,
            });
            let _ = self.tts_sender.send(TtsCommand::Stop);
            let _ = self.tts_sender.send(TtsCommand::Speak(target_desc));

            // Laisser le temps à l'application cible de réagir au clic (navigation, menu, etc.)
            std::thread::sleep(Duration::from_millis(250));
            self.switch_focus_index = 0;

            // Régénération automatique des numéros/badges après le clic pour enchaîner
            if self.switch_access_mode {
                #[cfg(windows)]
                {
                    let fg = unsafe { windows::Win32::UI::WindowsAndMessaging::GetForegroundWindow() };
                    let target_hwnd = if !fg.0.is_null() && !is_window_hung(fg) {
                        fg.0 as isize
                    } else {
                        target.window_id.unwrap_or(0)
                    };
                    if target_hwnd != 0 {
                        self.refresh_numbered_targets_for_window(target_hwnd);
                    }
                }
                #[cfg(not(windows))]
                {
                    self.refresh_numbered_targets_for_window(0);
                }
            }
        } else {
            let max_idx = self.numbered_targets.iter().map(|t| t.index).max()
                .unwrap_or(0);
            play_earcon(Earcon::Error);
            let msg = if max_idx > 0 {
                t!("commands.target_not_found_range", index = idx, max = max_idx).to_string()
            } else {
                t!("commands.target_not_found_none", index = idx).to_string()
            };
            self.chat_history.push(ChatEntry {
                role: ChatRole::Agent,
                text: msg.clone(),
                timestamp: current_time_str(),
                quick_suggestions: vec![QuickSuggestionItem::Text(t!("commands.close_badges").to_string())],
                screen_size_kb: None,
            });
            let _ = self.tts_sender.send(TtsCommand::Stop);
            let _ = self.tts_sender.send(TtsCommand::Speak(msg));
        }
    }

    /// Transitionne de l'étape fenêtre vers l'étape des Chunks de badges / Zones
    pub fn transition_to_zones_for_window(&mut self, window_id: isize, win_title: &str) {
        self.switch_active_window_id = Some(window_id);
        self.switch_step = SwitchAccessStep::Zones;
        self.switch_focus_index = 0;
        self.switch_auto_scroll = self.is_single_switch_mode();
        self.switch_last_tick = Instant::now() + Duration::from_millis(1500);
        clear_screen_numbered_badges();

        #[cfg(windows)]
        let target_hwnd = windows::Win32::Foundation::HWND(window_id as _);
        #[cfg(windows)]
        {
            if !target_hwnd.0.is_null() && !is_window_hung(target_hwnd) {
                force_foreground_window(target_hwnd);
                restore_if_iconic(target_hwnd);
            }
        }

        if let Some(cached) = self.window_analysis_cache.get(&window_id).cloned() {
            self.switch_zones = cached.zones;
            self.display_zones_overlay(&cached.summary);
        } else {
            #[cfg(windows)]
            let raw_targets = collect_numbered_targets_for_window(target_hwnd);
            #[cfg(not(windows))]
            let raw_targets = collect_numbered_targets_for_window(win_title);

            let (summary, fallback_zones) = build_fallback_navigation_zones(win_title, &raw_targets);
            self.window_analysis_cache.insert(window_id, CachedWindowAnalysis {
                summary: summary.clone(),
                zones: fallback_zones.clone(),
            });
            self.switch_zones = fallback_zones;
            self.display_zones_overlay(&summary);

            let _ = self.command_sender.send(AgentCommand::AnalyzeNavigationZones {
                window_id,
                window_title: win_title.to_string(),
                targets: raw_targets,
                context_meta: None,
            });
        }
    }

    pub fn display_zones_overlay(&mut self, speech_text: &str) {
        let zone_infos: Vec<ZoneOverlayInfo> = self.switch_zones.iter().enumerate().map(|(i, z)| {
            ZoneOverlayInfo {
                index: i + 1,
                title: z.title.clone(),
                rect: z.rect,
                is_focused: (self.switch_focus_index % self.switch_zones.len().max(1)) == i,
            }
        }).collect();

        // N'injecter que les badges jaunes de la zone active/focalisée
        if let Some(cur_zone) = self.switch_zones.get(self.switch_focus_index % self.switch_zones.len().max(1)) {
            let mut active_targets = cur_zone.targets.clone();
            for (i, t) in active_targets.iter_mut().enumerate() {
                t.index = i + 1;
            }
            self.numbered_targets = active_targets;
        } else {
            self.numbered_targets.clear();
        }

        let focused_target_idx = self.numbered_targets.first().map(|t| t.index);
        draw_screen_zones_and_targets(&zone_infos, &self.numbered_targets, self.zoom_factor, focused_target_idx);

        self.chat_history.push(ChatEntry {
            role: ChatRole::Agent,
            text: speech_text.to_string(),
            timestamp: current_time_str(),
            quick_suggestions: Vec::new(),
            screen_size_kb: None,
        });
        let _ = self.tts_sender.send(TtsCommand::Stop);
        let _ = self.tts_sender.send(TtsCommand::Speak(speech_text.to_string()));
    }

    /// Active une fenêtre et affiche immédiatement la première zone bleue identifiée ainsi que ses points jaunes
    pub fn activate_window_and_display_first_zone(
        &mut self,
        window_id: isize,
        win_title: &str,
        win_rect: [i32; 4],
        speak_summary: bool,
    ) {
        self.switch_active_window_id = Some(window_id);

        #[cfg(windows)]
        {
            let target_hwnd = windows::Win32::Foundation::HWND(window_id as _);
            focus_and_isolate_window(target_hwnd);
            let mut r = windows::Win32::Foundation::RECT::default();
            if unsafe { windows::Win32::UI::WindowsAndMessaging::GetWindowRect(target_hwnd, &mut r).is_ok() } {
                draw_screen_orange_border([r.left, r.top, r.right, r.bottom]);
            } else {
                draw_screen_orange_border(win_rect);
            }
        }
        #[cfg(not(windows))]
        {
            focus_and_isolate_window(win_title);
        }

        let (summary, zones) = if let Some(cached) = self.window_analysis_cache.get(&window_id).cloned() {
            (cached.summary, cached.zones)
        } else {
            #[cfg(windows)]
            let raw_targets = collect_numbered_targets_for_window(windows::Win32::Foundation::HWND(window_id as _));
            #[cfg(not(windows))]
            let raw_targets = collect_numbered_targets_for_window(win_title);

            let (sum, fb_zones) = build_fallback_navigation_zones(win_title, &raw_targets);
            self.window_analysis_cache.insert(window_id, CachedWindowAnalysis {
                summary: sum.clone(),
                zones: fb_zones.clone(),
            });

            if !raw_targets.is_empty() {
                #[cfg(windows)]
                let context_meta = extract_window_context_meta(windows::Win32::Foundation::HWND(window_id as _), win_title);
                #[cfg(not(windows))]
                let context_meta = None;

                let _ = self.command_sender.send(AgentCommand::AnalyzeNavigationZones {
                    window_id,
                    window_title: win_title.to_string(),
                    targets: raw_targets,
                    context_meta,
                });
            }
            (sum, fb_zones)
        };

        self.switch_zones = zones;
        if let Some(first_zone) = self.switch_zones.first() {
            let mut active_targets = first_zone.targets.clone();
            for (i, t) in active_targets.iter_mut().enumerate() {
                t.index = i + 1;
            }
            self.numbered_targets = active_targets;
        } else {
            self.numbered_targets.clear();
        }

        let zone_infos: Vec<ZoneOverlayInfo> = self.switch_zones.iter().enumerate().map(|(i, z)| {
            ZoneOverlayInfo {
                index: i + 1,
                title: z.title.clone(),
                rect: z.rect,
                is_focused: i == 0,
            }
        }).collect();

        let focused_target_idx = self.numbered_targets.first().map(|t| t.index);
        draw_screen_zones_and_targets(&zone_infos, &self.numbered_targets, self.zoom_factor, focused_target_idx);

        if speak_summary {
            let speech = if !summary.is_empty() { summary } else { win_title.to_string() };
            let _ = self.tts_sender.send(TtsCommand::Stop);
            let _ = self.tts_sender.send(TtsCommand::Speak(speech));
        }
    }

    pub fn refresh_numbered_targets_for_window(&mut self, hwnd_id: isize) {
        #[cfg(windows)]
        {
            let hwnd = windows::Win32::Foundation::HWND(hwnd_id as _);
            if hwnd.0.is_null() || is_window_hung(hwnd) {
                return;
            }

            let mut r = windows::Win32::Foundation::RECT::default();
            let has_rect = unsafe { windows::Win32::UI::WindowsAndMessaging::GetWindowRect(hwnd, &mut r).is_ok() };
            let cur_rect = if has_rect { Some([r.left, r.top, r.right, r.bottom]) } else { None };

            self.numbered_last_refresh = Instant::now();
            self.numbered_last_hwnd = Some(hwnd_id);
            self.numbered_last_win_rect = cur_rect;
            self.numbered_chunk_offset = 0;
            self.switch_active_window_id = Some(hwnd_id);
            self.numbered_input_buffer.clear();

            self.scan_sequence_counter += 1;
            let req = TargetsScanRequest {
                hwnd: hwnd_id,
                win_rect: cur_rect,
                previous: self.numbered_targets.clone(),
                sequence: self.scan_sequence_counter,
            };
            self.is_scan_in_progress = true;
            let _ = self.targets_request_tx.send(req);
        }
        #[cfg(not(windows))]
        {
            self.numbered_last_refresh = Instant::now();
            self.numbered_chunk_offset = 0;
            self.switch_active_window_id = Some(hwnd_id);
            self.numbered_input_buffer.clear();
            self.scan_sequence_counter += 1;
            let req = TargetsScanRequest {
                hwnd: hwnd_id,
                win_rect: None,
                previous: self.numbered_targets.clone(),
                sequence: self.scan_sequence_counter,
            };
            self.is_scan_in_progress = true;
            let _ = self.targets_request_tx.send(req);
        }
    }

    /// Sélectionne un Chunk / Zone et bascule vers ses contrôles et éléments OCR individuels
    pub fn select_switch_zone(&mut self, zone_idx: usize) {
        if zone_idx >= self.switch_zones.len() {
            return;
        }
        self.last_switch_input = Instant::now();
        self.switch_auto_scroll = self.is_single_switch_mode();
        self.hover_physical_mouse_moved = false;
        play_earcon(Earcon::Success);

        let cur_zone = self.switch_zones[zone_idx].clone();
        self.numbered_targets = cur_zone.targets.clone();
        self.switch_active_zone_index = Some(zone_idx);
        self.switch_step = SwitchAccessStep::Elements;
        self.switch_focus_index = 0;
        self.hover_controls_speech_enabled = true;

        // Réaffecter des indices séquentiels pour ce chunk
        for (i, t) in self.numbered_targets.iter_mut().enumerate() {
            t.index = i + 1;
        }

        if let Some(first_target) = self.numbered_targets.first() {
            focus_element_target(first_target);
            self.hover_last_cursor_pos = Some((first_target.click_x, first_target.click_y));
            self.hover_cursor_still_since = Instant::now();
            self.hover_last_announced_target = Some(first_target.index);
        }

        self.switch_last_tick = Instant::now() + Duration::from_millis(1500);
        let vocal_msg = format!("Zone {} sélectionnée. {}", cur_zone.title, format_plural_control_type(&self.numbered_targets));
        let _ = self.tts_sender.send(TtsCommand::Stop);
        let _ = self.tts_sender.send(TtsCommand::Speak(vocal_msg));

        // Maintenir l'encadré bleu et son badge de titre autour de la zone active
        let zone_info = vec![ZoneOverlayInfo {
            index: zone_idx + 1,
            title: cur_zone.title.clone(),
            rect: cur_zone.rect,
            is_focused: true,
        }];
        draw_screen_zones_and_targets(&zone_info, &self.numbered_targets, self.zoom_factor, Some(1));
        #[cfg(windows)]
        draw_screen_control_border(cur_zone.rect, true);
    }
}