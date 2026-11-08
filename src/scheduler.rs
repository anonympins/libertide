use std::sync::Mutex;
use std::sync::mpsc::Sender as StdSender;
use std::sync::atomic::{AtomicBool, AtomicU64, AtomicUsize, Ordering};
use std::time::{Duration, Instant};
use tokio::sync::mpsc::UnboundedSender;

use crate::automation::{AGENT_BUSY, IS_DICTATION_ACTIVE, IS_EMERGENCY_STOPPED, IS_USER_TYPING};
use crate::pattern::clean_words;
use crate::types::{AgentCommand, AgentEvent, TtsCommand};

#[derive(Clone, Debug)]
pub struct ActiveScheduledTask {
    pub id: u64,
    pub label: String,
    pub prompt: String,
    pub target_instant: Instant,
    pub total_duration: Duration,
    pub cron_expr: Option<String>,
}

pub static SCHEDULE_GENERATION: AtomicU64 = AtomicU64::new(0);
pub static SCHEDULED_TASKS_COUNT: AtomicUsize = AtomicUsize::new(0);
static NEXT_TASK_ID: AtomicU64 = AtomicU64::new(1);
static ACTIVE_TASKS: Mutex<Vec<ActiveScheduledTask>> = Mutex::new(Vec::new());
pub static PROACTIVE_COMPANION_ENABLED: AtomicBool = AtomicBool::new(true);
pub static PROACTIVE_INTERVAL_MINUTES: AtomicU64 = AtomicU64::new(22);

static SCHEDULER_CMD_TX: Mutex<Option<UnboundedSender<AgentCommand>>> = Mutex::new(None);
static SCHEDULER_EVENT_TX: Mutex<Option<StdSender<AgentEvent>>> = Mutex::new(None);
static SCHEDULER_TTS_TX: Mutex<Option<StdSender<TtsCommand>>> = Mutex::new(None);
static TOKIO_HANDLE: Mutex<Option<tokio::runtime::Handle>> = Mutex::new(None);

pub fn init_scheduler(
    tx: UnboundedSender<AgentCommand>,
    event_tx: StdSender<AgentEvent>,
    tts_tx: StdSender<TtsCommand>,
    handle: tokio::runtime::Handle,
) {
    if let Ok(mut lock) = SCHEDULER_CMD_TX.lock() {
        *lock = Some(tx);
    }
    if let Ok(mut lock) = SCHEDULER_EVENT_TX.lock() {
        *lock = Some(event_tx);
    }
    if let Ok(mut lock) = SCHEDULER_TTS_TX.lock() {
        *lock = Some(tts_tx);
    }
    if let Ok(mut lock) = TOKIO_HANDLE.lock() {
        *lock = Some(handle);
    }
}

pub fn cancel_all_scheduled_tasks() -> usize {
    SCHEDULE_GENERATION.fetch_add(1, Ordering::SeqCst);
    let mut count = 0;
    if let Ok(mut tasks) = ACTIVE_TASKS.lock() {
        count = tasks.len();
        tasks.clear();
    }
    SCHEDULED_TASKS_COUNT.swap(0, Ordering::SeqCst)
        .max(count)
}

pub fn cancel_scheduled_task_by_id(id: u64) -> bool {
    let mut removed = false;
    if let Ok(mut tasks) = ACTIVE_TASKS.lock() {
        let before = tasks.len();
        tasks.retain(|t| t.id != id);
        if tasks.len() < before {
            removed = true;
            SCHEDULED_TASKS_COUNT.fetch_sub(1, Ordering::SeqCst);
        }
    }
    removed
}

pub fn get_active_scheduled_tasks() -> Vec<ActiveScheduledTask> {
    if let Ok(tasks) = ACTIVE_TASKS.lock() {
        let now = Instant::now();
        let mut list: Vec<ActiveScheduledTask> = tasks
            .iter()
            .filter(|t| t.target_instant >= now || t.cron_expr.is_some())
            .cloned()
            .collect();
        list.sort_by_key(|t| t.target_instant.saturating_duration_since(now));
        list
    } else {
        Vec::new()
    }
}

pub fn set_proactive_companion_config(enabled: bool, interval_minutes: Option<u64>) {
    PROACTIVE_COMPANION_ENABLED.store(enabled, Ordering::SeqCst);
    if let Some(mins) = interval_minutes {
        PROACTIVE_INTERVAL_MINUTES.store(mins.clamp(5, 180), Ordering::SeqCst);
    }
    let _ = crate::fs_util::write_env_var_to_file(
        ".env",
        "PROACTIVE_COMPANION",
        if enabled { "true" } else { "false" },
    );
    if let Some(mins) = interval_minutes {
        let _ = crate::fs_util::write_env_var_to_file(
            ".env",
            "PROACTIVE_INTERVAL_MINS",
            &mins.to_string(),
        );
    }
}

