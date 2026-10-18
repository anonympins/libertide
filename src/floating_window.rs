use eframe::egui;
use eframe::egui::text::LayoutJob;
use eframe::egui::TextFormat;
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
            FloatingContentType::Markdown { .. } => [500.0, 360.0],
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
                                    render_markdown(ui, text, &card_id);
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

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum InlineStyle {
    Normal,
    Bold,
    Italic,
    BoldItalic,
    Code,
    Strike,
    Link,
}

struct InlineSpan<'a> {
    text: &'a str,
    style: InlineStyle,
}

fn parse_inline_markdown(line: &str) -> Vec<InlineSpan<'_>> {
    let mut spans = Vec::new();
    let mut i = 0;
    let bytes = line.as_bytes();
    let len = bytes.len();
    let mut plain_start = 0;

    while i < len {
        if bytes[i] == b'`' {
            if let Some(end) = line[i + 1..].find('`') {
                if i > plain_start {
                    spans.push(InlineSpan {
                        text: &line[plain_start..i],
                        style: InlineStyle::Normal,
                    });
                }
                spans.push(InlineSpan {
                    text: &line[i + 1..i + 1 + end],
                    style: InlineStyle::Code,
                });
                i += 1 + end + 1;
                plain_start = i;
                continue;
            }
        } else if line[i..].starts_with("***") {
            if let Some(end) = line[i + 3..].find("***") {
                if i > plain_start {
                    spans.push(InlineSpan {
                        text: &line[plain_start..i],
                        style: InlineStyle::Normal,
                    });
                }
                spans.push(InlineSpan {
                    text: &line[i + 3..i + 3 + end],
                    style: InlineStyle::BoldItalic,
                });
                i += 3 + end + 3;
                plain_start = i;
                continue;
            }
        } else if line[i..].starts_with("**") {
            if let Some(end) = line[i + 2..].find("**") {
                if i > plain_start {
                    spans.push(InlineSpan {
                        text: &line[plain_start..i],
                        style: InlineStyle::Normal,
                    });
                }
                spans.push(InlineSpan {
                    text: &line[i + 2..i + 2 + end],
                    style: InlineStyle::Bold,
                });
                i += 2 + end + 2;
                plain_start = i;
                continue;
            }
        } else if line[i..].starts_with("~~") {
            if let Some(end) = line[i + 2..].find("~~") {
                if i > plain_start {
                    spans.push(InlineSpan {
                        text: &line[plain_start..i],
                        style: InlineStyle::Normal,
                    });
                }
                spans.push(InlineSpan {
                    text: &line[i + 2..i + 2 + end],
                    style: InlineStyle::Strike,
                });
                i += 2 + end + 2;
                plain_start = i;
                continue;
            }
        } else if bytes[i] == b'*' && !line[i..].starts_with("**") {
            if let Some(end) = line[i + 1..].find('*') {
                if !line[i + 1 + end..].starts_with("**") {
                    if i > plain_start {
                        spans.push(InlineSpan {
                            text: &line[plain_start..i],
                            style: InlineStyle::Normal,
                        });
                    }
                    spans.push(InlineSpan {
                        text: &line[i + 1..i + 1 + end],
                        style: InlineStyle::Italic,
                    });
                    i += 1 + end + 1;
                    plain_start = i;
                    continue;
                }
            }
        } else if bytes[i] == b'[' {
            if let Some(bracket_end) = line[i + 1..].find(']') {
                let after_bracket = i + 1 + bracket_end + 1;
                if after_bracket < len && bytes[after_bracket] == b'(' {
                    if let Some(paren_end) = line[after_bracket + 1..].find(')') {
                        if i > plain_start {
                            spans.push(InlineSpan {
                                text: &line[plain_start..i],
                                style: InlineStyle::Normal,
                            });
                        }
                        spans.push(InlineSpan {
                            text: &line[i + 1..i + 1 + bracket_end],
                            style: InlineStyle::Link,
                        });
                        i = after_bracket + 1 + paren_end + 1;
                        plain_start = i;
                        continue;
                    }
                }
            }
        }

        if let Some(ch) = line[i..].chars().next() {
            i += ch.len_utf8();
        } else {
            break;
        }
    }

    if plain_start < len {
        spans.push(InlineSpan {
            text: &line[plain_start..len],
            style: InlineStyle::Normal,
        });
    }

    spans
}

