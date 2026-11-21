use std::sync::atomic::Ordering;
use std::time::{Duration, Instant};
use eframe::egui;
use rust_i18n::t;

use crate::audio;
use crate::automation::*;
use crate::clipboard::*;
use crate::floating_window::{self, update_open_floating_cards, FloatingCard, FloatingContentType};
use crate::fs_util::*;
use crate::intent::*;
use crate::keyboard_hook::*;
use crate::locale::*;
use crate::overlay::*;
use crate::overlay::play_earcon_spatial;
use crate::theme::{AppTheme, SettingsTab, ThemeColors};
use crate::tts::{sanitize_for_tts, TtsCommand};
use crate::types::*;
use crate::ui;
use crate::window_util::*;
use crate::OverlayApp;

impl OverlayApp {
    /// Traite l'échantillonnage et le positionnement du regard via l'Eye-Tracker
    pub fn process_gaze_tick(
        &mut self,
        ctx: &egui::Context,
        work_area: &WorkArea,
        ppp: f32,
        is_typing: bool,
    ) {
        let eye_tracking_needed = crate::eyetracking::is_eye_tracking_enabled()
            || self.eye_tracker.calibration.is_active;

        let enter_physically_pressed = crate::eyetracking::is_enter_key_pressed();
        let is_eye_on = crate::eyetracking::is_eye_tracking_enabled() && !self.eye_tracker.calibration.is_active;

        if is_eye_on && !is_typing {
            let gaze_click_queued = crate::eyetracking::GAZE_CLICK_REQUESTED.swap(false, Ordering::SeqCst);
            if (enter_physically_pressed && !self.enter_was_physically_down) || gaze_click_queued {
                self.enter_was_physically_down = true;
                if self.last_eye_click_time.elapsed() >= Duration::from_millis(250) {
                    self.last_eye_click_time = Instant::now();
                    self.trigger_gaze_click();
                    ctx.request_repaint();
                }
            } else if !enter_physically_pressed {
                self.enter_was_physically_down = false;
            }
        }

        if eye_tracking_needed {
            crate::eyetracking::ensure_camera_worker_running();
            let (screen_w, screen_h) = (work_area.w, work_area.h);
            let raw_gaze_opt = crate::eyetracking::get_latest_raw_gaze();

            if crate::eyetracking::is_eye_tracking_enabled() && self.numbered_targets.is_empty() && !self.is_scan_in_progress {
                #[cfg(windows)]
                {
                    let fg = unsafe { windows::Win32::UI::WindowsAndMessaging::GetForegroundWindow() };
                    if !fg.0.is_null() && !is_window_hung(fg) {
                        let mut fg_pid = 0u32;
                        unsafe { windows::Win32::UI::WindowsAndMessaging::GetWindowThreadProcessId(fg, Some(&mut fg_pid)) };
                        if fg_pid != std::process::id() {
                            self.refresh_numbered_targets_for_window(fg.0 as isize);
                        }
                    }
                }
                #[cfg(not(windows))]
                {
                    self.refresh_numbered_targets_for_window(1);
                }
            }

            let (norm_x, norm_y, yaw, pitch) = if let Some(m) = raw_gaze_opt {
                (m.eye_x, m.eye_y, m.head_yaw, m.head_pitch)
            } else {
                let raw_cursor = get_global_cursor_pos().unwrap_or(((work_area.x * ppp + screen_w * ppp / 2.0) as i32, (work_area.y * ppp + screen_h * ppp / 2.0) as i32));
                let local_x = (raw_cursor.0 as f32 / ppp) - work_area.x;
                let local_y = (raw_cursor.1 as f32 / ppp) - work_area.y;
                let nx = ((local_x / screen_w.max(1.0)).clamp(0.0, 1.0) - 0.5) * 2.0;
                let ny = ((local_y / screen_h.max(1.0)).clamp(0.0, 1.0) - 0.5) * 2.0;
                (nx, ny, 0.0, 0.0)
            };

            let (gaze_x, gaze_y, snapped_idx) = self.eye_tracker.process_gaze_input_scaled(
                norm_x,
                norm_y,
                yaw,
                pitch,
                screen_w,
                screen_h,
                (work_area.x, work_area.y),
                ppp,
                &self.numbered_targets,
            );

            if !self.eye_tracker.calibration.is_active && crate::eyetracking::is_eye_tracking_enabled() {
                if !self.eye_tracker.is_mouse_overridden() && !self.eye_tracker.is_frozen() {
                    let abs_x = ((work_area.x + gaze_x) * ppp).round() as i32;
                    let abs_y = ((work_area.y + gaze_y) * ppp).round() as i32;
                    self.hover_last_cursor_pos = Some((abs_x, abs_y));
                    self.eye_tracker.last_eye_applied_cursor = Some((abs_x, abs_y));
                    set_global_cursor_pos(abs_x, abs_y);
                } else if self.eye_tracker.is_mouse_overridden() {
                    self.eye_tracker.last_eye_applied_cursor = None;
                }

                if let Some(target_idx) = snapped_idx {
                    if self.switch_access_mode {
                        if let Some(target) = self.numbered_targets.iter().find(|t| t.index == target_idx) {
                            draw_screen_control_border(target.rect, false);
                        }
                    }
                    if self.switch_access_mode && self.hover_last_announced_target != Some(target_idx) {
                        if let Some(target) = self.numbered_targets.iter().find(|t| t.index == target_idx).cloned() {
                            self.hover_last_announced_target = Some(target_idx);
                            self.hover_cursor_still_since = Instant::now();
                            self.narrate_numbered_target(&target);
                        }
                    }
                } else if self.switch_access_mode {
                    set_overlay_focused_border(None);
                }
            }
            ctx.request_repaint();
        } else {
            crate::eyetracking::stop_camera_worker();
        }
    }

    /// Dépouillement des événements asynchrones de l'agent
    pub fn process_agent_events(&mut self, ctx: &egui::Context) {
        while let Ok(event) = self.event_receiver.try_recv() {
            match event {
                AgentEvent::DownloadProgress { label, percent, downloaded_mb, total_mb, speed_mb_s } => {
                    if percent >= 100.0 {
                        if let Some(prev) = self.active_download.take() {
                            self.chat_history.push(ChatEntry {
                                role: ChatRole::Agent,
                                text: format!("✅ Téléchargement terminé : **{}** ({:.1} Mo). Prêt à l'emploi.", prev.label, prev.total_mb),
                                timestamp: current_time_str(),
                                quick_suggestions: Vec::new(),
                                screen_size_kb: None,
                            });
                            play_earcon(Earcon::Success);
                        }
                    } else {
                        self.active_download = Some(ActiveDownloadState {
                            label,
                            percent,
                            downloaded_mb,
                            total_mb,
                            speed_mb_s,
                        });
                    }
                    ctx.request_repaint();
                }
                AgentEvent::StatusChanged(new_status) => {
                    if self.status != AgentStatus::EmergencyStopped || new_status == AgentStatus::EmergencyStopped {
                        let prev_status = self.status;
                        let has_pending_twitch = self.active_tab == ActiveTab::Chat && !self.pending_twitch_speech.is_empty();
                        if !has_pending_twitch {
                            self.status = new_status;
                        }
                        if new_status == AgentStatus::Listening {
                            self.is_recording = true;
                        } else if new_status == AgentStatus::Idle {
                            self.is_recording = false;
                            if !has_pending_twitch {
                                if !self.mic_manually_disabled
                                    && (self.dictation_mode
                                    || (prev_status == AgentStatus::Thinking && self.continuous_mode)
                                    || (prev_status == AgentStatus::Speaking && self.continuous_mode))
                                    && !crate::audio::is_live_subtitles_active()
                                    && self.whisper_ready
                                    && !self.show_api_key_modal
                                    && !self.voice_is_recording_sample
                                    && self.status != AgentStatus::EmergencyStopped
                                    && (!self.is_hidden || self.switch_access_mode)
                                    && !AGENT_BUSY.load(Ordering::SeqCst)
                                {
                                    self.start_recording(ctx, false);
                                }
                            }
                        }
                    }
                }
                AgentEvent::ScreenPayloadSize(kb) => {
                    if let Some(entry) = self.chat_history.iter_mut().rev().find(|e| e.role == ChatRole::User) {
                        entry.screen_size_kb = Some(kb);
                    }
                }
                AgentEvent::NarrationStart => {
                    self.is_streaming_narration = true;
                    self.auto_scroll_to_bottom = true;
                    self.chat_history.push(ChatEntry {
                        role: ChatRole::Agent,
                        text: String::new(),
                        timestamp: current_time_str(),
                        quick_suggestions: Vec::new(),
                        screen_size_kb: None,
                    });
                }
                AgentEvent::NarrationChunk(chunk) => {
                    self.auto_scroll_to_bottom = true;
                    if self.is_streaming_narration {
                        if let Some(last) = self.chat_history.last_mut() {
                            if matches!(last.role, ChatRole::Agent) {
                                last.text.push_str(&chunk);
                            }
                        }
                    } else if let Some(last) = self.chat_history.last_mut() {
                        if matches!(last.role, ChatRole::Agent) {
                            last.text.push_str(&chunk);
                        } else {
                            self.chat_history.push(ChatEntry {
                                role: ChatRole::Agent,
                                text: chunk,
                                timestamp: current_time_str(),
                                quick_suggestions: Vec::new(),
                                screen_size_kb: None,
                            });
                        }
                    } else {
                        self.chat_history.push(ChatEntry {
                            role: ChatRole::Agent,
                            text: chunk,
                            timestamp: current_time_str(),
                            quick_suggestions: Vec::new(),
                            screen_size_kb: None,
                        });
                    }
                }
                AgentEvent::ReplaceNarration { text, quick_suggestions } => {
                    let was_streaming = self.is_streaming_narration;
                    self.auto_scroll_to_bottom = true;
                    self.is_streaming_narration = false;

                    if text.trim().is_empty() && quick_suggestions.is_empty() {
                        continue;
                    }
                    if text.trim().is_empty() {
                        if let Some(last) = self.chat_history.last_mut().filter(|e| e.role == ChatRole::Agent) {
                            for sug in &quick_suggestions {
                                if !last.quick_suggestions.iter().any(|s| s.as_str() == sug.as_str()) {
                                    last.quick_suggestions.push(sug.clone());
                                }
                            }
                        }
                        if self.auto_read_suggestions && self.auto_read_agent_messages && self.active_tab == ActiveTab::Assistance {
                            if let Some(sug_speech) = format_suggestions_for_speech(&quick_suggestions) {
                                if !sanitize_for_tts(&sug_speech).trim().is_empty() {
                                    self.status = AgentStatus::Speaking;
                                    let _ = self.tts_sender.send(TtsCommand::Enqueue(sug_speech));
                                }
                            }
                        }
                    } else if was_streaming || self.llm_config.provider == LlmProvider::Local {
                        if let Some(last) = self.chat_history.last_mut().filter(|e| e.role == ChatRole::Agent) {
                            last.text = text.clone();
                            last.quick_suggestions = quick_suggestions.clone();
                        } else {
                            self.chat_history.push(ChatEntry {
                                role: ChatRole::Agent,
                                text: text.clone(),
                                timestamp: current_time_str(),
                                quick_suggestions: quick_suggestions.clone(),
                                screen_size_kb: None,
                            });
                        }
                        if self.auto_read_suggestions && self.auto_read_agent_messages && self.active_tab == ActiveTab::Assistance {
                            if let Some(sug_speech) = format_suggestions_for_speech(&quick_suggestions) {
                                if !sanitize_for_tts(&sug_speech).trim().is_empty() {
                                    self.status = AgentStatus::Speaking;
                                    let _ = self.tts_sender.send(TtsCommand::Enqueue(sug_speech));
                                }
                            }
                        }
                    } else {
                        let sug_speech_opt = if self.auto_read_suggestions && self.auto_read_agent_messages {
                            format_suggestions_for_speech(&quick_suggestions)
                        } else {
                            None
                        };

                        let full_speech = match (self.auto_read_agent_messages, sug_speech_opt) {
                            (true, Some(sug_speech)) => {
                                if !sanitize_for_tts(&text).trim().is_empty() {
                                    format!("{text}\n\n{sug_speech}")
                                } else {
                                    sug_speech
                                }
                            }
                            (true, None) => text.clone(),
                            (false, Some(sug_speech)) => sug_speech,
                            (false, None) => String::new(),
                        };
                        if self.active_tab == ActiveTab::Assistance {
                            if !sanitize_for_tts(&full_speech).trim().is_empty() {
                                self.status = AgentStatus::Speaking;
                                let _ = self.tts_sender.send(TtsCommand::Speak(full_speech));
                            }
                        }
                        self.chat_history.push(ChatEntry {
                            role: ChatRole::Agent,
                            text,
                            timestamp: current_time_str(),
                            quick_suggestions,
                            screen_size_kb: None,
                        });
                    }
                }
                AgentEvent::ReminderTriggered { label, prompt: _ } => {
                    self.is_hidden = false;
                    self.restore_window_if_minimized(ctx);
                    let reminder_display = format!("⏰ **Rappel :** {label}");
                    self.chat_history.push(ChatEntry {
                        role: ChatRole::Agent,
                        text: reminder_display,
                        timestamp: current_time_str(),
                        quick_suggestions: vec![
                            QuickSuggestionItem::Text("Merci !".to_string()),
                            QuickSuggestionItem::Text("Rappelle-moi dans 5 minutes".to_string()),
                        ],
                        screen_size_kb: None,
                    });
                    ctx.request_repaint();
                }
                AgentEvent::SilentNarration(full_text) => {
                    self.chat_history.push(ChatEntry {
                        role: ChatRole::Agent,
                        text: full_text,
                        timestamp: current_time_str(),
                        quick_suggestions: Vec::new(),
                        screen_size_kb: None,
                    });
                }
                AgentEvent::TranscriptionPartial(text) => {
                    if self.voice_is_recording_sample {
                        continue;
                    }
                    let is_status_msg = text == "Écoute de votre voix..." || text == "Transcription locale en cours...";
                    if !is_status_msg && !text.trim().is_empty() && !self.switch_access_mode {
                        self.stop_speaking();
                    }
                    if self.is_recording && (!self.is_hidden || self.switch_access_mode) {
                        self.live_transcript = text;
                    }
                }
                AgentEvent::VoicePromptReady(prompt) => {
                    self.handle_voice_prompt_ready(ctx, prompt);
                }
                AgentEvent::TtsFinished => {
                    if self.active_tab == ActiveTab::Chat {
                        if let Some(recent_speech) = self.pending_twitch_speech.pop_front() {
                            self.status = AgentStatus::Speaking;
                            let _ = self.tts_sender.send(TtsCommand::Speak(recent_speech));
                            continue;
                        }
                    }
                    if !self.mic_manually_disabled
                        && self.continuous_mode
                        && !crate::audio::is_live_subtitles_active()
                        && self.status != AgentStatus::EmergencyStopped
                        && !self.show_api_key_modal
                        && !self.voice_is_recording_sample
                        && !AGENT_BUSY.load(Ordering::SeqCst)
                        && self.whisper_ready
                    {
                        self.start_recording(ctx, true);
                    }
                }
                AgentEvent::WhisperReady => {
                    println!("[Whisper] Notification WhisperReady reçue.");
                    self.whisper_ready = true;
                    let is_user_typing = ctx.memory(|m| {
                        m.has_focus(egui::Id::NULL.with("prompt_input_text"))
                            || m.has_focus(egui::Id::NULL.with("api_key_input_field"))
                            || m.has_focus(egui::Id::NULL.with("model_input_field"))
                    });
                    if !self.mic_manually_disabled
                        && self.continuous_mode
                        && self.status != AgentStatus::EmergencyStopped
                        && self.status != AgentStatus::Speaking
                        && !self.show_api_key_modal
                        && !self.voice_is_recording_sample
                        && !AGENT_BUSY.load(Ordering::SeqCst)
                        && !is_user_typing
                    {
                        self.start_recording(ctx, true);
                    }
                    ctx.request_repaint();
                }
                AgentEvent::TwitchChatReceived(twitch_msg) => {
                    self.auto_scroll_to_bottom = true;
                    if self.active_tab == ActiveTab::Chat {
                        let speech = format!("{} : {}", twitch_msg.author, twitch_msg.text);
                        if self.status == AgentStatus::Speaking || !self.pending_twitch_speech.is_empty() {
                            const MAX_PENDING_TWITCH: usize = 30;
                            if self.pending_twitch_speech.len() >= MAX_PENDING_TWITCH {
                                self.pending_twitch_speech.pop_front();
                            }
                            self.pending_twitch_speech.push_back(speech);
                        } else {
                            self.status = AgentStatus::Speaking;
                            let _ = self.tts_sender.send(TtsCommand::Speak(speech));
                        }
                    }
                    self.twitch_messages.push(twitch_msg);
                    if self.twitch_messages.len() > 300 {
                        self.twitch_messages.drain(0..self.twitch_messages.len() - 300);
                    }
                }
                AgentEvent::TwitchSearchResults(results) => {
                    self.twitch_search_results = results;
                }
                AgentEvent::OpenTwitchChannel { channel, open_stream } => {
                    self.stop_speaking();
                    self.active_tab = ActiveTab::Chat;
                    let clean = channel.trim().trim_start_matches('#').to_lowercase();
                    if !clean.is_empty() {
                        self.current_twitch_channel = clean.clone();
                        let _ = self.twitch_channel_sender.send(clean.clone());
                        self.twitch_messages.clear();
                        self.twitch_search_results.clear();
                        if open_stream {
                            open_browser_smart(&format!("https://twitch.tv/{}", clean));
                        }
                    }
                }
                AgentEvent::SwitchTab(tab) => {
                    self.pending_twitch_speech.clear();
                    self.stop_speaking();
                    self.active_tab = tab;
                }
                AgentEvent::RequestIgnored => {
                    if let Some(last) = self.chat_history.last() {
                        if last.role == ChatRole::User {
                            self.chat_history.pop();
                        }
                    }
                    self.live_transcript.clear();
                    if !self.mic_manually_disabled && self.continuous_mode && self.status != AgentStatus::EmergencyStopped {
                        self.start_recording(ctx, false);
                    }
                }
                AgentEvent::RequestApiKey => {
                    self.sync_modal_inputs_for_provider(self.llm_config.provider);
                    self.show_api_key_modal = true;
                    self.is_hidden = false;
                }
                AgentEvent::ShowFloatingCard(card) => {
                    let vp_id = egui::ViewportId::from_hash_of(&card.id);
                    let card_title = card.title.clone();
                    let card_speech = card.build_narrator_speech();
                    if let Some(existing) = self.floating_cards.iter_mut().find(|c| c.id == card.id) {
                        *existing = card;
                    } else {
                        self.floating_cards.push(card);
                    }
                    update_open_floating_cards(&self.floating_cards);
                    let read_suggestion = t!("ui.read_card_prefix", title = card_title).to_string();
                    if let Some(last_entry) = self.chat_history.last_mut() {
                        if !last_entry.quick_suggestions.iter().any(|s| s.as_str() == Some(&read_suggestion)) {
                            last_entry.quick_suggestions.insert(0, QuickSuggestionItem::Text(read_suggestion));
                        }
                    } else {
                        self.chat_history.push(ChatEntry {
                            role: ChatRole::Agent,
                            text: t!("ui.card_displayed", title = card_title).to_string(),
                            timestamp: current_time_str(),
                            quick_suggestions: vec![QuickSuggestionItem::Text(read_suggestion)],
                            screen_size_kb: None,
                        });
                    }
                    if self.status != AgentStatus::Speaking && !card_speech.trim().is_empty() {
                        self.status = AgentStatus::Speaking;
                        let _ = self.tts_sender.send(TtsCommand::Speak(card_speech));
                    }
                    ctx.request_repaint();
                    ctx.request_repaint_of(vp_id);
                }
                AgentEvent::CloseFloatingCard(id) => {
                    let vp_id = egui::ViewportId::from_hash_of(&id);
                    if let Some(card) = self.floating_cards.iter().find(|c| c.id == id) {
                        if matches!(card.content, FloatingContentType::Subtitles { .. }) {
                            floating_window::stop_live_subtitles();
                            crate::audio::stop_live_subtitles();
                        }
                    }
                    self.floating_cards.retain(|c| c.id != id);
                    update_open_floating_cards(&self.floating_cards);
                    ctx.request_repaint();
                    ctx.request_repaint_of(vp_id);
                }
                AgentEvent::ClickNumberedTarget(idx) => {
                    self.click_numbered_target_by_index(idx);
                }
                AgentEvent::ToggleNumberedTargets(enabled) => {
                    self.toggle_switch_access_mode(enabled);
                }
                AgentEvent::NextNumberedChunk => {
                    let total = self.numbered_targets.len();
                    if total > 50 {
                        self.numbered_chunk_offset = (self.numbered_chunk_offset + 50) % total;
                        let end = (self.numbered_chunk_offset + 50).min(total);
                        let slice = &self.numbered_targets[self.numbered_chunk_offset..end];
                        let focused_idx = slice.first().map(|t| t.index);
                        draw_screen_numbered_badges(slice, self.zoom_factor, focused_idx);
                        let msg = format!("Affichage des badges {} à {}.", self.numbered_chunk_offset + 1, end);
                        let _ = self.tts_sender.send(TtsCommand::Speak(msg));
                        ctx.request_repaint();
                    }
                }
                AgentEvent::PreviousNumberedChunk => {
                    let total = self.numbered_targets.len();
                    if total > 0 {
                        self.numbered_chunk_offset = if self.numbered_chunk_offset >= 50 {
                            self.numbered_chunk_offset - 50
                        } else {
                            (total / 50) * 50
                        };
                        if self.numbered_chunk_offset >= total {
                            self.numbered_chunk_offset = 0;
                        }
                        let end = (self.numbered_chunk_offset + 50).min(total);
                        let slice = &self.numbered_targets[self.numbered_chunk_offset..end];
                        let focused_idx = slice.first().map(|t| t.index);
                        draw_screen_numbered_badges(slice, self.zoom_factor, focused_idx);
                        let msg = format!("Affichage des badges {} à {}.", self.numbered_chunk_offset + 1, end);
                        let _ = self.tts_sender.send(TtsCommand::Speak(msg));
                        ctx.request_repaint();
                    }
                }
                AgentEvent::LiveSubtitleUpdate { text, spectrum } => {
                    if !crate::audio::is_live_subtitles_active() {
                        continue;
                    }
                    floating_window::update_live_subtitles(text, spectrum);
                    for card in &self.floating_cards {
                        if matches!(card.content, FloatingContentType::Subtitles { .. }) {
                            let vp_id = egui::ViewportId::from_hash_of(&card.id);
                            ctx.request_repaint_of(vp_id);
                        }
                    }
                }
                AgentEvent::LiveSubtitlesClosed => {
                    floating_window::stop_live_subtitles();
                    crate::audio::stop_live_subtitles();
                    self.floating_cards.retain(|c| !matches!(c.content, FloatingContentType::Subtitles { .. }));
                    update_open_floating_cards(&self.floating_cards);
                    ctx.request_repaint();
                }
                AgentEvent::NavigationZonesReady { window_id, window_title: _, summary, zones } => {
                    self.window_analysis_cache.insert(window_id, CachedWindowAnalysis {
                        summary: summary.clone(),
                        zones: zones.clone(),
                    });

                    if self.switch_access_mode && self.switch_active_window_id == Some(window_id) {
                        self.switch_zones = zones;
                        if self.switch_step == SwitchAccessStep::Elements {
                            if let Some(first_zone) = self.switch_zones.first() {
                                let mut active_targets = first_zone.targets.clone();
                                for (i, t) in active_targets.iter_mut().enumerate() {
                                    t.index = i + 1;
                                }
                                self.numbered_targets = active_targets;
                            }
                        } else if self.switch_step == SwitchAccessStep::Windows {
                            let all_targets: Vec<NumberedElementTarget> = self.switch_zones.iter().flat_map(|z| z.targets.clone()).collect();
                            if !all_targets.is_empty() {
                                self.numbered_targets = all_targets;
                            }
                            self.sync_switch_access_overlays();
                        } else if self.switch_step == SwitchAccessStep::Zones {
                            let zone_infos: Vec<ZoneOverlayInfo> = self.switch_zones.iter().enumerate().map(|(i, z)| {
                                ZoneOverlayInfo {
                                    index: i + 1,
                                    title: z.title.clone(),
                                    rect: z.rect,
                                    is_focused: i == (self.switch_focus_index % self.switch_zones.len().max(1)),
                                }
                            }).collect();
                            if let Some(cur_zone) = self.switch_zones.get(self.switch_focus_index % self.switch_zones.len().max(1)) {
                                let mut active_targets = cur_zone.targets.clone();
                                for (i, t) in active_targets.iter_mut().enumerate() {
                                    t.index = i + 1;
                                }
                                self.numbered_targets = active_targets;
                            }
                            let focused_target_idx = self.numbered_targets.first().map(|t| t.index);
                            draw_screen_zones_and_targets(&zone_infos, &self.numbered_targets, self.zoom_factor, focused_target_idx);
                        }
                        ctx.request_repaint();
                    }
                }
            }
        }
    }

