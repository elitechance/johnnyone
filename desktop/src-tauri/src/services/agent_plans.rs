use crate::db::models::{
    AgentPlan, AgentPlanEvent, AgentPlanPhase, AgentPlanTask, CreateAgentPlanInput,
    CreateSessionInput,
};
use crate::events::AgentPlanRunEvent;
use crate::services::planner_prompts;
use crate::services::sessions;
use crate::state::app_state::AppState;
use crate::terminal;
use base64::{engine::general_purpose, Engine as _};
use rusqlite::{params, OptionalExtension};
use serde::{Deserialize, Serialize};
use serde_json::json;
use std::collections::{HashMap, HashSet};
use std::fs;
use std::path::{Path, PathBuf};
use tokio::time::{sleep, Duration, Instant};
use uuid::Uuid;

const IDLE_WINDOW_MS: u64 = 2_000;
const TERMINAL_STARTUP_WAIT_MS: u64 = 2_500;
const WORKER_READY_MARKER: &str = "READY_FOR_T2_VALIDATION";
const PLANNER_READY_MARKER: &str = "READY_FOR_T2_PLAN_REVIEW";
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
pub struct HostFileContent {
    pub path: String,
    pub name: String,
    pub kind: String,
    pub content_type: String,
    pub encoding: String,
    pub content: String,
    pub size: u64,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct WorkspaceFileDiff {
    pub path: String,
    pub diff: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct GitFilesView {
    pub path: String,
    pub repo_root: Option<String>,
    pub branch: Option<String>,
    pub clean: bool,
    pub entries: Vec<HostFileEntry>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct GitActionResult {
    pub success: bool,
    pub output: String,
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
struct ReviewInsights {
    summary: Option<String>,
    findings: Vec<String>,
    next_steps: Vec<String>,
    reason: Option<String>,
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
    status: String,
    decisions_path: Option<PathBuf>,
}

#[derive(Debug, Deserialize)]
struct TaskStatusYaml {
    state: Option<String>,
    status: Option<String>,
}

pub fn create_plan(state: &AppState, input: CreateAgentPlanInput) -> Result<AgentPlanRun, String> {
    let run_type = input
        .run_type
        .clone()
        .unwrap_or_else(|| "development".to_string());
    if run_type == "planning" {
        return create_planning_run(state, input);
    }

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
            kind: Some("agent".to_string()),
        },
    )?;
    let reviewer_session = sessions::create_session(
        state,
        CreateSessionInput {
            provider: Some(input.reviewer_provider.clone()),
            model: default_model_for_provider(&input.reviewer_provider),
            working_directory: Some(parsed.workspace_path.to_string_lossy().to_string()),
            title: Some(format!("T2 Reviewer - {}", title)),
            kind: Some("agent".to_string()),
        },
    )?;

    state.db.with_conn(|conn| {
        conn.execute(
            "INSERT INTO agent_plans (id, run_type, title, workspace_path, plan_path, status, worker_session_id, reviewer_session_id, worker_provider, reviewer_provider, current_phase_id, current_phase_index)
             VALUES (?1, 'development', ?2, ?3, ?4, 'draft', ?5, ?6, ?7, ?8, ?9, 0)",
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
                    "INSERT INTO agent_plan_tasks (id, plan_id, phase_id, task_id, task_title, task_index, prompt_path, status_path, decisions_path, status)
                     VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10)",
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
                        task.status,
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

fn create_planning_run(
    state: &AppState,
    input: CreateAgentPlanInput,
) -> Result<AgentPlanRun, String> {
    let workspace = normalize_path(Path::new(&input.workspace_path))?;
    if !workspace.is_dir() {
        return Err("Workspace path is not a directory".to_string());
    }
    let raw_plan = Path::new(&input.plan_path);
    let plan = if raw_plan.is_absolute() {
        normalize_path(raw_plan)?
    } else {
        normalize_path(&workspace.join(raw_plan))?
    };
    if !plan.starts_with(&workspace) {
        return Err("Plan path must be inside the selected workspace".to_string());
    }
    let app_scope = normalize_optional_workspace_path(&workspace, input.app_scope.as_deref())?;
    let docs_scope = normalize_optional_workspace_path(&workspace, input.docs_scope.as_deref())?;
    let reference_paths = normalize_reference_paths(&workspace, input.reference_paths.as_deref())?;
    let plan_id = Uuid::new_v4().to_string();
    let title = input.title.clone().unwrap_or_else(|| {
        plan.file_name()
            .unwrap_or_default()
            .to_string_lossy()
            .replace('-', " ")
    });

    let worker_session = sessions::create_session(
        state,
        CreateSessionInput {
            provider: Some(input.worker_provider.clone()),
            model: default_model_for_provider(&input.worker_provider),
            working_directory: Some(workspace.to_string_lossy().to_string()),
            title: Some(format!("T1 Planner - {}", title)),
            kind: Some("agent".to_string()),
        },
    )?;
    let reviewer_session = sessions::create_session(
        state,
        CreateSessionInput {
            provider: Some(input.reviewer_provider.clone()),
            model: default_model_for_provider(&input.reviewer_provider),
            working_directory: Some(workspace.to_string_lossy().to_string()),
            title: Some(format!("T2 Plan Reviewer - {}", title)),
            kind: Some("agent".to_string()),
        },
    )?;

    state.db.with_conn(|conn| {
        conn.execute(
            "INSERT INTO agent_plans (id, run_type, title, workspace_path, plan_path, status, worker_session_id, reviewer_session_id, worker_provider, reviewer_provider, current_phase_index, brief, app_scope, docs_scope, reference_paths)
             VALUES (?1, 'planning', ?2, ?3, ?4, 'draft', ?5, ?6, ?7, ?8, 0, ?9, ?10, ?11, ?12)",
            params![
                plan_id,
                title,
                workspace.to_string_lossy(),
                plan.to_string_lossy(),
                worker_session.id,
                reviewer_session.id,
                input.worker_provider,
                input.reviewer_provider,
                input.brief.unwrap_or_default(),
                app_scope,
                docs_scope,
                reference_paths,
            ],
        )
        .map_err(|e| format!("Failed to create planning run: {}", e))
    })?;

    append_event(
        state,
        &plan_id,
        None,
        "planning_run_created",
        json!({ "plannerSessionId": worker_session.id, "reviewerSessionId": reviewer_session.id }),
    )?;

    get_plan(state, &plan_id)
}

pub fn list_plans(
    state: &AppState,
    status: Option<String>,
    run_type: Option<String>,
    only_existing: bool,
) -> Result<Vec<AgentPlan>, String> {
    state.db.with_conn(|conn| {
        let sql = "SELECT id, run_type, title, workspace_path, plan_path, status, worker_session_id, reviewer_session_id, worker_provider, reviewer_provider, current_phase_id, current_phase_index, error, brief, app_scope, docs_scope, reference_paths, amend_brief, phase_run_mode, created_at, updated_at FROM agent_plans
            WHERE (?1 IS NULL OR status = ?1) AND (?2 IS NULL OR run_type = ?2)
            ORDER BY updated_at DESC";
        let mut stmt = conn.prepare(sql).map_err(|e| e.to_string())?;
        let mut rows: Vec<AgentPlan> = stmt
            .query_map(params![status.as_deref(), run_type.as_deref()], agent_plan_from_row)
            .map_err(|e| e.to_string())?
            .filter_map(|row| row.ok())
            .collect();
        if only_existing {
            rows.retain(|plan| Path::new(&plan.plan_path).exists());
        } else if status.is_none() {
            rows.retain(|plan| plan.status != "closed");
        }
        Ok(rows)
    })
}

pub fn get_plan(state: &AppState, id: &str) -> Result<AgentPlanRun, String> {
    sync_task_statuses_from_files(state, id)?;
    let plan = state.db.with_conn(|conn| {
        conn.query_row(
            "SELECT id, run_type, title, workspace_path, plan_path, status, worker_session_id, reviewer_session_id, worker_provider, reviewer_provider, current_phase_id, current_phase_index, error, brief, app_scope, docs_scope, reference_paths, amend_brief, phase_run_mode, created_at, updated_at FROM agent_plans WHERE id = ?1",
            params![id],
            agent_plan_from_row,
        )
        .map_err(|e| format!("Agent plan not found: {}", e))
    })?;
    let phases = list_phases(state, id)?;
    let tasks = list_tasks(state, id)?;
    let events = list_events(state, id, 80)?;
    Ok(AgentPlanRun {
        plan,
        phases,
        tasks,
        events,
    })
}

pub async fn start_plan(
    state: AppState,
    id: String,
    phase_id: Option<String>,
    phase_run_mode: Option<String>,
) -> Result<AgentPlanRun, String> {
    let run = get_plan(&state, &id)?;
    // Initialize the per-plan git repo on first start. Idempotent — re-runs
    // are no-ops. Failures (git not installed, permissions) are logged but
    // don't block the plan from running.
    if let Err(err) = super::git_history::ensure_repo(&run.plan.plan_path) {
        tracing::warn!(plan_id = %id, %err, "git_history::ensure_repo failed; plan history disabled");
    }
    if run.plan.run_type == "planning" {
        return start_planning_run(state, id).await;
    }
    if matches!(run.plan.status.as_str(), "blocked" | "stopped") {
        return Ok(run);
    }
    if run.plan.status == "approved" && phase_id.is_none() {
        return Ok(run);
    }
    if matches!(
        run.plan.status.as_str(),
        "phase_worker_running" | "phase_review_running"
    ) && phase_id.is_none()
    {
        spawn_coordinator_loop(state.clone(), id.clone()).await;
        return get_plan(&state, &id);
    }
    let phase = if let Some(phase_id) = phase_id.as_deref() {
        find_phase(&run, phase_id)?
    } else {
        current_phase(&run)?
    };
    let phase_run_mode = match phase_run_mode.as_deref() {
        Some("single") => "single",
        _ => "continue",
    };
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
    sleep(Duration::from_millis(TERMINAL_STARTUP_WAIT_MS)).await;

    state.db.with_conn(|conn| {
        conn.execute(
            "UPDATE agent_plans SET status = 'phase_worker_running', current_phase_id = ?1, current_phase_index = ?2, phase_run_mode = ?3, updated_at = datetime('now') WHERE id = ?4",
            params![phase.phase_id, phase.phase_index, phase_run_mode, id],
        )
        .map_err(|e| e.to_string())?;
        conn.execute(
            "UPDATE agent_plan_phases SET status = 'worker_running', worker_started_at = COALESCE(worker_started_at, datetime('now')), updated_at = datetime('now') WHERE plan_id = ?1 AND phase_id = ?2",
            params![id, phase.phase_id],
        )
        .map_err(|e| e.to_string())
    })?;
    append_event(
        &state,
        &id,
        Some(&phase.phase_id),
        "agent_phase_started",
        json!({}),
    )?;

    let prompt = worker_phase_prompt(&run, &phase)?;
    terminal::send_terminal_input(&state, worker_session_id, format!("{}\r", prompt)).await?;

    spawn_coordinator_loop(state.clone(), id.clone()).await;
    get_plan(&state, &id)
}

pub async fn resume_active_plan_loops(state: AppState) -> Result<usize, String> {
    let plan_ids = state.db.with_conn(|conn| {
        let mut stmt = conn
            .prepare(
                "SELECT id FROM agent_plans WHERE status IN ('phase_worker_running', 'phase_review_running', 'planning_planner_running', 'planning_review_running') ORDER BY updated_at ASC",
            )
            .map_err(|e| e.to_string())?;
        let rows = stmt
            .query_map([], |row| row.get::<_, String>(0))
            .map_err(|e| e.to_string())?;
        let mut ids = Vec::new();
        for row in rows {
            ids.push(row.map_err(|e| e.to_string())?);
        }
        Ok(ids)
    })?;

    let count = plan_ids.len();
    for plan_id in plan_ids {
        if needs_amend_planner_prompt_on_resume(&state, &plan_id)? {
            start_planning_run_inner(state.clone(), plan_id, true).await?;
        } else {
            spawn_coordinator_loop(state.clone(), plan_id).await;
        }
    }
    Ok(count)
}

fn needs_amend_planner_prompt_on_resume(state: &AppState, plan_id: &str) -> Result<bool, String> {
    state.db.with_conn(|conn| {
        conn.query_row(
            "SELECT COALESCE(amend_brief, ''), status FROM agent_plans WHERE id = ?1",
            params![plan_id],
            |row| Ok((row.get::<_, String>(0)?, row.get::<_, String>(1)?)),
        )
        .map_err(|e| e.to_string())
        .and_then(|(amend_brief, status)| {
            if amend_brief.trim().is_empty() || status != "planning_planner_running" {
                return Ok(false);
            }
            let latest_event = conn
                .query_row(
                    "SELECT type FROM agent_plan_events WHERE plan_id = ?1 ORDER BY created_at DESC, rowid DESC LIMIT 1",
                    params![plan_id],
                    |row| row.get::<_, String>(0),
                )
                .optional()
                .map_err(|e| e.to_string())?;
            Ok(latest_event.as_deref() == Some("planning_amend_requested"))
        })
    })
}

/// Amend an existing approved plan: set `amend_brief` on the row, kick the
/// status back to `planning_planner_running`, and trigger a fresh planning
/// run. T1's prompt picks up the amend brief and switches to "edit mode"
/// (read the existing plan + apply changes in place instead of creating from
/// scratch). T2 reviews the diff against HEAD. On T2 PASS, `amend_brief`
/// is cleared and a new commit lands on `main` with the amendment as its
/// message.
pub async fn amend_plan(
    state: AppState,
    id: String,
    brief: String,
) -> Result<AgentPlanRun, String> {
    let brief = brief.trim().to_string();
    if brief.is_empty() {
        return Err("Amendment brief is required".to_string());
    }
    let run = get_plan(&state, &id)?;
    if run.plan.run_type != "planning" {
        return Err("Amend is only valid for planning-mode runs".to_string());
    }
    // Stash the brief + flip status so the next planning run picks up where
    // we left off.
    state.db.with_conn(|conn| {
        conn.execute(
            "UPDATE agent_plans SET amend_brief = ?1, status = 'planning_planner_running', error = NULL, updated_at = datetime('now') WHERE id = ?2",
            params![brief, id],
        )
        .map_err(|e| e.to_string())
    })?;
    append_event(
        &state,
        &id,
        None,
        "planning_amend_requested",
        json!({ "brief": brief }),
    )?;
    // Reuse the existing planning pipeline — start_planning_run reads the
    // current state (including the freshly-set amend_brief) and walks T1→T2
    // the same way an initial run does.
    start_planning_run_inner(state, id, true).await
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

pub fn update_plan_title(
    state: &AppState,
    id: String,
    title: String,
) -> Result<AgentPlanRun, String> {
    let next_title = title.trim().to_string();
    if next_title.is_empty() {
        return Err("Plan title is required".to_string());
    }
    let current = get_plan(state, &id)?;
    if current.plan.title == next_title {
        return Ok(current);
    }

    state.db.with_conn(|conn| {
        conn.execute(
            "UPDATE agent_plans SET title = ?1, updated_at = datetime('now') WHERE id = ?2",
            params![next_title, id],
        )
        .map_err(|e| e.to_string())
    })?;
    append_event(
        state,
        &id,
        None,
        "agent_plan_renamed",
        json!({ "oldTitle": current.plan.title, "newTitle": title.trim() }),
    )?;
    get_plan(state, &id)
}

pub async fn delete_plan(state: &AppState, id: String) -> Result<bool, String> {
    let run = get_plan(state, &id)?;

    if let Some(session_id) = run.plan.worker_session_id {
        let _ = terminal::kill_terminal_session(state, &session_id).await;
        let _ = sessions::archive_session(state, session_id).await;
    }
    if let Some(session_id) = run.plan.reviewer_session_id {
        let _ = terminal::kill_terminal_session(state, &session_id).await;
        let _ = sessions::archive_session(state, session_id).await;
    }

    state.db.with_conn(|conn| {
        conn.execute(
            "UPDATE agent_plans SET status = 'closed', updated_at = datetime('now') WHERE id = ?1",
            params![id],
        )
        .map_err(|e| format!("Failed to close plan: {}", e))?;
        Ok(())
    })?;

    append_event(state, &id, None, "agent_plan_closed", json!({}))?;
    publish_plan_deleted(state, &id);
    Ok(true)
}

pub async fn block_plan(
    state: &AppState,
    id: String,
    reason: String,
) -> Result<AgentPlanRun, String> {
    state.db.with_conn(|conn| {
        conn.execute(
            "UPDATE agent_plans SET status = 'blocked', error = ?1, updated_at = datetime('now') WHERE id = ?2",
            params![reason, id],
        )
        .map_err(|e| e.to_string())
    })?;
    append_event(
        state,
        &id,
        None,
        "agent_plan_blocked",
        json!({ "reason": reason }),
    )?;
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
    pass_phase(
        &state,
        &id,
        &phase_id,
        "Manual pass after failed T2 verdict clarification",
    )
    .await?;
    get_plan(&state, &id)
}

pub async fn send_feedback_to_worker(state: AppState, id: String) -> Result<AgentPlanRun, String> {
    let run = get_plan(&state, &id)?;
    if run.plan.run_type == "planning" {
        let planner_session_id = run
            .plan
            .worker_session_id
            .clone()
            .ok_or_else(|| "Planning run has no planner session".to_string())?;
        let prompt = format!(
            "Continue updating the plan at {}. When ready for T2 review, say exactly {}.",
            run.plan.plan_path, PLANNER_READY_MARKER
        );
        terminal::send_terminal_input(&state, planner_session_id, format!("{}\r", prompt)).await?;
        state.db.with_conn(|conn| {
            conn.execute(
                "UPDATE agent_plans SET status = 'planning_planner_running', updated_at = datetime('now') WHERE id = ?1",
                params![id],
            )
            .map_err(|e| e.to_string())
        })?;
        append_event(
            &state,
            &id,
            None,
            "planning_feedback_sent_to_planner",
            json!({ "source": "manual" }),
        )?;
        spawn_coordinator_loop(state.clone(), id.clone()).await;
        return get_plan(&state, &id);
    }
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
    append_event(
        &state,
        &id,
        Some(&phase.phase_id),
        "agent_feedback_sent_to_worker",
        json!({}),
    )?;
    spawn_coordinator_loop(state.clone(), id.clone()).await;
    get_plan(&state, &id)
}

pub async fn rerun_reviewer(state: AppState, id: String) -> Result<AgentPlanRun, String> {
    let run = get_plan(&state, &id)?;
    if run.plan.run_type == "planning" {
        dispatch_planning_review(&state, &run).await?;
        spawn_coordinator_loop(state.clone(), id.clone()).await;
        return get_plan(&state, &id);
    }
    let phase = current_phase(&run)?;
    dispatch_review(&state, &run, &phase).await?;
    spawn_coordinator_loop(state.clone(), id.clone()).await;
    get_plan(&state, &id)
}

async fn start_planning_run(state: AppState, id: String) -> Result<AgentPlanRun, String> {
    start_planning_run_inner(state, id, false).await
}

async fn start_planning_run_inner(
    state: AppState,
    id: String,
    force_planner_prompt: bool,
) -> Result<AgentPlanRun, String> {
    let run = get_plan(&state, &id)?;
    if matches!(run.plan.status.as_str(), "approved" | "blocked" | "stopped") {
        return Ok(run);
    }
    if matches!(
        run.plan.status.as_str(),
        "planning_planner_running" | "planning_review_running"
    ) && !force_planner_prompt {
        spawn_coordinator_loop(state.clone(), id.clone()).await;
        return get_plan(&state, &id);
    }
    let planner_session_id = run
        .plan
        .worker_session_id
        .clone()
        .ok_or_else(|| "Planning run has no planner session".to_string())?;
    let reviewer_session_id = run
        .plan
        .reviewer_session_id
        .clone()
        .ok_or_else(|| "Planning run has no reviewer session".to_string())?;

    terminal::attach_terminal_headless(&state, planner_session_id.clone(), 120, 36).await?;
    terminal::attach_terminal_headless(&state, reviewer_session_id, 120, 36).await?;
    sleep(Duration::from_millis(TERMINAL_STARTUP_WAIT_MS)).await;

    state.db.with_conn(|conn| {
        conn.execute(
            "UPDATE agent_plans SET status = 'planning_planner_running', updated_at = datetime('now') WHERE id = ?1",
            params![id],
        )
        .map_err(|e| e.to_string())
    })?;
    append_event(&state, &id, None, "planning_started", json!({}))?;
    let prompt = planning_planner_prompt(&run)?;
    terminal::send_terminal_input(&state, planner_session_id, format!("{}\r", prompt)).await?;
    spawn_coordinator_loop(state.clone(), id.clone()).await;
    get_plan(&state, &id)
}

pub fn refresh_plan_phases(state: &AppState, id: String) -> Result<AgentPlanRun, String> {
    let run = get_plan(state, &id)?;
    let parsed = parse_plan(&run.plan.workspace_path, &run.plan.plan_path)?;
    let mut inserted_phases = 0;
    let mut updated_phases = 0;
    let mut inserted_tasks = 0;
    let mut updated_tasks = 0;

    state.db.with_conn(|conn| {
        for (phase_index, phase) in parsed.phases.iter().enumerate() {
            let phase_exists: bool = conn
                .query_row(
                    "SELECT 1 FROM agent_plan_phases WHERE plan_id = ?1 AND phase_id = ?2",
                    params![id, phase.phase_id],
                    |_| Ok(true),
                )
                .optional()
                .map_err(|e| e.to_string())?
                .unwrap_or(false);
            if phase_exists {
                conn.execute(
                    "UPDATE agent_plan_phases SET phase_title = ?1, phase_index = ?2, updated_at = datetime('now') WHERE plan_id = ?3 AND phase_id = ?4",
                    params![phase.title, phase_index as i64, id, phase.phase_id],
                )
                .map_err(|e| format!("Failed to update phase {}: {}", phase.phase_id, e))?;
                updated_phases += 1;
            } else {
                conn.execute(
                    "INSERT INTO agent_plan_phases (id, plan_id, phase_id, phase_title, phase_index, status)
                     VALUES (?1, ?2, ?3, ?4, ?5, 'locked')",
                    params![Uuid::new_v4().to_string(), id, phase.phase_id, phase.title, phase_index as i64],
                )
                .map_err(|e| format!("Failed to insert phase {}: {}", phase.phase_id, e))?;
                inserted_phases += 1;
            }

            for (task_index, task) in phase.tasks.iter().enumerate() {
                let task_exists: bool = conn
                    .query_row(
                        "SELECT 1 FROM agent_plan_tasks WHERE plan_id = ?1 AND phase_id = ?2 AND task_id = ?3",
                        params![id, phase.phase_id, task.task_id],
                        |_| Ok(true),
                    )
                    .optional()
                    .map_err(|e| e.to_string())?
                    .unwrap_or(false);
                if task_exists {
                    conn.execute(
                        "UPDATE agent_plan_tasks
                         SET task_title = ?1, task_index = ?2, prompt_path = ?3, status_path = ?4, decisions_path = ?5, updated_at = datetime('now')
                         WHERE plan_id = ?6 AND phase_id = ?7 AND task_id = ?8",
                        params![
                            task.title,
                            task_index as i64,
                            task.prompt_path.to_string_lossy(),
                            task.status_path.as_ref().map(|p| p.to_string_lossy().to_string()),
                            task.decisions_path.as_ref().map(|p| p.to_string_lossy().to_string()),
                            id,
                            phase.phase_id,
                            task.task_id,
                        ],
                    )
                    .map_err(|e| format!("Failed to update task {}/{}: {}", phase.phase_id, task.task_id, e))?;
                    updated_tasks += 1;
                } else {
                    conn.execute(
                        "INSERT INTO agent_plan_tasks (id, plan_id, phase_id, task_id, task_title, task_index, prompt_path, status_path, decisions_path, status)
                         VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10)",
                        params![
                            Uuid::new_v4().to_string(),
                            id,
                            phase.phase_id,
                            task.task_id,
                            task.title,
                            task_index as i64,
                            task.prompt_path.to_string_lossy(),
                            task.status_path.as_ref().map(|p| p.to_string_lossy().to_string()),
                            task.decisions_path.as_ref().map(|p| p.to_string_lossy().to_string()),
                            task.status,
                        ],
                    )
                    .map_err(|e| format!("Failed to insert task {}/{}: {}", phase.phase_id, task.task_id, e))?;
                    inserted_tasks += 1;
                }
            }
        }

        if run.plan.current_phase_id.is_none() {
            conn.execute(
                "UPDATE agent_plans SET current_phase_id = ?1, current_phase_index = 0, updated_at = datetime('now') WHERE id = ?2",
                params![parsed.phases.first().map(|phase| phase.phase_id.as_str()), id],
            )
            .map_err(|e| e.to_string())?;
        }
        Ok(())
    })?;

    append_event(
        state,
        &id,
        None,
        "agent_plan_phases_refreshed",
        json!({
            "insertedPhases": inserted_phases,
            "updatedPhases": updated_phases,
            "insertedTasks": inserted_tasks,
            "updatedTasks": updated_tasks,
        }),
    )?;
    get_plan(state, &id)
}

pub fn validate_workspace_and_plan_path(
    workspace_path: String,
    plan_path: String,
) -> WorkspaceValidation {
    match parse_plan(&workspace_path, &plan_path) {
        Ok(parsed) => {
            let task_count = parsed
                .phases
                .iter()
                .map(|phase| phase.tasks.len() as i64)
                .sum();
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
            size: if meta.is_file() {
                Some(meta.len())
            } else {
                None
            },
        });
    }
    entries.sort_by(|a, b| a.kind.cmp(&b.kind).then(a.name.cmp(&b.name)));
    Ok(entries)
}

pub fn list_workspace_files(
    state: &AppState,
    id: String,
    mode: String,
    path: Option<String>,
) -> Result<Vec<HostFileEntry>, String> {
    let run = get_plan(state, &id)?;
    if mode == "changed" {
        return git_changed_files(&run.plan.workspace_path, path.as_deref());
    }
    all_workspace_files(&run.plan.workspace_path)
}

pub fn git_file_view(
    state: &AppState,
    id: String,
    path: Option<String>,
) -> Result<GitFilesView, String> {
    let run = get_plan(state, &id)?;
    let workspace_root = normalize_path(Path::new(&run.plan.workspace_path))?;
    let browse_path = match path.filter(|value| !value.trim().is_empty()) {
        Some(path) => resolve_workspace_file_path(&workspace_root, &path)?,
        None => workspace_root.clone(),
    };
    let browse_path = if browse_path.is_file() {
        browse_path.parent().unwrap_or(&workspace_root).to_path_buf()
    } else {
        browse_path
    };
    let mut entries = browse_host_directory(browse_path.to_string_lossy().to_string())?;
    let repo_root = git_root_for_path(&browse_path).ok();
    let mut branch = None;
    let mut clean = true;
    if let Some(repo_root) = &repo_root {
        branch = git_current_branch(repo_root).ok();
        let statuses = git_status_map(repo_root)?;
        clean = statuses.is_empty();
        for entry in &mut entries {
            let entry_path = normalize_path(Path::new(&entry.path))?;
            let rel = entry_path
                .strip_prefix(repo_root)
                .unwrap_or(&entry_path)
                .to_string_lossy()
                .replace('\\', "/");
            entry.status = git_status_for_entry(&statuses, &rel, entry.kind == "directory");
        }
    }
    Ok(GitFilesView {
        path: browse_path.to_string_lossy().to_string(),
        repo_root: repo_root.map(|path| path.to_string_lossy().to_string()),
        branch,
        clean,
        entries,
    })
}

pub fn run_git_action(
    state: &AppState,
    id: String,
    path: Option<String>,
    action: String,
    message: Option<String>,
) -> Result<GitActionResult, String> {
    let run = get_plan(state, &id)?;
    let workspace_root = normalize_path(Path::new(&run.plan.workspace_path))?;
    let action_path = match path.filter(|value| !value.trim().is_empty()) {
        Some(path) => resolve_workspace_file_path(&workspace_root, &path)?,
        None => workspace_root,
    };
    let repo_root = git_root_for_path(&action_path)?;
    match action.as_str() {
        "fetch" => run_git_command(&repo_root, &["fetch"]),
        "pull" => run_git_command(&repo_root, &["pull"]),
        "push" => run_git_command(&repo_root, &["push"]),
        "commit" => {
            let message = message
                .map(|value| value.trim().to_string())
                .filter(|value| !value.is_empty())
                .ok_or_else(|| "Commit message is required".to_string())?;
            let add = run_git_command(&repo_root, &["add", "-A"])?;
            let commit = run_git_command(&repo_root, &["commit", "-m", &message])?;
            Ok(GitActionResult {
                success: add.success && commit.success,
                output: format!("{}\n{}", add.output, commit.output).trim().to_string(),
            })
        }
        _ => Err(format!("Unsupported git action: {}", action)),
    }
}

pub fn read_host_file(
    state: &AppState,
    id: String,
    path: String,
) -> Result<HostFileContent, String> {
    let run = get_plan(state, &id)?;
    let workspace_root = normalize_path(Path::new(&run.plan.workspace_path))?;
    let file_path = resolve_workspace_file_path(&workspace_root, &path)?;
    let meta = fs::metadata(&file_path).map_err(|e| format!("Failed to inspect file: {}", e))?;
    if !meta.is_file() {
        return Err("Path is not a file".to_string());
    }
    if meta.len() > 5 * 1024 * 1024 {
        return Err("File is too large to preview".to_string());
    }

    let bytes = fs::read(&file_path).map_err(|e| format!("Failed to read file: {}", e))?;
    let content_type = content_type_for_path(&file_path);
    let is_text = is_text_content_type(&content_type);
    let (encoding, content) = if is_text {
        match String::from_utf8(bytes.clone()) {
            Ok(text) => {
                let preview_text = if content_type == "text/html" {
                    prepare_html_preview(&workspace_root, &file_path, &text)
                } else {
                    text
                };
                ("utf8".to_string(), preview_text)
            }
            Err(_) => (
                "base64".to_string(),
                general_purpose::STANDARD.encode(bytes),
            ),
        }
    } else {
        (
            "base64".to_string(),
            general_purpose::STANDARD.encode(bytes),
        )
    };

    let rel = file_path
        .strip_prefix(&workspace_root)
        .unwrap_or(&file_path)
        .to_string_lossy()
        .to_string();

    Ok(HostFileContent {
        path: rel,
        name: file_path
            .file_name()
            .unwrap_or_default()
            .to_string_lossy()
            .to_string(),
        kind: "file".to_string(),
        content_type,
        encoding,
        content,
        size: meta.len(),
    })
}

pub fn get_workspace_file_diff(
    state: &AppState,
    id: String,
    path: String,
) -> Result<WorkspaceFileDiff, String> {
    let run = get_plan(state, &id)?;
    let workspace_root = normalize_path(Path::new(&run.plan.workspace_path))?;
    let git_root_output = std::process::Command::new("git")
        .args(["rev-parse", "--show-toplevel"])
        .current_dir(&workspace_root)
        .output()
        .map_err(|e| format!("Failed to inspect git workspace: {}", e))?;
    if !git_root_output.status.success() {
        return Ok(WorkspaceFileDiff {
            path,
            diff: "Workspace is not a git repository.".to_string(),
        });
    }

    let git_root = String::from_utf8_lossy(&git_root_output.stdout)
        .trim()
        .to_string();
    let file_path = resolve_workspace_file_path(Path::new(&git_root), &path)?;
    let rel = file_path
        .strip_prefix(&git_root)
        .unwrap_or(&file_path)
        .to_string_lossy()
        .to_string();
    let output = std::process::Command::new("git")
        .args(["diff", "--", &rel])
        .current_dir(&git_root)
        .output()
        .map_err(|e| format!("Failed to run git diff: {}", e))?;
    if !output.status.success() {
        return Err(String::from_utf8_lossy(&output.stderr).trim().to_string());
    }
    let mut diff = String::from_utf8_lossy(&output.stdout).to_string();
    if diff.trim().is_empty() {
        let staged = std::process::Command::new("git")
            .args(["diff", "--cached", "--", &rel])
            .current_dir(&git_root)
            .output()
            .map_err(|e| format!("Failed to run staged git diff: {}", e))?;
        if staged.status.success() {
            diff = String::from_utf8_lossy(&staged.stdout).to_string();
        }
    }
    if diff.trim().is_empty() {
        diff = "No diff available for this file.".to_string();
    }

    Ok(WorkspaceFileDiff { path: rel, diff })
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
        if run.plan.run_type == "planning" {
            match run.plan.status.as_str() {
                "planning_planner_running" => {
                    let session_id = run
                        .plan
                        .worker_session_id
                        .clone()
                        .ok_or_else(|| "Planning run has no planner session".to_string())?;
                    wait_for_planner_ready(&state, &session_id).await?;
                    state.db.with_conn(|conn| {
                        conn.execute(
                            "UPDATE agent_plans SET status = 'planning_review_running', updated_at = datetime('now') WHERE id = ?1 AND status = 'planning_planner_running'",
                            params![plan_id],
                        )
                        .map_err(|e| e.to_string())
                    })?;
                    append_event(&state, &plan_id, None, "planning_planner_ready", json!({}))?;
                    let refreshed = get_plan(&state, &plan_id)?;
                    dispatch_planning_review(&state, &refreshed).await?;
                }
                "planning_review_running" => {
                    let session_id = run
                        .plan
                        .reviewer_session_id
                        .clone()
                        .ok_or_else(|| "Planning run has no reviewer session".to_string())?;
                    let snapshot = wait_for_idle(&state, &session_id).await?;
                    handle_planning_reviewer_output(&state, &run, &snapshot.content).await?;
                }
                "approved" | "blocked" | "stopped" | "needs_attention" => break,
                _ => break,
            }
            sleep(Duration::from_millis(300)).await;
            continue;
        }
        match run.plan.status.as_str() {
            "phase_worker_running" => {
                let phase = current_phase(&run)?;
                let session_id = run
                    .plan
                    .worker_session_id
                    .clone()
                    .ok_or_else(|| "Plan has no worker session".to_string())?;
                wait_for_worker_ready(&state, &session_id).await?;
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
                append_event(
                    &state,
                    &plan_id,
                    Some(&phase.phase_id),
                    "agent_phase_worker_idle",
                    json!({}),
                )?;
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
    let mut last_snapshot =
        terminal::capture_terminal_session_with_history(state, session_id).await?;
    loop {
        let snapshot = terminal::capture_terminal_session_with_history(state, session_id).await?;
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

async fn wait_for_worker_ready(
    state: &AppState,
    session_id: &str,
) -> Result<terminal::TerminalSnapshot, String> {
    let mut last_key = String::new();
    let mut last_changed_at = Instant::now();
    let mut last_snapshot =
        terminal::capture_terminal_session_with_history(state, session_id).await?;

    loop {
        let snapshot = terminal::capture_terminal_session_with_history(state, session_id).await?;
        let key = format!(
            "{}\n{}:{}:{}",
            snapshot.content, snapshot.cursor_x, snapshot.cursor_y, snapshot.history_lines
        );
        if key != last_key {
            last_key = key;
            last_changed_at = Instant::now();
            last_snapshot = snapshot;
        }

        if has_ready_marker_line(&last_snapshot.content)
            && last_changed_at.elapsed() >= Duration::from_millis(IDLE_WINDOW_MS)
        {
            return Ok(last_snapshot);
        }

        sleep(Duration::from_millis(1_000)).await;
    }
}

fn has_ready_marker_line(content: &str) -> bool {
    content
        .lines()
        .any(|line| line_contains_marker(line, WORKER_READY_MARKER))
}

fn line_contains_marker(line: &str, marker: &str) -> bool {
    line.chars()
        .filter(|ch| !ch.is_control())
        .collect::<String>()
        .contains(marker)
}

async fn wait_for_planner_ready(
    state: &AppState,
    session_id: &str,
) -> Result<terminal::TerminalSnapshot, String> {
    let mut last_key = String::new();
    let mut last_changed_at = Instant::now();
    let mut last_snapshot =
        terminal::capture_terminal_session_with_history(state, session_id).await?;

    loop {
        let snapshot = terminal::capture_terminal_session_with_history(state, session_id).await?;
        let key = format!(
            "{}\n{}:{}:{}",
            snapshot.content, snapshot.cursor_x, snapshot.cursor_y, snapshot.history_lines
        );
        if key != last_key {
            last_key = key;
            last_changed_at = Instant::now();
            last_snapshot = snapshot;
        }

        if last_snapshot
            .content
            .lines()
            .any(|line| line_contains_marker(line, PLANNER_READY_MARKER))
            && last_changed_at.elapsed() >= Duration::from_millis(IDLE_WINDOW_MS)
        {
            return Ok(last_snapshot);
        }

        sleep(Duration::from_millis(1_000)).await;
    }
}

async fn dispatch_planning_review(state: &AppState, run: &AgentPlanRun) -> Result<(), String> {
    let reviewer_session_id = run
        .plan
        .reviewer_session_id
        .clone()
        .ok_or_else(|| "Planning run has no reviewer session".to_string())?;
    let prompt = planning_reviewer_prompt(run)?;
    terminal::send_terminal_input(state, reviewer_session_id, format!("{}\r", prompt)).await?;
    state.db.with_conn(|conn| {
        conn.execute(
            "UPDATE agent_plans SET status = 'planning_review_running', updated_at = datetime('now') WHERE id = ?1",
            params![run.plan.id],
        )
        .map_err(|e| e.to_string())
    })?;
    append_event(
        state,
        &run.plan.id,
        None,
        "planning_review_started",
        json!({}),
    )
}

async fn handle_planning_reviewer_output(
    state: &AppState,
    run: &AgentPlanRun,
    output: &str,
) -> Result<(), String> {
    let verdict = parse_verdict(output);
    let review = parse_review_insights(output);
    match verdict.as_deref() {
        Some("PASS") => {
            state.db.with_conn(|conn| {
                conn.execute(
                    "UPDATE agent_plans SET status = 'approved', error = NULL, updated_at = datetime('now') WHERE id = ?1",
                    params![run.plan.id],
                )
                .map_err(|e| e.to_string())
            })?;
            // Commit the approved plan state. The message format depends on
            // whether this is the very first approval (initial commit) or a
            // re-approval after an amendment edit. Heuristic: if there are
            // existing commits, treat as amend; if HEAD doesn't exist yet,
            // treat as initial.
            commit_plan_on_pass(state, run);
            append_event(
                state,
                &run.plan.id,
                None,
                "planning_gate_result",
                review_payload("PASS", None, &review),
            )
        }
        Some("NEEDS_CHANGES") | Some("BLOCKED") => {
            let verdict = verdict.unwrap();
            state.db.with_conn(|conn| {
                conn.execute(
                    "UPDATE agent_plans SET status = 'planning_planner_running', error = ?1, updated_at = datetime('now') WHERE id = ?2",
                    params![review.summary.clone().unwrap_or_else(|| summarize_output(output)), run.plan.id],
                )
                .map_err(|e| e.to_string())
            })?;
            append_event(
                state,
                &run.plan.id,
                None,
                "planning_gate_result",
                review_payload(&verdict, Some("sent_back_to_planner"), &review),
            )?;
            send_planning_feedback_to_planner(state, run, output, &review).await
        }
        _ => clarify_planning_or_needs_attention(state, run).await,
    }
}

async fn send_planning_feedback_to_planner(
    state: &AppState,
    run: &AgentPlanRun,
    reviewer_output: &str,
    review: &ReviewInsights,
) -> Result<(), String> {
    let planner_session_id = run
        .plan
        .worker_session_id
        .clone()
        .ok_or_else(|| "Planning run has no planner session".to_string())?;
    let feedback = reviewer_verdict_block(reviewer_output);
    let prompt = format!(
        "T2 reviewed the plan and requested changes.\n\nReviewer feedback:\n{}\n\nUpdate only the plan files at {}. When the plan is ready for review again, say exactly {}.",
        feedback, run.plan.plan_path, PLANNER_READY_MARKER
    );
    terminal::send_terminal_input(state, planner_session_id, format!("{}\r", prompt)).await?;
    append_event(
        state,
        &run.plan.id,
        None,
        "planning_feedback_sent_to_planner",
        feedback_event_payload(review),
    )
}

async fn clarify_planning_or_needs_attention(
    state: &AppState,
    run: &AgentPlanRun,
) -> Result<(), String> {
    let attempts = planning_clarification_attempts(run)? + 1;
    if attempts > CLARIFICATION_LIMIT {
        state.db.with_conn(|conn| {
            conn.execute(
                "UPDATE agent_plans SET status = 'needs_attention', error = 'T2 did not return a parseable planning verdict after 5 clarification attempts', updated_at = datetime('now') WHERE id = ?1",
                params![run.plan.id],
            )
            .map_err(|e| e.to_string())
        })?;
        return append_event(
            state,
            &run.plan.id,
            None,
            "planning_needs_attention",
            json!({ "reason": "unknown_verdict" }),
        );
    }

    let reviewer_session_id = run
        .plan
        .reviewer_session_id
        .clone()
        .ok_or_else(|| "Planning run has no reviewer session".to_string())?;
    let prompt = "Return only this footer, with no extra text:\nPLAN: <plan path>\nVERDICT: PASS | NEEDS_CHANGES | BLOCKED\nSUMMARY: <one paragraph>\nFINDINGS:\n- <finding or none>\nNEXT_STEPS:\n- <step or none>";
    terminal::send_terminal_input(state, reviewer_session_id, format!("{}\r", prompt)).await?;
    append_event(
        state,
        &run.plan.id,
        None,
        "planning_verdict_clarification_requested",
        json!({ "attempt": attempts }),
    )
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
    append_event(
        state,
        &run.plan.id,
        Some(&phase.phase_id),
        "agent_phase_review_started",
        json!({}),
    )
}

async fn handle_reviewer_output(
    state: &AppState,
    run: &AgentPlanRun,
    phase: &AgentPlanPhase,
    output: &str,
) -> Result<(), String> {
    let verdict = parse_verdict(output);
    let review = parse_review_insights(output);
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
                    params![review.summary.clone().unwrap_or_else(|| summarize_output(output)), run.plan.id, phase.phase_id],
                )
                .map_err(|e| e.to_string())
            })?;
            append_event(
                state,
                &run.plan.id,
                Some(&phase.phase_id),
                "agent_phase_gate_result",
                review_payload("NEEDS_CHANGES", None, &review),
            )?;
            send_reviewer_feedback_to_worker(state, run, phase, output, &review).await
        }
        Some("BLOCKED") => {
            state.db.with_conn(|conn| {
                conn.execute(
                    "UPDATE agent_plans SET status = 'phase_needs_changes', updated_at = datetime('now') WHERE id = ?1",
                    params![run.plan.id],
                )
                .map_err(|e| e.to_string())?;
                conn.execute(
                    "UPDATE agent_plan_phases SET status = 'needs_changes', gate_verdict = 'blocked', reviewer_idle_at = datetime('now'), summary = ?1, updated_at = datetime('now') WHERE plan_id = ?2 AND phase_id = ?3",
                    params![review.summary.clone().unwrap_or_else(|| summarize_output(output)), run.plan.id, phase.phase_id],
                )
                .map_err(|e| e.to_string())
            })?;
            append_event(
                state,
                &run.plan.id,
                Some(&phase.phase_id),
                "agent_phase_gate_result",
                review_payload("BLOCKED", Some("sent_back_to_worker"), &review),
            )?;
            send_reviewer_feedback_to_worker(state, run, phase, output, &review).await
        }
        _ => clarify_or_needs_attention(state, run, phase).await,
    }
}

async fn send_reviewer_feedback_to_worker(
    state: &AppState,
    run: &AgentPlanRun,
    phase: &AgentPlanPhase,
    reviewer_output: &str,
    review: &ReviewInsights,
) -> Result<(), String> {
    let worker_session_id = run
        .plan
        .worker_session_id
        .clone()
        .ok_or_else(|| "Plan has no worker session".to_string())?;
    let prompt = reviewer_feedback_prompt(phase, reviewer_output);
    terminal::send_terminal_input(state, worker_session_id, format!("{}\r", prompt)).await?;
    state.db.with_conn(|conn| {
        conn.execute(
            "UPDATE agent_plans SET status = 'phase_worker_running', updated_at = datetime('now') WHERE id = ?1",
            params![run.plan.id],
        )
        .map_err(|e| e.to_string())?;
        conn.execute(
            "UPDATE agent_plan_phases SET status = 'worker_running', updated_at = datetime('now') WHERE plan_id = ?1 AND phase_id = ?2",
            params![run.plan.id, phase.phase_id],
        )
        .map_err(|e| e.to_string())
    })?;
    append_event(
        state,
        &run.plan.id,
        Some(&phase.phase_id),
        "agent_feedback_sent_to_worker",
        feedback_event_payload(review),
    )
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
        return append_event(
            state,
            &run.plan.id,
            Some(&phase.phase_id),
            "agent_phase_needs_attention",
            json!({ "reason": "unknown_verdict" }),
        );
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
    append_event(
        state,
        &run.plan.id,
        Some(&phase.phase_id),
        "agent_phase_verdict_clarification_requested",
        json!({ "attempt": next_attempt }),
    )
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
    let should_continue = run.plan.phase_run_mode != "single";
    state.db.with_conn(|conn| {
        conn.execute(
            "UPDATE agent_plan_phases SET status = 'passed', gate_verdict = 'pass', reviewer_idle_at = datetime('now'), summary = ?1, updated_at = datetime('now') WHERE plan_id = ?2 AND phase_id = ?3",
            params![summary, plan_id, phase_id],
        )
        .map_err(|e| e.to_string())?;
        if should_continue {
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
                "UPDATE agent_plans SET status = 'approved', phase_run_mode = 'continue', updated_at = datetime('now') WHERE id = ?1",
                params![plan_id],
            )
            .map_err(|e| e.to_string())?;
        }
        } else {
            conn.execute(
                "UPDATE agent_plans SET status = 'approved', phase_run_mode = 'continue', updated_at = datetime('now') WHERE id = ?1",
                params![plan_id],
            )
            .map_err(|e| e.to_string())?;
        }
        Ok(())
    })?;
    // Commit the phase-pass state. Message ties the commit to a phase ID so
    // `git log` reads like a progression through the plan's phases.
    let message = format!("phase {}: {} (validated)", phase.phase_id, phase.phase_title);
    if let Err(err) = super::git_history::commit_all(&run.plan.plan_path, &message) {
        tracing::warn!(plan_id = %plan_id, phase_id = %phase_id, %err, "git_history::commit_all (phase pass) failed");
    }

    append_event(
        state,
        plan_id,
        Some(phase_id),
        "agent_phase_gate_result",
        json!({ "verdict": "PASS" }),
    )?;

    if should_continue {
        if let Some(next) = next_phase {
        let refreshed = get_plan(state, plan_id)?;
        let worker_session_id = refreshed
            .plan
            .worker_session_id
            .clone()
            .ok_or_else(|| "Plan has no worker session".to_string())?;
        let prompt = worker_phase_prompt(&refreshed, &next)?;
        terminal::send_terminal_input(state, worker_session_id, format!("{}\r", prompt)).await?;
        append_event(
            state,
            plan_id,
            Some(&next.phase_id),
            "agent_phase_unlocked",
            json!({}),
        )?;
        } else {
            append_event(state, plan_id, None, "agent_plan_completed", json!({}))?;
        }
    } else {
        append_event(
            state,
            plan_id,
            Some(phase_id),
            "agent_single_phase_completed",
            json!({ "phaseId": phase_id }),
        )?;
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
    let title = first_markdown_heading(&plan.join("overview.md")).unwrap_or_else(|| {
        plan.file_name()
            .unwrap_or_default()
            .to_string_lossy()
            .to_string()
    });
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
        phases.push(ParsedPhase {
            phase_id,
            title,
            tasks,
        });
    }
    phases.sort_by(|a, b| a.phase_id.cmp(&b.phase_id));
    if phases.is_empty() {
        return Err("Plan has no phases with overview.md files".to_string());
    }
    Ok(ParsedPlan {
        title,
        workspace_path: workspace,
        plan_path: plan,
        phases,
    })
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
        let status_path = task_status_path(&entry.path());
        let status = status_path
            .as_ref()
            .and_then(|path| task_status_from_file(path))
            .unwrap_or_else(|| "planned".to_string());
        tasks.push(ParsedTask {
            task_id,
            title,
            prompt_path,
            status_path,
            status,
            decisions_path: Some(entry.path().join("decisions.md")).filter(|p| p.exists()),
        });
    }
    tasks.sort_by(|a, b| a.task_id.cmp(&b.task_id));
    Ok(tasks)
}

