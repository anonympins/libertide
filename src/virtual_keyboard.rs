use std::collections::VecDeque;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::mpsc::{channel, Receiver as StdReceiver, Sender as StdSender};
use std::sync::Mutex;
use std::time::{Duration, Instant};
use eframe::egui;
use llama_cpp_2::model::LlamaModel;
use rust_i18n::t;
use crate::clipboard::*;
use crate::floating_window::FloatingCardOutput;
use crate::overlay::{apply_dwell_to_response, DwellState, play_earcon, Earcon};
use crate::fs_util::read_env_var_from_file;
use crate::types::AgentEvent;
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

/// Bi-grammes et tri-grammes fréquents pour la prédiction instantanée du mot suivant
const FRENCH_BIGRAMS: &[(&str, &[&str])] = &[
    ("j'ai", &["soif", "faim", "mal", "froid", "chaud", "sommeil", "besoin", "peur"]),
    ("je", &["veux", "peux", "suis", "vais", "dois", "voudrais", "souhaite", "sais"]),
    ("je veux", &["boire", "manger", "dormir", "m'allonger", "partir", "me reposer"]),
    ("je suis", &["fatigué", "bien", "assis", "allongé", "prêt", "malade"]),
    ("il", &["faut", "fait", "est", "a", "va", "y a"]),
    ("il fait", &["froid", "chaud", "beau", "nuit"]),
    ("s'il", &["vous plaît", "te plaît"]),
    ("merci", &["beaucoup", "pour l'aide", "infiniment"]),
    ("aide", &["moi", "soignant", "s'il vous plaît"]),
    ("le", &["lit", "bras", "dos", "docteur", "soignant", "médicament"]),
    ("la", &["jambe", "tête", "famille", "position", "douleur"]),
    ("donne", &["moi de l'eau", "moi mon médicament", "moi à boire"]),
    ("est-ce", &["que tu peux m'aider", "que c'est possible", "que le médecin arrive"]),
];

const ENGLISH_BIGRAMS: &[(&str, &[&str])] = &[
    ("i", &["want", "need", "am", "feel", "can", "have"]),
    ("i am", &["tired", "cold", "hot", "thirsty", "hungry", "okay", "in pain"]),
    ("i want", &["water", "to sleep", "to drink", "to eat", "to sit", "help"]),
    ("i need", &["help", "water", "medicine", "my caregiver", "to breathe"]),
    ("please", &["help", "come", "water", "open", "close"]),
    ("could", &["you help me", "you please come", "i have some water"]),
    ("thank", &["you", "you very much"]),
    ("how", &["are you", "much", "long"]),
];

fn get_bigrams(lang: &str) -> &'static [(&'static str, &'static [&'static str])] {
    match lang {
        "en" => ENGLISH_BIGRAMS,
        _ => FRENCH_BIGRAMS,
    }
}

// ============================================================================
// Modèle LLM Qwen 2.5 0.5B de Complétion de Phrase & Téléchargement Asynchrone
// ============================================================================

pub const KEYBOARD_MODEL_FILENAME: &str = "qwen2.5-0.5b-instruct-q4_k_m.gguf";
pub const KEYBOARD_MODEL_HF_URL: &str = "https://huggingface.co/Qwen/Qwen2.5-0.5B-Instruct-GGUF/resolve/main/qwen2.5-0.5b-instruct-q4_k_m.gguf";

static KEYBOARD_MODEL_LOADED: AtomicBool = AtomicBool::new(false);
static KEYBOARD_MODEL_DOWNLOADING: AtomicBool = AtomicBool::new(false);
static LLM_PREDICTOR_ENGINE: Mutex<Option<crate::local_llm::LocalLlamaEngine>> = Mutex::new(None);

static LLM_REQUEST_TX: Mutex<Option<StdSender<(String, String)>>> = Mutex::new(None);
static ASYNC_PREDICTIONS_CACHE: Mutex<Option<(String, Vec<String>)>> = Mutex::new(None);

// ============================================================================
// Grille CAA / Pictogrammes ARASAAC & Code Couleur de Fitzgerald
// ============================================================================

#[derive(Clone, Copy, Debug, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub enum FitzgeraldCategory {
    /// Personnes, pronoms, sujets -> Jaune (#FFD600)
    Subject,
    /// Actions, verbes -> Vert (#00E676)
    Verb,
    /// Objets, besoins concrets -> Bleu (#00B0FF)
    Object,
    /// Douleurs, sensations, sentiments -> Rose / Magenta (#FF4081)
    Feeling,
    /// Mots de liaison, réponses sociales -> Orange / Blanc (#FF9100)
    Social,
}

impl FitzgeraldCategory {
    pub fn display_name(&self) -> String {
        let key = match self {
            Self::Subject => "aac.category_subject",
            Self::Verb => "aac.category_verb",
            Self::Object => "aac.category_object",
            Self::Feeling => "aac.category_feeling",
            Self::Social => "aac.category_social",
        };
        t!(key).to_string()
    }

    pub fn bg_color(&self) -> egui::Color32 {
        match self {
            Self::Subject => egui::Color32::from_rgb(255, 214, 0),
            Self::Verb => egui::Color32::from_rgb(0, 230, 118),
            Self::Object => egui::Color32::from_rgb(0, 176, 255),
            Self::Feeling => egui::Color32::from_rgb(255, 64, 129),
            Self::Social => egui::Color32::from_rgb(255, 145, 0),
        }
    }

    pub fn text_color(&self) -> egui::Color32 {
        match self {
            Self::Subject => egui::Color32::BLACK,
            Self::Verb => egui::Color32::BLACK,
            Self::Object => egui::Color32::BLACK,
            Self::Feeling => egui::Color32::WHITE,
            Self::Social => egui::Color32::BLACK,
        }
    }
}

#[derive(Clone, Debug)]
pub struct AacPictogram {
    pub label: &'static str,
    pub icon: &'static str,
    pub category: FitzgeraldCategory,
}

