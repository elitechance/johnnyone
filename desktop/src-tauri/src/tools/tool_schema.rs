use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};

/// Definition of a tool that the desktop agent can execute.
/// Sent to the server during capability negotiation.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ToolDefinition {
    /// Unique name of the tool (e.g., "shell", "filesystem").
    pub name: String,
    /// Human-readable description of what the tool does.
    pub description: String,
    /// JSON Schema describing the tool's input parameters.
    pub input_schema: serde_json::Value,
    /// Whether this tool requires user approval before execution.
    pub requires_approval: bool,
    /// Risk level of the tool for UI display.
    pub risk_level: RiskLevel,
}

/// Risk level classification for tools.
#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "lowercase")]
pub enum RiskLevel {
    Low,
    Medium,
    High,
    Critical,
}

/// Status of a tool execution.
#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq)]
pub enum ToolExecutionStatus {
    Pending,
    Running,
    Completed,
    Failed,
    Cancelled,
}

/// Record of a tool execution for display in the UI.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ToolExecutionRecord {
    /// The call_id from the original ToolCall.
    pub call_id: String,
    /// Name of the tool.
    pub tool_name: String,
    /// Current execution status.
    pub status: ToolExecutionStatus,
    /// Input parameters.
    pub input: serde_json::Value,
    /// Output data (if completed).
    pub output: Option<serde_json::Value>,
    /// Error message (if failed).
    pub error: Option<String>,
    /// When the execution started.
    pub started_at: DateTime<Utc>,
    /// When the execution completed (or failed/cancelled).
    pub completed_at: Option<DateTime<Utc>>,
}

impl ToolExecutionRecord {
    /// Create a new pending execution record.
    pub fn new_pending(call_id: String, tool_name: String, input: serde_json::Value) -> Self {
        Self {
            call_id,
            tool_name,
            status: ToolExecutionStatus::Pending,
            input,
            output: None,
            error: None,
            started_at: Utc::now(),
            completed_at: None,
        }
    }

    /// Mark the execution as running.
    pub fn mark_running(&mut self) {
        self.status = ToolExecutionStatus::Running;
    }

    /// Complete the execution with a ToolResult.
    pub fn complete(&mut self, result: crate::agent::message_types::ToolResult) {
        self.completed_at = Some(Utc::now());
        if result.success {
            self.status = ToolExecutionStatus::Completed;
            self.output = result.output;
        } else {
            self.status = ToolExecutionStatus::Failed;
            self.error = result.error;
        }
    }

    /// Cancel the execution.
    pub fn cancel(&mut self, reason: String) {
        self.status = ToolExecutionStatus::Cancelled;
        self.error = Some(reason);
        self.completed_at = Some(Utc::now());
    }
}