fn task_status_path(task_path: &Path) -> Option<PathBuf> {
    ["status.yml", "status.yaml", "status.md"]
        .iter()
        .map(|name| task_path.join(name))
        .find(|path| path.is_file())
}

fn task_status_from_file(path: &Path) -> Option<String> {
    let content = fs::read_to_string(path).ok()?;
    match path.extension().and_then(|ext| ext.to_str()) {
        Some("yml") | Some("yaml") => {
            let parsed = serde_yaml::from_str::<TaskStatusYaml>(&content).ok()?;
            parsed
                .state
                .or(parsed.status)
                .map(|status| normalize_task_status(&status))
        }
        _ => task_status_from_markdown(&content).map(|status| normalize_task_status(&status)),
    }
}

fn task_status_from_markdown(content: &str) -> Option<String> {
    content.lines().find_map(|line| {
        let trimmed = line.trim();
        let value = trimmed
            .strip_prefix("**State:**")
            .or_else(|| trimmed.strip_prefix("State:"))
            .or_else(|| trimmed.strip_prefix("Status:"))?
            .trim()
            .trim_matches('*')
            .trim();
        value
            .split('|')
            .next()
            .map(str::trim)
            .filter(|value| !value.is_empty())
            .map(str::to_string)
    })
}

fn normalize_task_status(status: &str) -> String {
    let normalized = status.trim().to_ascii_lowercase().replace('_', "-");
    match normalized.as_str() {
        "not started" | "not-started" => "not-started".to_string(),
        "in progress" | "in-progress" | "running" | "worker-running" => "in-progress".to_string(),
        "done" | "passed" | "complete" | "completed" => "done".to_string(),
        "blocked" => "blocked".to_string(),
        "needs changes" | "needs-changes" => "needs-changes".to_string(),
        "planned" | "draft" | "locked" => "planned".to_string(),
        _ => normalized,
    }
}

