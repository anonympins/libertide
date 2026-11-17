use std::sync::Mutex;
use std::time::{Duration, Instant};
use eframe::egui;
use rust_i18n::t;
use crate::floating_window::FloatingCardOutput;
use crate::overlay::{apply_dwell_to_response, DwellState, play_earcon, Earcon};
use crate::theme::ThemeColors;

const FRENCH_DICTIONARY: &[&str] = &[
    "oui", "non", "merci", "aide", "soif", "faim", "mal", "douleur", "soignant",
    "fatigue", "chaud", "froid", "position", "respirer", "aspirer", "famille",
    "bonjour", "bonsoir", "s'il vous plaît", "au revoir", "d'accord", "bien",
    "je", "tu", "il", "elle", "nous", "vous", "ils", "le", "la", "les", "un", "une",
    "des", "ce", "cette", "qui", "que", "quoi", "où", "quand", "comment", "pourquoi",
    "être", "avoir", "faire", "dire", "aller", "voir", "pouvoir", "vouloir", "savoir",
    "besoin", "dormir", "manger", "boire", "assis", "allonge", "médicament", "eau",
    "lit", "bras", "jambe", "dos", "tête", "yeux", "gorge", "peur", "calme",
    "plus", "moins", "beaucoup", "un peu", "trop", "très", "maintenant", "après",
];

pub const SUPPORTED_LANGUAGES: &[(&str, &str)] = &[
    ("fr", "Français"),
    ("en", "English"),
    ("de", "Deutsch"),
    ("es", "Español"),
    ("it", "Italiano"),
    ("pt", "Português"),
    ("ru", "Русский"),
    ("ja", "日本語"),
    ("zh", "中文"),
];

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ScanningStage {
    Row,
    Key,
}

#[derive(Clone, Debug)]
pub struct VirtualKeyboardState {
    pub text_buffer: String,
    pub scanning_enabled: bool,
    pub scan_stage: ScanningStage,
    pub current_row: usize,
    pub current_col: usize,
    pub last_scan_tick: Instant,
    pub scan_interval: Duration,
    pub language: String,
}

impl Default for VirtualKeyboardState {
    fn default() -> Self {
        Self {
            text_buffer: String::new(),
            scanning_enabled: true,
            scan_stage: ScanningStage::Row,
            current_row: 0,
            current_col: 0,
            last_scan_tick: Instant::now(),
            scan_interval: Duration::from_millis(1200),
            language: String::new(),
        }
    }
}

static KEYBOARD_STATE: Mutex<Option<VirtualKeyboardState>> = Mutex::new(None);

fn get_letter_rows(lang: &str) -> &'static [&'static [&'static str]] {
    match lang {
        "en" => &[
            &["E", "T", "A", "O", "I", "N", "S", "H"],
            &["R", "D", "L", "C", "U", "M", "W", "F"],
            &["G", "Y", "P", "B", "V", "K", "J", "X"],
            &["Q", "Z", "'", "-", ",", ".", "?", "!"],
        ],
        "de" => &[
            &["E", "N", "I", "S", "R", "A", "T", "D"],
            &["H", "U", "L", "C", "G", "M", "O", "B"],
            &["W", "F", "K", "Z", "P", "V", "J", "Y"],
            &["Ä", "Ö", "Ü", "ß", "X", "Q", ".", "!"],
        ],
        "es" => &[
            &["E", "A", "O", "S", "R", "N", "I", "D"],
            &["L", "C", "T", "U", "M", "P", "B", "G"],
            &["V", "Y", "Q", "H", "F", "Z", "J", "X"],
            &["Ñ", "Á", "É", "Í", "Ó", "Ú", "¿", "!"],
        ],
        "it" => &[
            &["E", "A", "I", "O", "N", "T", "R", "L"],
            &["S", "C", "D", "U", "P", "M", "V", "G"],
            &["H", "F", "B", "Q", "Z", "K", "W", "Y"],
            &["À", "È", "É", "Ì", "Ò", "Ù", "'", "!"],
        ],
        "pt" => &[
            &["A", "E", "O", "S", "R", "I", "N", "D"],
            &["M", "U", "T", "C", "L", "P", "V", "G"],
            &["H", "B", "Q", "F", "Z", "J", "X", "K"],
            &["Ã", "Õ", "Ç", "Á", "É", "Ó", "Í", "!"],
        ],
        "ru" => &[
            &["О", "Е", "А", "И", "Н", "Т", "С", "Р"],
            &["В", "Л", "К", "М", "Д", "П", "У", "Я"],
            &["Ы", "Ь", "Г", "З", "Б", "Ч", "Й", "Х"],
            &["Ж", "Ш", "Ю", "Ц", "Щ", "Э", "Ф", "Ъ"],
        ],
        "ja" => &[
            &["A", "I", "U", "E", "O", "K", "S", "T"],
            &["N", "H", "M", "Y", "R", "W", "G", "Z"],
            &["D", "B", "P", "J", "C", "F", "X", "V"],
            &["-", "Q", "L", "。", "、", "！", "？", "〜"],
        ],
        "zh" => &[
            &["A", "O", "E", "I", "U", "V", "N", "G"],
            &["B", "P", "M", "F", "D", "T", "N", "L"],
            &["G", "K", "H", "J", "Q", "X", "R", "W"],
            &["Z", "C", "S", "Y", "，", "。", "！", "？"],
        ],
        _ => &[
            &["E", "S", "A", "I", "T", "N", "R", "U"],
            &["L", "O", "D", "C", "M", "P", "V", "Q"],
            &["F", "B", "G", "H", "J", "X", "Y", "Z"],
            &["É", "È", "À", "Ç", "'", ".", "?", "!"],
        ],
    }
}