/// Nomme automatiquement et lisiblement un rappel à partir de la consigne
pub fn generate_task_label(prompt: &str) -> String {
    let clean = prompt.trim();
    let lower = clean.to_lowercase();

    let prefixes = [
        "rappelle-moi de ", "rappelle moi de ", "rappelle-moi d'", "rappelle moi d'",
        "rappelle-moi d' ", "rappelle moi ", "rappel : ", "rappel: ", "pense à ",
        "pense a ", "penser à ", "penser a ", "n'oublie pas de ", "n oublie pas de ",
        "minuterie pour ", "minuteur pour ", "timer pour ", "alarme pour ",
        "remind me to ", "remind me ", "reminder: ", "reminder : ",
    ];

    for p in prefixes {
        if lower.starts_with(p) {
            let rest = clean[p.len()..].trim();
            if !rest.is_empty() {
                let mut chars = rest.chars();
                if let Some(first) = chars.next() {
                    let cap: String = first.to_uppercase().collect::<String>() + chars.as_str();
                    return cap.chars().take(45).collect();
                }
            }
        }
    }

    let words: Vec<&str> = clean.split_whitespace().take(6).collect();
    if !words.is_empty() {
        let short = words.join(" ");
        let mut chars = short.chars();
        if let Some(first) = chars.next() {
            return first.to_uppercase().collect::<String>() + chars.as_str();
        }
    }
    "Rappel programmé".to_string()
}

/// Détecte si la consigne est un simple rappel/minuteur à annoncer ou une action système à exécuter
pub fn is_pure_reminder_prompt(prompt: &str) -> bool {
    let clean = prompt.trim().to_lowercase();

    if clean.starts_with("rappelle")
        || clean.starts_with("rappel")
        || clean.starts_with("remind")
        || clean.starts_with("minuteur")
        || clean.starts_with("minuterie")
        || clean.starts_with("timer")
        || clean.starts_with("alarme")
        || clean.starts_with("alarm")
        || clean.contains("minuteur terminé")
        || clean.contains("timer terminé")
    {
        return true;
    }

    let is_system_action = clean.starts_with("ouvre ")
        || clean.starts_with("ouvrir ")
        || clean.starts_with("open ")
        || clean.starts_with("lance ")
        || clean.starts_with("lancer ")
        || clean.starts_with("launch ")
        || clean.starts_with("ferme ")
        || clean.starts_with("fermer ")
        || clean.starts_with("close ")
        || clean.starts_with("clique ")
        || clean.starts_with("cliquer ")
        || clean.starts_with("click ")
        || clean.starts_with("navigue ")
        || clean.starts_with("va sur ")
        || clean.starts_with("cherche ")
        || clean.starts_with("recherche ")
        || clean.starts_with("search ")
        || clean.starts_with("écris ")
        || clean.starts_with("ecris ")
        || clean.starts_with("write ")
        || clean.starts_with("scroll ")
        || clean.starts_with("descends ")
        || clean.starts_with("monte ")
        || clean.starts_with("coupe le son")
        || clean.starts_with("mets le son")
        || clean.starts_with("mute")
        || clean.starts_with("unmute")
        || clean.starts_with("résume")
        || clean.starts_with("resume");

    !is_system_action
}

