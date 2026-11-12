use std::sync::atomic::Ordering;
use std::time::{Duration, Instant};
use eframe::egui;

use crate::automation::{
    get_clipboard_text, get_plain_clipboard_text, SWITCH_DEBOUNCE_MS,
    SWITCH_LONG_PRESS_MS,
};
use crate::fs_util::{read_env_var_from_file, save_macros, write_env_var_to_file};
use crate::local_llm;
use crate::locale::get_system_locale_language;
use crate::overlay::{
    apply_dwell_to_response, make_slider_accessible, make_toggle_accessible,
    make_widget_accessible_and_activated, play_earcon, render_accessible_input, resolve_llm_config, Earcon,
};
use crate::theme::{localized_theme_desc, localized_theme_name, AppTheme, SettingsTab, ThemeColors};
use crate::types::{AgentEvent, DisabilityPreset, LlmProvider, TtsCommand, VoiceProfile};
use crate::OverlayApp;

impl OverlayApp {
    pub fn set_voice_clone_gender(&mut self, gender: &str) {
        let clean = if gender.trim().eq_ignore_ascii_case("male") || gender.trim().eq_ignore_ascii_case("homme") {
            "male"
        } else {
            "female"
        };
        self.voice_clone_gender = clean.to_string();
        std::env::set_var("VOICE_CLONE_GENDER", clean);
        let _ = write_env_var_to_file(".env", "VOICE_CLONE_GENDER", clean);

        let gender_label = if clean == "male" { "homme" } else { "femme" };
        self.voice_status_feedback = Some(format!("Genre configuré : {gender_label}. Chargement du modèle de voix..."));
        play_earcon(Earcon::Navigation);

        let tts_tx = self.tts_sender.clone();
        let event_tx = self.event_sender.clone();
        let is_clone_active = self.voice_gender == VoiceProfile::Clone;
        let clean_string = clean.to_string();

        std::thread::spawn(move || {
            let model_dir = std::path::PathBuf::from("models").join("voice-clone").join(&clean_string);
            match crate::tts::ensure_voice_clone_model_for_gender(&model_dir, &clean_string) {
                Ok(()) => {
                    let _ = tts_tx.send(TtsCommand::SetGender(VoiceProfile::Clone));
                    let _ = event_tx.send(AgentEvent::SilentNarration(format!(
                        "Modèle vocal cloné ({}) chargé avec succès.",
                        if clean_string == "male" { "homme" } else { "femme" }
                    )));
                }
                Err(e) => {
                    eprintln!("[TTS Clone] Erreur chargement modèle {clean_string}: {e}");
                }
            }
        });

        if is_clone_active {
            let _ = self.tts_sender.send(TtsCommand::SetGender(VoiceProfile::Clone));
        }
    }

    pub fn generate_new_voice_guide_phrase(&mut self) {
        if self.voice_guide_generating {
            return;
        }
        self.voice_guide_generating = true;
        let (tx, rx) = std::sync::mpsc::channel();
        self.voice_guide_rx = Some(rx);

        let config = resolve_llm_config();
        let (_, sys_lang) = get_system_locale_language();

        std::thread::spawn(move || {
            const FALLBACKS: &[&str] = &[
                "Portez ce vieux whisky au juge blond qui fume sur son île enchantée.",
                "Voix ambiguë d’un cœur qui, au zéphyr printanier, préfère les jattes de kiwis frais.",
                "Pendant que le vent souffle sur l'océan, je savoure le calme d'un beau matin d'automne.",
                "L'aube dorée illumine doucement l'horizon lointain tandis que la nature s'éveille paisiblement.",
                "Voyagez à travers les paysages sauvages et découvrez mille merveilles au crépuscule.",
                "Chaque parole partagée avec clarté et bienveillance tisse des liens précieux autour de nous.",
                "Les cinq grands cygnes blancs glissent silencieusement sur le miroir tranquille du lac.",
                "Dans la douce lumière du crépuscule, les oiseaux chantent une mélodie réconfortante.",
                "Prenez le temps d'écouter le murmure du ruisseau qui serpente entre les rochers moussus.",
                "Sous la canopée verdoyante, les rayons du soleil dessinent des motifs dorés sur le sol.",
            ];

            let fallback_choice = {
                let nanos = std::time::SystemTime::now()
                    .duration_since(std::time::UNIX_EPOCH)
                    .unwrap_or_default()
                    .as_nanos();
                let idx = ((nanos ^ (nanos >> 16)) as usize) % FALLBACKS.len();
                format!("« {} »", FALLBACKS[idx])
            };

            let prompt = format!(
                "Tu es un linguiste expert en phonétique ({sys_lang}) et en synthèse vocale.\n\
                 Génère une unique phrase en {sys_lang}, naturelle, expressive, riche en sonorités variées (voyelles et consonnes), idéale pour servir d'échantillon d'apprentissage vocal.\n\
                 Longueur : entre 12 et 22 mots.\n\
                 Réponds STRICTEMENT avec un objet JSON au format : {{\"phrase\": \"la phrase ici\"}}"
            );

            let result: Result<String, String> = if config.provider == LlmProvider::Local {
                local_llm::LocalLlamaEngine::generate_local_text(&prompt, 80)
            } else {
                let rt = tokio::runtime::Builder::new_current_thread()
                    .enable_all()
                    .build();
                match rt {
                    Ok(rt) => rt.block_on(async {
                        let client = reqwest::Client::builder()
                            .timeout(Duration::from_secs(8))
                            .build()
                            .map_err(|e| e.to_string())?;
                        let messages = vec![crate::types::ChatMessage {
                            role: "user".to_string(),
                            content: "Génère une nouvelle phrase d'entraînement vocal variée au format JSON.".to_string(),
                        }];

                        let resp = crate::agent_backend::send_llm_chat_completion(
                            &client,
                            &config,
                            &prompt,
                            &messages,
                            0.7,
                            80,
                        ).await.map_err(|e| e.to_string())?;
                        crate::agent_backend::parse_llm_response_text(resp, config.provider).await
                    }),
                    Err(e) => Err(e.to_string()),
                }
            };

            let final_phrase = match result {
                Ok(text) if !text.trim().is_empty() => {
                    let extracted = if let Ok(val) = serde_json::from_str::<serde_json::Value>(&text) {
                        val.get("phrase").and_then(|v| v.as_str()).unwrap_or(&text).to_string()
                    } else if let (Some(s), Some(e)) = (text.find('{'), text.rfind('}')) {
                        if let Ok(val) = serde_json::from_str::<serde_json::Value>(&text[s..=e]) {
                            val.get("phrase").and_then(|v| v.as_str()).unwrap_or(&text).to_string()
                        } else {
                            text
                        }
                    } else {
                        text
                    };
                    let cleaned = extracted.trim().trim_matches(['"', '«', '»', '`', '\'', '*', ' ', '{', '}']).trim();
                    if !cleaned.is_empty() {
                        format!("« {cleaned} »")
                    } else {
                        fallback_choice
                    }
                }
                _ => fallback_choice,
            };
            let _ = tx.send(final_phrase);
        });
    }

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

    pub fn reset_disability_parameters_to_default(&mut self, ctx: &egui::Context) {
        self.set_theme(AppTheme::CyberCyan);
        self.set_zoom_factor(1.0);
        ctx.set_zoom_factor(1.0);
        self.auto_read_agent_messages = true;
        self.auto_read_suggestions = true;
        self.earcons_enabled = true;
        crate::overlay::set_earcons_enabled(true);
        self.dwell_state.dwell_duration = Duration::ZERO;
        self.dwell_state.activation_delay = Duration::from_millis(200);
        SWITCH_DEBOUNCE_MS.store(180, Ordering::Relaxed);
        SWITCH_LONG_PRESS_MS.store(1100, Ordering::Relaxed);
        self.switch_auto_scroll = false;
    }

    pub fn select_exclusive_disability_preset(&mut self, preset: DisabilityPreset, ctx: &egui::Context) {
        self.active_disability_presets.clear();
        self.active_disability_presets.insert(preset);
        self.apply_active_disability_presets(ctx);
    }

    pub fn toggle_disability_preset(&mut self, preset: DisabilityPreset, ctx: &egui::Context) {
        if self.active_disability_presets.contains(&preset) {
            self.active_disability_presets.remove(&preset);
        } else {
            self.active_disability_presets.insert(preset);
        }
        self.apply_active_disability_presets(ctx);
    }

