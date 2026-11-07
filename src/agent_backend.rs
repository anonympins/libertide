use std::sync::Arc;
use std::sync::atomic::Ordering;
use std::sync::mpsc::Sender as StdSender;
use std::time::{Duration, Instant};
use tokio::sync::mpsc::{UnboundedReceiver, UnboundedSender};
use rust_i18n::t;

use crate::automation::*;
use crate::html_util;
use crate::local_llm;
use crate::tts::sanitize_for_tts;
use crate::twitch::search_twitch_channels;
use crate::types::*;
use crate::window_util::*;
use crate::floating_window::{FloatingCard, FloatingContentType, OPEN_FLOATING_CARDS};
use crate::get_system_locale_language;
use crate::{analyze_navigation_zones_with_llm, parse_agent_response, resolve_llm_config};

/// Formate les éléments interactifs d'une fenêtre de manière ultra-compacte pour les petits LLMs locaux
pub fn format_compact_window_elements(win_title: &str, targets: &[NumberedElementTarget]) -> String {
    let mut out = String::new();
    let title_clean = win_title.trim();
    if !title_clean.is_empty() {
        out.push_str(&format!("[Fenêtre active au premier plan : \"{}\"]\n", title_clean));
    } else {
        out.push_str("[Fenêtre active au premier plan]\n");
    }
    if targets.is_empty() {
        out.push_str("(Aucun contrôle interactif identifié)\n");
    } else {
        out.push_str("Contrôles interactifs détectés :\n");
        for t in targets.iter().take(110) {
            let role = match t.role.as_str() {
                "bouton" | "button" => "bouton",
                "champ de saisie" | "input" => "saisie",
                "zone de texte" | "textarea" => "texte",
                "lien" | "link" => "lien",
                "onglet" | "tab" => "onglet",
                "case à cocher" | "checkbox" => "case",
                "liste déroulante" | "select" | "combobox" => "menu",
                other => other,
            };
            let name = t.name.trim();
            let val = t.value.trim();
            if !name.is_empty() && !val.is_empty() && val != name {
                out.push_str(&format!("- #{} [{}] \"{}\" (val: \"{}\")\n", t.index, role, name, val));
            } else if !name.is_empty() {
                out.push_str(&format!("- #{} [{}] \"{}\"\n", t.index, role, name));
            } else if !val.is_empty() {
                out.push_str(&format!("- #{} [{}] val: \"{}\"\n", t.index, role, val));
            } else {
                out.push_str(&format!("- #{} [{}]\n", t.index, role));
            }
        }
        if targets.len() > 110 {
            out.push_str(&format!("... et {} autres éléments non listés.\n", targets.len() - 110));
        }
    }
    out
}

/// Récupère le titre de la fenêtre active au premier plan (avec repli sur la dernière fenêtre externe)
#[cfg(windows)]
pub fn get_foreground_window_title() -> Option<String> {
    let fg = unsafe { windows::Win32::UI::WindowsAndMessaging::GetForegroundWindow() };
    let current_pid = std::process::id();
    let (fg_pid, _) = get_window_process_info(fg);
    let target = if !fg.0.is_null() && fg_pid != current_pid && !is_window_hung(fg) {
        Some(fg)
    } else {
        LAST_EXTERNAL_HWND
            .lock()
            .ok()
            .and_then(|g| *g)
            .map(|raw| windows::Win32::Foundation::HWND(raw as _))
            .filter(|&h| !h.0.is_null() && !is_window_hung(h))
    };

    target.map(|h| {
        let title = crate::get_window_title(h);
        if title.trim().is_empty() {
            "Fenêtre active".to_string()
        } else {
            title
        }
    })
}

#[cfg(not(windows))]
pub fn get_foreground_window_title() -> Option<String> {
    #[cfg(target_os = "macos")]
    {
        crate::window_util::get_active_macos_window()
    }
    #[cfg(all(not(windows), not(target_os = "macos")))]
    {
        crate::window_util::get_active_x11_window().and_then(|id| {
            let user_wins = crate::window_util::list_user_windows();
            user_wins.into_iter().find(|(w_id, _)| *w_id == id).map(|(_, title)| title)
        })
    }
}

/// Extrait les derniers messages de l'historique en conservant au maximum `max_user_msgs` messages utilisateur
pub fn get_recent_history_capped_at_user_messages(history: &[ChatMessage], max_user_msgs: usize) -> Vec<ChatMessage> {
    let mut user_count = 0;
    let mut start_idx = 0;
    for (i, msg) in history.iter().enumerate().rev() {
        if msg.role == "user" {
            user_count += 1;
            if user_count == max_user_msgs {
                start_idx = i;
                break;
            }
        }
    }
    let mut recent = history[start_idx..].to_vec();
    let user_indices: Vec<usize> = recent
        .iter()
        .enumerate()
        .filter(|(_, m)| m.role == "user")
        .map(|(i, _)| i)
        .collect();

    if let Some(&last_user_idx) = user_indices.last() {
        for &idx in &user_indices {
            if idx != last_user_idx {
                let content = &recent[idx].content;
                if let Some(pos) = content.rfind("[Demande utilisateur]\n") {
                    recent[idx].content = content[pos + "[Demande utilisateur]\n".len()..].trim().to_string();
                } else if let Some(pos) = content.rfind("[Étape ") {
                    recent[idx].content = content[pos..].trim().to_string();
                } else if let Some(pos) = content.find("[État actuel de l'écran]") {
                    if let Some(end_pos) = content[pos..].find("\n\n") {
                        recent[idx].content = content[pos + end_pos + 2..].trim().to_string();
                    }
                } else if let Some(pos) = content.find("[État de l'écran suite aux actions]") {
                    if let Some(end_pos) = content[pos..].find("\n\n") {
                        recent[idx].content = content[pos + end_pos + 2..].trim().to_string();
                    }
                }
            }
        }
    }
    recent
}

pub struct BusyGuard;
impl Drop for BusyGuard {
    fn drop(&mut self) {
        AGENT_BUSY.store(false, Ordering::SeqCst);
    }
}

/// Envoi unifié d'une requête de complétion selon le fournisseur configuré
pub async fn send_llm_chat_completion(
    client: &reqwest::Client,
    config: &LlmConfig,
    system_prompt: &str,
    messages_history: &[ChatMessage],
    temperature: f32,
    max_tokens: u32,
) -> Result<reqwest::Response, reqwest::Error> {
    if config.provider == LlmProvider::Anthropic {
        let mut anthropic_messages: Vec<ChatMessage> = Vec::new();
        for msg in messages_history {
            if msg.role == "system" {
                continue;
            }
            let role = if msg.role == "assistant" { "assistant" } else { "user" };
            if let Some(last) = anthropic_messages.last_mut() {
                if last.role == role {
                    last.content.push_str("\n\n");
                    last.content.push_str(&msg.content);
                    continue;
                }
            }
            anthropic_messages.push(ChatMessage {
                role: role.to_string(),
                content: msg.content.clone(),
            });
        }
        if anthropic_messages.first().map_or(false, |m| m.role != "user") {
            anthropic_messages.insert(0, ChatMessage {
                role: "user".to_string(),
                content: "Bonjour".to_string(),
            });
        }

        let req_payload = AnthropicMessagesRequest {
            model: config.model.clone(),
            system: if system_prompt.trim().is_empty() { None } else { Some(system_prompt.to_string()) },
            messages: anthropic_messages,
            max_tokens,
            temperature,
        };

        let mut req = client
            .post(&config.endpoint)
            .header("anthropic-version", "2023-06-01")
            .header("content-type", "application/json");

        if !config.api_key.trim().is_empty() {
            req = req.header("x-api-key", &config.api_key);
        }

        req
            .json(&req_payload)
            .send()
            .await
    } else {
        let mut messages = Vec::new();
        if !system_prompt.trim().is_empty() {
            messages.push(ChatMessage {
                role: "system".to_string(),
                content: system_prompt.to_string(),
            });
        }
        messages.extend_from_slice(messages_history);

        let response_format = match config.provider {
            LlmProvider::DeepSeek | LlmProvider::OpenAI | LlmProvider::Mistral | LlmProvider::Groq => {
                Some(ChatResponseFormat {
                    r#type: "json_object".to_string(),
                })
            }
            _ => None,
        };

        let req_payload = DeepSeekChatRequest {
            model: config.model.clone(),
            messages,
            temperature,
            max_tokens,
            top_p: 1.0,
            stream: false,
            response_format,
        };

        let mut req = client.post(&config.endpoint);
        let key_clean = config.api_key.trim();
        if !key_clean.is_empty() {
            req = req.bearer_auth(key_clean);
        }

        req
            .json(&req_payload)
            .send()
            .await
    }
}

pub async fn parse_llm_response_text(res: reqwest::Response, provider: LlmProvider) -> Result<String, String> {
    if provider == LlmProvider::Anthropic {
        let body = res.json::<AnthropicMessagesResponse>().await
            .map_err(|e| format!("Erreur décodage JSON Anthropic: {e}"))?;
        let content = body.content
            .into_iter()
            .filter(|b| b.block_type == "text")
            .filter_map(|b| b.text)
            .collect::<Vec<_>>()
            .join("\n");
        if content.is_empty() {
            Err("Réponse Anthropic vide".to_string())
        } else {
            Ok(content)
        }
    } else {
        let body = res.json::<DeepSeekChatResponse>().await
            .map_err(|e| format!("Erreur décodage JSON OpenAI/DeepSeek: {e}"))?;
        if let Some(choice) = body.choices.first() {
            Ok(choice.message.content.clone())
        } else {
            Err("Aucun choix retourné dans la réponse".to_string())
        }
    }
}