    fn handle_voice_prompt_ready(&mut self, ctx: &egui::Context, prompt: String) {
        if self.voice_is_recording_sample {
            return;
        }
        self.is_recording = false;
        self.live_transcript.clear();
        let is_user_typing = ctx.memory(|m| m.has_focus(egui::Id::NULL.with("prompt_input_text")))
            && !self.input_text.trim().is_empty();
        if is_user_typing {
            if self.continuous_mode && self.status != AgentStatus::EmergencyStopped {
                self.start_recording(ctx, false);
            }
            return;
        }
        self.stop_speaking();

        let raw_prompt = prompt.clone();
        let prompt = self.expand_macro(&raw_prompt);
        if prompt != raw_prompt {
            println!("[Macro] Déploiement vocal : \"{}\"", prompt);
        }

        let norm_prompt = normalize_for_command_match(&prompt);
        if norm_prompt.trim().is_empty() {
            println!("[Voice] Bruit parasite ignoré (aucun mot détecté) : aucune requête LLM.");
            if !self.mic_manually_disabled && self.continuous_mode && self.status != AgentStatus::EmergencyStopped {
                self.start_recording(ctx, false);
            }
            return;
        }

        if norm_prompt == "coupe le micro"
            || norm_prompt == "couper le micro"
            || norm_prompt == "eteins le micro"
            || norm_prompt == "éteins le micro"
            || norm_prompt == "eteindre le micro"
            || norm_prompt == "éteindre le micro"
            || norm_prompt == "desactive le micro"
            || norm_prompt == "désactive le micro"
            || norm_prompt == "desactiver le micro"
            || norm_prompt == "désactiver le micro"
            || norm_prompt == "arrete le micro"
            || norm_prompt == "arrête le micro"
            || norm_prompt == "ferme le micro"
            || norm_prompt == "mute micro"
            || norm_prompt == "mute mic"
            || norm_prompt == "stop micro"
            || norm_prompt == "stop mic"
        {
            self.mic_manually_disabled = true;
            self.stop_recording();
            play_earcon(Earcon::MicClose);
            let reply = "Microphone éteint.".to_string();
            self.chat_history.push(ChatEntry {
                role: ChatRole::Agent,
                text: reply.clone(),
                timestamp: current_time_str(),
                quick_suggestions: vec![QuickSuggestionItem::Text("Activer le micro".to_string())],
                screen_size_kb: None,
            });
            let _ = self.tts_sender.send(TtsCommand::Speak(reply));
            return;
        }
        if norm_prompt == "active le micro"
            || norm_prompt == "activer le micro"
            || norm_prompt == "allume le micro"
            || norm_prompt == "allumer le micro"
            || norm_prompt == "unmute micro"
            || norm_prompt == "unmute mic"
        {
            self.mic_manually_disabled = false;
            self.start_recording(ctx, true);
            let reply = "Microphone activé.".to_string();
            self.chat_history.push(ChatEntry {
                role: ChatRole::Agent,
                text: reply.clone(),
                timestamp: current_time_str(),
                quick_suggestions: vec![QuickSuggestionItem::Text("Couper le micro".to_string())],
                screen_size_kb: None,
            });
            let _ = self.tts_sender.send(TtsCommand::Speak(reply));
            return;
        }

        let target_lang = if norm_prompt.contains("passer en anglais") || norm_prompt.contains("switch to english") || norm_prompt.contains("langue anglaise") || norm_prompt.contains("in english") {
            Some("en")
        } else if norm_prompt.contains("passer en francais") || norm_prompt.contains("passer en français")
            || norm_prompt.contains("switch to french") || norm_prompt.contains("langue francaise") || norm_prompt.contains("langue française") || norm_prompt.contains("en francais") || norm_prompt.contains("en français") {
            Some("fr")
        } else if norm_prompt.contains("passer en allemand") || norm_prompt.contains("switch to german") || norm_prompt.contains("auf deutsch") || norm_prompt.contains("langue allemande") || norm_prompt.contains("in german") {
            Some("de")
        } else if norm_prompt.contains("passer en espagnol") || norm_prompt.contains("switch to spanish") || norm_prompt.contains("en español") || norm_prompt.contains("en espanol") || norm_prompt.contains("langue espagnole") || norm_prompt.contains("in spanish") {
            Some("es")
        } else if norm_prompt.contains("passer en italien") || norm_prompt.contains("switch to italian") || norm_prompt.contains("in italiano") || norm_prompt.contains("langue italienne") {
            Some("it")
        } else if norm_prompt.contains("passer en portugais") || norm_prompt.contains("switch to portuguese") || norm_prompt.contains("em português") || norm_prompt.contains("em portugues") || norm_prompt.contains("langue portugaise") {
            Some("pt")
        } else if norm_prompt.contains("passer en russe") || norm_prompt.contains("switch to russian") || norm_prompt.contains("по-русски") || norm_prompt.contains("langue russe") {
            Some("ru")
        } else if norm_prompt.contains("passer en japonais") || norm_prompt.contains("switch to japanese") || norm_prompt.contains("日本語") || norm_prompt.contains("langue japonaise") {
            Some("ja")
        } else if norm_prompt.contains("passer en chinois") || norm_prompt.contains("switch to chinese") || norm_prompt.contains("中文") || norm_prompt.contains("langue chinoise") {
            Some("zh")
        } else {
            None
        };

        if let Some(target_code) = target_lang {
            play_earcon(Earcon::Success);
            self.set_app_language(target_code);
            if self.continuous_mode && self.status != AgentStatus::EmergencyStopped {
                self.start_recording(ctx, false);
            }
            return;
        }

        if norm_prompt.contains("theme suivant") || norm_prompt.contains("thème suivant") || norm_prompt.contains("change de theme") || norm_prompt.contains("change de thème") || norm_prompt.contains("changer de theme") || norm_prompt.contains("changer de thème") {
            play_earcon(Earcon::Success);
            self.set_theme(self.theme.next());
            if self.continuous_mode && self.status != AgentStatus::EmergencyStopped {
                self.start_recording(ctx, false);
            }
            return;
        }
        if norm_prompt.contains("contraste eleve") || norm_prompt.contains("contraste élevé") || norm_prompt.contains("high contrast") {
            play_earcon(Earcon::Success);
            self.set_theme(AppTheme::HighContrastYellow);
            if self.continuous_mode && self.status != AgentStatus::EmergencyStopped {
                self.start_recording(ctx, false);
            }
            return;
        }
        if norm_prompt.contains("theme sombre") || norm_prompt.contains("thème sombre") || norm_prompt.contains("theme par defaut") || norm_prompt.contains("thème par défaut") {
            play_earcon(Earcon::Success);
            self.set_theme(AppTheme::CyberCyan);
            if self.continuous_mode && self.status != AgentStatus::EmergencyStopped {
                self.start_recording(ctx, false);
            }
            return;
        }

        if norm_prompt.contains("voix suivante") || norm_prompt.contains("autre voix") || norm_prompt.contains("change de voix") || norm_prompt.contains("changer voix") || norm_prompt.contains("changer de voix") {
            play_earcon(Earcon::Success);
            self.set_voice_gender(self.voice_gender.next());
            if self.continuous_mode && self.status != AgentStatus::EmergencyStopped {
                self.start_recording(ctx, false);
            }
            return;
        }
        if norm_prompt.contains("voix clone") || norm_prompt.contains("voix clonée") || norm_prompt == "clone" {
            play_earcon(Earcon::Success);
            self.set_voice_gender(VoiceProfile::Clone);
            if self.continuous_mode && self.status != AgentStatus::EmergencyStopped {
                self.start_recording(ctx, false);
            }
            return;
        }
        if norm_prompt.contains("voix de denise") || norm_prompt == "denise" || norm_prompt.contains("voix feminine") || norm_prompt.contains("voix féminine") {
            play_earcon(Earcon::Success);
            self.set_voice_gender(VoiceProfile::Denise);
            if self.continuous_mode && self.status != AgentStatus::EmergencyStopped {
                self.start_recording(ctx, false);
            }
            return;
        }
        if norm_prompt.contains("voix d henri") || norm_prompt.contains("voix d'henri") || norm_prompt == "henri" || norm_prompt.contains("voix masculine") {
            play_earcon(Earcon::Success);
            self.set_voice_gender(VoiceProfile::Henri);
            if self.continuous_mode && self.status != AgentStatus::EmergencyStopped {
                self.start_recording(ctx, false);
            }
            return;
        }
        if norm_prompt.contains("voix d hortense") || norm_prompt.contains("voix d'hortense") || norm_prompt.contains("voix de hortense") || norm_prompt == "hortense" {
            play_earcon(Earcon::Success);
            self.set_voice_gender(VoiceProfile::Hortense);
            if self.continuous_mode && self.status != AgentStatus::EmergencyStopped {
                self.start_recording(ctx, false);
            }
            return;
        }
        if norm_prompt.contains("voix de paul") || norm_prompt == "paul" {
            play_earcon(Earcon::Success);
            self.set_voice_gender(VoiceProfile::Paul);
            if self.continuous_mode && self.status != AgentStatus::EmergencyStopped {
                self.start_recording(ctx, false);
            }
            return;
        }
        if norm_prompt.contains("voix de sylvie") || norm_prompt == "sylvie" || norm_prompt.contains("voix quebecoise") || norm_prompt.contains("voix québécoise") || norm_prompt.contains("voix canadienne") {
            play_earcon(Earcon::Success);
            self.set_voice_gender(VoiceProfile::Sylvie);
            if self.continuous_mode && self.status != AgentStatus::EmergencyStopped {
                self.start_recording(ctx, false);
            }
            return;
        }
        if norm_prompt.contains("voix de jean") || norm_prompt == "jean" {
            play_earcon(Earcon::Success);
            self.set_voice_gender(VoiceProfile::Jean);
            if self.continuous_mode && self.status != AgentStatus::EmergencyStopped {
                self.start_recording(ctx, false);
            }
            return;
        }

        if norm_prompt.contains("boite a rythme")
            || norm_prompt.contains("boîte à rythme")
            || norm_prompt.contains("instrument")
            || norm_prompt.contains("mode musique")
            || norm_prompt.contains("musique adaptee")
            || norm_prompt.contains("musique adaptée")
            || norm_prompt.contains("lance la musique")
            || norm_prompt.contains("ouvre la musique")
            || norm_prompt.contains("mini jeu")
            || norm_prompt.contains("minijeu")
            || norm_prompt.contains("joue de la musique")
        {
            self.open_instrument_card();
            let msg = t!("instrument.opened_speech").to_string();
            self.chat_history.push(ChatEntry {
                role: ChatRole::Agent,
                text: msg.clone(),
                timestamp: current_time_str(),
                quick_suggestions: vec![QuickSuggestionItem::Text(t!("instrument.close_suggestion").to_string())],
                screen_size_kb: None,
            });
            let _ = self.tts_sender.send(TtsCommand::Speak(msg));
            return;
        }
        if norm_prompt.contains("clavier virtuel")
            || norm_prompt.contains("ouvre le clavier")
            || norm_prompt.contains("afficher le clavier")
            || norm_prompt.contains("mode clavier")
            || norm_prompt.contains("virtual keyboard")
        {
            self.open_keyboard_card();
            let msg = "Clavier virtuel ouvert.".to_string();
            self.chat_history.push(ChatEntry { role: ChatRole::Agent, text: msg.clone(), timestamp: current_time_str(), quick_suggestions: Vec::new(), screen_size_kb: None });
            let _ = self.tts_sender.send(TtsCommand::Speak(msg));
            return;
        }

        if norm_prompt == "mode sommeil" || norm_prompt == "mode nuit" || norm_prompt == "bonne nuit" || norm_prompt == "active le mode sommeil" {
            let res = crate::home_automation::execute_medical_scene("sleep");
            play_earcon(Earcon::Success);
            self.chat_history.push(ChatEntry {
                role: ChatRole::User,
                text: prompt,
                timestamp: current_time_str(),
                quick_suggestions: vec![QuickSuggestionItem::Text("Mode repas / travail".to_string())],
                screen_size_kb: None,
            });
            self.chat_history.push(ChatEntry {
                role: ChatRole::Agent,
                text: res.clone(),
                timestamp: current_time_str(),
                quick_suggestions: vec![QuickSuggestionItem::Text("Mode repas / travail".to_string())],
                screen_size_kb: None,
            });
            let _ = self.tts_sender.send(TtsCommand::Speak(res));
            return;
        }
        if norm_prompt == "mode repas" || norm_prompt == "mode travail" || norm_prompt == "active le mode repas" || norm_prompt == "active le mode travail" {
            let res = crate::home_automation::execute_medical_scene("meal_work");
            play_earcon(Earcon::Success);
            self.chat_history.push(ChatEntry {
                role: ChatRole::User,
                text: prompt,
                timestamp: current_time_str(),
                quick_suggestions: vec![QuickSuggestionItem::Text("Mode sommeil".to_string())],
                screen_size_kb: None,
            });
            self.chat_history.push(ChatEntry {
                role: ChatRole::Agent,
                text: res.clone(),
                timestamp: current_time_str(),
                quick_suggestions: vec![QuickSuggestionItem::Text("Mode sommeil".to_string())],
                screen_size_kb: None,
            });
            let _ = self.tts_sender.send(TtsCommand::Speak(res));
            return;
        }

        if norm_prompt.contains("grossis") || norm_prompt.contains("zoom plus") || norm_prompt.contains("agrandis") || norm_prompt.contains("zoom in")
            || norm_prompt.contains("police plus grande") || norm_prompt.contains("texte plus grand") || norm_prompt.contains("agrandir la police") || norm_prompt.contains("agrandir le texte") {
            self.set_zoom_factor(self.zoom_factor + 0.15);
            play_earcon(Earcon::Success);
            if self.continuous_mode && self.status != AgentStatus::EmergencyStopped {
                self.start_recording(ctx, false);
            }
            return;
        }
        if norm_prompt.contains("zoom moins") || norm_prompt.contains("retrecis") || norm_prompt.contains("rétrécis") || norm_prompt.contains("zoom out")
            || norm_prompt.contains("police plus petite") || norm_prompt.contains("texte plus petit") || norm_prompt.contains("diminuer la police") || norm_prompt.contains("diminuer le texte") {
            self.set_zoom_factor(self.zoom_factor - 0.15);
            play_earcon(Earcon::Success);
            if self.continuous_mode && self.status != AgentStatus::EmergencyStopped {
                self.start_recording(ctx, false);
            }
            return;
        }
        if norm_prompt.contains("zoom normal") || norm_prompt.contains("taille normale") || norm_prompt.contains("reset zoom")
            || norm_prompt.contains("police normale") || norm_prompt.contains("texte normal") || norm_prompt.contains("reinitialiser police") || norm_prompt.contains("réinitialiser police") {
            self.set_zoom_factor(1.0);
            play_earcon(Earcon::Success);
            if self.continuous_mode && self.status != AgentStatus::EmergencyStopped {
                self.start_recording(ctx, false);
            }
            return;
        }

        if self.switch_access_mode {
            let parsed_idx = extract_badge_number(&norm_prompt);
            if let Some(idx) = parsed_idx {
                self.chat_history.push(ChatEntry {
                    role: ChatRole::User,
                    text: prompt,
                    timestamp: current_time_str(),
                    quick_suggestions: Vec::new(),
                    screen_size_kb: None,
                });
                self.click_numbered_target_by_index(idx);
                if self.continuous_mode && self.status != AgentStatus::EmergencyStopped {
                    self.start_recording(ctx, false);
                }
                return;
            }
        }

        if let Some(card) = is_read_card_intent(&norm_prompt, &self.floating_cards) {
            let speech = card.build_narrator_speech();
            self.stop_speaking();
            play_earcon(Earcon::Success);
            self.status = AgentStatus::Speaking;
            self.chat_history.push(ChatEntry {
                role: ChatRole::User,
                text: prompt,
                timestamp: current_time_str(),
                quick_suggestions: Vec::new(),
                screen_size_kb: None,
            });
            self.chat_history.push(ChatEntry {
                role: ChatRole::Agent,
                text: speech.clone(),
                timestamp: current_time_str(),
                quick_suggestions: Vec::new(),
                screen_size_kb: None,
            });
            let _ = self.tts_sender.send(TtsCommand::Speak(speech));
            if self.continuous_mode && self.status != AgentStatus::EmergencyStopped {
                self.start_recording(ctx, false);
            }
            return;
        }

        let (_locale, _) = get_system_locale_language();
        if is_switch_access_command(&prompt) {
            mark_hover_nav_asked();
            self.toggle_switch_access_mode(None);
            if self.continuous_mode && self.status != AgentStatus::EmergencyStopped {
                self.start_recording(ctx, false);
            }
            return;
        }

        if is_start_subtitles_command(&norm_prompt) {
            self.start_live_subtitles(None);
            if self.continuous_mode && self.status != AgentStatus::EmergencyStopped {
                self.start_recording(ctx, false);
            }
            return;
        }
        if is_stop_subtitles_command(&norm_prompt) {
            self.stop_live_subtitles();
            return;
        }

        if is_dismiss_hover_command(&prompt) {
            mark_hover_nav_asked();
            play_earcon(Earcon::Success);
            self.status = AgentStatus::Idle;
            let reply = t!("speech.dismiss_hover").to_string();
            self.chat_history.push(ChatEntry {
                role: ChatRole::User,
                text: prompt,
                timestamp: current_time_str(),
                quick_suggestions: Vec::new(),
                screen_size_kb: None,
            });
            self.chat_history.push(ChatEntry {
                role: ChatRole::Agent,
                text: reply.clone(),
                timestamp: current_time_str(),
                quick_suggestions: Vec::new(),
                screen_size_kb: None,
            });
            let _ = self.tts_sender.send(TtsCommand::Speak(reply));
            if self.continuous_mode && self.status != AgentStatus::EmergencyStopped {
                self.start_recording(ctx, false);
            }
            return;
        }

        if is_emergency_stop_command(&prompt) {
            self.trigger_emergency_stop(ctx);
            return;
        }

        if is_cancel_scheduled_command(&prompt) {
            let cancelled = cancel_all_scheduled_tasks();
            play_earcon(Earcon::Success);
            let reply = if cancelled > 0 {
                t!("speech.scheduled_tasks_cancelled_count", count = cancelled).to_string()
            } else {
                t!("speech.scheduled_tasks_cancelled").to_string()
            };
            self.chat_history.push(ChatEntry {
                role: ChatRole::User,
                text: prompt,
                timestamp: current_time_str(),
                quick_suggestions: Vec::new(),
                screen_size_kb: None,
            });
            self.chat_history.push(ChatEntry {
                role: ChatRole::Agent,
                text: reply.clone(),
                timestamp: current_time_str(),
                quick_suggestions: Vec::new(),
                screen_size_kb: None,
            });
            let _ = self.tts_sender.send(TtsCommand::Speak(reply));
            if self.continuous_mode && self.status != AgentStatus::EmergencyStopped {
                self.start_recording(ctx, false);
            }
            return;
        }

        if norm_prompt == "annule" || norm_prompt == "annuler" || norm_prompt == "undo" || norm_prompt == "reviens en arriere" || norm_prompt == "reviens en arrière" {
            let _ = CURRENT_REQUEST_ID.fetch_add(1, Ordering::SeqCst);
            let undo_feedback = if let Some(fb) = undo_last_system_action() {
                fb
            } else {
                #[cfg(windows)]
                send_hotkey(&[windows::Win32::UI::Input::KeyboardAndMouse::VIRTUAL_KEY(0x11)], windows::Win32::UI::Input::KeyboardAndMouse::VIRTUAL_KEY(0x5A));
                #[cfg(not(windows))]
                let _ = std::process::Command::new("xdotool").args(["key", "ctrl+z"]).status();
                t!("commands.last_action_undone").to_string()
            };
            play_earcon(Earcon::Success);
            self.chat_history.push(ChatEntry {
                role: ChatRole::User,
                text: prompt,
                timestamp: current_time_str(),
                quick_suggestions: Vec::new(),
                screen_size_kb: None,
            });
            self.chat_history.push(ChatEntry {
                role: ChatRole::Agent,
                text: undo_feedback.clone(),
                timestamp: current_time_str(),
                quick_suggestions: Vec::new(),
                screen_size_kb: None,
            });
            let _ = self.tts_sender.send(TtsCommand::Speak(undo_feedback));
            self.status = AgentStatus::Idle;
            return;
        }

        if self.dictation_mode && is_stop_dictation_command(&prompt) {
            self.exit_dictation_mode();
            return;
        }
        if !self.dictation_mode && is_start_dictation_command(&prompt) {
            self.enter_dictation_mode();
            return;
        }

        if let Some(feedback) = try_execute_local_navigation_shortcut(&prompt) {
            play_earcon(Earcon::Success);
            self.chat_history.push(ChatEntry {
                role: ChatRole::User,
                text: prompt,
                timestamp: current_time_str(),
                quick_suggestions: Vec::new(),
                screen_size_kb: None,
            });
            self.chat_history.push(ChatEntry {
                role: ChatRole::Agent,
                text: feedback.to_string(),
                timestamp: current_time_str(),
                quick_suggestions: Vec::new(),
                screen_size_kb: None,
            });
            let _ = self.tts_sender.send(TtsCommand::Speak(feedback.to_string()));
            if self.continuous_mode && self.status != AgentStatus::EmergencyStopped {
                self.start_recording(ctx, false);
            }
            return;
        }

        if is_api_key_command(&prompt) {
            self.sync_modal_inputs_for_provider(self.llm_config.provider);
            self.show_api_key_modal = true;
            self.is_hidden = false;
            let reply = t!("speech.api_key_modal_opened").to_string();
            self.chat_history.push(ChatEntry {
                role: ChatRole::Agent,
                text: reply.clone(),
                timestamp: current_time_str(),
                quick_suggestions: Vec::new(),
                screen_size_kb: None,
            });
            let _ = self.tts_sender.send(TtsCommand::Speak(reply));
            self.continuous_mode = false;
            return;
        }

        if self.is_hidden && !self.switch_access_mode {
            if is_show_command(&prompt) {
                self.is_hidden = false;
                self.status = AgentStatus::Idle;
                self.chat_history.push(ChatEntry {
                    role: ChatRole::User,
                    text: prompt,
                    timestamp: current_time_str(),
                    quick_suggestions: Vec::new(),
                    screen_size_kb: None,
                });
                let reply = t!("speech.overlay_restored").to_string();
                self.chat_history.push(ChatEntry {
                    role: ChatRole::Agent,
                    text: reply.clone(),
                    timestamp: current_time_str(),
                    quick_suggestions: Vec::new(),
                    screen_size_kb: None,
                });
                let _ = self.tts_sender.send(TtsCommand::Speak(reply));
                self.continuous_mode = true;
            } else if self.continuous_mode && self.status != AgentStatus::EmergencyStopped {
                self.start_recording(ctx, false);
            }
        } else {
            if is_hide_command(&prompt) {
                self.is_hidden = true;
                self.status = AgentStatus::Idle;
                self.chat_history.push(ChatEntry {
                    role: ChatRole::User,
                    text: prompt,
                    timestamp: current_time_str(),
                    quick_suggestions: Vec::new(),
                    screen_size_kb: None,
                });
                let reply = t!("speech.overlay_hidden").to_string();
                self.chat_history.push(ChatEntry {
                    role: ChatRole::Agent,
                    text: reply.clone(),
                    timestamp: current_time_str(),
                    quick_suggestions: Vec::new(),
                    screen_size_kb: None,
                });
                let _ = self.tts_sender.send(TtsCommand::Speak(reply));
                self.continuous_mode = true;
            } else if is_show_command(&prompt) {
                let reply = t!("speech.overlay_already_visible").to_string();
                self.chat_history.push(ChatEntry {
                    role: ChatRole::Agent,
                    text: reply.clone(),
                    timestamp: current_time_str(),
                    quick_suggestions: Vec::new(),
                    screen_size_kb: None,
                });
                let _ = self.tts_sender.send(TtsCommand::Speak(reply));
                self.continuous_mode = true;
            } else if !prompt.trim().is_empty() {
                self.status = AgentStatus::Thinking;
                self.continuous_mode = !self.mic_manually_disabled;
                self.input_text.clear();
                self.chat_history.push(ChatEntry {
                    role: ChatRole::User,
                    text: prompt.clone(),
                    timestamp: current_time_str(),
                    quick_suggestions: Vec::new(),
                    screen_size_kb: None,
                });
                let _ = self.command_sender.send(AgentCommand::Prompt(prompt));
            } else if self.continuous_mode && self.status != AgentStatus::EmergencyStopped {
                self.start_recording(ctx, false);
            }
        }
    }

