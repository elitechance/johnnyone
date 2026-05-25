use serde::{Deserialize, Serialize};

// ── Session ──────────────────────────────────────────────────────────────────

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Session {
    pub id: String,
    pub title: String,
    pub provider: String,
    pub model: String,
    pub working_directory: String,
    pub status: String,
    #[serde(default)]
    pub cli_session_id: Option<String>,
    pub total_input_tokens: i64,
    pub total_output_tokens: i64,
    pub total_cost_cents: i64,
    pub created_at: String,
    pub updated_at: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct CreateSessionInput {
    pub provider: Option<String>,
    pub model: Option<String>,
    pub working_directory: Option<String>,
    pub title: Option<String>,
}

// ── Message ──────────────────────────────────────────────────────────────────

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Message {
    pub id: String,
    pub session_id: String,
    pub role: String,
    pub content: String,
    #[serde(default)]
    pub tool_calls: Option<String>,
    #[serde(default)]
    pub tool_call_id: Option<String>,
    #[serde(default)]
    pub finish_reason: Option<String>,
    pub input_tokens: i64,
    pub output_tokens: i64,
    pub cost_cents: i64,
    pub created_at: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SendMessageInput {
    pub session_id: String,
    pub content: String,
}

// ── Provider Config ──────────────────────────────────────────────────────────

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ProviderConfig {
    pub id: String,
    pub provider: String,
    pub cli_path: String,
    pub api_key: String,
    pub default_model: String,
    pub settings: String,
    pub is_available: bool,
    pub updated_at: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct UpsertProviderConfigInput {
    pub provider: String,
    pub cli_path: Option<String>,
    pub api_key: Option<String>,
    pub default_model: Option<String>,
    pub settings: Option<String>,
}

// ── Agent Planner ───────────────────────────────────────────────────────────

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct AgentPlan {
    pub id: String,
    pub run_type: String,
    pub title: String,
    pub workspace_path: String,
    pub plan_path: String,
    pub status: String,
    pub worker_session_id: Option<String>,
    pub reviewer_session_id: Option<String>,
    pub worker_provider: String,
    pub reviewer_provider: String,
    pub current_phase_id: Option<String>,
    pub current_phase_index: i64,
    pub error: Option<String>,
    pub brief: Option<String>,
    pub app_scope: Option<String>,
    pub docs_scope: Option<String>,
    pub reference_paths: Option<String>,
    /// Non-null when an "amend" cycle is in flight — the brief the user gave
    /// when they clicked Amend. Cleared back to NULL by `commit_plan_on_pass`
    /// after T2 PASSes the amended state.
    pub amend_brief: Option<String>,
    pub created_at: String,
    pub updated_at: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct AgentPlanPhase {
    pub id: String,
    pub plan_id: String,
    pub phase_id: String,
    pub phase_title: String,
    pub phase_index: i64,
    pub status: String,
    pub worker_started_at: Option<String>,
    pub worker_idle_at: Option<String>,
    pub reviewer_started_at: Option<String>,
    pub reviewer_idle_at: Option<String>,
    pub gate_verdict: String,
    pub clarification_attempts: i64,
    pub summary: Option<String>,
    pub findings_json: String,
    pub created_at: String,
    pub updated_at: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct AgentPlanTask {
    pub id: String,
    pub plan_id: String,
    pub phase_id: String,
    pub task_id: String,
    pub task_title: String,
    pub task_index: i64,
    pub prompt_path: String,
    pub status_path: Option<String>,
    pub decisions_path: Option<String>,
    pub status: String,
    pub created_at: String,
    pub updated_at: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct AgentPlanEvent {
    pub id: String,
    pub plan_id: String,
    pub phase_id: Option<String>,
    pub event_type: String,
    pub payload_json: String,
    pub created_at: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct CreateAgentPlanInput {
    pub run_type: Option<String>,
    pub title: Option<String>,
    pub workspace_path: String,
    pub plan_path: String,
    pub worker_provider: String,
    pub reviewer_provider: String,
    pub brief: Option<String>,
    pub app_scope: Option<String>,
    pub docs_scope: Option<String>,
    pub reference_paths: Option<String>,
}

// ── Usage Log ────────────────────────────────────────────────────────────────

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct UsageLogEntry {
    pub id: i64,
    pub session_id: String,
    pub message_id: Option<String>,
    pub provider: String,
    pub model: String,
    pub input_tokens: i64,
    pub output_tokens: i64,
    pub cost_cents: i64,
    pub created_at: String,
}
