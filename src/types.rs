use serde::{Deserialize, Serialize};
pub use crate::floating_window::FloatingCard;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum SwitchAccessStep {
    Windows,
    Zones,
    Elements,
}

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq)]
pub struct NumberedElementTarget {
    pub index: usize,
    pub click_x: i32,
    pub click_y: i32,
    pub rect: [i32; 4],
    pub name: String,
    #[serde(default)]
    pub role: String,
    #[serde(default)]
    pub shortcut: String,
    #[serde(default)]
    pub value: String,
    #[serde(default)]
    pub window_id: Option<isize>,
}

impl NumberedElementTarget {
    pub fn is_text_reading(&self) -> bool {
        self.role == "lecture textuelle"
    }

    pub fn is_editable(&self) -> bool {
        self.role == "champ de saisie" || self.role == "zone de texte"
    }

    pub fn is_label(&self) -> bool {
        self.role == "étiquette" || self.role == "label" || self.role == "texte"
    }

    pub fn is_ocr(&self) -> bool {
        self.role == "visuel (ocr)" || self.role == "ocr"
    }

    /// Vérifie si deux cibles partagent la même signature logique (nom et rôle)
    pub fn has_same_signature(&self, other: &Self) -> bool {
        self.role == other.role && self.name.eq_ignore_ascii_case(&other.name)
    }

    /// Proximité physique entre deux cibles (tolérance en pixels)
    pub fn is_near(&self, other: &Self, tolerance: i32) -> bool {
        (self.click_x - other.click_x).abs() <= tolerance && (self.click_y - other.click_y).abs() <= tolerance
    }
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct NavigationZone {
    pub title: String,
    pub description: String,
    pub click_x: i32,
    pub click_y: i32,
    pub rect: [i32; 4],
    pub targets: Vec<NumberedElementTarget>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SwitchKeyEvent {
    Select,
    Next,
    Prev,
    Back,
    Escape,
    ToggleSwitch,
    ToggleDictation,
    ToggleVoice,
    Clear,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum LlmProvider {
    DeepSeek,
    OpenAI,
    Anthropic,
    Mistral,
    Glm,
    Groq,
}

impl LlmProvider {
    pub const ALL: &'static [Self] = &[
        Self::DeepSeek,
        Self::OpenAI,
        Self::Anthropic,
        Self::Mistral,
        Self::Glm,
        Self::Groq,
    ];

    pub fn display_name(&self) -> &'static str {
        match self {
            Self::DeepSeek => "DeepSeek",
            Self::OpenAI => "OpenAI",
            Self::Anthropic => "Anthropic",
            Self::Mistral => "Mistral AI",
            Self::Glm => "Zhipu GLM",
            Self::Groq => "Groq",
        }
    }

    pub fn env_key_name(&self) -> &'static str {
        match self {
            Self::DeepSeek => "DEEPSEEK_API_KEY",
            Self::OpenAI => "OPENAI_API_KEY",
            Self::Anthropic => "ANTHROPIC_API_KEY",
            Self::Mistral => "MISTRAL_API_KEY",
            Self::Glm => "GLM_API_KEY",
            Self::Groq => "GROQ_API_KEY",
        }
    }

