use std::sync::atomic::Ordering;
use std::time::Duration;
use eframe::egui;

use crate::automation::{
    get_clipboard_text, get_plain_clipboard_text, set_clipboard_text, SWITCH_DEBOUNCE_MS,
    SWITCH_LONG_PRESS_MS,
};
use crate::fs_util::{read_env_var_from_file, save_macros, write_env_var_to_file};
use crate::local_llm;
use crate::locale::get_system_locale_language;
use crate::overlay::{apply_dwell_to_response, play_earcon, resolve_llm_config, Earcon};
use crate::theme::{localized_theme_desc, localized_theme_name, AppTheme, SettingsTab, ThemeColors};
use crate::types::{AgentEvent, LlmProvider};
use crate::OverlayApp;

impl OverlayApp {
    pub fn sync_modal_inputs_for_provider(&mut self, provider: LlmProvider) {
        self.selected_provider_modal = provider;
        let key_name = provider.env_key_name();
        self.api_key_input = std::env::var(key_name)
            .or_else(|_| read_env_var_from_file(".env", key_name).ok_or(()))
            .unwrap_or_default();

        let model_var = provider.env_model_name();
        self.model_input = std::env::var(model_var)
            .ok()
            .or_else(|| read_env_var_from_file(".env", model_var))
            .or_else(|| std::env::var("LLM_MODEL").ok())
            .or_else(|| read_env_var_from_file(".env", "LLM_MODEL"))
            .map(|s| s.trim().to_string())
            .filter(|s| !s.is_empty())
            .unwrap_or_else(|| provider.default_model().to_string());

        let endpoint_var = provider.env_endpoint_name();
        self.endpoint_input = std::env::var(endpoint_var)
            .ok()
            .or_else(|| read_env_var_from_file(".env", endpoint_var))
            .map(|s| s.trim().to_string())
            .filter(|s| !s.is_empty())
            .unwrap_or_else(|| provider.default_endpoint().to_string());

        if provider == LlmProvider::Local {
            self.model_input = local_llm::get_active_preset().name.to_string();
        }

        self.api_key_error = None;
    }

    pub fn refresh_devices(&mut self) {
        self.detected_microphones = crate::devices::list_microphones();
        self.selected_microphone = crate::devices::get_active_microphone();
        self.detected_pointing_devices = crate::devices::list_pointing_devices();
    }

    pub fn set_theme(&mut self, theme: AppTheme) {
        self.theme = theme;
        let _ = write_env_var_to_file(".env", "THEME", theme.key());
        std::env::set_var("THEME", theme.key());
    }

    pub fn current_agent_model_display_name(&self) -> String {
        if self.llm_config.provider == LlmProvider::Local {
            local_llm::get_active_preset().name.to_string()
        } else if !self.llm_config.model.is_empty() {
            format!("{} ({})", self.llm_config.provider.display_name(), self.llm_config.model)
        } else {
            self.llm_config.provider.display_name().to_string()
        }
    }

    pub fn set_zoom_factor(&mut self, factor: f32) {
        let clamped = factor.clamp(0.7, 2.5);
        if (self.zoom_factor - clamped).abs() > 0.001 {
            self.zoom_factor = clamped;
            let val_str = format!("{:.2}", clamped);
            let _ = write_env_var_to_file(".env", "FONT_SIZE", &val_str);
            std::env::set_var("FONT_SIZE", &val_str);
        }
    }

    pub fn set_app_language(&mut self, lang: &str) {
        self.app_language = lang.to_string();
        std::env::set_var("APP_LANG", lang);
        let _ = write_env_var_to_file(".env", "APP_LANG", lang);
        let active_locale = if lang != "auto" {
            lang.to_string()
        } else {
            let (locale, _) = get_system_locale_language();
            locale.split(['-', '_']).next().unwrap_or("fr").to_lowercase()
        };
        rust_i18n::set_locale(&active_locale);
    }