fn create_layout_job(
    line: &str,
    base_size: f32,
    max_width: f32,
    force_italic: bool,
    base_color: egui::Color32,
) -> LayoutJob {
    let mut job = LayoutJob::default();
    job.wrap.max_width = max_width;

    let spans = parse_inline_markdown(line);
    for span in spans {
        let mut fmt = TextFormat::default();
        match span.style {
            InlineStyle::Normal => {
                fmt.font_id = egui::FontId::proportional(base_size);
                fmt.color = base_color;
                fmt.italics = force_italic;
            }
            InlineStyle::Bold => {
                fmt.font_id = egui::FontId::proportional(base_size + 0.2);
                fmt.color = egui::Color32::WHITE;
                fmt.italics = force_italic;
            }
            InlineStyle::Italic => {
                fmt.font_id = egui::FontId::proportional(base_size);
                fmt.color = egui::Color32::from_rgb(205, 225, 250);
                fmt.italics = true;
            }
            InlineStyle::BoldItalic => {
                fmt.font_id = egui::FontId::proportional(base_size + 0.2);
                fmt.color = egui::Color32::WHITE;
                fmt.italics = true;
            }
            InlineStyle::Code => {
                fmt.font_id = egui::FontId::monospace((base_size - 1.0).max(10.5));
                fmt.color = egui::Color32::from_rgb(255, 215, 130);
                fmt.background = egui::Color32::from_rgba_unmultiplied(255, 255, 255, 22);
            }
            InlineStyle::Strike => {
                fmt.font_id = egui::FontId::proportional(base_size);
                fmt.color = egui::Color32::from_rgb(140, 155, 175);
                fmt.strikethrough = egui::Stroke::new(1.0, egui::Color32::from_rgb(140, 155, 175));
            }
            InlineStyle::Link => {
                fmt.font_id = egui::FontId::proportional(base_size);
                fmt.color = egui::Color32::from_rgb(0, 215, 255);
                fmt.underline = egui::Stroke::new(1.0, egui::Color32::from_rgb(0, 215, 255));
            }
        }
        job.append(span.text, 0.0, fmt);
    }

    job
}

fn parse_numbered_list(s: &str) -> Option<(&str, &str)> {
    let bytes = s.as_bytes();
    let mut num_end = 0;
    while num_end < bytes.len() && bytes[num_end].is_ascii_digit() {
        num_end += 1;
    }
    if num_end > 0 && num_end + 1 < bytes.len() && bytes[num_end] == b'.' && bytes[num_end + 1] == b' ' {
        let num_str = &s[..num_end + 1];
        let content = s[num_end + 2..].trim();
        return Some((num_str, content));
    }
    None
}