fn normalize_path(path: &Path) -> Result<PathBuf, String> {
    if path.exists() {
        path.canonicalize()
            .map_err(|e| format!("Invalid path: {}", e))
    } else {
        let parent = path
            .parent()
            .ok_or_else(|| "Path has no parent".to_string())?
            .canonicalize()
            .map_err(|e| format!("Invalid parent path: {}", e))?;
        Ok(parent.join(path.file_name().unwrap_or_default()))
    }
}

fn normalize_optional_workspace_path(
    workspace: &Path,
    path: Option<&str>,
) -> Result<Option<String>, String> {
    let Some(path) = path.map(str::trim).filter(|value| !value.is_empty()) else {
        return Ok(None);
    };
    let candidate = if Path::new(path).is_absolute() {
        PathBuf::from(path)
    } else {
        workspace.join(path)
    };
    let normalized_workspace = normalize_path(workspace)?;
    let normalized = normalize_path(&candidate)?;
    if !normalized.starts_with(&normalized_workspace) {
        return Err(format!("Path must stay inside workspace: {}", path));
    }
    Ok(Some(normalized.to_string_lossy().to_string()))
}

fn normalize_reference_paths(
    workspace: &Path,
    paths: Option<&str>,
) -> Result<Option<String>, String> {
    let Some(paths) = paths.map(str::trim).filter(|value| !value.is_empty()) else {
        return Ok(None);
    };
    let mut normalized = Vec::new();
    for line in paths.lines() {
        let value = line.trim();
        if value.is_empty() {
            continue;
        }
        if let Some(path) = normalize_optional_workspace_path(workspace, Some(value))? {
            normalized.push(path);
        }
    }
    Ok((!normalized.is_empty()).then(|| normalized.join("\n")))
}

