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
}

#[derive(Debug, Clone)]
pub enum AgentEvent {
    StatusChanged(AgentStatus),
    NarrationChunk(String),
    ReplaceNarration(String),
    SilentNarration(String),
    TranscriptionPartial(String),
    VoicePromptReady(String),
    TtsFinished,
    TwitchChatReceived(TwitchMessage),
    TwitchSearchResults(Vec<TwitchChannelItem>),
    RequestIgnored,
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
#[serde(tag = "action", rename_all = "snake_case")]
pub enum AgentAction {
    OpenApp {
        name: String,
    },
    CloseApp {
        name: String,
    },
    ClickButton {
        window: Option<String>,
        button_name: String,
    },
    OpenBrowser {
        url: Option<String>,
    },
    NavigateToUrl {
        url: String,
        #[serde(default)]
        window: Option<String>,
    },
    WriteText {
        text: String,
        #[serde(default, alias = "target_name")]
        target: Option<String>,
        #[serde(default)]
        window: Option<String>,
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
    ActivateImmersion {
        #[serde(default)]
        apps: Vec<String>,
        #[serde(default)]
        layout: Option<String>,
    },
    DeactivateImmersion,
}

#[derive(Serialize, Deserialize, Debug, Clone)]
pub struct AgentResponsePayload {
    #[serde(default)]
    pub narration: String,
    #[serde(default)]
    pub actions: Vec<AgentAction>,
    #[serde(default)]
    pub invalid_request: bool,
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