use std::sync::Mutex;
use eframe::egui;
use eframe::egui::text::LayoutJob;
use eframe::egui::TextFormat;
use serde::{Deserialize, Serialize};
use crate::automation::set_clipboard_text;
use crate::{apply_dwell_to_response, DwellState};
use crate::{play_earcon, Earcon};

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct OpenCardSummary {
    pub id: String,
    pub title: String,
    pub card_type: String,
    pub content_preview: String,
}

/// Registre global en temps réel des fenêtres volantes ouvertes
pub static OPEN_FLOATING_CARDS: Mutex<Vec<OpenCardSummary>> = Mutex::new(Vec::new());

/// Met à jour le registre global des fenêtres volantes ouvertes
pub fn update_open_floating_cards(cards: &[FloatingCard]) {
    if let Ok(mut lock) = OPEN_FLOATING_CARDS.lock() {
        *lock = cards
            .iter()
            .filter(|c| c.open)
            .map(|c| {
                let (ctype, preview) = match &c.content {
                    FloatingContentType::Table { headers, rows } => {
                        let row_sample = rows.iter().take(10).map(|r| r.join(" | ")).collect::<Vec<_>>().join("\n");
                        ("table", format!("Headers: {}\nRows:\n{}", headers.join(" | "), row_sample))
                    }
                    FloatingContentType::Stats { items } => {
                        let it = items.iter().map(|(k, v)| format!("{k}: {v}")).collect::<Vec<_>>().join(", ");
                        ("stats", it)
                    }
                    FloatingContentType::Markdown { text } => {
                        ("markdown", text.clone())
                    }
                    FloatingContentType::Form { fields, .. } => {
                        let f_summary = fields.iter().map(|f| f.label.clone()).collect::<Vec<_>>().join(", ");
                        ("form", format!("Fields: {}", f_summary))
                    }
                };
                OpenCardSummary {
                    id: c.id.clone(),
                    title: c.title.clone(),
                    card_type: ctype.to_string(),
                    content_preview: preview,
                }
            })
            .collect();
    }
}

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