pub async fn compact_history_if_needed(
    llm_config: &LlmConfig,
    history: &mut Vec<ChatMessage>,
    client: &reqwest::Client,
    last_call_time: &mut Option<Instant>,
) {
    const COMPACTION_CHAR_THRESHOLD: usize = 6000;
    const COMPACTION_MSG_THRESHOLD: usize = 8;
    const KEEP_RECENT_COUNT: usize = 3;

    let total_chars: usize = history.iter().map(|m| m.content.len()).sum();
    if (history.len() < COMPACTION_MSG_THRESHOLD && total_chars < COMPACTION_CHAR_THRESHOLD)
        || history.len() <= KEEP_RECENT_COUNT
    {
        return;
    }

    let split_idx = history.len().saturating_sub(KEEP_RECENT_COUNT);
    let to_summarize = &history[..split_idx];
    let recent = history[split_idx..].to_vec();

    let mut conversation_text = String::new();
    for msg in to_summarize {
        conversation_text.push_str(&format!("{}: {}\n", msg.role, msg.content));
    }

    let safe_conversation_text = truncate_with_notice(&conversation_text, 8000);

    let min_debounce = Duration::from_millis(3000);
    if let Some(prev) = *last_call_time {
        let elapsed = prev.elapsed();
        if elapsed < min_debounce {
            tokio::time::sleep(min_debounce - elapsed).await;
        }
    }

    let (_, system_lang_name) = get_system_locale_language();
    let sys_prompt = format!("Tu es un synthétiseur de mémoire conversationnelle. Résume fidèlement les points clés, décisions et actions passées en 250 mots maximum dans la langue du système de l'utilisateur ({system_lang_name}) sous forme de points synthétiques.");
    let messages_to_condense = vec![
        ChatMessage {
            role: "user".to_string(),
            content: format!("Voici les échanges passés à condenser :\n\n{}", safe_conversation_text),
        },
    ];

    let response = send_llm_chat_completion(
        client,
        llm_config,
        &sys_prompt,
        &messages_to_condense,
        0.2,
        400,
    ).await;

    *last_call_time = Some(Instant::now());

    if let Ok(res) = response {
        if res.status().is_success() {
            if let Ok(summary_text) = parse_llm_response_text(res, llm_config.provider).await {
                let summary = summary_text.trim();
                if !summary.is_empty() {
                    println!("[Mémoire] Compaction contextuelle réussie ({} messages condensés).", to_summarize.len());
                    let mut new_history = Vec::with_capacity(recent.len() + 2);
                    new_history.push(ChatMessage {
                        role: "user".to_string(),
                        content: format!("[Note contextuelle - résumé des échanges antérieurs] :\n{}", summary),
                    });
                    new_history.push(ChatMessage {
                        role: "assistant".to_string(),
                        content: "Contexte précédent bien assimilé.".to_string(),
                    });
                    new_history.extend(recent);
                    *history = new_history;
                    return;
                }
            }
        }
    }

    println!("[Mémoire] Repli : éviction fifo sans résumé.");
    *history = recent;
}

pub fn build_standard_system_instructions(lang_name: &str) -> String {
    local_llm::build_routed_system_prompt(local_llm::CommandTag::ALL, lang_name)
}

pub const DICTATION_SYSTEM_INSTRUCTIONS: &str = r#"Tu es l'assistant de dictée vocale et de saisie textuelle pour Windows.
 Tu ne traites AUCUNE tâche d'exploration, navigation web, clic d'interface ou manipulation de fenêtres.
 Tu analyses STRICTEMENT la voix pour retranscrire du texte, déplacer le curseur ou stopper la dictée.
 Réponds STRICTEMENT en JSON sans aucun texte autour.

 Règles de décision :
 1. Arrêt de la dictée ("deactivate_dictation") :
    - Si l'utilisateur demande d'arrêter, quitter, couper ou stopper la dictée (ex: "arrête la dictée", "stop dictée", "quitter la dictée", "pause dictée", "c'est fini", "termine la dictée") :
      N'écris JAMAIS cette consigne dans le document ! Émets l'action "deactivate_dictation" avec "narration": "Dictée désactivée.".
 2. Contrôle et déplacement du curseur ("cursor_move") :
    - Si l'utilisateur donne un ordre de mise en page ou de déplacement de curseur ("à la ligne", "nouveau paragraphe", "curseur à gauche", "efface le mot", "début de ligne"), utilise l'action "cursor_move" avec le libellé de commande adéquat. Maintiens "narration": "".
 3. Retranscription de texte dicté ("dictate") :
    - Pour tout autre propos dicté : retranscris fidèlement le texte parlé avec ponctuation naturelle et soignée (points, virgules, majuscules).
    - Utilise l'action "dictate" avec "target": null, "window": null, "press_enter": false (ou true si "entrée" explicite).
    - Maintiens STRICTEMENT "narration": "" (silence complet).
 4. Silence / Bruit :
    - Si inintelligible ou bruit de micro : "invalid_request": true, "narration": "", "actions": [].

 Format JSON obligatoire :
 {
   "narration": "",
   "actions": [
     {"action": "dictate", "text": "texte à insérer", "target": null, "window": null, "press_enter": false}
   ],
   "quick_suggestions": []
 }"#;

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum PromptTrigger {
    User,
    Surveillance,
    Immersion { screen_changed: bool },
}

fn url_encode(input: &str) -> String {
    let mut out = String::with_capacity(input.len());
    for b in input.bytes() {
        match b {
            b'a'..=b'z' | b'A'..=b'Z' | b'0'..=b'9' | b'-' | b'_' | b'.' | b'~' => {
                out.push(b as char);
            }
            b' ' => out.push('+'),
            _ => {
                out.push_str(&format!("%{:02X}", b));
            }
        }
    }
    out
}