fn get_dictionary(lang: &str) -> &'static [&'static str] {
    match lang {
        "en" => &[
            "yes", "no", "thank", "thanks", "please", "help", "thirsty", "hungry", "pain", "hurt",
            "caregiver", "tired", "hot", "cold", "position", "breathe", "family", "hello", "goodbye",
            "okay", "good", "i", "you", "he", "she", "we", "they", "the", "a", "this", "that",
            "who", "what", "where", "when", "why", "how", "be", "have", "do", "say", "go", "see",
            "can", "want", "know", "need", "sleep", "eat", "drink", "sit", "lie", "medicine", "water",
            "bed", "arm", "leg", "back", "head", "eyes", "throat", "fear", "calm", "more", "less",
            "now", "after",
        ],
        "de" => &[
            "ja", "nein", "danke", "bitte", "hilfe", "durst", "hunger", "schmerz", "weh", "pfleger",
            "müde", "kalt", "warm", "position", "atmen", "familie", "hallo", "gut", "ich", "du",
            "er", "sie", "wir", "ihr", "der", "die", "das", "ein", "eine", "wer", "was", "wo",
            "wann", "warum", "wie", "sein", "haben", "machen", "sagen", "gehen", "sehen",
            "können", "wollen", "wissen", "brauchen", "schlafen", "essen", "trinken", "wasser",
            "bett", "arm", "bein", "rücken", "kopf", "augen", "jetzt", "später",
        ],
        "es" => &[
            "sí", "no", "gracias", "por favor", "ayuda", "sed", "hambre", "dolor", "cuidador",
            "cansado", "calor", "frío", "posición", "respirar", "familia", "hola", "adiós",
            "bien", "yo", "tú", "él", "ella", "nosotros", "ellos", "el", "la", "los", "las",
            "un", "una", "quién", "qué", "dónde", "cuándo", "por qué", "cómo", "ser", "estar",
            "tener", "hacer", "decir", "ir", "ver", "poder", "querer", "saber", "necesitar",
            "dormir", "comer", "beber", "medicina", "agua", "cama", "brazo", "pierna", "espalda",
            "cabeza", "ojos", "ahora", "después",
        ],
        "it" => &[
            "sì", "no", "grazie", "per favore", "aiuto", "sete", "fame", "dolore", "male",
            "assistente", "stanco", "caldo", "freddo", "posizione", "respirare", "famiglia",
            "ciao", "arrivederci", "bene", "io", "tu", "lui", "lei", "noi", "voi", "loro",
            "il", "la", "i", "le", "un", "una", "chi", "cosa", "dove", "quando", "perché",
            "come", "essere", "avere", "fare", "dire", "andare", "vedere", "potere", "volere",
            "sapere", "bisogno", "dormire", "mangiare", "bere", "medicina", "acqua", "letto",
            "braccio", "gamba", "schiena", "testa", "occhi", "ora", "dopo",
        ],
        "pt" => &[
            "sim", "não", "obrigado", "por favor", "ajuda", "sede", "fome", "dor", "cuidador",
            "cansado", "quente", "frio", "posição", "respirar", "família", "olá", "adeus",
            "bem", "eu", "tu", "ele", "ela", "nós", "eles", "o", "a", "os", "as", "um", "uma",
            "quem", "que", "onde", "quando", "porque", "como", "ser", "estar", "ter", "fazer",
            "dizer", "ir", "ver", "poder", "querer", "saber", "precisar", "dormir", "comer",
            "beber", "remédio", "água", "cama", "braço", "perna", "costas", "cabeça", "olhos",
            "agora", "depois",
        ],
        "ru" => &[
            "да", "нет", "спасибо", "пожалуйста", "помощь", "жажда", "голод", "боль", "врач",
            "устал", "жарко", "холодно", "дышать", "семья", "привет", "пока", "хорошо",
            "я", "ты", "он", "она", "мы", "они", "кто", "что", "где", "когда", "почему", "как",
            "быть", "пить", "есть", "спать", "лекарство", "вода", "кровать", "рука", "нога",
            "спина", "голова", "глаза", "сейчас", "потом",
        ],
        "ja" => &[
            "hai", "iie", "arigatou", "tasukete", "mizu", "itai", "kusuri", "tsukareta",
            "samui", "atsui", "nemui", "tabetai", "nomitai", "konnichiwa", "sayounara",
        ],
        "zh" => &[
            "shi", "bushi", "xiexie", "bangzhu", "keshui", "iteng", "yaowu", "lei",
            "leng", "re", "shuijiao", "chifan", "heshui", "nihao", "zaijian",
        ],
        _ => FRENCH_DICTIONARY,
    }
}