    pub fn apply_active_disability_presets(&mut self, ctx: &egui::Context) {
        let keys_str = self
            .active_disability_presets
            .iter()
            .map(|p| p.key())
            .collect::<Vec<_>>()
            .join(",");
        let _ = write_env_var_to_file(".env", "DISABILITY_PRESETS", &keys_str);
        std::env::set_var("DISABILITY_PRESETS", &keys_str);

        // Si aucun profil ou un seul profil est actif (bascule d'un preset à l'autre),
        // on réinitialise l'ensemble des réglages par défaut avant d'appliquer le profil.
        if self.active_disability_presets.len() <= 1 {
            self.reset_disability_parameters_to_default(ctx);
        }

        let has_visual = self.active_disability_presets.contains(&DisabilityPreset::Visual);
        let has_switch = self.active_disability_presets.contains(&DisabilityPreset::MotorSwitch);
        let has_tremor = self.active_disability_presets.contains(&DisabilityPreset::Tremor);
        let has_cognitive = self.active_disability_presets.contains(&DisabilityPreset::Cognitive);

        // 1. Thème d'affichage cumulatif
        if has_visual {
            self.set_theme(AppTheme::HighContrastYellow);
        } else if has_cognitive {
            self.set_theme(AppTheme::SoftAmber);
        }

        // 2. Facteur de zoom cumulatif (prend la valeur la plus adaptée aux besoins cochés)
        let mut min_zoom = 1.0f32;
        if has_visual {
            min_zoom = min_zoom.max(1.40);
        }
        if has_tremor {
            min_zoom = min_zoom.max(1.20);
        }
        if has_switch {
            min_zoom = min_zoom.max(1.15);
        }
        if has_cognitive {
            min_zoom = min_zoom.max(1.10);
        }
        self.set_zoom_factor(min_zoom);
        ctx.set_zoom_factor(min_zoom);

        // 3. Synthèse vocale & retours sonores
        if has_visual {
            self.auto_read_agent_messages = true;
            self.auto_read_suggestions = true;
            self.earcons_enabled = true;
            crate::overlay::set_earcons_enabled(true);
        } else if has_cognitive || has_tremor {
            self.auto_read_agent_messages = true;
            self.auto_read_suggestions = false;
        } else if !has_switch {
            self.auto_read_agent_messages = true;
            self.auto_read_suggestions = true;
        }

        // 4. Contacteur, Dwell time et anti-rebond (debounce)
        if has_tremor && has_switch {
            self.dwell_state.dwell_duration = Duration::from_millis(1800);
            self.dwell_state.activation_delay = Duration::from_millis(600);
            SWITCH_DEBOUNCE_MS.store(450, Ordering::Relaxed);
            SWITCH_LONG_PRESS_MS.store(1600, Ordering::Relaxed);
            self.switch_auto_scroll = true;
        } else if has_tremor {
            self.dwell_state.dwell_duration = Duration::ZERO;
            self.dwell_state.activation_delay = Duration::from_millis(200);
            SWITCH_DEBOUNCE_MS.store(450, Ordering::Relaxed);
            SWITCH_LONG_PRESS_MS.store(1600, Ordering::Relaxed);
            self.switch_auto_scroll = false;
        } else if has_switch {
            self.dwell_state.dwell_duration = Duration::from_millis(1400);
            self.dwell_state.activation_delay = Duration::from_millis(450);
            SWITCH_DEBOUNCE_MS.store(220, Ordering::Relaxed);
            SWITCH_LONG_PRESS_MS.store(1200, Ordering::Relaxed);
            self.switch_auto_scroll = true;
        }
        let _ = write_env_var_to_file(".env", "DWELL_TIME_MS", &self.dwell_state.dwell_duration.as_millis().to_string());
        let _ = write_env_var_to_file(".env", "DWELL_ACTIVATION_DELAY_MS", &self.dwell_state.activation_delay.as_millis().to_string());
        let _ = write_env_var_to_file(".env", "SWITCH_DEBOUNCE_MS", &SWITCH_DEBOUNCE_MS.load(Ordering::Relaxed).to_string());
        let _ = write_env_var_to_file(".env", "SWITCH_LONG_PRESS_MS", &SWITCH_LONG_PRESS_MS.load(Ordering::Relaxed).to_string());
        let _ = write_env_var_to_file(".env", "AUTO_READ_MESSAGES", if self.auto_read_agent_messages { "true" } else { "false" });
        let _ = write_env_var_to_file(".env", "AUTO_READ_SUGGESTIONS", if self.auto_read_suggestions { "true" } else { "false" });
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
                        let close_btn = ui.add(
                            egui::Button::new(
                                egui::RichText::new("✕")
                                    .size(11.0)
                                    .color(egui::Color32::from_rgb(255, 130, 130)),
                            )
                            .min_size(egui::vec2(18.0, 16.0))
                            .corner_radius(egui::CornerRadius::same(4)),
                        ).on_hover_text(t!("ui.btn_close"));
                        close_btn.widget_info(|| {
                            egui::WidgetInfo::labeled(egui::WidgetType::Button, ui.is_enabled(), t!("ui.btn_close").to_string())
                        });
                        if make_widget_accessible_and_activated(ui, &close_btn, theme.accent) {
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
                                let is_llm_key = make_widget_accessible_and_activated(ui, &tab_llm, theme.accent);
                                if llm_activated || is_llm_key {
                                    self.settings_tab = SettingsTab::LlmProvider;
                                }

                                let is_a11y_active = self.settings_tab == SettingsTab::Accessibility;
                                let tab_a11y = ui.selectable_label(
                                    is_a11y_active,
                                    egui::RichText::new(t!("ui.settings_tab_accessibility"))
                                        .size(12.0)
                                        .color(if is_a11y_active { theme.accent } else { theme.text_secondary })
                                        .strong(),
                                );
                                let tab_a11y_id = ui.make_persistent_id("dwell_tab_accessibility");
                                let is_a11y_key = make_widget_accessible_and_activated(ui, &tab_a11y, theme.accent);
                                if !is_a11y_active {
                                    if apply_dwell_to_response(ui, &mut self.dwell_state, tab_a11y_id, &tab_a11y) || is_a11y_key {
                                        self.settings_tab = SettingsTab::Accessibility;
                                    }
                                } else if tab_a11y.clicked() || is_a11y_key {
                                    self.settings_tab = SettingsTab::Accessibility;
                                }

                                let is_voice_active = self.settings_tab == SettingsTab::Voice;
                                let tab_voice = ui.selectable_label(
                                    is_voice_active,
                                    egui::RichText::new("🗣️ Voix")
                                        .size(12.0)
                                        .color(if is_voice_active { theme.accent } else { theme.text_secondary })
                                        .strong(),
                                );
                                let tab_voice_id = ui.make_persistent_id("dwell_tab_voice");
                                let voice_activated = if !is_voice_active {
                                    apply_dwell_to_response(ui, &mut self.dwell_state, tab_voice_id, &tab_voice)
                                } else {
                                    tab_voice.clicked()
                                };
                                let is_voice_key = make_widget_accessible_and_activated(ui, &tab_voice, theme.accent);
                                if voice_activated || is_voice_key {
                                    self.settings_tab = SettingsTab::Voice;
                                    self.voice_status_feedback = None;
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
                                let is_theme_key = make_widget_accessible_and_activated(ui, &tab_theme, theme.accent);
                                if theme_activated || is_theme_key {
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
                                let is_lang_key = make_widget_accessible_and_activated(ui, &tab_lang, theme.accent);
                                if lang_activated || is_lang_key {
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
                                let is_macro_key = make_widget_accessible_and_activated(ui, &tab_macro, theme.accent);
                                if macro_activated || is_macro_key {
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
                                let is_devices_key = make_widget_accessible_and_activated(ui, &tab_devices, theme.accent);
                                if devices_activated || is_devices_key {
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

                                        let p_resp = ui.selectable_label(is_selected_in_modal, label_rich);
                                        if make_widget_accessible_and_activated(ui, &p_resp, theme.accent) {
                                            self.sync_modal_inputs_for_provider(provider);
                                        }
                                    }
                                });

                                ui.add_space(4.0);

                                ui.horizontal(|ui| {
                                    let mut is_enabled = self.selected_provider_modal.is_enabled();
                                    let chk_resp = ui.checkbox(&mut is_enabled, t!("ui.provider_enabled_checkbox"));
                                    if make_toggle_accessible(ui, &chk_resp, theme.accent) {
                                        let _ = self.selected_provider_modal.set_enabled(is_enabled);
                                        self.llm_config = resolve_llm_config();
                                    }

                                    if self.selected_provider_modal != self.llm_config.provider {
                                        let prim_btn = ui.button(t!("ui.btn_set_primary_provider"));
                                        if make_widget_accessible_and_activated(ui, &prim_btn, theme.accent) {
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
                                                    if make_widget_accessible_and_activated(ui, &radio_resp, theme.accent) && !is_active {
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
                                                        let del_btn = ui.small_button("🗑").on_hover_text(t!("ui.local_model_delete_tooltip"));
                                                        if make_widget_accessible_and_activated(ui, &del_btn, theme.accent) {
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
                                            if let Some(ref dl) = self.active_download {
                                                ui.add(egui::ProgressBar::new(dl.percent / 100.0).text(format!("{} - {:.1} % ({:.1} / {:.1} Mo - {:.1} Mo/s)", dl.label, dl.percent, dl.downloaded_mb, dl.total_mb, dl.speed_mb_s)).animate(true));
                                            } else if !active_installed {
                                                let event_tx_clone = self.event_sender.clone();
                                                let p = *active_preset;
                                                let btn_text = t!("ui.local_model_download_btn", name = p.name, size = p.size_desc).to_string();
                                                let dl_btn = ui.button(egui::RichText::new(btn_text).strong().color(theme.accent));
                                                if make_widget_accessible_and_activated(ui, &dl_btn, theme.accent) {
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

                                    // Bloc dédié Qwen2-VL : Supplément visuel & Grounding GUI
                                    egui::Frame::new()
                                        .fill(theme.control_bg)
                                        .stroke(egui::Stroke::new(1.0, theme.accent))
                                        .corner_radius(egui::CornerRadius::same(8))
                                        .inner_margin(egui::Margin::symmetric(10, 8))
                                        .show(ui, |ui| {
                                            ui.horizontal(|ui| {
                                                ui.label(
                                                    egui::RichText::new("👁️ Supplément Visuel & GUI Grounding (Qwen2-VL)")
                                                        .size(11.5)
                                                        .color(theme.accent)
                                                        .strong(),
                                                );
                                            });
                                            ui.add_space(2.0);
                                            ui.label(
                                                egui::RichText::new("Détecte visuellement les icônes sans texte (☰, ⚙️, 🔍), les toiles Canvas HTML5 et les interfaces non exposées par l'accessibilité native.")
                                                    .size(10.5)
                                                    .color(theme.text_secondary),
                                            );
                                            ui.add_space(4.0);

                                            let mut vlm_on = local_llm::is_vlm_grounding_enabled();
                                            if ui.checkbox(&mut vlm_on, "Activer le Grounding Visuel Qwen2-VL").changed() {
                                                local_llm::set_vlm_grounding_enabled(vlm_on);
                                                if vlm_on {
                                                    local_llm::init_vlm_async(self.event_sender.clone());
                                                }
                                                play_earcon(Earcon::Success);
                                            }

                                            ui.add_space(4.0);
                                            let active_vlm = local_llm::get_active_vlm_preset();
                                            let mut model_to_delete = None;

                                            for preset in local_llm::VLM_PRESETS {
                                                let is_active = active_vlm.id == preset.id;
                                                let is_installed = local_llm::is_preset_downloaded(preset);

                                                ui.horizontal(|ui| {
                                                    let radio_label = format!("{} ({})", preset.name, preset.size_desc);
                                                    let radio_resp = ui.radio(is_active, radio_label);
                                                    if make_widget_accessible_and_activated(ui, &radio_resp, theme.accent) && !is_active {
                                                        local_llm::set_active_vlm_preset(preset.id);
                                                        if local_llm::is_vlm_grounding_enabled() {
                                                            local_llm::init_vlm_async(self.event_sender.clone());
                                                        }
                                                        play_earcon(Earcon::Success);
                                                    }

                                                    if is_installed {
                                                        ui.label(egui::RichText::new("✓ Installé").size(10.5).color(egui::Color32::from_rgb(100, 225, 130)).strong());
                                                        let del_btn = ui.small_button("🗑").on_hover_text("Supprimer ce modèle local du disque");
                                                        if make_widget_accessible_and_activated(ui, &del_btn, theme.accent) {
                                                            model_to_delete = Some(preset);
                                                        }
                                                    } else {
                                                        let event_tx_clone = self.event_sender.clone();
                                                        let p = *preset;
                                                        let dl_btn = ui.small_button(format!("📥 Télécharger ({})", p.size_desc));
                                                        if make_widget_accessible_and_activated(ui, &dl_btn, theme.accent) {
                                                            std::thread::spawn(move || {
                                                                let _ = local_llm::ensure_llama_model_downloaded_for_preset(&p, &event_tx_clone);
                                                            });
                                                        }
                                                    }
                                                });
                                            }

                                            if let Some(p) = model_to_delete {
                                                let event_tx = self.event_sender.clone();
                                                std::thread::spawn(move || {
                                                    match local_llm::delete_preset_model(p) {
                                                        Ok(()) => {
                                                            play_earcon(Earcon::Success);
                                                            let _ = event_tx.send(AgentEvent::SilentNarration(format!("Modèle {} supprimé du disque.", p.name)));
                                                        }
                                                        Err(e) => {
                                                            play_earcon(Earcon::Error);
                                                            let _ = event_tx.send(AgentEvent::SilentNarration(format!("Erreur suppression {} : {}", p.name, e)));
                                                        }
                                                    }
                                                });
                                            }

                                            if let Some(ref dl) = self.active_download {
                                                if dl.label.contains("VL") || dl.label.contains("Grounding") {
                                                    ui.add_space(3.0);
                                                    ui.add(
                                                        egui::ProgressBar::new(dl.percent / 100.0)
                                                            .text(format!("{:.1}% ({:.1}/{:.1} Mo - {:.1} Mo/s)", dl.percent, dl.downloaded_mb, dl.total_mb, dl.speed_mb_s))
                                                            .animate(true),
                                                    );
                                                }
                                            }
                                        });

                                    ui.add_space(6.0);

                                    ui.label(egui::RichText::new(t!("ui.api_key_label", env = env_var_target)).size(11.0).color(egui::Color32::WHITE).strong());
                                    ui.add_space(2.0);

                                    ui.horizontal(|ui| {
                                        let key_input_id = egui::Id::NULL.with("api_key_input_field");
                                        let is_key_focused = ctx.memory(|m| m.has_focus(key_input_id));

                                        let paste_key_hit = ctx.input(|i| {
                                            i.events.iter().any(|e| {
                                                matches!(e, egui::Event::Paste(_))
                                                    || matches!(e, egui::Event::Key { key: egui::Key::V, pressed: true, modifiers, .. } if modifiers.ctrl || modifiers.command)
                                                    || matches!(e, egui::Event::Key { key: egui::Key::Insert, pressed: true, modifiers, .. } if modifiers.shift)
                                            })
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
                                        let key_hint = t!("ui.api_key_hint").to_string();
                                        render_accessible_input(
                                            ui,
                                            key_input_id,
                                            &mut self.api_key_input,
                                            &key_hint,
                                            false,
                                            Some(230.0),
                                            None,
                                            !self.show_api_key_plain,
                                            theme,
                                        );

                                        let eye_icon = if self.show_api_key_plain { "🔒" } else { "👁" };
                                        let eye_btn = ui.button(eye_icon).on_hover_text(t!("ui.toggle_api_key_visibility"));
                                        if make_widget_accessible_and_activated(ui, &eye_btn, theme.accent) {
                                            self.show_api_key_plain = !self.show_api_key_plain;
                                        }

                                        let paste_btn = ui.button("📋").on_hover_text(t!("ui.paste_from_clipboard"));
                                        if make_widget_accessible_and_activated(ui, &paste_btn, theme.accent) {
                                            if let Some(clip) = get_clipboard_text() {
                                                self.api_key_input = clip.trim().to_string();
                                            }
                                        }
                                    });

                                    ui.add_space(5.0);
                                    ui.horizontal(|ui| {
                                        ui.label(egui::RichText::new(t!("ui.model_label", env = env_model_target)).size(11.0).color(egui::Color32::WHITE).strong());
                                        ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                                                let rec_btn = ui.small_button(t!("ui.btn_model_recommended")).on_hover_text(t!("ui.restore_default_model_tooltip", model = default_model_name));
                                                if make_widget_accessible_and_activated(ui, &rec_btn, theme.accent) {
                                                self.model_input = default_model_name.to_string();
                                            }
                                        });
                                    });
                                    ui.add_space(2.0);
                                    ui.horizontal(|ui| {
                                        let model_input_id = egui::Id::NULL.with("model_input_field");
                                        render_accessible_input(
                                            ui,
                                            model_input_id,
                                            &mut self.model_input,
                                            default_model_name,
                                            false,
                                            Some(240.0),
                                            None,
                                            false,
                                            theme,
                                        );
                                    });

                                    ui.add_space(5.0);
                                    ui.horizontal(|ui| {
                                        ui.label(egui::RichText::new(t!("ui.endpoint_label")).size(11.0).color(egui::Color32::WHITE).strong());
                                        ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                                                let def_ep_btn = ui.small_button(t!("ui.btn_endpoint_default")).on_hover_text(t!("ui.btn_endpoint_default_tooltip", url = default_endpoint_url).to_string());
                                                if make_widget_accessible_and_activated(ui, &def_ep_btn, theme.accent) {
                                                self.endpoint_input = default_endpoint_url.to_string();
                                            }
                                        });
                                    });
                                    ui.add_space(2.0);
                                    ui.horizontal(|ui| {
                                        let endpoint_id = egui::Id::NULL.with("endpoint_input_field");
                                        render_accessible_input(
                                            ui,
                                            endpoint_id,
                                            &mut self.endpoint_input,
                                            default_endpoint_url,
                                            false,
                                            Some(320.0),
                                            None,
                                            false,
                                            theme,
                                        ).on_hover_text(t!("ui.endpoint_hint_tooltip"));
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

                                let enter_pressed_in_fields = ctx.memory(|m| {
                                    m.has_focus(egui::Id::NULL.with("api_key_input_field"))
                                        || m.has_focus(egui::Id::NULL.with("model_input_field"))
                                        || m.has_focus(egui::Id::NULL.with("endpoint_input_field"))
                                }) && ctx.input(|i| {
                                    i.events.iter().any(|ev| match ev {
                                        egui::Event::Key { key: egui::Key::Enter, pressed: true, .. } => true,
                                        egui::Event::Text(t) if t == "\n" || t == "\r" => true,
                                        _ => false,
                                    })
                                });

                                ui.add_space(5.0);
                                ui.horizontal(|ui| {
                                    let save_btn = ui.button(t!("ui.btn_save_apply"));
                                    let save_btn_hit = make_widget_accessible_and_activated(ui, &save_btn, theme.accent);
                                    if save_btn_hit || enter_pressed_in_fields {
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
                                    let cancel_btn = ui.button(t!("ui.btn_cancel"));
                                    if make_widget_accessible_and_activated(ui, &cancel_btn, theme.accent) {
                                        self.show_api_key_modal = false;
                                        self.api_key_error = None;
                                    }
                                });
                            }
                            SettingsTab::Accessibility => {
                                // 1. Profils de handicap
                                ui.label(
                                    egui::RichText::new(t!("ui.a11y_presets_title"))
                                        .size(12.0)
                                        .color(theme.accent)
                                        .strong(),
                                );
                                ui.label(
                                    egui::RichText::new(t!("ui.a11y_presets_desc"))
                                        .size(11.0)
                                        .color(theme.text_secondary),
                                );
                                ui.add_space(6.0);

                                egui::Frame::new()
                                    .fill(theme.control_bg)
                                    .stroke(egui::Stroke::new(1.0, theme.border_subtle))
                                    .corner_radius(egui::CornerRadius::same(8))
                                    .inner_margin(egui::Margin::symmetric(10, 8))
                                    .show(ui, |ui| {
                                        for &preset in DisabilityPreset::ALL {
                                            let mut is_checked = self.active_disability_presets.contains(&preset);
                                            ui.horizontal(|ui| {
                                                let chk_resp = ui.checkbox(&mut is_checked, preset.display_name());
                                                let chk_id = ui.make_persistent_id(format!("dwell_preset_{:?}", preset));
                                                let is_toggle_hit = make_toggle_accessible(ui, &chk_resp, theme.accent);
                                                if chk_resp.changed() || apply_dwell_to_response(ui, &mut self.dwell_state, chk_id, &chk_resp) || is_toggle_hit {
                                                    self.toggle_disability_preset(preset, ctx);
                                                    play_earcon(Earcon::Success);
                                                }
                                                ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                                                    let is_solo = is_checked && self.active_disability_presets.len() == 1;
                                                    let solo_label = if is_solo { t!("ui.a11y_preset_solo_active").to_string() } else { t!("ui.a11y_preset_solo").to_string() };
                                                    let solo_btn = ui.small_button(solo_label).on_hover_text(t!("ui.a11y_preset_solo_tooltip"));
                                                    if make_widget_accessible_and_activated(ui, &solo_btn, theme.accent) {
                                                        self.select_exclusive_disability_preset(preset, ctx);
                                                        play_earcon(Earcon::Success);
                                                    }
                                            ui.add_space(4.0);
                                            let open_inst_btn = ui.small_button("🎵 Ouvrir la Boîte à Rythme & Instrument adapté")
                                                .on_hover_text("Ouvre le mini-jeu musical adapté au contacteur unique.");
                                            if make_widget_accessible_and_activated(ui, &open_inst_btn, theme.accent) {
                                                self.open_instrument_card();
                                                self.show_api_key_modal = false;
                                                play_earcon(Earcon::Success);
                                            }
                                                });
                                            });
                                            ui.add_space(2.0);
                                        }
                                        ui.add_space(4.0);
                                        ui.horizontal(|ui| {
                                            let reset_all_btn = ui.small_button(t!("ui.a11y_reset_presets_btn"))
                                                .on_hover_text(t!("ui.a11y_reset_presets_tooltip"));
                                            if make_widget_accessible_and_activated(ui, &reset_all_btn, theme.accent) {
                                                self.active_disability_presets.clear();
                                                self.apply_active_disability_presets(ctx);
                                                play_earcon(Earcon::Success);
                                            }
                                            ui.add_space(4.0);
                                            let open_inst_btn = ui.small_button(t!("instrument.open_btn"))
                                                .on_hover_text(t!("instrument.open_btn_tooltip"));
                                            if make_widget_accessible_and_activated(ui, &open_inst_btn, theme.accent) {
                                                self.open_instrument_card();
                                                self.show_api_key_modal = false;
                                                play_earcon(Earcon::Success);
                                            }
                                        });
                                    });

                                ui.add_space(8.0);
                                ui.separator();
                                ui.add_space(6.0);

                                // 2. Options du narrateur et retours audio
                                ui.label(
                                    egui::RichText::new(t!("ui.a11y_audio_section_title"))
                                        .size(12.0)
                                        .color(theme.accent)
                                        .strong(),
                                );
                                ui.add_space(4.0);

                                egui::Frame::new()
                                    .fill(theme.control_bg)
                                    .stroke(egui::Stroke::new(1.0, theme.border_subtle))
                                    .corner_radius(egui::CornerRadius::same(8))
                                    .inner_margin(egui::Margin::symmetric(10, 8))
                                    .show(ui, |ui| {
                                        ui.label(
                                            egui::RichText::new(t!("ui.a11y_narrator_voice"))
                                                .size(11.5)
                                                .strong()
                                                .color(egui::Color32::WHITE),
                                        );
                                        ui.add_space(4.0);
                                        ui.horizontal_wrapped(|ui| {
                                            ui.spacing_mut().item_spacing = egui::vec2(6.0, 4.0);
                                            for &profile in VoiceProfile::ALL {
                                                let is_selected = self.voice_gender == profile;
                                                let (icon, label) = match profile {
                                                    VoiceProfile::Denise => ("👩", "Denise"),
                                                    VoiceProfile::Henri => ("👨", "Henri"),
                                                    VoiceProfile::Hortense => ("🎙️", "Hortense"),
                                                    VoiceProfile::Paul => ("🎩", "Paul"),
                                                    VoiceProfile::Sylvie => ("🍁", "Sylvie"),
                                                    VoiceProfile::Jean => ("⚡", "Jean"),
                                                    VoiceProfile::Clone => ("🧬", "Clonée"),
                                                };
                                                let btn_text = format!("{icon} {label}");
                                                let resp = ui.selectable_label(is_selected, btn_text);
                                                let id = ui.make_persistent_id(format!("dwell_voice_{:?}", profile));
                                                let is_key = make_widget_accessible_and_activated(ui, &resp, theme.accent);
                                                if resp.clicked() || apply_dwell_to_response(ui, &mut self.dwell_state, id, &resp) || is_key {
                                                    if !is_selected {
                                                        self.set_voice_gender(profile);
                                                        play_earcon(Earcon::Success);
                                                    }
                                                }
                                            }
                                        });
                                        ui.add_space(6.0);
                                        ui.separator();
                                        ui.add_space(4.0);
                                        let mut read_msgs = self.auto_read_agent_messages;
                                        let resp_msgs = ui.checkbox(
                                            &mut read_msgs,
                                            egui::RichText::new(t!("ui.a11y_auto_read_msgs"))
                                                .size(11.5)
                                                .color(theme.text_primary),
                                        );
                                        let id_msgs = ui.make_persistent_id("dwell_auto_read_msgs");
                                        let is_msgs_key = make_toggle_accessible(ui, &resp_msgs, theme.accent);
                                        if resp_msgs.changed() || apply_dwell_to_response(ui, &mut self.dwell_state, id_msgs, &resp_msgs) || is_msgs_key {
                                            if resp_msgs.changed() {
                                                self.auto_read_agent_messages = read_msgs;
                                            } else {
                                                self.auto_read_agent_messages = !self.auto_read_agent_messages;
                                            }
                                            let val_str = if self.auto_read_agent_messages { "true" } else { "false" };
                                            let _ = write_env_var_to_file(".env", "AUTO_READ_MESSAGES", val_str);
                                            std::env::set_var("AUTO_READ_MESSAGES", val_str);
                                        }

                                        ui.add_space(4.0);

                                        let mut read_sugs = self.auto_read_suggestions;
                                        let resp_sugs = ui.checkbox(
                                            &mut read_sugs,
                                            egui::RichText::new(t!("ui.a11y_auto_read_sugs"))
                                                .size(11.5)
                                                .color(theme.text_primary),
                                        );
                                        let id_sugs = ui.make_persistent_id("dwell_auto_read_sugs");
                                        let is_sugs_key = make_toggle_accessible(ui, &resp_sugs, theme.accent);
                                        if resp_sugs.changed() || apply_dwell_to_response(ui, &mut self.dwell_state, id_sugs, &resp_sugs) || is_sugs_key {
                                            if resp_sugs.changed() {
                                                self.auto_read_suggestions = read_sugs;
                                            } else {
                                                self.auto_read_suggestions = !self.auto_read_suggestions;
                                            }
                                            let val_str = if self.auto_read_suggestions { "true" } else { "false" };
                                            let _ = write_env_var_to_file(".env", "AUTO_READ_SUGGESTIONS", val_str);
                                            std::env::set_var("AUTO_READ_SUGGESTIONS", val_str);
                                        }

                                        ui.add_space(4.0);

                                        let mut earcons = self.earcons_enabled;
                                        let resp_earcons = ui.checkbox(
                                            &mut earcons,
                                            egui::RichText::new(t!("ui.a11y_earcons"))
                                                .size(11.5)
                                                .color(theme.text_primary),
                                        );
                                        let id_earcons = ui.make_persistent_id("dwell_earcons_enabled");
                                        let is_earcons_key = make_toggle_accessible(ui, &resp_earcons, theme.accent);
                                        if resp_earcons.changed() || apply_dwell_to_response(ui, &mut self.dwell_state, id_earcons, &resp_earcons) || is_earcons_key {
                                            if resp_earcons.changed() {
                                                self.earcons_enabled = earcons;
                                            } else {
                                                self.earcons_enabled = !self.earcons_enabled;
                                            }
                                            crate::overlay::set_earcons_enabled(self.earcons_enabled);
                                        }
                                    });

                                ui.add_space(8.0);
                                ui.separator();
                                ui.add_space(6.0);

                                // 3. Clic par temporisation au survol (Dwell Timing)
                                ui.label(egui::RichText::new(t!("ui.dwell_timing_label")).size(12.0).color(theme.accent).strong());
                                ui.add_space(4.0);
                                egui::Frame::new()
                                    .fill(theme.control_bg)
                                    .stroke(egui::Stroke::new(1.0, theme.border_subtle))
                                    .corner_radius(egui::CornerRadius::same(8))
                                    .inner_margin(egui::Margin::symmetric(10, 8))
                                    .show(ui, |ui| {
                                        let mut current_dwell_ms = self.dwell_state.dwell_duration.as_millis() as u64;
                                        let prev_dwell_ms = current_dwell_ms;
                                        ui.horizontal(|ui| {
                                            ui.label(egui::RichText::new(t!("ui.dwell_delay_label")).size(11.5).strong().color(egui::Color32::WHITE));
                                            let slider = egui::Slider::new(&mut current_dwell_ms, 0..=5000).step_by(50.0).custom_formatter(|val, _| {
                                                if val <= 0.0 { t!("ui.dwell_disabled").to_string() } else { format!("{:.0} ms", val) }
                                            });
                                            let slider_resp = ui.add(slider).on_hover_text(t!("ui.dwell_tooltip"));
                                            make_slider_accessible(ui, &slider_resp, theme.accent);
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
                                                ui.label(egui::RichText::new(t!("ui.dwell_gauge_delay_label")).size(11.5).strong().color(egui::Color32::WHITE));
                                                let act_slider = egui::Slider::new(&mut current_act_delay_ms, 0..=2500).step_by(50.0).custom_formatter(|val, _| {
                                                    if val <= 0.0 { t!("ui.dwell_gauge_delay_immediate").to_string() } else { format!("{:.0} ms", val) }
                                                });
                                                let act_resp = ui.add(act_slider).on_hover_text(t!("ui.dwell_gauge_delay_tooltip"));
                                                make_slider_accessible(ui, &act_resp, theme.accent);
                                            });
                                            if current_act_delay_ms != prev_act_delay_ms {
                                                self.dwell_state.activation_delay = Duration::from_millis(current_act_delay_ms);
                                                let _ = write_env_var_to_file(".env", "DWELL_ACTIVATION_DELAY_MS", &current_act_delay_ms.to_string());
                                                std::env::set_var("DWELL_ACTIVATION_DELAY_MS", current_act_delay_ms.to_string());
                                            }
                                        }

                                        ui.add_space(4.0);
                                        ui.separator();
                                        ui.add_space(4.0);

                                        let mut auto_scan = self.switch_auto_scroll;
                                        let resp_auto_scan = ui.checkbox(
                                            &mut auto_scan,
                                            egui::RichText::new("Défilement automatique (Balayage contacteur unique)")
                                                .size(11.5)
                                                .color(theme.text_primary),
                                        ).on_hover_text("Active le balayage automatique cadencé en mode contacteur unique (désactivé par défaut en mode survol)");
                                        let id_auto_scan = ui.make_persistent_id("dwell_switch_auto_scroll");
                                        let is_scan_key = make_toggle_accessible(ui, &resp_auto_scan, theme.accent);
                                        if resp_auto_scan.changed() || apply_dwell_to_response(ui, &mut self.dwell_state, id_auto_scan, &resp_auto_scan) || is_scan_key {
                                            if resp_auto_scan.changed() {
                                                self.switch_auto_scroll = auto_scan;
                                            } else {
                                                self.switch_auto_scroll = !self.switch_auto_scroll;
                                            }
                                            let val_str = if self.switch_auto_scroll { "true" } else { "false" };
                                            let _ = write_env_var_to_file(".env", "SWITCH_AUTO_SCROLL", val_str);
                                            std::env::set_var("SWITCH_AUTO_SCROLL", val_str);
                                        }

                                        // 4. Contacteur unique (Switch Access)
                                        let mut cur_long_press = SWITCH_LONG_PRESS_MS.load(Ordering::Relaxed);
                                        let prev_long_press = cur_long_press;
                                        ui.horizontal(|ui| {
                                            ui.label(egui::RichText::new(t!("ui.switch_long_press_label")).size(11.5).strong().color(egui::Color32::WHITE));
                                            let slider = egui::Slider::new(&mut cur_long_press, 400..=4000).step_by(50.0).suffix(" ms");
                                            let slider_resp = ui.add(slider).on_hover_text(t!("ui.switch_long_press_tooltip"));
                                            make_slider_accessible(ui, &slider_resp, theme.accent);
                                        });
                                        if cur_long_press != prev_long_press {
                                            SWITCH_LONG_PRESS_MS.store(cur_long_press, Ordering::Relaxed);
                                            let _ = write_env_var_to_file(".env", "SWITCH_LONG_PRESS_MS", &cur_long_press.to_string());
                                            std::env::set_var("SWITCH_LONG_PRESS_MS", cur_long_press.to_string());
                                        }

                                        let mut cur_debounce = SWITCH_DEBOUNCE_MS.load(Ordering::Relaxed);
                                        let prev_debounce = cur_debounce;
                                        ui.horizontal(|ui| {
                                            ui.label(egui::RichText::new(t!("ui.switch_debounce_label")).size(11.5).strong().color(egui::Color32::WHITE));
                                            let slider = egui::Slider::new(&mut cur_debounce, 50..=1500).step_by(25.0).suffix(" ms");
                                            let slider_resp = ui.add(slider).on_hover_text(t!("ui.switch_debounce_tooltip"));
                                            make_slider_accessible(ui, &slider_resp, theme.accent);
                                        });
                                        if cur_debounce != prev_debounce {
                                            SWITCH_DEBOUNCE_MS.store(cur_debounce, Ordering::Relaxed);
                                            let _ = write_env_var_to_file(".env", "SWITCH_DEBOUNCE_MS", &cur_debounce.to_string());
                                            std::env::set_var("SWITCH_DEBOUNCE_MS", cur_debounce.to_string());
                                        }
                                    });

                                ui.add_space(8.0);
                                ui.horizontal(|ui| {
                                    let close_a11y_btn = ui.button(t!("ui.btn_close"));
                                    if make_widget_accessible_and_activated(ui, &close_a11y_btn, theme.accent) {
                                        self.show_api_key_modal = false;
                                    }
                                });
                            }
                            SettingsTab::Voice => {
                                ui.label(
                                    egui::RichText::new("Sélection de la voix du narrateur et clonage vocal personnalisé.")
                                        .size(11.5)
                                        .color(theme.text_secondary),
                                );
                                ui.add_space(6.0);

                                // 1. Choix du profil vocal
                                egui::Frame::new()
                                    .fill(theme.control_bg)
                                    .stroke(egui::Stroke::new(1.0, theme.border_subtle))
                                    .corner_radius(egui::CornerRadius::same(8))
                                    .inner_margin(egui::Margin::symmetric(10, 8))
                                    .show(ui, |ui| {
                                        ui.label(
                                            egui::RichText::new("Voix active du narrateur")
                                                .size(12.0)
                                                .strong()
                                                .color(theme.accent),
                                        );
                                        ui.add_space(4.0);
                                        ui.horizontal_wrapped(|ui| {
                                            ui.spacing_mut().item_spacing = egui::vec2(6.0, 4.0);
                                            for &profile in VoiceProfile::ALL {
                                                let is_selected = self.voice_gender == profile;
                                                let (icon, label) = match profile {
                                                    VoiceProfile::Denise => ("👩", "Denise (Féminine)"),
                                                    VoiceProfile::Henri => ("👨", "Henri (Masculin)"),
                                                    VoiceProfile::Hortense => ("🎙️", "Hortense"),
                                                    VoiceProfile::Paul => ("🎩", "Paul"),
                                                    VoiceProfile::Sylvie => ("🍁", "Sylvie (CA)"),
                                                    VoiceProfile::Jean => ("⚡", "Jean"),
                                                    VoiceProfile::Clone => {
                                                        let g = if self.voice_clone_gender == "male" { "Masculine" } else { "Féminine" };
                                                        ("🧬", if g == "Masculine" { "Voix clonée (Masculine)" } else { "Voix clonée (Féminine)" })
                                                    }
                                                };
                                                let btn_text = format!("{icon} {label}");
                                                let resp = ui.selectable_label(is_selected, btn_text);
                                                let id = ui.make_persistent_id(format!("dwell_voice_tab_{:?}", profile));
                                                let is_key = make_widget_accessible_and_activated(ui, &resp, theme.accent);
                                                if resp.clicked() || apply_dwell_to_response(ui, &mut self.dwell_state, id, &resp) || is_key {
                                                    if !is_selected {
                                                        self.set_voice_gender(profile);
                                                        play_earcon(Earcon::Success);
                                                    }
                                                }
                                            }
                                        });

                                        ui.add_space(8.0);
                                        ui.separator();
                                        ui.add_space(6.0);

                                        // Zone de test de la voix
                                        ui.label(
                                            egui::RichText::new("Tester la voix")
                                                .size(11.0)
                                                .color(egui::Color32::WHITE)
                                                .strong(),
                                        );
                                        ui.add_space(2.0);
                                        let enter_in_test_field = ctx.memory(|m| {
                                            m.has_focus(egui::Id::NULL.with("voice_test_text_input"))
                                        }) && ctx.input(|i| {
                                            i.events.iter().any(|ev| match ev {
                                                egui::Event::Key { key: egui::Key::Enter, pressed: true, .. } => true,
                                                egui::Event::Text(t) if t == "\n" || t == "\r" => true,
                                                _ => false,
                                            })
                                        });

                                        ui.horizontal(|ui| {
                                            render_accessible_input(
                                                ui,
                                                egui::Id::NULL.with("voice_test_text_input"),
                                                &mut self.voice_test_text_input,
                                                "Phrase de test audio...",
                                                false,
                                                Some((ui.available_width() - 95.0).max(120.0)),
                                                None,
                                                false,
                                                theme,
                                            );
                                            let test_btn = ui.button("🔊 Tester");
                                            let test_btn_id = ui.make_persistent_id("dwell_voice_test_btn");
                                            let test_btn_hit = make_widget_accessible_and_activated(ui, &test_btn, theme.accent);
                                            let test_dwell = apply_dwell_to_response(ui, &mut self.dwell_state, test_btn_id, &test_btn);
                                            if test_btn_hit || test_dwell || enter_in_test_field {
                                                let tts_text = if self.voice_test_text_input.trim().is_empty() {
                                                    "Bonjour, ceci est un test de synthèse vocale avec Libertide.".to_string()
                                                } else {
                                                    self.voice_test_text_input.trim().to_string()
                                                };
                                                let _ = self.tts_sender.send(TtsCommand::Speak(tts_text));
                                                play_earcon(Earcon::Success);
                                            }
                                        });
                                    });

                                ui.add_space(8.0);
                                ui.separator();
                                ui.add_space(6.0);

                                // 2. Entraîneur de données "Click & Record" & Clonage vocal
                                let sample_path = std::path::Path::new("models/voice_reference.wav");
                                let effective_path = if sample_path.is_file() {
                                    Some(sample_path)
                                } else if !self.voice_reference_wav_input.trim().is_empty() && std::path::Path::new(self.voice_reference_wav_input.trim()).is_file() {
                                    Some(std::path::Path::new(self.voice_reference_wav_input.trim()))
                                } else {
                                    None
                                };
                                let sample_exists = effective_path.is_some();

                                if sample_exists && self.voice_sample_duration_secs <= 0.0 {
                                    if let Some(p) = effective_path {
                                        if let Some(dur) = crate::tts::get_wav_duration_secs(p) {
                                            self.voice_sample_duration_secs = dur;
                                        }
                                    }
                                }
                                let total_dur = self.voice_sample_duration_secs;

                                // Dépouillement asynchrone de la phrase guide générée par le LLM
                                if let Some(ref rx) = self.voice_guide_rx {
                                    match rx.try_recv() {
                                        Ok(phrase) => {
                                            self.voice_guide_phrase = phrase;
                                            self.voice_guide_generating = false;
                                            self.voice_guide_rx = None;
                                            ctx.request_repaint();
                                        }
                                        Err(std::sync::mpsc::TryRecvError::Disconnected) => {
                                            self.voice_guide_generating = false;
                                            self.voice_guide_rx = None;
                                            ctx.request_repaint();
                                        }
                                        Err(std::sync::mpsc::TryRecvError::Empty) => {
                                            ctx.request_repaint_after(Duration::from_millis(80));
                                        }
                                    }
                                }

                                // Dépouillement des samples une fois l'enregistrement terminé
                                if let Some(ref rx) = self.voice_recorder_result_rx {
                                    if let Ok(samples) = rx.try_recv() {
                                        self.voice_is_recording_sample = false;
                                        self.voice_recorder_stop_tx = None;
                                        self.voice_recorder_result_rx = None;
                                        self.voice_record_start = None;

                                        if !samples.is_empty() {
                                            let sample_rate = 16000;
                                            let dest_path = std::path::Path::new("models/voice_reference.wav");
                                            let (saved_dur, save_res) = if self.voice_record_append {
                                                match crate::tts::append_wav_file(dest_path, &samples, sample_rate) {
                                                    Ok((_count, dur)) => (dur, Ok(())),
                                                    Err(e) => (samples.len() as f32 / sample_rate as f32, Err(e)),
                                                }
                                            } else {
                                                let dur = samples.len() as f32 / sample_rate as f32;
                                                let res = crate::tts::save_wav_file(dest_path, &samples, sample_rate);
                                                (dur, res)
                                            };

                                            if let Err(e) = save_res {
                                                self.voice_status_feedback = Some(format!("Erreur sauvegarde WAV : {e}"));
                                            } else {
                                                self.voice_sample_duration_secs = saved_dur;
                                                self.voice_reference_wav_input = "models/voice_reference.wav".to_string();
                                                let _ = write_env_var_to_file(".env", "VOICE_REFERENCE_WAV", "models/voice_reference.wav");
                                                std::env::set_var("VOICE_REFERENCE_WAV", "models/voice_reference.wav");

                                                // Transcription automatique en direct par Whisper
                                                if let Some(transcription) = crate::audio::transcribe_audio_samples(&samples) {
                                                    let clean_tr = transcription.trim().to_string();
                                                    if !clean_tr.is_empty() {
                                                        let new_text = if self.voice_record_append && !self.voice_reference_text_input.trim().is_empty() {
                                                            let existing = self.voice_reference_text_input.trim();
                                                            let sep = if existing.ends_with('.') || existing.ends_with('!') || existing.ends_with('?') {
                                                                " "
                                                            } else {
                                                                ". "
                                                            };
                                                            format!("{}{}{}", existing, sep, clean_tr)
                                                        } else {
                                                            clean_tr
                                                        };
                                                        self.voice_reference_text_input = new_text.clone();
                                                        let _ = write_env_var_to_file(".env", "VOICE_REFERENCE_TEXT", &new_text);
                                                        std::env::set_var("VOICE_REFERENCE_TEXT", &new_text);
                                                    }
                                                }

                                                self.set_voice_gender_silent(VoiceProfile::Clone);
                                                if self.voice_record_append {
                                                    self.voice_status_feedback = Some(format!("Extrait ajouté avec succès ! Durée cumulée totale : {:.1}s.", saved_dur));
                                                } else {
                                                    self.voice_status_feedback = Some(format!("Échantillon ({:.1}s) enregistré avec succès et voix clonée prête !", saved_dur));
                                                }
                                                play_earcon(Earcon::Success);
                                            }
                                        }
                                    }
                                }

                                ui.label(
                                    egui::RichText::new("🧬 Entraîneur vocal instantané (Click & Record)")
                                        .size(12.0)
                                        .color(theme.accent)
                                        .strong(),
                                );
                                ui.label(
                                    egui::RichText::new("Enregistrez votre voix en un clic pour créer votre profil vocal de référence sans manipuler de fichiers.")
                                        .size(11.0)
                                        .color(theme.text_secondary),
                                );
                                ui.add_space(4.0);

                                egui::Frame::new()
                                    .fill(theme.control_bg)
                                    .stroke(egui::Stroke::new(1.0, theme.border_subtle))
                                    .corner_radius(egui::CornerRadius::same(8))
                                    .inner_margin(egui::Margin::symmetric(10, 8))
                                    .show(ui, |ui| {
                                        // Sélecteur de genre pour la voix clonée
                                        ui.horizontal(|ui| {
                                            ui.label(
                                                egui::RichText::new("Genre de la voix clonée :")
                                                    .size(11.0)
                                                    .color(egui::Color32::WHITE)
                                                    .strong(),
                                            );
                                            let is_female = self.voice_clone_gender != "male";
                                            let female_btn = ui.selectable_label(
                                                is_female,
                                                egui::RichText::new("👩 Femme (Féminin)")
                                                    .size(11.0)
                                                    .color(if is_female { theme.accent } else { theme.text_secondary }),
                                            ).on_hover_text("Calibre et télécharge le modèle neuronal sur une voix féminine.");
                                            let female_btn_id = ui.make_persistent_id("dwell_voice_clone_gender_female");
                                            let is_female_hit = make_widget_accessible_and_activated(ui, &female_btn, theme.accent);
                                            let is_female_dwell = apply_dwell_to_response(ui, &mut self.dwell_state, female_btn_id, &female_btn);
                                            if (is_female_hit || is_female_dwell) && !is_female {
                                                self.set_voice_clone_gender("female");
                                            }

                                            let is_male = self.voice_clone_gender == "male";
                                            let male_btn = ui.selectable_label(
                                                is_male,
                                                egui::RichText::new("👨 Homme (Masculin)")
                                                    .size(11.0)
                                                    .color(if is_male { theme.accent } else { theme.text_secondary }),
                                            ).on_hover_text("Calibre et télécharge le modèle neuronal sur une voix masculine.");
                                            let male_btn_id = ui.make_persistent_id("dwell_voice_clone_gender_male");
                                            let is_male_hit = make_widget_accessible_and_activated(ui, &male_btn, theme.accent);
                                            let is_male_dwell = apply_dwell_to_response(ui, &mut self.dwell_state, male_btn_id, &male_btn);
                                            if (is_male_hit || is_male_dwell) && !is_male {
                                                self.set_voice_clone_gender("male");
                                            }
                                        });
                                        ui.add_space(4.0);
                                        ui.separator();
                                        ui.add_space(4.0);

                                        // Phrase guide
                                        ui.horizontal(|ui| {
                                            ui.label(egui::RichText::new("Phrase recommandée à lire à voix haute :").size(10.5).color(theme.text_secondary));
                                            ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                                                let btn_text = if self.voice_guide_generating {
                                                    "⏳ Génération..."
                                                } else {
                                                    "✨ Nouvelle phrase (IA)"
                                                };
                                                let gen_btn = ui.small_button(btn_text).on_hover_text("Demande au LLM de générer une phrase phonétiquement riche et naturelle.");
                                                let gen_btn_id = ui.make_persistent_id("dwell_voice_gen_phrase_btn");
                                                let gen_btn_hit = make_widget_accessible_and_activated(ui, &gen_btn, theme.accent);
                                                let gen_dwell = apply_dwell_to_response(ui, &mut self.dwell_state, gen_btn_id, &gen_btn);

                                                if (gen_btn_hit || gen_dwell) && !self.voice_guide_generating {
                                                    self.generate_new_voice_guide_phrase();
                                                    play_earcon(Earcon::Navigation);
                                                    ctx.request_repaint();
                                                }
                                            });
                                        });
                                        ui.add_space(1.0);
                                        egui::Frame::new()
                                            .fill(theme.modal_bg)
                                            .stroke(egui::Stroke::new(1.0, theme.border_subtle))
                                            .corner_radius(egui::CornerRadius::same(6))
                                            .inner_margin(egui::Margin::symmetric(8, 6))
                                            .show(ui, |ui| {
                                                let btn_w = 26.0;
                                                let spacing = 6.0;
                                                let text_w = (ui.available_width() - btn_w - spacing).max(60.0);

                                                ui.horizontal(|ui| {
                                                    ui.add_sized(
                                                        [text_w, 0.0],
                                                        egui::Label::new(
                                                            egui::RichText::new(&self.voice_guide_phrase)
                                                                .size(11.5)
                                                                .italics()
                                                                .color(egui::Color32::from_rgb(230, 240, 255)),
                                                        )
                                                        .wrap(),
                                                    );
                                                    let copy_btn = ui.small_button("📋").on_hover_text("Copier la phrase guide");
                                                    let copy_btn_id = ui.make_persistent_id("dwell_voice_copy_phrase_btn");
                                                    let copy_btn_hit = make_widget_accessible_and_activated(ui, &copy_btn, theme.accent);
                                                    let copy_dwell = apply_dwell_to_response(ui, &mut self.dwell_state, copy_btn_id, &copy_btn);
                                                    if copy_btn_hit || copy_dwell {
                                                        crate::automation::set_clipboard_text(&self.voice_guide_phrase);
                                                        play_earcon(Earcon::Success);
                                                    }
                                                });
                                            });

                                        ui.add_space(6.0);

                                        // Bouton d'enregistrement Click & Record
                                        ui.horizontal(|ui| {
                                            if !self.voice_is_recording_sample {
                                                if sample_exists {
                                                    // Bouton ➕ Ajouter un extrait vocal (Cumuler)
                                                    let append_btn = ui.button(
                                                        egui::RichText::new("➕ Ajouter un extrait vocal (Cumuler)")
                                                            .strong()
                                                            .color(theme.accent),
                                                    ).on_hover_text("Enregistre un nouvel extrait audio et l'ajoute à votre voix de référence existante.");
                                                    let append_btn_id = ui.make_persistent_id("dwell_voice_append_record_btn");
                                                    let append_btn_hit = make_widget_accessible_and_activated(ui, &append_btn, theme.accent);
                                                    let append_dwell = apply_dwell_to_response(ui, &mut self.dwell_state, append_btn_id, &append_btn);

                                                    if append_btn_hit || append_dwell {
                                                        if self.is_recording {
                                                            self.stop_recording();
                                                        }
                                                        self.stop_speaking();
                                                        match crate::audio::record_voice_sample(16000) {
                                                            Ok((stop_tx, result_rx)) => {
                                                                self.voice_is_recording_sample = true;
                                                                self.voice_record_append = true;
                                                                self.voice_record_start = Some(Instant::now());
                                                                self.voice_recorder_stop_tx = Some(stop_tx);
                                                                self.voice_recorder_result_rx = Some(result_rx);
                                                                self.voice_status_feedback = Some("Enregistrement du nouvel extrait... Lisez une nouvelle phrase.".to_string());
                                                                play_earcon(Earcon::MicOpen);
                                                            }
                                                            Err(err) => {
                                                                self.voice_status_feedback = Some(format!("Erreur micro : {err}"));
                                                                play_earcon(Earcon::Error);
                                                            }
                                                        }
                                                    }

                                                    // Bouton de réinitialisation
                                                    let reset_btn = ui.button(
                                                        egui::RichText::new("🔄 Réinitialiser")
                                                            .color(egui::Color32::from_rgb(255, 140, 140)),
                                                    ).on_hover_text("Réinitialise et ré-enregistre un nouvel extrait de zéro (écrase l'échantillon précédent).");
                                                    let reset_btn_id = ui.make_persistent_id("dwell_voice_reset_record_btn");
                                                    let reset_btn_hit = make_widget_accessible_and_activated(ui, &reset_btn, theme.accent);
                                                    let reset_dwell = apply_dwell_to_response(ui, &mut self.dwell_state, reset_btn_id, &reset_btn);

                                                    if reset_btn_hit || reset_dwell {
                                                        if self.is_recording {
                                                            self.stop_recording();
                                                        }
                                                        self.stop_speaking();
                                                        match crate::audio::record_voice_sample(16000) {
                                                            Ok((stop_tx, result_rx)) => {
                                                                self.voice_is_recording_sample = true;
                                                                self.voice_record_append = false;
                                                                self.voice_record_start = Some(Instant::now());
                                                                self.voice_recorder_stop_tx = Some(stop_tx);
                                                                self.voice_recorder_result_rx = Some(result_rx);
                                                                self.voice_status_feedback = Some("Enregistrement en cours... Lisez la phrase ci-dessus.".to_string());
                                                                play_earcon(Earcon::MicOpen);
                                                            }
                                                            Err(err) => {
                                                                self.voice_status_feedback = Some(format!("Erreur micro : {err}"));
                                                                play_earcon(Earcon::Error);
                                                            }
                                                        }
                                                    }
                                                } else {
                                                    let rec_btn = ui.button(egui::RichText::new("🎙️ Enregistrer ma voix (3 à 10s)").strong().color(egui::Color32::WHITE));
                                                let rec_btn_id = ui.make_persistent_id("dwell_voice_start_record_btn");
                                                let rec_btn_hit = make_widget_accessible_and_activated(ui, &rec_btn, theme.accent);
                                                let rec_dwell = apply_dwell_to_response(ui, &mut self.dwell_state, rec_btn_id, &rec_btn);

                                                if rec_btn_hit || rec_dwell {
                                                    if self.is_recording {
                                                        self.stop_recording();
                                                    }
                                                    self.stop_speaking();
                                                    match crate::audio::record_voice_sample(16000) {
                                                        Ok((stop_tx, result_rx)) => {
                                                            self.voice_is_recording_sample = true;
                                                            self.voice_record_append = false;
                                                            self.voice_record_start = Some(Instant::now());
                                                            self.voice_recorder_stop_tx = Some(stop_tx);
                                                            self.voice_recorder_result_rx = Some(result_rx);
                                                            self.voice_status_feedback = Some("Enregistrement en cours... Lisez la phrase ci-dessus.".to_string());
                                                            play_earcon(Earcon::MicOpen);
                                                        }
                                                        Err(err) => {
                                                            self.voice_status_feedback = Some(format!("Erreur micro : {err}"));
                                                            play_earcon(Earcon::Error);
                                                        }
                                                    }
                                                }
                                                }
                                            } else {
                                                let elapsed = self.voice_record_start.map_or(0.0, |s| s.elapsed().as_secs_f32());
                                                let pulse = ((ui.input(|i| i.time) * 4.0).sin().abs() * 0.4 + 0.6) as f32;
                                                let red_dot = egui::Color32::from_rgb((255.0 * pulse) as u8, 40, 40);

                                                ui.label(egui::RichText::new("●").size(14.0).color(red_dot));
                                                let status_str = if self.voice_record_append {
                                                    let projected = total_dur + elapsed;
                                                    format!("Cumul en cours : +{:.1}s | Total cumulé : {:.1}s (max 15s)", elapsed, projected)
                                                } else {
                                                    format!("En cours : {:.1}s (max 15s)", elapsed)
                                                };
                                                ui.label(egui::RichText::new(status_str).size(11.5).strong().color(red_dot));

                                                let stop_rec_btn = ui.button(egui::RichText::new("⏹️ Terminer l'enregistrement").strong().color(egui::Color32::WHITE));
                                                let stop_rec_id = ui.make_persistent_id("dwell_voice_stop_record_btn");
                                                let stop_rec_hit = make_widget_accessible_and_activated(ui, &stop_rec_btn, theme.accent);
                                                let stop_rec_dwell = apply_dwell_to_response(ui, &mut self.dwell_state, stop_rec_id, &stop_rec_btn);

                                                if stop_rec_hit || stop_rec_dwell || elapsed >= 15.0 {
                                                    if let Some(stop_tx) = self.voice_recorder_stop_tx.take() {
                                                        let _ = stop_tx.send(());
                                                    }
                                                    play_earcon(Earcon::MicClose);
                                                    if self.voice_record_append {
                                                        self.voice_status_feedback = Some("Ingestion et cumul du nouvel extrait...".to_string());
                                                    } else {
                                                        self.voice_status_feedback = Some("Ingestion et analyse de l'échantillon...".to_string());
                                                    }
                                                }
                                                ctx.request_repaint();
                                            }

                                            // Bouton écouter l'échantillon existant
                                            if sample_exists && !self.voice_is_recording_sample {
                                                let play_sample_btn = ui.button("▶ Écouter l'échantillon");
                                                let play_sample_id = ui.make_persistent_id("dwell_voice_play_sample_btn");
                                                let play_sample_hit = make_widget_accessible_and_activated(ui, &play_sample_btn, theme.accent);
                                                let play_sample_dwell = apply_dwell_to_response(ui, &mut self.dwell_state, play_sample_id, &play_sample_btn);

                                                if play_sample_hit || play_sample_dwell {
                                                    let p = if sample_path.is_file() {
                                                        sample_path
                                                    } else {
                                                        std::path::Path::new(self.voice_reference_wav_input.trim())
                                                    };
                                                    if let Ok(bytes) = std::fs::read(p) {
                                                        if let Some((mut samples, rate)) = crate::tts::decode_wav_to_samples(&bytes) {
                                                        crate::tts::normalize_audio_samples(&mut samples, 0.92);
                                                            if let Some(ref player) = self.voice_sample_player {
                                                                player.play_samples(samples, rate);
                                                            }
                                                        }
                                                    }
                                                }
                                            }
                                        });

                                        if sample_exists {
                                            ui.add_space(4.0);
                                            ui.horizontal(|ui| {
                                                ui.label(
                                                    egui::RichText::new("✓ Échantillon vocal actif : models/voice_reference.wav")
                                                        .size(10.5)
                                                        .color(egui::Color32::from_rgb(100, 225, 130)),
                                                );
                                                ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                                                    ui.label(
                                                        egui::RichText::new(format!("⏱️ Durée cumulée totale : {:.1}s", total_dur))
                                                            .size(11.0)
                                                            .strong()
                                                            .color(theme.accent),
                                                    );
                                                });
                                            });
                                        }

                                        // Champ de transcription de référence directement éditable
                                        ui.add_space(6.0);
                                        ui.horizontal(|ui| {
                                            ui.label(
                                                egui::RichText::new("Transcription de référence (éditable) :")
                                                    .size(11.0)
                                                    .color(egui::Color32::WHITE)
                                                    .strong(),
                                            );
                                            ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                                                if !self.voice_reference_text_input.trim().is_empty() {
                                                    let clear_btn = ui.small_button("🗑 Effacer").on_hover_text("Effacer le texte de transcription");
                                                    if make_widget_accessible_and_activated(ui, &clear_btn, theme.accent) {
                                                        self.voice_reference_text_input.clear();
                                                        let _ = write_env_var_to_file(".env", "VOICE_REFERENCE_TEXT", "");
                                                        std::env::set_var("VOICE_REFERENCE_TEXT", "");
                                                    }
                                                }
                                            });
                                        });
                                        ui.add_space(2.0);
                                        let ref_input_id = egui::Id::NULL.with("voice_reference_text_input");
                                        let ref_resp = render_accessible_input(
                                            ui,
                                            ref_input_id,
                                            &mut self.voice_reference_text_input,
                                            "Texte transcrit ou prononcé dans votre enregistrement de référence...",
                                            true,
                                            Some(ui.available_width()),
                                            Some(46.0),
                                            false,
                                            theme,
                                        );
                                        if ref_resp.changed() {
                                            let clean = self.voice_reference_text_input.trim();
                                            let _ = write_env_var_to_file(".env", "VOICE_REFERENCE_TEXT", clean);
                                            std::env::set_var("VOICE_REFERENCE_TEXT", clean);
                                        }

                                        if let Some(ref fb) = self.voice_status_feedback {
                                            ui.add_space(4.0);
                                            ui.label(egui::RichText::new(fb).size(10.5).color(theme.accent));
                                        }

                                        ui.add_space(6.0);
                                        ui.separator();
                                        ui.add_space(4.0);

                                        // Informations moteur natif et serveur optionnel
                                        ui.horizontal(|ui| {
                                            ui.label(egui::RichText::new("Moteur natif Sherpa actif (temps réel local). URL serveur de repli (optionnel) :").size(10.5).color(theme.text_secondary));
                                        });
                                        render_accessible_input(
                                            ui,
                                            egui::Id::NULL.with("voice_clone_url_input"),
                                            &mut self.voice_clone_url_input,
                                            "http://127.0.0.1:8020/tts",
                                            false,
                                            Some(ui.available_width()),
                                            None,
                                            false,
                                            theme,
                                        );

                                        let enter_in_clone_fields = ctx.memory(|m| {
                                            m.has_focus(egui::Id::NULL.with("voice_clone_url_input"))
                                        }) && ctx.input(|i| {
                                            i.events.iter().any(|ev| match ev {
                                                egui::Event::Key { key: egui::Key::Enter, pressed: true, .. } => true,
                                                egui::Event::Text(t) if t == "\n" || t == "\r" => true,
                                                _ => false,
                                            })
                                        });

                                        ui.add_space(4.0);
                                        ui.horizontal(|ui| {
                                            let save_clone_btn = ui.button(egui::RichText::new("💾 Enregistrer et Activer la voix clonée").strong().color(theme.accent));
                                            let save_clone_id = ui.make_persistent_id("dwell_voice_save_clone_btn");
                                            let save_btn_hit = make_widget_accessible_and_activated(ui, &save_clone_btn, theme.accent);
                                            let save_dwell = apply_dwell_to_response(ui, &mut self.dwell_state, save_clone_id, &save_clone_btn);
                                            if save_btn_hit || save_dwell || enter_in_clone_fields {
                                                let url = self.voice_clone_url_input.trim();
                                                let wav = self.voice_reference_wav_input.trim();
                                                let text = self.voice_reference_text_input.trim();

                                                let _ = write_env_var_to_file(".env", "VOICE_CLONE_URL", url);
                                                std::env::set_var("VOICE_CLONE_URL", url);
                                                let _ = write_env_var_to_file(".env", "VOICE_CLONE_GENDER", &self.voice_clone_gender);
                                                std::env::set_var("VOICE_CLONE_GENDER", &self.voice_clone_gender);
                                                if !wav.is_empty() {
                                                    let _ = write_env_var_to_file(".env", "VOICE_REFERENCE_WAV", wav);
                                                    std::env::set_var("VOICE_REFERENCE_WAV", wav);
                                                }
                                                if !text.is_empty() {
                                                    let _ = write_env_var_to_file(".env", "VOICE_REFERENCE_TEXT", text);
                                                    std::env::set_var("VOICE_REFERENCE_TEXT", text);
                                                }

                                                self.set_voice_gender_silent(VoiceProfile::Clone);
                                                self.voice_status_feedback = Some("Paramètres enregistrés et profil vocal cloné activé.".to_string());
                                                play_earcon(Earcon::Success);
                                            }
                                        });
                                    });

                                ui.add_space(6.0);
                                ui.horizontal(|ui| {
                                    let close_voice_btn = ui.button(t!("ui.btn_close"));
                                    let close_voice_id = ui.make_persistent_id("dwell_close_voice_tab");
                                    let is_close_key = make_widget_accessible_and_activated(ui, &close_voice_btn, theme.accent);
                                    let is_close_dwell = apply_dwell_to_response(ui, &mut self.dwell_state, close_voice_id, &close_voice_btn);
                                    if is_close_key || is_close_dwell {
                                        self.show_api_key_modal = false;
                                    }
                                });
                            }
                            SettingsTab::Theme => {
                                // 1. Sélection du thème et des contrastes
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

                                // 2. Taille de police & Facteur de zoom
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
                                    let is_minus_key = make_widget_accessible_and_activated(ui, &resp_minus, theme.accent);
                                    if apply_dwell_to_response(ui, &mut self.dwell_state, id_minus, &resp_minus) || is_minus_key {
                                        current_zoom = ((current_zoom - 0.10).max(0.70) * 100.0).round() / 100.0;
                                    }

                                    ui.add(egui::Label::new(egui::RichText::new(format!("{:.0} %", current_zoom * 100.0)).size(13.0).strong().color(theme.accent)));

                                    let btn_plus = egui::Button::new(egui::RichText::new("A +").size(13.0).strong().color(egui::Color32::WHITE)).min_size(egui::vec2(52.0, 30.0)).corner_radius(egui::CornerRadius::same(6));
                                    let resp_plus = ui.add(btn_plus).on_hover_text(t!("ui.font_plus_tooltip"));
                                    let id_plus = ui.make_persistent_id("dwell_zoom_plus");
                                    let is_plus_key = make_widget_accessible_and_activated(ui, &resp_plus, theme.accent);
                                    if apply_dwell_to_response(ui, &mut self.dwell_state, id_plus, &resp_plus) || is_plus_key {
                                        current_zoom = ((current_zoom + 0.10).min(2.50) * 100.0).round() / 100.0;
                                    }

                                    let btn_reset = egui::Button::new(egui::RichText::new("100 %").size(12.0).color(theme.text_secondary)).min_size(egui::vec2(52.0, 30.0)).corner_radius(egui::CornerRadius::same(6));
                                    let resp_reset = ui.add(btn_reset).on_hover_text(t!("ui.font_reset_tooltip"));
                                    let id_reset = ui.make_persistent_id("dwell_zoom_reset");
                                    let is_reset_key = make_widget_accessible_and_activated(ui, &resp_reset, theme.accent);
                                    if apply_dwell_to_response(ui, &mut self.dwell_state, id_reset, &resp_reset) || is_reset_key {
                                        current_zoom = 1.0;
                                    }
                                });
                                if (current_zoom - prev_zoom).abs() > 0.001 {
                                    self.set_zoom_factor(current_zoom);
                                    ctx.set_zoom_factor(self.zoom_factor);
                                }

                                ui.add_space(6.0);
                                ui.horizontal(|ui| {
                                    let close_theme_btn = ui.button(t!("ui.btn_close"));
                                    if make_widget_accessible_and_activated(ui, &close_theme_btn, theme.accent) {
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
                                        let is_lang_key = make_widget_accessible_and_activated(ui, &btn, theme.accent);
                                        if apply_dwell_to_response(ui, &mut self.dwell_state, btn_id, &btn) || is_lang_key {
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
                                    let close_lang_btn = ui.button(t!("ui.btn_close"));
                                    if make_widget_accessible_and_activated(ui, &close_lang_btn, theme.accent) {
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
                                            let sc_hint = t!("ui.macros_shortcut_hint").to_string();
                                            render_accessible_input(
                                                ui,
                                                egui::Id::NULL.with("macro_shortcut_input"),
                                                &mut self.macro_shortcut_input,
                                                &sc_hint,
                                                false,
                                                Some(120.0),
                                                None,
                                                false,
                                                theme,
                                            );
                                        });
                                        ui.add_space(3.0);

                                        ui.horizontal(|ui| {
                                            ui.label(egui::RichText::new(t!("ui.macros_text_label")).size(11.0).color(egui::Color32::WHITE));
                                            let tx_hint = t!("ui.macros_text_hint").to_string();
                                            render_accessible_input(
                                                ui,
                                                egui::Id::NULL.with("macro_text_input"),
                                                &mut self.macro_text_input,
                                                &tx_hint,
                                                false,
                                                Some(260.0),
                                                None,
                                                false,
                                                theme,
                                            );
                                        });
                                        ui.add_space(4.0);

                                        if let Some(ref err) = self.macro_error {
                                            ui.label(egui::RichText::new(err).color(egui::Color32::from_rgb(255, 110, 110)).size(10.5));
                                            ui.add_space(2.0);
                                        }

                                        let enter_in_macro_input = ctx.memory(|m| {
                                            m.has_focus(egui::Id::NULL.with("macro_shortcut_input"))
                                                || m.has_focus(egui::Id::NULL.with("macro_text_input"))
                                        }) && ctx.input(|i| {
                                            i.events.iter().any(|ev| match ev {
                                                egui::Event::Key { key: egui::Key::Enter, pressed: true, .. } => true,
                                                egui::Event::Text(t) if t == "\n" || t == "\r" => true,
                                                _ => false,
                                            })
                                        });

                                        ui.horizontal(|ui| {
                                            let add_btn = ui.button(egui::RichText::new(t!("ui.macros_add_btn")).strong().color(theme.accent));
                                            let add_btn_hit = make_widget_accessible_and_activated(ui, &add_btn, theme.accent);
                                            if add_btn_hit || enter_in_macro_input {
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
                                                    let del_macro_btn = ui.small_button("🗑").on_hover_text(t!("ui.macros_delete_tooltip"));
                                                    if make_widget_accessible_and_activated(ui, &del_macro_btn, theme.accent) {
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
                                        let ref_btn = ui.button(t!("ui.devices_refresh_btn")).on_hover_text(t!("ui.devices_refresh_btn"));
                                        if make_widget_accessible_and_activated(ui, &ref_btn, theme.accent) {
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
                                    let close_dev_btn = ui.button(t!("ui.btn_close"));
                                    if make_widget_accessible_and_activated(ui, &close_dev_btn, theme.accent) {
                                        self.show_api_key_modal = false;
                                    }
                                });
                            }
                        }
                    });
            });
    }
}