fn first_markdown_heading(path: &Path) -> Option<String> {
    fs::read_to_string(path).ok()?.lines().find_map(|line| {
        line.strip_prefix("# ")
            .map(|heading| heading.trim().to_string())
    })
}

fn default_model_for_provider(provider: &str) -> Option<String> {
    if provider == "ollama" {
        Some("qwen3.5:2b".to_string())
    } else {
        None
    }
}

fn worker_phase_prompt(run: &AgentPlanRun, phase: &AgentPlanPhase) -> Result<String, String> {
    let phase_path = Path::new(&run.plan.plan_path)
        .join("phases")
        .join(&phase.phase_id);
    let tasks_path = phase_path.join("tasks");
    let settings = planner_prompts::load_prompt_settings()?;
    Ok(planner_prompts::render_template(
        &settings.development.worker,
        &phase_template_values(run, phase, &phase_path, &tasks_path),
    ))
}

fn reviewer_phase_prompt(run: &AgentPlanRun, phase: &AgentPlanPhase) -> Result<String, String> {
    let phase_path = Path::new(&run.plan.plan_path)
        .join("phases")
        .join(&phase.phase_id);
    let tasks_path = phase_path.join("tasks");
    let settings = planner_prompts::load_prompt_settings()?;
    Ok(planner_prompts::render_template(
        &settings.development.reviewer,
        &phase_template_values(run, phase, &phase_path, &tasks_path),
    ))
}

