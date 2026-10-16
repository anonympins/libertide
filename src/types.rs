use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum AgentStatus {
    Idle,
    Thinking,
    Speaking,
    Listening,
    EmergencyStopped,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ActiveTab {
    Assistance,
    Chat,
}

#[derive(Debug, Clone)]
pub struct TwitchMessage {
    pub author: String,
    pub text: String,
    pub timestamp: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct TwitchChannelItem {
    pub login: String,
    pub display_name: String,
    pub profile_image_url: Option<String>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ChatRole {
    User,
    Agent,
}

#[derive(Debug, Clone)]
pub struct ChatEntry {
    pub role: ChatRole,
    pub text: String,
    pub timestamp: String,
    pub quick_suggestions: Vec<QuickSuggestionItem>,
    pub screen_size_kb: Option<f32>,
}

#[derive(Debug, Clone)]
pub enum AgentEvent {
    StatusChanged(AgentStatus),
    NarrationChunk(String),
    ReplaceNarration {
        text: String,
        quick_suggestions: Vec<QuickSuggestionItem>,
    },
    SilentNarration(String),
    TranscriptionPartial(String),
    VoicePromptReady(String),
    TtsFinished,
    ScreenPayloadSize(f32),
    TwitchChatReceived(TwitchMessage),
    TwitchSearchResults(Vec<TwitchChannelItem>),
    RequestIgnored,
    RequestApiKey,
}

#[derive(Debug, Clone)]
pub enum AudioCommand {
    Start,
    Stop,
}

#[derive(Debug, Clone)]
pub enum TtsCommand {
    Speak(String),
    Stop,
}

#[derive(Debug, Clone)]
pub enum AgentCommand {
    Prompt(String),
    ClearHistory,
    SearchTwitch(String),
    PeriodicScreenCheck,
}

#[derive(Serialize, Deserialize, Debug, Clone)]
pub struct ScheduledTaskItem {
    #[serde(default, alias = "delay", alias = "seconds", alias = "wait_seconds", alias = "delay_sec")]
    pub delay_seconds: u64,
    #[serde(default, alias = "instruction", alias = "command", alias = "text")]
    pub prompt: String,
}

#[derive(Serialize, Deserialize, Debug, Clone)]
#[serde(tag = "action", rename_all = "snake_case")]
pub enum AgentAction {
    OpenApp {
        name: String,
    },
    CloseApp {
        name: String,
    },
    ClickElement {
        #[serde(default)]
        window: Option<String>,
        #[serde(alias = "name", alias = "element_name", alias = "link_name", alias = "target")]
        target_name: String,
    },
    ClickButton {
        window: Option<String>,
        button_name: String,
    },
    Scroll {
        #[serde(default)]
        direction: String,
        #[serde(default)]
        amount: Option<i32>,
        #[serde(default)]
        window: Option<String>,
    },
    OpenBrowser {
        url: Option<String>,
    },
    NavigateToUrl {
        url: String,
        #[serde(default)]
        target: Option<String>,
        #[serde(default)]
        window: Option<String>,
        #[serde(default)]
        new_window: Option<bool>,
    },
    WriteText {
        text: String,
        #[serde(default, alias = "target_name")]
        target: Option<String>,
        #[serde(default)]
        window: Option<String>,
        #[serde(default)]
        press_enter: Option<bool>,
    },
    ReplaceFieldText {
        text: String,
        #[serde(default, alias = "target_name")]
        target: Option<String>,
        #[serde(default)]
        window: Option<String>,
    },
    ClearText {
        window: Option<String>,
        #[serde(default, alias = "target_name")]
        target: Option<String>,
    },
    FocusElement {
        window: Option<String>,
        target_name: String,
    },
    RunCommand {
        command: String,
    },
    ArrangeWindow {
        title: String,
        position: String,
    },
    FocusWindow {
        #[serde(default)]
        title: Option<String>,
        #[serde(default)]
        pid: Option<u32>,
    },
    KillProcess {
        #[serde(default)]
        pid: Option<u32>,
        #[serde(default)]
        name: Option<String>,
    },
    TileWindows {
        layout: Option<String>,
        windows: Vec<String>,
    },
    MoveWindow {
        title: String,
        x: i32,
        y: i32,
        width: i32,
        height: i32,
    },
    AccessibilityShortcut {
        shortcut: String,
    },
    SummarizeScreen {
        #[serde(default)]
        window: Option<String>,
    },
    ReadEditorContent {
        #[serde(default)]
        window: Option<String>,
        #[serde(default, alias = "target_name")]
        target: Option<String>,
    },
    ActivateImmersion {
        #[serde(default)]
        apps: Vec<String>,
        #[serde(default)]
        urls: Vec<String>,
        #[serde(default)]
        layout: Option<String>,
    },
    Dictate {
        text: String,
        #[serde(default)]
        window: Option<String>,
        #[serde(default, alias = "target_name")]
        target: Option<String>,
        #[serde(default)]
        press_enter: Option<bool>,
    },
    ActivateDictation {
        #[serde(default)]
        window: Option<String>,
        #[serde(default, alias = "target_name")]
        target: Option<String>,
    },
    ToggleDictation {
        #[serde(default)]
        window: Option<String>,
    },
    #[serde(alias = "move_cursor", alias = "cursor_action")]
    CursorMove {
        #[serde(alias = "direction", alias = "type", alias = "movement")]
        command: String,
    },
    DeactivateImmersion,
    DeactivateDictation,
    SelectTextBetween {
        from: String,
        to: String,
        #[serde(default)]
        window: Option<String>,
        #[serde(default, alias = "target_name")]
        target: Option<String>,
    },
    #[serde(alias = "activate_autostart", alias = "enable_startup", alias = "register_autostart")]
    EnableAutostart,
    #[serde(alias = "deactivate_autostart", alias = "disable_startup", alias = "unregister_autostart")]
    DisableAutostart,
    #[serde(alias = "query_autostart", alias = "get_autostart_status")]
    CheckAutostart,
    #[serde(alias = "set_startup")]
    SetAutostart {
        #[serde(default)]
        enabled: bool,
    },
    SetReasoningPasses {
        passes: usize,
        #[serde(default)]
        reason: Option<String>,
    },
    #[serde(alias = "schedule", alias = "delay_prompt", alias = "schedule_task")]
    SchedulePrompt {
        #[serde(default, alias = "delay", alias = "seconds", alias = "wait_seconds", alias = "delay_sec")]
        delay_seconds: u64,
        #[serde(default, alias = "instruction", alias = "command", alias = "text")]
        prompt: String,
    },
    #[serde(alias = "schedule_tasks", alias = "delayed_prompts")]
    SchedulePrompts {
        #[serde(default, alias = "prompts", alias = "scheduled_tasks", alias = "items")]
        tasks: Vec<ScheduledTaskItem>,
    },
    #[serde(alias = "mute_sound", alias = "sound_mute", alias = "couper_son")]
    MuteAudio,
    #[serde(alias = "volume", alias = "adjust_volume", alias = "sound")]
    SetVolume {
        #[serde(default, alias = "level", alias = "value", alias = "type", alias = "mode")]
        command: String,
    },
}

#[derive(Serialize, Deserialize, Debug, Clone)]
#[serde(untagged)]
pub enum QuickSuggestionItem {
    Text(String),
    Action(AgentAction),
    Value(serde_json::Value),
}

impl QuickSuggestionItem {
    pub fn as_str(&self) -> Option<&str> {
        match self {
            QuickSuggestionItem::Text(s) => Some(s.as_str()),
            QuickSuggestionItem::Value(v) => v.as_str(),
            _ => None,
        }
    }

    pub fn to_display_string(&self) -> Option<String> {
        match self {
            QuickSuggestionItem::Text(s) => Some(s.clone()),
            QuickSuggestionItem::Value(v) => {
                if let Some(s) = v.as_str() {
                    Some(s.to_string())
                } else {
                    Some(v.to_string())
                }
            }
            QuickSuggestionItem::Action(act) => match act {
                AgentAction::OpenApp { name } => Some(format!("Ouvrir {name}")),
                AgentAction::NavigateToUrl { url, .. } => Some(format!("Naviguer vers {url}")),
                AgentAction::ClickElement { target_name, .. } => Some(format!("Cliquer {target_name}")),
                AgentAction::ClickButton { button_name, .. } => Some(format!("Cliquer {button_name}")),
                AgentAction::CloseApp { name } => Some(format!("Fermer {name}")),
                AgentAction::Scroll { direction, .. } => Some(format!("Faire défiler ({direction})")),
                AgentAction::ReadEditorContent { window, .. } => Some(format!("Lire le fichier ({})", window.as_deref().unwrap_or("éditeur"))),
                AgentAction::Dictate { text, .. } => {
                    let preview: String = text.chars().take(22).collect();
                    Some(format!("Dicter : \"{preview}\""))
                }
                AgentAction::CursorMove { command } => {
                    Some(format!("Curseur : {command}"))
                }
                AgentAction::ActivateDictation { .. } => Some("Activer le mode dictée".to_string()),
                AgentAction::DeactivateDictation => Some("Arrêter la dictée".to_string()),
                AgentAction::ToggleDictation { .. } => Some("Basculer la dictée".to_string()),
                AgentAction::SelectTextBetween { from, to, .. } => {
                    Some(format!("Sélectionner de \"{from}\" à \"{to}\""))
                }
                AgentAction::EnableAutostart => Some("Activer le démarrage automatique".to_string()),
                AgentAction::DisableAutostart => Some("Désactiver le démarrage automatique".to_string()),
                AgentAction::CheckAutostart => Some("Statut du démarrage automatique".to_string()),
                AgentAction::SetAutostart { enabled } => {
                    if *enabled {
                        Some("Activer le démarrage automatique".to_string())
                    } else {
                        Some("Désactiver le démarrage automatique".to_string())
                    }
                }
                AgentAction::SetReasoningPasses { passes, .. } => {
                    Some(format!("Ajuster à {passes} passes"))
                }
                AgentAction::SchedulePrompt { delay_seconds, prompt } => {
                    Some(format!("Différer ({delay_seconds}s) : {prompt}"))
                }
                AgentAction::SchedulePrompts { tasks } => {
                    Some(format!("Planifier {} tâche(s) différée(s)", tasks.len()))
                }
                AgentAction::MuteAudio => Some("Couper les flux audio parasites".to_string()),
                AgentAction::SetVolume { command } => Some(format!("Volume : {command}")),
                _ => None,
            },
        }
    }
}

#[derive(Serialize, Deserialize, Debug, Clone)]
pub struct AgentResponsePayload {
    #[serde(default)]
    pub narration: String,
    #[serde(default)]
    pub actions: Vec<AgentAction>,
    #[serde(default)]
    pub quick_suggestions: Vec<QuickSuggestionItem>,
    #[serde(default)]
    pub invalid_request: bool,
    #[serde(default, alias = "max_passes", alias = "reasoning_passes")]
    pub next_passes: Option<usize>,
}

#[derive(Serialize, Deserialize, Debug, Clone)]
pub struct ChatMessage {
    pub role: String,
    pub content: String,
}

#[derive(Serialize, Debug)]
pub struct DeepSeekChatRequest {
    pub model: String,
    pub messages: Vec<ChatMessage>,
    pub temperature: f32,
    pub max_tokens: u32,
    pub top_p: f32,
    pub stream: bool,
}

#[derive(Deserialize, Debug)]
pub struct DeepSeekChoice {
    pub message: ChatMessage,
}

#[derive(Deserialize, Debug)]
pub struct DeepSeekChatResponse {
    pub choices: Vec<DeepSeekChoice>,
}