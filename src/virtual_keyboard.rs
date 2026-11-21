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