/// Gère le défilement par clic-glissé (drag-to-scroll) natif via les événements de pointeur
fn handle_drag_to_scroll(ui: &egui::Ui, allow_x: bool, allow_y: bool) {
    let (is_dragging, delta, started_in_ui) = ui.input(|i| {
        let is_down = i.pointer.primary_down() || i.pointer.middle_down();
        let dragging = i.pointer.is_decidedly_dragging() && is_down;
        let in_ui = i.pointer
            .press_origin()
            .or_else(|| i.pointer.interact_pos())
            .map_or(false, |pos| ui.clip_rect().contains(pos));
        (dragging, i.pointer.delta(), in_ui)
    });

    if is_dragging && started_in_ui && !ui.ctx().egui_is_using_pointer() {
        let mut scroll_delta = egui::Vec2::ZERO;
        if allow_x {
            scroll_delta.x = delta.x;
        }
        if allow_y {
            scroll_delta.y = delta.y;
        }
        if scroll_delta != egui::Vec2::ZERO {
            ui.scroll_with_delta(scroll_delta);
            ui.ctx().set_cursor_icon(egui::CursorIcon::Grabbing);
        }
    }
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

    pub fn show(&mut self, ctx: &egui::Context, dwell_state: &mut DwellState) -> Option<String> {
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
            FloatingContentType::Markdown { .. } => [560.0, 380.0],
            FloatingContentType::Table { .. } => [580.0, 340.0],
            _ => [480.0, 280.0],
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
            vp_ctx.request_repaint();
            if vp_ctx.input(|i| i.viewport().close_requested()) {
                self.open = false;
            }

            let is_maximized = vp_ctx.input(|i| i.viewport().maximized.unwrap_or(false));

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
                        
                        // Raccourcis et boutons de copie pour le contenu Markdown
                        if let FloatingContentType::Markdown { ref text } = self.content {
                            let copy_all = ui.input(|i| {
                                (i.modifiers.ctrl || i.modifiers.command) && i.key_pressed(egui::Key::C)
                            });
                            let select_all = ui.input(|i| {
                                (i.modifiers.ctrl || i.modifiers.command) && i.key_pressed(egui::Key::A)
                            });

                            if select_all || copy_all {
                                set_clipboard_text(text);
                                ui.ctx().copy_text(text.clone());
                                play_earcon(Earcon::Success);
                            }

                            ui.with_layout(egui::Layout::left_to_right(egui::Align::Center), |ui| {
                                ui.add_space(8.0);
                                let copy_btn = ui.add(
                                    egui::Button::new(
                                        egui::RichText::new("📋 Copier tout")
                                            .size(10.5)
                                            .color(egui::Color32::from_rgb(180, 220, 255)),
                                    )
                                    .small()
                                    .corner_radius(egui::CornerRadius::same(4)),
                                );
                                let copy_btn_id = ui.make_persistent_id(format!("dwell_copy_all_{}", self.id));
                                if apply_dwell_to_response(ui, dwell_state, copy_btn_id, &copy_btn) {
                                    set_clipboard_text(text);
                                    ui.ctx().copy_text(text.clone());
                                    play_earcon(Earcon::Success);
                                }
                            });
                        }

                        ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                            let close_btn = ui.button(
                                egui::RichText::new("✕")
                                    .size(12.5)
                                    .color(egui::Color32::from_rgb(255, 130, 130)),
                            ).on_hover_text("Fermer cette fenêtre volante");
                            let close_id = ui.make_persistent_id(format!("dwell_close_card_{}", self.id));
                            if apply_dwell_to_response(ui, dwell_state, close_id, &close_btn) {
                                self.open = false;
                            }

                            let max_icon = if is_maximized { "⧉" } else { "□" };
                            let max_btn = ui.button(
                                egui::RichText::new(max_icon)
                                    .size(11.0)
                                    .color(egui::Color32::from_rgb(200, 215, 230)),
                            ).on_hover_text(if is_maximized { "Restaurer" } else { "Agrandir" });
                            let max_id = ui.make_persistent_id(format!("dwell_max_card_{}", self.id));
                            if apply_dwell_to_response(ui, dwell_state, max_id, &max_btn) {
                                ui.ctx().send_viewport_cmd(egui::ViewportCommand::Maximized(!is_maximized));
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

                    egui::ScrollArea::vertical()
                        .id_salt(format!("card_scroll_vert_{card_id}"))
                        .auto_shrink([false, false])
                        .show(ui, |ui| {
                            handle_drag_to_scroll(ui, false, true);
                            match &mut self.content {
                                FloatingContentType::Table { headers, rows } => {
                                    if headers.is_empty() && rows.is_empty() {
                                        ui.label(egui::RichText::new("Tableau vide").italics().color(egui::Color32::GRAY));
                                        return;
                                    }

                                    let mut full_table_md = String::new();
                                    if !headers.is_empty() {
                                        full_table_md.push_str(&format!("| {} |\n", headers.join(" | ")));
                                        let sep: Vec<String> = headers.iter().map(|h| "-".repeat(h.len().max(3))).collect();
                                        full_table_md.push_str(&format!("| {} |\n", sep.join(" | ")));
                                    }
                                    for r in rows.iter() {
                                        full_table_md.push_str(&format!("| {} |\n", r.join(" | ")));
                                    }

                                    ui.horizontal(|ui| {
                                        ui.label(egui::RichText::new("📊 Données").size(11.0).color(egui::Color32::from_rgb(0, 200, 255)).strong());
                                        ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                                            let copy_tbl = ui.add(
                                                egui::Button::new(
                                                    egui::RichText::new("📋 Copier le tableau")
                                                        .size(10.5)
                                                        .color(egui::Color32::from_rgb(180, 220, 255)),
                                                )
                                                .small()
                                                .corner_radius(egui::CornerRadius::same(4)),
                                            );
                                            let copy_tbl_id = ui.make_persistent_id(format!("dwell_copy_table_{}", self.id));
                                            if apply_dwell_to_response(ui, dwell_state, copy_tbl_id, &copy_tbl) {
                                                set_clipboard_text(&full_table_md);
                                                ui.ctx().copy_text(full_table_md.clone());
                                                play_earcon(Earcon::Success);
                                            }
                                        });
                                    });
                                    ui.add_space(2.0);

                                    let num_cols = headers.len().max(rows.iter().map(|r| r.len()).max().unwrap_or(1));
                                    let col_lens = calculate_col_lengths(headers, rows, num_cols);
                                    let avail_w = ui.available_width();
                                    let col_widths = calculate_proportional_col_widths(&col_lens, avail_w, 14.0);

                                    egui::ScrollArea::horizontal()
                                        .id_salt(format!("card_tbl_scroll_horiz_{card_id}"))
                                        .auto_shrink([false, false])
                                        .show(ui, |ui| {
                                            handle_drag_to_scroll(ui, true, false);
                                            egui::Grid::new(format!("grid_{card_id}"))
                                                .striped(true)
                                                .spacing([14.0, 6.0])
                                                .min_col_width(0.0)
                                                .show(ui, |ui| {
                                                    for (c_idx, h) in headers.iter().enumerate() {
                                                        let w = col_widths.get(c_idx).copied().unwrap_or(85.0);
                                                        render_table_cell(ui, h, egui::Color32::from_rgb(0, 210, 255), headers, &full_table_md, true, w, dwell_state, &format!("th_{card_id}_{c_idx}"));
                                                    }
                                                    ui.end_row();

                                                    for (r_idx, row) in rows.iter().enumerate() {
                                                        for (c_idx, cell) in row.iter().enumerate() {
                                                            let text_color = if c_idx == 0 {
                                                                egui::Color32::from_rgb(240, 245, 255)
                                                            } else {
                                                                egui::Color32::from_rgb(205, 220, 235)
                                                            };
                                                            let w = col_widths.get(c_idx).copied().unwrap_or(85.0);
                                                            if let Some(act) = render_table_cell(ui, cell, text_color, row, &full_table_md, false, w, dwell_state, &format!("td_{card_id}_{r_idx}_{c_idx}")) {
                                                                submitted_prompt = Some(act);
                                                            }
                                                        }
                                                        ui.end_row();
                                                    }
                                                });
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
                                    // Permet le clic droit pour copier l'ensemble du Markdown
                                    let full_rect = ui.available_rect_before_wrap();
                                    let bg_resp = ui.interact(full_rect, ui.id().with("md_bg_context"), egui::Sense::click());
                                    bg_resp.context_menu(|ui| {
                                        if ui.button("📋 Tout copier (Ctrl+C / Ctrl+A)").clicked() {
                                            set_clipboard_text(text);
                                            ui.ctx().copy_text(text.clone());
                                            play_earcon(Earcon::Success);
                                            ui.close();
                                        }
                                    });

                                    let select_or_copy = ui.input(|i| {
                                        (i.modifiers.ctrl || i.modifiers.command) && (i.key_pressed(egui::Key::C) || i.key_pressed(egui::Key::A))
                                    });
                                    if select_or_copy && (bg_resp.hovered() || ui.rect_contains_pointer(full_rect)) {
                                        set_clipboard_text(text);
                                        ui.ctx().copy_text(text.clone());
                                        play_earcon(Earcon::Success);
                                    }

                                    if let Some(prompt) = render_markdown(ui, text, &card_id, dwell_state) {
                                        submitted_prompt = Some(prompt);
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

                                            let submit_resp = ui.add(submit_btn).on_hover_text("Valider les critères et lancer la requête");
                                            let submit_id = ui.make_persistent_id(format!("dwell_form_submit_{}", self.id));
                                            if apply_dwell_to_response(ui, dwell_state, submit_id, &submit_resp) {
                                                form_submitted = true;
                                            }

                                            let cancel_resp = ui.button("Annuler");
                                            let cancel_id = ui.make_persistent_id(format!("dwell_form_cancel_{}", self.id));
                                            if apply_dwell_to_response(ui, dwell_state, cancel_id, &cancel_resp) {
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

                    // Zones de redimensionnement interactif sur le pourtour et poignée visuelle aux coins
                    if !is_maximized {
                        let screen_rect = ui.ctx().input(|i| i.content_rect());
                        let total_w = screen_rect.width();
                        let total_h = screen_rect.height();
                        let min = screen_rect.min;
                        let border_thickness = 6.0;

                        let resize_zones = [
                            // Bord haut et coins hauts
                            (egui::Rect::from_min_size(min, egui::vec2(border_thickness, border_thickness)), egui::viewport::ResizeDirection::NorthWest, egui::CursorIcon::ResizeNorthWest),
                            (egui::Rect::from_min_size(egui::pos2(min.x + total_w - border_thickness, min.y), egui::vec2(border_thickness, border_thickness)), egui::viewport::ResizeDirection::NorthEast, egui::CursorIcon::ResizeNorthEast),
                            (egui::Rect::from_min_size(egui::pos2(min.x + border_thickness, min.y), egui::vec2((total_w - 2.0 * border_thickness).max(0.0), border_thickness)), egui::viewport::ResizeDirection::North, egui::CursorIcon::ResizeNorth),

                            // Côtés gauche et droite
                            (egui::Rect::from_min_size(egui::pos2(min.x, min.y + border_thickness), egui::vec2(border_thickness, (total_h - 2.0 * border_thickness).max(0.0))), egui::viewport::ResizeDirection::West, egui::CursorIcon::ResizeWest),
                            (egui::Rect::from_min_size(egui::pos2(min.x + total_w - border_thickness, min.y + border_thickness), egui::vec2(border_thickness, (total_h - 2.0 * border_thickness).max(0.0))), egui::viewport::ResizeDirection::East, egui::CursorIcon::ResizeEast),

                            // Bord bas et coins bas
                            (egui::Rect::from_min_size(egui::pos2(min.x, min.y + total_h - border_thickness), egui::vec2(border_thickness, border_thickness)), egui::viewport::ResizeDirection::SouthWest, egui::CursorIcon::ResizeSouthWest),
                            (egui::Rect::from_min_size(egui::pos2(min.x + total_w - border_thickness, min.y + total_h - border_thickness), egui::vec2(border_thickness, border_thickness)), egui::viewport::ResizeDirection::SouthEast, egui::CursorIcon::ResizeSouthEast),
                            (egui::Rect::from_min_size(egui::pos2(min.x + border_thickness, min.y + total_h - border_thickness), egui::vec2((total_w - 2.0 * border_thickness).max(0.0), border_thickness)), egui::viewport::ResizeDirection::South, egui::CursorIcon::ResizeSouth),
                        ];

                        for (rect, direction, cursor_icon) in resize_zones {
                            let resp = ui.interact(rect, egui::Id::new(format!("card_resize_{:?}_{card_id}", direction)), egui::Sense::drag());
                            if resp.hovered() {
                                ui.ctx().set_cursor_icon(cursor_icon);
                            }
                            if resp.drag_started() {
                                ui.ctx().send_viewport_cmd(egui::ViewportCommand::BeginResize(direction));
                            }
                        }

                        let painter = ui.painter();
                        let grip_se = egui::Rect::from_min_size(egui::pos2(min.x + total_w - 12.0, min.y + total_h - 12.0), egui::vec2(10.0, 10.0));
                        painter.line_segment(
                            [egui::pos2(grip_se.min.x, grip_se.max.y), egui::pos2(grip_se.max.x, grip_se.min.y)],
                            egui::Stroke::new(1.0, egui::Color32::from_rgba_unmultiplied(0, 180, 255, 90)),
                        );
                        painter.line_segment(
                            [egui::pos2(grip_se.min.x + 3.0, grip_se.max.y), egui::pos2(grip_se.max.x, grip_se.min.y + 3.0)],
                            egui::Stroke::new(1.0, egui::Color32::from_rgba_unmultiplied(0, 180, 255, 50)),
                        );
                    }

                    if close_window {
                        self.open = false;
                    }
                });
        });
        ctx.request_repaint_of(viewport_id);

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
    target: Option<&'a str>,
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
                        target: None,
                    });
                }
                spans.push(InlineSpan {
                    text: &line[i + 1..i + 1 + end],
                    style: InlineStyle::Code,
                    target: None,
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
                        target: None,
                    });
                }
                spans.push(InlineSpan {
                    text: &line[i + 3..i + 3 + end],
                    style: InlineStyle::BoldItalic,
                    target: None,
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
                        target: None,
                    });
                }
                spans.push(InlineSpan {
                    text: &line[i + 2..i + 2 + end],
                    style: InlineStyle::Bold,
                    target: None,
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
                        target: None,
                    });
                }
                spans.push(InlineSpan {
                    text: &line[i + 2..i + 2 + end],
                    style: InlineStyle::Strike,
                    target: None,
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
                            target: None,
                        });
                    }
                    spans.push(InlineSpan {
                        text: &line[i + 1..i + 1 + end],
                        style: InlineStyle::Italic,
                        target: None,
                    });
                    i += 1 + end + 1;
                    plain_start = i;
                    continue;
                }
            }
        } else if line[i..].starts_with("http://") || line[i..].starts_with("https://") {
            // Détection automatique des URLs brutes
            let end = line[i..]
                .find(|c: char| c.is_whitespace() || c == ')' || c == ']' || c == '>')
                .map(|p| i + p)
                .unwrap_or(len);
            if i > plain_start {
                spans.push(InlineSpan {
                    text: &line[plain_start..i],
                    style: InlineStyle::Normal,
                    target: None,
                });
            }
            let url_slice = &line[i..end];
            spans.push(InlineSpan {
                text: url_slice,
                style: InlineStyle::Link,
                target: Some(url_slice),
            });
            i = end;
            plain_start = i;
            continue;
        } else if bytes[i] == b'[' {
            // Détection et parsing de la macro [link="url"]label[/link] ou auto-fermante [link="..." /]
            let remaining = &line[i..];
            let rem_lower = remaining.to_ascii_lowercase();
            if rem_lower.starts_with("[link=") || rem_lower.starts_with("[link ") {
                if let Some(open_tag_end) = remaining.find(']') {
                    let open_tag_content = &remaining[..open_tag_end];
                    let is_self_closing = open_tag_content.trim_end().ends_with('/');
                    let target_val = if let Some(eq_pos) = open_tag_content.find('=') {
                        let after_eq = open_tag_content[eq_pos + 1..].trim();
                        if let Some(quote) = after_eq.chars().next().filter(|&c| c == '"' || c == '\'') {
                            let rest = &after_eq[quote.len_utf8()..];
                            if let Some(end_q) = rest.find(quote) {
                                Some(&rest[..end_q])
                            } else {
                                Some(rest.trim())
                            }
                        } else {
                            let end_val = after_eq
                                .find(|c: char| c.is_whitespace() || c == ']' || c == '/')
                                .unwrap_or(after_eq.len());
                            Some(&after_eq[..end_val])
                        }
                    } else {
                        None
                    };

                    if is_self_closing {
                        let label_val = crate::html_util::extract_attribute(open_tag_content, "label")
                            .or_else(|| crate::html_util::extract_attribute(open_tag_content, "text"))
                            .or_else(|| crate::html_util::extract_attribute(open_tag_content, "title"))
                            .or(target_val)
                            .unwrap_or("Lien");

                        if i > plain_start {
                            spans.push(InlineSpan {
                                text: &line[plain_start..i],
                                style: InlineStyle::Normal,
                                target: None,
                            });
                        }
                        spans.push(InlineSpan {
                            text: label_val,
                            style: InlineStyle::Link,
                            target: target_val,
                        });
                        i += open_tag_end + 1;
                        plain_start = i;
                        continue;
                    }

                    let after_open = i + open_tag_end + 1;
                    if let Some(close_pos) = line[after_open..].to_ascii_lowercase().find("[/link]") {
                        if i > plain_start {
                            spans.push(InlineSpan {
                                text: &line[plain_start..i],
                                style: InlineStyle::Normal,
                                target: None,
                            });
                        }
                        let link_text = &line[after_open..after_open + close_pos];
                        spans.push(InlineSpan {
                            text: link_text,
                            style: InlineStyle::Link,
                            target: target_val,
                        });
                        i = after_open + close_pos + "[/link]".len();
                        plain_start = i;
                        continue;
                    }
                }
            }

            if let Some(bracket_end) = line[i + 1..].find(']') {
                let mut after_bracket = i + 1 + bracket_end + 1;
                while after_bracket < len && (bytes[after_bracket] == b' ' || bytes[after_bracket] == b'\t') {
                    after_bracket += 1;
                }
                if after_bracket < len && bytes[after_bracket] == b'(' {
                    if let Some(paren_end) = line[after_bracket + 1..].find(')') {
                        if i > plain_start {
                            spans.push(InlineSpan {
                                text: &line[plain_start..i],
                                style: InlineStyle::Normal,
                                target: None,
                            });
                        }
                        let link_text = &line[i + 1..i + 1 + bracket_end];
                        let link_target = &line[after_bracket + 1..after_bracket + 1 + paren_end];
                        spans.push(InlineSpan {
                            text: link_text,
                            style: InlineStyle::Link,
                            target: Some(link_target),
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
            target: None,
        });
    }

    spans
}

pub fn render_markdown_line_to_html(line: &str) -> String {
    let spans = parse_inline_markdown(line);
    let mut out = String::with_capacity(line.len() * 2);
    for span in spans {
        let escaped = span.text
            .replace('&', "&amp;")
            .replace('<', "&lt;")
            .replace('>', "&gt;")
            .replace('"', "&quot;");

        if span.style == InlineStyle::Link {
            if let Some(target) = span.target {
                let target_esc = target
                    .replace('&', "&amp;")
                    .replace('<', "&lt;")
                    .replace('>', "&gt;")
                    .replace('"', "&quot;");
                out.push_str(&format!("<a href=\"{target_esc}\">{escaped}</a>"));
            } else {
                out.push_str(&escaped);
            }
        } else {
            match span.style {
                InlineStyle::Bold => out.push_str(&format!("<b>{escaped}</b>")),
                InlineStyle::Italic => out.push_str(&format!("<i>{escaped}</i>")),
                InlineStyle::BoldItalic => out.push_str(&format!("<b><i>{escaped}</i></b>")),
                InlineStyle::Code => out.push_str(&format!("<code>{escaped}</code>")),
                InlineStyle::Strike => out.push_str(&format!("<s>{escaped}</s>")),
                _ => out.push_str(&escaped),
            }
        }
    }
    out
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

fn extract_action_prompt(target: &str) -> Option<String> {
    let t = target.trim();
    if t.is_empty() {
        return None;
    }
    let clean = t.trim_start_matches('<').trim_end_matches('>').trim_matches('"').trim_matches('\'').trim();
    let lower = clean.to_lowercase();
    if lower.starts_with("mailto:") {
        return None;
    }
    
    // Si c'est un libellé préfixé par un symbole vidéo
    if clean.starts_with('▶') || clean.starts_with("🎬") || lower.starts_with("voir :") || lower.starts_with("voir:") {
        let clean_video_title = clean.trim_start_matches(['▶', '🎬', ' ']).trim();
        return Some(format!("Lancer la vidéo ou chercher : {clean_video_title}"));
    }

    if clean.starts_with('🎵') || clean.starts_with("🎙") || clean.starts_with("📻") || lower.starts_with("audio:") || lower.starts_with("ecouter:") || lower.starts_with("écouter:") {
        let clean_audio_title = clean.trim_start_matches(['🎵', '🎙', '📻', ' ']).trim();
        return Some(format!("Lancer l'audio ou écouter : {clean_audio_title}"));
    }

    if clean.starts_with('📄') || lower.starts_with("doc:") || lower.starts_with("file:") || lower.starts_with("pdf:") {
        let clean_file_path = clean.trim_start_matches(['📄', ' ']).trim();
        return Some(format!("Ouvrir le fichier {clean_file_path}"));
    }

    if clean.starts_with('🖼') || lower.starts_with("image:") || lower.starts_with("img:") {
        let clean_img_path = clean.trim_start_matches(['🖼', ' ']).trim();
        return Some(format!("Ouvrir l'image {clean_img_path}"));
    }

    if lower.starts_with("file://") {
        let path = clean.trim_start_matches("file://").trim_start_matches('/');
        return Some(format!("Ouvrir le fichier {path}"));
    }

    if lower.starts_with("http://") || lower.starts_with("https://") {
        let url_clean = clean.split_whitespace().next().unwrap_or(clean);
        return Some(format!("Navigue vers {url_clean}"));
    }

    let prompt_raw = if let Some(rest) = t.strip_prefix("action:") {
        rest.trim()
    } else if let Some(rest) = t.strip_prefix("prompt:") {
        rest.trim()
    } else if let Some(rest) = t.strip_prefix("send:") {
        rest.trim()
    } else if let Some(rest) = t.strip_prefix("cmd:") {
        rest.trim()
    } else {
        t
    };

    let unquoted = prompt_raw
        .strip_prefix('"').and_then(|s| s.strip_suffix('"'))
        .or_else(|| prompt_raw.strip_prefix('\'').and_then(|s| s.strip_suffix('\'')))
        .unwrap_or(prompt_raw)
        .trim();

    if !unquoted.is_empty() {
        Some(unquoted.to_string())
    } else {
        None
    }
}

fn handle_link_click(ctx: &egui::Context, target: &str, fallback_text: &str) -> Option<String> {
    let t = target.trim();
    if let Some(prompt) = extract_action_prompt(t) {
        return Some(prompt);
    }
    let clean = t.trim_start_matches('<').trim_end_matches('>').trim_matches('"').trim_matches('\'').trim();
    if clean.starts_with("http://") || clean.starts_with("https://") {
        let url_clean = clean.split_whitespace().next().unwrap_or(clean);
        return Some(format!("Navigue vers {url_clean}"));
    }
    if clean.starts_with("mailto:") {
        let _ = ctx.open_url(egui::OpenUrl::new_tab(clean));
        return Some(format!("Écris un email à {}", clean.trim_start_matches("mailto:")));
    }
    let fallback_clean = fallback_text.trim();
    if !fallback_clean.is_empty() {
        Some(fallback_clean.to_string())
    } else {
        None
    }
}

fn render_line_spans(
    ui: &mut egui::Ui,
    line: &str,
    base_size: f32,
    force_italic: bool,
    base_color: egui::Color32,
    dwell_state: &mut DwellState,
    span_prefix_id: &str,
) -> Option<String> {
    let spans = parse_inline_markdown(line);
    let has_links = spans.iter().any(|s| s.style == InlineStyle::Link && s.target.is_some());

    if !has_links {
        let avail = ui.available_width().max(40.0);
        let job = create_layout_job(line, base_size, avail, force_italic, base_color);
        let resp = ui.add(egui::Label::new(job).wrap().sense(egui::Sense::click()));
        resp.context_menu(|ui| {
            if ui.button("📋 Copier le texte").clicked() {
                set_clipboard_text(line);
                ui.ctx().copy_text(line.to_string());
                play_earcon(Earcon::Success);
                ui.close();
            }
        });
        return None;
    }

    let mut action_to_run: Option<String> = None;

    let row_resp = ui.horizontal_wrapped(|ui| {
        ui.spacing_mut().item_spacing = egui::vec2(3.5, 3.5);

        for (span_idx, span) in spans.into_iter().enumerate() {
            if span.style == InlineStyle::Link && span.target.is_some() {
                let raw_text = span.text.trim();
                let clean_text = raw_text
                    .trim_start_matches("**")
                    .trim_end_matches("**")
                    .trim_start_matches('*')
                    .trim_end_matches('*')
                    .trim();
                let target = span.target.unwrap_or(clean_text);

                let is_action = extract_action_prompt(target).is_some();
                let has_icon = clean_text.starts_with(|c: char| !c.is_alphanumeric() && c != '[' && c != '(');
                let display_label = if has_icon {
                    clean_text.to_string()
                } else {
                    let icon = if is_action { "⚡ " } else { "🔗 " };
                    format!("{icon}{clean_text}")
                };

                let (bg_color, stroke_color, text_color) = if is_action {
                    (
                        egui::Color32::from_rgba_unmultiplied(0, 180, 255, 38),
                        egui::Color32::from_rgba_unmultiplied(0, 215, 255, 130),
                        egui::Color32::from_rgb(0, 235, 255),
                    )
                } else {
                    (
                        egui::Color32::from_rgba_unmultiplied(100, 180, 255, 20),
                        egui::Color32::from_rgba_unmultiplied(100, 180, 255, 70),
                        egui::Color32::from_rgb(140, 200, 255),
                    )
                };

                let btn = egui::Button::new(
                    egui::RichText::new(display_label)
                        .size(base_size)
                        .color(text_color)
                        .strong(),
                )
                .fill(bg_color)
                .stroke(egui::Stroke::new(1.0, stroke_color))
                .corner_radius(egui::CornerRadius::same(5))
                .small();

                let hover_desc = if let Some(prompt) = extract_action_prompt(target) {
                    format!("⚡ Demander à l'IA : « {prompt} »\nCliquer pour exécuter immédiatement")
                } else {
                    format!("Ouvrir le lien web : {target}")
                };

                let btn_resp = ui.add(btn).on_hover_text(hover_desc);
                let span_dwell_id = ui.make_persistent_id(format!("{span_prefix_id}_span_{span_idx}"));
                if apply_dwell_to_response(ui, dwell_state, span_dwell_id, &btn_resp) {
                    action_to_run = handle_link_click(ui.ctx(), target, clean_text);
                }
            } else {
                let words: Vec<&str> = span.text.split_whitespace().collect();
                if words.is_empty() {
                    continue;
                }
                for word in words {
                    ui.add(egui::Label::new(egui::RichText::new(word).size(base_size).color(base_color)).wrap());
                }
            }
        }
    });

    row_resp.response.context_menu(|ui| {
        if ui.button("📋 Copier le texte").clicked() {
            set_clipboard_text(line);
            ui.ctx().copy_text(line.to_string());
            play_earcon(Earcon::Success);
            ui.close();
        }
    });

    action_to_run
}

/// Découpe une ligne de tableau Markdown en respectant les barres obliques échappées (\|), les crochets et les guillemets
fn split_table_row(line: &str) -> Vec<String> {
    let trimmed = line.trim();
    let content = if trimmed.starts_with('|') {
        let without_start = &trimmed[1..];
        if without_start.ends_with('|') {
            &without_start[..without_start.len() - 1]
        } else {
            without_start
        }
    } else if trimmed.ends_with('|') {
        &trimmed[..trimmed.len() - 1]
    } else {
        trimmed
    };

    let mut cells = Vec::new();
    let mut cur = String::new();
    let chars: Vec<char> = content.chars().collect();
    let mut in_bracket = false;
    let mut in_quotes = false;
    let mut i = 0;

    while i < chars.len() {
        let c = chars[i];
        if c == '\\' && i + 1 < chars.len() {
            if chars[i + 1] == '|' {
                cur.push('|');
                i += 2;
                continue;
            }
            cur.push(c);
            cur.push(chars[i + 1]);
            i += 2;
            continue;
        }

        if c == '"' {
            in_quotes = !in_quotes;
        } else if c == '[' {
            in_bracket = true;
        } else if c == ']' {
            in_bracket = false;
        }

        if c == '|' && !in_bracket && !in_quotes {
            cells.push(cur.trim().to_string());
            cur.clear();
        } else {
            cur.push(c);
        }
        i += 1;
    }
    cells.push(cur.trim().to_string());
    cells
}

fn estimate_visible_length(text: &str) -> usize {
    let spans = parse_inline_markdown(text);
    let mut len = 0;
    for span in &spans {
        len += span.text.chars().count();
    }
    len.max(text.chars().count().min(8))
}

fn calculate_col_lengths(headers: &[String], rows: &[Vec<String>], num_cols: usize) -> Vec<usize> {
    let mut lens = vec![8usize; num_cols];
    for (c, l) in lens.iter_mut().enumerate() {
        if let Some(h) = headers.get(c) {
            *l = (*l).max(estimate_visible_length(h).clamp(6, 120));
        }
        for row in rows {
            if let Some(cell) = row.get(c) {
                *l = (*l).max(estimate_visible_length(cell).clamp(6, 120));
            }
        }
    }
    lens
}

fn calculate_proportional_col_widths(col_lens: &[usize], avail_w: f32, spacing: f32) -> Vec<f32> {
    let n = col_lens.len();
    if n == 0 {
        return Vec::new();
    }
    let total_spacing = (n.saturating_sub(1) as f32) * spacing;
    let min_col_w = 70.0f32;
    let usable_w = (avail_w - total_spacing).max(n as f32 * min_col_w);

    let total_len: usize = col_lens.iter().sum::<usize>().max(1);
    let mut widths: Vec<f32> = col_lens
        .iter()
        .map(|&l| (l as f32 / total_len as f32) * usable_w)
        .collect();

    let mut underflow = 0.0f32;
    let mut flexible_indices = Vec::new();
    let mut flexible_total_weight = 0.0f32;

    for (i, w) in widths.iter_mut().enumerate() {
        if *w < min_col_w {
            underflow += min_col_w - *w;
            *w = min_col_w;
        } else {
            flexible_indices.push(i);
            flexible_total_weight += col_lens[i] as f32;
        }
    }

    if underflow > 0.0 && flexible_total_weight > 0.0 {
        for &idx in &flexible_indices {
            let share = (col_lens[idx] as f32 / flexible_total_weight) * underflow;
            widths[idx] = (widths[idx] - share).max(min_col_w);
        }
    }

    widths
}

fn render_table_cell(
    ui: &mut egui::Ui,
    cell: &str,
    text_color: egui::Color32,
    row: &[String],
    table_markdown: &str,
    is_header: bool,
    col_width: f32,
    dwell_state: &mut DwellState,
    cell_id: &str,
) -> Option<String> {
    let mut clicked_action = None;
    let resp = ui.scope(|ui| {
        ui.set_min_width(col_width);
        ui.set_max_width(col_width);

        if is_header {
            let job = create_layout_job(cell, 12.0, col_width, false, text_color);
            let l_resp = ui.add(egui::Label::new(job).wrap().sense(egui::Sense::click()));
            l_resp.context_menu(|ui| {
                if ui.button("📋 Copier l'en-tête").clicked() {
                    set_clipboard_text(cell);
                    ui.ctx().copy_text(cell.to_string());
                    play_earcon(Earcon::Success);
                    ui.close();
                }
                if ui.button("📋 Copier tout le tableau").clicked() {
                    set_clipboard_text(table_markdown);
                    ui.ctx().copy_text(table_markdown.to_string());
                    play_earcon(Earcon::Success);
                    ui.close();
                }
            });
        } else {
            clicked_action = render_line_spans(ui, cell, 11.5, false, text_color, dwell_state, cell_id);
        }
    });

    resp.response.context_menu(|ui| {
        if ui.button("📋 Copier la cellule").clicked() {
            set_clipboard_text(cell);
            ui.ctx().copy_text(cell.to_string());
            play_earcon(Earcon::Success);
            ui.close();
        }
        if !is_header && !row.is_empty() {
            if ui.button("📋 Copier la ligne").clicked() {
                let row_str = row.join(" | ");
                set_clipboard_text(&row_str);
                ui.ctx().copy_text(row_str);
                play_earcon(Earcon::Success);
                ui.close();
            }
        }
        if ui.button("📋 Copier tout le tableau").clicked() {
            set_clipboard_text(table_markdown);
            ui.ctx().copy_text(table_markdown.to_string());
            play_earcon(Earcon::Success);
            ui.close();
        }
    });

    clicked_action
}

pub fn render_markdown(ui: &mut egui::Ui, text: &str, card_id: &str, dwell_state: &mut DwellState) -> Option<String> {
    let mut lines = text.lines().peekable();
    let mut block_idx = 0usize;
    let mut clicked_action: Option<String> = None;

    while let Some(line) = lines.next() {
        let trimmed = line.trim();

        if trimmed.starts_with("```") {
            block_idx += 1;
            let code_scroll_id = format!("md_code_scroll_{card_id}_{block_idx}");
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
                            let copy_code_btn = ui.small_button("📋 Copier").on_hover_text("Copier ce code");
                            let copy_code_id = ui.make_persistent_id(format!("dwell_copy_code_{code_scroll_id}"));
                            if apply_dwell_to_response(ui, dwell_state, copy_code_id, &copy_code_btn) {
                                ui.ctx().copy_text(full_code.clone());
                                play_earcon(Earcon::Success);
                            }
                        });
                    });
                    ui.add_space(2.0);
                    ui.separator();
                    ui.add_space(2.0);

                    egui::ScrollArea::horizontal()
                        .id_salt(code_scroll_id)
                        .auto_shrink([false, true])
                        .show(ui, |ui| {
                            handle_drag_to_scroll(ui, true, false);
                            let mut code_job = LayoutJob::default();
                            code_job.wrap.max_width = f32::INFINITY;
                            let mut fmt = TextFormat::default();
                            fmt.font_id = egui::FontId::monospace(11.5);
                            fmt.color = egui::Color32::from_rgb(215, 230, 245);
                            code_job.append(&full_code, 0.0, fmt);
                            ui.label(code_job);
                        });
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
                if let Some(act) = render_line_spans(ui, content, font_size, false, color, dwell_state, &format!("hd_{card_id}_{block_idx}")) {
                    clicked_action = Some(act);
                }

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
                        if let Some(act) = render_line_spans(ui, &quote_text, 12.0, true, egui::Color32::from_rgb(205, 225, 245), dwell_state, &format!("quote_{card_id}_{block_idx}")) {
                            clicked_action = Some(act);
                        }
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
            let table_scroll_id = format!("md_tbl_scroll_{card_id}_{block_idx}");

            let mut headers = Vec::new();
            let mut rows = Vec::new();
            for (idx, t_line) in table_lines.iter().enumerate() {
                let cells = split_table_row(t_line);
                if idx == 0 {
                    headers = cells;
                } else if cells.iter().all(|c| c.chars().all(|ch| ch == '-' || ch == ':' || ch == ' ')) {
                    continue;
                } else {
                    rows.push(cells);
                }
            }

            if !headers.is_empty() || !rows.is_empty() {
                let raw_table_md = table_lines.join("\n");
                ui.add_space(3.0);
                egui::Frame::new()
                    .fill(egui::Color32::from_rgba_unmultiplied(10, 14, 20, 180))
                    .stroke(egui::Stroke::new(1.0, egui::Color32::from_rgba_unmultiplied(0, 180, 255, 50)))
                    .corner_radius(egui::CornerRadius::same(6))
                    .inner_margin(egui::Margin::symmetric(8, 6))
                    .show(ui, |ui| {
                        ui.horizontal(|ui| {
                            ui.label(egui::RichText::new("📊 Tableau").size(10.5).color(egui::Color32::from_rgb(0, 200, 255)).strong());
                            ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                                let copy_tbl_btn = ui.add(
                                    egui::Button::new(
                                        egui::RichText::new("📋 Copier le tableau")
                                            .size(10.0)
                                            .color(egui::Color32::from_rgb(180, 220, 255)),
                                    )
                                    .small()
                                    .corner_radius(egui::CornerRadius::same(4)),
                                );
                                let copy_tbl_id = ui.make_persistent_id(format!("dwell_copy_tbl_{table_id}"));
                                if apply_dwell_to_response(ui, dwell_state, copy_tbl_id, &copy_tbl_btn) {
                                    set_clipboard_text(&raw_table_md);
                                    ui.ctx().copy_text(raw_table_md.clone());
                                    play_earcon(Earcon::Success);
                                }
                            });
                        });
                        ui.add_space(2.0);
                        ui.separator();
                        ui.add_space(2.0);

                        let num_cols = headers.len().max(rows.iter().map(|r| r.len()).max().unwrap_or(1));
                        let col_lens = calculate_col_lengths(&headers, &rows, num_cols);
                        let avail_w = ui.available_width();
                        let col_widths = calculate_proportional_col_widths(&col_lens, avail_w, 12.0);

                        egui::ScrollArea::horizontal()
                            .id_salt(table_scroll_id)
                            .auto_shrink([false, false])
                            .show(ui, |ui| {
                                handle_drag_to_scroll(ui, true, false);
                                egui::Grid::new(&table_id)
                                    .striped(true)
                                    .spacing([12.0, 5.0])
                                    .min_col_width(0.0)
                                    .show(ui, |ui| {
                                        for (c_idx, h) in headers.iter().enumerate() {
                                            let w = col_widths.get(c_idx).copied().unwrap_or(70.0);
                                            render_table_cell(ui, h, egui::Color32::from_rgb(0, 215, 255), &headers, &raw_table_md, true, w, dwell_state, &format!("md_th_{table_id}_{c_idx}"));
                                        }
                                        ui.end_row();

                                        for (r_idx, row) in rows.iter().enumerate() {
                                            for (c_idx, cell) in row.iter().enumerate() {
                                                let text_color = if c_idx == 0 {
                                                    egui::Color32::from_rgb(240, 245, 255)
                                                } else {
                                                    egui::Color32::from_rgb(205, 220, 235)
                                                };
                                                let w = col_widths.get(c_idx).copied().unwrap_or(70.0);
                                                if let Some(act) = render_table_cell(ui, cell, text_color, row, &raw_table_md, false, w, dwell_state, &format!("md_td_{table_id}_{r_idx}_{c_idx}")) {
                                                    clicked_action = Some(act);
                                                }
                                            }
                                            ui.end_row();
                                        }
                                    });
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

        block_idx += 1;
        let indent_spaces = line.chars().take_while(|c| c.is_whitespace()).count();
        let indent_level = (indent_spaces / 2).min(4);

        // Auto-génération de liens pour les vidéos et éléments multimédias commençant par ▶ ou 🎬
        if (trimmed.starts_with("▶ ") || trimmed.starts_with("🎬 ")) && !trimmed.contains("](") && !trimmed.contains("[/link]") {
            let video_title = trimmed.trim_start_matches(['▶', '🎬', ' ']).trim();
            let prompt_target = format!("action:Lancer la vidéo : {video_title}");
            ui.horizontal(|ui| {
                if indent_level > 0 {
                    ui.add_space(indent_level as f32 * 12.0);
                }
                let synthetic_link = format!("[▶ {video_title}]({prompt_target})");
                if let Some(act) = render_line_spans(ui, &synthetic_link, 12.5, false, egui::Color32::from_rgb(0, 235, 255), dwell_state, &format!("media_vid_{card_id}_{block_idx}")) {
                    clicked_action = Some(act);
                }
            });
            ui.add_space(2.0);
            continue;
        }

        // Auto-génération de liens pour les fichiers audio, documents et images avec emojis
        if (trimmed.starts_with("🎵 ") || trimmed.starts_with("🎙️ ") || trimmed.starts_with("📻 ")) && !trimmed.contains("](") && !trimmed.contains("[/link]") {
            let audio_title = trimmed.trim_start_matches(['🎵', '🎙', '📻', ' ']).trim();
            let prompt_target = format!("action:Lancer l'audio : {audio_title}");
            ui.horizontal(|ui| {
                if indent_level > 0 {
                    ui.add_space(indent_level as f32 * 12.0);
                }
                let synthetic_link = format!("[🎵 {audio_title}]({prompt_target})");
                if let Some(act) = render_line_spans(ui, &synthetic_link, 12.5, false, egui::Color32::from_rgb(0, 235, 255), dwell_state, &format!("media_aud_{card_id}_{block_idx}")) {
                    clicked_action = Some(act);
                }
            });
            ui.add_space(2.0);
            continue;
        }

        if trimmed.starts_with("📄 ") && !trimmed.contains("](") && !trimmed.contains("[/link]") {
            let doc_title = trimmed.trim_start_matches(['📄', ' ']).trim();
            let prompt_target = format!("action:Ouvrir le fichier : {doc_title}");
            ui.horizontal(|ui| {
                if indent_level > 0 {
                    ui.add_space(indent_level as f32 * 12.0);
                }
                let synthetic_link = format!("[📄 {doc_title}]({prompt_target})");
                if let Some(act) = render_line_spans(ui, &synthetic_link, 12.5, false, egui::Color32::from_rgb(0, 235, 255), dwell_state, &format!("media_doc_{card_id}_{block_idx}")) {
                    clicked_action = Some(act);
                }
            });
            ui.add_space(2.0);
            continue;
        }

        if trimmed.starts_with("🖼️ ") && !trimmed.contains("](") && !trimmed.contains("[/link]") {
            let img_title = trimmed.trim_start_matches(['🖼', ' ']).trim();
            let prompt_target = format!("action:Ouvrir l'image : {img_title}");
            ui.horizontal(|ui| {
                if indent_level > 0 {
                    ui.add_space(indent_level as f32 * 12.0);
                }
                let synthetic_link = format!("[🖼️ {img_title}]({prompt_target})");
                if let Some(act) = render_line_spans(ui, &synthetic_link, 12.5, false, egui::Color32::from_rgb(0, 235, 255), dwell_state, &format!("media_img_{card_id}_{block_idx}")) {
                    clicked_action = Some(act);
                }
            });
            ui.add_space(2.0);
            continue;
        }

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
                if let Some(act) = render_line_spans(ui, item_content, 12.5, false, egui::Color32::from_rgb(225, 235, 245), dwell_state, &format!("li_{card_id}_{block_idx}")) {
                    clicked_action = Some(act);
                }
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
                if let Some(act) = render_line_spans(ui, item_content, 12.5, false, egui::Color32::from_rgb(225, 235, 245), dwell_state, &format!("num_{card_id}_{block_idx}")) {
                    clicked_action = Some(act);
                }
            });
            ui.add_space(1.5);
            continue;
        }

        if let Some(act) = render_line_spans(ui, trimmed, 12.5, false, egui::Color32::from_rgb(220, 230, 242), dwell_state, &format!("p_{card_id}_{block_idx}")) {
            clicked_action = Some(act);
        }
        ui.add_space(2.0);
    }

    clicked_action
}