pub const ARASAAC_PICTOGRAMS: &[AacPictogram] = &[
    // --- 1. Sujets / Pronoms (Jaune) ---
    AacPictogram { label: "Moi / Je", icon: "🙋‍♂️", category: FitzgeraldCategory::Subject },
    AacPictogram { label: "Tu / Toi", icon: "👉", category: FitzgeraldCategory::Subject },
    AacPictogram { label: "Aidant", icon: "🧑‍⚕️", category: FitzgeraldCategory::Subject },
    AacPictogram { label: "Médecin", icon: "👨‍⚕️", category: FitzgeraldCategory::Subject },
    AacPictogram { label: "Famille", icon: "👨‍👩‍👧", category: FitzgeraldCategory::Subject },
    AacPictogram { label: "Nous", icon: "👥", category: FitzgeraldCategory::Subject },

    // --- 2. Actions / Verbes (Vert) ---
    AacPictogram { label: "Vouloir", icon: "🤲", category: FitzgeraldCategory::Verb },
    AacPictogram { label: "Avoir", icon: "✊", category: FitzgeraldCategory::Verb },
    AacPictogram { label: "Boire", icon: "🥤", category: FitzgeraldCategory::Verb },
    AacPictogram { label: "Manger", icon: "🍽️", category: FitzgeraldCategory::Verb },
    AacPictogram { label: "Aider", icon: "🤝", category: FitzgeraldCategory::Verb },
    AacPictogram { label: "Dormir", icon: "😴", category: FitzgeraldCategory::Verb },
    AacPictogram { label: "Respirer", icon: "🫁", category: FitzgeraldCategory::Verb },
    AacPictogram { label: "Tourner", icon: "🔄", category: FitzgeraldCategory::Verb },
    AacPictogram { label: "Arrêter", icon: "🛑", category: FitzgeraldCategory::Verb },
    AacPictogram { label: "Aller", icon: "🚶", category: FitzgeraldCategory::Verb },

    // --- 3. Objets / Besoins (Bleu) ---
    AacPictogram { label: "Eau", icon: "💧", category: FitzgeraldCategory::Object },
    AacPictogram { label: "Chaud", icon: "☕", category: FitzgeraldCategory::Object },
    AacPictogram { label: "Médicament", icon: "💊", category: FitzgeraldCategory::Object },
    AacPictogram { label: "Lit", icon: "🛏️", category: FitzgeraldCategory::Object },
    AacPictogram { label: "Fauteuil", icon: "🦼", category: FitzgeraldCategory::Object },
    AacPictogram { label: "Toilettes", icon: "🚽", category: FitzgeraldCategory::Object },
    AacPictogram { label: "Couverture", icon: "🧣", category: FitzgeraldCategory::Object },
    AacPictogram { label: "Téléphone", icon: "📱", category: FitzgeraldCategory::Object },
    AacPictogram { label: "Musique", icon: "🎵", category: FitzgeraldCategory::Object },
    AacPictogram { label: "Lumière", icon: "💡", category: FitzgeraldCategory::Object },

    // --- 4. Sentiments / Sensations (Rose) ---
    AacPictogram { label: "Mal / Douleur", icon: "😣", category: FitzgeraldCategory::Feeling },
    AacPictogram { label: "Soif", icon: "🥛", category: FitzgeraldCategory::Feeling },
    AacPictogram { label: "Faim", icon: "🍲", category: FitzgeraldCategory::Feeling },
    AacPictogram { label: "Froid", icon: "🥶", category: FitzgeraldCategory::Feeling },
    AacPictogram { label: "Chaleur", icon: "🥵", category: FitzgeraldCategory::Feeling },
    AacPictogram { label: "Fatigué", icon: "🥱", category: FitzgeraldCategory::Feeling },
    AacPictogram { label: "Peur / Inquiet", icon: "😨", category: FitzgeraldCategory::Feeling },
    AacPictogram { label: "Bien / Content", icon: "😊", category: FitzgeraldCategory::Feeling },

    // --- 5. Social / Réponses (Orange) ---
    AacPictogram { label: "Oui", icon: "✅", category: FitzgeraldCategory::Social },
    AacPictogram { label: "Non", icon: "❌", category: FitzgeraldCategory::Social },
    AacPictogram { label: "Merci", icon: "🙏", category: FitzgeraldCategory::Social },
    AacPictogram { label: "S'il vous plaît", icon: "🙇", category: FitzgeraldCategory::Social },
    AacPictogram { label: "Urgent", icon: "🚨", category: FitzgeraldCategory::Social },
    AacPictogram { label: "Maintenant", icon: "⏱️", category: FitzgeraldCategory::Social },
];

#[derive(Clone, Debug)]
pub struct AacBoardState {
    pub selected_symbols: Vec<AacPictogram>,
    pub active_category: Option<FitzgeraldCategory>,
    pub expanded_sentence: Option<String>,
    pub is_expanding: bool,
    pub scanning_enabled: bool,
    pub scan_cat_index: usize,
    pub scan_item_index: usize,
    pub scan_in_items: bool,
    pub last_scan_tick: Instant,
    pub scan_interval: Duration,
}

impl Default for AacBoardState {
    fn default() -> Self {
        Self {
            selected_symbols: Vec::new(),
            active_category: None,
            expanded_sentence: None,
            is_expanding: false,
            scanning_enabled: true,
            scan_cat_index: 0,
            scan_item_index: 0,
            scan_in_items: false,
            last_scan_tick: Instant::now(),
            scan_interval: Duration::from_millis(1400),
        }
    }
}

static AAC_BOARD_STATE: Mutex<Option<AacBoardState>> = Mutex::new(None);
static AAC_EXPANSION_RX: Mutex<Option<StdReceiver<(String, bool)>>> = Mutex::new(None);