/// Déclenchement autonome du rappel directement au niveau du worker
pub fn trigger_scheduled_task(
    id: u64,
    label: String,
    prompt: String,
    is_cron: bool,
    cmd_tx: &UnboundedSender<AgentCommand>,
    event_tx_opt: Option<&StdSender<AgentEvent>>,
    _tts_tx_opt: Option<&StdSender<TtsCommand>>,
) {
    if !is_cron {
        let _ = cancel_scheduled_task_by_id(id);
    }

    println!("[Scheduler Worker] Exécution programmée #{id} : \"{prompt}\" (label: \"{label}\")");

    // 1. Réveil physique et restauration immédiate au niveau de l'OS même si minimisé
    #[cfg(windows)]
    unsafe {
        use windows::core::w;
        use windows::Win32::UI::WindowsAndMessaging::{FindWindowW, SetForegroundWindow, ShowWindow, SW_RESTORE};
        let overlay = FindWindowW(None, w!("Libertide overlay"))
            .unwrap_or(windows::Win32::Foundation::HWND(std::ptr::null_mut()));
        if !overlay.0.is_null() {
            let _ = ShowWindow(overlay, SW_RESTORE);
            let _ = SetForegroundWindow(overlay);
        }
    }

    // 2. Chime d'alerte sonore immédiat
    crate::overlay::play_earcon(crate::overlay::Earcon::Warning);

    let is_pure_reminder = is_pure_reminder_prompt(&prompt);

    if is_pure_reminder {
        if let Some(event_tx) = event_tx_opt {
            let _ = event_tx.send(AgentEvent::ReminderTriggered {
                label: label.clone(),
                prompt: prompt.clone(),
            });
        }
        let reminder_prompt = format!(
            "[RAPPEL PROGRAMMÉ - FORCE_TAGS] (forcetags) C'est l'heure du rappel : \"{label}\". Consigne d'origine : \"{prompt}\". Analyse la situation actuelle à l'écran et annonce ce rappel à l'utilisateur de manière contextuelle et fluide, en lui proposant ton aide si nécessaire."
        );
        let _ = cmd_tx.send(AgentCommand::Prompt(reminder_prompt));
    } else {
        if let Some(event_tx) = event_tx_opt {
            let _ = event_tx.send(AgentEvent::ReminderTriggered {
                label: format!("Action programmée : {label}"),
                prompt: prompt.clone(),
            });
        }
        let action_prompt = format!("[FORCE_TAGS] (forcetags) {prompt}");
        let _ = cmd_tx.send(AgentCommand::Prompt(action_prompt));
    }
}

pub fn schedule_prompt_execution(delay: Duration, prompt: String) {
    schedule_prompt_execution_with_label(delay, prompt, None);
}

pub fn schedule_prompt_execution_with_label(delay: Duration, prompt: String, label_opt: Option<String>) {
    let id = NEXT_TASK_ID.fetch_add(1, Ordering::SeqCst);
    let label = label_opt.unwrap_or_else(|| generate_task_label(&prompt));
    let target_instant = Instant::now() + delay;

    if let Ok(mut tasks) = ACTIVE_TASKS.lock() {
        tasks.push(ActiveScheduledTask {
            id,
            label: label.clone(),
            prompt: prompt.clone(),
            target_instant,
            total_duration: delay,
            cron_expr: None,
        });
    }

    let current_gen = SCHEDULE_GENERATION.load(Ordering::SeqCst);
    SCHEDULED_TASKS_COUNT.fetch_add(1, Ordering::SeqCst);

    let tx_opt = SCHEDULER_CMD_TX.lock().ok().and_then(|g| g.clone());
    let Some(cmd_tx) = tx_opt else {
        eprintln!("[Scheduler] Erreur : SCHEDULER_CMD_TX non initialisé.");
        return;
    };
    let event_tx_opt = SCHEDULER_EVENT_TX.lock().ok().and_then(|g| g.clone());
    let tts_tx_opt = SCHEDULER_TTS_TX.lock().ok().and_then(|g| g.clone());

    if let Ok(guard) = TOKIO_HANDLE.lock() {
        if let Some(ref handle) = *guard {
            let label_c = label.clone();
            handle.spawn(async move {
                tokio::time::sleep(delay).await;

                if SCHEDULE_GENERATION.load(Ordering::SeqCst) != current_gen
                    || IS_EMERGENCY_STOPPED.load(Ordering::SeqCst)
                {
                    println!("[Scheduler] Tâche différée annulée ou invalidée : \"{prompt}\"");
                    let _ = cancel_scheduled_task_by_id(id);
                    return;
                }

                trigger_scheduled_task(
                    id,
                    label_c,
                    prompt,
                    false,
                    &cmd_tx,
                    event_tx_opt.as_ref(),
                    tts_tx_opt.as_ref(),
                );
            });
            return;
        }
    }

    let label_c = label.clone();
    std::thread::spawn(move || {
        std::thread::sleep(delay);
        if SCHEDULE_GENERATION.load(Ordering::SeqCst) != current_gen
            || IS_EMERGENCY_STOPPED.load(Ordering::SeqCst)
        {
            println!("[Scheduler] Tâche différée annulée ou invalidée : \"{prompt}\"");
            let _ = cancel_scheduled_task_by_id(id);
            return;
        }

        trigger_scheduled_task(
            id,
            label_c,
            prompt,
            false,
            &cmd_tx,
            event_tx_opt.as_ref(),
            tts_tx_opt.as_ref(),
        );
    });
}

