use crate::state::app_state::AppState;
use crate::tools::tool_schema::{ToolExecutionRecord, ToolExecutionStatus};
use serde::Serialize;
use tauri::State;

#[derive(Debug, Serialize)]
pub struct ToolExecutionResponse {
    pub call_id: String,
    pub tool_name: String,
    pub status: String,
    pub input: serde_json::Value,
    pub output: Option<serde_json::Value>,
    pub error: Option<String>,
    pub started_at: String,
    pub completed_at: Option<String>,
}

#[derive(Debug, Serialize)]
pub struct PendingApprovalResponse {
    pub call_id: String,
    pub tool_name: String,
    pub description: String,
    pub input: serde_json::Value,
    pub requested_at: String,
}

impl From<&ToolExecutionRecord> for ToolExecutionResponse {
    fn from(record: &ToolExecutionRecord) -> Self {
        Self {
            call_id: record.call_id.clone(),
            tool_name: record.tool_name.clone(),
            status: match record.status {
                ToolExecutionStatus::Pending => "pending".to_string(),
                ToolExecutionStatus::Running => "running".to_string(),
                ToolExecutionStatus::Completed => "completed".to_string(),
                ToolExecutionStatus::Failed => "failed".to_string(),
                ToolExecutionStatus::Cancelled => "cancelled".to_string(),
            },
            input: record.input.clone(),
            output: record.output.clone(),
            error: record.error.clone(),
            started_at: record.started_at.to_rfc3339(),
            completed_at: record.completed_at.map(|t| t.to_rfc3339()),
        }
    }
}

/// Get all tool executions (recent history).
#[tauri::command]
pub async fn get_tool_executions(
    state: State<'_, AppState>,
) -> Result<Vec<ToolExecutionResponse>, String> {
    let executions = state.tool_executions.lock().await;
    let response: Vec<ToolExecutionResponse> = executions.iter().map(|r| r.into()).collect();
    Ok(response)
}

/// Get tool calls that are pending user approval.
#[tauri::command]
pub async fn get_pending_approvals(
    state: State<'_, AppState>,
) -> Result<Vec<PendingApprovalResponse>, String> {
    let executions = state.tool_executions.lock().await;
    let pending: Vec<PendingApprovalResponse> = executions
        .iter()
        .filter(|r| matches!(r.status, ToolExecutionStatus::Pending))
        .map(|r| PendingApprovalResponse {
            call_id: r.call_id.clone(),
            tool_name: r.tool_name.clone(),
            description: format!("Execute {} tool", r.tool_name),
            input: r.input.clone(),
            requested_at: r.started_at.to_rfc3339(),
        })
        .collect();
    Ok(pending)
}

/// Approve a pending tool execution.
#[tauri::command]
pub async fn approve_tool(call_id: String, state: State<'_, AppState>) -> Result<(), String> {
    tracing::info!(call_id = %call_id, "Approving tool execution");

    let mut executions = state.tool_executions.lock().await;
    if let Some(record) = executions.iter_mut().find(|r| r.call_id == call_id) {
        if matches!(record.status, ToolExecutionStatus::Pending) {
            record.status = ToolExecutionStatus::Running;
            // The tool dispatcher will pick this up and execute it
            Ok(())
        } else {
            Err(format!("Tool {} is not pending approval", call_id))
        }
    } else {
        Err(format!("Tool execution {} not found", call_id))
    }
}

/// Deny a pending tool execution.
#[tauri::command]
pub async fn deny_tool(call_id: String, state: State<'_, AppState>) -> Result<(), String> {
    tracing::info!(call_id = %call_id, "Denying tool execution");

    let mut executions = state.tool_executions.lock().await;
    if let Some(record) = executions.iter_mut().find(|r| r.call_id == call_id) {
        if matches!(record.status, ToolExecutionStatus::Pending) {
            record.status = ToolExecutionStatus::Cancelled;
            record.error = Some("Denied by user".to_string());
            record.completed_at = Some(chrono::Utc::now());
            Ok(())
        } else {
            Err(format!("Tool {} is not pending approval", call_id))
        }
    } else {
        Err(format!("Tool execution {} not found", call_id))
    }
}