pub fn render_markdown(ui: &mut egui::Ui, text: &str, card_id: &str) {
    let mut lines = text.lines().peekable();
    let mut block_idx = 0usize;

    while let Some(line) = lines.next() {
        let trimmed = line.trim();

        if trimmed.starts_with("```") {
            let lang = trimmed.trim_start_matches('`').trim();
            let mut code_lines = Vec::new();
            while let Some(&next_line) = lines.peek() {
                if next_line.trim().starts_with("```") {
                    lines.next();
                    break;
                }
                code_lines.push(next_line);
                lines.next();
            }
            ui.add_space(3.0);
            let full_code = code_lines.join("\n");
            egui::Frame::new()
                .fill(egui::Color32::from_rgb(10, 13, 18))
                .stroke(egui::Stroke::new(1.0, egui::Color32::from_rgba_unmultiplied(0, 180, 255, 60)))
                .corner_radius(egui::CornerRadius::same(6))
                .inner_margin(egui::Margin::symmetric(10, 8))
                .show(ui, |ui| {
                    ui.horizontal(|ui| {
                        let badge = if lang.is_empty() { "code" } else { lang };
                        ui.label(
                            egui::RichText::new(badge)
                                .size(10.5)
                                .color(egui::Color32::from_rgb(0, 200, 255))
                                .strong(),
                        );
                        ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                            if ui.small_button("📋 Copier").on_hover_text("Copier ce code").clicked() {
                                ui.ctx().copy_text(full_code.clone());
                            }
                        });
                    });
                    ui.add_space(2.0);
                    ui.separator();
                    ui.add_space(2.0);

                    let mut code_job = LayoutJob::default();
                    code_job.wrap.max_width = ui.available_width().max(40.0);
                    let mut fmt = TextFormat::default();
                    fmt.font_id = egui::FontId::monospace(11.5);
                    fmt.color = egui::Color32::from_rgb(215, 230, 245);
                    code_job.append(&full_code, 0.0, fmt);
                    ui.label(code_job);
                });
            ui.add_space(3.0);
            continue;
        }

        if trimmed == "---" || trimmed == "***" || trimmed == "___" {
            ui.add_space(4.0);
            ui.separator();
            ui.add_space(4.0);
            continue;
        }

        if trimmed.starts_with('#') {
            let hashes = trimmed.chars().take_while(|&c| c == '#').count();
            if hashes > 0 && trimmed.chars().nth(hashes) == Some(' ') {
                let content = trimmed[hashes + 1..].trim();
                let (font_size, color, extra_space) = match hashes {
                    1 => (16.0, egui::Color32::from_rgb(0, 220, 255), 6.0),
                    2 => (14.0, egui::Color32::from_rgb(70, 215, 255), 4.0),
                    3 => (13.0, egui::Color32::from_rgb(130, 210, 255), 3.0),
                    _ => (12.0, egui::Color32::from_rgb(180, 225, 255), 2.0),
                };

                ui.add_space(extra_space);
                let avail = ui.available_width().max(40.0);
                let job = create_layout_job(content, font_size, avail, false, color);
                ui.label(job);

                if hashes == 1 {
                    ui.add_space(2.0);
                    ui.painter().hline(
                        ui.available_rect_before_wrap().x_range(),
                        ui.cursor().top(),
                        egui::Stroke::new(1.0, egui::Color32::from_rgba_unmultiplied(0, 220, 255, 60)),
                    );
                    ui.add_space(3.0);
                } else {
                    ui.add_space(2.0);
                }
                continue;
            }
        }

        if trimmed.starts_with('>') {
            let mut quote_lines = vec![trimmed.strip_prefix('>').unwrap_or("").trim()];
            while let Some(&next_line) = lines.peek() {
                let next_trimmed = next_line.trim();
                if next_trimmed.starts_with('>') {
                    quote_lines.push(next_trimmed.strip_prefix('>').unwrap_or("").trim());
                    lines.next();
                } else {
                    break;
                }
            }
            ui.add_space(2.0);
            let quote_text = quote_lines.join(" ");
            egui::Frame::new()
                .fill(egui::Color32::from_rgba_unmultiplied(0, 160, 255, 16))
                .stroke(egui::Stroke::new(1.0, egui::Color32::from_rgba_unmultiplied(0, 180, 255, 45)))
                .corner_radius(egui::CornerRadius::same(6))
                .inner_margin(egui::Margin::symmetric(10, 6))
                .show(ui, |ui| {
                    ui.horizontal(|ui| {
                        ui.label(egui::RichText::new("▍").size(13.0).color(egui::Color32::from_rgb(0, 200, 255)));
                        let avail = ui.available_width().max(40.0);
                        let job = create_layout_job(&quote_text, 12.0, avail, true, egui::Color32::from_rgb(205, 225, 245));
                        ui.label(job);
                    });
                });
            ui.add_space(2.0);
            continue;
        }

        if trimmed.starts_with('|') && trimmed.ends_with('|') && trimmed.matches('|').count() >= 2 {
            let mut table_lines = vec![trimmed.to_string()];
            while let Some(&next_line) = lines.peek() {
                let next_trimmed = next_line.trim();
                if next_trimmed.starts_with('|') && next_trimmed.ends_with('|') {
                    table_lines.push(next_trimmed.to_string());
                    lines.next();
                } else {
                    break;
                }
            }
            block_idx += 1;
            let table_id = format!("md_tbl_{card_id}_{block_idx}");

            let mut headers = Vec::new();
            let mut rows = Vec::new();
            for (idx, t_line) in table_lines.iter().enumerate() {
                let cells: Vec<String> = t_line
                    .trim()
                    .trim_matches('|')
                    .split('|')
                    .map(|c| c.trim().to_string())
                    .collect();
                if idx == 0 {
                    headers = cells;
                } else if cells.iter().all(|c| c.chars().all(|ch| ch == '-' || ch == ':' || ch == ' ')) {
                    continue;
                } else {
                    rows.push(cells);
                }
            }

            if !headers.is_empty() || !rows.is_empty() {
                ui.add_space(3.0);
                egui::Frame::new()
                    .fill(egui::Color32::from_rgba_unmultiplied(10, 14, 20, 180))
                    .stroke(egui::Stroke::new(1.0, egui::Color32::from_rgba_unmultiplied(0, 180, 255, 50)))
                    .corner_radius(egui::CornerRadius::same(6))
                    .inner_margin(egui::Margin::symmetric(8, 6))
                    .show(ui, |ui| {
                        egui::Grid::new(table_id)
                            .striped(true)
                            .spacing([12.0, 5.0])
                            .show(ui, |ui| {
                                for h in &headers {
                                    let avail = ui.available_width().max(40.0);
                                    let job = create_layout_job(h, 12.0, avail, false, egui::Color32::from_rgb(0, 215, 255));
                                    ui.label(job);
                                }
                                ui.end_row();

                                for row in &rows {
                                    for (c_idx, cell) in row.iter().enumerate() {
                                        let text_color = if c_idx == 0 {
                                            egui::Color32::from_rgb(240, 245, 255)
                                        } else {
                                            egui::Color32::from_rgb(205, 220, 235)
                                        };
                                        let avail = ui.available_width().max(40.0);
                                        let job = create_layout_job(cell, 11.5, avail, false, text_color);
                                        ui.label(job);
                                    }
                                    ui.end_row();
                                }
                            });
                    });
                ui.add_space(3.0);
            }
            continue;
        }

        if trimmed.is_empty() {
            ui.add_space(4.0);
            continue;
        }

        let indent_spaces = line.chars().take_while(|c| c.is_whitespace()).count();
        let indent_level = (indent_spaces / 2).min(4);

        if trimmed.starts_with("- ") || trimmed.starts_with("* ") || trimmed.starts_with("+ ") || trimmed.starts_with("• ") {
            let item_content = &trimmed[2..].trim();
            let (bullet, bullet_color) = match indent_level {
                0 => ("•", egui::Color32::from_rgb(0, 215, 255)),
                1 => ("◦", egui::Color32::from_rgb(0, 190, 235)),
                _ => ("▪", egui::Color32::from_rgb(0, 165, 215)),
            };

            ui.horizontal(|ui| {
                if indent_level > 0 {
                    ui.add_space(indent_level as f32 * 12.0);
                }
                ui.label(egui::RichText::new(bullet).size(12.0).color(bullet_color).strong());
                let avail = ui.available_width().max(40.0);
                let job = create_layout_job(item_content, 12.5, avail, false, egui::Color32::from_rgb(225, 235, 245));
                ui.label(job);
            });
            ui.add_space(1.5);
            continue;
        }

        if let Some((num_str, item_content)) = parse_numbered_list(trimmed) {
            ui.horizontal(|ui| {
                if indent_level > 0 {
                    ui.add_space(indent_level as f32 * 12.0);
                }
                ui.label(egui::RichText::new(num_str).size(12.0).color(egui::Color32::from_rgb(255, 215, 110)).strong());
                let avail = ui.available_width().max(40.0);
                let job = create_layout_job(item_content, 12.5, avail, false, egui::Color32::from_rgb(225, 235, 245));
                ui.label(job);
            });
            ui.add_space(1.5);
            continue;
        }

        let avail = ui.available_width().max(40.0);
        let job = create_layout_job(trimmed, 12.5, avail, false, egui::Color32::from_rgb(220, 230, 242));
        ui.label(job);
        ui.add_space(2.0);
    }
}