    pub fn render_settings_modal(
        &mut self,
        ctx: &egui::Context,
        theme: &ThemeColors,
        win_w: f32,
        active_win_h: f32,
    ) {
        let modal_w = (win_w - 20.0).max(340.0);
        let modal_h = (active_win_h - 16.0).max(180.0);
        egui::Window::new("settings_modal")
            .title_bar(false)
            .collapsible(false)
            .resizable(false)
            .anchor(egui::Align2::CENTER_CENTER, egui::vec2(0.0, 0.0))
            .fixed_size(egui::vec2(modal_w, modal_h))
            .frame(
                egui::Frame::new()
                    .fill(theme.modal_bg)
                    .stroke(egui::Stroke::new(1.2, theme.border))
                    .corner_radius(egui::CornerRadius::same(10))
                    .inner_margin(egui::Margin::symmetric(12, 10)),
            )
            .show(ctx, |ui| {
                ui.style_mut().visuals.window_fill = theme.control_bg;
                ui.style_mut().visuals.window_stroke = egui::Stroke::new(1.0, theme.border);
                ui.style_mut().visuals.widgets.inactive.bg_fill = theme.control_bg;
                ui.style_mut().visuals.widgets.inactive.weak_bg_fill = theme.control_bg;

                ui.horizontal(|ui| {
                    ui.with_layout(egui::Layout::right_to_left(egui::Align::Min), |ui| {
                        if ui.add(
                            egui::Button::new(
                                egui::RichText::new("✕")
                                    .size(11.0)
                                    .color(egui::Color32::from_rgb(255, 130, 130)),
                            )
                            .min_size(egui::vec2(18.0, 16.0))
                            .corner_radius(egui::CornerRadius::same(4)),
                        ).on_hover_text(t!("ui.btn_close")).clicked() {
                            self.show_api_key_modal = false;
                            self.api_key_error = None;
                        }

                        ui.with_layout(egui::Layout::left_to_right(egui::Align::Center), |ui| {
                            ui.horizontal_wrapped(|ui| {
                                ui.spacing_mut().item_spacing = egui::vec2(6.0, 4.0);

                                let is_llm_active = self.settings_tab == SettingsTab::LlmProvider;
                                let tab_llm = ui.selectable_label(
                                    is_llm_active,
                                    egui::RichText::new(t!("ui.settings_tab_llm"))
                                        .size(12.0)
                                        .color(if is_llm_active { theme.accent } else { theme.text_secondary })
                                        .strong(),
                                );
                                let tab_llm_id = ui.make_persistent_id("dwell_tab_llm_provider");
                                let llm_activated = if !is_llm_active {
                                    apply_dwell_to_response(ui, &mut self.dwell_state, tab_llm_id, &tab_llm)
                                } else {
                                    tab_llm.clicked()
                                };
                                if llm_activated {
                                    self.settings_tab = SettingsTab::LlmProvider;
                                }

                                let is_theme_active = self.settings_tab == SettingsTab::Theme;
                                let tab_theme = ui.selectable_label(
                                    is_theme_active,
                                    egui::RichText::new(t!("ui.settings_tab_theme"))
                                        .size(12.0)
                                        .color(if is_theme_active { theme.accent } else { theme.text_secondary })
                                        .strong(),
                                );
                                let tab_theme_id = ui.make_persistent_id("dwell_tab_theme");
                                let theme_activated = if !is_theme_active {
                                    apply_dwell_to_response(ui, &mut self.dwell_state, tab_theme_id, &tab_theme)
                                } else {
                                    tab_theme.clicked()
                                };
                                if theme_activated {
                                    self.settings_tab = SettingsTab::Theme;
                                }

                                let is_lang_active = self.settings_tab == SettingsTab::Language;
                                let tab_lang = ui.selectable_label(
                                    is_lang_active,
                                    egui::RichText::new(t!("ui.settings_tab_language"))
                                        .size(12.0)
                                        .color(if is_lang_active { theme.accent } else { theme.text_secondary })
                                        .strong(),
                                );
                                let tab_lang_id = ui.make_persistent_id("dwell_tab_language");
                                let lang_activated = if !is_lang_active {
                                    apply_dwell_to_response(ui, &mut self.dwell_state, tab_lang_id, &tab_lang)
                                } else {
                                    tab_lang.clicked()
                                };
                                if lang_activated {
                                    self.settings_tab = SettingsTab::Language;
                                }

                                let is_macro_active = self.settings_tab == SettingsTab::Macros;
                                let tab_macro = ui.selectable_label(
                                    is_macro_active,
                                    egui::RichText::new(t!("ui.settings_tab_macros"))
                                        .size(12.0)
                                        .color(if is_macro_active { theme.accent } else { theme.text_secondary })
                                        .strong(),
                                );
                                let tab_macro_id = ui.make_persistent_id("dwell_tab_macros");
                                let macro_activated = if !is_macro_active {
                                    apply_dwell_to_response(ui, &mut self.dwell_state, tab_macro_id, &tab_macro)
                                } else {
                                    tab_macro.clicked()
                                };
                                if macro_activated {
                                    self.settings_tab = SettingsTab::Macros;
                                }

                                let is_devices_active = self.settings_tab == SettingsTab::Devices;
                                let tab_devices = ui.selectable_label(
                                    is_devices_active,
                                    egui::RichText::new(t!("ui.settings_tab_devices"))
                                        .size(12.0)
                                        .color(if is_devices_active { theme.accent } else { theme.text_secondary })
                                        .strong(),
                                );
                                let tab_devices_id = ui.make_persistent_id("dwell_tab_devices");
                                let devices_activated = if !is_devices_active {
                                    apply_dwell_to_response(ui, &mut self.dwell_state, tab_devices_id, &tab_devices)
                                } else {
                                    tab_devices.clicked()
                                };
                                if devices_activated {
                                    self.settings_tab = SettingsTab::Devices;
                                    self.refresh_devices();
                                }
                            });
                        });
                    });
                });
                ui.add_space(4.0);
                ui.separator();
                ui.add_space(4.0);

                let max_scroll_h = (modal_h - 58.0).max(150.0);
                egui::ScrollArea::vertical()
                    .id_salt("settings_modal_scroll")
                    .auto_shrink([false, false])
                    .max_height(max_scroll_h)
                    .show(ui, |ui| {
                        match self.settings_tab {
                            SettingsTab::LlmProvider => {
                                let env_var_target = self.selected_provider_modal.env_key_name();
                                let env_model_target = self.selected_provider_modal.env_model_name();
                                let env_endpoint_target = self.selected_provider_modal.env_endpoint_name();
                                let default_model_name = self.selected_provider_modal.default_model();
                                let default_endpoint_url = self.selected_provider_modal.default_endpoint();

                                ui.horizontal_wrapped(|ui| {
                                    ui.spacing_mut().item_spacing = egui::vec2(4.0, 3.0);
                                    for &provider in LlmProvider::ALL {
                                        let is_current = self.llm_config.provider == provider;
                                        let is_enabled = provider.is_enabled();
                                        let is_selected_in_modal = self.selected_provider_modal == provider;

                                        let badge = if is_current { "★ " } else if is_enabled { "● " } else { "○ " };
                                        let label = format!("{}{}", badge, provider.display_name());
                                        let label_rich = if is_current {
                                            egui::RichText::new(label).strong().color(theme.accent)
                                        } else if !is_enabled {
                                            egui::RichText::new(label).color(egui::Color32::from_gray(140))
                                        } else {
                                            egui::RichText::new(label)
                                        };

                                        if ui.selectable_label(is_selected_in_modal, label_rich).clicked() {
                                            self.sync_modal_inputs_for_provider(provider);
                                        }
                                    }
                                });

                                ui.add_space(4.0);

                                ui.horizontal(|ui| {
                                    let mut is_enabled = self.selected_provider_modal.is_enabled();
                                    if ui.checkbox(&mut is_enabled, t!("ui.provider_enabled_checkbox")).changed() {
                                        let _ = self.selected_provider_modal.set_enabled(is_enabled);
                                        self.llm_config = resolve_llm_config();
                                    }

                                    if self.selected_provider_modal != self.llm_config.provider {
                                        if ui.button(t!("ui.btn_set_primary_provider")).clicked() {
                                            let _ = self.selected_provider_modal.set_enabled(true);
                                            let _ = write_env_var_to_file(".env", "LLM_PROVIDER", self.selected_provider_modal.display_name());
                                            std::env::set_var("LLM_PROVIDER", self.selected_provider_modal.display_name());
                                            self.llm_config = resolve_llm_config();

                                            if self.selected_provider_modal == LlmProvider::Local {
                                                std::thread::spawn(|| {
                                                    local_llm::init_local_llm();
                                                });
                                            }
                                            play_earcon(Earcon::Success);
                                        }
                                    } else {
                                        ui.label(egui::RichText::new(t!("ui.primary_provider_badge")).size(11.0).color(theme.accent));
                                    }
                                });
                                ui.add_space(4.0);

                                if self.selected_provider_modal == LlmProvider::Local {
                                    let active_preset = local_llm::get_active_preset();
                                    egui::Frame::new()
                                        .fill(theme.control_bg)
                                        .stroke(egui::Stroke::new(1.0, theme.border_subtle))
                                        .corner_radius(egui::CornerRadius::same(8))
                                        .inner_margin(egui::Margin::symmetric(10, 8))
                                        .show(ui, |ui| {
                                            ui.horizontal(|ui| {
                                                ui.label(egui::RichText::new(t!("ui.local_models_header")).size(11.0).color(theme.accent).strong());
                                            });
                                            ui.add_space(4.0);

                                            let mut model_to_delete = None;
                                            for preset in local_llm::MODEL_PRESETS {
                                                let is_active = active_preset.id == preset.id;
                                                let is_installed = local_llm::is_preset_downloaded(preset);

                                                ui.horizontal(|ui| {
                                                    let radio_label = format!("{} ({})", preset.name, preset.size_desc);
                                                    let radio_resp = ui.radio(is_active, radio_label);
                                                    if radio_resp.clicked() && !is_active {
                                                        self.llm_config.model = preset.name.to_string();
                                                        self.model_input = preset.name.to_string();
                                                        play_earcon(Earcon::Success);

                                                        let preset_id = preset.id;
                                                        let filename = preset.filename;
                                                        std::thread::spawn(move || {
                                                            local_llm::set_active_preset(preset_id);
                                                            let _ = write_env_var_to_file(".env", "LLM_MODEL", filename);
                                                            std::env::set_var("LLM_MODEL", filename);
                                                            let _ = write_env_var_to_file(".env", "LOCAL_LLAMA_MODEL", filename);
                                                            std::env::set_var("LOCAL_LLAMA_MODEL", filename);
                                                            local_llm::init_local_llm();
                                                        });
                                                    }

                                                    let family_badge = match preset.family {
                                                        local_llm::ModelFamily::ChatML => "ChatML",
                                                        local_llm::ModelFamily::Llama3 => "Llama 3",
                                                    };
                                                    ui.label(egui::RichText::new(family_badge).size(9.5).color(theme.text_secondary));

                                                    if is_installed {
                                                        ui.label(egui::RichText::new(t!("ui.local_model_ready")).size(10.5).color(egui::Color32::from_rgb(100, 225, 130)).strong());
                                                        if ui.small_button("🗑").on_hover_text(t!("ui.local_model_delete_tooltip")).clicked() {
                                                            model_to_delete = Some(preset);
                                                        }
                                                    } else {
                                                        ui.label(egui::RichText::new(t!("ui.local_model_not_downloaded")).size(10.5).color(egui::Color32::from_rgb(255, 180, 80)));
                                                    }
                                                });
                                            }

                                            if let Some(p) = model_to_delete {
                                                let event_tx = self.event_sender.clone();
                                                self.api_key_error = None;
                                                std::thread::spawn(move || {
                                                    match local_llm::delete_preset_model(p) {
                                                        Ok(()) => {
                                                            play_earcon(Earcon::Success);
                                                            let _ = event_tx.send(AgentEvent::SilentNarration(t!("ui.local_model_delete_success", name = p.name).to_string()));
                                                        }
                                                        Err(e) => {
                                                            play_earcon(Earcon::Error);
                                                            let _ = event_tx.send(AgentEvent::SilentNarration(t!("ui.local_model_delete_error", name = p.name, err = e).to_string()));
                                                        }
                                                    }
                                                });
                                            }

                                            ui.add_space(6.0);
                                            let active_installed = local_llm::is_preset_downloaded(active_preset);
                                            if let Some((percent, downloaded_mb, total_mb, speed_mb_s)) = self.local_download_progress {
                                                ui.add(egui::ProgressBar::new(percent / 100.0).text(format!("{percent:.1} % ({downloaded_mb:.1} / {total_mb:.1} Mo - {speed_mb_s:.1} Mo/s)")).animate(true));
                                            } else if !active_installed {
                                                let event_tx_clone = self.event_sender.clone();
                                                let p = *active_preset;
                                                let btn_text = t!("ui.local_model_download_btn", name = p.name, size = p.size_desc).to_string();
                                                if ui.button(egui::RichText::new(btn_text).strong().color(theme.accent)).clicked() {
                                                    std::thread::spawn(move || {
                                                        if local_llm::ensure_llama_model_downloaded_for_preset(&p, &event_tx_clone).is_ok() {
                                                            local_llm::init_local_llm();
                                                        }
                                                    });
                                                }
                                            } else {
                                                ui.label(egui::RichText::new(t!("ui.local_model_footer_info", name = active_preset.name).to_string()).size(10.5).color(theme.text_secondary));
                                            }
                                        });
                                } else {
                                    ui.add_space(4.0);
                                    ui.label(egui::RichText::new(t!("ui.api_key_label", env = env_var_target)).size(11.0).color(egui::Color32::WHITE).strong());
                                    ui.add_space(2.0);

                                    ui.horizontal(|ui| {
                                        let key_input_id = egui::Id::NULL.with("api_key_input_field");
                                        let is_key_focused = ctx.memory(|m| m.has_focus(key_input_id));
                                        let paste_key_hit = ctx.input(|i| {
                                            i.events.iter().any(|e| matches!(e, egui::Event::Paste(_)))
                                                || ((i.modifiers.ctrl || i.modifiers.command) && i.key_pressed(egui::Key::V))
                                                || (i.modifiers.shift && i.key_pressed(egui::Key::Insert))
                                        });
                                        if is_key_focused && paste_key_hit {
                                            if let Some(clip) = get_clipboard_text().or_else(get_plain_clipboard_text) {
                                                let trimmed = clip.trim().to_string();
                                                if !trimmed.is_empty() {
                                                    self.api_key_input = trimmed;
                                                    play_earcon(Earcon::Success);
                                                }
                                            }
                                            ctx.input_mut(|i| i.consume_key(egui::Modifiers::COMMAND, egui::Key::V));
                                            ctx.input_mut(|i| i.consume_key(egui::Modifiers::CTRL, egui::Key::V));
                                        }
                                        let edit = egui::TextEdit::singleline(&mut self.api_key_input)
                                            .id(key_input_id)
                                            .password(!self.show_api_key_plain)
                                            .hint_text(t!("ui.api_key_hint"))
                                            .desired_width(230.0);
                                        let edit_resp = ui.add(edit);

                                        let eye_icon = if self.show_api_key_plain { "🔒" } else { "👁" };
                                        if ui.button(eye_icon).on_hover_text(t!("ui.toggle_api_key_visibility")).clicked() {
                                            self.show_api_key_plain = !self.show_api_key_plain;
                                        }

                                        if ui.button("📋").on_hover_text(t!("ui.paste_from_clipboard")).clicked() {
                                            if let Some(clip) = get_clipboard_text() {
                                                self.api_key_input = clip.trim().to_string();
                                            }
                                        }
                                    });

                                    ui.add_space(5.0);
                                    ui.horizontal(|ui| {
                                        ui.label(egui::RichText::new(t!("ui.model_label", env = env_model_target)).size(11.0).color(egui::Color32::WHITE).strong());
                                        ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                                            if ui.small_button(t!("ui.btn_model_recommended")).on_hover_text(t!("ui.restore_default_model_tooltip", model = default_model_name)).clicked() {
                                                self.model_input = default_model_name.to_string();
                                            }
                                        });
                                    });
                                    ui.add_space(2.0);
                                    ui.horizontal(|ui| {
                                        let model_input_id = egui::Id::NULL.with("model_input_field");
                                        let model_edit = egui::TextEdit::singleline(&mut self.model_input).id(model_input_id).hint_text(default_model_name).desired_width(240.0);
                                        ui.add(model_edit);
                                    });

                                    ui.add_space(5.0);
                                    ui.horizontal(|ui| {
                                        ui.label(egui::RichText::new(t!("ui.endpoint_label")).size(11.0).color(egui::Color32::WHITE).strong());
                                        ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                                            if ui.small_button(t!("ui.btn_endpoint_default")).on_hover_text(t!("ui.btn_endpoint_default_tooltip", url = default_endpoint_url).to_string()).clicked() {
                                                self.endpoint_input = default_endpoint_url.to_string();
                                            }
                                        });
                                    });
                                    ui.add_space(2.0);
                                    ui.horizontal(|ui| {
                                        let endpoint_edit = egui::TextEdit::singleline(&mut self.endpoint_input).hint_text(default_endpoint_url).desired_width(320.0);
                                        ui.add(endpoint_edit).on_hover_text(t!("ui.endpoint_hint_tooltip"));
                                    });
                                }

                                if let Some(ref err) = self.api_key_error {
                                    ui.add_space(2.0);
                                    ui.label(egui::RichText::new(err).color(egui::Color32::from_rgb(255, 110, 110)).size(11.0));
                                }

                                if self.selected_provider_modal == LlmProvider::Local {
                                    ui.add_space(4.0);
                                    ui.label(egui::RichText::new(t!("ui.local_engine_no_key_needed")).size(10.5).color(egui::Color32::from_rgb(140, 200, 255)));
                                    ui.add_space(2.0);
                                }

                                ui.add_space(5.0);
                                ui.horizontal(|ui| {
                                    if ui.button(t!("ui.btn_save_apply")).clicked() {
                                        let key = self.api_key_input.trim();
                                        let model = if self.model_input.trim().is_empty() { default_model_name } else { &self.model_input };
                                        let endpoint = if self.endpoint_input.trim().is_empty() { default_endpoint_url } else { &self.endpoint_input };

                                        if self.selected_provider_modal == LlmProvider::Local {
                                            let active_p = local_llm::get_active_preset();
                                            let active_id = active_p.id;
                                            let active_filename = active_p.filename;
                                            let provider_name = self.selected_provider_modal.display_name();

                                            self.llm_config = resolve_llm_config();
                                            self.llm_config.model = active_p.name.to_string();
                                            self.show_api_key_modal = false;
                                            self.api_key_error = None;

                                            std::thread::spawn(move || {
                                                let _ = write_env_var_to_file(".env", "LLM_PROVIDER", provider_name);
                                                std::env::set_var("LLM_PROVIDER", provider_name);
                                                local_llm::set_active_preset(active_id);
                                                let _ = write_env_var_to_file(".env", "LLM_MODEL", active_filename);
                                                std::env::set_var("LLM_MODEL", active_filename);
                                                let _ = write_env_var_to_file(".env", "LOCAL_LLAMA_MODEL", active_filename);
                                                std::env::set_var("LOCAL_LLAMA_MODEL", active_filename);
                                                local_llm::init_local_llm();
                                            });
                                        } else if let Err(e) = write_env_var_to_file(".env", env_var_target, key) {
                                            self.api_key_error = Some(t!("ui.api_key_write_err", err = e).to_string());
                                        } else if let Err(e) = write_env_var_to_file(".env", env_model_target, model) {
                                            self.api_key_error = Some(t!("ui.model_write_err", err = e).to_string());
                                        } else if let Err(e) = write_env_var_to_file(".env", env_endpoint_target, endpoint) {
                                            self.api_key_error = Some(t!("ui.endpoint_write_err", err = e).to_string());
                                        } else {
                                            let _ = write_env_var_to_file(".env", "LLM_PROVIDER", self.selected_provider_modal.display_name());
                                            std::env::set_var("LLM_PROVIDER", self.selected_provider_modal.display_name());
                                            std::env::set_var(env_var_target, key);
                                            std::env::set_var(env_model_target, model);
                                            std::env::set_var(env_endpoint_target, endpoint);
                                            self.llm_config = resolve_llm_config();
                                            self.show_api_key_modal = false;
                                        }
                                    }
                                    if ui.button(t!("ui.btn_cancel")).clicked() {
                                        self.show_api_key_modal = false;
                                        self.api_key_error = None;
                                    }
                                });
                            }
                            SettingsTab::Theme => {
                                ui.label(egui::RichText::new(t!("ui.theme_profiles_label")).size(11.5).color(theme.text_secondary));
                                ui.add_space(6.0);

                                let mut current_theme = self.theme;
                                ui.horizontal(|ui| {
                                    ui.label(egui::RichText::new(t!("ui.theme_label")).size(12.0).strong().color(egui::Color32::WHITE));
                                    egui::ComboBox::from_id_salt("settings_theme_combobox")
                                        .selected_text(localized_theme_name(current_theme))
                                        .width(260.0)
                                        .show_ui(ui, |ui| {
                                            for &t in AppTheme::ALL {
                                                let t_colors = t.colors();
                                                ui.horizontal(|ui| {
                                                    if ui.selectable_value(&mut current_theme, t, localized_theme_name(t)).clicked() {
                                                        self.set_theme(t);
                                                    }
                                                    let (rect, _) = ui.allocate_exact_size(egui::vec2(44.0, 12.0), egui::Sense::hover());
                                                    let p = ui.painter();
                                                    p.rect_filled(egui::Rect::from_min_size(rect.min, egui::vec2(10.0, 12.0)), egui::CornerRadius::same(2), t_colors.control_bg);
                                                    p.rect_filled(egui::Rect::from_min_size(egui::pos2(rect.min.x + 11.0, rect.min.y), egui::vec2(10.0, 12.0)), egui::CornerRadius::same(2), t_colors.border);
                                                    p.rect_filled(egui::Rect::from_min_size(egui::pos2(rect.min.x + 22.0, rect.min.y), egui::vec2(10.0, 12.0)), egui::CornerRadius::same(2), t_colors.accent);
                                                    p.rect_filled(egui::Rect::from_min_size(egui::pos2(rect.min.x + 33.0, rect.min.y), egui::vec2(10.0, 12.0)), egui::CornerRadius::same(2), t_colors.text_user);
                                                });
                                            }
                                        });
                                });

                                if current_theme != self.theme {
                                    self.set_theme(current_theme);
                                }

                                ui.add_space(8.0);
                                let cur_colors = self.theme.colors();
                                egui::Frame::new()
                                    .fill(cur_colors.control_bg)
                                    .stroke(egui::Stroke::new(1.0, cur_colors.border))
                                    .corner_radius(egui::CornerRadius::same(8))
                                    .inner_margin(egui::Margin::symmetric(10, 8))
                                    .show(ui, |ui| {
                                        ui.horizontal(|ui| {
                                            ui.label(egui::RichText::new(localized_theme_name(self.theme)).strong().size(12.0).color(cur_colors.accent));
                                        });
                                        ui.add_space(3.0);
                                        ui.label(egui::RichText::new(localized_theme_desc(self.theme)).size(10.5).color(cur_colors.text_secondary).italics());
                                    });

                                ui.add_space(8.0);
                                ui.separator();
                                ui.add_space(4.0);
                                ui.label(egui::RichText::new(t!("ui.font_size_label")).size(11.5).color(theme.text_secondary));
                                ui.add_space(6.0);
                                let mut current_zoom = self.zoom_factor;
                                let prev_zoom = current_zoom;
                                ui.horizontal(|ui| {
                                    ui.spacing_mut().item_spacing.x = 8.0;
                                    ui.label(egui::RichText::new(t!("ui.font_label")).size(12.0).strong().color(egui::Color32::WHITE));

                                    let btn_minus = egui::Button::new(egui::RichText::new("A −").size(13.0).strong().color(egui::Color32::WHITE)).min_size(egui::vec2(52.0, 30.0)).corner_radius(egui::CornerRadius::same(6));
                                    let resp_minus = ui.add(btn_minus).on_hover_text(t!("ui.font_minus_tooltip"));
                                    let id_minus = ui.make_persistent_id("dwell_zoom_minus");
                                    if apply_dwell_to_response(ui, &mut self.dwell_state, id_minus, &resp_minus) {
                                        current_zoom = ((current_zoom - 0.10).max(0.70) * 100.0).round() / 100.0;
                                    }

                                    ui.add(egui::Label::new(egui::RichText::new(format!("{:.0} %", current_zoom * 100.0)).size(13.0).strong().color(theme.accent)));

                                    let btn_plus = egui::Button::new(egui::RichText::new("A +").size(13.0).strong().color(egui::Color32::WHITE)).min_size(egui::vec2(52.0, 30.0)).corner_radius(egui::CornerRadius::same(6));
                                    let resp_plus = ui.add(btn_plus).on_hover_text(t!("ui.font_plus_tooltip"));
                                    let id_plus = ui.make_persistent_id("dwell_zoom_plus");
                                    if apply_dwell_to_response(ui, &mut self.dwell_state, id_plus, &resp_plus) {
                                        current_zoom = ((current_zoom + 0.10).min(2.50) * 100.0).round() / 100.0;
                                    }

                                    let btn_reset = egui::Button::new(egui::RichText::new("100 %").size(12.0).color(theme.text_secondary)).min_size(egui::vec2(52.0, 30.0)).corner_radius(egui::CornerRadius::same(6));
                                    let resp_reset = ui.add(btn_reset).on_hover_text(t!("ui.font_reset_tooltip"));
                                    let id_reset = ui.make_persistent_id("dwell_zoom_reset");
                                    if apply_dwell_to_response(ui, &mut self.dwell_state, id_reset, &resp_reset) {
                                        current_zoom = 1.0;
                                    }
                                });
                                if (current_zoom - prev_zoom).abs() > 0.001 {
                                    self.set_zoom_factor(current_zoom);
                                    ctx.set_zoom_factor(self.zoom_factor);
                                }

                                ui.add_space(8.0);
                                ui.separator();
                                ui.add_space(4.0);
                                ui.label(egui::RichText::new(t!("ui.dwell_timing_label")).size(11.5).color(theme.text_secondary));
                                ui.add_space(3.0);
                                let mut current_dwell_ms = self.dwell_state.dwell_duration.as_millis() as u64;
                                let prev_dwell_ms = current_dwell_ms;
                                ui.horizontal(|ui| {
                                    ui.label(egui::RichText::new(t!("ui.dwell_delay_label")).size(12.0).strong().color(egui::Color32::WHITE));
                                    let slider = egui::Slider::new(&mut current_dwell_ms, 0..=2500).step_by(50.0).custom_formatter(|val, _| {
                                        if val <= 0.0 { t!("ui.dwell_disabled").to_string() } else { format!("{:.0} ms", val) }
                                    });
                                    ui.add(slider).on_hover_text(t!("ui.dwell_tooltip"));
                                });
                                if current_dwell_ms != prev_dwell_ms {
                                    self.dwell_state.dwell_duration = Duration::from_millis(current_dwell_ms);
                                    let _ = write_env_var_to_file(".env", "DWELL_TIME_MS", &current_dwell_ms.to_string());
                                    std::env::set_var("DWELL_TIME_MS", current_dwell_ms.to_string());
                                }

                                if current_dwell_ms > 0 {
                                    ui.add_space(3.0);
                                    let mut current_act_delay_ms = self.dwell_state.activation_delay.as_millis() as u64;
                                    let prev_act_delay_ms = current_act_delay_ms;
                                    ui.horizontal(|ui| {
                                        ui.label(egui::RichText::new(t!("ui.dwell_gauge_delay_label")).size(12.0).strong().color(egui::Color32::WHITE));
                                        let act_slider = egui::Slider::new(&mut current_act_delay_ms, 0..=600).step_by(25.0).custom_formatter(|val, _| {
                                            if val <= 0.0 { t!("ui.dwell_gauge_delay_immediate").to_string() } else { format!("{:.0} ms", val) }
                                        });
                                        ui.add(act_slider).on_hover_text(t!("ui.dwell_gauge_delay_tooltip"));
                                    });
                                    if current_act_delay_ms != prev_act_delay_ms {
                                        self.dwell_state.activation_delay = Duration::from_millis(current_act_delay_ms);
                                        let _ = write_env_var_to_file(".env", "DWELL_ACTIVATION_DELAY_MS", &current_act_delay_ms.to_string());
                                        std::env::set_var("DWELL_ACTIVATION_DELAY_MS", current_act_delay_ms.to_string());
                                    }
                                }

                                ui.add_space(8.0);
                                ui.separator();
                                ui.add_space(4.0);
                                ui.label(egui::RichText::new(t!("ui.switch_settings_label")).size(11.5).color(theme.text_secondary));
                                ui.add_space(3.0);

                                let mut cur_long_press = SWITCH_LONG_PRESS_MS.load(Ordering::Relaxed);
                                let prev_long_press = cur_long_press;
                                ui.horizontal(|ui| {
                                    ui.label(egui::RichText::new(t!("ui.switch_long_press_label")).size(12.0).strong().color(egui::Color32::WHITE));
                                    let slider = egui::Slider::new(&mut cur_long_press, 400..=2500).step_by(50.0).suffix(" ms");
                                    ui.add(slider).on_hover_text(t!("ui.switch_long_press_tooltip"));
                                });
                                if cur_long_press != prev_long_press {
                                    SWITCH_LONG_PRESS_MS.store(cur_long_press, Ordering::Relaxed);
                                    let _ = write_env_var_to_file(".env", "SWITCH_LONG_PRESS_MS", &cur_long_press.to_string());
                                    std::env::set_var("SWITCH_LONG_PRESS_MS", cur_long_press.to_string());
                                }

                                let mut cur_debounce = SWITCH_DEBOUNCE_MS.load(Ordering::Relaxed);
                                let prev_debounce = cur_debounce;
                                ui.horizontal(|ui| {
                                    ui.label(egui::RichText::new(t!("ui.switch_debounce_label")).size(12.0).strong().color(egui::Color32::WHITE));
                                    let slider = egui::Slider::new(&mut cur_debounce, 50..=800).step_by(25.0).suffix(" ms");
                                    ui.add(slider).on_hover_text(t!("ui.switch_debounce_tooltip"));
                                });
                                if cur_debounce != prev_debounce {
                                    SWITCH_DEBOUNCE_MS.store(cur_debounce, Ordering::Relaxed);
                                    let _ = write_env_var_to_file(".env", "SWITCH_DEBOUNCE_MS", &cur_debounce.to_string());
                                    std::env::set_var("SWITCH_DEBOUNCE_MS", cur_debounce.to_string());
                                }

                                ui.add_space(6.0);
                                ui.horizontal(|ui| {
                                    if ui.button(t!("ui.btn_close")).clicked() {
                                        self.show_api_key_modal = false;
                                    }
                                });
                            }
                            SettingsTab::Language => {
                                ui.label(egui::RichText::new(t!("ui.language_heading")).size(11.5).color(theme.text_secondary));
                                ui.add_space(6.0);

                                let mut selected_lang = self.app_language.clone();
                                ui.horizontal_wrapped(|ui| {
                                    ui.spacing_mut().item_spacing = egui::vec2(6.0, 6.0);
                                    let options = [
                                        ("auto", "🌐 Système (Auto)"),
                                        ("fr", "🇫🇷 Français"),
                                        ("en", "🇬🇧 English"),
                                        ("de", "🇩🇪 Deutsch"),
                                        ("es", "🇪🇸 Español"),
                                        ("it", "🇮🇹 Italiano"),
                                        ("pt", "🇵🇹 Português"),
                                        ("ru", "🇷🇺 Русский"),
                                        ("ja", "🇯🇵 日本語"),
                                        ("zh", "🇨🇳 中文"),
                                    ];

                                    for (code, label) in options {
                                        let is_selected = selected_lang == code;
                                        let btn = ui.selectable_label(is_selected, label);
                                        let btn_id = ui.make_persistent_id(format!("dwell_lang_{code}"));
                                        if apply_dwell_to_response(ui, &mut self.dwell_state, btn_id, &btn) {
                                            selected_lang = code.to_string();
                                        }
                                    }
                                });
                                if selected_lang != self.app_language {
                                    self.set_app_language(&selected_lang);
                                    ctx.request_repaint();
                                }

                                ui.add_space(8.0);
                                ui.separator();
                                ui.add_space(6.0);
                                ui.horizontal(|ui| {
                                    if ui.button(t!("ui.btn_close")).clicked() {
                                        self.show_api_key_modal = false;
                                    }
                                });
                            }
                            SettingsTab::Macros => {
                                ui.label(egui::RichText::new(t!("ui.macros_description")).size(11.5).color(theme.text_secondary));
                                ui.add_space(6.0);

                                egui::Frame::new()
                                    .fill(theme.control_bg)
                                    .stroke(egui::Stroke::new(1.0, theme.border_subtle))
                                    .corner_radius(egui::CornerRadius::same(8))
                                    .inner_margin(egui::Margin::symmetric(10, 8))
                                    .show(ui, |ui| {
                                        ui.label(egui::RichText::new(t!("ui.macros_new_header")).size(11.0).color(theme.accent).strong());
                                        ui.add_space(3.0);

                                        ui.horizontal(|ui| {
                                            ui.label(egui::RichText::new(t!("ui.macros_shortcut_label")).size(11.0).color(egui::Color32::WHITE));
                                            ui.add(egui::TextEdit::singleline(&mut self.macro_shortcut_input).id(egui::Id::NULL.with("macro_shortcut_input")).hint_text(t!("ui.macros_shortcut_hint")).desired_width(120.0));
                                        });
                                        ui.add_space(3.0);

                                        ui.horizontal(|ui| {
                                            ui.label(egui::RichText::new(t!("ui.macros_text_label")).size(11.0).color(egui::Color32::WHITE));
                                            ui.add(egui::TextEdit::singleline(&mut self.macro_text_input).id(egui::Id::NULL.with("macro_text_input")).hint_text(t!("ui.macros_text_hint")).desired_width(260.0));
                                        });
                                        ui.add_space(4.0);

                                        if let Some(ref err) = self.macro_error {
                                            ui.label(egui::RichText::new(err).color(egui::Color32::from_rgb(255, 110, 110)).size(10.5));
                                            ui.add_space(2.0);
                                        }

                                        ui.horizontal(|ui| {
                                            if ui.button(egui::RichText::new(t!("ui.macros_add_btn")).strong().color(theme.accent)).clicked() {
                                                let sc = self.macro_shortcut_input.trim().to_string();
                                                let tx = self.macro_text_input.trim().to_string();
                                                if sc.is_empty() {
                                                    self.macro_error = Some(t!("ui.macros_error_empty_shortcut").to_string());
                                                } else if tx.is_empty() {
                                                    self.macro_error = Some(t!("ui.macros_error_empty_text").to_string());
                                                } else {
                                                    self.macros.retain(|(k, _)| !k.eq_ignore_ascii_case(&sc));
                                                    self.macros.push((sc, tx));
                                                    let _ = save_macros(&self.macros);
                                                    self.macro_shortcut_input.clear();
                                                    self.macro_text_input.clear();
                                                    self.macro_error = None;
                                                    play_earcon(Earcon::Success);
                                                }
                                            }
                                        });
                                    });

                                ui.add_space(8.0);
                                ui.separator();
                                ui.add_space(6.0);

                                ui.label(egui::RichText::new(t!("ui.macros_active_title", count = self.macros.len()).to_string()).size(11.5).strong().color(egui::Color32::WHITE));
                                ui.add_space(4.0);

                                let mut to_remove = None;
                                egui::ScrollArea::vertical().max_height(140.0).auto_shrink([false, false]).show(ui, |ui| {
                                    if self.macros.is_empty() {
                                        ui.label(egui::RichText::new(t!("ui.macros_none")).size(11.0).italics().color(theme.text_secondary));
                                    } else {
                                        for (idx, (sc, tx)) in self.macros.iter().enumerate() {
                                            ui.horizontal(|ui| {
                                                ui.label(egui::RichText::new(format!("⚡ {}", sc)).size(11.0).color(theme.accent).strong());
                                                ui.label(egui::RichText::new("➔").size(10.0).color(theme.text_secondary));
                                                let preview = if tx.len() > 38 { format!("{}...", &tx[..35]) } else { tx.clone() };
                                                ui.label(egui::RichText::new(preview).size(11.0).color(egui::Color32::WHITE)).on_hover_text(tx);
                                                ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                                                    if ui.small_button("🗑").on_hover_text(t!("ui.macros_delete_tooltip")).clicked() {
                                                        to_remove = Some(idx);
                                                    }
                                                });
                                            });
                                            ui.add_space(2.0);
                                        }
                                    }
                                });

                                if let Some(idx) = to_remove {
                                    self.macros.remove(idx);
                                    let _ = save_macros(&self.macros);
                                    play_earcon(Earcon::Success);
                                }
                            }
                            SettingsTab::Devices => {
                                ui.horizontal(|ui| {
                                    ui.label(egui::RichText::new(t!("ui.devices_tab_desc")).size(11.5).color(theme.text_secondary));
                                    ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                                        if ui.button(t!("ui.devices_refresh_btn")).on_hover_text(t!("ui.devices_refresh_btn")).clicked() {
                                            self.refresh_devices();
                                            play_earcon(Earcon::Success);
                                        }
                                    });
                                });
                                ui.add_space(6.0);
                                ui.label(egui::RichText::new(t!("ui.devices_mic_header")).size(12.0).color(egui::Color32::WHITE).strong());
                                ui.add_space(2.0);

                                egui::Frame::new()
                                    .fill(theme.control_bg)
                                    .stroke(egui::Stroke::new(1.0, theme.border_subtle))
                                    .corner_radius(egui::CornerRadius::same(8))
                                    .inner_margin(egui::Margin::symmetric(10, 8))
                                    .show(ui, |ui| {
                                        let default_mic_name = crate::devices::get_default_microphone().unwrap_or_else(|| t!("ui.devices_default_mic").to_string());
                                        let is_auto = self.selected_microphone == "auto" || self.selected_microphone.is_empty();
                                        let auto_label = t!("ui.devices_mic_auto_prefix", name = default_mic_name).to_string();
                                        let mut current_selection = if is_auto { "auto".to_string() } else { self.selected_microphone.clone() };
                                        let mut changed = false;

                                        egui::ComboBox::from_id_salt("mic_select_combobox")
                                            .selected_text(if is_auto { &auto_label } else { &self.selected_microphone })
                                            .width(ui.available_width().max(240.0))
                                            .show_ui(ui, |ui| {
                                                if ui.selectable_value(&mut current_selection, "auto".to_string(), &auto_label).clicked() {
                                                    changed = true;
                                                }
                                                ui.separator();
                                                for mic in &self.detected_microphones {
                                                    let is_system_default = Some(mic) == crate::devices::get_default_microphone().as_ref();
                                                    let display_text = if is_system_default { format!("🎙️ {mic} ★") } else { format!("🎙️ {mic}") };
                                                    if ui.selectable_value(&mut current_selection, mic.clone(), display_text).clicked() {
                                                        changed = true;
                                                    }
                                                }
                                            });

                                        if changed {
                                            self.selected_microphone = current_selection.clone();
                                            let _ = crate::devices::set_active_microphone(&current_selection);
                                            play_earcon(Earcon::Success);
                                        }
                                    });

                                ui.add_space(10.0);
                                ui.label(egui::RichText::new(t!("ui.devices_pointing_header")).size(12.0).color(egui::Color32::WHITE).strong());
                                ui.add_space(2.0);

                                egui::Frame::new()
                                    .fill(theme.control_bg)
                                    .stroke(egui::Stroke::new(1.0, theme.border_subtle))
                                    .corner_radius(egui::CornerRadius::same(8))
                                    .inner_margin(egui::Margin::symmetric(10, 8))
                                    .show(ui, |ui| {
                                        if self.detected_pointing_devices.is_empty() {
                                            ui.label(egui::RichText::new(t!("ui.devices_pointing_none")).size(11.0).color(theme.text_secondary).italics());
                                        } else {
                                            for (idx, dev) in self.detected_pointing_devices.iter().enumerate() {
                                                let icon = if dev.kind.contains("Trackball") { "🔴" } else if dev.kind.contains("Tactile") || dev.kind.contains("Touchpad") { "👆" } else if dev.kind.contains("Stylet") || dev.kind.contains("Tablette") { "🖊️" } else { "🖱️" };
                                                ui.horizontal(|ui| {
                                                    ui.label(egui::RichText::new(icon).size(13.0));
                                                    ui.vertical(|ui| {
                                                        ui.label(egui::RichText::new(&dev.name).size(11.5).color(theme.accent).strong());
                                                        ui.label(egui::RichText::new(format!("{} — {}", dev.kind, dev.details)).size(10.0).color(theme.text_secondary));
                                                    });
                                                });
                                                if idx + 1 < self.detected_pointing_devices.len() {
                                                    ui.add_space(2.0);
                                                    ui.separator();
                                                    ui.add_space(2.0);
                                                }
                                            }
                                        }
                                    });

                                ui.add_space(8.0);
                                ui.horizontal(|ui| {
                                    if ui.button(t!("ui.btn_close")).clicked() {
                                        self.show_api_key_modal = false;
                                    }
                                });
                            }
                        }
                    });
            });
    }
}