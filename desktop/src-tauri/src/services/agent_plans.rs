use crate::db::models::{
    AgentPlan, AgentPlanEvent, AgentPlanPhase, AgentPlanTask, CreateAgentPlanInput,
    CreateSessionInput,
};
use crate::events::AgentPlanRunEvent;
use crate::services::sessions;
use crate::state::app_state::AppState;
use crate::terminal;
use rusqlite::params;
use serde::{Deserialize, Serialize};
use serde_json::json;
use std::collections::HashSet;
use std::fs;
use std::path::{Path, PathBuf};
use tokio::time::{sleep, Duration, Instant};
use uuid::Uuid;

const IDLE_WINDOW_MS: u64 = 2_000;
const CLARIFICATION_LIMIT: i64 = 5;

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct AgentPlanRun {
    pub plan: AgentPlan,
    pub phases: Vec<AgentPlanPhase>,
    pub tasks: Vec<AgentPlanTask>,
    pub events: Vec<AgentPlanEvent>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct HostFileEntry {
    pub path: String,
    pub name: String,
    pub kind: String,
    pub status: Option<String>,
    pub size: Option<u64>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct WorkspaceValidation {
    pub valid: bool,
    pub workspace_path: String,
    pub plan_path: String,
    pub title: Option<String>,
    pub phase_count: i64,
    pub task_count: i64,
    pub error: Option<String>,
}

#[derive(Debug, Clone)]
struct ParsedPlan {
    title: String,
    workspace_path: PathBuf,
    plan_path: PathBuf,
    phases: Vec<ParsedPhase>,
}

#[derive(Debug, Clone)]
struct ParsedPhase {
    phase_id: String,
    title: String,
    tasks: Vec<ParsedTask>,
}

#[derive(Debug, Clone)]
struct ParsedTask {
    task_id: String,
    title: String,
    prompt_path: PathBuf,
    status_path: Option<PathBuf>,
    decisions_path: Option<PathBuf>,
}

pub fn create_plan(state: &AppState, input: CreateAgentPlanInput) -> Result<AgentPlanRun, String> {
    let parsed = parse_plan(&input.workspace_path, &input.plan_path)?;
    let plan_id = Uuid::new_v4().to_string();
    let title = input.title.unwrap_or_else(|| parsed.title.clone());

    let worker_session = sessions::create_session(
        state,
        CreateSessionInput {
            provider: Some(input.worker_provider.clone()),
            model: default_model_for_provider(&input.worker_provider),
            working_directory: Some(parsed.workspace_path.to_string_lossy().to_string()),
            title: Some(format!("T1 Worker - {}", title)),
        },
    )?;
    let reviewer_session = sessions::create_session(
        state,
        CreateSessionInput {
            provider: Some(input.reviewer_provider.clone()),
            model: default_model_for_provider(&input.reviewer_provider),
            working_directory: Some(parsed.workspace_path.to_string_lossy().to_string()),
            title: Some(format!("T2 Reviewer - {}", title)),
        },
    )?;

    state.db.with_conn(|conn| {
        conn.execute(
            "INSERT INTO agent_plans (id, title, workspace_path, plan_path, status, worker_session_id, reviewer_session_id, worker_provider, reviewer_provider, current_phase_id, current_phase_index)
             VALUES (?1, ?2, ?3, ?4, 'draft', ?5, ?6, ?7, ?8, ?9, 0)",
            params![
                plan_id,
                title,
                parsed.workspace_path.to_string_lossy(),
                parsed.plan_path.to_string_lossy(),
                worker_session.id,
                reviewer_session.id,
                input.worker_provider,
                input.reviewer_provider,
                parsed.phases.first().map(|phase| phase.phase_id.as_str()),
            ],
        )
        .map_err(|e| format!("Failed to create agent plan: {}", e))?;

        for (phase_index, phase) in parsed.phases.iter().enumerate() {
            let phase_row_id = Uuid::new_v4().to_string();
            conn.execute(
                "INSERT INTO agent_plan_phases (id, plan_id, phase_id, phase_title, phase_index, status)
                 VALUES (?1, ?2, ?3, ?4, ?5, ?6)",
                params![
                    phase_row_id,
                    plan_id,
                    phase.phase_id,
                    phase.title,
                    phase_index as i64,
                    if phase_index == 0 { "locked" } else { "locked" },
                ],
            )
            .map_err(|e| format!("Failed to create agent plan phase: {}", e))?;

            for (task_index, task) in phase.tasks.iter().enumerate() {
                conn.execute(
                    "INSERT INTO agent_plan_tasks (id, plan_id, phase_id, task_id, task_title, task_index, prompt_path, status_path, decisions_path)
                     VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9)",
                    params![
                        Uuid::new_v4().to_string(),
                        plan_id,
                        phase.phase_id,
                        task.task_id,
                        task.title,
                        task_index as i64,
                        task.prompt_path.to_string_lossy(),
                        task.status_path.as_ref().map(|p| p.to_string_lossy().to_string()),
                        task.decisions_path.as_ref().map(|p| p.to_string_lossy().to_string()),
                    ],
                )
                .map_err(|e| format!("Failed to create agent plan task: {}", e))?;
            }
        }

        Ok(())
    })?;

    append_event(
        state,
        &plan_id,
        None,
        "agent_plan_created",
        json!({ "workerSessionId": worker_session.id, "reviewerSessionId": reviewer_session.id }),
    )?;

    get_plan(state, &plan_id)
}

pub fn list_plans(state: &AppState, status: Option<String>) -> Result<Vec<AgentPlan>, String> {
    state.db.with_conn(|conn| {
        let (sql, param_value);
        let params: Vec<&dyn rusqlite::ToSql>;
        if let Some(ref status) = status {
            sql = "SELECT id, title, workspace_path, plan_path, status, worker_session_id, reviewer_session_id, worker_provider, reviewer_provider, current_phase_id, current_phase_index, error, created_at, updated_at FROM agent_plans WHERE status = ?1 ORDER BY updated_at DESC";
            param_value = status.clone();
            params = vec![&param_value as &dyn rusqlite::ToSql];
        } else {
            sql = "SELECT id, title, workspace_path, plan_path, status, worker_session_id, reviewer_session_id, worker_provider, reviewer_provider, current_phase_id, current_phase_index, error, created_at, updated_at FROM agent_plans ORDER BY updated_at DESC";
            params = vec![];
        }
        let mut stmt = conn.prepare(sql).map_err(|e| e.to_string())?;
        let rows = stmt
            .query_map(params.as_slice(), agent_plan_from_row)
            .map_err(|e| e.to_string())?
            .filter_map(|row| row.ok())
            .collect();
        Ok(rows)
    })
}

pub fn get_plan(state: &AppState, id: &str) -> Result<AgentPlanRun, String> {
    let plan = state.db.with_conn(|conn| {
        conn.query_row(
            "SELECT id, title, workspace_path, plan_path, status, worker_session_id, reviewer_session_id, worker_provider, reviewer_provider, current_phase_id, current_phase_index, error, created_at, updated_at FROM agent_plans WHERE id = ?1",
            params![id],
            agent_plan_from_row,
        )
        .map_err(|e| format!("Agent plan not found: {}", e))
    })?;
    let phases = list_phases(state, id)?;
    let tasks = list_tasks(state, id)?;
    let events = list_events(state, id, 80)?;
    Ok(AgentPlanRun { plan, phases, tasks, events })
}

pub async fn start_plan(state: AppState, id: String) -> Result<AgentPlanRun, String> {
    let run = get_plan(&state, &id)?;
    if matches!(run.plan.status.as_str(), "approved" | "blocked" | "stopped") {
        return Ok(run);
    }
    let phase = current_phase(&run)?;
    let worker_session_id = run
        .plan
        .worker_session_id
        .clone()
        .ok_or_else(|| "Plan has no worker session".to_string())?;
    let reviewer_session_id = run
        .plan
        .reviewer_session_id
        .clone()
        .ok_or_else(|| "Plan has no reviewer session".to_string())?;

    terminal::attach_terminal_headless(&state, worker_session_id.clone(), 120, 36).await?;
    terminal::attach_terminal_headless(&state, reviewer_session_id, 120, 36).await?;

    state.db.with_conn(|conn| {
        conn.execute(
            "UPDATE agent_plans SET status = 'phase_worker_running', current_phase_id = ?1, current_phase_index = ?2, updated_at = datetime('now') WHERE id = ?3",
            params![phase.phase_id, phase.phase_index, id],
        )
        .map_err(|e| e.to_string())?;
        conn.execute(
            "UPDATE agent_plan_phases SET status = 'worker_running', worker_started_at = COALESCE(worker_started_at, datetime('now')), updated_at = datetime('now') WHERE plan_id = ?1 AND phase_id = ?2",
            params![id, phase.phase_id],
        )
        .map_err(|e| e.to_string())
    })?;
    append_event(&state, &id, Some(&phase.phase_id), "agent_phase_started", json!({}))?;

    let prompt = worker_phase_prompt(&run, &phase)?;
    terminal::send_terminal_input(&state, worker_session_id, format!("{}\r", prompt)).await?;

    spawn_coordinator_loop(state.clone(), id.clone()).await;
    get_plan(&state, &id)
}

pub async fn stop_plan(state: &AppState, id: String) -> Result<AgentPlanRun, String> {
    let run = get_plan(state, &id)?;
    if let Some(session_id) = &run.plan.worker_session_id {
        let _ = terminal::kill_terminal_session(state, session_id).await;
        let _ = sessions::archive_session(state, session_id.clone()).await;
    }
    if let Some(session_id) = &run.plan.reviewer_session_id {
        let _ = terminal::kill_terminal_session(state, session_id).await;
        let _ = sessions::archive_session(state, session_id.clone()).await;
    }
    state.db.with_conn(|conn| {
        conn.execute(
            "UPDATE agent_plans SET status = 'stopped', updated_at = datetime('now') WHERE id = ?1",
            params![id],
        )
        .map_err(|e| e.to_string())
    })?;
    append_event(state, &id, None, "agent_plan_stopped", json!({}))?;
    get_plan(state, &id)
}

pub async fn delete_plan(state: &AppState, id: String) -> Result<bool, String> {
    let run = get_plan(state, &id)?;

    if let Some(session_id) = run.plan.worker_session_id {
        let _ = sessions::delete_session(state, session_id).await;
    }
    if let Some(session_id) = run.plan.reviewer_session_id {
        let _ = sessions::delete_session(state, session_id).await;
    }

    state.db.with_conn(|conn| {
        conn.execute("DELETE FROM agent_plan_events WHERE plan_id = ?1", params![id])
            .map_err(|e| format!("Failed to delete plan events: {}", e))?;
        conn.execute("DELETE FROM agent_plan_tasks WHERE plan_id = ?1", params![id])
            .map_err(|e| format!("Failed to delete plan tasks: {}", e))?;
        conn.execute("DELETE FROM agent_plan_phases WHERE plan_id = ?1", params![id])
            .map_err(|e| format!("Failed to delete plan phases: {}", e))?;
        conn.execute("DELETE FROM agent_plans WHERE id = ?1", params![id])
            .map_err(|e| format!("Failed to delete plan: {}", e))?;
        Ok(())
    })?;

    publish_plan_deleted(state, &id);
    Ok(true)
}

pub async fn block_plan(state: &AppState, id: String, reason: String) -> Result<AgentPlanRun, String> {
    state.db.with_conn(|conn| {
        conn.execute(
            "UPDATE agent_plans SET status = 'blocked', error = ?1, updated_at = datetime('now') WHERE id = ?2",
            params![reason, id],
        )
        .map_err(|e| e.to_string())
    })?;
    append_event(state, &id, None, "agent_plan_blocked", json!({ "reason": reason }))?;
    get_plan(state, &id)
}

pub async fn manual_pass_phase(
    state: AppState,
    id: String,
    phase_id: String,
) -> Result<AgentPlanRun, String> {
    let phase = find_phase(&get_plan(&state, &id)?, &phase_id)?;
    if phase.clarification_attempts < CLARIFICATION_LIMIT {
        return Err(format!(
            "Manual pass is available after {} failed clarification attempts",
            CLARIFICATION_LIMIT
        ));
    }
    pass_phase(&state, &id, &phase_id, "Manual pass after failed T2 verdict clarification").await?;
    get_plan(&state, &id)
}

pub async fn send_feedback_to_worker(state: AppState, id: String) -> Result<AgentPlanRun, String> {
    let run = get_plan(&state, &id)?;
    let phase = current_phase(&run)?;
    let worker_session_id = run
        .plan
        .worker_session_id
        .clone()
        .ok_or_else(|| "Plan has no worker session".to_string())?;
    let prompt = feedback_prompt(&phase);
    terminal::send_terminal_input(&state, worker_session_id, format!("{}\r", prompt)).await?;
    state.db.with_conn(|conn| {
        conn.execute(
            "UPDATE agent_plans SET status = 'phase_worker_running', updated_at = datetime('now') WHERE id = ?1",
            params![id],
        )
        .map_err(|e| e.to_string())?;
        conn.execute(
            "UPDATE agent_plan_phases SET status = 'worker_running', updated_at = datetime('now') WHERE plan_id = ?1 AND phase_id = ?2",
            params![id, phase.phase_id],
        )
        .map_err(|e| e.to_string())
    })?;
    append_event(&state, &id, Some(&phase.phase_id), "agent_feedback_sent_to_worker", json!({}))?;
    spawn_coordinator_loop(state.clone(), id.clone()).await;
    get_plan(&state, &id)
}

pub async fn rerun_reviewer(state: AppState, id: String) -> Result<AgentPlanRun, String> {
    let run = get_plan(&state, &id)?;
    let phase = current_phase(&run)?;
    dispatch_review(&state, &run, &phase).await?;
    spawn_coordinator_loop(state.clone(), id.clone()).await;
    get_plan(&state, &id)
}

pub fn validate_workspace_and_plan_path(
    workspace_path: String,
    plan_path: String,
) -> WorkspaceValidation {
    match parse_plan(&workspace_path, &plan_path) {
        Ok(parsed) => {
            let task_count = parsed.phases.iter().map(|phase| phase.tasks.len() as i64).sum();
            WorkspaceValidation {
                valid: true,
                workspace_path: parsed.workspace_path.to_string_lossy().to_string(),
                plan_path: parsed.plan_path.to_string_lossy().to_string(),
                title: Some(parsed.title),
                phase_count: parsed.phases.len() as i64,
                task_count,
                error: None,
            }
        }
        Err(error) => WorkspaceValidation {
            valid: false,
            workspace_path,
            plan_path,
            title: None,
            phase_count: 0,
            task_count: 0,
            error: Some(error),
        },
    }
}

pub fn browse_host_directory(path: String) -> Result<Vec<HostFileEntry>, String> {
    let base = normalize_path(Path::new(&path))?;
    let mut entries = Vec::new();
    for entry in fs::read_dir(&base).map_err(|e| format!("Failed to read directory: {}", e))? {
        let entry = entry.map_err(|e| e.to_string())?;
        let meta = entry.metadata().map_err(|e| e.to_string())?;
        entries.push(HostFileEntry {
            path: entry.path().to_string_lossy().to_string(),
            name: entry.file_name().to_string_lossy().to_string(),
            kind: if meta.is_dir() { "directory" } else { "file" }.to_string(),
            status: None,
            size: if meta.is_file() { Some(meta.len()) } else { None },
        });
    }
    entries.sort_by(|a, b| a.kind.cmp(&b.kind).then(a.name.cmp(&b.name)));
    Ok(entries)
}

pub fn list_workspace_files(state: &AppState, id: String, mode: String) -> Result<Vec<HostFileEntry>, String> {
    let run = get_plan(state, &id)?;
    if mode == "changed" {
        return git_changed_files(&run.plan.workspace_path);
    }
    all_workspace_files(&run.plan.workspace_path)
}

async fn spawn_coordinator_loop(state: AppState, plan_id: String) {
    tokio::spawn(async move {
        if let Err(error) = coordinator_loop(state.clone(), plan_id.clone()).await {
            let _ = state.db.with_conn(|conn| {
                conn.execute(
                    "UPDATE agent_plans SET status = 'needs_attention', error = ?1, updated_at = datetime('now') WHERE id = ?2 AND status NOT IN ('approved', 'blocked', 'stopped')",
                    params![error, plan_id],
                )
                .map_err(|e| e.to_string())
            });
        }
    });
}

async fn coordinator_loop(state: AppState, plan_id: String) -> Result<(), String> {
    loop {
        let run = get_plan(&state, &plan_id)?;
        match run.plan.status.as_str() {
            "phase_worker_running" => {
                let phase = current_phase(&run)?;
                let session_id = run
                    .plan
                    .worker_session_id
                    .clone()
                    .ok_or_else(|| "Plan has no worker session".to_string())?;
                wait_for_idle(&state, &session_id).await?;
                state.db.with_conn(|conn| {
                    conn.execute(
                        "UPDATE agent_plans SET status = 'phase_review_running', updated_at = datetime('now') WHERE id = ?1 AND status = 'phase_worker_running'",
                        params![plan_id],
                    )
                    .map_err(|e| e.to_string())?;
                    conn.execute(
                        "UPDATE agent_plan_phases SET status = 'worker_idle', worker_idle_at = datetime('now'), updated_at = datetime('now') WHERE plan_id = ?1 AND phase_id = ?2",
                        params![plan_id, phase.phase_id],
                    )
                    .map_err(|e| e.to_string())
                })?;
                append_event(&state, &plan_id, Some(&phase.phase_id), "agent_phase_worker_idle", json!({}))?;
                let refreshed = get_plan(&state, &plan_id)?;
                dispatch_review(&state, &refreshed, &phase).await?;
            }
            "phase_review_running" => {
                let phase = current_phase(&run)?;
                let session_id = run
                    .plan
                    .reviewer_session_id
                    .clone()
                    .ok_or_else(|| "Plan has no reviewer session".to_string())?;
                let snapshot = wait_for_idle(&state, &session_id).await?;
                handle_reviewer_output(&state, &run, &phase, &snapshot.content).await?;
            }
            "approved" | "blocked" | "stopped" | "needs_attention" => break,
            _ => break,
        }
        sleep(Duration::from_millis(300)).await;
    }
    Ok(())
}

async fn wait_for_idle(
    state: &AppState,
    session_id: &str,
) -> Result<terminal::TerminalSnapshot, String> {
    let mut last_key = String::new();
    let mut last_changed_at = Instant::now();
    let mut last_snapshot = terminal::capture_terminal_session(state, session_id).await?;
    loop {
        let snapshot = terminal::capture_terminal_session(state, session_id).await?;
        let key = format!(
            "{}\n{}:{}:{}",
            snapshot.content, snapshot.cursor_x, snapshot.cursor_y, snapshot.history_lines
        );
        if key != last_key {
            last_key = key;
            last_changed_at = Instant::now();
            last_snapshot = snapshot;
        }
        if last_changed_at.elapsed() >= Duration::from_millis(IDLE_WINDOW_MS) {
            return Ok(last_snapshot);
        }
        sleep(Duration::from_millis(350)).await;
    }
}

async fn dispatch_review(
    state: &AppState,
    run: &AgentPlanRun,
    phase: &AgentPlanPhase,
) -> Result<(), String> {
    let reviewer_session_id = run
        .plan
        .reviewer_session_id
        .clone()
        .ok_or_else(|| "Plan has no reviewer session".to_string())?;
    let prompt = reviewer_phase_prompt(run, phase)?;
    terminal::send_terminal_input(state, reviewer_session_id, format!("{}\r", prompt)).await?;
    state.db.with_conn(|conn| {
        conn.execute(
            "UPDATE agent_plans SET status = 'phase_review_running', updated_at = datetime('now') WHERE id = ?1",
            params![run.plan.id],
        )
        .map_err(|e| e.to_string())?;
        conn.execute(
            "UPDATE agent_plan_phases SET status = 'reviewer_running', reviewer_started_at = COALESCE(reviewer_started_at, datetime('now')), updated_at = datetime('now') WHERE plan_id = ?1 AND phase_id = ?2",
            params![run.plan.id, phase.phase_id],
        )
        .map_err(|e| e.to_string())
    })?;
    append_event(state, &run.plan.id, Some(&phase.phase_id), "agent_phase_review_started", json!({}))
}

async fn handle_reviewer_output(
    state: &AppState,
    run: &AgentPlanRun,
    phase: &AgentPlanPhase,
    output: &str,
) -> Result<(), String> {
    let verdict = parse_verdict(output);
    match verdict.as_deref() {
        Some("PASS") => pass_phase(state, &run.plan.id, &phase.phase_id, "T2 passed phase").await,
        Some("NEEDS_CHANGES") => {
            state.db.with_conn(|conn| {
                conn.execute(
                    "UPDATE agent_plans SET status = 'phase_needs_changes', updated_at = datetime('now') WHERE id = ?1",
                    params![run.plan.id],
                )
                .map_err(|e| e.to_string())?;
                conn.execute(
                    "UPDATE agent_plan_phases SET status = 'needs_changes', gate_verdict = 'needs_changes', reviewer_idle_at = datetime('now'), summary = ?1, updated_at = datetime('now') WHERE plan_id = ?2 AND phase_id = ?3",
                    params![summarize_output(output), run.plan.id, phase.phase_id],
                )
                .map_err(|e| e.to_string())
            })?;
            append_event(state, &run.plan.id, Some(&phase.phase_id), "agent_phase_gate_result", json!({ "verdict": "NEEDS_CHANGES" }))
        }
        Some("BLOCKED") => block_plan(state, run.plan.id.clone(), summarize_output(output)).await.map(|_| ()),
        _ => clarify_or_needs_attention(state, run, phase).await,
    }
}

async fn clarify_or_needs_attention(
    state: &AppState,
    run: &AgentPlanRun,
    phase: &AgentPlanPhase,
) -> Result<(), String> {
    let next_attempt = phase.clarification_attempts + 1;
    if next_attempt > CLARIFICATION_LIMIT {
        state.db.with_conn(|conn| {
            conn.execute(
                "UPDATE agent_plans SET status = 'needs_attention', error = 'T2 did not return a parseable verdict after 5 clarification attempts', updated_at = datetime('now') WHERE id = ?1",
                params![run.plan.id],
            )
            .map_err(|e| e.to_string())?;
            conn.execute(
                "UPDATE agent_plan_phases SET status = 'needs_attention', gate_verdict = 'unknown', updated_at = datetime('now') WHERE plan_id = ?1 AND phase_id = ?2",
                params![run.plan.id, phase.phase_id],
            )
            .map_err(|e| e.to_string())
        })?;
        return append_event(state, &run.plan.id, Some(&phase.phase_id), "agent_phase_needs_attention", json!({ "reason": "unknown_verdict" }));
    }

    state.db.with_conn(|conn| {
        conn.execute(
            "UPDATE agent_plan_phases SET clarification_attempts = ?1, updated_at = datetime('now') WHERE plan_id = ?2 AND phase_id = ?3",
            params![next_attempt, run.plan.id, phase.phase_id],
        )
        .map_err(|e| e.to_string())
    })?;
    let reviewer_session_id = run
        .plan
        .reviewer_session_id
        .clone()
        .ok_or_else(|| "Plan has no reviewer session".to_string())?;
    let prompt = "Return only this footer, with no extra text:\nPHASE: <phase id>\nVERDICT: PASS | NEEDS_CHANGES | BLOCKED\nSUMMARY: <one sentence>\nFINDINGS:\n- <finding or none>\nNEXT_STEPS:\n- <step or none>";
    terminal::send_terminal_input(state, reviewer_session_id, format!("{}\r", prompt)).await?;
    append_event(state, &run.plan.id, Some(&phase.phase_id), "agent_phase_verdict_clarification_requested", json!({ "attempt": next_attempt }))
}

async fn pass_phase(
    state: &AppState,
    plan_id: &str,
    phase_id: &str,
    summary: &str,
) -> Result<(), String> {
    let run = get_plan(state, plan_id)?;
    let phase = find_phase(&run, phase_id)?;
    let next_phase = run
        .phases
        .iter()
        .find(|candidate| candidate.phase_index == phase.phase_index + 1)
        .cloned();
    state.db.with_conn(|conn| {
        conn.execute(
            "UPDATE agent_plan_phases SET status = 'passed', gate_verdict = 'pass', reviewer_idle_at = datetime('now'), summary = ?1, updated_at = datetime('now') WHERE plan_id = ?2 AND phase_id = ?3",
            params![summary, plan_id, phase_id],
        )
        .map_err(|e| e.to_string())?;
        if let Some(next) = &next_phase {
            conn.execute(
                "UPDATE agent_plans SET status = 'phase_worker_running', current_phase_id = ?1, current_phase_index = ?2, updated_at = datetime('now') WHERE id = ?3",
                params![next.phase_id, next.phase_index, plan_id],
            )
            .map_err(|e| e.to_string())?;
            conn.execute(
                "UPDATE agent_plan_phases SET status = 'worker_running', worker_started_at = COALESCE(worker_started_at, datetime('now')), updated_at = datetime('now') WHERE plan_id = ?1 AND phase_id = ?2",
                params![plan_id, next.phase_id],
            )
            .map_err(|e| e.to_string())?;
        } else {
            conn.execute(
                "UPDATE agent_plans SET status = 'approved', updated_at = datetime('now') WHERE id = ?1",
                params![plan_id],
            )
            .map_err(|e| e.to_string())?;
        }
        Ok(())
    })?;
    append_event(state, plan_id, Some(phase_id), "agent_phase_gate_result", json!({ "verdict": "PASS" }))?;

    if let Some(next) = next_phase {
        let refreshed = get_plan(state, plan_id)?;
        let worker_session_id = refreshed
            .plan
            .worker_session_id
            .clone()
            .ok_or_else(|| "Plan has no worker session".to_string())?;
        let prompt = worker_phase_prompt(&refreshed, &next)?;
        terminal::send_terminal_input(state, worker_session_id, format!("{}\r", prompt)).await?;
        append_event(state, plan_id, Some(&next.phase_id), "agent_phase_unlocked", json!({}))?;
    } else {
        append_event(state, plan_id, None, "agent_plan_completed", json!({}))?;
    }
    Ok(())
}

fn parse_plan(workspace_path: &str, plan_path: &str) -> Result<ParsedPlan, String> {
    let workspace = normalize_path(Path::new(workspace_path))?;
    if !workspace.is_dir() {
        return Err("Workspace path is not a directory".to_string());
    }
    let raw_plan = Path::new(plan_path);
    let plan = if raw_plan.is_absolute() {
        normalize_path(raw_plan)?
    } else {
        normalize_path(&workspace.join(raw_plan))?
    };
    if !plan.starts_with(&workspace) {
        return Err("Plan path must be inside the selected workspace".to_string());
    }
    if !plan.join("overview.md").is_file() {
        return Err("Plan overview.md was not found".to_string());
    }
    let phases_dir = plan.join("phases");
    if !phases_dir.is_dir() {
        return Err("Plan phases directory was not found".to_string());
    }
    let title = first_markdown_heading(&plan.join("overview.md"))
        .unwrap_or_else(|| plan.file_name().unwrap_or_default().to_string_lossy().to_string());
    let mut phases = Vec::new();
    for entry in fs::read_dir(&phases_dir).map_err(|e| e.to_string())? {
        let entry = entry.map_err(|e| e.to_string())?;
        if !entry.file_type().map_err(|e| e.to_string())?.is_dir() {
            continue;
        }
        let phase_id = entry.file_name().to_string_lossy().to_string();
        let overview_path = entry.path().join("overview.md");
        if !overview_path.is_file() {
            continue;
        }
        let title = first_markdown_heading(&overview_path).unwrap_or_else(|| phase_id.clone());
        let tasks = parse_tasks(&entry.path(), &phase_id)?;
        phases.push(ParsedPhase { phase_id, title, tasks });
    }
    phases.sort_by(|a, b| a.phase_id.cmp(&b.phase_id));
    if phases.is_empty() {
        return Err("Plan has no phases with overview.md files".to_string());
    }
    Ok(ParsedPlan { title, workspace_path: workspace, plan_path: plan, phases })
}

fn parse_tasks(phase_path: &Path, phase_id: &str) -> Result<Vec<ParsedTask>, String> {
    let tasks_dir = phase_path.join("tasks");
    if !tasks_dir.is_dir() {
        return Ok(Vec::new());
    }
    let mut tasks = Vec::new();
    for entry in fs::read_dir(&tasks_dir).map_err(|e| e.to_string())? {
        let entry = entry.map_err(|e| e.to_string())?;
        if !entry.file_type().map_err(|e| e.to_string())?.is_dir() {
            continue;
        }
        let task_id = entry.file_name().to_string_lossy().to_string();
        let prompt_path = entry.path().join("prompt.md");
        if !prompt_path.is_file() {
            continue;
        }
        let title = first_markdown_heading(&prompt_path)
            .unwrap_or_else(|| format!("{} {}", phase_id, task_id));
        tasks.push(ParsedTask {
            task_id,
            title,
            prompt_path,
            status_path: Some(entry.path().join("status.md")).filter(|p| p.exists()),
            decisions_path: Some(entry.path().join("decisions.md")).filter(|p| p.exists()),
        });
    }
    tasks.sort_by(|a, b| a.task_id.cmp(&b.task_id));
    Ok(tasks)
}

fn normalize_path(path: &Path) -> Result<PathBuf, String> {
    if path.exists() {
        path.canonicalize().map_err(|e| format!("Invalid path: {}", e))
    } else {
        let parent = path
            .parent()
            .ok_or_else(|| "Path has no parent".to_string())?
            .canonicalize()
            .map_err(|e| format!("Invalid parent path: {}", e))?;
        Ok(parent.join(path.file_name().unwrap_or_default()))
    }
}

fn first_markdown_heading(path: &Path) -> Option<String> {
    fs::read_to_string(path)
        .ok()?
        .lines()
        .find_map(|line| line.strip_prefix("# ").map(|heading| heading.trim().to_string()))
}

fn default_model_for_provider(provider: &str) -> Option<String> {
    if provider == "ollama" {
        Some("qwen3.5:2b".to_string())
    } else {
        None
    }
}

fn worker_phase_prompt(run: &AgentPlanRun, phase: &AgentPlanPhase) -> Result<String, String> {
    let phase_overview = fs::read_to_string(Path::new(&run.plan.plan_path).join("phases").join(&phase.phase_id).join("overview.md"))
        .unwrap_or_default();
    let tasks = run
        .tasks
        .iter()
        .filter(|task| task.phase_id == phase.phase_id)
        .map(|task| format!("- {}: {}", task.task_id, task.task_title))
        .collect::<Vec<_>>()
        .join("\n");
    Ok(format!(
        "JOHNNYONE_RUN_ID: {}\nJOHNNYONE_PHASE_ID: {}\nROLE: T1_WORKER\n\nRead common/methodology.md. Work only on this phase. Do not start later phases.\n\nWorkspace: {}\nPlan: {}\nPhase: {} - {}\n\nPhase overview:\n{}\n\nTasks:\n{}\n\nWhen done, update task status/decisions/artifacts and say READY_FOR_T2_VALIDATION.",
        run.plan.id,
        phase.phase_id,
        run.plan.workspace_path,
        run.plan.plan_path,
        phase.phase_id,
        phase.phase_title,
        phase_overview,
        if tasks.is_empty() { "- No task files found".to_string() } else { tasks }
    ))
}

fn reviewer_phase_prompt(run: &AgentPlanRun, phase: &AgentPlanPhase) -> Result<String, String> {
    let phase_overview = fs::read_to_string(Path::new(&run.plan.plan_path).join("phases").join(&phase.phase_id).join("overview.md"))
        .unwrap_or_default();
    Ok(format!(
        "JOHNNYONE_RUN_ID: {}\nJOHNNYONE_PHASE_ID: {}\nROLE: T2_REVIEWER\n\nRead common/methodology.md. Validate only; do not implement app changes.\n\nWorkspace: {}\nPlan: {}\nPhase: {} - {}\n\nPhase overview:\n{}\n\nFocus on methodology status files, decisions, tests, E2E artifacts, screenshots, and acceptance criteria. Return this footer exactly:\n\nPHASE: {}\nVERDICT: PASS | NEEDS_CHANGES | BLOCKED\nSUMMARY: <one paragraph>\nFINDINGS:\n- <finding or none>\nNEXT_STEPS:\n- <step for T1 or none>",
        run.plan.id,
        phase.phase_id,
        run.plan.workspace_path,
        run.plan.plan_path,
        phase.phase_id,
        phase.phase_title,
        phase_overview,
        phase.phase_id,
    ))
}

fn feedback_prompt(phase: &AgentPlanPhase) -> String {
    format!(
        "T2 returned NEEDS_CHANGES for phase {}. Review the findings in the terminal above, fix what is needed or provide evidence it is already done, then say READY_FOR_T2_VALIDATION again.",
        phase.phase_id
    )
}

fn parse_verdict(output: &str) -> Option<String> {
    output.lines().rev().find_map(|line| {
        let trimmed = line.trim();
        let value = trimmed.strip_prefix("VERDICT:")?.trim().to_ascii_uppercase();
        match value.as_str() {
            "PASS" | "NEEDS_CHANGES" | "BLOCKED" => Some(value),
            _ => None,
        }
    })
}

fn summarize_output(output: &str) -> String {
    output
        .lines()
        .rev()
        .find(|line| !line.trim().is_empty())
        .unwrap_or("No summary")
        .trim()
        .chars()
        .take(500)
        .collect()
}

fn current_phase(run: &AgentPlanRun) -> Result<AgentPlanPhase, String> {
    let phase_id = run
        .plan
        .current_phase_id
        .as_ref()
        .ok_or_else(|| "Plan has no current phase".to_string())?;
    find_phase(run, phase_id)
}

fn find_phase(run: &AgentPlanRun, phase_id: &str) -> Result<AgentPlanPhase, String> {
    run.phases
        .iter()
        .find(|phase| phase.phase_id == phase_id)
        .cloned()
        .ok_or_else(|| format!("Phase not found: {}", phase_id))
}

fn list_phases(state: &AppState, plan_id: &str) -> Result<Vec<AgentPlanPhase>, String> {
    state.db.with_conn(|conn| {
        let mut stmt = conn.prepare(
            "SELECT id, plan_id, phase_id, phase_title, phase_index, status, worker_started_at, worker_idle_at, reviewer_started_at, reviewer_idle_at, gate_verdict, clarification_attempts, summary, findings_json, created_at, updated_at FROM agent_plan_phases WHERE plan_id = ?1 ORDER BY phase_index",
        ).map_err(|e| e.to_string())?;
        let rows = stmt
            .query_map(params![plan_id], agent_plan_phase_from_row)
            .map_err(|e| e.to_string())?
            .filter_map(|row| row.ok())
            .collect();
        Ok(rows)
    })
}

fn list_tasks(state: &AppState, plan_id: &str) -> Result<Vec<AgentPlanTask>, String> {
    state.db.with_conn(|conn| {
        let mut stmt = conn.prepare(
            "SELECT id, plan_id, phase_id, task_id, task_title, task_index, prompt_path, status_path, decisions_path, status, created_at, updated_at FROM agent_plan_tasks WHERE plan_id = ?1 ORDER BY phase_id, task_index",
        ).map_err(|e| e.to_string())?;
        let rows = stmt
            .query_map(params![plan_id], agent_plan_task_from_row)
            .map_err(|e| e.to_string())?
            .filter_map(|row| row.ok())
            .collect();
        Ok(rows)
    })
}

fn list_events(state: &AppState, plan_id: &str, limit: i64) -> Result<Vec<AgentPlanEvent>, String> {
    state.db.with_conn(|conn| {
        let mut stmt = conn.prepare(
            "SELECT id, plan_id, phase_id, type, payload_json, created_at FROM agent_plan_events WHERE plan_id = ?1 ORDER BY created_at DESC, rowid DESC LIMIT ?2",
        ).map_err(|e| e.to_string())?;
        let mut rows: Vec<AgentPlanEvent> = stmt
            .query_map(params![plan_id, limit], agent_plan_event_from_row)
            .map_err(|e| e.to_string())?
            .filter_map(|row| row.ok())
            .collect();
        rows.reverse();
        Ok(rows)
    })
}

fn append_event(
    state: &AppState,
    plan_id: &str,
    phase_id: Option<&str>,
    event_type: &str,
    payload: serde_json::Value,
) -> Result<(), String> {
    state.db.with_conn(|conn| {
        conn.execute(
            "INSERT INTO agent_plan_events (id, plan_id, phase_id, type, payload_json) VALUES (?1, ?2, ?3, ?4, ?5)",
            params![Uuid::new_v4().to_string(), plan_id, phase_id, event_type, payload.to_string()],
        )
        .map_err(|e| e.to_string())?;
        conn.execute(
            "UPDATE agent_plans SET updated_at = datetime('now') WHERE id = ?1",
            params![plan_id],
        )
        .map_err(|e| e.to_string())?;
        Ok(())
    })?;
    publish_plan_update(state, plan_id);
    Ok(())
}

fn publish_plan_update(state: &AppState, plan_id: &str) {
    let run = get_plan(state, plan_id)
        .ok()
        .and_then(|run| serde_json::to_value(run).ok());
    if run.is_none() {
        return;
    }
    let _ = state.agent_plan_run_tx.send(AgentPlanRunEvent {
        plan_id: plan_id.to_string(),
        deleted: false,
        run,
    }).map(|count| {
        tracing::debug!(plan_id = %plan_id, subscribers = count, "published agent plan run update");
    });
}

fn publish_plan_deleted(state: &AppState, plan_id: &str) {
    let _ = state.agent_plan_run_tx.send(AgentPlanRunEvent {
        plan_id: plan_id.to_string(),
        deleted: true,
        run: None,
    }).map(|count| {
        tracing::debug!(plan_id = %plan_id, subscribers = count, "published agent plan run delete");
    });
}

fn all_workspace_files(workspace_path: &str) -> Result<Vec<HostFileEntry>, String> {
    let root = normalize_path(Path::new(workspace_path))?;
    let mut entries = Vec::new();
    let mut stack = vec![root.clone()];
    let ignored: HashSet<&str> = [
        ".git",
        "node_modules",
        "target",
        "dist",
        ".angular",
        ".nx",
        ".wrangler",
    ]
    .into_iter()
    .collect();
    while let Some(dir) = stack.pop() {
        for entry in fs::read_dir(&dir).map_err(|e| e.to_string())? {
            let entry = entry.map_err(|e| e.to_string())?;
            let path = entry.path();
            let name = entry.file_name().to_string_lossy().to_string();
            if ignored.contains(name.as_str()) {
                continue;
            }
            let meta = entry.metadata().map_err(|e| e.to_string())?;
            let rel = path.strip_prefix(&root).unwrap_or(&path).to_string_lossy().to_string();
            entries.push(HostFileEntry {
                path: rel,
                name,
                kind: if meta.is_dir() { "directory" } else { "file" }.to_string(),
                status: None,
                size: if meta.is_file() { Some(meta.len()) } else { None },
            });
            if meta.is_dir() && entries.len() < 1000 {
                stack.push(path);
            }
        }
        if entries.len() >= 1000 {
            break;
        }
    }
    entries.sort_by(|a, b| a.path.cmp(&b.path));
    Ok(entries)
}

fn git_changed_files(workspace_path: &str) -> Result<Vec<HostFileEntry>, String> {
    let root_output = std::process::Command::new("git")
        .args(["rev-parse", "--show-toplevel"])
        .current_dir(workspace_path)
        .output()
        .map_err(|e| format!("Failed to inspect git workspace: {}", e))?;
    if !root_output.status.success() {
        return Ok(vec![HostFileEntry {
            path: workspace_path.to_string(),
            name: "Workspace is not a git repository".to_string(),
            kind: "info".to_string(),
            status: Some("non-git".to_string()),
            size: None,
        }]);
    }

    let git_root = String::from_utf8_lossy(&root_output.stdout).trim().to_string();
    let output = std::process::Command::new("git")
        .args(["status", "--porcelain"])
        .current_dir(&git_root)
        .output()
        .map_err(|e| format!("Failed to run git status: {}", e))?;
    if !output.status.success() {
        return Err(String::from_utf8_lossy(&output.stderr).trim().to_string());
    }
    let mut entries = Vec::new();
    for line in String::from_utf8_lossy(&output.stdout).lines() {
        if line.len() < 4 {
            continue;
        }
        let status = line[..2].trim().to_string();
        let path = line[3..].trim().to_string();
        let full_path = Path::new(&git_root).join(&path);
        let meta = fs::metadata(&full_path).ok();
        entries.push(HostFileEntry {
            name: Path::new(&path)
                .file_name()
                .unwrap_or_default()
                .to_string_lossy()
                .to_string(),
            path,
            kind: if meta.as_ref().map(|m| m.is_dir()).unwrap_or(false) {
                "directory"
            } else {
                "file"
            }
            .to_string(),
            status: Some(status),
            size: meta.filter(|m| m.is_file()).map(|m| m.len()),
        });
    }
    Ok(entries)
}

fn agent_plan_from_row(row: &rusqlite::Row<'_>) -> rusqlite::Result<AgentPlan> {
    Ok(AgentPlan {
        id: row.get(0)?,
        title: row.get(1)?,
        workspace_path: row.get(2)?,
        plan_path: row.get(3)?,
        status: row.get(4)?,
        worker_session_id: row.get(5)?,
        reviewer_session_id: row.get(6)?,
        worker_provider: row.get(7)?,
        reviewer_provider: row.get(8)?,
        current_phase_id: row.get(9)?,
        current_phase_index: row.get(10)?,
        error: row.get(11)?,
        created_at: row.get(12)?,
        updated_at: row.get(13)?,
    })
}

fn agent_plan_phase_from_row(row: &rusqlite::Row<'_>) -> rusqlite::Result<AgentPlanPhase> {
    Ok(AgentPlanPhase {
        id: row.get(0)?,
        plan_id: row.get(1)?,
        phase_id: row.get(2)?,
        phase_title: row.get(3)?,
        phase_index: row.get(4)?,
        status: row.get(5)?,
        worker_started_at: row.get(6)?,
        worker_idle_at: row.get(7)?,
        reviewer_started_at: row.get(8)?,
        reviewer_idle_at: row.get(9)?,
        gate_verdict: row.get(10)?,
        clarification_attempts: row.get(11)?,
        summary: row.get(12)?,
        findings_json: row.get(13)?,
        created_at: row.get(14)?,
        updated_at: row.get(15)?,
    })
}

fn agent_plan_task_from_row(row: &rusqlite::Row<'_>) -> rusqlite::Result<AgentPlanTask> {
    Ok(AgentPlanTask {
        id: row.get(0)?,
        plan_id: row.get(1)?,
        phase_id: row.get(2)?,
        task_id: row.get(3)?,
        task_title: row.get(4)?,
        task_index: row.get(5)?,
        prompt_path: row.get(6)?,
        status_path: row.get(7)?,
        decisions_path: row.get(8)?,
        status: row.get(9)?,
        created_at: row.get(10)?,
        updated_at: row.get(11)?,
    })
}

fn agent_plan_event_from_row(row: &rusqlite::Row<'_>) -> rusqlite::Result<AgentPlanEvent> {
    Ok(AgentPlanEvent {
        id: row.get(0)?,
        plan_id: row.get(1)?,
        phase_id: row.get(2)?,
        event_type: row.get(3)?,
        payload_json: row.get(4)?,
        created_at: row.get(5)?,
    })
}
