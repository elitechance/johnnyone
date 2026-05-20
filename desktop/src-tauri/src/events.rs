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

#[derive(Debug, Clone, serde::Serialize)]
#[serde(rename_all = "camelCase")]
pub struct AgentPlanRunEvent {
    pub plan_id: String,
    pub deleted: bool,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub run: Option<serde_json::Value>,
}