fn phase_template_values(
    run: &AgentPlanRun,
    phase: &AgentPlanPhase,
    phase_path: &Path,
    tasks_path: &Path,
) -> Vec<(&'static str, String)> {
    let conventions_path = Path::new(&run.plan.workspace_path).join("common/conventions");
    vec![
        ("run_id", run.plan.id.clone()),
        ("phase_id", phase.phase_id.clone()),
        ("workspace_path", run.plan.workspace_path.clone()),
        ("plan_path", run.plan.plan_path.clone()),
        ("phase_path", phase_path.to_string_lossy().to_string()),
        ("tasks_path", tasks_path.to_string_lossy().to_string()),
        (
            "conventions_path",
            conventions_path.to_string_lossy().to_string(),
        ),
    ]
}

fn planning_template_values(run: &AgentPlanRun) -> Vec<(&'static str, String)> {
    let methodology_path = Path::new(&run.plan.workspace_path).join("common/methodology.md");
    let conventions_path = Path::new(&run.plan.workspace_path).join("common/conventions");
    vec![
        ("run_id", run.plan.id.clone()),
        ("workspace_path", run.plan.workspace_path.clone()),
        ("plan_output_path", run.plan.plan_path.clone()),
        (
            "methodology_path",
            methodology_path.to_string_lossy().to_string(),
        ),
        (
            "conventions_path",
            conventions_path.to_string_lossy().to_string(),
        ),
        (
            "app_scope",
            run.plan
                .app_scope
                .clone()
                .unwrap_or_else(|| run.plan.workspace_path.clone()),
        ),
        (
            "docs_scope",
            run.plan
                .docs_scope
                .clone()
                .unwrap_or_else(|| run.plan.plan_path.clone()),
        ),
        ("user_brief", run.plan.brief.clone().unwrap_or_default()),
        (
            "reference_paths",
            run.plan.reference_paths.clone().unwrap_or_default(),
        ),
        // Slice 2: amend-cycle placeholders. Empty when not amending so the
        // template renders cleanly in either mode.
        (
            "amendment_brief",
            run.plan.amend_brief.clone().unwrap_or_default(),
        ),
        (
            "git_diff",
            super::git_history::diff_head(&run.plan.plan_path).unwrap_or_default(),
        ),
    ]
}

fn planning_planner_prompt(run: &AgentPlanRun) -> Result<String, String> {
    let settings = planner_prompts::load_prompt_settings()?;
    // Pick the template variant based on whether an amend cycle is in flight.
    // The `amend_planner` / `amend_reviewer` templates instruct T1 to edit in
    // place + T2 to validate the diff, rather than treating the plan as a
    // greenfield creation.
    let amending = run
        .plan
        .amend_brief
        .as_deref()
        .map(|s| !s.trim().is_empty())
        .unwrap_or(false);
    let template = if amending {
        &settings.planning.amend_planner
    } else {
        &settings.planning.planner
    };
    Ok(planner_prompts::render_template(
        template,
        &planning_template_values(run),
    ))
}

fn planning_reviewer_prompt(run: &AgentPlanRun) -> Result<String, String> {
    let settings = planner_prompts::load_prompt_settings()?;
    let amending = run
        .plan
        .amend_brief
        .as_deref()
        .map(|s| !s.trim().is_empty())
        .unwrap_or(false);
    let template = if amending {
        &settings.planning.amend_reviewer
    } else {
        &settings.planning.reviewer
    };
    Ok(planner_prompts::render_template(
        template,
        &planning_template_values(run),
    ))
}

fn planning_clarification_attempts(run: &AgentPlanRun) -> Result<i64, String> {
    Ok(run
        .events
        .iter()
        .filter(|event| event.event_type == "planning_verdict_clarification_requested")
        .count() as i64)
}

fn feedback_prompt(phase: &AgentPlanPhase) -> String {
    format!(
        "T2 returned a non-pass verdict for phase {}. Review the findings in the terminal above, fix what is needed or provide evidence it is already done, then say READY_FOR_T2_VALIDATION again.",
        phase.phase_id
    )
}

