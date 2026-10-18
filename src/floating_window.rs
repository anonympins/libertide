use eframe::egui;
use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum FormFieldType {
    #[serde(alias = "input")]
    Text {
        #[serde(default)]
        placeholder: String,
        #[serde(default)]
        value: String,
    },
    #[serde(alias = "slider", alias = "gauge", alias = "number")]
    NumberSlider {
        #[serde(default)]
        min: f64,
        #[serde(default = "default_max_slider")]
        max: f64,
        #[serde(default)]
        step: Option<f64>,
        #[serde(default)]
        value: f64,
        #[serde(default)]
        unit: String,
    },
    #[serde(alias = "dropdown", alias = "combobox")]
    Select {
        options: Vec<String>,
        #[serde(default)]
        selected: String,
    },
    #[serde(alias = "bool", alias = "check")]
    Checkbox {
        #[serde(default)]
        checked: bool,
    },
    #[serde(alias = "choice")]
    Radio {
        options: Vec<String>,
        #[serde(default)]
        selected: String,
    },
}

fn default_max_slider() -> f64 {
    100.0
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct FormField {
    #[serde(default)]
    pub id: String,
    pub label: String,
    #[serde(flatten)]
    pub field_type: FormFieldType,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum FloatingContentType {
    Table {
        headers: Vec<String>,
        rows: Vec<Vec<String>>,
    },
    Stats {
        items: Vec<(String, String)>,
    },
    Markdown {
        text: String,
    },
    Form {
        fields: Vec<FormField>,
        #[serde(default = "default_submit_label")]
        submit_label: String,
        #[serde(default)]
        prompt_prefix: Option<String>,
    },
}

fn default_submit_label() -> String {
    "🔍 Lancer la recherche".to_string()
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct FloatingCard {
    pub id: String,
    pub title: String,
    pub content: FloatingContentType,
    pub open: bool,
}

impl FloatingCard {
    pub fn new(id: impl Into<String>, title: impl Into<String>, content: FloatingContentType) -> Self {
        Self {
            id: id.into(),
            title: title.into(),
            content,
            open: true,
        }
    }

    pub fn from_action_data(
        id: Option<String>,
        title: String,
        card_type: &str,
        data: &serde_json::Value,
    ) -> Self {
        let clean_id = id.unwrap_or_else(|| {
            format!("card_{}", std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap_or_default()
                .as_millis())
        });

        let content = match card_type.to_lowercase().as_str() {
            "table" => {
                let mut headers = Vec::new();
                let mut rows = Vec::new();

                if let Some(h_arr) = data.get("headers").and_then(|v| v.as_array()) {
                    headers = h_arr.iter().filter_map(|v| v.as_str().map(String::from)).collect();
                }
                if let Some(r_arr) = data.get("rows").and_then(|v| v.as_array()) {
                    for row_val in r_arr {
                        if let Some(cols) = row_val.as_array() {
                            rows.push(cols.iter().map(|c| c.as_str().unwrap_or(&c.to_string()).to_string()).collect());
                        }
                    }
                }
                // Si data est une liste d'objets [{"colA": "valA", "colB": "valB"}]
                if headers.is_empty() && data.is_array() {
                    if let Some(first_obj) = data.as_array().and_then(|arr| arr.first()).and_then(|v| v.as_object()) {
                        headers = first_obj.keys().cloned().collect();
                        for item in data.as_array().unwrap() {
                            if let Some(obj) = item.as_object() {
                                let row = headers.iter().map(|k| {
                                    obj.get(k).map(|v| v.as_str().unwrap_or(&v.to_string()).to_string()).unwrap_or_default()
                                }).collect();
                                rows.push(row);
                            }
                        }
                    }
                }
                FloatingContentType::Table { headers, rows }
            }
            "stats" | "key_values" => {
                let mut items = Vec::new();
                if let Some(obj) = data.as_object() {
                    for (k, v) in obj {
                        let val_str = v.as_str().map(String::from).unwrap_or_else(|| v.to_string());
                        items.push((k.clone(), val_str));
                    }
                } else if let Some(arr) = data.get("items").and_then(|v| v.as_array()) {
                    for item in arr {
                        if let Some(pair) = item.as_array() {
                            if pair.len() >= 2 {
                                items.push((
                                    pair[0].as_str().unwrap_or("").to_string(),
                                    pair[1].as_str().unwrap_or("").to_string(),
                                ));
                            }
                        }
                    }
                }
                FloatingContentType::Stats { items }
            }
            "form" | "search_form" | "filter" | "filters" | "search" => {
                if let Ok(form_data) = serde_json::from_value::<FloatingContentType>(data.clone()) {
                    form_data
                } else {
                    let mut fields = Vec::new();
                    let submit_label = data.get("submit_label")
                        .and_then(|v| v.as_str())
                        .unwrap_or("🔍 Lancer la recherche")
                        .to_string();
                    let prompt_prefix = data.get("prompt_prefix")
                        .and_then(|v| v.as_str())
                        .map(|s| s.to_string());

                    let raw_fields = data.get("fields")
                        .and_then(|v| v.as_array())
                        .or_else(|| data.as_array());

                    if let Some(arr) = raw_fields {
                        for (idx, item) in arr.iter().enumerate() {
                            if let Ok(field) = serde_json::from_value::<FormField>(item.clone()) {
                                fields.push(field);
                            } else if let Some(obj) = item.as_object() {
                                let id = obj.get("id")
                                    .and_then(|v| v.as_str())
                                    .unwrap_or(&format!("field_{idx}"))
                                    .to_string();
                                let label = obj.get("label")
                                    .and_then(|v| v.as_str())
                                    .unwrap_or(&id)
                                    .to_string();
                                let ftype = obj.get("type")
                                    .and_then(|v| v.as_str())
                                    .unwrap_or("text")
                                    .to_lowercase();

                                let field_type = match ftype.as_str() {
                                    "slider" | "number" | "gauge" | "number_slider" => {
                                        let min = obj.get("min").and_then(|v| v.as_f64()).unwrap_or(0.0);
                                        let max = obj.get("max").and_then(|v| v.as_f64()).unwrap_or(100.0);
                                        let step = obj.get("step").and_then(|v| v.as_f64());
                                        let val = obj.get("value").and_then(|v| v.as_f64()).unwrap_or(min);
                                        let unit = obj.get("unit")
                                            .or_else(|| obj.get("suffix"))
                                            .and_then(|v| v.as_str())
                                            .unwrap_or("")
                                            .to_string();
                                        FormFieldType::NumberSlider { min, max, step, value: val, unit }
                                    }
                                    "select" | "dropdown" | "combobox" => {
                                        let options: Vec<String> = obj.get("options")
                                            .and_then(|v| v.as_array())
                                            .map(|arr| arr.iter().filter_map(|s| s.as_str().map(String::from)).collect())
                                            .unwrap_or_default();
                                        let selected = obj.get("selected")
                                            .or_else(|| obj.get("value"))
                                            .and_then(|v| v.as_str())
                                            .unwrap_or_else(|| options.first().map(|s| s.as_str()).unwrap_or(""))
                                            .to_string();
                                        FormFieldType::Select { options, selected }
                                    }
                                    "checkbox" | "check" | "bool" => {
                                        let checked = obj.get("checked")
                                            .or_else(|| obj.get("value"))
                                            .and_then(|v| v.as_bool())
                                            .unwrap_or(false);
                                        FormFieldType::Checkbox { checked }
                                    }
                                    "radio" | "choice" => {
                                        let options: Vec<String> = obj.get("options")
                                            .and_then(|v| v.as_array())
                                            .map(|arr| arr.iter().filter_map(|s| s.as_str().map(String::from)).collect())
                                            .unwrap_or_default();
                                        let selected = obj.get("selected")
                                            .or_else(|| obj.get("value"))
                                            .and_then(|v| v.as_str())
                                            .unwrap_or_else(|| options.first().map(|s| s.as_str()).unwrap_or(""))
                                            .to_string();
                                        FormFieldType::Radio { options, selected }
                                    }
                                    _ => {
                                        let placeholder = obj.get("placeholder").and_then(|v| v.as_str()).unwrap_or("").to_string();
                                        let value = obj.get("value").and_then(|v| v.as_str()).unwrap_or("").to_string();
                                        FormFieldType::Text { placeholder, value }
                                    }
                                };
                                fields.push(FormField { id, label, field_type });
                            }
                        }
                    }
                    FloatingContentType::Form { fields, submit_label, prompt_prefix }
                }
            }
            _ => {
                let text = if let Some(s) = data.as_str() {
                    s.to_string()
                } else if let Some(t) = data.get("text").and_then(|v| v.as_str()) {
                    t.to_string()
                } else {
                    data.to_string()
                };
                FloatingContentType::Markdown { text }
            }
        };

        Self::new(clean_id, title, content)
    }

    pub fn show(&mut self, ctx: &egui::Context) -> Option<String> {
        if !self.open {
            return None;
        }

        let viewport_id = egui::ViewportId::from_hash_of(&self.id);
        let icon_prefix = match &self.content {
            FloatingContentType::Form { .. } => "📝",
            FloatingContentType::Table { .. } => "📊",
            FloatingContentType::Stats { .. } => "📈",
            FloatingContentType::Markdown { .. } => "📄",
        };
        let viewport_title = format!("{icon_prefix} {}", self.title);

        let default_size = match &self.content {
            FloatingContentType::Form { .. } => [460.0, 390.0],
            _ => [440.0, 270.0],
        };

        let builder = egui::ViewportBuilder::default()
            .with_title(&viewport_title)
            .with_transparent(true)
            .with_decorations(false)
            .with_always_on_top()
            .with_inner_size(default_size)
            .with_min_inner_size([280.0, 160.0])
            .with_resizable(true);

        let mut submitted_prompt = None;

        ctx.show_viewport_immediate(viewport_id, builder, |vp_ctx, _class| {
            if vp_ctx.input(|i| i.viewport().close_requested()) {
                self.open = false;
            }

            egui::CentralPanel::default()
                .frame(
                    egui::Frame::new()
                        .fill(egui::Color32::from_rgba_unmultiplied(14, 18, 24, 230))
                        .stroke(egui::Stroke::new(1.2, egui::Color32::from_rgba_unmultiplied(0, 195, 255, 140)))
                        .corner_radius(egui::CornerRadius::same(12))
                        .inner_margin(egui::Margin::symmetric(12, 10)),
                )
                .show(vp_ctx, |ui| {
                    // Barre d'en-tête volante : déplaçable par clic-glissé avec bouton fermer
                    let header_resp = ui.horizontal(|ui| {
                        let title_resp = ui.add(
                            egui::Label::new(
                                egui::RichText::new(&viewport_title)
                                    .strong()
                                    .size(13.0)
                                    .color(egui::Color32::from_rgb(0, 215, 255)),
                            )
                            .sense(egui::Sense::drag()),
                        );

                        if title_resp.dragged() {
                            ui.ctx().send_viewport_cmd(egui::ViewportCommand::StartDrag);
                        }

                        ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                            if ui.button(
                                egui::RichText::new("✕")
                                    .size(12.5)
                                    .color(egui::Color32::from_rgb(255, 130, 130)),
                            ).on_hover_text("Fermer cette fenêtre volante").clicked() {
                                self.open = false;
                            }
                        });
                    });

                    if header_resp.response.dragged() {
                        ui.ctx().send_viewport_cmd(egui::ViewportCommand::StartDrag);
                    }

                    ui.add_space(4.0);
                    ui.separator();
                    ui.add_space(4.0);

                    let card_id = self.id.clone();
                    let card_title = self.title.clone();
                    let mut close_window = false;

                    egui::ScrollArea::both()
                        .auto_shrink([false, false])
                        .show(ui, |ui| {
                            match &mut self.content {
                                FloatingContentType::Table { headers, rows } => {
                                    if headers.is_empty() && rows.is_empty() {
                                        ui.label(egui::RichText::new("Tableau vide").italics().color(egui::Color32::GRAY));
                                        return;
                                    }
                                    egui::Grid::new(format!("grid_{card_id}"))
                                        .striped(true)
                                        .spacing([14.0, 6.0])
                                        .show(ui, |ui| {
                                            for h in headers.iter() {
                                                ui.label(egui::RichText::new(h).strong().color(egui::Color32::from_rgb(0, 210, 255)));
                                            }
                                            ui.end_row();

                                            for row in rows.iter() {
                                                for cell in row.iter() {
                                                    ui.label(egui::RichText::new(cell).color(egui::Color32::from_rgb(230, 235, 245)));
                                                }
                                                ui.end_row();
                                            }
                                        });
                                }
                                FloatingContentType::Stats { items } => {
                                    ui.vertical(|ui| {
                                        for (k, v) in items.iter() {
                                            ui.horizontal(|ui| {
                                                ui.label(egui::RichText::new(format!("{k} :")).color(egui::Color32::from_rgb(170, 190, 215)));
                                                ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                                                    ui.label(
                                                        egui::RichText::new(v)
                                                            .strong()
                                                            .color(egui::Color32::from_rgb(255, 215, 110)),
                                                    );
                                                });
                                            });
                                            ui.separator();
                                        }
                                    });
                                }
                                FloatingContentType::Markdown { text } => {
                                    for line in text.lines() {
                                        if line.starts_with('#') {
                                            ui.heading(egui::RichText::new(line.trim_start_matches('#').trim()).color(egui::Color32::from_rgb(0, 200, 255)));
                                        } else {
                                            ui.label(egui::RichText::new(line).color(egui::Color32::WHITE));
                                        }
                                    }
                                }
                                FloatingContentType::Form { fields, submit_label, prompt_prefix } => {
                                    let mut form_submitted = false;

                                    ui.vertical(|ui| {
                                        for field in fields.iter_mut() {
                                            ui.add_space(3.0);
                                            match &mut field.field_type {
                                                FormFieldType::Checkbox { checked } => {
                                                    let resp = ui.checkbox(
                                                        checked,
                                                        egui::RichText::new(&field.label)
                                                            .size(12.5)
                                                            .color(egui::Color32::from_rgb(230, 240, 255)),
                                                    );
                                                    resp.on_hover_text(format!("Activer/Désactiver : {}", field.label));
                                                }
                                                FormFieldType::Text { placeholder, value } => {
                                                    ui.label(
                                                        egui::RichText::new(&field.label)
                                                            .size(11.5)
                                                            .color(egui::Color32::from_rgb(0, 200, 255))
                                                            .strong(),
                                                    );
                                                    ui.add_space(1.0);
                                                    let edit = egui::TextEdit::singleline(value)
                                                        .hint_text(placeholder.as_str())
                                                        .desired_width(ui.available_width());
                                                    ui.add(edit);
                                                }
                                                FormFieldType::NumberSlider { min, max, step, value, unit } => {
                                                    ui.horizontal(|ui| {
                                                        ui.label(
                                                            egui::RichText::new(&field.label)
                                                                .size(11.5)
                                                                .color(egui::Color32::from_rgb(0, 200, 255))
                                                                .strong(),
                                                        );
                                                        ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                                                            let display_val = if unit.is_empty() {
                                                                format!("{:.0}", *value)
                                                            } else {
                                                                format!("{:.0} {}", *value, unit)
                                                            };
                                                            ui.label(
                                                                egui::RichText::new(display_val)
                                                                    .size(12.0)
                                                                    .color(egui::Color32::from_rgb(255, 215, 110))
                                                                    .strong(),
                                                            );
                                                        });
                                                    });
                                                    ui.add_space(1.0);
                                                    let mut slider = egui::Slider::new(value, *min..=*max)
                                                        .show_value(false);
                                                    if let Some(s) = step {
                                                        slider = slider.step_by(*s);
                                                    }
                                                    ui.add_sized([ui.available_width(), 20.0], slider);
                                                }
                                                FormFieldType::Select { options, selected } => {
                                                    ui.label(
                                                        egui::RichText::new(&field.label)
                                                            .size(11.5)
                                                            .color(egui::Color32::from_rgb(0, 200, 255))
                                                            .strong(),
                                                    );
                                                    ui.add_space(1.0);
                                                    let combo_id = format!("combo_{card_id}_{}", field.id);
                                                    egui::ComboBox::from_id_salt(combo_id)
                                                        .selected_text(selected.as_str())
                                                        .width(ui.available_width().max(120.0))
                                                        .show_ui(ui, |ui| {
                                                            for opt in options {
                                                                ui.selectable_value(selected, opt.clone(), opt.as_str());
                                                            }
                                                        });
                                                }
                                                FormFieldType::Radio { options, selected } => {
                                                    ui.label(
                                                        egui::RichText::new(&field.label)
                                                            .size(11.5)
                                                            .color(egui::Color32::from_rgb(0, 200, 255))
                                                            .strong(),
                                                    );
                                                    ui.add_space(2.0);
                                                    ui.horizontal_wrapped(|ui| {
                                                        ui.spacing_mut().item_spacing = egui::vec2(8.0, 4.0);
                                                        for opt in options {
                                                            ui.radio_value(selected, opt.clone(), opt.as_str());
                                                        }
                                                    });
                                                }
                                            }
                                            ui.add_space(4.0);
                                        }

                                        ui.add_space(6.0);
                                        ui.separator();
                                        ui.add_space(6.0);

                                        ui.horizontal(|ui| {
                                            let submit_btn = egui::Button::new(
                                                egui::RichText::new(submit_label.as_str())
                                                    .strong()
                                                    .size(12.5)
                                                    .color(egui::Color32::from_rgb(18, 20, 26)),
                                            )
                                            .fill(egui::Color32::from_rgb(0, 215, 255))
                                            .corner_radius(egui::CornerRadius::same(6))
                                            .min_size(egui::vec2(130.0, 26.0));

                                            if ui.add(submit_btn).on_hover_text("Valider les critères et lancer la requête").clicked() {
                                                form_submitted = true;
                                            }

                                            if ui.button("Annuler").clicked() {
                                                close_window = true;
                                            }
                                        });
                                    });

                                    if form_submitted {
                                        let mut prompt_lines = Vec::new();
                                        if let Some(prefix) = prompt_prefix {
                                            if !prefix.trim().is_empty() {
                                                prompt_lines.push(prefix.trim().to_string());
                                            }
                                        } else {
                                            prompt_lines.push(format!("Consigne de recherche ({card_title}) :"));
                                        }

                                        for field in fields.iter() {
                                            match &field.field_type {
                                                FormFieldType::Text { value, .. } => {
                                                    if !value.trim().is_empty() {
                                                        prompt_lines.push(format!("- {} : {}", field.label, value.trim()));
                                                    }
                                                }
                                                FormFieldType::NumberSlider { value, unit, .. } => {
                                                    if unit.is_empty() {
                                                        prompt_lines.push(format!("- {} : {:.0}", field.label, value));
                                                    } else {
                                                        prompt_lines.push(format!("- {} : {:.0} {}", field.label, value, unit));
                                                    }
                                                }
                                                FormFieldType::Select { selected, .. } => {
                                                    if !selected.trim().is_empty() {
                                                        prompt_lines.push(format!("- {} : {}", field.label, selected.trim()));
                                                    }
                                                }
                                                FormFieldType::Checkbox { checked } => {
                                                    prompt_lines.push(format!("- {} : {}", field.label, if *checked { "Oui" } else { "Non" }));
                                                }
                                                FormFieldType::Radio { selected, .. } => {
                                                    if !selected.trim().is_empty() {
                                                        prompt_lines.push(format!("- {} : {}", field.label, selected.trim()));
                                                    }
                                                }
                                            }
                                        }
                                        close_window = true;
                                        submitted_prompt = Some(prompt_lines.join("\n"));
                                    }
                                }
                            }
                        });

                    if close_window {
                        self.open = false;
                    }
                });
        });

        submitted_prompt
    }
}