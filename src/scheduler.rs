use std::sync::Mutex;
use std::sync::atomic::{AtomicBool, AtomicU64, AtomicUsize, Ordering};
use std::time::{Duration, Instant};
use tokio::sync::mpsc::UnboundedSender;

use crate::automation::{AGENT_BUSY, IS_DICTATION_ACTIVE, IS_EMERGENCY_STOPPED, IS_USER_TYPING};
use crate::pattern::clean_words;
use crate::types::AgentCommand;

pub static SCHEDULE_GENERATION: AtomicU64 = AtomicU64::new(0);
pub static SCHEDULED_TASKS_COUNT: AtomicUsize = AtomicUsize::new(0);
pub static PROACTIVE_COMPANION_ENABLED: AtomicBool = AtomicBool::new(true);
pub static PROACTIVE_INTERVAL_MINUTES: AtomicU64 = AtomicU64::new(22);

static SCHEDULER_CMD_TX: Mutex<Option<UnboundedSender<AgentCommand>>> = Mutex::new(None);
static TOKIO_HANDLE: Mutex<Option<tokio::runtime::Handle>> = Mutex::new(None);

pub fn init_scheduler(tx: UnboundedSender<AgentCommand>, handle: tokio::runtime::Handle) {
    if let Ok(mut lock) = SCHEDULER_CMD_TX.lock() {
        *lock = Some(tx);
    }
    if let Ok(mut lock) = TOKIO_HANDLE.lock() {
        *lock = Some(handle);
    }
}

pub fn cancel_all_scheduled_tasks() -> usize {
    SCHEDULE_GENERATION.fetch_add(1, Ordering::SeqCst);
    SCHEDULED_TASKS_COUNT.swap(0, Ordering::SeqCst)
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

pub fn schedule_prompt_execution(delay: Duration, prompt: String) {
    let current_gen = SCHEDULE_GENERATION.load(Ordering::SeqCst);
    SCHEDULED_TASKS_COUNT.fetch_add(1, Ordering::SeqCst);

    let tx_opt = SCHEDULER_CMD_TX.lock().ok().and_then(|g| g.clone());
    let Some(cmd_tx) = tx_opt else {
        eprintln!("[Scheduler] Erreur : SCHEDULER_CMD_TX non initialisé.");
        return;
    };

    if let Ok(guard) = TOKIO_HANDLE.lock() {
        if let Some(ref handle) = *guard {
            handle.spawn(async move {
                tokio::time::sleep(delay).await;

                if SCHEDULE_GENERATION.load(Ordering::SeqCst) != current_gen
                    || IS_EMERGENCY_STOPPED.load(Ordering::SeqCst)
                {
                    println!("[Scheduler] Tâche différée annulée ou invalidée : \"{prompt}\"");
                    return;
                }

                SCHEDULED_TASKS_COUNT.fetch_sub(1, Ordering::SeqCst);
                println!("[Scheduler] Exécution programmée de la tâche : \"{prompt}\"");
                let _ = cmd_tx.send(AgentCommand::Prompt(prompt));
            });
            return;
        }
    }

    std::thread::spawn(move || {
        std::thread::sleep(delay);
        if SCHEDULE_GENERATION.load(Ordering::SeqCst) != current_gen
            || IS_EMERGENCY_STOPPED.load(Ordering::SeqCst)
        {
            println!("[Scheduler] Tâche différée annulée ou invalidée : \"{prompt}\"");
            return;
        }
        SCHEDULED_TASKS_COUNT.fetch_sub(1, Ordering::SeqCst);
        println!("[Scheduler] Exécution programmée de la tâche : \"{prompt}\"");
        let _ = cmd_tx.send(AgentCommand::Prompt(prompt));
    });
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