/// Moteur d'expansion générative (LLM 0.5B / 1.5B ou modèle distant)
pub fn expand_aac_symbols_with_llm(symbols: &[AacPictogram], lang: &str, clear_after: bool) {
    if symbols.is_empty() {
        return;
    }

    let (tx, rx) = channel();
    if let Ok(mut lock) = AAC_EXPANSION_RX.lock() {
        *lock = Some(rx);
    }

    let symbol_labels: Vec<&str> = symbols.iter().map(|s| s.label).collect();
    let sequence_str = symbol_labels.join(" + ");
    let lang_str = lang.to_string();

    std::thread::spawn(move || {
        // 1. Essai déterministe ultra-rapide (règles de formulation courantes)
        let fast_expansion = match sequence_str.to_lowercase().as_str() {
            s if s.contains("moi") && s.contains("boire") && s.contains("chaud") => {
                Some("Pourrais-je avoir une boisson chaude s'il vous plaît ?".to_string())
            }
            s if s.contains("moi") && s.contains("boire") && s.contains("eau") => {
                Some("J'ai soif, puis-je avoir un verre d'eau s'il vous plaît ?".to_string())
            }
            s if s.contains("moi") && s.contains("dormir") => {
                Some("Je suis fatigué, j'aimerais dormir maintenant.".to_string())
            }
            s if s.contains("moi") && s.contains("mal") => {
                Some("J'ai mal, pourriez-vous m'aider s'il vous plaît ?".to_string())
            }
            s if s.contains("aidant") && s.contains("urgent") => {
                Some("J'ai besoin de l'aide de mon soignant en urgence.".to_string())
            }
            _ => None,
        };

        if let Some(phrase) = fast_expansion {
            let _ = tx.send((phrase, clear_after));
            return;
        }

        // 2. Appel au modèle de langage pour l'expansion générative
        let system_prompt = format!(
            "Tu es un interprète expert de Communication Alternative et Améliorée (CAA). Ton rôle est de prendre une séquence de mots-clés issus de pictogrammes et de la transformer en une phrase complète et naturelle qui exprime l'intention ou l'état d'être de la personne.\n\
             Recolle les mots suivants et formule la phrase la plus probable, polie et fluide en {lang_str}, prête à être prononcée oralement par une personne non-verbale.\n\
             N'ajoute aucune explication ni guillemets, écris uniquement la phrase finale complète."
        );
        let user_prompt = format!("Pictogrammes : [{sequence_str}]\nPhrase reformulée :");

        let config = crate::overlay::resolve_llm_config();

        // Tentative sur le moteur local Llama/Qwen s'il est configuré
        let result = if config.provider == crate::types::LlmProvider::Local {
            // Utiliser le moteur de prédiction du clavier (Qwen 0.5B) s'il est chargé
            if let Ok(engine_guard) = LLM_PREDICTOR_ENGINE.lock() {
                if let Some(engine) = engine_guard.as_ref() {
                    let formatted_prompt = format!(
                        "<|im_start|>system\n{system_prompt}<|im_end|>\n<|im_start|>user\n{user_prompt}<|im_end|>\n<|im_start|>assistant\n"
                    );
                    // Utilise la génération rapide du petit modèle, qui est optimisée.
                    engine.generate_fast(&formatted_prompt, 60)
                } else {
                    // Si le moteur du clavier n'est pas prêt, essayer le moteur principal de l'agent (comportement original)
                    crate::local_llm::LocalLlamaEngine::generate_local_text(&format!("{}\n{}", system_prompt, user_prompt), 60)
                }
            } else {
                Err("Le mutex du moteur de prédiction du clavier est empoisonné.".to_string())
            }
        } else {
            let rt = tokio::runtime::Builder::new_current_thread().enable_all().build();
            match rt {
                Ok(runtime) => runtime.block_on(async {
                    let client = reqwest::Client::builder().timeout(Duration::from_secs(6)).build().map_err(|e| e.to_string())?;
                    let msgs = vec![crate::types::ChatMessage { role: "user".to_string(), content: user_prompt.clone() }];
                    let resp = crate::agent_backend::send_llm_chat_completion(&client, &config, &system_prompt, &msgs, 0.4, 60, false).await.map_err(|e| e.to_string())?;
                    crate::agent_backend::parse_llm_response_text(resp, config.provider).await
                }),
                Err(e) => Err(e.to_string()),
            }
        };

        let final_text = match result {
            Ok(text) if !text.trim().is_empty() => {
                let clean = text.trim().trim_matches(['"', '\'', '`', '«', '»']).trim();
                if !clean.is_empty() {
                    clean.to_string()
                } else {
                    format!("{}. S'il vous plaît.", symbol_labels.join(" "))
                }
            }
            Ok(empty_text) => {
                eprintln!("[AAC Expansion] Réponse LLM vide ou blanche reçue: '{}'. Repli sur la concaténation.", empty_text);
                format!("{}. S'il vous plaît.", symbol_labels.join(" "))
            }
            Err(e) => {
                eprintln!("[AAC Expansion] Erreur LLM lors de l'expansion de symboles : {e}. Repli sur la concaténation.");
                // Repli direct par concaténation soignée
                format!("{}. S'il vous plaît.", symbol_labels.join(" "))
            }
        };

        let _ = tx.send((final_text, clear_after));
    });
}

pub fn get_categories_ordered() -> &'static [FitzgeraldCategory] {
    &[
        FitzgeraldCategory::Subject,
        FitzgeraldCategory::Verb,
        FitzgeraldCategory::Object,
        FitzgeraldCategory::Feeling,
        FitzgeraldCategory::Social,
    ]
}

pub fn get_pictograms_by_category(cat: FitzgeraldCategory) -> Vec<AacPictogram> {
    ARASAAC_PICTOGRAMS.iter().filter(|p| p.category == cat).cloned().collect()
}

