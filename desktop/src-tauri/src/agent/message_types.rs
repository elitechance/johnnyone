use serde::{Deserialize, Serialize};

/// Top-level envelope for all WebSocket messages between the desktop agent
/// and the Cloudflare Worker ChatRelayDO.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AgentEnvelope {
    #[serde(flatten)]
    pub message: AgentMessage,
}

/// Discriminated union of all message types in the agent protocol.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "type", content = "data")]
pub enum AgentMessage {
    /// Server requests the desktop agent to execute a tool.
    #[serde(rename = "tool_call")]
    ToolCall(ToolCall),

    /// Desktop agent returns the result of a tool execution.
    #[serde(rename = "tool_result")]
    ToolResult(ToolResult),

    /// Heartbeat for connection keepalive.
    #[serde(rename = "heartbeat")]
    Heartbeat(Heartbeat),

    /// Error message from either side.
    #[serde(rename = "error")]
    Error(ErrorMessage),

    /// Mobile relay: a chat request forwarded from mobile via worker.
    #[serde(rename = "chat_request")]
    RelayChatRequest(RelayChatRequest),

    /// Desktop → worker: streaming delta from CLI subprocess.
    #[serde(rename = "chat_delta")]
    RelayChatDelta(RelayChatDelta),

    /// Desktop → worker: final response from CLI subprocess.
    #[serde(rename = "chat_complete")]
    RelayChatComplete(RelayChatComplete),

    /// Desktop → worker: a complete message (user or assistant).
    #[serde(rename = "chat_message")]
    RelayChatMessage(RelayChatMessage),

    /// Worker → desktop: RPC request (query sessions/messages from SQLite).
    #[serde(rename = "rpc_request")]
    RpcRequest(RpcRequest),

    /// Desktop → worker: RPC response with query results.
    #[serde(rename = "rpc_response")]
    RpcResponse(RpcResponse),

    /// Desktop → mobile: a session was deleted on the desktop.
    #[serde(rename = "session_deleted")]
    SessionDeleted(SessionDeleted),

    /// Desktop → remote clients: a session was created or updated.
    #[serde(rename = "session_updated")]
    SessionUpdated(SessionUpdated),

    /// Desktop → remote clients: current tmux-visible terminal screen.
    #[serde(rename = "terminal_screen")]
    TerminalScreen(TerminalScreen),

    /// Remote clients → desktop: raw terminal input for a tmux pane.
    #[serde(rename = "terminal_command")]
    TerminalCommand(TerminalCommand),

    /// Remote clients → desktop: visible UI started watching a tmux pane.
    #[serde(rename = "terminal_visual_subscribe")]
    TerminalVisualSubscribe(TerminalVisualSubscription),

    /// Remote clients → desktop: visible UI stopped watching a tmux pane.
    #[serde(rename = "terminal_visual_unsubscribe")]
    TerminalVisualUnsubscribe(TerminalVisualSubscription),

    /// Remote clients → desktop: resize a tmux pane to match the UI terminal.
    #[serde(rename = "terminal_resize")]
    TerminalResize(TerminalResize),

    /// Remote clients → desktop: kill a tmux terminal session.
    #[serde(rename = "terminal_kill")]
    TerminalKill(TerminalKill),

    /// Desktop → remote clients: terminal command acknowledgement.
    #[serde(rename = "terminal_command_ack")]
    TerminalCommandAck(TerminalCommandAck),

    /// Desktop → remote clients: planner run state update.
    #[serde(rename = "agent_plan_run_updated")]
    AgentPlanRunUpdated(AgentPlanRunUpdated),
}

// ── Existing Types ───────────────────────────────────────────────────────────

/// A request to execute a tool on the desktop agent.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ToolCall {
    pub call_id: String,
    pub tool_name: String,
    pub input: serde_json::Value,
    #[serde(default)]
    pub timeout_ms: Option<u64>,
    #[serde(default)]
    pub requires_approval: bool,
}

/// The result of a tool execution.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ToolResult {
    pub call_id: String,
    pub success: bool,
    #[serde(default)]
    pub output: Option<serde_json::Value>,
    #[serde(default)]
    pub error: Option<String>,
    #[serde(default)]
    pub duration_ms: Option<u64>,
}

impl ToolResult {
    pub fn success(call_id: String, output: serde_json::Value, duration_ms: u64) -> Self {
        Self {
            call_id,
            success: true,
            output: Some(output),
            error: None,
            duration_ms: Some(duration_ms),
        }
    }

    pub fn failure(call_id: String, error: String, duration_ms: u64) -> Self {
        Self {
            call_id,
            success: false,
            output: None,
            error: Some(error),
            duration_ms: Some(duration_ms),
        }
    }
}

/// Heartbeat message for connection keepalive.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Heartbeat {
    pub timestamp: String,
    #[serde(default)]
    pub node_id: Option<String>,
    #[serde(default)]
    pub system_info: Option<SystemInfoSnapshot>,
}

/// Brief system info included with periodic heartbeats.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct SystemInfoSnapshot {
    pub cpu_usage_percent: f32,
    pub memory_used_mb: u64,
    pub memory_total_mb: u64,
    pub uptime_seconds: u64,
}

/// Error message in the agent protocol.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ErrorMessage {
    pub code: String,
    pub message: String,
    #[serde(default)]
    pub call_id: Option<String>,
}

// ── Relay Types (Mobile ↔ Desktop via Worker) ────────────────────────────────