pub async fn call_llm_prompt(
    llm_config: LlmConfig,
    user_instruction: String,
    full_prompt: String,
    history: Arc<tokio::sync::Mutex<Vec<ChatMessage>>>,
    event_tx: StdSender<AgentEvent>,
    tts_tx: StdSender<TtsCommand>,
    last_call_time: Arc<tokio::sync::Mutex<Option<Instant>>>,
    trigger: PromptTrigger,
    request_id: u64,
) {
    if is_request_cancelled(request_id) {
        return;
    }

    if user_instruction.trim().is_empty() || !user_instruction.chars().any(|c| c.is_alphanumeric()) {
        println!("[LLM] Instruction vide ou bruit parasite ignoré, aucune requête LLM envoyée.");
        let _ = event_tx.send(AgentEvent::StatusChanged(AgentStatus::Idle));
        return;
    }

    AGENT_BUSY.store(true, Ordering::SeqCst);
    let _busy_guard = BusyGuard;
    let _ = event_tx.send(AgentEvent::StatusChanged(AgentStatus::Thinking));

    let is_local_target = llm_config.provider.is_local_or_custom_endpoint(&llm_config.endpoint);
    if llm_config.api_key.is_empty() && !is_local_target {
        let _ = event_tx.send(AgentEvent::ReplaceNarration {
            text: format!("Veuillez renseigner {} dans le fichier .env.", llm_config.provider.env_key_name()),
            quick_suggestions: Vec::new(),
        });
        let _ = event_tx.send(AgentEvent::RequestApiKey);
        let _ = event_tx.send(AgentEvent::StatusChanged(AgentStatus::Idle));
        return;
    }

    let client = reqwest::Client::new();
    let (_, system_lang) = get_system_locale_language();
    let is_dictation = IS_DICTATION_ACTIVE.load(Ordering::SeqCst);
    let local_opts = local_llm::get_local_llm_options();
    let safe_user_instruction = truncate_with_notice(&user_instruction, 1000);
    let safe_full_prompt = truncate_with_notice(&full_prompt, 32000);

    let system_instructions = if is_dictation {
        DICTATION_SYSTEM_INSTRUCTIONS.to_string()
    } else {
        let clean_goal = local_llm::extract_clean_user_message(&user_instruction);
        let tags = local_llm::resolve_query_tags(clean_goal, Some(&safe_full_prompt));
        println!(
            "[Universal LLM Router] Modèle: {:?} ({}) | Consigne: \"{}\" | Tags: {:?}",
            llm_config.provider, llm_config.model, clean_goal, tags
        );
        local_llm::build_routed_system_prompt(&tags, &system_lang)
    };

    {
        let mut hist_guard = history.lock().await;
        hist_guard.push(ChatMessage {
            role: "user".to_string(),
            content: safe_full_prompt.clone(),
        });
    }

    let mut max_agent_passes = TARGET_REASONING_PASSES.load(Ordering::SeqCst).clamp(1, 15);
    let mut has_spoken = false;
    let mut pass = 1;
    let mut consecutive_repeat_count = 0usize;
    let mut last_executed_actions: Vec<String> = Vec::new();

    while pass <= max_agent_passes {
        if is_request_cancelled(request_id) {
            return;
        }
        let _ = event_tx.send(AgentEvent::StatusChanged(AgentStatus::Thinking));

        {
            let mut hist_guard = history.lock().await;
            let mut time_guard = last_call_time.lock().await;
            compact_history_if_needed(&llm_config, &mut hist_guard, &client, &mut time_guard).await;
        }

        if is_request_cancelled(request_id) {
            return;
        }

        let min_debounce = if is_dictation {
            Duration::from_millis(400)
        } else if pass == 1 {
            Duration::from_millis(3000)
        } else {
            Duration::from_millis(2000)
        };
        let prev_call = { *last_call_time.lock().await };
        if let Some(prev) = prev_call {
            let elapsed = prev.elapsed();
            if elapsed < min_debounce {
                tokio::time::sleep(min_debounce - elapsed).await;
            }
        }

        if is_request_cancelled(request_id) {
            return;
        }

        let (temperature, max_tokens) = if is_dictation {
            (0.3, 300)
        } else if llm_config.provider == LlmProvider::Local {
            (0.2, 1000)
        } else {
            (0.5, 1800)
        };

        let mut hist_snapshot = if !local_opts.include_history {
            vec![ChatMessage {
                role: "user".to_string(),
                content: safe_full_prompt.clone(),
            }]
        } else {
            let guard = history.lock().await;
            get_recent_history_capped_at_user_messages(&guard, 4)
        };

        // Régulation préventive du quota de contexte selon le modèle choisi
        let max_quota = llm_config.max_context_tokens();
        let max_input_budget = max_quota.saturating_sub(max_tokens as usize + 256);
        let sys_tokens = estimate_token_count(&system_instructions);
        let mut total_tokens = sys_tokens;
        for msg in &hist_snapshot {
            total_tokens += estimate_token_count(&msg.content) + 4;
        }

        if total_tokens > max_input_budget {
            println!(
                "[LLM Contexte] Quota dépassé pour {:?} ({} tokens estimés > budget {}). Régulation du contexte.",
                llm_config.provider, total_tokens, max_input_budget
            );

            // Élagage des messages d'historique les plus anciens
            while hist_snapshot.len() > 1 && total_tokens > max_input_budget {
                let removed = hist_snapshot.remove(0);
                total_tokens = total_tokens.saturating_sub(estimate_token_count(&removed.content) + 4);
            }

            // Si même le dernier message dépasse le budget, tronquer proportionnellement l'écran plutôt que de tout vider
            if total_tokens > max_input_budget {
                println!("[LLM Contexte] Tronquage proportionnel de l'état d'écran pour respecter le budget.");
                let available_for_msg = max_input_budget.saturating_sub(sys_tokens + 32);
                let max_chars = available_for_msg * 3;
                if let Some(last_msg) = hist_snapshot.last_mut() {
                    if last_msg.content.len() > max_chars {
                        last_msg.content = truncate_with_notice(&last_msg.content, max_chars);
                    }
                }
            }
        }

        let mut attempts = 0;
        const MAX_RETRIES: usize = 3;
        const DEFAULT_RETRY_DELAY: Duration = Duration::from_millis(4000);
        let mut success_payload: Option<AgentResponsePayload> = None;

        loop {
            if is_request_cancelled(request_id) {
                return;
            }

            if llm_config.provider == LlmProvider::Local {
                match local_llm::run_local_llama_completion(
                    &system_instructions,
                    &hist_snapshot,
                    max_tokens,
                    &event_tx,
                    Some(&tts_tx),
                ) {
                    Ok(raw_content) => {
                        history.lock().await.push(ChatMessage {
                            role: "assistant".to_string(),
                            content: raw_content.clone(),
                        });
                        success_payload = Some(parse_agent_response(&raw_content));
                        break;
                    }
                    Err(err) => {
                        let _ = event_tx.send(AgentEvent::ReplaceNarration {
                            text: format!("Erreur modèle local Qwen: {err}"),
                            quick_suggestions: Vec::new(),
                        });
                        break;
                    }
                }
            }

            attempts += 1;

            let response = send_llm_chat_completion(
                &client,
                &llm_config,
                &system_instructions,
                &hist_snapshot,
                temperature,
                max_tokens,
            ).await;

            match response {
                Ok(res) if res.status().is_success() => {
                    {
                        let mut time_guard = last_call_time.lock().await;
                        *time_guard = Some(Instant::now());
                    }
                    if let Ok(raw_content) = parse_llm_response_text(res, llm_config.provider).await {
                        println!("\n=================== [Réponse {} brute (passe {}/{})] ===================", llm_config.provider.display_name(), pass, max_agent_passes);
                        println!("{}", raw_content.trim());
                        println!("============================================================");

                        history.lock().await.push(ChatMessage {
                            role: "assistant".to_string(),
                            content: raw_content.clone(),
                        });

                        let payload = parse_agent_response(&raw_content);
                        success_payload = Some(payload);
                        break;
                    }
                    history.lock().await.pop();
                    if !is_request_cancelled(request_id) {
                        let _ = event_tx.send(AgentEvent::ReplaceNarration {
                            text: "Format de réponse inattendu.".into(),
                            quick_suggestions: Vec::new(),
                        });
                    }
                    break;
                }
                Ok(res) if res.status() == reqwest::StatusCode::BAD_REQUEST && attempts <= MAX_RETRIES => {
                    let err_body = res.text().await.unwrap_or_default();
                    println!("[LLM 400 Bad Request] Réponse API : {}", err_body);
                    let err_low = err_body.to_lowercase();
                    if err_low.contains("context") || err_low.contains("token") || err_low.contains("length") || err_low.contains("too large") || err_low.contains("maximum") {
                        println!("[LLM Contexte] Dépassement de quota signalé par l'API. Effacement de la mémoire de l'historique et nouvel essai immédiat.");
                        history.lock().await.clear();
                        let _ = event_tx.send(AgentEvent::SilentNarration(
                            "Quota de contexte dépassé : mémoire réinitialisée et nouvel essai.".into(),
                        ));
                        hist_snapshot = vec![ChatMessage {
                            role: "user".to_string(),
                            content: format!("[Demande utilisateur]\n{}", safe_user_instruction),
                        }];
                        continue;
                    }
                    if !is_request_cancelled(request_id) {
                        let _ = event_tx.send(AgentEvent::ReplaceNarration {
                            text: "Format de réponse inattendu.".into(),
                            quick_suggestions: Vec::new(),
                        });
                    }
                    break;
                }
                Ok(res) if res.status() == reqwest::StatusCode::TOO_MANY_REQUESTS && attempts <= MAX_RETRIES => {
                    let wait_secs = res.headers()
                        .get("retry-after")
                        .and_then(|h| h.to_str().ok())
                        .and_then(|s| s.parse::<u64>().ok())
                        .unwrap_or(4);

                    let safe_wait_secs = (wait_secs + 1).max(4);

                    if wait_secs > 12 {
                        let err_body = res.text().await.unwrap_or_default();
                        println!("[LLM 429] Détails renvoyés par l'API : {}", err_body);
                        history.lock().await.pop();
                        let mins = (wait_secs + 59) / 60;
                        if !is_request_cancelled(request_id) {
                            let _ = event_tx.send(AgentEvent::ReplaceNarration {
                                text: format!("Plafond instantané DeepSeek atteint (pause requise par l'API : {mins} min). Historique réinitialisé."),
                                quick_suggestions: Vec::new(),
                            });
                        }
                        history.lock().await.clear();
                        TARGET_REASONING_PASSES.store(5, Ordering::SeqCst);
                        break;
                    }

                    let _ = event_tx.send(AgentEvent::SilentNarration(format!(
                        "Limite de requêtes atteinte. Pause de {safe_wait_secs} s avant réessai..."
                    )));
                    tokio::time::sleep(Duration::from_secs(safe_wait_secs)).await;
                    {
                        let mut time_guard = last_call_time.lock().await;
                        *time_guard = Some(Instant::now());
                    }
                    continue;
                }
                Ok(res) => {
                    {
                        let mut time_guard = last_call_time.lock().await;
                        *time_guard = Some(Instant::now());
                    }
                    history.lock().await.pop();
                    let status = res.status();
                    if !is_request_cancelled(request_id) {
                        let _ = event_tx.send(AgentEvent::ReplaceNarration {
                            text: format!("Erreur API {} : {status}", llm_config.provider.display_name()),
                            quick_suggestions: Vec::new(),
                        });
                    }
                    break;
                }
                Err(_err) if attempts <= MAX_RETRIES => {
                    let _ = event_tx.send(AgentEvent::SilentNarration(
                        "Connexion interrompue, nouvelle tentative dans 3 secondes...".into(),
                    ));
                    tokio::time::sleep(DEFAULT_RETRY_DELAY).await;
                    {
                        let mut time_guard = last_call_time.lock().await;
                        *time_guard = Some(Instant::now());
                    }
                    continue;
                }
                Err(err) => {
                    {
                        let mut time_guard = last_call_time.lock().await;
                        *time_guard = Some(Instant::now());
                    }
                    history.lock().await.pop();
                    if !is_request_cancelled(request_id) {
                        let _ = event_tx.send(AgentEvent::ReplaceNarration {
                            text: format!("Erreur réseau : {err}"),
                            quick_suggestions: Vec::new(),
                        });
                    }
                    break;
                }
            }
        }

        let Some(mut payload) = success_payload else {
            break;
        };

        if is_request_cancelled(request_id) {
            return;
        }

        let requested_passes = if consecutive_repeat_count == 0 {
            payload.next_passes.or_else(|| {
                payload.actions.iter().find_map(|act| match act {
                    AgentAction::SetReasoningPasses { passes, .. } => Some(*passes),
                    _ => None,
                })
            })
        } else {
            None
        };

        let mut stop_requested = false;
        if let Some(np) = requested_passes {
            const MAX_LIMIT: usize = 15;
            if np == 0 || (payload.actions.is_empty() && pass >= 1) {
                max_agent_passes = pass;
                TARGET_REASONING_PASSES.store(max_agent_passes, Ordering::SeqCst);
                stop_requested = true;
                println!("[Agent] Arrêt demandé (next_passes: {np}) : fin après la passe courante ({pass}).");
            } else if np <= pass && pass > 1 {
                max_agent_passes = pass;
                TARGET_REASONING_PASSES.store(max_agent_passes, Ordering::SeqCst);
                stop_requested = true;
                println!("[Agent] Quota de passes atteint (next_passes: {np} <= passe {pass}) : arrêt à cette passe.");
            } else {
                max_agent_passes = np.clamp(pass, MAX_LIMIT);
                TARGET_REASONING_PASSES.store(max_agent_passes, Ordering::SeqCst);
                println!("[Agent] Quota de raisonnement mis à jour via next_passes (+{np}) : {max_agent_passes} passe(s) max");
            }
        }

        if payload.invalid_request
            || payload.narration.trim().eq_ignore_ascii_case("invalid_request")
            || (payload.narration.trim().is_empty() && payload.actions.is_empty())
        {
            println!("[Agent] Requête incomplète, bruit parasite ou incomprise (narration vide sans action) : mode silencieux activé (aucun affichage, TTS ni requête LLM supplémentaire).");
            history.lock().await.pop();
            let _ = event_tx.send(AgentEvent::StatusChanged(AgentStatus::Idle));
            let _ = event_tx.send(AgentEvent::RequestIgnored);
            return;
        }

        let shown_card_ids: Vec<String> = payload.actions.iter().filter_map(|act| match act {
            AgentAction::ShowFloatingCard { id: Some(cid), .. } => Some(cid.clone()),
            _ => None,
        }).collect();

        let mut card_speeches: Vec<String> = Vec::new();
        for act in &payload.actions {
            match act {
                AgentAction::ShowFloatingCard { id, title, card_type, data } => {
                    let card = FloatingCard::from_action_data(id.clone(), title.clone(), card_type, data);
                    let speech = card.build_narrator_speech();
                    if !speech.trim().is_empty() {
                        card_speeches.push(speech);
                    }
                    if matches!(card.content, FloatingContentType::Subtitles { .. }) {
                        let target_win = match &card.content {
                            FloatingContentType::Subtitles { target_window, .. } => target_window.clone(),
                            _ => None,
                        };
                        crate::audio::start_live_subtitles(target_win);
                    }
                    let _ = event_tx.send(AgentEvent::ShowFloatingCard(card));
                }
                AgentAction::CloseFloatingCard { id } => {
                    if shown_card_ids.contains(id) {
                        println!("[Agent] Ignoré close_floating_card(\"{id}\") car la carte a été mise à jour dans la même passe.");
                    } else {
                        let _ = event_tx.send(AgentEvent::CloseFloatingCard(id.clone()));
                    }
                }
                AgentAction::LiveSubtitles { target_window, enabled } => {
                    if *enabled {
                        crate::audio::start_live_subtitles(target_window.clone());
                        let default_title = t!("commands.subtitles_title").to_string();
                        let card = FloatingCard::new(
                            "live_subtitles",
                            target_window.as_deref().unwrap_or(&default_title),
                            FloatingContentType::Subtitles {
                                text: String::new(),
                                target_window: target_window.clone(),
                                history: Vec::new(),
                            },
                        );
                        let _ = event_tx.send(AgentEvent::ShowFloatingCard(card));
                    } else {
                        crate::audio::stop_live_subtitles();
                        let _ = event_tx.send(AgentEvent::LiveSubtitlesClosed);
                    }
                }
                AgentAction::ToggleNumberedTargets { enabled } => {
                    let _ = event_tx.send(AgentEvent::ToggleNumberedTargets(*enabled));
                }
                AgentAction::ClickNumberedTarget { index } => {
                    let _ = event_tx.send(AgentEvent::ClickNumberedTarget(*index));
                }
                AgentAction::NextNumberedChunk => {
                    let _ = event_tx.send(AgentEvent::NextNumberedChunk);
                }
                AgentAction::PreviousNumberedChunk => {
                    let _ = event_tx.send(AgentEvent::PreviousNumberedChunk);
                }
                _ => {}
            }
        }

        if !card_speeches.is_empty() {
            let combined = card_speeches.join("\n\n");
            let trimmed_narr = payload.narration.trim();
            if trimmed_narr.is_empty() {
                payload.narration = combined;
            } else if !trimmed_narr.contains(&combined) {
                let sep = if trimmed_narr.ends_with('.') || trimmed_narr.ends_with(':') || trimmed_narr.ends_with('!') { " " } else { ". " };
                payload.narration = format!("{}{}{}", trimmed_narr, sep, combined);
            }
        }

        for act in &payload.actions {
            if let AgentAction::ShowFloatingCard { title, .. } = act {
                let read_btn_text = t!("ui.read_card_prefix", title = title).to_string();
                if !payload.quick_suggestions.iter().any(|s| s.as_str() == Some(&read_btn_text)) {
                    payload.quick_suggestions.insert(0, QuickSuggestionItem::Text(read_btn_text));
                }
            }
        }

        if let PromptTrigger::Immersion { screen_changed } = trigger {
            let no_actions_or_suggestions = payload.actions.is_empty() && payload.quick_suggestions.is_empty();
            if (!screen_changed && no_actions_or_suggestions) || (payload.narration.trim().is_empty() && no_actions_or_suggestions) {
                println!("[Immersion] Écran inchangé ou silence demandé sans action/suggestion : préservation du silence.");
                let mut hist_guard = history.lock().await;
                hist_guard.pop();
                hist_guard.pop();
                let _ = event_tx.send(AgentEvent::StatusChanged(AgentStatus::Idle));
                return;
            }
        }

        if trigger == PromptTrigger::Surveillance
            && payload.actions.is_empty() && payload.quick_suggestions.is_empty() && payload.narration.trim().is_empty()
        {
            println!("[Surveillance] L'IA a analysé l'écran : aucune action nécessaire.");
            let mut hist_guard = history.lock().await;
            hist_guard.pop();
            hist_guard.pop();
            let _ = event_tx.send(AgentEvent::StatusChanged(AgentStatus::Idle));
            return;
        }

        println!("[Agent] Narration (passe {}) : \"{}\"", pass, payload.narration);
        println!("[Agent] {} action(s) planifiée(s) :", payload.actions.len());
        for (i, act) in payload.actions.iter().enumerate() {
            println!("  [{}] {:?}", i + 1, act);
        }

        if payload.actions.is_empty() {
            let lower_user = safe_user_instruction.to_lowercase();
            let is_action_request = lower_user.starts_with("mets")
                || lower_user.starts_with("lance")
                || lower_user.starts_with("ouvre")
                || lower_user.starts_with("cherche")
                || lower_user.starts_with("trouve")
                || lower_user.starts_with("joue")
                || lower_user.starts_with("écoute")
                || lower_user.starts_with("fais")
                || lower_user.contains("cherche")
                || lower_user.contains("recherche")
                || lower_user.contains("choisis")
                || lower_user.contains("choisi")
                || lower_user.contains("radio")
                || lower_user.contains("musique")
                || lower_user.contains("youtube")
                || lower_user.contains("arte")
                || lower_user.contains("twitch")
                || lower_user.contains("netflix")
                || lower_user.contains("film")
                || lower_user.contains("streaming")
                || lower_user.contains("vidéo")
                || lower_user.contains("video");

            let lower_narration = payload.narration.to_lowercase();
            let is_passive_excuse = lower_narration.contains("il faudrait")
                || lower_narration.contains("faudrait ouvrir")
                || lower_narration.contains("aucune radio")
                || lower_narration.contains("aucun média")
                || lower_narration.contains("aucun resultat")
                || lower_narration.contains("vous pouvez")
                || lower_narration.contains("il existe plusieurs")
                || lower_narration.contains("pour trouver")
                || lower_narration.contains("voici comment")
                || lower_narration.contains("méthode");

            if (is_action_request || is_passive_excuse) && pass == 1 {
                println!("[Agent Résilience] L'IA s'est défaussée sans action pour \"{}\". Forçage d'une action de recherche / média...", safe_user_instruction);
                let target_query = if lower_user.contains("radio") {
                    "https://www.radiofrance.fr".to_string()
                } else if lower_user.contains("arte") {
                    let topic = if lower_user.contains("choisis") || lower_user.contains("choisi") {
                        "documentaire".to_string()
                    } else {
                        url_encode(&safe_user_instruction)
                    };
                    format!("https://www.arte.tv/fr/search/?q={topic}")
                } else if lower_user.contains("twitch") {
                    let topic = url_encode(&safe_user_instruction);
                    format!("https://www.twitch.tv/search?term={topic}")
                } else if lower_user.contains("netflix") {
                    "https://www.netflix.com".to_string()
                } else if lower_user.contains("vidéo") || lower_user.contains("video") || lower_user.contains("film") || lower_user.contains("youtube") {
                    let topic = if lower_user.contains("choisis") || lower_user.contains("choisi") || lower_user.trim() == "mets une vidéo" || lower_user.trim() == "lance une vidéo" {
                        "documentaire+nature+4k"
                    } else {
                        &url_encode(&safe_user_instruction)
                    };
                    format!("https://www.youtube.com/results?search_query={topic}")
                } else {
                    format!("https://www.google.com/search?q={}", url_encode(&safe_user_instruction))
                };
                payload.actions.push(AgentAction::NavigateToUrl {
                    url: target_query,
                    target: Some("new_tab".to_string()),
                    new_window: Default::default(),
                    window: Default::default(),
                });
                payload.narration = format!("J'ouvre le navigateur pour lancer {} immédiatement.", safe_user_instruction);
                stop_requested = false;
                max_agent_passes = 6.max(max_agent_passes);
                TARGET_REASONING_PASSES.store(max_agent_passes, Ordering::SeqCst);
            } else if is_action_request && pass < max_agent_passes && consecutive_repeat_count == 0 {
                let screen_now = {
                    LAST_SCREEN_SUMMARY.lock().ok().and_then(|s| s.clone()).unwrap_or_default().to_lowercase()
                };
                let on_search_engine = screen_now.contains("google") || screen_now.contains("bing") || screen_now.contains("recherche");
                let on_youtube_results = screen_now.contains("youtube.com/results") || screen_now.contains("résultats");
                let on_arte_results = screen_now.contains("arte.tv") && (screen_now.contains("/search/") || screen_now.contains("recherche"));
                let cookie_banner_present = screen_now.contains("cookie") || screen_now.contains("consentement") || screen_now.contains("tout accepter");
                let on_media_player = screen_now.contains("direct") || screen_now.contains("écouter") || screen_now.contains("lecture") || screen_now.contains("play");

                let objective_clearly_done = if lower_user.contains("radio") || lower_user.contains("musique") {
                    !on_search_engine && (screen_now.contains("[@audio:") || screen_now.contains("en direct") || screen_now.contains("pause"))
                } else if lower_user.contains("vidéo") || lower_user.contains("video") || lower_user.contains("film") || lower_user.contains("youtube") || lower_user.contains("arte") || lower_user.contains("twitch") {
                    !on_search_engine && (screen_now.contains("watch?v=") || screen_now.contains("[@video:") || screen_now.contains("pause") || screen_now.contains("plein écran"))
                } else {
                    false
                };

                if !objective_clearly_done && (on_search_engine || on_youtube_results || on_arte_results || cookie_banner_present || on_media_player) {
                    println!("[Agent Vérification Objectif] L'IA s'est arrêtée mais l'écran montre une étape intermédiaire (recherche/youtube/cookies/lecteur). Prolongation des passes...");
                    let continuation_action = if cookie_banner_present {
                        AgentAction::ClickButton { window: None, button_name: "Tout accepter".to_string() }
                    } else if on_search_engine && (lower_user.contains("radio") || lower_user.contains("france inter")) {
                        AgentAction::ClickElement { window: None, target_name: "Écouter en direct".to_string() }
                    } else if on_youtube_results || on_arte_results {
                        AgentAction::ClickElement { window: None, target_name: "video".to_string() }
                    } else {
                        AgentAction::Scroll { direction: "down".to_string(), amount: Some(1), window: None }
                    };
                    payload.actions.push(continuation_action);
                    payload.narration = "Je poursuis la navigation pour finaliser votre demande...".to_string();
                    stop_requested = false;
                    max_agent_passes = (pass + 2).min(15);
                }
            }
        }

        if payload.actions.is_empty() {
            println!("[Agent] Tâche accomplie : aucune action supplémentaire. Fin de la séquence après {} passe(s).", pass);
            if payload.quick_suggestions.is_empty() {
                let screen = {
                    LAST_SCREEN_SUMMARY.lock().ok().and_then(|s| s.clone()).unwrap_or_else(|| {
                        #[cfg(windows)]
                        { summarize_screen_state(None, true) }
                        #[cfg(not(windows))]
                        { summarize_screen_state(None, false) }
                    })
                };
                payload.quick_suggestions = generate_fallback_suggestions_from_screen(&screen);
            }
            if !payload.narration.trim().is_empty() || !payload.quick_suggestions.is_empty() {
                if !sanitize_for_tts(&payload.narration).trim().is_empty() || !payload.quick_suggestions.is_empty() {
                    has_spoken = true;
                }
                let _ = event_tx.send(AgentEvent::ReplaceNarration {
                    text: payload.narration.clone(),
                    quick_suggestions: payload.quick_suggestions.clone(),
                });
            }
            break;
        }

        if is_request_cancelled(request_id) {
            return;
        }

        if !payload.narration.trim().is_empty() || !payload.quick_suggestions.is_empty() {
            if !sanitize_for_tts(&payload.narration).trim().is_empty() || !payload.quick_suggestions.is_empty() {
                has_spoken = true;
            }
            let _ = event_tx.send(AgentEvent::ReplaceNarration {
                text: payload.narration.clone(),
                quick_suggestions: payload.quick_suggestions.clone(),
            });
        }

        let actions_to_run = payload.actions;

        let lower_user_cmd = safe_user_instruction.to_lowercase();
        let is_media_request = lower_user_cmd.contains("radio")
            || lower_user_cmd.contains("radio france")
            || lower_user_cmd.contains("musique")
            || lower_user_cmd.contains("fip")
            || lower_user_cmd.contains("france inter")
            || lower_user_cmd.contains("franceinfo")
            || lower_user_cmd.contains("écouter")
            || lower_user_cmd.contains("stream")
            || lower_user_cmd.contains("vidéo")
            || lower_user_cmd.contains("video")
            || lower_user_cmd.contains("youtube")
            || lower_user_cmd.contains("arte")
            || lower_user_cmd.contains("film");
        if pass == 1 && is_media_request && actions_to_run.iter().any(|a| matches!(a, AgentAction::NavigateToUrl { .. } | AgentAction::OpenBrowser { .. })) {
            stop_requested = false;
            if max_agent_passes < 6 {
                max_agent_passes = 6;
                TARGET_REASONING_PASSES.store(max_agent_passes, Ordering::SeqCst);
            }
            println!("[Agent Radio/Média - NAVIGATE_IN_WEBPAGE] Page ouverte : maintien des passes actives (max: {max_agent_passes}) pour naviguer dans la page web.");
        }

        let current_action_signatures: Vec<String> = actions_to_run
            .iter()
            .filter(|a| !matches!(a, AgentAction::SetReasoningPasses { .. }))
            .map(|a| format!("{:?}", a))
            .collect();

        if !current_action_signatures.is_empty() && current_action_signatures == last_executed_actions {
            consecutive_repeat_count += 1;
            println!("[Agent] Répétition d'action détectée (occurrence {}) : {:?}", consecutive_repeat_count, current_action_signatures);
            if consecutive_repeat_count >= 2 {
                println!("[Agent] Boucle répétitive interrompue (action identique répétée {} fois consécutives).", consecutive_repeat_count + 1);
                if !payload.narration.trim().is_empty() || !payload.quick_suggestions.is_empty() {
                    let _ = event_tx.send(AgentEvent::ReplaceNarration {
                        text: payload.narration.clone(),
                        quick_suggestions: payload.quick_suggestions.clone(),
                    });
                }
                break;
            }
        } else {
            consecutive_repeat_count = 0;
        }
        last_executed_actions = current_action_signatures;

        let has_concrete_actions = actions_to_run.iter().any(|a| {
            !matches!(a, AgentAction::SetReasoningPasses { .. })
        });

        let has_web_action = actions_to_run.iter().any(|a| {
            matches!(
                a,
                AgentAction::OpenBrowser { .. }
                    | AgentAction::NavigateToUrl { .. }
                    | AgentAction::ClickElement { .. }
                    | AgentAction::ClickButton { .. }
                    | AgentAction::Scroll { .. }
                    | AgentAction::SummarizeScreen { .. }
                    | AgentAction::FocusWindow { .. }
            )
        });

        let (report, screen_after) = tokio::task::spawn_blocking(move || {
            if is_request_cancelled(request_id) {
                return ("Actions interrompues.".to_string(), String::new());
            }
            let outcome = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
                let rep = execute_system_actions(&actions_to_run, request_id);
                if is_request_cancelled(request_id) {
                    return (rep, String::new());
                }
                if is_dictation {
                    return (rep, String::new());
                }
                let wait_ms = if has_web_action { 2200 } else { 1200 };
                std::thread::sleep(Duration::from_millis(wait_ms));
                if is_request_cancelled(request_id) {
                    return (rep, String::new());
                }
                #[cfg(windows)]
                let sc = if local_opts.active_window_only {
                    let targets = collect_numbered_targets_for_active_window();
                    let fg_title = get_foreground_window_title().unwrap_or_else(|| "Fenêtre active".to_string());
                    format_compact_window_elements(&fg_title, &targets)
                } else {
                    summarize_screen_state(None, true)
                };
                #[cfg(not(windows))]
                let sc = if local_opts.active_window_only {
                    let targets = collect_numbered_targets_for_active_window();
                    format_compact_window_elements("Fenêtre active", &targets)
                } else {
                    summarize_screen_state(None, false)
                };
                (rep, sc)
            }));
            match outcome {
                Ok(res) => res,
                Err(err) => {
                    eprintln!("[Résilience] Panique interceptée lors de l'exécution des actions : {:?}", err);
                    ("Une action système a rencontré une erreur non critique et a été interrompue en toute sécurité.".to_string(), String::new())
                }
            }
        }).await.unwrap_or_else(|_| ("Erreur d'exécution du worker.".to_string(), String::new()));

        if is_request_cancelled(request_id) {
            return;
        }

        let action_failed = report.contains("introuvable")
            || report.contains("non réactive")
            || report.contains("Ciblage rejeté");
        if action_failed && pass > 1 {
            println!("[Résilience Agent] Échec d'action détecté dans la passe {pass}. Déclenchement de l'undo automatique.");
            let _ = undo_last_system_action();
        }

        const MAX_CLAMP: usize = 15;
        let synced_passes = TARGET_REASONING_PASSES.load(Ordering::SeqCst).clamp(1, MAX_CLAMP);
        if synced_passes > max_agent_passes && !stop_requested {
            max_agent_passes = synced_passes;
            println!("[Agent] Quota de raisonnement resynchronisé avec TARGET_REASONING_PASSES : {max_agent_passes} passe(s) max");
        }

        if is_dictation {
            println!("[Dictée] Action exécutée instantanément, fin de séquence.");
            break;
        }

        if !screen_after.trim().is_empty() {
            if let Ok(mut lock) = LAST_SCREEN_SUMMARY.lock() {
                *lock = Some(screen_after.clone());
            }
        }

        if !stop_requested && pass >= max_agent_passes && has_concrete_actions && max_agent_passes < MAX_CLAMP && consecutive_repeat_count == 0 {
            max_agent_passes += 1;
            TARGET_REASONING_PASSES.store(max_agent_passes, Ordering::SeqCst);
            println!("[Agent] Passe de réévaluation post-action allouée automatiquement (passe {pass}/{max_agent_passes}).");
        }

        if pass < max_agent_passes {
            let mut step_feedback = String::new();
            let include_feedback = local_opts.include_feedback;
            if include_feedback {
                if !screen_after.trim().is_empty() {
                    let screen_after_clean = if screen_after.contains("<html") || screen_after.contains("<table") || screen_after.contains("<body") {
                        html_util::sanitize_html_to_markdown(&screen_after)
                    } else {
                        screen_after.clone()
                    };
                    let max_screen = if screen_after_clean.contains("[Page web") || screen_after_clean.contains("[Navigateur]") {
                        15360
                    } else {
                        12000
                    };
                    let safe_screen = truncate_with_notice(screen_after_clean.trim(), max_screen);
                    step_feedback.push_str(&format!("[État de l'écran suite aux actions] :\n{safe_screen}\n\n"));
                }
                let compact_report = truncate_with_notice(&report, 1500);
                step_feedback.push_str(&format!("[Retour d'exécution étape {pass}] :\n{compact_report}"));
            }

            let active_win_only = local_opts.active_window_only;
            if !active_win_only {
                if let Ok(guard) = OPEN_FLOATING_CARDS.lock() {
                    if !guard.is_empty() {
                        step_feedback.push_str("\n\n[Fenêtres volantes ouvertes à l'écran] :\n");
                        for c in guard.iter() {
                            step_feedback.push_str(&format!("- ID: \"{}\" | Titre: \"{}\" | Type: \"{}\"\n", c.id, c.title, c.card_type));
                            if !c.content_preview.is_empty() {
                                let preview = truncate_with_notice(&c.content_preview, 1500);
                                step_feedback.push_str(&format!("  [Contenu actuel déjà affiché à conserver/fusionner] :\n\"\"\"\n{}\n\"\"\"\n", preview.trim()));
                            }
                        }
                        step_feedback.push_str("RÈGLE DE FUSION IMPÉRATIVE : Conserve et fusionne les éléments déjà présents ci-dessus avec les nouvelles informations dans \"show_floating_card\". Ne ferme JAMAIS la carte (\"close_floating_card\") !");
                    }
                }
            }

            let mut step_prompt = format!(
                "\n\n[Étape {next_pass}] Vérification d'avancement pour l'objectif : \"{}\".\n\
                - RÈGLE LINGUISTIQUE STRICTE : Traduis et formule TOUT impérativement en {system_lang} dans \"narration\" et les suggestions.\n\
                - RÈGLE NARRATION STRICTE : Ne décris et ne cite AUCUN contrôle technique ou composant de l'écran (boutons, liens, cibles #N). Rédige une réponse fluide focalisée UNIQUEMENT sur l'information et le contenu attendu par l'utilisateur.\n\
                - POSTURE CONVIVIALE : Sois bavard, joyeux, lucide et éthique ! N'hésite pas à parler de toi pour papoter sans te focaliser sur le grand nombre d'éléments analysés.\n\
                - TANT QUE L'OBJECTIF N'EST PAS SATISFAIT : continue les passes, choisis l'action suivante en JSON.\n\
                - DÉTECTION DE BOUCLE : si l'action précédente a échoué ou que l'écran stagne, pivote (autre lien/bouton, scroll, navigate_back).\n\
                - Dès que l'objectif initial est ATTEINT (application ouverte, information trouvée, action exécutée, média en cours), ARRÊTE IMMÉDIATEMENT : renvoie \"actions\": [], \"next_passes\": 0 et décris en détail la situation et le résultat dans \"narration\" rédigée en {system_lang}.\n\
                - UN SEUL CLIC PAR PASSE : 1 seule action concrète à la fois. Clique directement sur le résultat du sujet visé avec \"click_element\" ou \"click_button\".\n\
                - Bannière cookies -> clique automatiquement 'Tout accepter' / 'Accepter'.\n\
                - Bannières de notification web / push -> clique sur la croix ✕ pour libérer la vue.\n\
                - Audio, son & paramètres OS -> INTERDICTION de faire 'run_command' (utilise 'set_volume' ou 'mute_audio').\n\
                Réponds STRICTEMENT en JSON.",
                safe_user_instruction,
                next_pass = pass + 1
            );
            if is_media_request {
                if lower_user_cmd.contains("vidéo") || lower_user_cmd.contains("video") || lower_user_cmd.contains("youtube") || lower_user_cmd.contains("arte") || lower_user_cmd.contains("film") {
                    step_prompt.push_str("\n\n- [NAVIGATION VIDÉO EN COURS] : Valide/ferme les cookies si présents ('Tout accepter'), et clique DIRECTEMENT sur le premier résultat vidéo pour lancer la lecture immédiatement.");
                } else {
                    step_prompt.push_str("\n\n- [NAVIGATION MÉDIA EN COURS] : Valide/ferme les cookies si présents, et clique DIRECTEMENT sur 'Écouter', 'Direct', 'Play' ou la station pour lancer la lecture audio.");
                }
            }
            step_feedback.push_str(&step_prompt);

            history.lock().await.push(ChatMessage {
                role: "user".to_string(),
                content: step_feedback,
            });
            {
                let mut time_guard = last_call_time.lock().await;
                *time_guard = Some(Instant::now());
            }
            pass += 1;
        } else {
            println!("[Agent] Nombre maximal de passes ({max_agent_passes}) atteint.");
            break;
        }
    }

    drop(_busy_guard);

    if !is_request_cancelled(request_id) && !has_spoken {
        let _ = event_tx.send(AgentEvent::StatusChanged(AgentStatus::Idle));
    }
}