pub fn render_aac_board_view(
    ui: &mut egui::Ui,
    dwell_state: &mut DwellState,
    theme: &ThemeColors,
) -> Option<FloatingCardOutput> {
    let mut state = {
        let mut guard = AAC_BOARD_STATE.lock().unwrap_or_else(|e| e.into_inner());
        if guard.is_none() {
            *guard = Some(AacBoardState::default());
        }
        guard.as_ref().cloned().unwrap()
    };

    let mut output_event = None;
    let now = Instant::now();
    let categories = get_categories_ordered();
    let active_cat = state.active_category.unwrap_or(categories[state.scan_cat_index % categories.len()]);
    let items_in_cat = get_pictograms_by_category(active_cat);

    // Réception asynchrone de l'expansion générative
    if let Ok(mut rx_guard) = AAC_EXPANSION_RX.lock() {
        if let Some(ref rx) = *rx_guard {
            if let Ok((expanded, should_clear)) = rx.try_recv() {
                state.expanded_sentence = Some(expanded.clone());
                state.is_expanding = false;
                play_earcon(Earcon::Success);
                output_event = Some(FloatingCardOutput::Speak(expanded));
                if should_clear {
                    state.selected_symbols.clear();
                    state.expanded_sentence = None;
                    state.scan_in_items = false;
                }
                *rx_guard = None;
            }
        }
    }

    // Balayage automatique (Auto-Scan) pour contacteur unique
    if state.scanning_enabled && now.duration_since(state.last_scan_tick) >= state.scan_interval {
        state.last_scan_tick = now;
        if !state.scan_in_items {
            state.scan_cat_index = (state.scan_cat_index + 1) % categories.len();
        } else {
            // Le nombre d'éléments à balayer est : pictogrammes + "effacer" + "valider" + "retour"
            let count = items_in_cat.len() + 3;
            state.scan_item_index = (state.scan_item_index + 1) % count;
        }
        ui.ctx().request_repaint();
    }

    // Entrée contacteur (Espace ou Entrée)
    let switch_hit = ui.input(|i| i.key_pressed(egui::Key::Space) || i.key_pressed(egui::Key::Enter));
    if switch_hit {
        state.last_scan_tick = now;
        if !state.scan_in_items {
            state.scan_in_items = true;
            state.scan_item_index = 0;
            play_earcon(Earcon::Navigation);
        } else {
            let idx = state.scan_item_index;
            let items_count = items_in_cat.len();

            if idx < items_count {
                // Un pictogramme a été sélectionné
                if let Some(item) = items_in_cat.get(idx) {
                    state.selected_symbols.push(item.clone());
                    play_earcon(Earcon::Success);
                    // Amélioration accessibilité : sauter directement sur "Valider"
                    state.scan_item_index = items_count;
                }
            } else if idx == items_count {
                // "Valider et Parler" a été sélectionné
                if !state.selected_symbols.is_empty() {
                    if let Some(ref expanded) = state.expanded_sentence {
                        output_event = Some(FloatingCardOutput::Speak(expanded.clone()));
                        state.selected_symbols.clear();
                        state.expanded_sentence = None;
                        state.scan_in_items = false;
                        play_earcon(Earcon::Success);
                    } else {
                        state.is_expanding = true;
                        let cur_lang = get_active_app_language();
                        expand_aac_symbols_with_llm(&state.selected_symbols, &cur_lang, true);
                    }
                }
            } else if idx == items_count + 1 {
                // "Effacer dernier" a été sélectionné
                if state.selected_symbols.pop().is_some() {
                    state.expanded_sentence = None;
                    play_earcon(Earcon::Navigation);
                }
            } else { // idx == items_count + 2
                // "Retour" a été sélectionné
                state.scan_in_items = false;
                state.scan_item_index = 0;
                play_earcon(Earcon::Navigation);
            }
        }
    }

    if ui.input(|i| i.key_pressed(egui::Key::Escape)) {
        state.scan_in_items = false;
        play_earcon(Earcon::Navigation);
    }

    // 1. Bandeau supérieur : Phrase composée en pictogrammes
    egui::Frame::new()
        .fill(egui::Color32::from_rgb(14, 16, 22))
        .stroke(egui::Stroke::new(1.2, theme.accent))
        .corner_radius(egui::CornerRadius::same(8))
        .inner_margin(egui::Margin::symmetric(8, 6))
        .show(ui, |ui| {
            ui.horizontal_wrapped(|ui| {
                ui.spacing_mut().item_spacing = egui::vec2(6.0, 4.0);
                if state.selected_symbols.is_empty() { ui.label(egui::RichText::new(t!("aac.placeholder")).italics().size(13.0).color(theme.text_secondary)); }
 else {
                    for (idx, sym) in state.selected_symbols.iter().enumerate() {
                        let tag = egui::Button::new(
                            egui::RichText::new(format!("{} {}", sym.icon, sym.label))
                                .size(12.0)
                                .color(sym.category.text_color())
                                .strong(),
                        )
                        .fill(sym.category.bg_color())
                        .corner_radius(egui::CornerRadius::same(6));
                        if ui.add(tag).on_hover_text(t!("aac.remove_pictogram_tooltip")).clicked() {
                            state.selected_symbols.remove(idx);
                            play_earcon(Earcon::Navigation);
                            break;
                        }
                    }
                }
            });
        });

    if let Some(ref expanded) = state.expanded_sentence {
        ui.add_space(3.0);
        ui.horizontal(|ui| {
            ui.label(egui::RichText::new("✨").size(14.0));
            ui.label(egui::RichText::new(format!("« {expanded} »")).size(14.0).strong().color(egui::Color32::from_rgb(255, 230, 120)));
        });
    }

    ui.add_space(4.0);

    // 2. Boutons d'action (Oraliser, Expansion IA, Effacer, Basculer Clavier)
    ui.horizontal(|ui| {
        ui.spacing_mut().item_spacing.x = 6.0;

        // Bouton Expansion IA
        let ai_expand_btn = egui::Button::new(
            egui::RichText::new(t!("aac.expand_ai"))
                .size(12.0)
                .color(theme.control_bg)
                .strong(),
        )
        .fill(theme.accent)
        .corner_radius(egui::CornerRadius::same(6));
        let ai_id = ui.make_persistent_id("dwell_aac_ai_expand");
        let ai_resp = ui.add_enabled(!state.selected_symbols.is_empty(), ai_expand_btn);
        if (ai_resp.clicked() || apply_dwell_to_response(ui, dwell_state, ai_id, &ai_resp)) && !state.selected_symbols.is_empty() {
            state.is_expanding = true;
            let cur_lang = get_active_app_language();
            expand_aac_symbols_with_llm(&state.selected_symbols, &cur_lang, false);
            play_earcon(Earcon::Navigation);
        }

        // Bouton Parler direct
        let speak_btn = ui.button(egui::RichText::new(t!("aac.speak_direct")).size(12.0).strong());
        let spk_id = ui.make_persistent_id("dwell_aac_speak_raw");
        if (speak_btn.clicked() || apply_dwell_to_response(ui, dwell_state, spk_id, &speak_btn)) && !state.selected_symbols.is_empty() {
            let raw_text = state.selected_symbols.iter().map(|s| s.label).collect::<Vec<_>>().join(", ");
            output_event = Some(FloatingCardOutput::Speak(raw_text));
            play_earcon(Earcon::Success);
        }

        // Effacer
        let clear_btn = ui.button(t!("aac.clear_all"));
        let clr_id = ui.make_persistent_id("dwell_aac_clear");
        if clear_btn.clicked() || apply_dwell_to_response(ui, dwell_state, clr_id, &clear_btn) {
            state.selected_symbols.clear();
            state.expanded_sentence = None;
            play_earcon(Earcon::MicClose);
        }

        // Bascule vers le clavier virtuel alphabétique
        ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
            let kbd_btn = ui.button(egui::RichText::new(t!("aac.switch_to_alphabetical")).size(11.5).color(theme.text_secondary));
            let kbd_id = ui.make_persistent_id("dwell_aac_to_alphabet");
            if kbd_btn.clicked() || apply_dwell_to_response(ui, dwell_state, kbd_id, &kbd_btn) {
                output_event = Some(FloatingCardOutput::Prompt("clavier virtuel".to_string()));
            }
        });
    });

    ui.add_space(4.0);
    ui.separator();
    ui.add_space(4.0);

    // 3. Onglets de Catégories Fitzgerald (Jaune, Vert, Bleu, Rose, Orange)
    ui.horizontal(|ui| {
        ui.spacing_mut().item_spacing.x = 6.0;
        for (cat_idx, &cat) in categories.iter().enumerate() {
            let is_selected = state.active_category == Some(cat) || (state.active_category.is_none() && state.scan_cat_index == cat_idx);
            let is_scanned_cat = state.scanning_enabled && !state.scan_in_items && state.scan_cat_index == cat_idx;

            let btn = egui::Button::new(
                egui::RichText::new(cat.display_name())
                    .size(12.0)
                    .color(if is_selected { cat.text_color() } else { egui::Color32::WHITE })
                    .strong(),
            )
            .fill(if is_selected { cat.bg_color() } else { theme.control_bg })
            .stroke(egui::Stroke::new(if is_scanned_cat { 3.0 } else { 1.0 }, if is_scanned_cat { egui::Color32::WHITE } else { theme.border }))
            .corner_radius(egui::CornerRadius::same(8));

            let resp = ui.add(btn);
            let cat_dwell_id = ui.make_persistent_id(format!("dwell_aac_cat_{cat_idx}"));
            if resp.clicked() || apply_dwell_to_response(ui, dwell_state, cat_dwell_id, &resp) {
                state.active_category = Some(cat);
                state.scan_cat_index = cat_idx;
                state.scan_in_items = false;
                play_earcon(Earcon::Navigation);
            }
        }
    });

    ui.add_space(6.0);

    // 4. Grille de pictogrammes
    let grid_id = ui.id().with("aac_pictogram_grid");
    egui::ScrollArea::vertical().id_salt(grid_id).show(ui, |ui| {
        egui::Grid::new(grid_id.with("inner_grid"))
            .num_columns(5)
            .spacing([6.0, 6.0])
            .show(ui, |ui| {
                for (item_idx, item) in items_in_cat.iter().enumerate() {
                    if item_idx > 0 && item_idx % 5 == 0 {
                        ui.end_row();
                    }

                    let is_scanned_item = state.scanning_enabled && state.scan_in_items && state.scan_item_index == item_idx;
                    let item_id = ui.make_persistent_id(format!("dwell_aac_item_{item_idx}"));

                    let picto_btn = egui::Button::new(
                        egui::RichText::new(format!("{}\n{}", item.icon, item.label))
                            .size(14.0)
                            .color(item.category.text_color())
                            .strong(),
                    )
                    .min_size(egui::vec2(110.0, 70.0))
                    .wrap()
                    .fill(item.category.bg_color())
                    .stroke(egui::Stroke::new(if is_scanned_item { 3.5 } else { 1.5 }, if is_scanned_item { egui::Color32::WHITE } else { egui::Color32::from_rgba_unmultiplied(0, 0, 0, 80) }))
                    .corner_radius(egui::CornerRadius::same(10));

                    let resp = ui.add(picto_btn);
                    if resp.clicked() || apply_dwell_to_response(ui, dwell_state, item_id, &resp) {
                        state.selected_symbols.push(item.clone());
                        play_earcon(Earcon::Success);
                        // Amélioration accessibilité : sauter directement sur "Valider"
                        state.scan_item_index = items_in_cat.len();
                    }
                }

                // --- Boutons d'action à la fin de la catégorie ---

                // 1. Bouton "Valider et Parler"
                let speak_idx = items_in_cat.len();
                if speak_idx > 0 && speak_idx % 5 == 0 { ui.end_row(); }
                let is_scanned_speak = state.scanning_enabled && state.scan_in_items && state.scan_item_index == speak_idx;
                let speak_id = ui.make_persistent_id("dwell_aac_item_speak_validate");
                let speak_btn = egui::Button::new(egui::RichText::new(t!("aac.btn_validate_speak")).size(13.0).strong())
                    .min_size(egui::vec2(110.0, 70.0)).wrap().fill(theme.control_bg)
                    .stroke(egui::Stroke::new(if is_scanned_speak { 3.5 } else { 1.5 }, if is_scanned_speak { egui::Color32::from_rgb(0, 230, 118) } else { theme.border }))
                    .corner_radius(egui::CornerRadius::same(10));

                let speak_resp = ui.add_enabled(!state.selected_symbols.is_empty(), speak_btn);
                if (speak_resp.clicked() || apply_dwell_to_response(ui, dwell_state, speak_id, &speak_resp)) && !state.selected_symbols.is_empty() {
                    if let Some(ref expanded) = state.expanded_sentence {
                        output_event = Some(FloatingCardOutput::Speak(expanded.clone()));
                        state.selected_symbols.clear();
                        state.expanded_sentence = None;
                        state.scan_in_items = false;
                        play_earcon(Earcon::Success);
                    } else {
                        state.is_expanding = true;
                        let cur_lang = get_active_app_language();
                        expand_aac_symbols_with_llm(&state.selected_symbols, &cur_lang, true);
                    }
                }

                // 2. Bouton "Effacer dernier" (Backspace)
                let clear_last_idx = items_in_cat.len() + 1;
                if clear_last_idx > 0 && clear_last_idx % 5 == 0 { ui.end_row(); }
                let is_scanned_clear = state.scanning_enabled && state.scan_in_items && state.scan_item_index == clear_last_idx;
                let clear_id = ui.make_persistent_id("dwell_aac_item_clear_last");let clear_btn = egui::Button::new(egui::RichText::new(t!("aac.btn_clear_last")).size(13.0).strong())
                    .min_size(egui::vec2(110.0, 70.0)).wrap().fill(theme.control_bg)
                    .stroke(egui::Stroke::new(if is_scanned_clear { 3.5 } else { 1.5 }, if is_scanned_clear { egui::Color32::WHITE } else { theme.border }))
                    .corner_radius(egui::CornerRadius::same(10));

                let clear_resp = ui.add_enabled(!state.selected_symbols.is_empty(), clear_btn);
                if (clear_resp.clicked() || apply_dwell_to_response(ui, dwell_state, clear_id, &clear_resp)) && !state.selected_symbols.is_empty() {
                    if state.selected_symbols.pop().is_some() {
                        state.expanded_sentence = None;
                        play_earcon(Earcon::Navigation);
                    }
                }

                // 3. Bouton "Retour"
                let back_idx = items_in_cat.len() + 2;
                if back_idx > 0 && back_idx % 5 == 0 { ui.end_row(); }
                let is_scanned_back = state.scanning_enabled && state.scan_in_items && state.scan_item_index == back_idx;
                let back_id = ui.make_persistent_id("dwell_aac_item_back");
                let back_btn = egui::Button::new(egui::RichText::new(t!("aac.btn_back_categories")).size(13.0).strong())
                    .min_size(egui::vec2(110.0, 70.0)).wrap().fill(theme.control_bg)
                    .stroke(egui::Stroke::new(if is_scanned_back { 3.5 } else { 1.5 }, if is_scanned_back { egui::Color32::WHITE } else { theme.border }))
                    .corner_radius(egui::CornerRadius::same(10));

                let back_resp = ui.add(back_btn);
                if back_resp.clicked() || apply_dwell_to_response(ui, dwell_state, back_id, &back_resp) {
                    state.scan_in_items = false;
                    state.scan_item_index = 0;
                    play_earcon(Earcon::Navigation);
                }
            });
    });

    if let Ok(mut guard) = AAC_BOARD_STATE.lock() {
        *guard = Some(state);
    }

    output_event
}

