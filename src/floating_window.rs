use eframe::egui;
use serde::{Deserialize, Serialize};

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

    pub fn show(&mut self, ctx: &egui::Context) {
        if !self.open {
            return;
        }

        let viewport_id = egui::ViewportId::from_hash_of(&self.id);
        let viewport_title = format!("📊 {}", self.title);

        let builder = egui::ViewportBuilder::default()
            .with_title(&viewport_title)
            .with_transparent(true)
            .with_decorations(false)
            .with_always_on_top()
            .with_inner_size([440.0, 270.0])
            .with_min_inner_size([280.0, 150.0])
            .with_resizable(true);

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

                    egui::ScrollArea::both()
                        .auto_shrink([false, false])
                        .show(ui, |ui| {
                            match &self.content {
                                FloatingContentType::Table { headers, rows } => {
                                    if headers.is_empty() && rows.is_empty() {
                                        ui.label(egui::RichText::new("Tableau vide").italics().color(egui::Color32::GRAY));
                                        return;
                                    }
                                    egui::Grid::new(format!("grid_{}", self.id))
                                        .striped(true)
                                        .spacing([14.0, 6.0])
                                        .show(ui, |ui| {
                                            for h in headers {
                                                ui.label(egui::RichText::new(h).strong().color(egui::Color32::from_rgb(0, 210, 255)));
                                            }
                                            ui.end_row();

                                            for row in rows {
                                                for cell in row {
                                                    ui.label(egui::RichText::new(cell).color(egui::Color32::from_rgb(230, 235, 245)));
                                                }
                                                ui.end_row();
                                            }
                                        });
                                }
                                FloatingContentType::Stats { items } => {
                                    ui.vertical(|ui| {
                                        for (k, v) in items {
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
                            }
                        });
                });
        });
    }
}