fn get_default_predictions(lang: &str) -> Vec<String> {
    let list: &[&str] = match lang {
        "en" => &["Yes", "No", "Thanks", "Please", "Thirsty", "Help"],
        "de" => &["Ja", "Nein", "Danke", "Bitte", "Durst", "Hilfe"],
        "es" => &["Sí", "No", "Gracias", "Por favor", "Tengo sed", "Ayuda"],
        "it" => &["Sì", "No", "Grazie", "Per favore", "Ho sete", "Aiuto"],
        "pt" => &["Sim", "Não", "Obrigado", "Por favor", "Tenho sede", "Ajuda"],
        "ru" => &["Да", "Нет", "Спасибо", "Пожалуйста", "Пить", "Помощь"],
        "ja" => &["はい", "いいえ", "ありがとう", "水", "痛い", "助けて"],
        "zh" => &["是", "不是", "谢谢", "请", "喝水", "帮助"],
        _ => &["Oui", "Non", "Merci", "S'il vous plaît", "J'ai soif", "Aide"],
    };
    list.iter().map(|s| s.to_string()).collect()
}

fn get_active_app_language() -> String {
    let current_locale = rust_i18n::locale();
    current_locale.split(['-', '_']).next().unwrap_or("fr").to_lowercase()
}

fn get_predictions(lang: &str, current_text: &str) -> Vec<String> {
    let last_word = current_text
        .split_whitespace()
        .last()
        .unwrap_or("")
        .to_lowercase();

    if last_word.is_empty() {
        return get_default_predictions(lang);
    }

    let dict = get_dictionary(lang);
    let mut matches: Vec<String> = dict
        .iter()
        .filter(|w| w.starts_with(&last_word) && **w != last_word.as_str())
        .take(6)
        .map(|w| {
            let mut c = w.chars();
            match c.next() {
                None => String::new(),
                Some(f) => f.to_uppercase().collect::<String>() + c.as_str(),
            }
        })
        .collect();

    if matches.is_empty() {
        get_default_predictions(lang)
    } else {
        matches
    }
}

