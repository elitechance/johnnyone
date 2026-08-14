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
    /// Where this session came from. `'user'` = created from /terminal,
    /// `'agent'` = created by the planner/development coordinator. The
    /// /terminal UI filters to user-only via the kind filter on list_sessions.
    #[serde(default = "default_session_kind")]
    pub kind: String,
    pub created_at: String,
    pub updated_at: String,
    /// True when this session attaches to an external tmux session (so the UI
    /// offers "Detach" instead of "Close", and archive won't kill the tmux).
    #[serde(default)]
    pub attached_tmux: bool,
}

fn default_session_kind() -> String {
    "user".to_string()
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct CreateSessionInput {
    pub provider: Option<String>,
    pub model: Option<String>,
    pub working_directory: Option<String>,
    pub title: Option<String>,
    /// Optional. Defaults to `'user'`. Planner/development internals pass
    /// `'agent'` so those sessions don't show up in the /terminal tab strip.
    #[serde(default)]
    pub kind: Option<String>,
    /// Optional. For `shell` sessions, commands run in the pane on first spawn
    /// (e.g. cd into the app + launch an agent CLI). Ignored for other providers.
    #[serde(default)]
    pub setup_commands: Option<String>,
    /// Optional. When set, this session ATTACHES to an existing external tmux
    /// session of this name (e.g. "kloo") instead of spawning a new
    /// `johnnyone_<id>` pane. Such sessions are never killed on archive — closing
    /// the JohnnyOne view only detaches it, leaving the real tmux session running.
    #[serde(default)]
    pub tmux_session_name: Option<String>,
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

/// One configurable review lens (overhaul P7, D1/D14). Persisted as an element of the
/// nullable JSON array in `AgentPlan.validation_config`; when that column is NULL/empty the
/// review fan-out resolves to `default_validation_config` (= today's product/qa/lead triad).
/// `vision` traces to design-authority §3 ("a lens may be vision-capable"); a vision lens's
/// reviewer prompt is told to approve on functional grounds when it can't read a screenshot.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct ValidationLens {
    pub name: String,
    pub provider: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub model: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub prompt: Option<String>,
    /// design-authority §3: a lens may be vision-capable (D14). `#[serde(default)]` so a
    /// legacy/hand-written config lacking the key deserializes to `false`.
    #[serde(default)]
    pub vision: bool,
    pub blocking: bool,
}

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
    pub phase_run_mode: String,
    /// Groups a planning stage-run + a development stage-run into one Initiative.
    pub initiative_id: String,
    /// Lifecycle stage: briefing | planning | development | review | done.
    pub initiative_status: String,
    /// Condition axis: in-progress | needs-attention | blocked (independent of stage).
    pub health: String,
    /// Non-null only for a briefing Initiative: the `kind='user'` chat session
    /// that carries the clarification conversation (overhaul P4, D3). Stays NULL
    /// on every non-briefing row; distinct from `worker_session_id` (the T1
    /// planner session, set only when the brief is accepted).
    pub briefing_session_id: Option<String>,
    pub created_at: String,
    pub updated_at: String,
    /// Nullable JSON array of `ValidationLens` (overhaul P7, D1/D3). NULL/empty → the review
    /// fan-out resolves to `default_validation_config`. Appended last in the positional row map
    /// and every SELECT (order is load-bearing); nullable for deploy-skew safety like the
    /// initiative axes.
    pub validation_config: Option<String>,
    /// Nullable JSON `{mode, profile?, provider?, model?, ctx?}`. NULL/empty = commercial.
    /// Same deploy-skew pattern as `validation_config` (migration 021).
    pub executor_config: Option<String>,
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
    pub phase_index: Option<i64>,
    pub phase_title: Option<String>,
    pub event_type: String,
    pub actor: String,
    pub category: String,
    pub summary: String,
    pub status_before: Option<String>,
    pub status_after: Option<String>,
    pub reason: Option<String>,
    pub verdict: Option<String>,
    pub task_id: Option<String>,
    pub clarification_attempt: Option<i64>,
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
    /// When `worker_provider` is "shell", commands run in the spawned shell on
    /// first launch (e.g. cd + start an agent CLI). Stored on the worker session.
    #[serde(default)]
    pub worker_setup_commands: Option<String>,
    /// Same, for a "shell" `reviewer_provider`. Stored on the plan and read when
    /// each (lazily-created) reviewer shell is spawned.
    #[serde(default)]
    pub reviewer_setup_commands: Option<String>,
}