    /// Traite l'ensemble des événements matériels et utilisateur (clavier, contacteur, numéros, défilement)
    pub fn process_input_events(&mut self, ctx: &egui::Context, is_typing: bool) {
        let mut hook_events = Vec::new();
        if let Ok(mut q) = SWITCH_KEY_QUEUE.lock() {
            hook_events.append(&mut *q);
        }

        let mut hook_digits = Vec::new();
        if let Ok(mut nq) = NUMBER_KEY_QUEUE.lock() {
            hook_digits.append(&mut *nq);
        }

        if !is_typing && self.switch_access_mode {
            ctx.input(|i| {
                for ev in &i.events {
                    if let egui::Event::Text(ref text) = ev {
                        for ch in text.chars() {
                            if ch.is_ascii_digit() {
                                hook_digits.push(ch);
                            }
                        }
                    }
                }
            });
        }

        let k_switch = KEY_BIND_SWITCH.load(Ordering::Relaxed);
        let k_dict = KEY_BIND_DICTATION.load(Ordering::Relaxed);
        let k_voice = KEY_BIND_VOICE.load(Ordering::Relaxed);
        let k_clear = KEY_BIND_CLEAR.load(Ordering::Relaxed);
        let check_egui_shortcut = |binding: u32| -> bool {
            let base_vk = binding & VK_KEY_MASK;
            let Some(k) = vk_to_egui_key(base_vk) else { return false };
            let modifiers = egui::Modifiers {
                ctrl: (binding & MOD_CTRL) != 0,
                shift: (binding & MOD_SHIFT) != 0,
                alt: (binding & MOD_ALT) != 0,
                ..Default::default()
            };
            ctx.input_mut(|i| i.consume_key(modifiers, k))
        };
        let egui_f7 = check_egui_shortcut(k_switch);
        let egui_f8 = check_egui_shortcut(k_dict);
        let egui_f9 = check_egui_shortcut(k_voice);
        let egui_f4 = check_egui_shortcut(k_clear);

        if hook_events.contains(&SwitchKeyEvent::GazeClick) {
            let is_eye_on = crate::eyetracking::is_eye_tracking_enabled() && !self.eye_tracker.calibration.is_active;
            if is_eye_on && !is_typing && self.last_eye_click_time.elapsed() >= Duration::from_millis(250) {
                self.last_eye_click_time = Instant::now();
                self.trigger_gaze_click();
                ctx.request_repaint();
            }
        }
        if hook_events.contains(&SwitchKeyEvent::StopSpeech) {
            self.stop_speaking();
            play_earcon(Earcon::Success);
            ctx.request_repaint();
        }
        if (egui_f8 || hook_events.contains(&SwitchKeyEvent::ToggleDictation))
            && self.last_switch_input.elapsed() >= Duration::from_millis(200)
        {
            self.last_switch_input = Instant::now();
            self.last_trackball_input = Instant::now();
            if self.dictation_mode {
                self.exit_dictation_mode();
            } else {
                self.enter_dictation_mode();
            }
            ctx.request_repaint();
        }
        if (egui_f7 || hook_events.contains(&SwitchKeyEvent::ToggleSwitch))
            && self.last_switch_input.elapsed() >= Duration::from_millis(200)
        {
            self.last_switch_input = Instant::now();
            self.last_trackball_input = Instant::now();
            self.toggle_switch_access_mode(None);
            ctx.request_repaint();
        }
        if (egui_f9 || hook_events.contains(&SwitchKeyEvent::ToggleVoice))
            && self.last_switch_input.elapsed() >= Duration::from_millis(200)
        {
            self.last_switch_input = Instant::now();
            self.last_trackball_input = Instant::now();
            self.set_voice_gender(self.voice_gender.next());
            ctx.request_repaint();
        }
        if (egui_f4 || hook_events.contains(&SwitchKeyEvent::Clear))
            && self.last_switch_input.elapsed() >= Duration::from_millis(200)
        {
            self.last_switch_input = Instant::now();
            self.last_trackball_input = Instant::now();
            let _ = self.command_sender.send(AgentCommand::ClearHistory);
            self.chat_history.clear();
            self.window_analysis_cache.clear();
            self.chat_history.push(ChatEntry {
                role: ChatRole::Agent,
                text: t!("commands.context_cleared").to_string(),
                timestamp: current_time_str(),
                quick_suggestions: Vec::new(),
                screen_size_kb: None,
            });
            self.live_transcript.clear();
            self.stop_speaking();
            ctx.request_repaint();
        }

        let hook_escape_hit = hook_events.contains(&SwitchKeyEvent::Escape)
            || (hook_events.contains(&SwitchKeyEvent::Back) && self.numbered_input_buffer.is_empty());

        let egui_backspace_hit = ctx.input(|i| {
            i.events.iter().any(|e| matches!(e, egui::Event::Key { key: egui::Key::Backspace, pressed: true, .. }))
        });
        let backspace_hit = (egui_backspace_hit && !is_typing) || hook_events.contains(&SwitchKeyEvent::Back);

        if backspace_hit && !self.numbered_input_buffer.is_empty() {
            self.numbered_input_buffer.pop();
            self.numbered_input_last_char_time = Instant::now();
            ctx.request_repaint();
        }

        let egui_escape_hit = ctx.input(|i| {
            i.events.iter().any(|e| matches!(e, egui::Event::Key { key: egui::Key::Escape, pressed: true, .. }))
        }) || (egui_backspace_hit && !is_typing && self.numbered_input_buffer.is_empty());
        let escape_triggered = (egui_escape_hit || hook_escape_hit)
            && self.last_switch_input.elapsed() >= Duration::from_millis(200);

        if escape_triggered {
            self.last_switch_input = Instant::now();
            self.stop_speaking();
            if self.dictation_mode {
                self.exit_dictation_mode();
            } else if crate::eyetracking::is_eye_tracking_enabled() || self.eye_tracker.calibration.is_active {
                crate::eyetracking::set_eye_tracking_enabled(false);
                self.eye_tracker.cancel_calibration();
                self.position_initialized = false;
                if let Ok(mut lock) = crate::eyetracking::GLOBAL_GAZE_POS.lock() {
                    *lock = None;
                }
                play_earcon(Earcon::MicClose);
                ctx.request_repaint();
            } else if self.switch_access_mode {
                if self.switch_step == SwitchAccessStep::Elements {
                    if !self.switch_zones.is_empty() {
                        self.switch_step = SwitchAccessStep::Zones;
                        self.switch_active_zone_index = None;
                        self.numbered_targets.clear();
                        clear_screen_numbered_badges();
                        self.switch_focus_index = 0;
                        self.hover_physical_mouse_moved = false;
                        self.switch_last_tick = Instant::now() + Duration::from_millis(1500);
                        let _ = self.tts_sender.send(TtsCommand::Stop);
                        let cur_zone = &self.switch_zones[0];
                        let msg = t!("commands.back_to_blocks", zone = cur_zone.title).to_string();
                        let _ = self.tts_sender.send(TtsCommand::Speak(msg));
                        play_earcon(Earcon::Success);
                        let zone_infos: Vec<ZoneOverlayInfo> = self.switch_zones.iter().enumerate().map(|(i, z)| {
                            ZoneOverlayInfo {
                                index: i + 1,
                                title: z.title.clone(),
                                rect: z.rect,
                                is_focused: i == 0,
                            }
                        }).collect();
                        if let Some(first_z) = self.switch_zones.first() {
                            let mut active_targets = first_z.targets.clone();
                            for (i, t) in active_targets.iter_mut().enumerate() {
                                t.index = i + 1;
                            }
                            self.numbered_targets = active_targets;
                        }
                        let focused_target_idx = self.numbered_targets.first().map(|t| t.index);
                        draw_screen_zones_and_targets(&zone_infos, &self.numbered_targets, self.zoom_factor, focused_target_idx);
                    } else {
                        self.switch_step = SwitchAccessStep::Windows;
                        self.numbered_targets.clear();
                        clear_screen_numbered_badges();
                        self.switch_zones.clear();
                        self.switch_active_zone_index = None;
                        self.switch_windows = collect_switch_candidate_windows();
                        self.switch_focus_index = 0;
                        self.hover_physical_mouse_moved = false;
                        self.numbered_targets = self.switch_windows.iter().enumerate().map(|(i, w)| {
                            NumberedElementTarget {
                                index: i + 1,
                                name: w.title.clone(),
                                role: "fenêtre".to_string(),
                                rect: w.rect,
                                click_x: w.click_x,
                                click_y: w.click_y,
                                shortcut: String::new(),
                                value: String::new(),
                                window_id: Some(w.id),
                            }
                        }).collect();
                        let _ = self.tts_sender.send(TtsCommand::Stop);
                        let base_msg = t!("speech.back_to_windows").to_string();
                        let msg = if let Some(first_win) = self.switch_windows.first() {
                            format!("{base_msg} : {}", first_win.title)
                        } else {
                            base_msg
                        };
                        let _ = self.tts_sender.send(TtsCommand::Speak(msg));
                        play_earcon(Earcon::Success);
                    }
                } else if self.switch_step == SwitchAccessStep::Zones {
                    self.switch_step = SwitchAccessStep::Windows;
                    self.numbered_targets.clear();
                    clear_screen_numbered_badges();
                    self.switch_zones.clear();
                    self.switch_active_zone_index = None;
                    self.switch_windows = collect_switch_candidate_windows();
                    self.switch_focus_index = 0;
                    self.hover_physical_mouse_moved = false;
                    self.numbered_targets = self.switch_windows.iter().enumerate().map(|(i, w)| {
                        NumberedElementTarget {
                            index: i + 1,
                            name: w.title.clone(),
                            role: "fenêtre".to_string(),
                            rect: w.rect,
                            click_x: w.click_x,
                            click_y: w.click_y,
                            shortcut: String::new(),
                            value: String::new(),
                            window_id: Some(w.id),
                        }
                    }).collect();
                    let base_msg = t!("speech.back_to_windows").to_string();
                    let msg = if let Some(first_win) = self.switch_windows.first() {
                        format!("{base_msg} : {}", first_win.title)
                    } else {
                        base_msg
                    };
                    let _ = self.tts_sender.send(TtsCommand::Speak(msg));
                    play_earcon(Earcon::Success);
                } else {
                    self.toggle_switch_access_mode(Some(false));
                }
            } else {
                self.trigger_emergency_stop(ctx);
            }
        }

        // Réception asynchrone des cibles de scan
        while let Ok(result) = self.targets_result_rx.try_recv() {
            if result.sequence >= self.last_applied_scan_sequence {
                self.last_applied_scan_sequence = result.sequence;
                self.is_scan_in_progress = false;

                let is_current_window = self.switch_active_window_id == Some(result.hwnd)
                    || self.numbered_last_hwnd == Some(result.hwnd);
                if is_current_window {
                    if !self.switch_access_mode {
                        self.numbered_targets = result.targets.clone();
                    } else {
                        let (summary, fb_zones) = build_fallback_navigation_zones("", &result.targets);
                        self.switch_zones = fb_zones;
                        self.window_analysis_cache.insert(result.hwnd, CachedWindowAnalysis {
                            summary,
                            zones: self.switch_zones.clone(),
                        });

                        if !result.targets.is_empty() {
                            let _ = self.command_sender.send(AgentCommand::AnalyzeNavigationZones {
                                window_id: result.hwnd,
                                window_title: String::new(),
                                targets: result.targets.clone(),
                                context_meta: None,
                            });
                        }

                        if self.switch_step == SwitchAccessStep::Elements {
                            if let Some(z_idx) = self.switch_active_zone_index {
                                if let Some(zone) = self.switch_zones.get_mut(z_idx) {
                                    let (z_min_x, z_min_y, z_max_x, z_max_y) = (
                                        zone.rect[0] - 15,
                                        zone.rect[1] - 15,
                                        zone.rect[2] + 15,
                                        zone.rect[3] + 15,
                                    );
                                    let fresh_zone_targets: Vec<NumberedElementTarget> = result.targets
                                        .iter()
                                        .filter(|t| {
                                            t.click_x >= z_min_x
                                                && t.click_x <= z_max_x
                                                && t.click_y >= z_min_y
                                                && t.click_y <= z_max_y
                                        })
                                        .cloned()
                                        .collect();

                                    if fresh_zone_targets.is_empty() {
                                        self.numbered_targets = result.targets.clone();
                                    } else {
                                        let mut active_targets = fresh_zone_targets;
                                        for (i, t) in active_targets.iter_mut().enumerate() {
                                            t.index = i + 1;
                                        }
                                        zone.targets = active_targets.clone();
                                        self.numbered_targets = active_targets;
                                    }
                                } else {
                                    self.numbered_targets = result.targets.clone();
                                }
                            } else {
                                self.numbered_targets = result.targets.clone();
                            }
                        } else if self.switch_step == SwitchAccessStep::Windows {
                            self.numbered_targets = result.targets.clone();
                        } else if self.switch_step == SwitchAccessStep::Zones {
                            if let Some(cur_zone) = self.switch_zones.get(self.switch_focus_index % self.switch_zones.len().max(1)) {
                                let mut active_targets = cur_zone.targets.clone();
                                for (i, t) in active_targets.iter_mut().enumerate() {
                                    t.index = i + 1;
                                }
                                self.numbered_targets = active_targets;
                            } else {
                                self.numbered_targets = result.targets.clone();
                            }
                        }
                    }
                    if self.switch_step == SwitchAccessStep::Elements && self.switch_focus_index >= self.numbered_targets.len() {
                        self.switch_focus_index = 0;
                    }
                    if self.switch_access_mode {
                        self.sync_switch_access_overlays();
                    }
                    ctx.request_repaint();
                }
            }
        }
        if self.is_scan_in_progress && self.numbered_last_refresh.elapsed() >= Duration::from_millis(2500) {
            self.is_scan_in_progress = false;
        }

        // Actualisation automatique avec debounce pour le mode survol/numéros
        let eye_tracking_active = crate::eyetracking::is_eye_tracking_enabled() && !self.eye_tracker.calibration.is_active;
        if self.switch_access_mode || eye_tracking_active {
            #[cfg(windows)]
            {
                let fg = unsafe { windows::Win32::UI::WindowsAndMessaging::GetForegroundWindow() };
                let current_pid = std::process::id();
                let mut fg_pid = 0u32;
                unsafe { windows::Win32::UI::WindowsAndMessaging::GetWindowThreadProcessId(fg, Some(&mut fg_pid)) };
                let is_overlay = fg_pid == current_pid;
                let is_tray = is_system_tray_or_desktop(fg);

                let (target_hwnd, cur_hwnd_id) = if !fg.0.is_null() && !is_overlay && !is_tray && !is_window_hung(fg) {
                    (fg, Some(fg.0 as isize))
                } else if let Some(wid) = self.switch_active_window_id {
                    let h = windows::Win32::Foundation::HWND(wid as _);
                    if !h.0.is_null() && !is_window_hung(h) {
                        (h, Some(wid))
                    } else {
                        (windows::Win32::Foundation::HWND(std::ptr::null_mut()), None)
                    }
                } else {
                    (windows::Win32::Foundation::HWND(std::ptr::null_mut()), None)
                };

                if !target_hwnd.0.is_null() {
                    let mut r = windows::Win32::Foundation::RECT::default();
                    let has_rect = unsafe { windows::Win32::UI::WindowsAndMessaging::GetWindowRect(target_hwnd, &mut r).is_ok() };
                    let cur_rect = if has_rect { Some([r.left, r.top, r.right, r.bottom]) } else { None };

                    let win_changed = cur_hwnd_id != self.numbered_last_hwnd;
                    let rect_changed = match (cur_rect, self.numbered_last_win_rect) {
                        (Some(c), Some(p)) => {
                            (c[0] - p[0]).abs() > 10 || (c[1] - p[1]).abs() > 10
                                || (c[2] - p[2]).abs() > 10 || (c[3] - p[3]).abs() > 10
                        }
                        (Some(_), None) | (None, Some(_)) => true,
                        (None, None) => false,
                    };
                    let user_navigating = self.last_switch_input.elapsed() < Duration::from_millis(600);

                    if win_changed {
                        self.numbered_last_hwnd = cur_hwnd_id;
                        self.switch_active_window_id = cur_hwnd_id;
                        self.numbered_last_win_rect = cur_rect;
                        self.numbered_chunk_offset = 0;
                        self.numbered_input_buffer.clear();

                        if !self.switch_access_mode {
                            self.refresh_numbered_targets_for_window(target_hwnd.0 as isize);
                        } else {
                            self.switch_windows = collect_switch_candidate_windows();
                            let mut new_win_idx = self.switch_windows.iter().position(|w| Some(w.id) == cur_hwnd_id);

                            let cur_win_title = if let Some(idx) = new_win_idx {
                                self.switch_windows[idx].title.clone()
                            } else {
                                get_window_title(target_hwnd)
                            };
                            let display_title = if cur_win_title.trim().is_empty() {
                                "Fenêtre".to_string()
                            } else {
                                cur_win_title.trim().to_string()
                            };

                            if new_win_idx.is_none() {
                                if let Some(rect) = cur_rect {
                                    let click_x = rect[0] + (rect[2] - rect[0]) / 2;
                                    let click_y = (rect[1] + 30).min(rect[3] - 10);
                                    self.switch_windows.push(SwitchWindowTarget {
                                        id: target_hwnd.0 as isize,
                                        title: display_title.clone(),
                                        click_x,
                                        click_y,
                                        rect,
                                    });
                                    new_win_idx = Some(self.switch_windows.len() - 1);
                                }
                            }

                            self.switch_focus_index = new_win_idx.unwrap_or(0);
                            self.switch_step = SwitchAccessStep::Zones;

                            if let Some(rect) = cur_rect {
                                draw_screen_orange_border(rect);
                            }

                            self.hover_last_announced_target = Some(target_hwnd.0 as usize);
                            self.transition_to_zones_for_window(target_hwnd.0 as isize, &display_title);
                            ctx.request_repaint();
                        }
                    } else if rect_changed && !user_navigating {
                        if !self.is_scan_in_progress {
                            self.refresh_numbered_targets_for_window(target_hwnd.0 as isize);
                        }
                    } else if (!user_navigating || !self.switch_access_mode) && !self.is_scan_in_progress {
                        let scan_interval = if self.switch_access_mode { Duration::from_millis(5000) } else { Duration::from_millis(2500) };
                        let should_periodic_refresh = self.numbered_last_refresh.elapsed() >= scan_interval;
                        if should_periodic_refresh {
                            self.refresh_numbered_targets_for_window(target_hwnd.0 as isize);
                        }
                    }
                }
            }
            #[cfg(not(windows))]
            {
                let user_navigating = self.last_switch_input.elapsed() < Duration::from_millis(600);
                let scan_interval = if self.switch_access_mode { Duration::from_millis(3500) } else { Duration::from_millis(2500) };
                if (!user_navigating || !self.switch_access_mode) && !self.is_scan_in_progress && self.numbered_last_refresh.elapsed() >= scan_interval {
                    if let Some(wid) = self.switch_active_window_id {
                        self.refresh_numbered_targets_for_window(wid);
                    }
                }
            }
        }

        // Défilement automatique et validation Switch Access
        if self.switch_access_mode && !is_typing {
            let is_shift = ctx.input(|i| i.modifiers.shift);
            let (egui_enter, egui_space, egui_tab, egui_nav_next, egui_nav_prev) = ctx.input(|i| {
                let mut enter = false;
                let mut space = false;
                let mut tab = false;
                let mut next = false;
                let mut prev = false;
                for ev in &i.events {
                    match ev {
                        egui::Event::Key { key, pressed: true, repeat: false, .. } => match key {
                            egui::Key::Enter => enter = true,
                            egui::Key::Space => space = true,
                            egui::Key::Tab => tab = true,
                            egui::Key::ArrowRight | egui::Key::ArrowDown => next = true,
                            egui::Key::ArrowLeft | egui::Key::ArrowUp => prev = true,
                            _ => {}
                        },
                        egui::Event::Text(ref text) => {
                            if text == "\n" || text == "\r" {
                                enter = true;
                            } else if text == " " {
                                space = true;
                            }
                        }
                        _ => {}
                    }
                }
                (enter, space, tab, next || (tab && !is_shift), prev || (tab && is_shift))
            });

            let hook_enter_or_space = hook_events.contains(&SwitchKeyEvent::Select);
            let hook_nav_next = hook_events.contains(&SwitchKeyEvent::Next);
            let hook_nav_prev = hook_events.contains(&SwitchKeyEvent::Prev);

            let is_single_switch = self.is_single_switch_mode();
            let auto_scan_interval = Duration::from_millis(1800);
            let auto_scan_tick = is_single_switch
                && self.status != AgentStatus::Speaking
                && self.last_switch_input.elapsed() >= Duration::from_millis(600)
                && self.switch_last_tick.elapsed() >= auto_scan_interval;

            let mut switch_input_hit = egui_enter || egui_space || hook_enter_or_space;
            let nav_next_hit = egui_nav_next || hook_nav_next || auto_scan_tick;
            let nav_prev_hit = egui_nav_prev || hook_nav_prev;

            let user_nav_hit = (egui_nav_next || hook_nav_next) || (egui_nav_prev || hook_nav_prev);

            if switch_input_hit || nav_next_hit || nav_prev_hit {
                self.last_switch_input = Instant::now();
            }

            if let Some((cur_x, cur_y)) = get_global_cursor_pos() {
                let mouse_moved = match self.hover_last_cursor_pos {
                    Some((lx, ly)) => (cur_x - lx).abs() > 14 || (cur_y - ly).abs() > 14,
                    None => true,
                };

                if mouse_moved {
                    self.hover_last_cursor_pos = Some((cur_x, cur_y));
                    self.hover_cursor_still_since = Instant::now();
                    self.hover_physical_mouse_moved = true;
                    self.switch_last_tick = Instant::now();
                }
            }

            if egui_enter || egui_space || egui_tab || egui_nav_next || egui_nav_prev {
                ctx.input_mut(|i| {
                    i.consume_key(egui::Modifiers::NONE, egui::Key::Enter);
                    i.consume_key(egui::Modifiers::NONE, egui::Key::Space);
                    i.consume_key(egui::Modifiers::NONE, egui::Key::Tab);
                    i.consume_key(egui::Modifiers::SHIFT, egui::Key::Tab);
                    i.consume_key(egui::Modifiers::NONE, egui::Key::ArrowRight);
                    i.consume_key(egui::Modifiers::NONE, egui::Key::ArrowDown);
                    i.consume_key(egui::Modifiers::NONE, egui::Key::ArrowLeft);
                    i.consume_key(egui::Modifiers::NONE, egui::Key::ArrowUp);
                });
            }

            if !is_typing && !hook_digits.is_empty() {
                if self.status == AgentStatus::Speaking {
                    self.stop_speaking();
                }
                play_earcon(Earcon::Success);
                for d in hook_digits {
                    self.numbered_input_buffer.push(d);
                }
                self.numbered_input_last_char_time = Instant::now();
                ctx.request_repaint();
            }

            if nav_next_hit || nav_prev_hit {
                self.numbered_input_buffer.clear();
            }

            let mut digit_confirmed = false;
            if switch_input_hit && !self.numbered_input_buffer.is_empty() {
                if let Ok(idx) = self.numbered_input_buffer.parse::<usize>() {
                    self.numbered_input_buffer.clear();
                    self.last_switch_input = Instant::now();
                    self.stop_speaking();
                    self.click_numbered_target_by_index(idx);
                    digit_confirmed = true;
                    switch_input_hit = false;
                    ctx.request_repaint();
                } else {
                    self.numbered_input_buffer.clear();
                }
            }

            let buffer_timeout = Duration::from_millis(900);
            if !digit_confirmed && !self.numbered_input_buffer.is_empty() {
                if self.numbered_input_last_char_time.elapsed() >= buffer_timeout {
                    if let Ok(idx) = self.numbered_input_buffer.parse::<usize>() {
                        if idx > 0 {
                            self.last_switch_input = Instant::now();
                            self.stop_speaking();
                            self.click_numbered_target_by_index(idx);
                        }
                    }
                    self.numbered_input_buffer.clear();
                    ctx.request_repaint();
                } else {
                    ctx.request_repaint_after(buffer_timeout - self.numbered_input_last_char_time.elapsed());
                }
            }

            match self.switch_step {
                SwitchAccessStep::Windows => {
                    if self.switch_windows.is_empty() {
                        self.switch_windows = collect_switch_candidate_windows();
                    }
                    let num_windows = self.switch_windows.len();
                    if num_windows > 0 {
                        let step_next = nav_next_hit;
                        let step_prev = nav_prev_hit;

                        self.switch_focus_index %= num_windows;
                        if step_next || step_prev {
                            clear_screen_numbered_badges();
                            self.numbered_targets.clear();
                            self.switch_zones.clear();
                            self.switch_active_zone_index = None;

                            if step_prev {
                                self.switch_focus_index = if self.switch_focus_index == 0 || self.switch_focus_index >= num_windows {
                                    num_windows - 1
                                } else {
                                    self.switch_focus_index - 1
                                };
                            } else {
                                self.switch_focus_index = (self.switch_focus_index + 1) % num_windows;
                            }
                            self.switch_focus_index %= num_windows;
                            let cur_win = self.switch_windows[self.switch_focus_index].clone();
                            self.switch_last_tick = Instant::now();
                            self.last_switch_input = Instant::now();
                            self.hover_controls_speech_enabled = false;
                            if user_nav_hit {
                                play_earcon_spatial(Earcon::Navigation, cur_win.click_x, cur_win.click_y);
                            }
                            self.switch_auto_scroll = self.is_single_switch_mode();
                            self.hover_physical_mouse_moved = false;

                            self.switch_active_window_id = Some(cur_win.id);
                            self.numbered_last_hwnd = Some(cur_win.id);
                            self.switch_zones.clear();
                            self.switch_active_zone_index = None;
                            self.numbered_targets.clear();
                            clear_screen_numbered_badges();

                            #[cfg(windows)]
                            {
                                let hwnd = windows::Win32::Foundation::HWND(cur_win.id as _);
                                focus_and_isolate_window(hwnd);
                                let mut r = windows::Win32::Foundation::RECT::default();
                                if unsafe { windows::Win32::UI::WindowsAndMessaging::GetWindowRect(hwnd, &mut r).is_ok() } {
                                    draw_screen_orange_border([r.left, r.top, r.right, r.bottom]);
                                } else {
                                    draw_screen_orange_border(cur_win.rect);
                                }
                                set_global_cursor_pos(cur_win.click_x, cur_win.click_y);
                                self.hover_last_cursor_pos = Some((cur_win.click_x, cur_win.click_y));
                            }
                            #[cfg(not(windows))]
                            {
                                focus_and_isolate_window(&cur_win.title);
                                set_global_cursor_pos(cur_win.click_x, cur_win.click_y);
                                self.hover_last_cursor_pos = Some((cur_win.click_x, cur_win.click_y));
                            }

                            self.refresh_numbered_targets_for_window(cur_win.id);

                            let _ = self.tts_sender.send(TtsCommand::Stop);
                            let vocal_msg = format!("{} : {}", self.switch_focus_index + 1, cur_win.title);
                            let _ = self.tts_sender.send(TtsCommand::Speak(vocal_msg));
                            self.hover_last_announced_target = Some(cur_win.id as usize);
                            ctx.request_repaint();
                        }

                        if switch_input_hit {
                            self.last_switch_input = Instant::now();
                            self.switch_auto_scroll = self.is_single_switch_mode();
                            self.hover_controls_speech_enabled = true;
                            self.hover_physical_mouse_moved = false;
                            play_earcon(Earcon::Success);

                            let win_idx = self.switch_focus_index % num_windows;
                            let cur_win = &self.switch_windows[win_idx];
                            self.switch_active_window_id = Some(cur_win.id);
                            let win_title = cur_win.title.clone();
                            let win_id = cur_win.id;
                            self.transition_to_zones_for_window(win_id, &win_title);
                            ctx.request_repaint();
                        }
                    }
                }
                SwitchAccessStep::Zones => {
                    let num_zones = self.switch_zones.len();
                    if num_zones > 0 {
                        let step_next = nav_next_hit;
                        let step_prev = nav_prev_hit;

                        self.switch_focus_index %= num_zones;
                        if step_next || step_prev {
                            if step_prev {
                                self.switch_focus_index = if self.switch_focus_index == 0 || self.switch_focus_index >= num_zones {
                                    num_zones - 1
                                } else {
                                    self.switch_focus_index - 1
                                };
                            } else {
                                self.switch_focus_index = (self.switch_focus_index + 1) % num_zones;
                            }
                            self.switch_focus_index %= num_zones;
                            let cur_zone = &self.switch_zones[self.switch_focus_index];
                            self.hover_controls_speech_enabled = true;
                            self.switch_last_tick = Instant::now();
                            if user_nav_hit {
                                play_earcon_spatial(Earcon::Navigation, cur_zone.click_x, cur_zone.click_y);
                            }
                            self.hover_physical_mouse_moved = false;
                            if nav_next_hit || nav_prev_hit {
                                self.last_switch_input = Instant::now();
                                self.switch_auto_scroll = self.is_single_switch_mode();
                            }

                            set_global_cursor_pos(cur_zone.click_x, cur_zone.click_y);
                            self.hover_last_cursor_pos = Some((cur_zone.click_x, cur_zone.click_y));
                            self.hover_cursor_still_since = Instant::now();

                            let zone_infos: Vec<ZoneOverlayInfo> = self.switch_zones.iter().enumerate().map(|(i, z)| {
                                ZoneOverlayInfo {
                                    index: i + 1,
                                    title: z.title.clone(),
                                    rect: z.rect,
                                    is_focused: (self.switch_focus_index % num_zones) == i,
                                }
                            }).collect();

                            let mut active_targets = cur_zone.targets.clone();
                            for (i, t) in active_targets.iter_mut().enumerate() {
                                t.index = i + 1;
                            }
                            self.numbered_targets = active_targets;
                            let focused_target_idx = self.numbered_targets.first().map(|t| t.index);
                            draw_screen_zones_and_targets(&zone_infos, &self.numbered_targets, self.zoom_factor, focused_target_idx);

                            let vocal_label = cur_zone.title.clone();
                            let _ = self.tts_sender.send(TtsCommand::Stop);
                            let _ = self.tts_sender.send(TtsCommand::Speak(vocal_label));
                            self.hover_pending_speech = None;
                            self.hover_speech_deadline = None;
                            self.hover_last_announced_zone = Some(cur_zone.title.clone());
                            ctx.request_repaint();
                        }

                        if switch_input_hit {
                            let zone_idx = self.switch_focus_index % num_zones;
                            self.select_switch_zone(zone_idx);
                            ctx.request_repaint();
                        }
                    }
                }
                SwitchAccessStep::Elements => {
                    let num_targets = self.numbered_targets.len();
                    if num_targets > 0 {
                        let step_next = nav_next_hit;
                        let step_prev = nav_prev_hit;

                        self.switch_focus_index %= num_targets;
                        if step_next || step_prev {
                            if step_prev {
                                self.switch_focus_index = if self.switch_focus_index == 0 || self.switch_focus_index >= num_targets {
                                    num_targets - 1
                                } else {
                                    self.switch_focus_index - 1
                                };
                            } else {
                                self.switch_focus_index = (self.switch_focus_index + 1) % num_targets;
                            }
                            self.switch_focus_index %= num_targets;
                            let cur_target = &self.numbered_targets[self.switch_focus_index];
                            self.hover_controls_speech_enabled = true;
                            self.switch_last_tick = Instant::now();
                            if user_nav_hit {
                                play_earcon_spatial(Earcon::Navigation, cur_target.click_x, cur_target.click_y);
                            }
                            self.hover_physical_mouse_moved = false;
                            if nav_next_hit || nav_prev_hit {
                                self.last_switch_input = Instant::now();
                                self.switch_auto_scroll = self.is_single_switch_mode();
                            }

                            focus_element_target(cur_target);
                            self.hover_last_cursor_pos = Some((cur_target.click_x, cur_target.click_y));
                            self.hover_cursor_still_since = Instant::now();
                            #[cfg(windows)]
                            draw_screen_control_border(cur_target.rect, false);

                            let parent_zone = self.switch_active_zone_index.and_then(|idx| self.switch_zones.get(idx));
                            let zone_title = parent_zone.map(|z| z.title.as_str());
                            let is_same_zone = self.hover_last_announced_zone.as_deref() == zone_title;

                            let announcement = build_targeted_hover_speech(
                                zone_title,
                                parent_zone.map_or(&self.numbered_targets, |z| &z.targets),
                                cur_target.index,
                                &cur_target.name,
                                &cur_target.role,
                                &cur_target.shortcut,
                                is_same_zone,
                            );

                            let _ = self.tts_sender.send(TtsCommand::Stop);
                            let _ = self.tts_sender.send(TtsCommand::Speak(announcement));
                            self.hover_pending_speech = None;
                            self.hover_speech_deadline = None;
                            self.hover_last_announced_target = Some(cur_target.index);
                            self.hover_last_announced_zone = zone_title.map(|s| s.to_string());

                            if let Some(zone) = parent_zone {
                                let zone_info = vec![ZoneOverlayInfo {
                                    index: self.switch_active_zone_index.unwrap_or(0) + 1,
                                    title: zone.title.clone(),
                                    rect: zone.rect,
                                    is_focused: true,
                                }];
                                draw_screen_zones_and_targets(&zone_info, &self.numbered_targets, self.zoom_factor, Some(cur_target.index));
                            } else {
                                draw_screen_numbered_badges(&self.numbered_targets, self.zoom_factor, Some(cur_target.index));
                            }

                            ctx.request_repaint();
                        }

                        if switch_input_hit {
                            let cur_target = self.numbered_targets[self.switch_focus_index % num_targets].clone();
                            self.last_switch_input = Instant::now();
                            self.switch_auto_scroll = self.is_single_switch_mode();
                            self.hover_physical_mouse_moved = false;
                            play_earcon_spatial(Earcon::Success, cur_target.click_x, cur_target.click_y);

                            self.click_numbered_target_by_index(cur_target.index);
                        }
                    }
                }
            }
        }

        let r_pressed = !self.is_hidden && !is_typing && ctx.input(|i| {
            i.events.iter().any(|ev| match ev {
                egui::Event::Key { key: egui::Key::R, pressed: true, .. } => true,
                egui::Event::Text(t) => t.eq_ignore_ascii_case("r"),
                _ => false,
            })
        });
        if r_pressed {
            self.toggle_recording(ctx);
        }

        if self.is_recording {
            let trimmed = self.input_text.trim();
            if !trimmed.is_empty() {
                self.live_transcript = trimmed.to_string();
            }
        }
    }