/// Calcule le délai jusqu'à la prochaine occurrence d'une expression cron ou périodique
pub fn next_cron_delay(expr: &str) -> Option<Duration> {
    let clean = expr.trim().to_lowercase();

    if clean == "@hourly" || clean == "hourly" {
        return Some(Duration::from_secs(3600));
    }
    if clean == "@daily" || clean == "daily" {
        return Some(Duration::from_secs(86400));
    }
    if clean == "@weekly" || clean == "weekly" {
        return Some(Duration::from_secs(604800));
    }

    if let Some(rest) = clean
        .strip_prefix("every ")
        .or_else(|| clean.strip_prefix("@every "))
        .or_else(|| clean.strip_prefix("toutes les "))
        .or_else(|| clean.strip_prefix("chaque "))
    {
        let r = rest.trim();
        if let Some(m) = r.strip_suffix('m').or_else(|| r.strip_suffix(" min")).or_else(|| r.strip_suffix(" minutes")) {
            if let Ok(v) = m.trim().parse::<u64>() {
                return Some(Duration::from_secs(v * 60));
            }
        }
        if let Some(h) = r.strip_suffix('h').or_else(|| r.strip_suffix(" heures")).or_else(|| r.strip_suffix(" hours")) {
            if let Ok(v) = h.trim().parse::<u64>() {
                return Some(Duration::from_secs(v * 3600));
            }
        }
        if let Some(s) = r.strip_suffix('s').or_else(|| r.strip_suffix(" sec")).or_else(|| r.strip_suffix(" secondes")) {
            if let Ok(v) = s.trim().parse::<u64>() {
                return Some(Duration::from_secs(v.max(5)));
            }
        }
    }

    let parts: Vec<&str> = clean.split_whitespace().collect();
    if parts.len() == 5 {
        if parts[0].starts_with("*/") {
            if let Ok(step) = parts[0][2..].parse::<u64>() {
                return Some(Duration::from_secs((step * 60).max(60)));
            }
        }
        if parts[1].starts_with("*/") {
            if let Ok(step) = parts[1][2..].parse::<u64>() {
                return Some(Duration::from_secs((step * 3600).max(3600)));
            }
        }
        return Some(Duration::from_secs(3600));
    }

    None
}

/// Planifie un cronjob récurrent qui s'auto-réévalue après chaque déclenchement
pub fn schedule_cron_execution(
    cron_expr: String,
    prompt: String,
    label_opt: Option<String>,
) -> Result<u64, String> {
    let initial_delay = next_cron_delay(&cron_expr)
        .ok_or_else(|| format!("Expression cron non valide : '{cron_expr}'. Formats acceptés : '*/15 * * * *', 'every 30m', '@hourly', '@daily'."))?;

    let id = NEXT_TASK_ID.fetch_add(1, Ordering::SeqCst);
    let label = label_opt.unwrap_or_else(|| format!("[Cron] {}", generate_task_label(&prompt)));
    let target_instant = Instant::now() + initial_delay;

    if let Ok(mut tasks) = ACTIVE_TASKS.lock() {
        tasks.push(ActiveScheduledTask {
            id,
            label: label.clone(),
            prompt: prompt.clone(),
            target_instant,
            total_duration: initial_delay,
            cron_expr: Some(cron_expr.clone()),
        });
    }

    SCHEDULED_TASKS_COUNT.fetch_add(1, Ordering::SeqCst);
    let tx_opt = SCHEDULER_CMD_TX.lock().ok().and_then(|g| g.clone());
    let Some(cmd_tx) = tx_opt else {
        return Err("Scheduler non initialisé.".to_string());
    };

    let cron_clone = cron_expr.clone();
    let prompt_clone = prompt.clone();

    std::thread::spawn(move || {
        loop {
            let current_gen = SCHEDULE_GENERATION.load(Ordering::SeqCst);
            let delay_to_wait = if let Ok(tasks) = ACTIVE_TASKS.lock() {
                if let Some(t) = tasks.iter().find(|t| t.id == id) {
                    t.target_instant.saturating_duration_since(Instant::now())
                } else {
                    break;
                }
            } else {
                initial_delay
            };

            std::thread::sleep(delay_to_wait);

            if SCHEDULE_GENERATION.load(Ordering::SeqCst) != current_gen
                || IS_EMERGENCY_STOPPED.load(Ordering::SeqCst)
            {
                println!("[Cron] Tâche récurrente invalidée ou arrêtée.");
                break;
            }

            let still_active = ACTIVE_TASKS
                .lock()
                .map(|t| t.iter().any(|item| item.id == id))
                .unwrap_or(false);
            if !still_active {
                break;
            }

            let event_tx_opt = SCHEDULER_EVENT_TX.lock().ok().and_then(|g| g.clone());
            let tts_tx_opt = SCHEDULER_TTS_TX.lock().ok().and_then(|g| g.clone());
            trigger_scheduled_task(
                id,
                label.clone(),
                prompt_clone.clone(),
                true,
                &cmd_tx,
                event_tx_opt.as_ref(),
                tts_tx_opt.as_ref(),
            );

            let next_delay = next_cron_delay(&cron_clone).unwrap_or(Duration::from_secs(3600));
            let next_target = Instant::now() + next_delay;

            if let Ok(mut tasks) = ACTIVE_TASKS.lock() {
                if let Some(item) = tasks.iter_mut().find(|t| t.id == id) {
                    item.target_instant = next_target;
                    item.total_duration = next_delay;
                } else {
                    break;
                }
            } else {
                break;
            }
        }
    });

    Ok(id)
}