    pub fn env_model_name(&self) -> &'static str {
        match self {
            Self::DeepSeek => "DEEPSEEK_MODEL",
            Self::OpenAI => "OPENAI_MODEL",
            Self::Anthropic => "ANTHROPIC_MODEL",
            Self::Mistral => "MISTRAL_MODEL",
            Self::Glm => "GLM_MODEL",
            Self::Groq => "GROQ_MODEL",
        }
    }

    pub fn default_model(&self) -> &'static str {
        match self {
            Self::DeepSeek => "deepseek-chat",
            Self::OpenAI => "gpt-4o",
            Self::Anthropic => "claude-3-5-sonnet-20241022",
            Self::Mistral => "mistral-small-latest",
            Self::Glm => "glm-4-flash",
            Self::Groq => "llama-3.3-70b-versatile",
        }
    }

    pub fn default_endpoint(&self) -> &'static str {
        match self {
            Self::DeepSeek => "https://api.deepseek.com/chat/completions",
            Self::OpenAI => "https://api.openai.com/v1/chat/completions",
            Self::Anthropic => "https://api.anthropic.com/v1/messages",
            Self::Mistral => "https://api.mistral.ai/v1/chat/completions",
            Self::Glm => "https://open.bigmodel.cn/api/paas/v4/chat/completions",
            Self::Groq => "https://api.groq.com/openai/v1/chat/completions",
        }
    }

    pub fn from_name(s: &str) -> Option<Self> {
        match s.trim().to_lowercase().as_str() {
            "deepseek" => Some(Self::DeepSeek),
            "openai" => Some(Self::OpenAI),
            "anthropic" | "claude" => Some(Self::Anthropic),
            "mistral" | "mistralai" => Some(Self::Mistral),
            "glm" | "zhipu" | "zhipuai" => Some(Self::Glm),
            "groq" => Some(Self::Groq),
            _ => None,
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct LlmConfig {
    pub provider: LlmProvider,
    pub api_key: String,
    pub model: String,
    pub endpoint: String,
}

impl Default for LlmConfig {
    fn default() -> Self {
        Self {
            provider: LlmProvider::DeepSeek,
            api_key: String::new(),
            model: LlmProvider::DeepSeek.default_model().to_string(),
            endpoint: LlmProvider::DeepSeek.default_endpoint().to_string(),
        }
    }
}

#[derive(Serialize, Debug)]
pub struct AnthropicMessagesRequest {
    pub model: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub system: Option<String>,
    pub messages: Vec<ChatMessage>,
    pub max_tokens: u32,
    pub temperature: f32,
}

#[derive(Deserialize, Debug)]
pub struct AnthropicContentBlock {
    #[serde(rename = "type")]
    pub block_type: String,
    pub text: Option<String>,
}

#[derive(Deserialize, Debug)]
pub struct AnthropicMessagesResponse {
    pub content: Vec<AnthropicContentBlock>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum AgentStatus {
    Idle,
    Thinking,
    Speaking,
    Listening,
    EmergencyStopped,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum VoiceProfile {
    Denise,
    Henri,
    Hortense,
    Paul,
    Sylvie,
    Jean,
}

pub type VoiceGender = VoiceProfile;

impl VoiceProfile {
    pub const FEMALE: Self = Self::Denise;
    pub const MALE: Self = Self::Henri;

    pub const ALL: &'static [Self] = &[
        Self::Denise,
        Self::Henri,
        Self::Hortense,
        Self::Paul,
        Self::Sylvie,
        Self::Jean,
    ];

    pub fn display_name(&self) -> &'static str {
        match self {
            Self::Denise => "Denise (Féminine naturelle)",
            Self::Henri => "Henri (Masculin naturel)",
            Self::Hortense => "Hortense (Féminine studio)",
            Self::Paul => "Paul (Masculin posé)",
            Self::Sylvie => "Sylvie (Féminine canadienne)",
            Self::Jean => "Jean (Masculin dynamique)",
        }
    }

    pub fn next(&self) -> Self {
        match self {
            Self::Denise => Self::Henri,
            Self::Henri => Self::Hortense,
            Self::Hortense => Self::Paul,
            Self::Paul => Self::Sylvie,
            Self::Sylvie => Self::Jean,
            Self::Jean => Self::Denise,
        }
    }

    pub fn to_key_str(&self) -> &'static str {
        match self {
            Self::Denise => "denise",
            Self::Henri => "henri",
            Self::Hortense => "hortense",
            Self::Paul => "paul",
            Self::Sylvie => "sylvie",
            Self::Jean => "jean",
        }
    }

    pub fn from_id_or_name(s: &str) -> Option<Self> {
        match s.trim().to_lowercase().as_str() {
            "denise" => Some(Self::Denise),
            "henri" => Some(Self::Henri),
            "hortense" => Some(Self::Hortense),
            "paul" => Some(Self::Paul),
            "sylvie" => Some(Self::Sylvie),
            "jean" => Some(Self::Jean),
            _ => None,
        }
    }

    pub fn edge_voice_id(&self) -> &'static str {
        match self {
            Self::Denise => "fr-FR-DeniseNeural",
            Self::Henri => "fr-FR-HenriNeural",
            Self::Hortense => "fr-FR-EloiseNeural",
            Self::Paul => "fr-FR-AlainNeural",
            Self::Sylvie => "fr-CA-SylvieNeural",
            Self::Jean => "fr-FR-JeanNeural",
        }
    }
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

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CachedWindowAnalysis {
    pub summary: String,
    pub zones: Vec<NavigationZone>,
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
    WhisperReady,
    TtsFinished,
    ScreenPayloadSize(f32),
    TwitchChatReceived(TwitchMessage),
    TwitchSearchResults(Vec<TwitchChannelItem>),
    RequestIgnored,
    RequestApiKey,
    ShowFloatingCard(FloatingCard),
    CloseFloatingCard(String),
    ToggleNumberedTargets(Option<bool>),
    ClickNumberedTarget(usize),
    NextNumberedChunk,
    PreviousNumberedChunk,
    NavigationZonesReady {
        window_id: isize,
        window_title: String,
        summary: String,
        zones: Vec<NavigationZone>,
    },
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
    SetGender(VoiceGender),
}

#[derive(Debug, Clone)]
pub enum AgentCommand {
    Prompt(String),
    ClearHistory,
    SearchTwitch(String),
    PeriodicScreenCheck,
    UndoLastAction,
    AnalyzeNavigationZones {
        window_id: isize,
        window_title: String,
        targets: Vec<NumberedElementTarget>,
        context_meta: Option<String>,
    },
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
    #[serde(alias = "search_windows", alias = "windows_search", alias = "system_search")]
    OpenSearch,
    #[serde(alias = "search_browser", alias = "browser_search", alias = "search_web")]
    OpenSearchBrowser,
    #[serde(alias = "browser_back", alias = "history_back", alias = "back", alias = "retour", alias = "retour_arriere")]
    NavigateBack,
    #[serde(alias = "browser_forward", alias = "history_forward", alias = "forward", alias = "avance")]
    NavigateForward,
    #[serde(alias = "undo_action", alias = "revert_action")]
    Undo,
    #[serde(alias = "redo_action")]
    Redo,
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
    #[serde(alias = "address_bar", alias = "select_address_bar", alias = "barre_adresse")]
    FocusAddressBar {
        #[serde(default)]
        window: Option<String>,
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
    #[serde(alias = "cancel_scheduled_prompts", alias = "clear_scheduled_tasks", alias = "cancel_timers", alias = "annuler_differe")]
    CancelScheduledTasks,
    #[serde(alias = "mute_sound", alias = "sound_mute", alias = "couper_son")]
    MuteAudio,
    #[serde(alias = "volume", alias = "adjust_volume", alias = "sound")]
    SetVolume {
        #[serde(default, alias = "level", alias = "value", alias = "type", alias = "mode")]
        command: String,
    },
    #[serde(alias = "open_document", alias = "launch_file", alias = "open_media")]
    OpenFile {
        path: String,
    },
    #[serde(alias = "show_floating_window", alias = "floating_card", alias = "display_card")]
    ShowFloatingCard {
        id: Option<String>,
        title: String,
        #[serde(default)]
        card_type: String,
        #[serde(default)]
        data: serde_json::Value,
    },
    CloseFloatingCard {
        id: String,
    },
    #[serde(alias = "toggle_numbers", alias = "show_numbers", alias = "show_numbered_badges", alias = "numbers", alias = "display_numbered_badges")]
    ToggleNumberedTargets {
        #[serde(default)]
        enabled: Option<bool>,
    },
    #[serde(alias = "click_badge", alias = "select_number", alias = "click_number")]
    ClickNumberedTarget {
        index: usize,
    },
    #[serde(alias = "next_badges", alias = "prochains_badges", alias = "badges_suivants")]
    NextNumberedChunk,
    #[serde(alias = "prev_badges", alias = "badges_precedents")]
    PreviousNumberedChunk,
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
                AgentAction::OpenSearch => Some("Recherche Windows (OS)".to_string()),
                AgentAction::OpenSearchBrowser => Some("Recherche Web (Navigateur)".to_string()),
                AgentAction::NavigateBack => Some("Page précédente (Retour)".to_string()),
                AgentAction::NavigateForward => Some("Page suivante (Avance)".to_string()),
                AgentAction::FocusAddressBar { .. } => Some("Sélectionner la barre d'adresse".to_string()),
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
                AgentAction::CancelScheduledTasks => Some("Annuler les commandes différées".to_string()),
                AgentAction::MuteAudio => Some("Couper les flux audio parasites".to_string()),
                AgentAction::SetVolume { command } => Some(format!("Volume : {command}")),
                AgentAction::ShowFloatingCard { title, .. } => Some(format!("Afficher : {title}")),
                AgentAction::CloseFloatingCard { id } => Some(format!("Fermer {id}")),
                AgentAction::ToggleNumberedTargets { enabled } => match enabled {
                    Some(true) => Some("Afficher les numéros".to_string()),
                    Some(false) => Some("Masquer les numéros".to_string()),
                    None => Some("Basculer les numéros".to_string()),
                },
                AgentAction::ClickNumberedTarget { index } => Some(format!("Cliquer sur le numéro {index}")),
                AgentAction::NextNumberedChunk => Some("50 badges suivants".to_string()),
                AgentAction::PreviousNumberedChunk => Some("50 badges précédents".to_string()),
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

#[derive(Serialize, Deserialize, Debug, Clone)]
pub struct ChatResponseFormat {
    pub r#type: String,
}

#[derive(Serialize, Debug)]
pub struct DeepSeekChatRequest {
    pub model: String,
    pub messages: Vec<ChatMessage>,
    pub temperature: f32,
    pub max_tokens: u32,
    pub top_p: f32,
    pub stream: bool,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub response_format: Option<ChatResponseFormat>,
}

#[derive(Deserialize, Debug)]
pub struct DeepSeekChoice {
    pub message: ChatMessage,
}

#[derive(Deserialize, Debug)]
pub struct DeepSeekChatResponse {
    pub choices: Vec<DeepSeekChoice>,
}