    /// Rendu de la barre d'en-tête (onglets et boutons système)
    pub fn render_header_bar(
        &mut self,
        ui: &mut egui::Ui,
        response_rect: egui::Rect,
        is_maximized: bool,
        theme: &ThemeColors,
    ) -> f32 {
        let mut tab_bar_max_x = response_rect.min.x + 210.0;
        let header_rect = egui::Rect::from_min_max(
            egui::pos2(response_rect.min.x + 10.0, response_rect.min.y + 6.0),
            egui::pos2(response_rect.max.x - 10.0, response_rect.min.y + 32.0),
        );

        ui.scope_builder(egui::UiBuilder::new().max_rect(header_rect), |ui| {
            ui.horizontal(|ui| {
                let is_assist_active = self.active_tab == ActiveTab::Assistance;
                let tab_assist = ui.selectable_label(is_assist_active, t!("ui.assistance_tab"));
                let is_assist_key_hit = make_widget_accessible_and_activated(ui, &tab_assist, theme.accent);
                let tab_assist_id = ui.make_persistent_id("dwell_tab_assistance");
                let assist_activated = if !is_assist_active {
                    apply_dwell_to_response(ui, &mut self.dwell_state, tab_assist_id, &tab_assist)
                } else {
                    tab_assist.clicked()
                };
                if assist_activated || is_assist_key_hit {
                    if self.active_tab != ActiveTab::Assistance {
                        self.stop_speaking();
                    }
                    self.active_tab = ActiveTab::Assistance;
                }

                let chat_title = if self.twitch_messages.is_empty() {
                    t!("ui.twitch_chat_tab").to_string()
                } else {
                    t!("ui.twitch_chat_count", count = self.twitch_messages.len()).to_string()
                };
                let is_chat_active = self.active_tab == ActiveTab::Chat;
                let tab_chat = ui.selectable_label(is_chat_active, chat_title);
                let is_chat_key_hit = make_widget_accessible_and_activated(ui, &tab_chat, theme.accent);
                let tab_chat_id = ui.make_persistent_id("dwell_tab_twitch_chat");
                let chat_activated = if !is_chat_active {
                    apply_dwell_to_response(ui, &mut self.dwell_state, tab_chat_id, &tab_chat)
                } else {
                    tab_chat.clicked()
                };
                if chat_activated || is_chat_key_hit {
                    if self.active_tab != ActiveTab::Chat {
                        self.stop_speaking();
                    }
                    self.active_tab = ActiveTab::Chat;
                    if self.twitch_search_results.is_empty() {
                        let _ = self.command_sender.send(AgentCommand::SearchTwitch(String::new()));
                    }
                }
                tab_bar_max_x = tab_assist.rect.max.x.max(tab_chat.rect.max.x);

                ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                    ui.spacing_mut().item_spacing.x = 4.0;

                    let close_btn = ui.add(
                        egui::Button::new(
                            egui::RichText::new("×")
                                .size(13.0)
                                .color(egui::Color32::from_rgb(235, 235, 245)),
                        )
                            .min_size(egui::vec2(20.0, 18.0))
                            .corner_radius(egui::CornerRadius::same(4)),
                    ).on_hover_text(t!("ui.btn_close"));
                    close_btn.widget_info(|| {
                        egui::WidgetInfo::labeled(egui::Role::Button, ui.is_enabled(), t!("ui.btn_close").to_string())
                    });
                    if make_widget_accessible_and_activated(ui, &close_btn, theme.accent) {
                        ui.ctx().send_viewport_cmd(egui::ViewportCommand::Close);
                    }

                    let max_icon = if is_maximized { "⧉" } else { "□" };
                    let max_label = if is_maximized { t!("ui.btn_restore").to_string() } else { t!("ui.btn_maximize").to_string() };
                    let max_btn = ui.add(
                        egui::Button::new(
                            egui::RichText::new(max_icon)
                                .size(10.5)
                                .color(egui::Color32::from_rgb(200, 215, 230)),
                        )
                            .min_size(egui::vec2(20.0, 18.0))
                            .corner_radius(egui::CornerRadius::same(4)),
                    ).on_hover_text(&max_label);
                    max_btn.widget_info(|| {
                        egui::WidgetInfo::labeled(egui::Role::Button, ui.is_enabled(), max_label.clone())
                    });
                    if make_widget_accessible_and_activated(ui, &max_btn, theme.accent) {
                        ui.ctx().send_viewport_cmd(egui::ViewportCommand::Maximized(!is_maximized));
                    }

                    let min_btn = ui.add(
                        egui::Button::new(
                            egui::RichText::new("—")
                                .size(10.5)
                                .color(egui::Color32::from_rgb(200, 215, 230)),
                        )
                            .min_size(egui::vec2(20.0, 18.0))
                            .corner_radius(egui::CornerRadius::same(4)),
                    ).on_hover_text(t!("ui.btn_minimize"));
                    min_btn.widget_info(|| {
                        egui::WidgetInfo::labeled(egui::Role::Button, ui.is_enabled(), t!("ui.btn_minimize").to_string())
                    });
                    if make_widget_accessible_and_activated(ui, &min_btn, theme.accent) {
                        ui.ctx().send_viewport_cmd(egui::ViewportCommand::Minimized(true));
                    }
                });
            });
        });

        tab_bar_max_x
    }

    /// Rendu du dock de contrôle (avatar, saisie, barre d'actions, equalizer)
    pub fn render_control_dock(
        &mut self,
        ui: &mut egui::Ui,
        control_rect: egui::Rect,
        time: f64,
        theme: &ThemeColors,
    ) {
        ui.scope_builder(egui::UiBuilder::new().max_rect(control_rect), |ui| {
            egui::Frame::new()
                .fill(theme.control_bg)
                .stroke(egui::Stroke::new(1.0, theme.border))
                .corner_radius(egui::CornerRadius::same(12))
                .inner_margin(egui::Margin::symmetric(10, 8))
                .show(ui, |ui| {
                    let inner_rect = ui.available_rect_before_wrap();
                    let avatar_pos = egui::pos2(inner_rect.max.x - 20.0, inner_rect.center().y);
                    self.draw_avatar(ui, avatar_pos, 14.0, time);

                    let avatar_rect = egui::Rect::from_center_size(avatar_pos, egui::vec2(32.0, 32.0));
                    let avatar_resp = ui.interact(avatar_rect, ui.scope_id().with("avatar_interact"), egui::Sense::click());
                    let layer_id = ui.layer_id();
                    let stop_speech_key = crate::keyboard_hook::vk_to_label(crate::keyboard_hook::KEY_BIND_STOP_SPEECH.load(Ordering::Relaxed));
                    let avatar_label = if self.status == AgentStatus::Speaking {
                        format!("{} [{}]", t!("ui.avatar_mute_tooltip"), stop_speech_key)
                    } else {
                        format!("Avatar Libertide ({:?})", self.status)
                    };
                    avatar_resp.widget_info(|| {
                        egui::WidgetInfo::labeled(egui::Role::Button, ui.is_enabled(), avatar_label.clone())
                    });
                    ui.memory_mut(|m| m.interested_in_focus(avatar_resp.id, layer_id));
                    if avatar_resp.has_focus() {
                        ui.painter().rect_stroke(
                            avatar_rect.expand(3.0),
                            egui::CornerRadius::same(16),
                            egui::Stroke::new(2.0, theme.accent),
                            egui::StrokeKind::Outside,
                        );
                    }
                    let is_avatar_key_hit = avatar_resp.has_focus() && ui.input(|i| {
                        i.events.iter().any(|ev| match ev {
                            egui::Event::Key { key, pressed: true, repeat: false, .. } => *key == egui::Key::Space || *key == egui::Key::Enter,
                            egui::Event::Text(t) => t == " " || t == "\n" || t == "\r",
                            _ => false,
                        })
                    });
                    if self.status == AgentStatus::Speaking {
                        if avatar_resp.hovered() {
                            ui.ctx().set_cursor_icon(egui::CursorIcon::PointingHand);
                        }
                        if avatar_resp.on_hover_text(format!("{} [{}]", t!("ui.avatar_mute_tooltip"), stop_speech_key)).clicked() || is_avatar_key_hit {
                            self.stop_speaking();
                        }
                    }

                    let content_area = egui::Rect::from_min_max(
                        inner_rect.min,
                        egui::pos2(inner_rect.max.x - 48.0, inner_rect.max.y),
                    );

                    ui.scope_builder(egui::UiBuilder::new().max_rect(content_area), |ui| {
                        ui.vertical(|ui| {
                            let prov_name = self.current_agent_model_display_name();
                            let status_badge = if self.dictation_mode {
                                t!("ui.dictation_badge").to_string()
                            } else if crate::eyetracking::is_eye_tracking_enabled() && self.eye_tracker.is_mouse_overridden() {
                                t!("eyetracking.mouse_override_badge").to_string()
                            } else if self.switch_access_mode {
                                if self.switch_step == SwitchAccessStep::Windows {
                                    if !self.switch_windows.is_empty() {
                                        let cur = &self.switch_windows[self.switch_focus_index % self.switch_windows.len()];
                                        t!("ui.select_window_hint", index = self.switch_focus_index + 1, title = cur.title).to_string()
                                    } else {
                                        t!("ui.searching_windows").to_string()
                                    }
                                } else if self.switch_step == SwitchAccessStep::Zones {
                                    if !self.switch_zones.is_empty() {
                                        let cur = &self.switch_zones[self.switch_focus_index % self.switch_zones.len()];
                                        t!("ui.open_zone_hint", index = self.switch_focus_index + 1, title = cur.title).to_string()
                                    } else {
                                        t!("ui.analyzing_zones").to_string()
                                    }
                                } else if self.switch_step == SwitchAccessStep::Elements {
                                    let buffer_feedback = if !self.numbered_input_buffer.is_empty() {
                                        t!("ui.keyboard_input_buffer", buffer = self.numbered_input_buffer).to_string()
                                    } else {
                                        String::new()
                                    };
                                    if let Some(target) = self.numbered_targets.get(self.switch_focus_index) {
                                        t!("ui.target_status_item", provider = prov_name, buffer = buffer_feedback, index = target.index, name = target.name, count = self.numbered_targets.len()).to_string()
                                    } else {
                                        t!("ui.target_status_active", provider = prov_name, buffer = buffer_feedback, count = self.numbered_targets.len()).to_string()
                                    }
                                } else {
                                    t!("ui.detecting_controls").to_string()
                                }
                            } else {
                                match self.status {
                                    AgentStatus::Idle => t!("ui.status_idle", provider = prov_name).to_string(),
                                    AgentStatus::Thinking => t!("ui.status_thinking", provider = prov_name).to_string(),
                                    AgentStatus::Speaking => t!("ui.status_speaking", provider = prov_name).to_string(),
                                    AgentStatus::Listening => String::new(),
                                    AgentStatus::EmergencyStopped => t!("ui.status_locked").to_string(),
                                }
                            };

                            let badge_color = if self.dictation_mode {
                                egui::Color32::from_rgb(255, 205, 70)
                            } else if crate::eyetracking::is_eye_tracking_enabled() && self.eye_tracker.is_mouse_overridden() {
                                egui::Color32::from_rgb(255, 170, 0)
                            } else if self.switch_access_mode {
                                egui::Color32::from_rgb(255, 140, 0)
                            } else if self.status == AgentStatus::Listening {
                                egui::Color32::from_rgb(255, 80, 80)
                            } else {
                                theme.accent
                            };

                            if !status_badge.is_empty() {
                                let badge_resp = ui.add(
                                    egui::Label::new(
                                        egui::RichText::new(&status_badge)
                                            .size(10.5)
                                            .color(badge_color),
                                    )
                                        .truncate()
                                        .sense(if self.status == AgentStatus::Speaking || self.switch_access_mode { egui::Sense::click() } else { egui::Sense::hover() }),
                                );
                                if self.status == AgentStatus::Speaking {
                                    if badge_resp.hovered() {
                                        ui.ctx().set_cursor_icon(egui::CursorIcon::PointingHand);
                                    }
                                    if badge_resp.on_hover_text(format!("{} [{}]", t!("ui.avatar_mute_tooltip"), stop_speech_key)).clicked() {
                                        self.stop_speaking();
                                    }
                                } else if self.switch_access_mode {
                                    if badge_resp.hovered() {
                                        ui.ctx().set_cursor_icon(egui::CursorIcon::PointingHand);
                                    }
                                    let switch_key = crate::keyboard_hook::vk_to_label(crate::keyboard_hook::KEY_BIND_SWITCH.load(Ordering::Relaxed));
                                    if badge_resp.on_hover_text(format!("{} [{}]", t!("ui.badge_disable_hover_tooltip"), switch_key)).clicked() {
                                        self.toggle_switch_access_mode(Some(false));
                                    }
                                } else {
                                    badge_resp.on_hover_text(&status_badge);
                                }
                                ui.add_space(2.0);
                            }

                            if self.is_recording && !self.live_transcript.is_empty() {
                                ui.add(
                                    egui::Label::new(
                                        egui::RichText::new(&self.live_transcript)
                                            .size(11.5)
                                            .color(egui::Color32::from_rgb(255, 220, 130)),
                                    )
                                        .truncate(),
                                );
                                ui.add_space(2.0);
                            }

                            ui.add_space(4.0);

                            let mut clicked_app_suggestion: Option<String> = None;
                            let mut autocomplete_input_to: Option<String> = None;

                            if !self.input_suggestions.is_empty() {
                                ui.horizontal(|ui| {
                                    ui.spacing_mut().item_spacing.x = 4.0;
                                    ui.label(
                                        egui::RichText::new(t!("ui.app_suggestions_header"))
                                            .size(10.5)
                                            .color(theme.accent)
                                            .strong(),
                                    );
                                    for sug in &self.input_suggestions {
                                        let chip = egui::Button::new(
                                            egui::RichText::new(format!("🚀 {sug}"))
                                                .size(10.5)
                                                .color(egui::Color32::from_rgb(220, 240, 255)),
                                        )
                                            .fill(egui::Color32::from_rgba_unmultiplied(0, 180, 255, 30))
                                            .stroke(egui::Stroke::new(1.0, theme.border_subtle))
                                            .corner_radius(egui::CornerRadius::same(6))
                                            .small();

                                        let resp = ui.add(chip)
                                            .on_hover_text(t!("ui.app_suggestion_tooltip", app = sug));
                                        let sug_dwell_id = ui.make_persistent_id(format!("dwell_app_sug_{sug}"));
                                        let is_chip_hit = make_widget_accessible_and_activated(ui, &resp, theme.accent);
                                        if apply_dwell_to_response(ui, &mut self.dwell_state, sug_dwell_id, &resp) || is_chip_hit {
                                            clicked_app_suggestion = Some(t!("ui.open_app_cmd", app = sug).to_string());
                                        }
                                        resp.context_menu(|ui| {
                                            if ui.button(t!("ui.autocomplete_field")).clicked() {
                                                autocomplete_input_to = Some(t!("ui.open_app_cmd", app = sug).to_string());
                                                ui.close();
                                            }
                                        });
                                    }
                                });
                                ui.add_space(2.0);
                            }

                            let mut enter_hit = false;
                            ui.horizontal(|ui| {
                                let edit_width = ui.available_width().max(60.0);
                                let input_id = egui::Id::NULL.with("prompt_input_text");
                                let is_input_focused = ui.ctx().memory(|m| m.has_focus(input_id));
                                let is_modal_open = self.show_api_key_modal;
                                let is_other_typing = ui.ctx().egui_wants_keyboard_input() && !is_input_focused;

                                let mut enter_from_events = false;
                                let mut tab_from_events = false;
                                if is_input_focused && !is_modal_open && !is_other_typing {
                                    ui.ctx().input(|i| {
                                        for ev in &i.events {
                                            match ev {
                                                egui::Event::Key { key: egui::Key::Enter, pressed: true, modifiers, .. } if !modifiers.shift => {
                                                    enter_from_events = true;
                                                }
                                                egui::Event::Key { key: egui::Key::Tab, pressed: true, .. } => {
                                                    tab_from_events = true;
                                                }
                                                egui::Event::Text(ref text) if text == "\n" || text == "\r" => {
                                                    enter_from_events = true;
                                                }
                                                _ => {}
                                            }
                                        }
                                    });
                                }

                                if tab_from_events {
                                    if let Some(top_sug) = self.input_suggestions.first() {
                                        autocomplete_input_to = Some(t!("ui.open_app_cmd", app = top_sug).to_string());
                                        ui.ctx().input_mut(|i| i.consume_key(egui::Modifiers::NONE, egui::Key::Tab));
                                    }
                                }

                                let enter_pressed_before = enter_from_events;
                                if enter_pressed_before {
                                    enter_hit = true;
                                    ui.ctx().input_mut(|i| {
                                        i.consume_key(egui::Modifiers::NONE, egui::Key::Enter);
                                        i.events.retain(|e| !matches!(e, egui::Event::Text(ref t) if t == "\n" || t == "\r"));
                                    });
                                    while self.input_text.ends_with('\n') || self.input_text.ends_with('\r') {
                                        self.input_text.pop();
                                    }
                                }

                                let hint = if self.is_recording {
                                    t!("ui.prompt_hint_listening").to_string()
                                } else {
                                    t!("ui.prompt_hint_idle").to_string()
                                };
                                let response = render_accessible_input(
                                    ui,
                                    input_id,
                                    &mut self.input_text,
                                    &hint,
                                    true,
                                    Some(edit_width),
                                    Some(54.0),
                                    false,
                                    theme,
                                );

                                if response.changed() {
                                    self.stop_speaking();
                                }

                                let shift_down = ui.ctx().input(|i| i.modifiers.shift);
                                if response.has_focus() && !shift_down {
                                    if self.input_text.ends_with('\n') || self.input_text.ends_with('\r') {
                                        while self.input_text.ends_with('\n') || self.input_text.ends_with('\r') {
                                            self.input_text.pop();
                                        }
                                        enter_hit = true;
                                    }
                                }

                                if let Some(completed) = autocomplete_input_to {
                                    self.input_text = completed;
                                    self.last_suggestion_query = self.input_text.trim().to_string();
                                    self.input_suggestions = find_matching_applications(&self.input_text, 5);
                                    ui.ctx().memory_mut(|m| m.request_focus(input_id));
                                    ui.ctx().request_repaint();
                                }

                                if let Some(prompt) = clicked_app_suggestion {
                                    self.submit_prompt(prompt);
                                }

                                if !enter_hit && !is_modal_open && !is_other_typing {
                                    enter_hit = enter_from_events || (response.has_focus() && ui.ctx().input(|i| {
                                        i.events.iter().any(|e| match e {
                                            egui::Event::Key { key: egui::Key::Enter, pressed: true, modifiers, .. } if !modifiers.shift => true,
                                            egui::Event::Text(t) if t == "\n" || t == "\r" => true,
                                            _ => false,
                                        })
                                    }));
                                    if enter_hit {
                                        ui.ctx().input_mut(|i| i.consume_key(egui::Modifiers::NONE, egui::Key::Enter));
                                        while self.input_text.ends_with('\n') || self.input_text.ends_with('\r') {
                                            self.input_text.pop();
                                        }
                                    }
                                }
                            });

                            ui.add_space(3.0);

                            // 2e rangée : Les 7 boutons d'action ergonomiques (WCAG 2.2 AAA)
                            ui.horizontal(|ui| {
                                ui.spacing_mut().item_spacing.x = 4.0;

                                // 1. Analyser
                                let analyze_id = ui.make_persistent_id("dwell_btn_analyze");
                                let analyze_prog = self.dwell_state.get_progress(analyze_id);
                                let analyze_tooltip = format!("{} [Entrée]", t!("ui.btn_analyze_tooltip"));
                                let analyze_resp = render_accessible_icon_button(
                                    ui,
                                    "🔍",
                                    &t!("ui.btn_analyze"),
                                    &analyze_tooltip,
                                    false,
                                    analyze_prog,
                                    Some(egui::Color32::from_rgb(0, 140, 220)),
                                    Some(egui::Color32::from_rgb(0, 165, 245)),
                                );
                                let (analyze_dwell, _) = self.dwell_state.update(analyze_id, analyze_resp.hovered());
                                let is_analyze_key = analyze_resp.has_focus() && ui.input(|i| {
                                    i.events.iter().any(|ev| match ev {
                                        egui::Event::Key { key, pressed: true, repeat: false, .. } => *key == egui::Key::Space || *key == egui::Key::Enter,
                                        egui::Event::Text(t) => t == " " || t == "\n" || t == "\r",
                                        _ => false,
                                    })
                                });
                                let analyze_btn_hit = analyze_resp.clicked() || analyze_dwell || is_analyze_key;
                                let enter_with_text = enter_hit && !self.input_text.trim().is_empty();

                                if analyze_btn_hit || enter_with_text {
                                    let typed_raw = std::mem::take(&mut self.input_text).trim().to_string();
                                    let prompt_raw = if analyze_btn_hit && typed_raw.is_empty() {
                                        t!("commands.analyze_screen_prompt").to_string()
                                    } else {
                                        typed_raw
                                    };
                                    self.submit_prompt(prompt_raw);
                                }

                                // 2. Micro
                                let mic_id = ui.make_persistent_id("dwell_btn_mic");
                                let mic_prog = self.dwell_state.get_progress(mic_id);
                                let (mic_icon, mic_sub, mic_color) = if self.is_recording {
                                    ("🔴", t!("ui.btn_mic_stop").to_string(), Some(egui::Color32::from_rgb(235, 60, 60)))
                                } else {
                                    ("🎙", t!("ui.btn_mic").to_string(), None)
                                };
                                let mic_raw_tip = t!("ui.btn_mic_tooltip").to_string();
                                let mic_tooltip = if mic_raw_tip.contains("[R]") || mic_raw_tip.contains("(R)") {
                                    mic_raw_tip
                                } else {
                                    format!("{mic_raw_tip} [R]")
                                };
                                let mic_resp = render_accessible_icon_button(
                                    ui,
                                    mic_icon,
                                    &mic_sub,
                                    &mic_tooltip,
                                    self.is_recording,
                                    mic_prog,
                                    mic_color,
                                    None,
                                );
                                let (mic_dwell, _) = self.dwell_state.update(mic_id, mic_resp.hovered());
                                let is_mic_key = mic_resp.has_focus() && ui.input(|i| {
                                    i.events.iter().any(|ev| match ev {
                                        egui::Event::Key { key, pressed: true, repeat: false, .. } => *key == egui::Key::Space || *key == egui::Key::Enter,
                                        egui::Event::Text(t) => t == " " || t == "\n" || t == "\r",
                                        _ => false,
                                    })
                                });
                                if mic_resp.clicked() || mic_dwell || is_mic_key {
                                    self.toggle_recording(ui.ctx());
                                }

                                // 2b. Sous-titres en direct (toujours présent et actif)
                                let sub_id = ui.make_persistent_id("dwell_btn_subtitles");
                                let sub_prog = self.dwell_state.get_progress(sub_id);
                                let is_sub_on = self.is_subtitles_open();
                                let (sub_sub, sub_tooltip) = if is_sub_on {
                                    (
                                        t!("ui.btn_subtitles_active").to_string(),
                                        t!("ui.btn_subtitles_active_tooltip").to_string(),
                                    )
                                } else {
                                    (
                                        t!("ui.btn_subtitles").to_string(),
                                        t!("ui.btn_subtitles_tooltip").to_string(),
                                    )
                                };
                                let sub_sub_display = if sub_sub == "ui.btn_subtitles_active" {
                                    "Actifs".to_string()
                                } else if sub_sub == "ui.btn_subtitles" {
                                    "S-Titres".to_string()
                                } else {
                                    sub_sub
                                };
                                let sub_tooltip_display = if sub_tooltip == "ui.btn_subtitles_tooltip" || sub_tooltip == "ui.btn_subtitles_active_tooltip" {
                                    t!("commands.subtitles_title").to_string()
                                } else {
                                    sub_tooltip
                                };
                                let sub_resp = render_accessible_icon_button(
                                    ui,
                                    "💬",
                                    &sub_sub_display,
                                    &sub_tooltip_display,
                                    is_sub_on,
                                    sub_prog,
                                    Some(egui::Color32::from_rgb(0, 200, 150)),
                                    Some(egui::Color32::from_rgb(0, 230, 175)),
                                );
                                let (sub_dwell, _) = self.dwell_state.update(sub_id, sub_resp.hovered());
                                let is_sub_key = sub_resp.has_focus() && ui.input(|i| {
                                    i.events.iter().any(|ev| match ev {
                                        egui::Event::Key { key, pressed: true, repeat: false, .. } => *key == egui::Key::Space || *key == egui::Key::Enter,
                                        egui::Event::Text(t) => t == " " || t == "\n" || t == "\r",
                                        _ => false,
                                    })
                                });
                                if sub_resp.clicked() || sub_dwell || is_sub_key {
                                    if is_sub_on {
                                        self.stop_live_subtitles();
                                    } else {
                                        self.start_live_subtitles(None);
                                    }
                                }

                                // 3. Eye-Tracking
                                if self.should_show_eye_tracking_button() {
                                    let eye_id = ui.make_persistent_id("dwell_btn_eyetracking");
                                    let eye_prog = self.dwell_state.get_progress(eye_id);
                                    let is_eye_on = crate::eyetracking::is_eye_tracking_enabled();
                                    let eye_sub = if is_eye_on { t!("ui.btn_eyetracking_active").to_string() } else { t!("ui.btn_eyetracking").to_string() };
                                    let gaze_key = crate::keyboard_hook::vk_to_label(crate::keyboard_hook::KEY_BIND_GAZE_CLICK.load(Ordering::Relaxed));
                                    let eye_tooltip = format!("{} [Clic: {}]", t!("ui.btn_eyetracking_tooltip"), gaze_key);
                                    let eye_resp = render_accessible_icon_button(
                                        ui,
                                        "👁",
                                        &eye_sub,
                                        &eye_tooltip,
                                        is_eye_on,
                                        eye_prog,
                                        Some(egui::Color32::from_rgb(0, 229, 255)),
                                        Some(egui::Color32::from_rgb(0, 245, 255)),
                                    );
                                    let (eye_dwell, _) = self.dwell_state.update(eye_id, eye_resp.hovered());
                                    let is_eye_key = eye_resp.has_focus() && ui.input(|i| {
                                        i.events.iter().any(|ev| match ev {
                                            egui::Event::Key { key, pressed: true, repeat: false, .. } => *key == egui::Key::Space || *key == egui::Key::Enter,
                                            egui::Event::Text(t) => t == " " || t == "\n" || t == "\r",
                                            _ => false,
                                        })
                                    });
                                    if eye_resp.clicked() || eye_dwell || is_eye_key {
                                        let new_state = !is_eye_on;
                                        crate::eyetracking::set_eye_tracking_enabled(new_state);
                                        if new_state {
                                            play_earcon(Earcon::Success);
                                        } else {
                                            self.eye_tracker.cancel_calibration();
                                            if let Ok(mut lock) = crate::eyetracking::GLOBAL_GAZE_POS.lock() {
                                                *lock = None;
                                            }
                                            play_earcon(Earcon::MicClose);
                                        }
                                    }
                                }

                                // 4. Clavier Virtuel
                                if self.has_motor_disability() || self.is_keyboard_open() {
                                    let kbd_id = ui.make_persistent_id("dwell_btn_keyboard");
                                    let kbd_prog = self.dwell_state.get_progress(kbd_id);
                                    let is_kbd_open = self.is_keyboard_open();
                                    let kbd_sub = if is_kbd_open { t!("ui.btn_keyboard_active").to_string() } else { t!("ui.btn_keyboard").to_string() };
                                    let kbd_resp = render_accessible_icon_button(
                                        ui,
                                        "⌨",
                                        &kbd_sub,
                                        &t!("ui.btn_keyboard_tooltip"),
                                        is_kbd_open,
                                        kbd_prog,
                                        Some(egui::Color32::from_rgb(0, 180, 255)),
                                        Some(egui::Color32::from_rgb(0, 210, 255)),
                                    );
                                    let (kbd_dwell, _) = self.dwell_state.update(kbd_id, kbd_resp.hovered());
                                    let is_kbd_key = kbd_resp.has_focus() && ui.input(|i| {
                                        i.events.iter().any(|ev| match ev {
                                            egui::Event::Key { key, pressed: true, repeat: false, .. } => *key == egui::Key::Space || *key == egui::Key::Enter,
                                            egui::Event::Text(t) => t == " " || t == "\n" || t == "\r",
                                            _ => false,
                                        })
                                    });
                                    if kbd_resp.clicked() || kbd_dwell || is_kbd_key {
                                        if is_kbd_open {
                                            self.floating_cards.retain(|c| c.id != "virtual_keyboard");
                                            crate::floating_window::update_open_floating_cards(&self.floating_cards);
                                            play_earcon(Earcon::MicClose);
                                        } else {
                                            self.open_keyboard_card();
                                        }
                                    }
                                }

                                // 5. Musique / Rythme
                                let rhythm_id = ui.make_persistent_id("dwell_btn_rhythm");
                                let rhythm_prog = self.dwell_state.get_progress(rhythm_id);
                                let is_open = self.is_instrument_open();
                                let rhythm_resp = render_accessible_icon_button(
                                    ui,
                                    "🎵",
                                    &t!("instrument.btn_rhythm"),
                                    &t!("instrument.btn_rhythm_tooltip"),
                                    is_open,
                                    rhythm_prog,
                                    Some(egui::Color32::from_rgb(180, 70, 240)),
                                    Some(egui::Color32::from_rgb(205, 95, 255)),
                                );
                                let (rhythm_dwell, _) = self.dwell_state.update(rhythm_id, rhythm_resp.hovered());
                                let is_rhythm_key = rhythm_resp.has_focus() && ui.input(|i| {
                                    i.events.iter().any(|ev| match ev {
                                        egui::Event::Key { key, pressed: true, repeat: false, .. } => *key == egui::Key::Space || *key == egui::Key::Enter,
                                        egui::Event::Text(t) => t == " " || t == "\n" || t == "\r",
                                        _ => false,
                                    })
                                });
                                if rhythm_resp.clicked() || rhythm_dwell || is_rhythm_key {
                                    if is_open {
                                        self.floating_cards.retain(|c| c.id != "instrument_toy");
                                        crate::floating_window::update_open_floating_cards(&self.floating_cards);
                                        play_earcon(Earcon::MicClose);
                                    } else {
                                        self.open_instrument_card();
                                    }
                                }

                                // 6. Survol / Switch Access
                                if self.should_show_hover_mode_button() {
                                    let switch_id = ui.make_persistent_id("dwell_btn_switch");
                                    let switch_prog = self.dwell_state.get_progress(switch_id);
                                    let switch_sub = if self.switch_access_mode { t!("ui.btn_hover_active").to_string() } else { t!("ui.btn_hover").to_string() };
                                    let switch_key = crate::keyboard_hook::vk_to_label(crate::keyboard_hook::KEY_BIND_SWITCH.load(Ordering::Relaxed));
                                    let switch_tooltip = format!("{} [{}]", t!("ui.btn_hover_tooltip"), switch_key);
                                    let switch_resp = render_accessible_icon_button(
                                        ui,
                                        "🎯",
                                        &switch_sub,
                                        &switch_tooltip,
                                        self.switch_access_mode,
                                        switch_prog,
                                        Some(egui::Color32::from_rgb(235, 115, 0)),
                                        None,
                                    );
                                    let (switch_dwell, _) = self.dwell_state.update(switch_id, switch_resp.hovered());
                                    let is_switch_key = switch_resp.has_focus() && ui.input(|i| {
                                        i.events.iter().any(|ev| match ev {
                                            egui::Event::Key { key, pressed: true, repeat: false, .. } => *key == egui::Key::Space || *key == egui::Key::Enter,
                                            egui::Event::Text(t) => t == " " || t == "\n" || t == "\r",
                                            _ => false,
                                        })
                                    });
                                    if switch_resp.clicked() || switch_dwell || is_switch_key {
                                        self.toggle_switch_access_mode(None);
                                    }
                                }

                                // 7. Réglages
                                let settings_id = ui.make_persistent_id("dwell_btn_settings");
                                let settings_prog = self.dwell_state.get_progress(settings_id);
                                let settings_resp = render_accessible_icon_button(
                                    ui,
                                    "⚙",
                                    &t!("ui.btn_settings"),
                                    &t!("ui.btn_settings_tooltip"),
                                    self.show_api_key_modal,
                                    settings_prog,
                                    Some(theme.accent),
                                    None,
                                );
                                let (settings_dwell, _) = self.dwell_state.update(settings_id, settings_resp.hovered());
                                let is_settings_key = settings_resp.has_focus() && ui.input(|i| {
                                    i.events.iter().any(|ev| match ev {
                                        egui::Event::Key { key, pressed: true, repeat: false, .. } => *key == egui::Key::Space || *key == egui::Key::Enter,
                                        egui::Event::Text(t) => t == " " || t == "\n" || t == "\r",
                                        _ => false,
                                    })
                                });
                                if settings_resp.clicked() || settings_dwell || is_settings_key {
                                    self.settings_tab = SettingsTab::LlmProvider;
                                    self.show_api_key_modal = true;
                                    self.sync_modal_inputs_for_provider(self.llm_config.provider);
                                    self.api_key_error = None;
                                    self.stop_speaking();
                                    if self.is_recording {
                                        self.stop_recording();
                                    }
                                }

                                // 8. Effacer
                                let clear_id = ui.make_persistent_id("dwell_btn_clear");
                                let clear_prog = self.dwell_state.get_progress(clear_id);
                                let clear_key = crate::keyboard_hook::vk_to_label(crate::keyboard_hook::KEY_BIND_CLEAR.load(Ordering::Relaxed));
                                let clear_tooltip = format!("{} [{}]", t!("ui.btn_clear_tooltip"), clear_key);
                                let clear_resp = render_accessible_icon_button(
                                    ui,
                                    "🗑",
                                    &t!("ui.btn_clear"),
                                    &clear_tooltip,
                                    false,
                                    clear_prog,
                                    None,
                                    None,
                                );
                                let (clear_dwell, _) = self.dwell_state.update(clear_id, clear_resp.hovered());
                                let is_clear_key = clear_resp.has_focus() && ui.input(|i| {
                                    i.events.iter().any(|ev| match ev {
                                        egui::Event::Key { key, pressed: true, repeat: false, .. } => *key == egui::Key::Space || *key == egui::Key::Enter,
                                        egui::Event::Text(t) => t == " " || t == "\n" || t == "\r",
                                        _ => false,
                                    })
                                });
                                if clear_resp.clicked() || clear_dwell || is_clear_key {
                                    let _ = self.command_sender.send(AgentCommand::ClearHistory);
                                    self.chat_history.clear();
                                    self.window_analysis_cache.clear();
                                    self.chat_history.push(ChatEntry {
                                        role: ChatRole::Agent,
                                        text: t!("commands.context_cleared").to_string(),
                                        timestamp: current_time_str(),
                                        quick_suggestions: Vec::new(),
                                        screen_size_kb: None,
                                    });
                                    self.live_transcript.clear();
                                    self.stop_speaking();
                                }

                                ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                                    self.draw_mini_equalizer(ui, time);
                                });
                            });
                        });
                    });
                });
        });
    }

    fn submit_prompt(&mut self, prompt_raw: String) {
        self.stop_speaking();
        if self.is_recording {
            self.stop_recording();
        }
        self.continuous_mode = !self.mic_manually_disabled;
        self.input_suggestions.clear();
        self.last_suggestion_query.clear();
        self.auto_scroll_to_bottom = true;
        let prompt = self.expand_macro(&prompt_raw);
        self.live_transcript.clear();

        let norm_prompt = normalize_for_command_match(&prompt);
        if is_start_subtitles_command(&norm_prompt) {
            self.start_live_subtitles(None);
            return;
        } else if is_stop_subtitles_command(&norm_prompt) {
            self.stop_live_subtitles();
            return;
        }

        if norm_prompt == "mode sommeil" || norm_prompt == "mode nuit" || norm_prompt == "bonne nuit" || norm_prompt == "active le mode sommeil" {
            let res = crate::home_automation::execute_medical_scene("sleep");
            play_earcon(Earcon::Success);
            self.chat_history.push(ChatEntry {
                role: ChatRole::User,
                text: prompt,
                timestamp: current_time_str(),
                quick_suggestions: vec![QuickSuggestionItem::Text("Mode repas / travail".to_string())],
                screen_size_kb: None,
            });
            self.chat_history.push(ChatEntry {
                role: ChatRole::Agent,
                text: res.clone(),
                timestamp: current_time_str(),
                quick_suggestions: vec![QuickSuggestionItem::Text("Mode repas / travail".to_string())],
                screen_size_kb: None,
            });
            let _ = self.tts_sender.send(TtsCommand::Speak(res));
            self.status = AgentStatus::Idle;
            return;
        }
        if norm_prompt == "mode repas" || norm_prompt == "mode travail" || norm_prompt == "active le mode repas" || norm_prompt == "active le mode travail" {
            let res = crate::home_automation::execute_medical_scene("meal_work");
            play_earcon(Earcon::Success);
            self.chat_history.push(ChatEntry {
                role: ChatRole::User,
                text: prompt,
                timestamp: current_time_str(),
                quick_suggestions: vec![QuickSuggestionItem::Text("Mode sommeil".to_string())],
                screen_size_kb: None,
            });
            self.chat_history.push(ChatEntry {
                role: ChatRole::Agent,
                text: res.clone(),
                timestamp: current_time_str(),
                quick_suggestions: vec![QuickSuggestionItem::Text("Mode sommeil".to_string())],
                screen_size_kb: None,
            });
            let _ = self.tts_sender.send(TtsCommand::Speak(res));
            self.status = AgentStatus::Idle;
            return;
        }

        if let Some(card) = is_read_card_intent(&prompt, &self.floating_cards) {
            let speech = card.build_narrator_speech();
            play_earcon(Earcon::Success);
            self.status = AgentStatus::Speaking;
            self.chat_history.push(ChatEntry {
                role: ChatRole::User,
                text: prompt.clone(),
                timestamp: current_time_str(),
                quick_suggestions: Vec::new(),
                screen_size_kb: None,
            });
            self.chat_history.push(ChatEntry {
                role: ChatRole::Agent,
                text: speech.clone(),
                timestamp: current_time_str(),
                quick_suggestions: Vec::new(),
                screen_size_kb: None,
            });
            let _ = self.tts_sender.send(TtsCommand::Speak(speech));
            return;
        }

        if is_switch_access_command(&norm_prompt) {
            self.toggle_switch_access_mode(None);
            return;
        }

        if self.switch_access_mode {
            if let Some(idx) = extract_badge_number(&norm_prompt) {
                self.chat_history.push(ChatEntry {
                    role: ChatRole::User,
                    text: prompt,
                    timestamp: current_time_str(),
                    quick_suggestions: Vec::new(),
                    screen_size_kb: None,
                });
                self.click_numbered_target_by_index(idx);
                self.status = AgentStatus::Idle;
                return;
            }
        }

        if norm_prompt == "annule" || norm_prompt == "annuler" || norm_prompt == "undo" {
            let undo_feedback = if let Some(fb) = undo_last_system_action() {
                fb
            } else {
                #[cfg(windows)]
                send_hotkey(&[windows::Win32::UI::Input::KeyboardAndMouse::VIRTUAL_KEY(0x11)], windows::Win32::UI::Input::KeyboardAndMouse::VIRTUAL_KEY(0x5A));
                #[cfg(not(windows))]
                let _ = std::process::Command::new("xdotool").args(["key", "ctrl+z"]).status();
                t!("commands.last_action_undone").to_string()
            };
            play_earcon(Earcon::Success);
            self.chat_history.push(ChatEntry {
                role: ChatRole::User,
                text: prompt,
                timestamp: current_time_str(),
                quick_suggestions: Vec::new(),
                screen_size_kb: None,
            });
            self.chat_history.push(ChatEntry {
                role: ChatRole::Agent,
                text: undo_feedback.clone(),
                timestamp: current_time_str(),
                quick_suggestions: Vec::new(),
                screen_size_kb: None,
            });
            let _ = self.tts_sender.send(TtsCommand::Speak(undo_feedback));
            self.status = AgentStatus::Idle;
            return;
        }

        if let Some(feedback) = try_execute_local_navigation_shortcut(&prompt) {
            play_earcon(Earcon::Success);
            self.chat_history.push(ChatEntry {
                role: ChatRole::User,
                text: prompt,
                timestamp: current_time_str(),
                quick_suggestions: Vec::new(),
                screen_size_kb: None,
            });
            self.chat_history.push(ChatEntry {
                role: ChatRole::Agent,
                text: feedback.to_string(),
                timestamp: current_time_str(),
                quick_suggestions: Vec::new(),
                screen_size_kb: None,
            });
            let _ = self.tts_sender.send(TtsCommand::Speak(feedback.to_string()));
            self.status = AgentStatus::Idle;
            return;
        }

        self.chat_history.push(ChatEntry {
            role: ChatRole::User,
            text: prompt.clone(),
            timestamp: current_time_str(),
            quick_suggestions: Vec::new(),
            screen_size_kb: None,
        });

        if is_cancel_scheduled_command(&prompt) {
            let cancelled = cancel_all_scheduled_tasks();
            play_earcon(Earcon::Success);
            let reply = if cancelled > 0 {
                t!("speech.scheduled_tasks_cancelled_count", count = cancelled).to_string()
            } else {
                t!("speech.scheduled_tasks_cancelled").to_string()
            };
            self.chat_history.push(ChatEntry {
                role: ChatRole::Agent,
                text: reply.clone(),
                timestamp: current_time_str(),
                quick_suggestions: Vec::new(),
                screen_size_kb: None,
            });
            let _ = self.tts_sender.send(TtsCommand::Speak(reply));
            self.status = AgentStatus::Idle;
            self.continuous_mode = false;
            return;
        }

        if is_api_key_command(&prompt) {
            self.sync_modal_inputs_for_provider(self.llm_config.provider);
            self.show_api_key_modal = true;
            let reply = t!("speech.api_key_modal_opened").to_string();
            self.chat_history.push(ChatEntry {
                role: ChatRole::Agent,
                text: reply.clone(),
                timestamp: current_time_str(),
                quick_suggestions: Vec::new(),
                screen_size_kb: None,
            });
            let _ = self.tts_sender.send(TtsCommand::Speak(reply));
            self.status = AgentStatus::Idle;
            self.continuous_mode = false;
        } else if is_hide_command(&prompt) {
            self.is_hidden = true;
            self.status = AgentStatus::Idle;
            let reply = t!("speech.overlay_hidden").to_string();
            self.chat_history.push(ChatEntry {
                role: ChatRole::Agent,
                text: reply.clone(),
                timestamp: current_time_str(),
                quick_suggestions: Vec::new(),
                screen_size_kb: None,
            });
            let _ = self.tts_sender.send(TtsCommand::Speak(reply));
        } else {
            self.status = AgentStatus::Thinking;
            let _ = self.command_sender.send(AgentCommand::Prompt(prompt));
        }
    }

    /// Rendu du flux de discussion principal (Assistance ou Chat Twitch)
    pub fn render_chat_stream(
        &mut self,
        ui: &mut egui::Ui,
        messages_rect: egui::Rect,
        current_chat_h: f32,
        min_chat_h: f32,
        max_chat_h: f32,
        theme: &ThemeColors,
    ) -> f32 {
        let mut computed_chat_h = current_chat_h;

        ui.scope_builder(egui::UiBuilder::new().max_rect(messages_rect), |ui| {
            ui.set_clip_rect(messages_rect);
            match self.active_tab {
                ActiveTab::Assistance => {
                    if let Some(ref dl) = self.active_download {
                        draw_active_download_box(ui, theme, &dl.label, dl.percent, dl.downloaded_mb, dl.total_mb, dl.speed_mb_s);
                        ui.add_space(3.0);
                    }

                    if let Some(reminder_feedback) = draw_active_reminders_box(ui, theme, &mut self.dwell_state) {
                        self.chat_history.push(ChatEntry {
                            role: ChatRole::Agent,
                            text: reminder_feedback.clone(),
                            timestamp: current_time_str(),
                            quick_suggestions: Vec::new(),
                            screen_size_kb: None,
                        });
                        let _ = self.tts_sender.send(TtsCommand::Speak(reminder_feedback));
                    }
                    ui.add_space(2.0);

                    let max_scroll_h = (messages_rect.height()).max(30.0);
                    let scroll_out = egui::ScrollArea::vertical()
                        .id_salt("assistance_chat_scroll")
                        .stick_to_bottom(true)
                        .max_height(max_scroll_h)
                        .auto_shrink([false, false])
                        .show(ui, |ui| {
                            let last_user_idx = self.chat_history.iter().rposition(|e| e.role == ChatRole::User);
                            let last_agent_idx = self.chat_history.iter().rposition(|e| e.role == ChatRole::Agent);
                            let mut clicked_quick_suggestion: Option<String> = None;
                            let mut request_stop_speaking = false;
                            let mut speak_message: Option<String> = None;
                            let agent_display_name = self.current_agent_model_display_name();
                            let is_speaking = self.status == AgentStatus::Speaking;
                            let dwell_state = &mut self.dwell_state;

                            for (idx, entry) in self.chat_history.iter().enumerate() {
                                let is_highlighted = Some(idx) == last_user_idx || Some(idx) == last_agent_idx;
                                let msg_id = ui.make_persistent_id(format!("chat_entry_item_{idx}"));
                                let layer_id = ui.layer_id();
                                ui.memory_mut(|m| m.interested_in_focus(msg_id, layer_id));
                                let is_msg_focused = ui.memory(|m| m.has_focus(msg_id));

                                ui.vertical(|ui| {
                                    let (label_name, label_color) = match entry.role {
                                        ChatRole::User => {
                                            let name = if let Some(kb) = entry.screen_size_kb {
                                                format!("{} - {:.1} ko", t!("ui.chat_user_label"), kb)
                                            } else {
                                                t!("ui.chat_user_label").to_string()
                                            };
                                            (name, egui::Color32::from_rgb(255, 215, 120))
                                        }
                                        ChatRole::Agent => (agent_display_name.clone(), theme.accent),
                                    };

                                    ui.horizontal(|ui| {
                                        ui.label(
                                            egui::RichText::new(&label_name)
                                                .size(if is_highlighted { 11.5 } else { 10.0 })
                                                .color(label_color)
                                                .strong(),
                                        );
                                        ui.label(
                                            egui::RichText::new(&entry.timestamp)
                                                .size(if is_highlighted { 10.0 } else { 9.0 })
                                                .color(egui::Color32::from_rgba_unmultiplied(170, 185, 205, 140)),
                                        );

                                        if entry.role == ChatRole::Agent && Some(idx) == last_agent_idx && is_speaking {
                                            ui.add_space(4.0);
                                            let stop_btn = ui.add(
                                                egui::Button::new(
                                                    egui::RichText::new(t!("ui.btn_mute_speech"))
                                                        .size(9.5)
                                                        .color(egui::Color32::from_rgb(255, 130, 130)),
                                                )
                                                    .small()
                                                    .corner_radius(egui::CornerRadius::same(6)),
                                            );
                                            let stop_key = crate::keyboard_hook::vk_to_label(crate::keyboard_hook::KEY_BIND_STOP_SPEECH.load(Ordering::Relaxed));
                                            if stop_btn.on_hover_text(format!("{} [{}]", t!("ui.avatar_mute_tooltip"), stop_key)).clicked() {
                                                request_stop_speaking = true;
                                            }
                                        }
                                    });

                                    let (text_size, line_height, text_color, is_strong) = match entry.role {
                                        ChatRole::User => {
                                            if is_highlighted {
                                                (14.5, 20.0, theme.text_user, true)
                                            } else {
                                                (12.0, 16.5, egui::Color32::from_rgba_unmultiplied(225, 210, 160, 185), false)
                                            }
                                        }
                                        ChatRole::Agent => {
                                            if is_highlighted {
                                                (15.5, 22.0, egui::Color32::from_rgb(255, 255, 255), true)
                                            } else {
                                                (12.5, 17.5, egui::Color32::from_rgba_unmultiplied(200, 210, 225, 180), false)
                                            }
                                        }
                                    };

                                    let has_interactive_md = (entry.text.contains('[') && entry.text.contains("]("))
                                        || entry.text.contains("[link")
                                        || entry.text.contains("[/link]")
                                        || entry.text.contains("```")
                                        || entry.text.contains("▶ ")
                                        || entry.text.contains("🎬 ")
                                        || entry.text.contains("🎵 ")
                                        || entry.text.contains("📄 ")
                                        || entry.text.contains("🖼")
                                        || (entry.text.contains('|') && entry.text.contains('\n'));

                                    if entry.role == ChatRole::Agent && has_interactive_md {
                                        if let Some(prompt) = floating_window::render_markdown(ui, &entry.text, &format!("chat_entry_{idx}"), dwell_state, theme) {
                                            clicked_quick_suggestion = Some(prompt);
                                        }
                                    } else {
                                        let mut rich = egui::RichText::new(&entry.text)
                                            .size(text_size)
                                            .line_height(Some(line_height))
                                            .color(text_color);
                                        if is_strong {
                                            rich = rich.strong();
                                        }
                                        let label_resp = ui.add(egui::Label::new(rich).wrap());
                                        let interact_resp = ui.interact(label_resp.rect, msg_id, egui::Sense::click());
                                        interact_resp.widget_info(|| {
                                            egui::WidgetInfo::labeled(
                                                egui::Role::Label,
                                                ui.is_enabled(),
                                                format!("{}: {}", label_name, entry.text),
                                            )
                                        });

                                        let msg_dwell_id = ui.make_persistent_id(format!("dwell_chat_msg_{idx}"));
                                        let dwell_triggered = apply_dwell_to_response(ui, dwell_state, msg_dwell_id, &interact_resp);
                                        if dwell_triggered {
                                            dwell_state.lock_id(msg_dwell_id, Duration::from_secs(6));
                                            let speech = if let Some(sug_speech) = format_suggestions_for_speech(&entry.quick_suggestions) {
                                                format!("{}\n\n{}", entry.text, sug_speech)
                                            } else {
                                                entry.text.clone()
                                            };
                                            speak_message = Some(speech);
                                        }

                                        if interact_resp.clicked() {
                                            ui.memory_mut(|m| m.request_focus(msg_id));
                                        }
                                        let effective_focused = is_msg_focused || interact_resp.has_focus();

                                        let copy_msg_hit = ui.ctx().input(|i| {
                                            i.events.iter().any(|e| match e {
                                                egui::Event::Copy => true,
                                                egui::Event::Key { key: egui::Key::C, pressed: true, modifiers, .. } if modifiers.ctrl || modifiers.command => true,
                                                _ => false,
                                            })
                                        });
                                        let should_copy = effective_focused
                                            || (interact_resp.hovered() && ui.memory(|m| m.focused().is_none()));

                                        if should_copy && copy_msg_hit {
                                            set_clipboard_text(&entry.text);
                                            ui.ctx().copy_text(entry.text.clone());
                                            play_earcon(Earcon::Success);
                                        }

                                        if effective_focused {
                                            label_resp.scroll_to_me(Some(egui::Align::Center));
                                            ui.painter().rect_stroke(
                                                label_resp.rect.expand(4.0),
                                                egui::CornerRadius::same(6),
                                                egui::Stroke::new(1.8, theme.accent),
                                                egui::StrokeKind::Outside,
                                            );
                                        }

                                        interact_resp.context_menu(|ui| {
                                            if ui.button(t!("ui.ctx_copy_text")).clicked() {
                                                set_clipboard_text(&entry.text);
                                                ui.ctx().copy_text(entry.text.clone());
                                                play_earcon(Earcon::Success);
                                                ui.close();
                                            }
                                        });
                                    }

                                    if !entry.quick_suggestions.is_empty() {
                                        ui.add_space(4.0);
                                        ui.horizontal_wrapped(|ui| {
                                            for (sug_idx, sug) in entry.quick_suggestions.iter().enumerate() {
                                                if let Some(clean_label) = sug.to_display_string() {
                                                    let clean_label = clean_label.trim();
                                                    if clean_label.is_empty() {
                                                        continue;
                                                    }
                                                    let btn_text = egui::RichText::new(format!("💡 {clean_label}"))
                                                        .size(11.0)
                                                        .color(theme.accent);

                                                    let btn = egui::Button::new(btn_text)
                                                        .stroke(egui::Stroke::new(1.0, theme.border))
                                                        .corner_radius(egui::CornerRadius::same(10));

                                                    let resp = ui.add(btn).on_hover_text(t!("ui.quick_suggestion_tooltip"));
                                                    let sug_dwell_id = ui.make_persistent_id(format!("dwell_quick_sug_{idx}_{sug_idx}"));
                                                    let is_quick_hit = make_widget_accessible_and_activated(ui, &resp, theme.accent);
                                                    if apply_dwell_to_response(ui, dwell_state, sug_dwell_id, &resp) || is_quick_hit {
                                                        clicked_quick_suggestion = Some(clean_label.to_string());
                                                    }
                                                }
                                            }
                                        });
                                    }
                                });
                                ui.add_space(if is_highlighted { 8.0 } else { 5.0 });
                            }

                            if let Some(prompt) = clicked_quick_suggestion {
                                self.submit_prompt(prompt);
                            }

                            if let Some(text_to_speak) = speak_message {
                                self.stop_speaking();
                                play_earcon(Earcon::Success);
                                let clean_text = sanitize_for_tts(&text_to_speak);
                                if !clean_text.trim().is_empty() {
                                    self.status = AgentStatus::Speaking;
                                    let _ = self.tts_sender.send(TtsCommand::Speak(clean_text));
                                    ui.ctx().request_repaint();
                                }
                            }

                            if request_stop_speaking {
                                self.stop_speaking();
                            }

                            if self.auto_scroll_to_bottom || self.is_streaming_narration || is_speaking {
                                ui.scroll_to_cursor(Some(egui::Align::BOTTOM));
                            }
                        });
                    if self.auto_scroll_to_bottom {
                        self.auto_scroll_to_bottom = false;
                    }
                    computed_chat_h = (scroll_out.content_size.y + 44.0).clamp(min_chat_h, max_chat_h);
                }
                ActiveTab::Chat => {
                    ui.horizontal(|ui| {
                        if !self.current_twitch_channel.is_empty() {
                            ui.label(
                                egui::RichText::new(t!("ui.twitch_channel_label"))
                                    .size(11.0)
                                    .color(egui::Color32::from_rgb(170, 185, 205)),
                            );
                            let ch_link = ui.add(
                                egui::Label::new(
                                    egui::RichText::new(format!("#{}", self.current_twitch_channel))
                                        .size(11.0)
                                        .color(egui::Color32::from_rgb(0, 195, 255))
                                        .underline(),
                                )
                                    .sense(egui::Sense::click()),
                            ).on_hover_cursor(egui::CursorIcon::PointingHand);
                            if ch_link.clicked() {
                                open_browser_smart(&format!("https://twitch.tv/{}", self.current_twitch_channel));
                            }
                            ui.separator();
                        }

                        let search_id = egui::Id::NULL.with("twitch_search_query_input");
                        let search_hint = t!("ui.twitch_search_hint").to_string();
                        let search_edit = render_accessible_input(
                            ui,
                            search_id,
                            &mut self.twitch_search_query,
                            &search_hint,
                            false,
                            Some(110.0),
                            Some(18.0),
                            false,
                            theme,
                        );
                        let enter_pressed = search_edit.lost_focus() && ui.input(|i| {
                            i.events.iter().any(|e| match e {
                                egui::Event::Key { key: egui::Key::Enter, pressed: true, .. } => true,
                                egui::Event::Text(t) if t == "\n" || t == "\r" => true,
                                _ => false,
                            })
                        });
                        if ui.small_button("🔍").clicked() || enter_pressed {
                            let _ = self.command_sender.send(AgentCommand::SearchTwitch(
                                self.twitch_search_query.trim().to_string(),
                            ));
                        }
                        if !self.twitch_search_results.is_empty() {
                            if ui.small_button("✕").on_hover_text(t!("ui.btn_close")).clicked() {
                                self.twitch_search_results.clear();
                            }
                        }
                    });

                    let mut search_viewport_h = 0.0;
                    if !self.twitch_search_results.is_empty() {
                        ui.add_space(3.0);
                        let live_count = self.twitch_search_results.iter().filter(|c| c.is_live).count();
                        let total_count = self.twitch_search_results.len();

                        draw_twitch_filter_bar(ui, &mut self.twitch_filter, live_count, total_count, theme);
                        ui.add_space(2.0);

                        let max_search_h = if self.twitch_messages.is_empty() {
                            (messages_rect.height() - 65.0).max(60.0)
                        } else {
                            ((messages_rect.height() - 75.0) * 0.45).clamp(55.0, 130.0)
                        };

                        let mut selected_channel = None;
                        let search_scroll = egui::ScrollArea::vertical()
                            .id_salt("twitch_search_results_scroll")
                            .max_height(max_search_h)
                            .auto_shrink([false, false])
                            .show(ui, |ui| {
                                let btn_w = 24.0;
                                let spacing = 4.0;
                                let total_row_w = ui.available_width();
                                let item_w = (total_row_w - btn_w - spacing).max(80.0);

                                for ch in self.twitch_search_results.iter().filter(|c| self.twitch_filter.matches(c)) {
                                    ui.horizontal(|ui| {
                                        ui.spacing_mut().item_spacing.x = spacing;
                                        let resp = draw_twitch_channel_search_item(ui, ch, theme, item_w);
                                        if resp.clicked() {
                                            selected_channel = Some(ch.login.clone());
                                        }
                                        if ui.add_sized([btn_w, 28.0], egui::Button::new("↗")).on_hover_text(t!("ui.twitch_open_tooltip")).clicked() {
                                            open_browser_smart(&format!("https://twitch.tv/{}", ch.login));
                                        }
                                    });
                                    ui.add_space(1.0);
                                }
                            });

                        search_viewport_h = search_scroll.content_size.y.min(max_search_h) + 36.0;

                        if let Some(ch) = selected_channel {
                            self.current_twitch_channel = ch.clone();
                            let _ = self.twitch_channel_sender.send(ch);
                            self.twitch_messages.clear();
                            self.twitch_search_results.clear();
                        }
                    }
                    ui.separator();

                    let mut channel_header_h = 0.0;
                    if !self.current_twitch_channel.is_empty() {
                        ui.horizontal(|ui| {
                            ui.label(
                                egui::RichText::new(format!("💬 Chat #{}", self.current_twitch_channel))
                                    .size(11.0)
                                    .strong()
                                    .color(egui::Color32::from_rgb(169, 112, 255)),
                            );
                            ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                                let join_btn = ui.add(
                                    egui::Button::new(
                                        egui::RichText::new("🌐 Rejoindre sur Twitch")
                                            .size(10.5)
                                            .strong()
                                            .color(egui::Color32::from_rgb(220, 245, 255)),
                                    )
                                        .fill(egui::Color32::from_rgba_unmultiplied(145, 70, 255, 60))
                                        .stroke(egui::Stroke::new(1.0, egui::Color32::from_rgb(145, 70, 255)))
                                        .corner_radius(egui::CornerRadius::same(6)),
                                ).on_hover_text(format!("Ouvrir le stream de {} sur Twitch dans le navigateur", self.current_twitch_channel));
                                if join_btn.clicked() {
                                    open_browser_smart(&format!("https://twitch.tv/{}", self.current_twitch_channel));
                                }
                            });
                        });
                        ui.add_space(2.0);
                        channel_header_h = 24.0;
                    }

                    let max_chat_scroll_h = (messages_rect.height() - 28.0 - search_viewport_h - channel_header_h).max(30.0);
                    let scroll_out = egui::ScrollArea::vertical()
                        .id_salt("twitch_chat_scroll")
                        .stick_to_bottom(true)
                        .max_height(max_chat_scroll_h)
                        .auto_shrink([false, false])
                        .show(ui, |ui| {
                            if self.twitch_messages.is_empty() {
                                ui.label(
                                    egui::RichText::new(t!("ui.twitch_waiting"))
                                        .size(12.0)
                                        .italics()
                                        .color(egui::Color32::from_rgba_unmultiplied(180, 190, 205, 150)),
                                );
                            } else {
                                let mut speak_twitch_message: Option<String> = None;
                                for (idx, msg) in self.twitch_messages.iter().enumerate() {
                                    let t_id = ui.make_persistent_id(format!("twitch_msg_item_{idx}"));
                                    let layer_id = ui.layer_id();
                                    ui.memory_mut(|m| m.interested_in_focus(t_id, layer_id));
                                    let is_t_focused = ui.memory(|m| m.has_focus(t_id));
                                    let resp = ui.horizontal_wrapped(|ui| {
                                        ui.label(
                                            egui::RichText::new(&msg.timestamp)
                                                .size(9.0)
                                                .color(egui::Color32::from_rgba_unmultiplied(140, 150, 170, 140)),
                                        );
                                        ui.label(
                                            egui::RichText::new(format!("{}:", msg.author))
                                                .size(12.0)
                                                .color(egui::Color32::from_rgb(169, 112, 255))
                                                .strong(),
                                        );
                                        ui.label(
                                            egui::RichText::new(&msg.text)
                                                .size(12.5)
                                                .color(egui::Color32::WHITE),
                                        );
                                    }).response;
                                    let resp = ui.interact(resp.rect, t_id, egui::Sense::click());
                                    resp.widget_info(|| {
                                        egui::WidgetInfo::labeled(
                                            egui::Role::Label,
                                            ui.is_enabled(),
                                            format!("{}: {}", msg.author, msg.text),
                                        )
                                    });

                                    let t_dwell_id = ui.make_persistent_id(format!("dwell_twitch_msg_{idx}"));
                                    if apply_dwell_to_response(ui, &mut self.dwell_state, t_dwell_id, &resp) {
                                        self.dwell_state.lock_id(t_dwell_id, Duration::from_secs(6));
                                        speak_twitch_message = Some(format!("{}: {}", msg.author, msg.text));
                                    }

                                    if resp.clicked() {
                                        ui.memory_mut(|m| m.request_focus(t_id));
                                    }
                                    let effective_t_focused = is_t_focused || resp.has_focus();
                                    let copy_t_hit = ui.ctx().input(|i| {
                                        i.events.iter().any(|e| match e {
                                            egui::Event::Copy => true,
                                            egui::Event::Key { key: egui::Key::C, pressed: true, modifiers, .. } if modifiers.ctrl || modifiers.command => true,
                                            _ => false,
                                        })
                                    });
                                    if (effective_t_focused || resp.hovered()) && copy_t_hit {
                                        let text = format!("{}: {}", msg.author, msg.text);
                                        set_clipboard_text(&text);
                                        ui.ctx().copy_text(text);
                                        play_earcon(Earcon::Success);
                                    }
                                    if effective_t_focused {
                                        resp.scroll_to_me(Some(egui::Align::Center));
                                        ui.painter().rect_stroke(
                                            resp.rect.expand(2.0),
                                            egui::CornerRadius::same(3),
                                            egui::Stroke::new(1.2, egui::Color32::from_rgb(0, 200, 255)),
                                            egui::StrokeKind::Outside,
                                        );
                                    }
                                    ui.add_space(3.0);
                                }

                                if let Some(t_speech) = speak_twitch_message {
                                    self.stop_speaking();
                                    play_earcon(Earcon::Success);
                                    let clean_speech = sanitize_for_tts(&t_speech);
                                    if !clean_speech.trim().is_empty() {
                                        self.status = AgentStatus::Speaking;
                                        let _ = self.tts_sender.send(TtsCommand::Speak(clean_speech));
                                        ui.ctx().request_repaint();
                                    }
                                }

                                if self.auto_scroll_to_bottom {
                                    ui.scroll_to_cursor(Some(egui::Align::BOTTOM));
                                }
                            }
                        });
                    if self.auto_scroll_to_bottom {
                        self.auto_scroll_to_bottom = false;
                    }
                    computed_chat_h = (scroll_out.content_size.y + search_viewport_h + 72.0).clamp(min_chat_h, max_chat_h);
                }
            }
        });

        computed_chat_h
    }

    pub fn render_hidden_state(&mut self, ui: &mut egui::Ui, ctx: &egui::Context, theme: &ThemeColors) {
        self.sync_switch_access_overlays();
        if crate::eyetracking::should_show_eye_debug() {
            crate::eyetracking::show_eye_debug_floating_window(ctx, theme);
        }
        egui::CentralPanel::default()
            .frame(egui::Frame::new().fill(egui::Color32::TRANSPARENT))
            .show(ui, |ui| {
                #[cfg(not(windows))]
                if self.switch_access_mode {
                    let state = crate::window_util::get_overlay_draw_state();
                    if state.is_visible {
                        let painter = ui.painter();
                        let ppp = ctx.pixels_per_point().max(0.5);

                        if let Some([l, t, r, b]) = state.orange_border {
                            let win_rect = egui::Rect::from_min_max(
                                egui::pos2(l as f32 / ppp, t as f32 / ppp),
                                egui::pos2(r as f32 / ppp, b as f32 / ppp),
                            );
                            painter.rect_stroke(
                                win_rect.expand(2.0),
                                egui::CornerRadius::same(8),
                                egui::Stroke::new(3.5, egui::Color32::from_rgb(255, 140, 0)),
                                egui::StrokeKind::Outside,
                            );
                        }

                        for zone in &state.zones {
                            let z_rect = egui::Rect::from_min_max(
                                egui::pos2(zone.rect[0] as f32 / ppp, zone.rect[1] as f32 / ppp),
                                egui::pos2(zone.rect[2] as f32 / ppp, zone.rect[3] as f32 / ppp),
                            );
                            let (hex, _) = (crate::ocr::visual_zone_color(zone.index), ());
                            let r = u8::from_str_radix(&hex[1..3], 16).unwrap_or(40);
                            let g = u8::from_str_radix(&hex[3..5], 16).unwrap_or(200);
                            let b = u8::from_str_radix(&hex[5..7], 16).unwrap_or(255);
                            let z_color = egui::Color32::from_rgb(r, g, b);

                            let stroke_w = if zone.is_focused { 4.0 } else { 2.0 };
                            painter.rect_stroke(
                                z_rect,
                                egui::CornerRadius::same(8),
                                egui::Stroke::new(stroke_w, z_color),
                                egui::StrokeKind::Inside,
                            );

                            let title_text = if zone.title.starts_with(|c: char| c.is_ascii_digit()) {
                                zone.title.clone()
                            } else {
                                format!("{}. {}", zone.index, zone.title)
                            };
                            let badge_pos = egui::pos2(z_rect.min.x, (z_rect.min.y - 24.0).max(4.0));
                            let badge_rect = egui::Rect::from_min_size(badge_pos, egui::vec2(title_text.len() as f32 * 8.5 + 16.0, 22.0));
                            painter.rect_filled(badge_rect, egui::CornerRadius::same(4), z_color);
                            painter.text(
                                badge_rect.center(),
                                egui::Align2::CENTER_CENTER,
                                title_text,
                                egui::FontId::proportional(12.0),
                                egui::Color32::BLACK,
                            );
                        }

                        if let Some(([l, t, r_b, b_b], is_reader)) = state.focused_border {
                            let t_rect = egui::Rect::from_min_max(
                                egui::pos2(l as f32 / ppp, t as f32 / ppp),
                                egui::pos2(r_b as f32 / ppp, b_b as f32 / ppp),
                            );
                            let color = if is_reader {
                                egui::Color32::from_rgb(0, 230, 255)
                            } else {
                                egui::Color32::from_rgb(255, 140, 0)
                            };
                            painter.rect_stroke(
                                t_rect.expand(2.0),
                                egui::CornerRadius::same(6),
                                egui::Stroke::new(3.0, color),
                                egui::StrokeKind::Outside,
                            );
                        }

                        let badge_h = (28.0 * self.zoom_factor).clamp(24.0, 48.0);
                        for target in &state.targets {
                            let is_focused = state.focused_index == Some(target.index);
                            let hex = if target.value.starts_with('#') {
                                target.value.as_str()
                            } else {
                                crate::ocr::visual_zone_color(target.index)
                            };
                            let cr = u8::from_str_radix(&hex[1..3], 16).unwrap_or(255);
                            let cg = u8::from_str_radix(&hex[3..5], 16).unwrap_or(214);
                            let cb = u8::from_str_radix(&hex[5..7], 16).unwrap_or(0);
                            let b_color = egui::Color32::from_rgb(cr, cg, cb);

                            let bx = target.rect[0] as f32 / ppp;
                            let by = (target.rect[1] as f32 / ppp - badge_h / 2.0).max(2.0);
                            let badge_w = if target.role == "lecture textuelle" || target.index < 10 {
                                badge_h
                            } else if target.index < 100 {
                                badge_h * 1.30
                            } else {
                                badge_h * 1.65
                            };
                            let badge_rect = egui::Rect::from_min_size(egui::pos2(bx, by), egui::vec2(badge_w, badge_h));

                            let stroke = if is_focused {
                                egui::Stroke::new(3.5, egui::Color32::BLACK)
                            } else {
                                egui::Stroke::new(1.8, egui::Color32::BLACK)
                            };
                            painter.rect(
                                badge_rect,
                                egui::CornerRadius::same((badge_h / 2.0).round() as u8),
                                b_color,
                                stroke,
                                egui::StrokeKind::Inside,
                            );

                            let label = if target.role == "lecture textuelle" {
                                "📖".to_string()
                            } else {
                                target.index.to_string()
                            };
                            painter.text(
                                badge_rect.center(),
                                egui::Align2::CENTER_CENTER,
                                label,
                                egui::FontId::proportional(badge_h * 0.55),
                                egui::Color32::BLACK,
                            );
                        }
                    }
                }
            });
    }

    pub fn render_calibration_screen(&mut self, ui: &mut egui::Ui, ctx: &egui::Context, work_area: &WorkArea) {
        if ctx.input(|i| i.key_pressed(egui::Key::Escape)) {
            self.eye_tracker.cancel_calibration();
            crate::eyetracking::set_eye_tracking_enabled(false);
            if let Ok(mut lock) = crate::eyetracking::GLOBAL_GAZE_POS.lock() {
                *lock = None;
            }
            self.position_initialized = false;
            play_earcon(Earcon::MicClose);
            ctx.request_repaint();
            return;
        }

        let painter = ui.painter();
        let (scr_w, scr_h) = (work_area.w, work_area.h);
        if let Some((target_x, target_y)) = self.eye_tracker.calibration.current_screen_target(scr_w, scr_h) {
            let center = egui::pos2(target_x, target_y);
            let progress = self.eye_tracker.calibration.progress();
            let p_idx = self.eye_tracker.calibration.current_point_index + 1;

            painter.rect_filled(
                egui::Rect::from_min_size(egui::pos2(0.0, 0.0), egui::vec2(scr_w, scr_h)),
                egui::CornerRadius::ZERO,
                egui::Color32::from_rgba_unmultiplied(10, 12, 16, 225),
            );

            painter.circle_stroke(
                center,
                32.0,
                egui::Stroke::new(3.0, egui::Color32::from_rgb(0, 180, 255)),
            );
            painter.circle_filled(
                center,
                30.0 * progress,
                egui::Color32::from_rgba_unmultiplied(0, 230, 118, 180),
            );
            painter.circle_filled(center, 7.0, egui::Color32::WHITE);

            let text = t!("eyetracking.calibration_instruction", current = p_idx, total = 9).to_string();
            painter.text(
                egui::pos2(center.x, center.y + 44.0),
                egui::Align2::CENTER_CENTER,
                text,
                egui::FontId::proportional(18.0),
                egui::Color32::WHITE,
            );
        } else {
            self.eye_tracker.calibration.is_active = false;
            self.position_initialized = false;
            ctx.request_repaint();
        }
        ctx.request_repaint();
    }

    pub fn render_gaze_reticle(&self, ui: &egui::Ui) {
        if let Ok(gaze_guard) = crate::eyetracking::GLOBAL_GAZE_POS.lock() {
            if let Some((gx, gy)) = *gaze_guard {
                let p = ui.painter();
                let center = egui::pos2(gx, gy);
                let is_mouse_active = self.eye_tracker.is_mouse_overridden();
                let is_snapped = self.eye_tracker.last_snapped_target_id.is_some() && self.eye_tracker.snapping_enabled && !is_mouse_active;
                let (reticle_radius, stroke_color, core_color) = if is_mouse_active {
                    (14.0, egui::Color32::from_rgba_unmultiplied(255, 170, 0, 120), egui::Color32::from_rgb(255, 170, 0))
                } else if is_snapped {
                    (22.0, egui::Color32::from_rgb(0, 230, 118), egui::Color32::from_rgb(0, 230, 118))
                } else {
                    (18.0, egui::Color32::from_rgba_unmultiplied(0, 229, 255, 140), egui::Color32::from_rgb(0, 229, 255))
                };
                p.circle_stroke(center, reticle_radius, egui::Stroke::new(if is_mouse_active { 1.2 } else { 2.0 }, stroke_color));
                p.circle_filled(center, 4.0, core_color);
            }
        }
    }

    pub fn render_switch_overlays(&self, painter: &egui::Painter, ctx: &egui::Context) {
        if self.switch_access_mode && self.switch_step == SwitchAccessStep::Zones && !self.switch_zones.is_empty() {
            #[cfg(not(windows))]
            {
                let cur_zone = &self.switch_zones[self.switch_focus_index % self.switch_zones.len()];
                let ppp = ctx.pixels_per_point().max(0.5);
                let window_origin = self.window_pos.unwrap_or_default();
                let z_rect = egui::Rect::from_min_max(
                    egui::pos2((cur_zone.rect[0] as f32 / ppp) - window_origin.x, (cur_zone.rect[1] as f32 / ppp) - window_origin.y),
                    egui::pos2((cur_zone.rect[2] as f32 / ppp) - window_origin.x, (cur_zone.rect[3] as f32 / ppp) - window_origin.y),
                );
                painter.rect_stroke(
                    z_rect.expand(6.0),
                    egui::CornerRadius::same(8),
                    egui::Stroke::new(2.5, egui::Color32::from_rgb(0, 200, 255)),
                    egui::StrokeKind::Outside,
                );
            }
        }

        if self.switch_access_mode && self.switch_step == SwitchAccessStep::Elements && !self.numbered_targets.is_empty() {
            #[cfg(not(windows))]
            {
                let cur_target = &self.numbered_targets[self.switch_focus_index % self.numbered_targets.len()];
                let ppp = ctx.pixels_per_point().max(0.5);
                let window_origin = self.window_pos.unwrap_or_default();
                let t_rect = egui::Rect::from_min_max(
                    egui::pos2((cur_target.rect[0] as f32 / ppp) - window_origin.x, (cur_target.rect[1] as f32 / ppp) - window_origin.y),
                    egui::pos2((cur_target.rect[2] as f32 / ppp) - window_origin.x, (cur_target.rect[3] as f32 / ppp) - window_origin.y),
                );
                let stroke_color = if cur_target.role == "lecture textuelle" {
                    egui::Color32::from_rgb(0, 230, 255)
                } else {
                    egui::Color32::from_rgb(255, 140, 0)
                };
                painter.rect_stroke(
                    t_rect.expand(3.0),
                    egui::CornerRadius::same(6),
                    egui::Stroke::new(2.5, stroke_color),
                    egui::StrokeKind::Outside,
                );
            }
        }

        if self.dictation_mode {
            if let Ok(guard) = LAST_FOCUSED_RECT.lock() {
                if let Some([left, top, right, bottom]) = *guard {
                    let ppp = ctx.pixels_per_point().max(0.5);
                    let window_origin = self.window_pos.unwrap_or_default();
                    let screen_rect = egui::Rect::from_min_max(
                        egui::pos2((left as f32 / ppp) - window_origin.x, (top as f32 / ppp) - window_origin.y),
                        egui::pos2((right as f32 / ppp) - window_origin.x, (bottom as f32 / ppp) - window_origin.y),
                    );
                    painter.rect_stroke(
                        screen_rect,
                        egui::CornerRadius::same(4),
                        egui::Stroke::new(2.0, egui::Color32::from_rgb(255, 205, 70)),
                        egui::StrokeKind::Outside,
                    );
                }
            }
        }
    }

    pub fn process_floating_cards(&mut self, ctx: &egui::Context, theme: &ThemeColors) {
        let mut submitted_card_prompts = Vec::new();
        let mut card_speech = None;
        let prev_card_count = self.floating_cards.len();
        let had_subtitles = self.floating_cards.iter().any(|c| matches!(c.content, FloatingContentType::Subtitles { .. }));
        let dwell_state = &mut self.dwell_state;
        let theme_colors = self.theme.colors();
        self.floating_cards.retain_mut(|card| {
            if let Some(out) = card.show(ctx, dwell_state, &theme_colors) {
                match out {
                    floating_window::FloatingCardOutput::Prompt(prompt) => submitted_card_prompts.push(prompt),
                    floating_window::FloatingCardOutput::Speak(speech) => card_speech = Some(speech),
                }
            }
            card.open
        });
        let has_subtitles = self.floating_cards.iter().any(|c| matches!(c.content, FloatingContentType::Subtitles { .. }));
        if had_subtitles && !has_subtitles {
            floating_window::stop_live_subtitles();
            crate::audio::stop_live_subtitles();
        }
        if self.floating_cards.len() != prev_card_count {
            update_open_floating_cards(&self.floating_cards);
            ctx.request_repaint();
        }
        if let Some(speech) = card_speech {
            self.stop_speaking();
            play_earcon(Earcon::Success);
            self.status = AgentStatus::Speaking;
            self.chat_history.push(ChatEntry {
                role: ChatRole::Agent,
                text: speech.clone(),
                timestamp: current_time_str(),
                quick_suggestions: Vec::new(),
                screen_size_kb: None,
            });
            let _ = self.tts_sender.send(TtsCommand::Speak(speech));
        }
        for prompt in submitted_card_prompts {
            self.submit_prompt(prompt);
        }
    }
}