pub fn spawn_periodic_screen_monitor(cmd_tx: UnboundedSender<AgentCommand>) {
    std::thread::spawn(move || {
        loop {
            std::thread::sleep(Duration::from_secs(60));
            let _ = cmd_tx.send(AgentCommand::PeriodicScreenCheck);
        }
    });
}

pub fn spawn_agent_backend(
    mut cmd_rx: UnboundedReceiver<AgentCommand>,
    cmd_tx: UnboundedSender<AgentCommand>,
    event_tx: StdSender<AgentEvent>,
    tts_tx: StdSender<TtsCommand>,
    llm_config: LlmConfig,
) {
    let bg_llm_config = llm_config.clone();
    let bg_tts_tx = tts_tx.clone();
    let scheduler_cmd_tx = cmd_tx.clone();
    std::thread::spawn(move || {
        let rt = tokio::runtime::Runtime::new().expect("Échec d'initialisation du runtime Tokio");
        init_scheduler(scheduler_cmd_tx, rt.handle().clone());
        rt.block_on(async move {
            let history = Arc::new(tokio::sync::Mutex::new(Vec::<ChatMessage>::new()));
            let last_call_time = Arc::new(tokio::sync::Mutex::new(None));
            let mut current_task: Option<tokio::task::JoinHandle<()>> = None;

            while let Some(cmd) = cmd_rx.recv().await {
                match cmd {
                    AgentCommand::AnalyzeNavigationZones { window_id, window_title, targets, context_meta } => {
                        let client = reqwest::Client::new();
                        let mut current_llm_config = resolve_llm_config();
                        if current_llm_config.provider == LlmProvider::Local {
                            current_llm_config.model = local_llm::get_active_preset().name.to_string();
                        }
                        let event_tx = event_tx.clone();
                        tokio::spawn(async move {
                            let (summary, zones) = analyze_navigation_zones_with_llm(&client, &current_llm_config, &window_title, &targets, context_meta.as_deref()).await;
                            let _ = event_tx.send(AgentEvent::NavigationZonesReady {
                                window_id,
                                window_title,
                                summary,
                                zones,
                            });
                        });
                    }
                    AgentCommand::Prompt(prompt) => {
                        println!("[Backend Tokio] AgentCommand::Prompt reçu : \"{}\"", prompt);
                        if prompt.trim().is_empty() || !prompt.chars().any(|c| c.is_alphanumeric()) {
                            println!("[Agent] Commande vide ignorée (bruit parasite), aucune requête LLM.");
                            let _ = event_tx.send(AgentEvent::StatusChanged(AgentStatus::Idle));
                            continue;
                        }
                        let request_id = CURRENT_REQUEST_ID.fetch_add(1, Ordering::SeqCst) + 1;
                        if let Some(handle) = current_task.take() {
                            handle.abort();
                        }
                        let _ = bg_tts_tx.send(TtsCommand::Stop);
                        IS_EMERGENCY_STOPPED.store(false, Ordering::SeqCst);
                        let initial_passes = 6;
                        TARGET_REASONING_PASSES.store(initial_passes, Ordering::SeqCst);

                        let history = history.clone();
                        let last_call_time = last_call_time.clone();
                        let event_tx = event_tx.clone();
                        let task_tts_tx = bg_tts_tx.clone();
                        let mut current_llm_config = resolve_llm_config();
                        if current_llm_config.provider == LlmProvider::Local {
                            current_llm_config.model = local_llm::get_active_preset().name.to_string();
                        }

                        current_task = Some(tokio::spawn(async move {
                            if is_request_cancelled(request_id) {
                                return;
                            }
                            if let Some(cli_feedback) = try_execute_direct_cli(&prompt) {
                                println!("[Backend Tokio] Commande CLI directe exécutée : {}", cli_feedback);
                                if is_request_cancelled(request_id) { return; }
                                let _ = event_tx.send(AgentEvent::ScreenPayloadSize(0.0));
                                let _ = event_tx.send(AgentEvent::ReplaceNarration {
                                    text: cli_feedback.clone(),
                                    quick_suggestions: Vec::new(),
                                });
                                let _ = event_tx.send(AgentEvent::StatusChanged(AgentStatus::Idle));
                            } else if let Some(text_to_write) = parse_write_command(&prompt) {
                                println!("[Backend Tokio] Commande d'écriture directe détectée");
                                if is_request_cancelled(request_id) { return; }
                                let _ = event_tx.send(AgentEvent::ScreenPayloadSize(0.0));
                                let _ = event_tx.send(AgentEvent::StatusChanged(AgentStatus::Thinking));
                                let narration = tokio::task::spawn_blocking(move || {
                                    write_to_browser_or_txt(&text_to_write)
                                }).await.unwrap_or_else(|_| "Erreur lors de l'écriture.".to_string());
                                if is_request_cancelled(request_id) { return; }
                                let _ = event_tx.send(AgentEvent::ReplaceNarration {
                                    text: narration,
                                    quick_suggestions: Vec::new(),
                                });
                                let _ = event_tx.send(AgentEvent::StatusChanged(AgentStatus::Idle));
                            } else if IS_DICTATION_ACTIVE.load(Ordering::SeqCst) {
                                println!("[Backend Tokio] Mode dictée actif, prompt dédié envoyé");
                                if is_request_cancelled(request_id) { return; }
                                let _ = event_tx.send(AgentEvent::ScreenPayloadSize(0.0));
                                let final_prompt = format!(
                                    "[MODE DICTÉE ACTIF] : L'utilisateur est en mode dictée vocale. Analyse sa voix : s'il s'agit d'une consigne pour arrêter/quitter la dictée, appelle 'deactivate_dictation'. S'il s'agit de texte à insérer, retranscris-le avec ponctuation soignée via 'dictate' (target: null) et garde narration: \"\".\n\n[Demande utilisateur]\n{prompt}"
                                );
                                call_llm_prompt(
                                    current_llm_config,
                                    prompt.clone(),
                                    final_prompt,
                                    history,
                                    event_tx,
                                    task_tts_tx.clone(),
                                    last_call_time,
                                    PromptTrigger::User,
                                    request_id,
                                ).await;
                            } else {
                                println!("[Backend Tokio] Collecte de l'état de l'écran et préparation de l'appel LLM...");
                                let clean_prompt = local_llm::extract_clean_user_message(&prompt);
                                let last_summary_hint = LAST_SCREEN_SUMMARY.lock().ok().and_then(|s| s.clone());
                                let fg_title = get_foreground_window_title();
                                let mut initial_screen_hint = String::new();
                                if let Some(ref title) = fg_title {
                                    initial_screen_hint.push_str(&format!("Fenêtre active: \"{}\"\n", title));
                                }
                                if let Some(ref sum) = last_summary_hint {
                                    let truncated = if sum.len() > 600 { &sum[..600] } else { sum };
                                    initial_screen_hint.push_str(truncated);
                                }
                                let detected_tags = local_llm::resolve_query_tags(clean_prompt, Some(&initial_screen_hint));
                                let is_pure_conversational_or_question = !detected_tags.is_empty()
                                    && detected_tags.iter().all(|t| matches!(
                                        t,
                                        local_llm::CommandTag::GeneralChitChat
                                            | local_llm::CommandTag::ClarificationRequired
                                            | local_llm::CommandTag::UnintelligibleOrNoise
                                            | local_llm::CommandTag::CorrectionOrDisagreement
                                            | local_llm::CommandTag::AgentIdentityAndCapabilities
                                            | local_llm::CommandTag::EmotionalSupportAndEmpathy
                                            | local_llm::CommandTag::ChattyAndEthicalCompanion
                                    ))
                                    && !clean_prompt.to_lowercase().contains("page")
                                    && !clean_prompt.to_lowercase().contains("site")
                                    && !clean_prompt.to_lowercase().contains("écran")
                                    && !clean_prompt.to_lowercase().contains("ecran")
                                    && !clean_prompt.to_lowercase().contains("analyse");

                                let active_win_only = local_llm::get_local_llm_options().active_window_only;
                                let (field_content, screen_state): (Option<String>, String) = tokio::task::spawn_blocking(move || {
                                    if is_pure_conversational_or_question {
                                        return (None, String::new());
                                    }
                                    #[cfg(windows)]
                                    {
                                        if active_win_only {
                                            let targets = collect_numbered_targets_for_active_window();
                                            let fg_title = get_foreground_window_title().unwrap_or_else(|| "Fenêtre active".to_string());
                                            let compact_ui = format_compact_window_elements(&fg_title, &targets);
                                            return (get_active_field_content(), compact_ui);
                                        }
                                        (get_active_field_content(), summarize_screen_state(None, true))
                                    }
                                    #[cfg(not(windows))]
                                    {
                                        if active_win_only {
                                            let targets = collect_numbered_targets_for_active_window();
                                            let compact_ui = format_compact_window_elements("Fenêtre active", &targets);
                                            return (get_active_field_content(), compact_ui);
                                        }
                                        (get_active_field_content(), summarize_screen_state(None, false))
                                    }
                                }).await.unwrap_or((None, String::new()));

                                if is_request_cancelled(request_id) { return; }
                                let mut prompt_sections = Vec::new();

                                if !screen_state.trim().is_empty() && !is_pure_conversational_or_question {
                                    if let Ok(mut lock) = LAST_SCREEN_SUMMARY.lock() {
                                        *lock = Some(screen_state.clone());
                                    }
                                    let screen_state = if screen_state.contains("<html") || screen_state.contains("<table") || screen_state.contains("<body") {
                                        html_util::sanitize_html_to_markdown(&screen_state)
                                    } else {
                                        screen_state
                                    };
                                    let max_screen = if screen_state.contains("[Page web") || screen_state.contains("[Navigateur]") {
                                        15360
                                    } else {
                                        12000
                                    };
                                    let safe_screen = truncate_with_notice(screen_state.trim(), max_screen);
                                    let kb = safe_screen.len() as f32 / 1024.0;
                                    let _ = event_tx.send(AgentEvent::ScreenPayloadSize(kb));
                                    prompt_sections.push(format!("[État actuel de l'écran]\n{safe_screen}"));
                                } else {
                                    let _ = event_tx.send(AgentEvent::ScreenPayloadSize(0.0));
                                }

                                if let Some(content) = field_content {
                                    if !content.trim().is_empty() {
                                        let safe_content = truncate_with_notice(content.trim(), 1200);
                                        prompt_sections.push(format!("[Contenu actuel du champ de saisie]\n\"\"\"\n{safe_content}\n\"\"\""));
                                    }
                                }

                                if !active_win_only && !is_pure_conversational_or_question {
                                    if let Ok(guard) = OPEN_FLOATING_CARDS.lock() {
                                        if !guard.is_empty() {
                                            let mut card_section = String::from("[Fenêtres volantes actuellement ouvertes à l'écran] :\n");
                                            for c in guard.iter() {
                                                card_section.push_str(&format!("- ID: \"{}\" | Titre: \"{}\" | Type: \"{}\"\n", c.id, c.title, c.card_type));
                                                if !c.content_preview.is_empty() {
                                                    let preview = truncate_with_notice(&c.content_preview, 1500);
                                                    card_section.push_str(&format!("  [Contenu actuel déjà affiché à conserver/fusionner] :\n\"\"\"\n{}\n\"\"\"\n", preview.trim()));
                                                }
                                            }
                                            card_section.push_str("Instruction : Ces fenêtres volantes sont déjà affichées à l'écran. Si ta réponse apporte de nouvelles données, une synthèse ou une mise à jour, réémets impérativement \"show_floating_card\" avec le même ID en FUSIONNANT les données existantes. Ne ferme JAMAIS la carte si tu la mets à jour.");
                                            prompt_sections.push(card_section);
                                        }
                                    }
                                }

                                let (_, prompt_lang) = get_system_locale_language();
                                prompt_sections.push(format!(
                                    "[Demande utilisateur]\n{prompt}\n\n[Consignes impératives pour la réponse]\n\
                                    1. Formule impérativement ta \"narration\" et tes suggestions en {prompt_lang}.\n\
                                    2. FOCALISATION EXCLUSIVE : Ne cite et ne décris JAMAIS les composants de l'interface ou les contrôles détectés (boutons, listes de cibles, champs). Ta \"narration\" doit s'adresser directement à l'utilisateur en ne traitant que du contenu pertinent pour sa demande.\n\
                                    3. COMPAGNON BAVARD ET ÉTHIQUE : Sois bavard, joyeux, lucide et éthique ! N'hésite pas à parler de toi à l'utilisateur pour papoter, sans te focaliser sur le grand nombre d'éléments analysés. Si l'utilisateur te salue simplement ou engage la conversation, réponds à sa salutation avec sympathie sans analyser ni commenter l'écran."
                                ));
                                let final_prompt = prompt_sections.join("\n\n");

                                call_llm_prompt(
                                    current_llm_config,
                                    prompt.clone(),
                                    final_prompt,
                                    history,
                                    event_tx,
                                    task_tts_tx,
                                    last_call_time,
                                    PromptTrigger::User,
                                    request_id,
                                ).await;
                            }
                        }));
                    }
                    AgentCommand::ClearHistory => {
                        let _ = CURRENT_REQUEST_ID.fetch_add(1, Ordering::SeqCst);
                        if let Some(handle) = current_task.take() {
                            handle.abort();
                        }
                        let _ = bg_tts_tx.send(TtsCommand::Stop);
                        cancel_all_scheduled_tasks();
                        IS_EMERGENCY_STOPPED.store(false, Ordering::SeqCst);
                        IS_DICTATION_ACTIVE.store(false, Ordering::SeqCst);
                        TARGET_REASONING_PASSES.store(5, Ordering::SeqCst);
                        if let Ok(mut hist) = IMMERSION_SCREEN_HISTORY.lock() {
                            hist.clear();
                        }
                        history.lock().await.clear();
                        if let Ok(mut lock) = LAST_SCREEN_SUMMARY.lock() {
                            *lock = None;
                        }
                    }
                    AgentCommand::SearchTwitch(query) => {
                        let event_tx = event_tx.clone();
                        tokio::spawn(async move {
                            let results = search_twitch_channels(&query).await;
                            let _ = event_tx.send(AgentEvent::TwitchSearchResults(results));
                        });
                    }
                    AgentCommand::PeriodicScreenCheck => {
                        if IS_EMERGENCY_STOPPED.load(Ordering::SeqCst)
                            || bg_llm_config.provider == LlmProvider::Local
                        {
                            continue;
                        }
                        if AGENT_BUSY.load(Ordering::SeqCst)
                            || current_task.as_ref().map_or(false, |h| !h.is_finished())
                        {
                            println!("[Surveillance] Agent occupé, analyse d'écran différée.");
                            continue;
                        }

                        let request_id = CURRENT_REQUEST_ID.fetch_add(1, Ordering::SeqCst) + 1;
                        let history = history.clone();
                        let last_call_time = last_call_time.clone();
                        let event_tx = event_tx.clone();
                        let task_tts_tx = bg_tts_tx.clone();
                        let mut current_llm_config = resolve_llm_config();
                        if current_llm_config.provider == LlmProvider::Local {
                            current_llm_config.model = local_llm::get_active_preset().name.to_string();
                        }

                        current_task = Some(tokio::spawn(async move {
                            if is_request_cancelled(request_id) { return; }
                            let current_summary = tokio::task::spawn_blocking(|| {
                                summarize_screen_state(None, false)
                            }).await.unwrap_or_default();

                            if is_request_cancelled(request_id) { return; }

                            let is_immersion = IS_IMMERSION_ACTIVE.load(Ordering::SeqCst);
                            if is_immersion {
                                let (screen_diff, screen_changed) = {
                                    let mut hist = IMMERSION_SCREEN_HISTORY.lock().unwrap_or_else(|e| e.into_inner());
                                    let last_summary = hist.last().cloned();
                                    hist.push(current_summary.clone());
                                    if hist.len() > 6 {
                                        hist.remove(0);
                                    }
                                    if let Some(prev) = last_summary {
                                        let diff = compute_screen_diff(&prev, &current_summary);
                                        let changed = !diff.trim().is_empty();
                                        (diff, changed)
                                    } else {
                                        (String::new(), true)
                                    }
                                };

                                if !screen_changed {
                                    println!("[Immersion] Aucun changement à l'écran, silence préservé.");
                                    return;
                                }

                                let immersion_prompt = if screen_diff.is_empty() {
                                    format!(
                                        "[Suivi du mode immersion - état initial]\n\n\
                                        [État d'écran actuel] :\n{}\n\n\
                                        Instruction : Session d'immersion active. Consigne de silence absolu : réponds STRICTEMENT avec \"narration\": \"\", \"actions\": [], \"quick_suggestions\": [].",
                                        truncate_with_notice(&current_summary, 1500)
                                    )
                                } else {
                                    format!(
                                        "[Suivi du mode immersion - différentiel des changements]\n\n\
                                        {}\n\n\
                                        Instruction impérative : Analyse UNIQUEMENT et STRICTEMENT les lignes de changement ci-dessus (+ et -). \
                                        Ne commente absolument rien de ce qui est fixe ou inchangé. \
                                        Consigne de silence absolu : si ces changements font partie du travail normal de l'utilisateur, réponds STRICTEMENT avec \"narration\": \"\", \"actions\": [], \"quick_suggestions\": []. \
                                        Ne commente que les éléments modifiés si une intervention ou suggestion est réellement utile.",
                                        truncate_with_notice(&screen_diff, 1800)
                                    )
                                };

                                println!("[Immersion] Changement détecté (diff: {} octets), transmission du diff au LLM...", screen_diff.len());
                                call_llm_prompt(current_llm_config, "Suivi immersion".to_string(), immersion_prompt, history, event_tx, task_tts_tx.clone(), last_call_time, PromptTrigger::Immersion { screen_changed: true }, request_id).await;
                                return;
                            }

                            let prev_summary = {
                                let mut lock = LAST_SCREEN_SUMMARY.lock().unwrap_or_else(|e| e.into_inner());
                                let prev = lock.clone();
                                *lock = Some(current_summary.clone());
                                prev
                            };

                            if let Some(prev) = prev_summary {
                                if prev.trim() == current_summary.trim() {
                                    println!("[Surveillance] Aucun changement à l'écran.");
                                    return;
                                }

                                println!("[Surveillance] Changement détecté, transmission du différentiel au LLM...");
                                let diff_prompt = format!(
                                    "[Surveillance périodique automatique de l'écran]\n\n\
                                    [État d'écran précédent] :\n{}\n\n\
                                    [État d'écran actuel] :\n{}\n\n\
                                    Instruction : Analyse les différences entre ces deux états d'écran. \
                                    Détermine si une intervention ou une suite d'actions est nécessaire ou utile pour assister l'utilisateur.\n\
                                    - Si aucune intervention n'est requise : réponds STRICTEMENT avec \"actions\": [] et \"narration\": \"\" pour préserver le silence.\n\
                                    - Si une intervention est pertinente : renseigne \"narration\" pour expliquer ce que tu constates et fournis la suite d'actions à exécuter dans \"actions\".",
                                    truncate_with_notice(&prev, 1500),
                                    truncate_with_notice(&current_summary, 1500)
                                );

                                call_llm_prompt(current_llm_config, "Surveillance écran".to_string(), diff_prompt, history, event_tx, task_tts_tx, last_call_time, PromptTrigger::Surveillance, request_id).await;
                            } else {
                                println!("[Surveillance] Premier instantané d'écran enregistré.");
                            }
                        }));
                    }
                    AgentCommand::UndoLastAction => {
                        let _ = CURRENT_REQUEST_ID.fetch_add(1, Ordering::SeqCst);
                        if let Some(handle) = current_task.take() {
                            handle.abort();
                        }
                        let _ = bg_tts_tx.send(TtsCommand::Stop);
                        let feedback = if let Some(fb) = undo_last_system_action() {
                            fb
                        } else {
                            #[cfg(windows)]
                            send_hotkey(&[windows::Win32::UI::Input::KeyboardAndMouse::VIRTUAL_KEY(0x11)], windows::Win32::UI::Input::KeyboardAndMouse::VIRTUAL_KEY(0x5A));
                            #[cfg(not(windows))]
                            let _ = std::process::Command::new("xdotool").args(["key", "ctrl+z"]).status();
                            t!("commands.last_action_undone").to_string()
                        };
                        let _ = event_tx.send(AgentEvent::ReplaceNarration {
                            text: feedback.clone(),
                            quick_suggestions: Vec::new(),
                        });
                        let _ = bg_tts_tx.send(TtsCommand::Speak(feedback));
                        let _ = event_tx.send(AgentEvent::StatusChanged(AgentStatus::Idle));
                    }
                }
            }
        });
    });
}