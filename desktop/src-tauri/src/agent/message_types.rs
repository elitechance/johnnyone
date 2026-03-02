use serde::{Deserialize, Serialize};

/// Top-level envelope for all WebSocket messages between the desktop agent
/// and the Cloudflare Worker AgentSessionDO.
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
}

/// A request to execute a tool on the desktop agent.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ToolCall {
    /// Unique identifier for this tool call.
    pub call_id: String,
    /// Name of the tool to execute (e.g., "shell", "filesystem", "process").
    pub tool_name: String,
    /// Input parameters for the tool, as a JSON object.
    pub input: serde_json::Value,
    /// Optional timeout in milliseconds for the tool execution.
    #[serde(default)]
    pub timeout_ms: Option<u64>,
    /// Whether this tool call requires user approval before execution.
    #[serde(default)]
    pub requires_approval: bool,
}

/// The result of a tool execution.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ToolResult {
    /// The call_id this result corresponds to.
    pub call_id: String,
    /// Whether the tool execution was successful.
    pub success: bool,
    /// The output data from the tool (if successful).
    #[serde(default)]
    pub output: Option<serde_json::Value>,
    /// Error message (if failed).
    #[serde(default)]
    pub error: Option<String>,
    /// Execution duration in milliseconds.
    #[serde(default)]
    pub duration_ms: Option<u64>,
}

impl ToolResult {
    /// Create a successful tool result.
    pub fn success(call_id: String, output: serde_json::Value, duration_ms: u64) -> Self {
        Self {
            call_id,
            success: true,
            output: Some(output),
            error: None,
            duration_ms: Some(duration_ms),
        }
    }

    /// Create a failed tool result.
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
pub struct Heartbeat {
    /// ISO 8601 timestamp of when the heartbeat was sent.
    pub timestamp: String,
    /// The node ID sending the heartbeat.
    #[serde(default)]
    pub node_id: Option<String>,
    /// System info snapshot (optional, sent periodically).
    #[serde(default)]
    pub system_info: Option<SystemInfoSnapshot>,
}

/// Brief system info included with periodic heartbeats.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SystemInfoSnapshot {
    pub cpu_usage_percent: f32,
    pub memory_used_mb: u64,
    pub memory_total_mb: u64,
    pub uptime_seconds: u64,
}

/// Error message in the agent protocol.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ErrorMessage {
    /// Error code (e.g., "AUTH_FAILED", "TOOL_NOT_FOUND", "TIMEOUT").
    pub code: String,
    /// Human-readable error message.
    pub message: String,
    /// Optional call_id if the error relates to a specific tool call.
    #[serde(default)]
    pub call_id: Option<String>,
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
    fn test_serialize_tool_result() {
        let result = ToolResult::success(
            "abc-123".to_string(),
            serde_json::json!({ "stdout": "hello world" }),
            150,
        );

        let json = serde_json::to_string(&result).unwrap();
        assert!(json.contains("abc-123"));
        assert!(json.contains("true")); // success
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