/// Input for `create_briefing_run` (overhaul P4, D1). Mirrors the briefing-relevant
/// subset of `CreateAgentPlanInput`: an Initiative created at
/// `initiative_status='briefing'` with a draft `brief` (the raw ask) and the
/// providers stored for the later planning kickoff on Accept.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct CreateBriefingInput {
    pub title: Option<String>,
    pub workspace_path: String,
    /// The raw ask; stored as the draft `brief` until Accept composes the final one.
    pub brief: Option<String>,
    pub worker_provider: String,
    pub reviewer_provider: String,
    #[serde(default)]
    pub model: Option<String>,
    /// Optional JSON executor config. NULL/absent = commercial.
    #[serde(default)]
    pub executor_config: Option<String>,
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

#[cfg(test)]
mod tests {
    use super::*;

    fn sample() -> AgentPlan {
        AgentPlan {
            id: "id".into(),
            run_type: "planning".into(),
            title: "t".into(),
            workspace_path: "/w".into(),
            plan_path: "/p".into(),
            status: "draft".into(),
            worker_session_id: None,
            reviewer_session_id: None,
            worker_provider: "claude_code".into(),
            reviewer_provider: "claude_code".into(),
            current_phase_id: None,
            current_phase_index: 0,
            error: None,
            brief: None,
            app_scope: None,
            docs_scope: None,
            reference_paths: None,
            amend_brief: None,
            phase_run_mode: "continue".into(),
            initiative_id: "INIT".into(),
            initiative_status: "planning".into(),
            health: "in-progress".into(),
            briefing_session_id: None,
            created_at: "".into(),
            updated_at: "".into(),
            validation_config: Some("[]".into()),
            executor_config: None,
        }
    }

    /// Pins the DB→worker→ui camelCase naming contract (decision D8's silent-null failure mode):
    /// the Rust field `initiative_id` MUST serialize as `initiativeId`, etc. Fails if
    /// `#[serde(rename_all = "camelCase")]` is dropped or a field is misnamed.
    #[test]
    fn agent_plan_serializes_initiative_axes_as_camelcase() {
        let v = serde_json::to_value(sample()).unwrap();
        // camelCase keys present…
        assert_eq!(v.get("initiativeId").and_then(|x| x.as_str()), Some("INIT"));
        assert_eq!(
            v.get("initiativeStatus").and_then(|x| x.as_str()),
            Some("planning")
        );
        assert_eq!(v.get("health").and_then(|x| x.as_str()), Some("in-progress"));
        // The briefing link serializes camelCase (present as null on a non-briefing row).
        assert!(v.as_object().unwrap().contains_key("briefingSessionId"));
        assert!(v.get("briefing_session_id").is_none());
        // The P7 validation config serializes camelCase (D3): field `validation_config` → key
        // `validationConfig`; snake_case form must be absent.
        assert_eq!(
            v.get("validationConfig").and_then(|x| x.as_str()),
            Some("[]")
        );
        assert!(v.get("validation_config").is_none());
        assert!(v.as_object().unwrap().contains_key("executorConfig"));
        assert!(v.get("executor_config").is_none());
        // …and the snake_case forms are absent (guards against a missing rename_all).
        assert!(v.get("initiative_id").is_none());
        assert!(v.get("initiative_status").is_none());
        // spot-check an existing renamed field so the whole struct's rename_all is proven.
        assert!(v.get("runType").is_some());
    }
}