pub fn get_keyboard_model_path() -> PathBuf {
    if let Ok(p) = std::env::var("KEYBOARD_MODEL_PATH") {
        let pb = PathBuf::from(p.trim());
        if !p.trim().is_empty() {
            return pb;
        }
    }
    if let Some(p) = read_env_var_from_file(".env", "KEYBOARD_MODEL_PATH") {
        let pb = PathBuf::from(p.trim());
        if !p.trim().is_empty() {
            return pb;
        }
    }

    let direct_models = PathBuf::from("models").join(KEYBOARD_MODEL_FILENAME);
    if direct_models.is_file() {
        return direct_models;
    }
    let base_dir = PathBuf::from("models").join("keyboard");
    base_dir.join(KEYBOARD_MODEL_FILENAME)
}

pub fn is_keyboard_model_installed() -> bool {
    let path = get_keyboard_model_path();
    path.is_file() && path.metadata().map_or(0, |m| m.len()) > 100_000_000
}

pub fn ensure_keyboard_model_downloaded(event_tx: &StdSender<AgentEvent>) -> Result<PathBuf, String> {
    let path = get_keyboard_model_path();
    if is_keyboard_model_installed() {
        KEYBOARD_MODEL_LOADED.store(true, Ordering::Relaxed);
        init_llm_predictor_engine();
        return Ok(path);
    }

    if KEYBOARD_MODEL_DOWNLOADING.swap(true, Ordering::SeqCst) {
        return Ok(path);
    }

    let target_dir = path.parent().unwrap_or_else(|| Path::new("models")).to_path_buf();
    let _ = std::fs::create_dir_all(&target_dir);
    let temp_path = target_dir.join(format!("{}.tmp", KEYBOARD_MODEL_FILENAME));

    let candidate_urls = [
        KEYBOARD_MODEL_HF_URL,
        "https://github.com/anonympins/libertide/releases/latest/download/text_prediction.onnx",
    ];

    let rt = tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()
        .map_err(|e| format!("Runtime Tokio: {e}"))?;

    let client = reqwest::Client::builder()
        .timeout(Duration::from_secs(300))
        .build()
        .map_err(|e| format!("Client HTTP: {e}"))?;

    let mut downloaded = false;
    for url in &candidate_urls {
        println!("[Keyboard Model] Téléchargement de Qwen 2.5 0.5B ({url}) ...");
        let event_tx_clone = event_tx.clone();
        let temp_path_thread = temp_path.clone();
        let req_url = url.to_string();

        let res = rt.block_on(async {
            use tokio::io::AsyncWriteExt;
            let res = client.get(&req_url).send().await.map_err(|e| e.to_string())?;
            if !res.status().is_success() {
                return Err(format!("Statut HTTP {}", res.status()));
            }

            let total_bytes = res.content_length().unwrap_or(398_000_000);
            let mut file = tokio::fs::File::create(&temp_path_thread).await.map_err(|e| e.to_string())?;
            let mut count: u64 = 0;
            let start_time = Instant::now();
            let mut last_update = Instant::now();
            let mut stream = res;

            while let Some(chunk) = stream.chunk().await.map_err(|e| e.to_string())? {
                file.write_all(&chunk).await.map_err(|e| e.to_string())?;
                count += chunk.len() as u64;

                if last_update.elapsed() >= Duration::from_millis(80) || count == total_bytes {
                    last_update = Instant::now();
                    let total_mb = total_bytes as f32 / (1024.0 * 1024.0);
                    let downloaded_mb = count as f32 / (1024.0 * 1024.0);
                    let elapsed = start_time.elapsed().as_secs_f32().max(0.001);
                    let speed_mb_s = downloaded_mb / elapsed;
                    let percent = (count as f32 / total_bytes as f32) * 100.0;

                    let _ = event_tx_clone.send(AgentEvent::DownloadProgress {
                        label: "Modèle Prédictif Qwen 2.5 0.5B (GGUF)".to_string(),
                        percent,
                        downloaded_mb,
                        total_mb,
                        speed_mb_s,
                    });
                }
            }
            file.flush().await.map_err(|e| e.to_string())?;
            Ok::<(), String>(())
        });

        if res.is_ok() && temp_path.is_file() {
            if std::fs::rename(&temp_path, &path).is_ok() {
                println!("[Keyboard Model] Modèle prêt dans {}", path.display());
                downloaded = true;
                break;
            }
        }
        let _ = std::fs::remove_file(&temp_path);
    }

    KEYBOARD_MODEL_DOWNLOADING.store(false, Ordering::SeqCst);

    if downloaded || is_keyboard_model_installed() {
        KEYBOARD_MODEL_LOADED.store(true, Ordering::Relaxed);
        init_llm_predictor_engine();
        Ok(path)
    } else {
        Err("Échec du téléchargement du modèle Qwen 2.5 0.5B pour le clavier.".to_string())
    }
}

