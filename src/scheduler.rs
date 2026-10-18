use std::sync::Mutex;
use std::sync::atomic::{AtomicU64, AtomicUsize, Ordering};
use std::time::Duration;
use tokio::sync::mpsc::UnboundedSender;

use crate::automation::IS_EMERGENCY_STOPPED;
use crate::pattern::clean_words;
use crate::types::AgentCommand;

pub static SCHEDULE_GENERATION: AtomicU64 = AtomicU64::new(0);
pub static SCHEDULED_TASKS_COUNT: AtomicUsize = AtomicUsize::new(0);

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
        "annule les differees", "annule les différées",
        "annuler differe", "annuler différé", "annule differe", "annule différé",
        "annule les timers", "annuler les timers", "arrete les timers", "arrête les timers",
        "supprime les commandes differees", "supprime les commandes différées",
        "cancel scheduled tasks", "cancel scheduled commands", "cancel scheduled prompt",
        "cancel scheduled prompts", "cancel timers", "stop timers", "clear timers",
        "cancelar comandos diferidos", "cancelar tareas programadas",
        "geplante aufgaben abbrechen", "timer abbrechen",
        "annulla comandi ritardati", "annulla compiti programmati",
    ];
    TRIGGERS.iter().any(|&trig| norm.contains(trig) || text.to_lowercase().contains(trig))
}