pub fn format_duration_friendly(d: Duration) -> String {
    let secs = d.as_secs();
    if secs >= 3600 {
        let hours = secs / 3600;
        let mins = (secs % 3600) / 60;
        if mins > 0 {
            format!("{hours} h {mins} min")
        } else {
            format!("{hours} heure{}", if hours > 1 { "s" } else { "" })
        }
    } else if secs >= 60 {
        let mins = secs / 60;
        let rem_secs = secs % 60;
        if rem_secs > 0 {
            format!("{mins} min {rem_secs} s")
        } else {
            format!("{mins} minute{}", if mins > 1 { "s" } else { "" })
        }
    } else {
        format!("{secs} seconde{}", if secs > 1 { "s" } else { "" })
    }
}

pub fn is_cancel_scheduled_command(text: &str) -> bool {
    let norm = clean_words(text).join(" ");
    const TRIGGERS: &[&str] = &[
        "annule les commandes differees", "annule les commandes différées",
        "annuler les commandes differees", "annuler les commandes différées",
        "annule les taches differees", "annule les tâches différées",
        "annuler les taches differees", "annuler les tâches différées",
        "annule la commande differee", "annule la commande différée",
        "annule la tache differee", "annule la tâche différée",
        "annuler la tache differee", "annuler la tâche différée",
        "annule les differees", "annule les différées",
        "annuler differe", "annuler différé", "annule differe", "annule différé",
        "annule les timers", "annuler les timers", "arrete les timers", "arrête les timers",
        "annule le timer", "annuler le timer", "arrete le timer", "arrête le timer",
        "stop timer", "stop timers", "annule l alarme", "annuler l alarme",
        "annule le rappel", "annuler le rappel", "annule les rappels", "annuler les rappels",
        "supprime les commandes differees", "supprime les commandes différées",
        "supprime le timer", "supprime les timers", "supprime le rappel", "supprime les rappels",
        "cancel scheduled tasks", "cancel scheduled commands", "cancel scheduled prompt",
        "cancel scheduled prompts", "cancel timers", "cancel timer", "stop timers", "stop timer", "clear timers",
        "cancelar comandos diferidos", "cancelar tareas programadas",
        "geplante aufgaben abbrechen", "timer abbrechen",
        "annulla comandi ritardati", "annulla compiti programmati",
        "cancelar comandos agendados", "cancelar tarefas agendadas",
        "cancelar temporizadores", "parar timers",
        "отменить запланированные задачи", "отменить задачи",
        "отменить таймеры", "остановить таймеры",
        "取消定时任务", "取消计划任务", "取消定时", "取消计时器", "停止计时器",
        "スケジュールされたタスクをキャンセル", "予定されたタスクをキャンセル",
        "タイマーをキャンセル", "タイマー停止",
    ];
    TRIGGERS.iter().any(|&trig| norm.contains(trig) || text.to_lowercase().contains(trig))
}