pub fn init_llm_predictor_engine() {
    let mut guard = match LLM_PREDICTOR_ENGINE.lock() {
        Ok(g) => g,
        Err(_) => return,
    };
    if guard.is_some() {
        return;
    }
    let model_path = get_keyboard_model_path();
    if !model_path.is_file() {
        return;
    }

    println!("[Keyboard Model] Chargement de Qwen 2.5 0.5B en mémoire...");
    let gpu_layers = std::env::var("KEYBOARD_LLM_GPU_LAYERS")
        .ok()
        .and_then(|s| s.trim().parse::<u32>().ok())
        .unwrap_or(24);

    match crate::local_llm::LocalLlamaEngine::load_with_layers(&model_path, gpu_layers) {
        Ok(engine) => {
            println!("[Keyboard Model] Qwen 2.5 0.5B prêt pour la prédiction de texte !");
            *guard = Some(engine);
            spawn_llm_prediction_worker();
        }
        Err(e) => {
            eprintln!("[Keyboard Model] Erreur chargement Qwen 2.5 0.5B : {e}");
        }
    }
}

fn spawn_llm_prediction_worker() {
    let mut tx_guard = match LLM_REQUEST_TX.lock() {
        Ok(g) => g,
        Err(_) => return,
    };
    if tx_guard.is_some() {
        return;
    }

    let (tx, rx): (StdSender<(String, String)>, StdReceiver<(String, String)>) = channel();
    *tx_guard = Some(tx);

    std::thread::Builder::new()
        .name("keyboard_llm_worker".to_string())
        .spawn(move || {
            while let Ok((mut lang, mut text)) = rx.recv() {
                // Coalescing : ne garder que la toute dernière frappe
                while let Ok((next_lang, next_text)) = rx.try_recv() {
                    lang = next_lang;
                    text = next_text;
                }

                let trimmed = text.trim();
                if trimmed.len() < 2 {
                    continue;
                }

                let sys_prompt = match lang.as_str() {
                    "en" => "You are an assistive AAC keyboard predictor. Given the user's beginning of sentence, provide 3 to 4 likely continuations (either next word or completed phrases), separated by commas. Output only the comma-separated options without explanation or quotes.",
                    _ => "Tu es un prédicteur de saisie pour clavier virtuel CAA. À partir du début de phrase fourni par l'utilisateur, propose 3 à 4 suites logiques ou fins de phrase complètes naturelles, séparées par des virgules. Renvoie uniquement les propositions séparées par des virgules, sans explications ni guillemets.",
                };

                let formatted_prompt = format!(
                    "<|im_start|>system\n{sys_prompt}<|im_end|>\n<|im_start|>user\n{text}<|im_end|>\n<|im_start|>assistant\n"
                );

                let generated_opt = {
                    let engine_guard = LLM_PREDICTOR_ENGINE.lock().ok();
                    engine_guard.as_ref().and_then(|opt| opt.as_ref().map(|eng| eng.generate_fast(&formatted_prompt, 40)))
                };

                if let Some(Ok(raw_reply)) = generated_opt {
                    let clean_reply = raw_reply.replace('\n', " ");
                    let candidates: Vec<String> = clean_reply
                        .split(|c: char| c == ',' || c == ';' || c == '|')
                        .map(|s| s.trim().trim_matches(['"', '\'', '.', '-']).trim())
                        .filter(|s| !s.is_empty() && s.len() >= 2 && s.len() <= 35)
                        .take(4)
                        .map(|s| s.to_string())
                        .collect();

                    if !candidates.is_empty() {
                        if let Ok(mut cache) = ASYNC_PREDICTIONS_CACHE.lock() {
                            *cache = Some((text, candidates));
                        }
                    }
                }
            }
        })
        .ok();
}

