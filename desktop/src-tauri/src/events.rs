#[derive(Debug, Clone)]
pub struct ChatDeltaEvent {
    pub session_id: String,
    pub message_id: String,
    pub delta: String,
    pub chunk_type: String,
    pub is_final: bool,
}

#[derive(Debug, Clone)]
pub struct ChatCompleteEvent {
    pub session_id: String,
    pub message_id: String,
}

#[derive(Debug, Clone, serde::Serialize)]
#[serde(rename_all = "camelCase")]
pub struct TerminalScreenEvent {
    pub session_id: String,
    pub tmux_session_name: String,
    pub pane_id: String,
    pub cursor: i64,
    pub content: String,
    pub cursor_x: u16,
    pub cursor_y: u16,
    pub history_lines: u16,
    pub rows: u16,
    pub cols: u16,
    pub status: String,
}

/// A structured provider/agent stream event (overhaul P2, decision D6). Rides the same WSS
/// envelope lane as terminal screens; subscribable per session. camelCase on the wire — the Rust,
/// DO envelope, and TS `StreamEvent` interface must agree. The transcript renderer is Phase 3;
/// this is plumbing + type only.
#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct StreamEvent {
    pub session_id: String,
    /// Monotonic per turn/session, for ordering/dedup on the client.
    pub seq: u64,
    /// "text" | "tool_call" | "tool_result" | "code" | "mermaid" | "error".
    pub kind: String,
    /// Prose / code body / mermaid source / error message.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub text: Option<String>,
    /// For kind:"code".
    #[serde(skip_serializing_if = "Option::is_none")]
    pub language: Option<String>,
    /// For tool_call/tool_result.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub tool_name: Option<String>,
    /// Structured payload (tool args/result); opaque this phase.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub data: Option<serde_json::Value>,
    /// Last event of a turn.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub r#final: Option<bool>,
}

#[derive(Debug, Clone, serde::Serialize)]
#[serde(rename_all = "camelCase")]
pub struct AgentPlanRunEvent {
    pub plan_id: String,
    pub deleted: bool,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub run: Option<serde_json::Value>,
}

#[derive(Debug, Clone, serde::Serialize)]
#[serde(rename_all = "camelCase")]
pub struct SessionUpdatedEvent {
    pub session_id: String,
    pub session: serde_json::Value,
}
