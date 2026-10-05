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
    TranscriptionPartial(String),
    VoicePromptReady(String),
    TtsFinished,
    TwitchChatReceived(TwitchMessage),
    TwitchSearchResults(Vec<TwitchChannelItem>),
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
    WriteText {
        text: String,
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
}

#[derive(Serialize, Deserialize, Debug, Clone)]
pub struct AgentResponsePayload {
    pub narration: String,
    #[serde(default)]
    pub actions: Vec<AgentAction>,
}

#[derive(Serialize, Deserialize, Debug, Clone)]
pub struct ChatMessage {
    pub role: String,
    pub content: String,
}

#[derive(Serialize, Debug)]
pub struct GroqChatRequest {
    pub model: String,
    pub messages: Vec<ChatMessage>,
    pub temperature: f32,
    pub max_completion_tokens: u32,
    pub top_p: f32,
    pub stream: bool,
}

#[derive(Deserialize, Debug)]
pub struct GroqChoice {
    pub message: ChatMessage,
}

#[derive(Deserialize, Debug)]
pub struct GroqChatResponse {
    pub choices: Vec<GroqChoice>,
}