pub fn init_keyboard_model_async(event_tx: StdSender<AgentEvent>) {
    std::thread::Builder::new()
        .name("keyboard_model_loader".to_string())
        .spawn(move || {
            let _ = ensure_keyboard_model_downloaded(&event_tx);
        })
        .ok();
}

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

pub fn reset_keyboard_scan() {
    let mut guard = KEYBOARD_STATE.lock().unwrap_or_else(|e| e.into_inner());
    if let Some(ref mut state) = *guard {
        state.scan_stage = ScanningStage::Row;
        state.current_row = 0;
        state.current_col = 0;
        state.last_scan_tick = Instant::now();
    } else {
        *guard = Some(VirtualKeyboardState::default());
    }
}

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
    // Déclencher la prédiction asynchrone par Qwen 2.5 0.5B dès que du texte est présent
    if !current_text.trim().is_empty() {
        if let Ok(guard) = LLM_REQUEST_TX.lock() {
            if let Some(ref tx) = *guard {
                let _ = tx.send((lang.to_string(), current_text.to_string()));
            }
        }
    }

    // Vérifier si des suggestions enrichies générées par Qwen sont prêtes pour ce texte
    let mut llm_suggestions = None;
    if let Ok(cache) = ASYNC_PREDICTIONS_CACHE.lock() {
        if let Some((ref cached_text, ref list)) = *cache {
            if cached_text == current_text || current_text.starts_with(cached_text.as_str()) {
                llm_suggestions = Some(list.clone());
            }
        }
    }

    let last_word = current_text
        .split_whitespace()
        .last()
        .unwrap_or("");

    let ends_with_space = current_text.ends_with(' ');

    // 1. Si on vient de valider un mot (espace) : Next-Word Prediction
    if ends_with_space && !last_word.is_empty() {
        if let Some(sugs) = llm_suggestions {
            return sugs;
        }

        let lower_last = last_word.to_lowercase();
        let bigrams = get_bigrams(lang);

        let words: Vec<&str> = current_text.split_whitespace().collect();
        let two_words = if words.len() >= 2 {
            format!("{} {}", words[words.len() - 2].to_lowercase(), lower_last)
        } else {
            String::new()
        };

        if let Some((_, next_words)) = bigrams.iter().find(|(prefix, _)| *prefix == two_words.as_str()) {
            return next_words.iter().take(6).map(|s| s.to_string()).collect();
        }
        if let Some((_, next_words)) = bigrams.iter().find(|(prefix, _)| *prefix == lower_last.as_str()) {
            return next_words.iter().take(6).map(|s| s.to_string()).collect();
        }
        return get_default_predictions(lang);
    } else if current_text.trim().is_empty() {
        return get_default_predictions(lang);
    }

    // 2. Si on est en train de taper un mot : autocomplétion par préfixe
    let lower_prefix = last_word.to_lowercase();
    let dict = get_dictionary(lang);
    let matches: Vec<String> = dict
        .iter()
        .filter(|w| w.starts_with(&lower_prefix) && **w != lower_prefix.as_str())
        .take(6)
        .map(|w| w.to_string())
        .collect();

    if let Some(sugs) = llm_suggestions {
        let mut combined = Vec::new();
        for s in sugs {
            if !combined.contains(&s) {
                combined.push(s);
            }
            if combined.len() >= 3 {
                break;
            }
        }
        for m in matches {
            if !combined.contains(&m) {
                combined.push(m);
            }
            if combined.len() >= 6 {
                break;
            }
        }
        return combined;
    }

    if matches.is_empty() {
        get_default_predictions(lang)
    } else {
        matches
    }
}

/// Fusionne intelligemment une suggestion (mot ou fin de phrase) avec le texte en cours
pub fn apply_prediction_to_buffer(buffer: &mut String, pred: &str) {
    let clean_pred = pred.trim().trim_matches(['"', '\'', '`']);
    if clean_pred.is_empty() {
        return;
    }

    let ends_with_space = buffer.ends_with(' ');
    let trimmed_buf = buffer.trim_end();

    if trimmed_buf.is_empty() {
        buffer.clear();
        buffer.push_str(clean_pred);
        buffer.push(' ');
        return;
    }

    let lower_buf = trimmed_buf.to_lowercase();
    let lower_pred = clean_pred.to_lowercase();

    // 1. Si pred commence directement par tout le buffer (complétion de phrase entière)
    // Ex: buffer = "J'ai" ou "J'ai ", pred = "J'ai soif"
    if lower_pred.starts_with(&lower_buf) {
        buffer.clear();
        buffer.push_str(clean_pred);
        buffer.push(' ');
        return;
    }

    // 2. Recherche du plus grand chevauchement (overlap) entre la fin de buffer et le début de pred
    let buf_chars: Vec<char> = trimmed_buf.chars().collect();
    let pred_chars: Vec<char> = clean_pred.chars().collect();
    let lower_buf_chars: Vec<char> = lower_buf.chars().collect();
    let lower_pred_chars: Vec<char> = lower_pred.chars().collect();

    let mut best_overlap_len = 0;

    let min_start_idx = if ends_with_space {
        0
    } else {
        buf_chars.iter().rposition(|&c| c == ' ').map(|p| p + 1).unwrap_or(0)
    };

    for start in 0..buf_chars.len() {
        if start < min_start_idx && start > 0 && buf_chars[start - 1] != ' ' {
            continue;
        }

        let suffix_len = buf_chars.len() - start;
        if suffix_len <= pred_chars.len() {
            let buf_slice = &lower_buf_chars[start..];
            let pred_slice = &lower_pred_chars[..suffix_len];
            if buf_slice == pred_slice {
                if ends_with_space && start > 0 && buf_chars[start - 1] != ' ' {
                    continue;
                }
                best_overlap_len = suffix_len;
                break;
            }
        }
    }

    let mut adjusted_pred = clean_pred.to_string();

    if best_overlap_len > 0 {
        let keep_char_count = buf_chars.len() - best_overlap_len;
        let prefix: String = buf_chars[..keep_char_count].iter().collect();

        // Harmonisation de la casse en milieu de phrase
        if !prefix.is_empty() {
            let trimmed_p = prefix.trim_end();
            let is_new_sentence = trimmed_p.ends_with('.') || trimmed_p.ends_with('!') || trimmed_p.ends_with('?');
            if !is_new_sentence {
                if let Some(first_char) = adjusted_pred.chars().next() {
                    if first_char.is_uppercase() {
                        let last_user_word = trimmed_buf.split_whitespace().last().unwrap_or("");
                        let user_is_caps = !last_user_word.is_empty() && last_user_word.chars().all(|c| c.is_uppercase());
                        if !user_is_caps {
                            let rest: String = adjusted_pred.chars().skip(1).collect();
                            adjusted_pred = format!("{}{rest}", first_char.to_lowercase());
                        }
                    }
                }
            }
        }

        buffer.clear();
        buffer.push_str(&prefix);
        if !buffer.is_empty() && !buffer.ends_with(' ') && !buffer.ends_with('\'') {
            buffer.push(' ');
        }
        buffer.push_str(&adjusted_pred);
        buffer.push(' ');
    } else {
        if !ends_with_space {
            let last_word = trimmed_buf.split_whitespace().last().unwrap_or("");
            let lower_last = last_word.to_lowercase();
            let first_pred_word = clean_pred.split_whitespace().next().unwrap_or("").to_lowercase();

            if first_pred_word.starts_with(&lower_last) {
                if let Some(last_space_idx) = buf_chars.iter().rposition(|&c| c == ' ') {
                    let prefix: String = buf_chars[..=last_space_idx].iter().collect();
                    buffer.clear();
                    buffer.push_str(&prefix);
                } else {
                    buffer.clear();
                }
                buffer.push_str(&adjusted_pred);
                buffer.push(' ');
                return;
            }
            buffer.push(' ');
        }
        buffer.push_str(&adjusted_pred);
        buffer.push(' ');
    }
}