/// Chat request relayed from mobile through the worker.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct RelayChatRequest {
    pub relay_id: String,
    pub session_id: String,
    pub content: String,
    #[serde(default)]
    pub provider: Option<String>,
    #[serde(default)]
    pub model: Option<String>,
    #[serde(default)]
    pub working_directory: Option<String>,
    #[serde(default)]
    pub user_id: Option<String>,
    #[serde(default)]
    pub tenant_id: Option<String>,
}

/// Streaming delta sent from desktop back through the relay.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct RelayChatDelta {
    pub relay_id: String,
    pub session_id: String,
    pub delta: String,
    pub chunk_type: String,
    pub is_final: bool,
}

/// Completion signal for a relayed chat request.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct RelayChatComplete {
    pub relay_id: String,
    pub session_id: String,
    pub message_id: String,
}

/// A complete message sent through the relay.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct RelayChatMessage {
    pub relay_id: String,
    pub session_id: String,
    pub role: String,
    pub content: String,
}

// ── RPC Types (Worker ↔ Desktop for query relay) ─────────────────────────────

/// RPC request from the worker to query desktop SQLite data.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct RpcRequest {
    pub request_id: String,
    pub method: String,
    #[serde(default)]
    pub params: serde_json::Value,
}

/// RPC response from the desktop with query results.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct RpcResponse {
    pub request_id: String,
    pub success: bool,
    #[serde(default)]
    pub data: Option<serde_json::Value>,
    #[serde(default)]
    pub error: Option<String>,
}

/// Notification that a session was deleted on the desktop.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct SessionDeleted {
    pub session_id: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct SessionUpdated {
    pub session_id: String,
    pub session: serde_json::Value,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct AgentPlanRunUpdated {
    pub plan_id: String,
    pub deleted: bool,
    #[serde(default)]
    pub run: Option<serde_json::Value>,
}

/// Current terminal screen replicated from tmux.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct TerminalScreen {
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

/// Raw terminal input request from a remote UI.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct TerminalCommand {
    pub request_id: String,
    pub session_id: String,
    pub data: String,
    #[serde(default)]
    pub control: Option<String>,
    #[serde(default)]
    pub attachments: Vec<TerminalCommandAttachment>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct TerminalCommandAttachment {
    pub id: String,
    pub original_name: String,
    pub content_type: String,
    pub size: i64,
}

/// Visual subscription request from a remote UI.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct TerminalVisualSubscription {
    pub request_id: String,
    pub session_id: String,
}

/// Resize request from a remote UI.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct TerminalResize {
    pub request_id: String,
    pub session_id: String,
    pub cols: u16,
    pub rows: u16,
}

/// Kill request from a remote UI.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct TerminalKill {
    pub request_id: String,
    pub session_id: String,
}

/// Acknowledgement for a raw terminal input request.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct TerminalCommandAck {
    pub request_id: String,
    pub session_id: String,
    pub accepted: bool,
    #[serde(default)]
    pub error: Option<String>,
}

/// Session view for RPC responses (camelCase for GraphQL compatibility).
#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct RpcSessionView {
    pub id: String,
    pub title: String,
    pub provider: String,
    pub model: String,
    pub working_directory: String,
    pub status: String,
    pub total_input_tokens: i64,
    pub total_output_tokens: i64,
    pub total_cost_cents: i64,
    pub created_at: String,
    pub updated_at: String,
}

/// Message view for RPC responses (camelCase for GraphQL compatibility).
#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct RpcMessageView {
    pub id: String,
    pub session_id: String,
    pub role: String,
    pub content: String,
    pub tool_calls: Option<String>,
    pub finish_reason: Option<String>,
    pub input_tokens: i64,
    pub output_tokens: i64,
    pub cost_cents: i64,
    pub created_at: String,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_serialize_tool_call() {
        let envelope = AgentEnvelope {
            message: AgentMessage::ToolCall(ToolCall {
                call_id: "abc-123".to_string(),
                tool_name: "shell".to_string(),
                input: serde_json::json!({ "command": "ls -la" }),
                timeout_ms: Some(30000),
                requires_approval: false,
            }),
        };

        let json = serde_json::to_string(&envelope).unwrap();
        assert!(json.contains("tool_call"));
        assert!(json.contains("abc-123"));
    }

    #[test]
    fn test_serialize_relay_request() {
        let envelope = AgentEnvelope {
            message: AgentMessage::RelayChatRequest(RelayChatRequest {
                relay_id: "relay-1".to_string(),
                session_id: "sess-1".to_string(),
                content: "Hello from mobile".to_string(),
                provider: Some("claude_code".to_string()),
                model: None,
                working_directory: Some("/home/user".to_string()),
                user_id: None,
                tenant_id: None,
            }),
        };

        let json = serde_json::to_string(&envelope).unwrap();
        assert!(json.contains("chat_request"));
        assert!(json.contains("Hello from mobile"));
        // Verify camelCase serialization
        assert!(json.contains("relayId"));
        assert!(json.contains("sessionId"));
    }

    #[test]
    fn test_deserialize_heartbeat() {
        let json = r#"{"type":"heartbeat","data":{"timestamp":"2025-01-01T00:00:00Z"}}"#;
        let envelope: AgentEnvelope = serde_json::from_str(json).unwrap();
        match envelope.message {
            AgentMessage::Heartbeat(hb) => {
                assert_eq!(hb.timestamp, "2025-01-01T00:00:00Z");
            }
            _ => panic!("Expected heartbeat"),
        }
    }
}