fn reviewer_feedback_prompt(phase: &AgentPlanPhase, reviewer_output: &str) -> String {
    let feedback = reviewer_verdict_block(reviewer_output);
    format!(
        "T2 returned a non-pass verdict for phase {}.\n\nReviewer feedback:\n{}\n\nFix what is needed, provide the missing evidence, or document the explicit waiver/scope decision T2 asked for. Do not start later phases. When done, update task status/decisions/artifacts and say READY_FOR_T2_VALIDATION again.",
        phase.phase_id, feedback
    )
}

fn reviewer_verdict_block(output: &str) -> String {
    let lines = output.lines().collect::<Vec<_>>();
    let start = lines
        .iter()
        .rposition(|line| {
            let normalized = marker_search_text(line);
            normalized.contains("PHASE:") || normalized.contains("PLAN:")
        })
        .unwrap_or(0);
    lines[start..]
        .iter()
        .copied()
        .filter(|line| !line.trim().is_empty())
        .take(80)
        .collect::<Vec<_>>()
        .join("\n")
}

fn parse_review_insights(output: &str) -> ReviewInsights {
    let summary = extract_summary_block(output).or_else(|| {
        let fallback = summarize_output(output);
        if fallback == "No summary" {
            None
        } else {
            Some(fallback)
        }
    });
    let findings = extract_bullet_section(output, "FINDINGS:");
    let next_steps = extract_bullet_section(output, "NEXT_STEPS:");
    let reason = findings
        .iter()
        .find(|item| !is_placeholder_bullet(item))
        .cloned()
        .or_else(|| summary.clone());

    ReviewInsights {
        summary,
        findings,
        next_steps,
        reason,
    }
}

fn review_payload(
    verdict: &str,
    action: Option<&str>,
    review: &ReviewInsights,
) -> serde_json::Value {
    json!({
        "verdict": verdict,
        "action": action,
        "summary": review.summary,
        "reason": review.reason,
        "findings": review.findings,
        "nextSteps": review.next_steps,
    })
}

fn feedback_event_payload(review: &ReviewInsights) -> serde_json::Value {
    json!({
        "source": "auto_needs_changes",
        "summary": review.summary,
        "reason": review.reason,
        "findings": review.findings,
        "nextSteps": review.next_steps,
    })
}

/// Commit the planning-mode plan on T2 PASS. The first PASS (no prior HEAD)
/// produces an `initial: <title>` commit. Subsequent PASSes (re-running T1
/// after edits or amendments) produce `amend: <brief excerpt>` commits if an
/// `amend_brief` is set on the plan row, otherwise `revalidate: <title>`.
///
/// Failures are logged + swallowed so a missing `git` binary never blocks the
/// planner from approving a plan.
fn commit_plan_on_pass(state: &AppState, run: &AgentPlanRun) {
    let plan_path = &run.plan.plan_path;
    let title = if run.plan.title.trim().is_empty() {
        "plan"
    } else {
        run.plan.title.trim()
    };

    // Message shape — three cases:
    //   1. First commit ever        → "initial: <title>"
    //   2. Subsequent + amend_brief → "amend: <brief excerpt>" (the user is amending)
    //   3. Subsequent + no amend    → "revalidate: <title>" (T1 re-ran without an amend trigger)
    let has_prior = super::git_history::has_any_commit(plan_path);
    let message = match (has_prior, run.plan.amend_brief.as_deref()) {
        (false, _) => format!("initial: {}", title),
        (true, Some(brief)) if !brief.trim().is_empty() => {
            super::git_history::amend_commit_message(brief)
        }
        (true, _) => format!("revalidate: {}", title),
    };

    match super::git_history::commit_all(plan_path, &message) {
        Ok(Some(sha)) => {
            tracing::info!(plan_id = %run.plan.id, %sha, %message, "plan committed to git history")
        }
        Ok(None) => {
            tracing::debug!(plan_id = %run.plan.id, "no changes to commit on T2 PASS")
        }
        Err(err) => {
            tracing::warn!(plan_id = %run.plan.id, %err, "git_history::commit_all (planning pass) failed")
        }
    }

    // Amend cycle complete — clear the brief so future PASSes don't keep
    // attributing themselves to this amendment.
    if run.plan.amend_brief.is_some() {
        let cleared = state.db.with_conn(|conn| {
            conn.execute(
                "UPDATE agent_plans SET amend_brief = NULL, updated_at = datetime('now') WHERE id = ?1",
                params![run.plan.id],
            )
            .map_err(|e| e.to_string())
        });
        if let Err(err) = cleared {
            tracing::warn!(plan_id = %run.plan.id, %err, "failed to clear amend_brief after commit");
        }
    }
}

fn parse_verdict(output: &str) -> Option<String> {
    output.lines().rev().find_map(|line| {
        let normalized = marker_search_text(line);
        let value = normalized
            .split("VERDICT:")
            .nth(1)?
            .trim()
            .to_ascii_uppercase();
        let verdict = value.split_whitespace().next().unwrap_or_default();
        match verdict {
            "PASS" | "NEEDS_CHANGES" | "BLOCKED" => Some(verdict.to_string()),
            _ => None,
        }
    })
}

fn marker_search_text(line: &str) -> String {
    line.chars()
        .filter(|ch| !ch.is_control())
        .collect::<String>()
}

fn summarize_output(output: &str) -> String {
    if let Some(summary) = extract_summary_block(output) {
        return summary;
    }

    output
        .lines()
        .rev()
        .map(marker_search_text)
        .find(|line| is_summary_fallback_line(line))
        .unwrap_or_else(|| "No summary".to_string())
        .trim()
        .chars()
        .take(500)
        .collect()
}

fn extract_summary_block(output: &str) -> Option<String> {
    let lines = output.lines().map(marker_search_text).collect::<Vec<_>>();
    let start = lines
        .iter()
        .rposition(|line| line.to_ascii_uppercase().contains("SUMMARY:"))?;
    let mut parts = Vec::new();
    if let Some((_, value)) = lines[start].split_once("SUMMARY:") {
        if !value.trim().is_empty() {
            parts.push(value.trim().to_string());
        }
    }
    for line in lines.iter().skip(start + 1) {
        let trimmed = line.trim();
        if trimmed.is_empty() {
            if !parts.is_empty() {
                break;
            }
            continue;
        }
        let upper = trimmed.to_ascii_uppercase();
        if upper.starts_with("FINDINGS:")
            || upper.starts_with("NEXT_STEPS:")
            || upper.starts_with("PLAN:")
            || upper.starts_with("PHASE:")
            || upper.starts_with("VERDICT:")
        {
            break;
        }
        parts.push(trimmed.to_string());
    }
    let summary = parts.join(" ");
    let summary = summary.split_whitespace().collect::<Vec<_>>().join(" ");
    if summary.is_empty() {
        None
    } else {
        Some(summary.chars().take(500).collect())
    }
}

fn extract_bullet_section(output: &str, header: &str) -> Vec<String> {
    let lines = output.lines().map(marker_search_text).collect::<Vec<_>>();
    let Some(start) = lines
        .iter()
        .rposition(|line| line.to_ascii_uppercase().contains(header))
    else {
        return Vec::new();
    };

    let mut items = Vec::new();
    for line in lines.iter().skip(start + 1) {
        let trimmed = line.trim();
        if trimmed.is_empty() {
            if !items.is_empty() {
                break;
            }
            continue;
        }
        let upper = trimmed.to_ascii_uppercase();
        if upper.starts_with("SUMMARY:")
            || upper.starts_with("FINDINGS:")
            || upper.starts_with("NEXT_STEPS:")
            || upper.starts_with("PLAN:")
            || upper.starts_with("PHASE:")
            || upper.starts_with("VERDICT:")
        {
            break;
        }
        let Some(item) = trimmed.strip_prefix('-').or_else(|| trimmed.strip_prefix('*')) else {
            if !items.is_empty() {
                break;
            }
            continue;
        };
        let normalized = item.split_whitespace().collect::<Vec<_>>().join(" ");
        if !normalized.is_empty() {
            items.push(normalized);
        }
    }

    items
}

fn is_placeholder_bullet(value: &str) -> bool {
    matches!(
        value.trim().to_ascii_lowercase().as_str(),
        "none" | "n/a" | "na" | "nil"
    )
}

