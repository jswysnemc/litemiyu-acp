//! ACP v1 协议数据结构（兼容 `agy acp` 与标准 Agent Client Protocol v1）。
#![allow(dead_code)]

use serde_json::Value;

#[derive(Debug, Clone)]
pub struct SessionSummary {
    pub session_id: String,
    pub cwd: String,
    pub title: String,
    pub updated_at: String,
}

#[derive(Debug, Clone)]
pub struct SlashCommandInfo {
    pub name: String,
    pub description: String,
}

#[derive(Debug, Clone)]
pub struct ConfigChoice {
    pub value: String,
    pub name: String,
    pub description: String,
}

#[derive(Debug, Clone)]
pub struct ConfigOption {
    pub id: String,
    pub name: String,
    pub category: String,
    pub current_value: String,
    pub options: Vec<ConfigChoice>,
}

#[derive(Debug, Clone)]
pub struct ModeInfo {
    pub id: String,
    pub name: String,
    pub description: String,
}

#[derive(Debug, Clone)]
pub struct PermissionOption {
    pub option_id: String,
    pub name: String,
    pub kind: String,
}

#[derive(Debug, Clone)]
pub struct PermissionRequest {
    pub rpc_id: Value,
    pub session_id: String,
    pub tool_call_id: String,
    pub title: String,
    pub kind: String,
    pub raw_input: Option<Value>,
    pub options: Vec<PermissionOption>,
}

#[derive(Debug, Clone, Default)]
pub struct TurnUsage {
    pub input_tokens: u64,
    pub output_tokens: u64,
    pub cached_read_tokens: u64,
    pub thought_tokens: u64,
    pub total_tokens: u64,
}

#[derive(Debug, Clone)]
pub struct PlanItem {
    pub content: String,
    pub status: String,
}

#[derive(Debug, Clone)]
pub struct ToolDiff {
    pub path: String,
    pub old_text: String,
    pub new_text: String,
}

#[derive(Debug, Clone)]
pub enum SessionUpdate {
    UserMessageChunk(String),
    AgentMessageChunk(String),
    AgentThoughtChunk(String),
    ToolCall {
        call_id: String,
        name: String,
        title: String,
        kind: String,
        status: String,
        raw_input: Option<Value>,
        output: Option<String>,
        diffs: Vec<ToolDiff>,
    },
    ToolCallUpdate {
        call_id: String,
        name: Option<String>,
        title: Option<String>,
        kind: Option<String>,
        status: Option<String>,
        raw_input: Option<Value>,
        output: Option<String>,
        diffs: Vec<ToolDiff>,
    },
    Plan(Vec<PlanItem>),
    AvailableCommands(Vec<SlashCommandInfo>),
    ConfigOptions(Vec<ConfigOption>),
    CurrentMode(String),
    SessionTitle(String),
    Usage {
        used: u64,
        size: u64,
    },
}

#[derive(Debug, Clone)]
pub struct ImagePayload {
    pub mime_type: String,
    pub base64_data: String,
}

#[derive(Debug)]
pub enum AcpCommand {
    NewSession {
        cwd: String,
    },
    LoadSession {
        session_id: String,
        cwd: String,
    },
    ListSessions {
        cwd: Option<String>,
    },
    Prompt {
        session_id: String,
        text: String,
        images: Vec<ImagePayload>,
    },
    Cancel {
        session_id: String,
    },
    SetConfigOption {
        session_id: String,
        config_id: String,
        value: String,
    },
    SetMode {
        session_id: String,
        mode_id: String,
    },
    AnswerPermission {
        rpc_id: Value,
        option_id: Option<String>,
    },
    Shutdown,
}

#[derive(Debug)]
pub enum AcpEvent {
    Initialized {
        agent_name: String,
        agent_version: String,
    },
    SessionReady {
        session_id: String,
        config_options: Vec<ConfigOption>,
        modes: Vec<ModeInfo>,
        current_mode: Option<String>,
        current_model_name: Option<String>,
        tier: Option<String>,
        is_load: bool,
    },
    SessionsListed(Vec<SessionSummary>),
    Update {
        session_id: String,
        update: SessionUpdate,
    },
    PermissionRequested(PermissionRequest),
    PromptFinished {
        stop_reason: String,
        usage: Option<TurnUsage>,
    },
    ConfigUpdated(Vec<ConfigOption>),
    Error(String),
    Disconnected(String),
}