pub fn render_keyboard_view(
    ui: &mut egui::Ui,
    dwell_state: &mut DwellState,
    theme: &ThemeColors,
) -> Option<FloatingCardOutput> {
    let mut state = {
        let mut guard = KEYBOARD_STATE.lock().unwrap();
        if guard.is_none() {
            *guard = Some(VirtualKeyboardState::default());
        }
        guard.as_ref().cloned().unwrap()
    };

    let active_lang = if !state.language.is_empty() {
        state.language.clone()
    } else {
        get_active_app_language()
    };

    let mut output_event = None;
    let now = Instant::now();

    let letter_rows = get_letter_rows(&active_lang);
    let predictions = get_predictions(&active_lang, &state.text_buffer);
    let total_rows = 1 + letter_rows.len() + 1; // 1 (predictions) + 4 (lettres) + 1 (actions)

    // Avancement automatique du balayage (Auto-Scanning)
    if state.scanning_enabled && now.duration_since(state.last_scan_tick) >= state.scan_interval {
        state.last_scan_tick = now;
        match state.scan_stage {
            ScanningStage::Row => {
                state.current_row = (state.current_row + 1) % total_rows;
                state.current_col = 0;
            }
            ScanningStage::Key => {
                let max_cols = if state.current_row == 0 {
                    predictions.len()
                } else if state.current_row <= letter_rows.len() {
                    letter_rows[state.current_row - 1].len()
                } else {
                    6 // Actions
                };
                state.current_col = (state.current_col + 1) % max_cols.max(1);
            }
        }
        ui.ctx().request_repaint();
    }

    // Navigation manuelle au clavier ordinaire (Tab, Shift+Tab, Flèches directionnelles)
    let key_tab = ui.input_mut(|i| i.consume_key(egui::Modifiers::NONE, egui::Key::Tab));
    let key_shift_tab = ui.input_mut(|i| i.consume_key(egui::Modifiers::SHIFT, egui::Key::Tab));
    let key_right = ui.input_mut(|i| i.consume_key(egui::Modifiers::NONE, egui::Key::ArrowRight));
    let key_left = ui.input_mut(|i| i.consume_key(egui::Modifiers::NONE, egui::Key::ArrowLeft));
    let key_down = ui.input_mut(|i| i.consume_key(egui::Modifiers::NONE, egui::Key::ArrowDown));
    let key_up = ui.input_mut(|i| i.consume_key(egui::Modifiers::NONE, egui::Key::ArrowUp));

    let get_max_cols = |row: usize, pred_len: usize| -> usize {
        if row == 0 {
            pred_len.max(1)
        } else if row <= letter_rows.len() {
            letter_rows[row - 1].len()
        } else {
            6
        }
    };

    if key_tab || key_right {
        state.last_scan_tick = now;
        match state.scan_stage {
            ScanningStage::Row => {
                state.scan_stage = ScanningStage::Key;
                state.current_col = 0;
            }
            ScanningStage::Key => {
                let max_c = get_max_cols(state.current_row, predictions.len());
                state.current_col += 1;
                if state.current_col >= max_c {
                    state.current_col = 0;
                    state.current_row = (state.current_row + 1) % total_rows;
                }
            }
        }
        play_earcon(Earcon::Navigation);
        ui.ctx().request_repaint();
    }

    if key_shift_tab || key_left {
        state.last_scan_tick = now;
        match state.scan_stage {
            ScanningStage::Row => {
                state.current_row = if state.current_row == 0 { total_rows - 1 } else { state.current_row - 1 };
            }
            ScanningStage::Key => {
                if state.current_col == 0 {
                    state.current_row = if state.current_row == 0 { total_rows - 1 } else { state.current_row - 1 };
                    let max_c = get_max_cols(state.current_row, predictions.len());
                    state.current_col = max_c.saturating_sub(1);
                } else {
                    state.current_col -= 1;
                }
            }
        }
        play_earcon(Earcon::Navigation);
        ui.ctx().request_repaint();
    }

    if key_down {
        state.last_scan_tick = now;
        state.current_row = (state.current_row + 1) % total_rows;
        state.scan_stage = ScanningStage::Key;
        let max_c = get_max_cols(state.current_row, predictions.len());
        state.current_col = state.current_col.min(max_c.saturating_sub(1));
        play_earcon(Earcon::Navigation);
        ui.ctx().request_repaint();
    }

    if key_up {
        state.last_scan_tick = now;
        state.current_row = if state.current_row == 0 { total_rows - 1 } else { state.current_row - 1 };
        state.scan_stage = ScanningStage::Key;
        let max_c = get_max_cols(state.current_row, predictions.len());
        state.current_col = state.current_col.min(max_c.saturating_sub(1));
        play_earcon(Earcon::Navigation);
        ui.ctx().request_repaint();
    }

    // Entrée contacteur physique (Espace ou Entrée)
    let switch_pressed = ui.input(|i| {
        i.key_pressed(egui::Key::Space) || i.key_pressed(egui::Key::Enter)
    });

    let mut trigger_selected_key = false;
    if switch_pressed {
        state.last_scan_tick = now;
        match state.scan_stage {
            ScanningStage::Row => {
                state.scan_stage = ScanningStage::Key;
                state.current_col = 0;
                play_earcon(Earcon::Navigation);
            }
            ScanningStage::Key => {
                trigger_selected_key = true;
            }
        }
    }

    // Échap pour remonter du niveau touche au niveau ligne
    if ui.input(|i| i.key_pressed(egui::Key::Escape)) {
        state.scan_stage = ScanningStage::Row;
        state.current_col = 0;
        play_earcon(Earcon::Navigation);
    }

    // 1. Zone d'affichage du texte composé
    egui::Frame::new()
        .fill(egui::Color32::from_rgb(12, 14, 18))
        .stroke(egui::Stroke::new(1.2, theme.accent))
        .corner_radius(egui::CornerRadius::same(8))
        .inner_margin(egui::Margin::symmetric(12, 10))
        .show(ui, |ui| {
            ui.horizontal(|ui| {
                let placeholder = match active_lang.as_str() {
                    "en" => "Type or scan to compose a message...",
                    "de" => "Tippen oder scannen, um eine Nachricht zu verfassen...",
                    "es" => "Escriba o escanee para redactar un mensaje...",
                    "it" => "Digita o scansiona per comporre un messaggio...",
                    "pt" => "Digite ou faça varredura para compor uma mensagem...",
                    "ru" => "Печатайте или сканируйте для сообщения...",
                    "ja" => "入力またはスキャンでメッセージを作成...",
                    "zh" => "输入或扫描以编写消息...",
                    _ => "Tapez ou balayez pour composer un message...",
                };
                let display_text = if state.text_buffer.is_empty() {
                    placeholder
                } else {
                    &state.text_buffer
                };
                let color = if state.text_buffer.is_empty() {
                    egui::Color32::from_gray(140)
                } else {
                    egui::Color32::WHITE
                };
                ui.label(egui::RichText::new(display_text).size(18.0).color(color).strong());
            });
        });

    ui.add_space(6.0);

    // 2. Rangée des prédictions de mots
    let is_row_0_active = state.scanning_enabled && state.current_row == 0;
    ui.horizontal(|ui| {
        for (col_idx, pred) in predictions.iter().enumerate() {
            let is_scanned_key = is_row_0_active && state.scan_stage == ScanningStage::Key && state.current_col == col_idx;
            let is_row_selected = is_row_0_active && state.scan_stage == ScanningStage::Row;

            let mut btn = egui::Button::new(egui::RichText::new(pred).size(13.0).color(theme.accent_text).strong())
                .fill(if is_scanned_key { theme.accent } else if is_row_selected { theme.tab_active_bg } else { theme.control_bg })
                .stroke(egui::Stroke::new(if is_scanned_key { 2.5 } else { 1.0 }, theme.border))
                .corner_radius(egui::CornerRadius::same(6));

            let resp = ui.add(btn);
            let pred_dwell_id = ui.make_persistent_id(format!("dwell_pred_{col_idx}"));
            let clicked = resp.clicked() || (is_scanned_key && trigger_selected_key) || apply_dwell_to_response(ui, dwell_state, pred_dwell_id, &resp);

            if clicked {
                if let Some(last_space) = state.text_buffer.rfind(' ') {
                    state.text_buffer.truncate(last_space + 1);
                } else {
                    state.text_buffer.clear();
                }
                state.text_buffer.push_str(pred);
                state.text_buffer.push(' ');
                play_earcon(Earcon::Success);
                state.scan_stage = ScanningStage::Row;
            }
        }
    });

    ui.add_space(6.0);
    ui.separator();
    ui.add_space(4.0);

    // 3. Grille des lettres (rangées adaptées à la langue)
    let total_w = ui.available_width();
    let max_cols_in_grid = letter_rows.iter().map(|r| r.len()).max().unwrap_or(8);
    let key_w = ((total_w - (max_cols_in_grid as f32 - 1.0) * 6.0) / max_cols_in_grid as f32).max(28.0);
    let key_h = 38.0;

    for (r_idx, row) in letter_rows.iter().enumerate() {
        let row_num = r_idx + 1;
        let is_current_row = state.scanning_enabled && state.current_row == row_num;
        let is_row_highlight = is_current_row && state.scan_stage == ScanningStage::Row;

        ui.horizontal(|ui| {
            ui.spacing_mut().item_spacing.x = 6.0;
            for (c_idx, &letter) in row.iter().enumerate() {
                let is_key_highlight = is_current_row && state.scan_stage == ScanningStage::Key && state.current_col == c_idx;

                let bg_color = if is_key_highlight {
                    egui::Color32::from_rgb(255, 215, 0)
                } else if is_row_highlight {
                    theme.tab_active_bg
                } else {
                    theme.control_bg
                };

                let text_color = if is_key_highlight {
                    egui::Color32::BLACK
                } else {
                    egui::Color32::WHITE
                };

                let btn = egui::Button::new(egui::RichText::new(letter).size(16.0).color(text_color).strong())
                    .fill(bg_color)
                    .stroke(egui::Stroke::new(if is_key_highlight { 2.5 } else { 1.0 }, if is_key_highlight { egui::Color32::WHITE } else { theme.border }))
                    .corner_radius(egui::CornerRadius::same(6))
                    .min_size(egui::vec2(key_w, key_h));

                let resp = ui.add(btn);
                let key_dwell_id = ui.make_persistent_id(format!("dwell_key_{r_idx}_{c_idx}"));
                let clicked = resp.clicked() || (is_key_highlight && trigger_selected_key) || apply_dwell_to_response(ui, dwell_state, key_dwell_id, &resp);

                if clicked {
                    state.text_buffer.push_str(letter);
                    play_earcon(Earcon::Success);
                    state.scan_stage = ScanningStage::Row;
                }
            }
        });
        ui.add_space(4.0);
    }

    ui.add_space(4.0);
    ui.separator();
    ui.add_space(6.0);

    // 4. Rangée des actions (Espace, Effacer, Parler, Tchat, Insérer, Vider) localisée
    let is_actions_row = state.scanning_enabled && state.current_row == (total_rows - 1);
    let is_action_row_selected = is_actions_row && state.scan_stage == ScanningStage::Row;

    ui.horizontal(|ui| {
        ui.spacing_mut().item_spacing.x = 6.0;

        let actions: [(&str, usize); 6] = match active_lang.as_str() {
            "en" => [("⎵ Space", 0), ("⌫ Delete", 1), ("🗣️ Speak", 2), ("💬 Chat", 3), ("✍️ Insert", 4), ("🗑️ Clear", 5)],
            "de" => [("⎵ Leer", 0), ("⌫ Löschen", 1), ("🗣️ Sprechen", 2), ("💬 Chat", 3), ("✍️ Einfügen", 4), ("🗑️ Leeren", 5)],
            "es" => [("⎵ Espacio", 0), ("⌫ Borrar", 1), ("🗣️ Hablar", 2), ("💬 Chat", 3), ("✍️ Insertar", 4), ("🗑️ Vaciar", 5)],
            "it" => [("⎵ Spazio", 0), ("⌫ Cancella", 1), ("🗣️ Parla", 2), ("💬 Chat", 3), ("✍️ Inserisci", 4), ("🗑️ Svuota", 5)],
            "pt" => [("⎵ Espaço", 0), ("⌫ Apagar", 1), ("🗣️ Falar", 2), ("💬 Chat", 3), ("✍️ Inserir", 4), ("🗑️ Limpar", 5)],
            "ru" => [("⎵ Пробел", 0), ("⌫ Стереть", 1), ("🗣️ Голос", 2), ("💬 Чат", 3), ("✍️ Вставить", 4), ("🗑️ Очистить", 5)],
            "ja" => [("⎵ 空白", 0), ("⌫ 削除", 1), ("🗣️ 発声", 2), ("💬 チャット", 3), ("✍️ 挿入", 4), ("🗑️ 消去", 5)],
            "zh" => [("⎵ 空格", 0), ("⌫ 删除", 1), ("🗣️ 朗读", 2), ("💬 对话", 3), ("✍️ 插入", 4), ("🗑️ 清空", 5)],
            _ => [("⎵ Espace", 0), ("⌫ Effacer", 1), ("🗣️ Parler", 2), ("💬 Tchat", 3), ("✍️ Insérer", 4), ("🗑️ Vider", 5)],
        };

        for (label, col_idx) in actions {
            let is_action_key = is_actions_row && state.scan_stage == ScanningStage::Key && state.current_col == col_idx;

            let bg = if is_action_key {
                egui::Color32::from_rgb(0, 230, 118)
            } else if is_action_row_selected {
                theme.tab_active_bg
            } else {
                theme.control_bg
            };

            let txt_col = if is_action_key {
                egui::Color32::BLACK
            } else if col_idx == 2 || col_idx == 3 {
                theme.accent
            } else {
                theme.text_primary
            };

            let btn = egui::Button::new(egui::RichText::new(label).size(13.0).color(txt_col).strong())
                .fill(bg)
                .stroke(egui::Stroke::new(if is_action_key { 2.5 } else { 1.0 }, theme.border))
                .corner_radius(egui::CornerRadius::same(6))
                .min_size(egui::vec2(68.0, 34.0));

            let resp = ui.add(btn);
            let act_dwell_id = ui.make_persistent_id(format!("dwell_act_{col_idx}"));
            let clicked = resp.clicked() || (is_action_key && trigger_selected_key) || apply_dwell_to_response(ui, dwell_state, act_dwell_id, &resp);

            if clicked {
                match col_idx {
                    0 => {
                        state.text_buffer.push(' ');
                        play_earcon(Earcon::Success);
                    }
                    1 => {
                        state.text_buffer.pop();
                        play_earcon(Earcon::Navigation);
                    }
                    2 => {
                        // Synthèse vocale TTS instantanée
                        if !state.text_buffer.trim().is_empty() {
                            output_event = Some(FloatingCardOutput::Speak(state.text_buffer.clone()));
                            play_earcon(Earcon::Success);
                        }
                    }
                    3 => {
                        // Envoi direct au Tchat / LLM
                        if !state.text_buffer.trim().is_empty() {
                            output_event = Some(FloatingCardOutput::Prompt(state.text_buffer.clone()));
                            state.text_buffer.clear();
                            play_earcon(Earcon::Success);
                        }
                    }
                    4 => {
                        // Injection dans l'application externe au premier plan
                        if !state.text_buffer.trim().is_empty() {
                            crate::automation::set_clipboard_text(&state.text_buffer);
                            crate::automation::restore_external_focus();
                            std::thread::sleep(Duration::from_millis(50));
                            crate::automation::send_paste();
                            play_earcon(Earcon::Success);
                        }
                    }
                    5 => {
                        state.text_buffer.clear();
                        play_earcon(Earcon::MicClose);
                    }
                    _ => {}
                }
                state.scan_stage = ScanningStage::Row;
            }
        }
    });

    ui.add_space(4.0);
    ui.horizontal(|ui| {
        let auto_scan_label = match active_lang.as_str() {
            "en" => "Auto-Scan",
            "de" => "Automatisches Scannen (Auto-Scan)",
            "es" => "Escaneo automático (Auto-Scan)",
            "it" => "Scansione automatica (Auto-Scan)",
            "pt" => "Varredura automática (Auto-Scan)",
            "ru" => "Автосканирование (Auto-Scan)",
            "ja" => "自動スキャン (Auto-Scan)",
            "zh" => "自动扫描 (Auto-Scan)",
            _ => "Balayage automatique (Auto-Scan)",
        };
        ui.checkbox(&mut state.scanning_enabled, auto_scan_label);
        let mut ms = state.scan_interval.as_millis() as u64;
        if ui.add(egui::Slider::new(&mut ms, 500..=3000).suffix(" ms")).changed() {
            state.scan_interval = Duration::from_millis(ms);
        }

        ui.separator();
        let display_name = SUPPORTED_LANGUAGES
            .iter()
            .find(|(c, _)| *c == active_lang)
            .map(|(_, n)| *n)
            .unwrap_or("Français");

        egui::ComboBox::from_id_salt("vk_lang_select")
            .selected_text(format!("🌐 {}", display_name))
            .show_ui(ui, |ui| {
                for (code, name) in SUPPORTED_LANGUAGES {
                    let is_selected = active_lang == *code;
                    if ui.selectable_label(is_selected, *name).clicked() {
                        state.language = code.to_string();
                        rust_i18n::set_locale(code);
                        std::env::set_var("APP_LANG", code);
                        let _ = crate::fs_util::write_env_var_to_file(".env", "APP_LANG", code);
                    }
                }
            });
    });

    if let Ok(mut guard) = KEYBOARD_STATE.lock() {
        *guard = Some(state);
    }

    output_event
}