fn is_summary_fallback_line(line: &str) -> bool {
    let trimmed = line.trim();
    if trimmed.is_empty() {
        return false;
    }
    let lower = trimmed.to_ascii_lowercase();
    !trimmed.starts_with('>')
        && !trimmed.starts_with('❯')
        && !trimmed
            .chars()
            .all(|ch| matches!(ch, '-' | '_' | '=' | ' '))
        && !lower.contains("bypass permissions")
        && !lower.starts_with("worked for")
        && !lower.starts_with("cogitated")
        && !lower.starts_with("cooked")
        && !lower.starts_with("churned")
        && !lower.starts_with("baked")
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

fn sync_task_statuses_from_files(state: &AppState, plan_id: &str) -> Result<(), String> {
    let rows: Vec<(String, Option<String>, String)> = state.db.with_conn(|conn| {
        let mut stmt = conn
            .prepare("SELECT id, status_path, status FROM agent_plan_tasks WHERE plan_id = ?1")
            .map_err(|e| e.to_string())?;
        let rows = stmt
            .query_map(params![plan_id], |row| {
                Ok((
                    row.get::<_, String>(0)?,
                    row.get::<_, Option<String>>(1)?,
                    row.get::<_, String>(2)?,
                ))
            })
            .map_err(|e| e.to_string())?
            .filter_map(|row| row.ok())
            .collect();
        Ok(rows)
    })?;

    for (id, status_path, current_status) in rows {
        let Some(status_path) = status_path else {
            continue;
        };
        let Some(next_status) = task_status_from_file(Path::new(&status_path)) else {
            continue;
        };
        if next_status == current_status {
            continue;
        }
        state.db.with_conn(|conn| {
            conn.execute(
                "UPDATE agent_plan_tasks SET status = ?1, updated_at = datetime('now') WHERE id = ?2",
                params![next_status, id],
            )
            .map_err(|e| e.to_string())
        })?;
    }

    Ok(())
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
        let mut phase_meta = HashMap::new();
        let mut phase_stmt = conn.prepare(
            "SELECT phase_id, phase_index, phase_title FROM agent_plan_phases WHERE plan_id = ?1",
        ).map_err(|e| e.to_string())?;
        let phase_rows = phase_stmt
            .query_map(params![plan_id], |row| {
                Ok((
                    row.get::<_, String>(0)?,
                    row.get::<_, i64>(1)?,
                    row.get::<_, String>(2)?,
                ))
            })
            .map_err(|e| e.to_string())?;
        for row in phase_rows {
            let (phase_id, phase_index, phase_title) = row.map_err(|e| e.to_string())?;
            phase_meta.insert(phase_id, (phase_index, phase_title));
        }
        for event in &mut rows {
            enrich_agent_plan_event(event, &phase_meta);
        }
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
            let rel = path
                .strip_prefix(&root)
                .unwrap_or(&path)
                .to_string_lossy()
                .to_string();
            entries.push(HostFileEntry {
                path: rel,
                name,
                kind: if meta.is_dir() { "directory" } else { "file" }.to_string(),
                status: None,
                size: if meta.is_file() {
                    Some(meta.len())
                } else {
                    None
                },
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

fn git_changed_files(workspace_path: &str, path_filter: Option<&str>) -> Result<Vec<HostFileEntry>, String> {
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

    let git_root = String::from_utf8_lossy(&root_output.stdout)
        .trim()
        .to_string();
    let filter_prefix = match path_filter.filter(|path| !path.trim().is_empty()) {
        Some(path) => {
            let filter_path = resolve_workspace_file_path(Path::new(&git_root), path)?;
            let rel = filter_path
                .strip_prefix(&git_root)
                .unwrap_or(&filter_path)
                .to_string_lossy()
                .trim_matches('/')
                .to_string();
            if rel.is_empty() { None } else { Some(format!("{}/", rel)) }
        }
        None => None,
    };
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
        if let Some(prefix) = &filter_prefix {
            if path != prefix.trim_end_matches('/') && !path.starts_with(prefix) {
                continue;
            }
        }
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

fn git_root_for_path(path: &Path) -> Result<PathBuf, String> {
    let current_dir = if path.is_file() {
        path.parent().unwrap_or(path)
    } else {
        path
    };
    let output = std::process::Command::new("git")
        .args(["rev-parse", "--show-toplevel"])
        .current_dir(current_dir)
        .output()
        .map_err(|e| format!("Failed to inspect git workspace: {}", e))?;
    if !output.status.success() {
        return Err("Path is not inside a git repository".to_string());
    }
    Ok(PathBuf::from(String::from_utf8_lossy(&output.stdout).trim()))
}

fn git_current_branch(repo_root: &Path) -> Result<String, String> {
    let output = std::process::Command::new("git")
        .args(["branch", "--show-current"])
        .current_dir(repo_root)
        .output()
        .map_err(|e| format!("Failed to read git branch: {}", e))?;
    if !output.status.success() {
        return Err(String::from_utf8_lossy(&output.stderr).trim().to_string());
    }
    let branch = String::from_utf8_lossy(&output.stdout).trim().to_string();
    Ok(if branch.is_empty() { "detached".to_string() } else { branch })
}

fn git_status_map(repo_root: &Path) -> Result<HashMap<String, String>, String> {
    let output = std::process::Command::new("git")
        .args(["status", "--porcelain"])
        .current_dir(repo_root)
        .output()
        .map_err(|e| format!("Failed to run git status: {}", e))?;
    if !output.status.success() {
        return Err(String::from_utf8_lossy(&output.stderr).trim().to_string());
    }
    let mut statuses = HashMap::new();
    for line in String::from_utf8_lossy(&output.stdout).lines() {
        if line.len() < 4 {
            continue;
        }
        let code = line[..2].to_string();
        let raw_path = line[3..].trim();
        let path = raw_path
            .rsplit_once(" -> ")
            .map(|(_, renamed)| renamed)
            .unwrap_or(raw_path)
            .to_string();
        statuses.insert(path, normalize_git_status(&code));
    }
    Ok(statuses)
}

fn git_status_for_entry(
    statuses: &HashMap<String, String>,
    rel_path: &str,
    is_directory: bool,
) -> Option<String> {
    if let Some(status) = statuses.get(rel_path) {
        return Some(status.clone());
    }
    if is_directory {
        let prefix = format!("{}/", rel_path.trim_end_matches('/'));
        if statuses.keys().any(|path| path.starts_with(&prefix)) {
            return Some("changed".to_string());
        }
    }
    None
}

fn normalize_git_status(code: &str) -> String {
    if code == "??" {
        return "untracked".to_string();
    }
    if code.contains('A') {
        return "added".to_string();
    }
    if code.contains('D') {
        return "deleted".to_string();
    }
    if code.contains('R') {
        return "renamed".to_string();
    }
    if code.contains('M') {
        return "modified".to_string();
    }
    "changed".to_string()
}

fn run_git_command(repo_root: &Path, args: &[&str]) -> Result<GitActionResult, String> {
    let output = std::process::Command::new("git")
        .args(args)
        .current_dir(repo_root)
        .output()
        .map_err(|e| format!("Failed to run git {}: {}", args.join(" "), e))?;
    let stdout = String::from_utf8_lossy(&output.stdout);
    let stderr = String::from_utf8_lossy(&output.stderr);
    Ok(GitActionResult {
        success: output.status.success(),
        output: format!("{}{}", stdout, stderr).trim().to_string(),
    })
}

fn resolve_workspace_file_path(root: &Path, path: &str) -> Result<PathBuf, String> {
    let candidate = if Path::new(path).is_absolute() {
        PathBuf::from(path)
    } else {
        root.join(path)
    };
    let normalized_root = normalize_path(root)?;
    let normalized = normalize_path(&candidate)?;
    if !normalized.starts_with(&normalized_root) {
        return Err("File path is outside the workspace".to_string());
    }
    Ok(normalized)
}

fn content_type_for_path(path: &Path) -> String {
    match path
        .extension()
        .and_then(|value| value.to_str())
        .unwrap_or_default()
        .to_lowercase()
        .as_str()
    {
        "md" | "markdown" => "text/markdown",
        "html" | "htm" => "text/html",
        "yaml" | "yml" => "application/yaml",
        "json" => "application/json",
        "css" => "text/css",
        "js" | "mjs" | "cjs" | "ts" | "tsx" | "jsx" => "text/plain",
        "txt" | "log" | "rs" | "toml" | "graphql" | "sql" | "sh" | "svg" => "text/plain",
        "png" => "image/png",
        "jpg" | "jpeg" => "image/jpeg",
        "gif" => "image/gif",
        "webp" => "image/webp",
        _ => "application/octet-stream",
    }
    .to_string()
}

fn prepare_html_preview(workspace_root: &Path, html_path: &Path, html: &str) -> String {
    let base_dir = html_path.parent().unwrap_or(workspace_root);
    let html = inline_link_stylesheets(workspace_root, base_dir, html);
    let html = inline_script_sources(workspace_root, base_dir, &html);
    inline_image_sources(workspace_root, base_dir, &html)
}

fn inline_link_stylesheets(workspace_root: &Path, base_dir: &Path, html: &str) -> String {
    replace_html_tags(html, "link", false, |tag, _body| {
        let lower = tag.to_lowercase();
        if !lower.contains("stylesheet") {
            return None;
        }
        let href = html_attr(tag, "href")?;
        let css_path = resolve_preview_asset(workspace_root, base_dir, &href)?;
        if content_type_for_path(&css_path) != "text/css" {
            return None;
        }
        let css = fs::read_to_string(css_path).ok()?;
        Some(format!(
            "<style data-johnnyone-inlined=\"{}\">\n{}\n</style>",
            escape_html_attr(&href),
            css
        ))
    })
}

fn inline_script_sources(workspace_root: &Path, base_dir: &Path, html: &str) -> String {
    replace_html_tags(html, "script", true, |tag, _body| {
        let src = html_attr(tag, "src")?;
        let script_path = resolve_preview_asset(workspace_root, base_dir, &src)?;
        let script = fs::read_to_string(script_path).ok()?;
        let mut attrs = remove_html_attr(tag, "src");
        if !attrs.contains("data-johnnyone-inlined") {
            attrs.push_str(&format!(
                " data-johnnyone-inlined=\"{}\"",
                escape_html_attr(&src)
            ));
        }
        Some(format!("{}{}\n</script>", attrs, script))
    })
}

fn inline_image_sources(workspace_root: &Path, base_dir: &Path, html: &str) -> String {
    replace_html_tags(html, "img", false, |tag, _body| {
        let src = html_attr(tag, "src")?;
        let image_path = resolve_preview_asset(workspace_root, base_dir, &src)?;
        let content_type = content_type_for_path(&image_path);
        if !content_type.starts_with("image/") {
            return None;
        }
        let meta = fs::metadata(&image_path).ok()?;
        if meta.len() > 1024 * 1024 {
            return None;
        }
        let bytes = fs::read(image_path).ok()?;
        let data_uri = format!(
            "data:{};base64,{}",
            content_type,
            general_purpose::STANDARD.encode(bytes)
        );
        Some(replace_html_attr(tag, "src", &data_uri))
    })
}

fn replace_html_tags<F>(html: &str, tag_name: &str, paired: bool, mut replace: F) -> String
where
    F: FnMut(&str, &str) -> Option<String>,
{
    let mut output = String::with_capacity(html.len());
    let mut cursor = 0;
    let lower = html.to_lowercase();
    let needle = format!("<{}", tag_name);
    let close_needle = format!("</{}>", tag_name);

    while let Some(relative_start) = lower[cursor..].find(&needle) {
        let start = cursor + relative_start;
        let after_name = start + needle.len();
        let next = lower.as_bytes().get(after_name).copied();
        if !matches!(
            next,
            Some(b' ') | Some(b'\t') | Some(b'\n') | Some(b'\r') | Some(b'>') | Some(b'/')
        ) {
            output.push_str(&html[cursor..after_name]);
            cursor = after_name;
            continue;
        }
        let Some(relative_tag_end) = lower[start..].find('>') else {
            break;
        };
        let tag_end = start + relative_tag_end + 1;
        let tag = &html[start..tag_end];

        if paired {
            let Some(relative_close) = lower[tag_end..].find(&close_needle) else {
                output.push_str(&html[cursor..tag_end]);
                cursor = tag_end;
                continue;
            };
            let close_start = tag_end + relative_close;
            let close_end = close_start + close_needle.len();
            let body = &html[tag_end..close_start];
            output.push_str(&html[cursor..start]);
            output.push_str(
                &replace(tag, body).unwrap_or_else(|| html[start..close_end].to_string()),
            );
            cursor = close_end;
        } else {
            output.push_str(&html[cursor..start]);
            output.push_str(&replace(tag, "").unwrap_or_else(|| tag.to_string()));
            cursor = tag_end;
        }
    }

    output.push_str(&html[cursor..]);
    output
}

fn resolve_preview_asset(workspace_root: &Path, base_dir: &Path, raw_url: &str) -> Option<PathBuf> {
    let value = raw_url.trim();
    let lower = value.to_lowercase();
    if value.is_empty()
        || value.starts_with('#')
        || lower.starts_with("http:")
        || lower.starts_with("https:")
        || lower.starts_with("data:")
        || lower.starts_with("blob:")
        || lower.starts_with("javascript:")
        || lower.starts_with("mailto:")
        || value.starts_with("//")
    {
        return None;
    }
    let path_part = value
        .split(['?', '#'])
        .next()
        .unwrap_or(value)
        .trim_start_matches("./");
    let candidate = normalize_path(&base_dir.join(path_part)).ok()?;
    if !candidate.starts_with(workspace_root) || !candidate.is_file() {
        return None;
    }
    Some(candidate)
}

fn html_attr(tag: &str, attr: &str) -> Option<String> {
    let lower = tag.to_lowercase();
    let needle = format!("{}=", attr.to_lowercase());
    let start = lower.find(&needle)? + needle.len();
    let bytes = tag.as_bytes();
    let quote = *bytes.get(start)?;
    if quote != b'\'' && quote != b'"' {
        return None;
    }
    let value_start = start + 1;
    let value_end = tag[value_start..].find(quote as char)? + value_start;
    Some(tag[value_start..value_end].to_string())
}

fn replace_html_attr(tag: &str, attr: &str, value: &str) -> String {
    let Some(old_value) = html_attr(tag, attr) else {
        return tag.to_string();
    };
    tag.replacen(
        &format!("{}=\"{}\"", attr, old_value),
        &format!("{}=\"{}\"", attr, escape_html_attr(value)),
        1,
    )
    .replacen(
        &format!("{}='{}'", attr, old_value),
        &format!("{}=\"{}\"", attr, escape_html_attr(value)),
        1,
    )
}

fn remove_html_attr(tag: &str, attr: &str) -> String {
    let Some(value) = html_attr(tag, attr) else {
        return tag.to_string();
    };
    tag.replacen(&format!(" {}=\"{}\"", attr, value), "", 1)
        .replacen(&format!(" {}='{}'", attr, value), "", 1)
}

fn escape_html_attr(value: &str) -> String {
    value
        .replace('&', "&amp;")
        .replace('"', "&quot;")
        .replace('<', "&lt;")
        .replace('>', "&gt;")
}

fn is_text_content_type(content_type: &str) -> bool {
    content_type.starts_with("text/")
        || matches!(
            content_type,
            "application/json" | "application/yaml" | "image/svg+xml"
        )
}

fn agent_plan_from_row(row: &rusqlite::Row<'_>) -> rusqlite::Result<AgentPlan> {
    Ok(AgentPlan {
        id: row.get(0)?,
        run_type: row.get(1)?,
        title: row.get(2)?,
        workspace_path: row.get(3)?,
        plan_path: row.get(4)?,
        status: row.get(5)?,
        worker_session_id: row.get(6)?,
        reviewer_session_id: row.get(7)?,
        worker_provider: row.get(8)?,
        reviewer_provider: row.get(9)?,
        current_phase_id: row.get(10)?,
        current_phase_index: row.get(11)?,
        error: row.get(12)?,
        brief: row.get(13)?,
        app_scope: row.get(14)?,
        docs_scope: row.get(15)?,
        reference_paths: row.get(16)?,
        amend_brief: row.get(17)?,
        phase_run_mode: row.get(18)?,
        created_at: row.get(19)?,
        updated_at: row.get(20)?,
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
        phase_index: None,
        phase_title: None,
        event_type: row.get(3)?,
        actor: "system".to_string(),
        category: "run".to_string(),
        summary: String::new(),
        status_before: None,
        status_after: None,
        reason: None,
        verdict: None,
        task_id: None,
        clarification_attempt: None,
        payload_json: row.get(4)?,
        created_at: row.get(5)?,
    })
}

fn enrich_agent_plan_event(
    event: &mut AgentPlanEvent,
    phase_meta: &HashMap<String, (i64, String)>,
) {
    if let Some(phase_id) = &event.phase_id {
        if let Some((phase_index, phase_title)) = phase_meta.get(phase_id) {
            event.phase_index = Some(*phase_index);
            event.phase_title = Some(phase_title.clone());
        }
    }

    let payload = serde_json::from_str::<serde_json::Value>(&event.payload_json)
        .unwrap_or_else(|_| json!({}));
    event.actor = event_actor(&event.event_type).to_string();
    event.category = event_category(&event.event_type).to_string();
    event.reason = event_reason(&event.event_type, &payload);
    event.verdict = payload_str(&payload, "verdict");
    event.task_id = payload_str(&payload, "taskId");
    event.clarification_attempt = payload_i64(&payload, "attempt");

    let (status_before, status_after) = event_status_transition(&event.event_type, &payload);
    event.status_before = status_before.map(str::to_string);
    event.status_after = status_after.map(str::to_string);
    event.summary = event_summary(event, &payload);
}

fn event_actor(event_type: &str) -> &'static str {
    match event_type {
        "planning_planner_ready" | "agent_phase_worker_idle" => "t1",
        "planning_gate_result" | "agent_phase_gate_result" => "t2",
        "agent_plan_created"
        | "planning_run_created"
        | "agent_plan_stopped"
        | "agent_plan_closed"
        | "agent_plan_blocked"
        | "planning_started"
        | "planning_amend_requested"
        | "agent_plan_phases_refreshed" => "user",
        _ => "coordinator",
    }
}

fn event_category(event_type: &str) -> &'static str {
    match event_type {
        "planning_started"
        | "planning_planner_ready"
        | "planning_review_started"
        | "planning_gate_result"
        | "planning_feedback_sent_to_planner"
        | "planning_verdict_clarification_requested"
        | "planning_needs_attention" => "planning",
        "planning_amend_requested" => "amend",
        "agent_phase_started"
        | "agent_phase_worker_idle"
        | "agent_phase_review_started"
        | "agent_phase_gate_result"
        | "agent_feedback_sent_to_worker"
        | "agent_phase_verdict_clarification_requested"
        | "agent_phase_needs_attention"
        | "agent_phase_unlocked"
        | "agent_single_phase_completed" => "phase",
        _ => "run",
    }
}

fn event_status_transition(
    event_type: &str,
    payload: &serde_json::Value,
) -> (Option<&'static str>, Option<&'static str>) {
    match event_type {
        "planning_started" => (None, Some("planning_planner_running")),
        "planning_planner_ready" => (Some("planning_planner_running"), Some("planning_review_running")),
        "planning_review_started" => (Some("planning_planner_running"), Some("planning_review_running")),
        "planning_gate_result" => match payload_str(payload, "verdict").as_deref() {
            Some("PASS") => (Some("planning_review_running"), Some("approved")),
            Some("NEEDS_CHANGES") | Some("BLOCKED") => {
                (Some("planning_review_running"), Some("planning_planner_running"))
            }
            _ => (Some("planning_review_running"), None),
        },
        "agent_phase_started" => (None, Some("phase_worker_running")),
        "agent_phase_worker_idle" => (Some("phase_worker_running"), Some("phase_review_running")),
        "agent_phase_review_started" => (Some("phase_worker_running"), Some("phase_review_running")),
        "agent_phase_gate_result" => match payload_str(payload, "verdict").as_deref() {
            Some("PASS") => (Some("phase_review_running"), Some("passed")),
            Some("NEEDS_CHANGES") | Some("BLOCKED") => {
                (Some("phase_review_running"), Some("needs_changes"))
            }
            _ => (Some("phase_review_running"), None),
        },
        "agent_feedback_sent_to_worker" => (Some("phase_review_running"), Some("phase_worker_running")),
        "agent_plan_stopped" => (None, Some("stopped")),
        "agent_plan_closed" => (None, Some("closed")),
        "agent_plan_completed" | "agent_single_phase_completed" => (None, Some("approved")),
        "agent_plan_blocked" => (None, Some("blocked")),
        _ => (None, None),
    }
}

fn event_summary(event: &AgentPlanEvent, payload: &serde_json::Value) -> String {
    match event.event_type.as_str() {
        "agent_plan_created" => "Created run".to_string(),
        "agent_plan_renamed" => match (payload_str(payload, "oldTitle"), payload_str(payload, "newTitle")) {
            (Some(old_title), Some(new_title)) => format!("Renamed run from '{}' to '{}'", old_title, new_title),
            (None, Some(new_title)) => format!("Renamed run to '{}'", new_title),
            _ => "Renamed run".to_string(),
        },
        "planning_run_created" => "Created planning run".to_string(),
        "agent_plan_stopped" => "Stopped run".to_string(),
        "agent_plan_closed" => "Closed run".to_string(),
        "agent_plan_blocked" => match payload_str(payload, "reason") {
            Some(reason) => format!("Blocked run: {}", reason),
            None => "Blocked run".to_string(),
        },
        "planning_started" => "Started planner pass".to_string(),
        "planning_amend_requested" => "Requested plan amendment".to_string(),
        "planning_planner_ready" => "Planner marked the plan ready for review".to_string(),
        "planning_review_started" => "Started planning review".to_string(),
        "planning_feedback_sent_to_planner" => match event_reason(&event.event_type, payload) {
            Some(reason) => format!("Sent T2 feedback back to the planner ({})", reason),
            None => "Sent T2 feedback back to the planner".to_string(),
        },
        "planning_verdict_clarification_requested" => match payload_i64(payload, "attempt") {
            Some(attempt) => format!("Requested planning verdict clarification (attempt {})", attempt),
            None => "Requested planning verdict clarification".to_string(),
        },
        "planning_needs_attention" => "Planning run needs manual attention".to_string(),
        "planning_gate_result" => match payload_str(payload, "verdict").as_deref() {
            Some("PASS") => "Planning review passed".to_string(),
            Some("NEEDS_CHANGES") => match event_reason(&event.event_type, payload) {
                Some(reason) => format!("Planning review requested changes ({})", reason),
                None => "Planning review requested changes".to_string(),
            },
            Some("BLOCKED") => match event_reason(&event.event_type, payload) {
                Some(reason) => format!("Planning review is blocked ({})", reason),
                None => "Planning review is blocked".to_string(),
            },
            _ => "Planning review returned a verdict".to_string(),
        },
        "agent_phase_started" => "Started phase work".to_string(),
        "agent_phase_worker_idle" => "T1 finished the current pass".to_string(),
        "agent_phase_review_started" => "Started T2 review".to_string(),
        "agent_feedback_sent_to_worker" => match event_reason(&event.event_type, payload) {
            Some(reason) => format!("Sent T2 feedback back to T1 ({})", reason),
            None => "Sent T2 feedback back to T1".to_string(),
        },
        "agent_phase_verdict_clarification_requested" => match payload_i64(payload, "attempt") {
            Some(attempt) => format!("Requested phase verdict clarification (attempt {})", attempt),
            None => "Requested phase verdict clarification".to_string(),
        },
        "agent_phase_needs_attention" => "Phase needs manual attention".to_string(),
        "agent_phase_gate_result" => match payload_str(payload, "verdict").as_deref() {
            Some("PASS") => "Phase review passed".to_string(),
            Some("NEEDS_CHANGES") => match event_reason(&event.event_type, payload) {
                Some(reason) => format!("Phase review requested changes ({})", reason),
                None => "Phase review requested changes".to_string(),
            },
            Some("BLOCKED") => match event_reason(&event.event_type, payload) {
                Some(reason) => format!("Phase review is blocked ({})", reason),
                None => "Phase review is blocked".to_string(),
            },
            _ => "Phase review returned a verdict".to_string(),
        },
        "agent_phase_unlocked" => "Unlocked the next phase".to_string(),
        "agent_plan_completed" => "Completed the full run".to_string(),
        "agent_single_phase_completed" => "Completed the selected phase only".to_string(),
        "agent_plan_phases_refreshed" => "Refreshed phases from plan files".to_string(),
        _ => humanize_event_type(&event.event_type),
    }
}

fn payload_str(payload: &serde_json::Value, key: &str) -> Option<String> {
    payload
        .get(key)
        .and_then(|value| value.as_str())
        .map(str::trim)
        .filter(|value| !value.is_empty())
        .map(str::to_string)
}

fn payload_i64(payload: &serde_json::Value, key: &str) -> Option<i64> {
    payload.get(key).and_then(|value| value.as_i64())
}

fn event_reason(event_type: &str, payload: &serde_json::Value) -> Option<String> {
    if let Some(reason) = payload_str(payload, "reason") {
        return Some(humanize_reason_value(event_type, &reason));
    }
    if let Some(source) = payload_str(payload, "source") {
        return Some(humanize_reason_value(event_type, &source));
    }
    None
}

fn humanize_reason_value(event_type: &str, value: &str) -> String {
    if value.contains(' ') || value.contains('.') || value.contains(',') || value.contains(':') {
        return value.trim().to_string();
    }
    humanize_reason_code(event_type, value)
}

fn humanize_reason_code(event_type: &str, code: &str) -> String {
    match (event_type, code) {
        ("planning_feedback_sent_to_planner", "auto_needs_changes")
        | ("agent_feedback_sent_to_worker", "auto_needs_changes") => {
            "automatic after T2 requested changes".to_string()
        }
        (_, "unknown_verdict") => "T2 did not return a parseable verdict".to_string(),
        _ => humanize_code(code),
    }
}

fn humanize_code(code: &str) -> String {
    code.replace('_', " ").to_lowercase()
}

fn humanize_event_type(event_type: &str) -> String {
    let mut out = String::with_capacity(event_type.len() + 8);
    let mut uppercase_next = true;
    for ch in event_type.chars() {
        if ch == '_' {
            out.push(' ');
            uppercase_next = true;
            continue;
        }
        if uppercase_next {
            out.extend(ch.to_uppercase());
            uppercase_next = false;
        } else {
            out.push(ch);
        }
    }
    out
}