/// Routine d'impulsion proactive périodique avec variation thématique et gigue temporelle
pub fn spawn_proactive_companion_loop(tx: UnboundedSender<AgentCommand>) {
    std::thread::Builder::new()
        .name("proactive_companion_worker".to_string())
        .spawn(move || {
            // Lecture de la configuration initiale depuis .env
            let env_enabled = crate::fs_util::read_env_var_from_file(".env", "PROACTIVE_COMPANION")
                .map(|v| v.trim().eq_ignore_ascii_case("true") || v == "1")
                .unwrap_or(true);
            PROACTIVE_COMPANION_ENABLED.store(env_enabled, Ordering::SeqCst);

            if let Some(m_str) = crate::fs_util::read_env_var_from_file(".env", "PROACTIVE_INTERVAL_MINS") {
                if let Ok(mins) = m_str.trim().parse::<u64>() {
                    PROACTIVE_INTERVAL_MINUTES.store(mins.clamp(5, 180), Ordering::SeqCst);
                }
            }

            let mut category_index = 0usize;
            let mut last_proactive_instant = Instant::now();

            // Délai initial de bienvenue (attente de 3 minutes après le démarrage)
            std::thread::sleep(Duration::from_secs(180));

            loop {
                std::thread::sleep(Duration::from_secs(15));

                if !PROACTIVE_COMPANION_ENABLED.load(Ordering::Relaxed) {
                    continue;
                }

                let base_mins = PROACTIVE_INTERVAL_MINUTES.load(Ordering::Relaxed);
                // Gigue pseudo-aléatoire naturelle (± 3 minutes)
                let jitter_secs = ((Instant::now().elapsed().as_millis() % 7) as i64 - 3) * 60;
                let target_interval = Duration::from_secs(
                    ((base_mins as i64 * 60) + jitter_secs).clamp(300, 10800) as u64,
                );

                if last_proactive_instant.elapsed() < target_interval {
                    continue;
                }

                // Vérification de la disponibilité de l'utilisateur pour ne pas le déranger
                let is_emergency = IS_EMERGENCY_STOPPED.load(Ordering::Relaxed);
                let is_dictation = IS_DICTATION_ACTIVE.load(Ordering::Relaxed);
                let is_busy = AGENT_BUSY.load(Ordering::Relaxed);
                let is_typing = IS_USER_TYPING.load(Ordering::Relaxed);

                if is_emergency || is_dictation || is_busy || is_typing {
                    // Remise à plus tard dans 45 secondes si l'utilisateur est occupé
                    continue;
                }

                last_proactive_instant = Instant::now();
                category_index = (category_index + 1) % 5;

                let proactive_prompt = match category_index {
                    0 => {
                        // Soin & Confort physique (CareAndComfort)
                        "[COMPAGNON PROACTIF - THÈME: CareAndComfort] : Fais une brève intervention bienveillante (1 à 2 phrases) pour suggérer gentiment à l'utilisateur de s'hydrater (boire un verre d'eau), relâcher ses épaules ou cligner des yeux. Propose 2 suggestions d'action simples dans quick_suggestions."
                    }
                    1 => {
                        // Question curieuse & Savoir (KnowledgeAndQuestions)
                        "[COMPAGNON PROACTIF - THÈME: KnowledgeAndQuestions] : Partage une courte pensée ou anecdote étonnante et intéressante (science, histoire, nature ou technologie) et pose une question ouverte pour susciter la curiosité de l'utilisateur."
                    }
                    2 => {
                        // Écoute émotionnelle & Humeur (EmotionalSupportAndEmpathy)
                        "[COMPAGNON PROACTIF - THÈME: EmotionalSupportAndEmpathy] : Prends doucement des nouvelles de l'utilisateur avec chaleur et empathie. Demande-lui comment se passe sa journée ou s'il aimerait papoter ou écouter quelque chose."
                    }
                    3 => {
                        // Pause musicale & Calme sensoriel (RadioAndMusicStream)
                        "[COMPAGNON PROACTIF - THÈME: RadioAndMusicStream] : Propose à l'utilisateur de lancer une musique d'ambiance relaxante, une webradio ou un moment de calme pour rythmer sa session. Inclus des suggestions cliquables."
                    }
                    _ => {
                        // Pensée philosophique & Complicité éthique (ChattyAndEthicalCompanion)
                        "[COMPAGNON PROACTIF - THÈME: ChattyAndEthicalCompanion] : Partage une pensée inspirante, optimiste ou un trait d'humour léger sur la vie quotidienne ou sur ton rôle d'assistant bienveillant, juste pour papoter agréablement."
                    }
                };

                println!(
                    "[Scheduler Proactif] Déclenchement de la pensée périodique (catégorie {}).",
                    category_index
                );
                let _ = tx.send(AgentCommand::Prompt(proactive_prompt.to_string()));
            }
        })
        .expect("Échec du lancement du worker de compagnon proactif");
}