pub fn render_keyboard_view(
    ui: &mut egui::Ui,
    dwell_state: &mut DwellState,
    theme: &ThemeColors,
) -> Option<FloatingCardOutput> {
    let mut state = {
        let mut guard = KEYBOARD_STATE.lock().unwrap_or_else(|e| e.into_inner());
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
    if &*rust_i18n::locale() != active_lang.as_str() {
        rust_i18n::set_locale(&active_lang);
    }

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
        .inner_margin(egui::Margin::symmetric(10, 6))
        .show(ui, |ui| {
            ui.horizontal(|ui| {
                let placeholder = t!("keyboard.placeholder").to_string();
                let display_text = if state.text_buffer.is_empty() {
                    placeholder.as_str()
                } else {
                    &state.text_buffer
                };
                let color = if state.text_buffer.is_empty() {
                    egui::Color32::from_gray(140)
                } else {
                    egui::Color32::WHITE
                };
                ui.label(egui::RichText::new(display_text).size(17.0).color(color).strong());
            });
        });

    ui.add_space(3.0);

    // 2. Rangée des prédictions de mots
    let is_row_0_active = state.scanning_enabled && state.current_row == 0;
    ui.horizontal(|ui| {
        for (col_idx, pred) in predictions.iter().enumerate() {
            let is_scanned_key = is_row_0_active && state.scan_stage == ScanningStage::Key && state.current_col == col_idx;
            let is_row_selected = is_row_0_active && state.scan_stage == ScanningStage::Row;

            let btn = egui::Button::new(egui::RichText::new(pred).size(13.0).color(theme.accent_text).strong())
                .fill(if is_scanned_key { theme.accent } else if is_row_selected { theme.tab_active_bg } else { theme.control_bg })
                .stroke(egui::Stroke::new(if is_scanned_key { 2.5 } else { 1.0 }, theme.border))
                .corner_radius(egui::CornerRadius::same(6));

            let resp = ui.add(btn);
            let pred_dwell_id = ui.make_persistent_id(format!("dwell_pred_{col_idx}"));
            let clicked = resp.clicked() || (is_scanned_key && trigger_selected_key) || apply_dwell_to_response(ui, dwell_state, pred_dwell_id, &resp);

            if clicked {
                apply_prediction_to_buffer(&mut state.text_buffer, pred);
                play_earcon(Earcon::Success);
                state.scan_stage = ScanningStage::Row;
            }
        }
    });

    ui.add_space(3.0);
    ui.separator();
    ui.add_space(3.0);

    // 3. Grille des lettres (rangées adaptées à la langue)
    let total_w = ui.available_width();
    let max_cols_in_grid = letter_rows.iter().map(|r| r.len()).max().unwrap_or(8);
    let key_w = ((total_w - (max_cols_in_grid as f32 - 1.0) * 5.0) / max_cols_in_grid as f32).max(28.0);
    let key_h = 34.0;

    for (r_idx, row) in letter_rows.iter().enumerate() {
        let row_num = r_idx + 1;
        let is_current_row = state.scanning_enabled && state.current_row == row_num;
        let is_row_highlight = is_current_row && state.scan_stage == ScanningStage::Row;

        ui.horizontal(|ui| {
            ui.spacing_mut().item_spacing.x = 5.0;
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
        ui.add_space(3.0);
    }

    ui.separator();
    ui.add_space(3.0);

    // Bouton de bascule vers la grille de pictogrammes CAA
    ui.horizontal(|ui| {
        let aac_switch_btn = ui.button(egui::RichText::new(t!("keyboard.switch_to_aac")).size(11.5).color(theme.accent).strong());
        if aac_switch_btn.clicked() {
            output_event = Some(FloatingCardOutput::Prompt("mode caa".to_string()));
        }
    });

    // 4. Rangée des actions (Espace, Effacer, Parler, Tchat, Insérer, Vider) localisée
    let is_actions_row = state.scanning_enabled && state.current_row == (total_rows - 1);
    let is_action_row_selected = is_actions_row && state.scan_stage == ScanningStage::Row;

    ui.horizontal(|ui| {
        ui.spacing_mut().item_spacing.x = 5.0;

        let actions: [(String, usize); 6] = [
            (t!("keyboard.btn_space").to_string(), 0),
            (t!("keyboard.btn_delete").to_string(), 1),
            (t!("keyboard.btn_speak").to_string(), 2),
            (t!("keyboard.btn_chat").to_string(), 3),
            (t!("keyboard.btn_insert").to_string(), 4),
            (t!("keyboard.btn_clear").to_string(), 5),
        ];

        for (label, col_idx) in &actions {
            let col_idx = *col_idx;
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

            let btn = egui::Button::new(egui::RichText::new(label.as_str()).size(12.5).color(txt_col).strong())
                .fill(bg)
                .stroke(egui::Stroke::new(if is_action_key { 2.5 } else { 1.0 }, theme.border))
                .corner_radius(egui::CornerRadius::same(6))
                .min_size(egui::vec2(66.0, 32.0));

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
                            set_clipboard_text(&state.text_buffer);
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

    ui.add_space(3.0);
    ui.horizontal(|ui| {
        ui.checkbox(&mut state.scanning_enabled, t!("keyboard.auto_scan"));
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

    let mut guard = KEYBOARD_STATE.lock().unwrap_or_else(|e| e.into_inner());
    *guard = Some(state);

    output_event
}