use crate::db::migrations::health_from_status;
use crate::db::models::{
    AgentPlan, AgentPlanEvent, AgentPlanPhase, AgentPlanTask, CreateAgentPlanInput,
    CreateSessionInput,
};
use crate::events::AgentPlanRunEvent;
use crate::services::planner_prompts;
use crate::services::sessions;
use crate::services::settings as settings_service;
use crate::state::app_state::{AgentReport, AppState};
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

const TERMINAL_STARTUP_WAIT_MS: u64 = 2_500;
/// Still referenced in planning prompt text that asks the planner to narrate
/// readiness; the coordinator no longer scrapes it (completion comes via the
/// `reportAgentResult` API).
const PLANNER_READY_MARKER: &str = "READY_FOR_T2_PLAN_REVIEW";
const CLARIFICATION_LIMIT: i64 = 5;
/// How many consecutive non-PASS review rounds (T2 returns NEEDS_CHANGES/BLOCKED →
/// back to T1 → re-review …) before the coordinator gives up and escalates to
/// `needs_attention` instead of looping forever. Without this a review that never
/// converges churns indefinitely (the Marketplace run hit 116 rounds over 4 days).
const MAX_REVISION_ROUNDS: i64 = 6;
/// How long the agent's normalized screen must sit unchanged *without* a structured
/// report before the coordinator re-requests it. The agent has clearly stopped
/// working (Grok's volatile chrome is filtered out of the idle key) but never ran
/// the report command.
const READY_NUDGE_IDLE_MS: u64 = 20_000;
/// How many times to re-request a report before escalating to needs_attention,
/// so a run can never hang forever waiting on a report that won't arrive.
const MAX_READY_NUDGES: u32 = 5;

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

/// The initiative_id of the planning stage-run that owns this plan path, if any.
/// Shared join key between a planning stage-run and its development stage-run (D3).
/// Extracted so the linkage query is regression-covered directly (task 02-04 test 4).
fn find_planning_initiative_id(conn: &rusqlite::Connection, plan_path: &str) -> Option<String> {
    conn.query_row(
        "SELECT initiative_id FROM agent_plans \
         WHERE plan_path = ?1 AND run_type = 'planning' \
         ORDER BY created_at ASC LIMIT 1",
        params![plan_path],
        |row| row.get::<_, String>(0),
    )
    .ok()
}

/// Set the execution `status` AND derive the `health` axis in one place so the two cannot
/// drift. `error` is set alongside (`None` clears it). Returns rows affected. Health is
/// derived via `health_from_status` so removing the pairing fails task 02-04 test 5.
fn update_plan_status_and_health(
    conn: &rusqlite::Connection,
    plan_id: &str,
    new_status: &str,
    error: Option<&str>,
) -> rusqlite::Result<usize> {
    conn.execute(
        "UPDATE agent_plans SET status = ?1, health = ?2, error = ?3, updated_at = datetime('now') WHERE id = ?4",
        params![new_status, health_from_status(new_status), error, plan_id],
    )
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
    // Link this development stage-run to the planning stage-run that owns the same plan_path
    // (now the global store path). Reuse its initiative_id; mint a fresh one if none exists (D3).
    let initiative_id = state
        .db
        .with_conn(|conn| Ok(find_planning_initiative_id(conn, &parsed.plan_path.to_string_lossy())))?
        .unwrap_or_else(|| plan_id.clone());
    let title = input.title.unwrap_or_else(|| parsed.title.clone());
    // Explicit app-repo path (where the docs-commit agent commits on completion).
    let app_scope = normalize_optional_workspace_path(
        &parsed.workspace_path,
        input.app_scope.as_deref(),
    )?;

    let worker_session = sessions::create_session(
        state,
        CreateSessionInput {
            provider: Some(input.worker_provider.clone()),
            model: default_model_for_provider(&input.worker_provider),
            working_directory: Some(parsed.workspace_path.to_string_lossy().to_string()),
            title: Some(format!("T1 Worker - {}", title)),
            kind: Some("agent".to_string()),
            setup_commands: input.worker_setup_commands.clone(),
            tmux_session_name: None,
        },
    )?;
    // No persistent T2 reviewer session: the 3-lens fan-out spawns ephemeral
    // reviewers per round, so a standing reviewer session would just sit idle.

    state.db.with_conn(|conn| {
        conn.execute(
            // A development run reuses its planning run's initiative_id (linked by shared store
            // plan_path); initiative_status='development', health seeded 'in-progress'.
            "INSERT INTO agent_plans (id, run_type, title, workspace_path, plan_path, status, worker_session_id, reviewer_session_id, worker_provider, reviewer_provider, current_phase_id, current_phase_index, app_scope, reviewer_setup_commands, initiative_id, initiative_status, health)
             VALUES (?1, 'development', ?2, ?3, ?4, 'draft', ?5, NULL, ?6, ?7, ?8, 0, ?9, ?10, ?11, 'development', 'in-progress')",
            params![
                plan_id,
                title,
                parsed.workspace_path.to_string_lossy(),
                parsed.plan_path.to_string_lossy(),
                worker_session.id,
                input.worker_provider,
                input.reviewer_provider,
                parsed.phases.first().map(|phase| phase.phase_id.as_str()),
                app_scope,
                input.reviewer_setup_commands.as_deref().filter(|s| !s.trim().is_empty()),
                initiative_id,
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
        json!({ "workerSessionId": worker_session.id, "reviewerSessionId": serde_json::Value::Null }),
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
    // The inbound plan_path (the ui's `docs/plans/<slug>`) is now advisory only — the plan
    // actually lives in the global initiatives store, outside every repo (D4). Still `..`-guard
    // it defensively (same message/logic as parse_plan) so no user-influenced component can
    // traverse, even though the derived store path is UUID-based and cannot contain `..`.
    if input.plan_path.split(['/', '\\']).any(|seg| seg == "..") {
        return Err("Plan path must not contain '..'".to_string());
    }
    // A planning stage-run's id IS its initiative id for a fresh initiative (D3).
    let initiative_id = Uuid::new_v4().to_string();
    let plan_id = initiative_id.clone();
    // Derive `<initiatives_dir>/<initiative_id>/plan` and create it. The store lives outside
    // every repo by design, so there is deliberately no in-workspace check on this path.
    let store = settings_service::resolve_initiatives_dir(state);
    let plan = settings_service::initiative_plan_path(&store, &initiative_id);
    std::fs::create_dir_all(&plan)
        .map_err(|e| format!("Failed to create initiative plan dir {}: {}", plan.display(), e))?;
    let app_scope = normalize_optional_workspace_path(&workspace, input.app_scope.as_deref())?;
    let docs_scope = normalize_optional_workspace_path(&workspace, input.docs_scope.as_deref())?;
    let reference_paths = normalize_reference_paths(&workspace, input.reference_paths.as_deref())?;
    let title = input.title.clone().unwrap_or_else(|| {
        // The advisory input path's last segment makes a nicer default than the store's "plan".
        Path::new(&input.plan_path)
            .file_name()
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
            setup_commands: input.worker_setup_commands.clone(),
            tmux_session_name: None,
        },
    )?;
    // No persistent T2 reviewer session — the 3-lens fan-out spawns ephemeral
    // reviewers per round, so a standing reviewer session would just sit idle.

    state.db.with_conn(|conn| {
        conn.execute(
            // Fresh initiative: id == initiative_id (?1); plan_path (?4) is the store path;
            // initiative_status='planning', health seeded 'in-progress'.
            "INSERT INTO agent_plans (id, run_type, title, workspace_path, plan_path, status, worker_session_id, reviewer_session_id, worker_provider, reviewer_provider, current_phase_index, brief, app_scope, docs_scope, reference_paths, reviewer_setup_commands, initiative_id, initiative_status, health)
             VALUES (?1, 'planning', ?2, ?3, ?4, 'draft', ?5, NULL, ?6, ?7, 0, ?8, ?9, ?10, ?11, ?12, ?1, 'planning', 'in-progress')",
            params![
                plan_id,
                title,
                workspace.to_string_lossy(),
                plan.to_string_lossy(),
                worker_session.id,
                input.worker_provider,
                input.reviewer_provider,
                input.brief.unwrap_or_default(),
                app_scope,
                docs_scope,
                reference_paths,
                input.reviewer_setup_commands.as_deref().filter(|s| !s.trim().is_empty()),
            ],
        )
        .map_err(|e| format!("Failed to create planning run: {}", e))
    })?;

    append_event(
        state,
        &plan_id,
        None,
        "planning_run_created",
        json!({ "plannerSessionId": worker_session.id, "reviewerSessionId": serde_json::Value::Null }),
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
        let sql = "SELECT id, run_type, title, workspace_path, plan_path, status, worker_session_id, reviewer_session_id, worker_provider, reviewer_provider, current_phase_id, current_phase_index, error, brief, app_scope, docs_scope, reference_paths, amend_brief, phase_run_mode, initiative_id, initiative_status, health, created_at, updated_at FROM agent_plans
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
            "SELECT id, run_type, title, workspace_path, plan_path, status, worker_session_id, reviewer_session_id, worker_provider, reviewer_provider, current_phase_id, current_phase_index, error, brief, app_scope, docs_scope, reference_paths, amend_brief, phase_run_mode, initiative_id, initiative_status, health, created_at, updated_at FROM agent_plans WHERE id = ?1",
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

    terminal::attach_terminal_headless(&state, worker_session_id.clone(), 120, 36).await?;
    // Reviewer session is optional now (fan-out uses ephemeral reviewers); only
    // attach if a legacy run still has one.
    if let Some(reviewer_session_id) = run.plan.reviewer_session_id.clone() {
        terminal::attach_terminal_headless(&state, reviewer_session_id, 120, 36).await?;
    }
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

    let prompt = worker_phase_prompt(&state, &run, &phase)?;
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
    // Tear down any in-flight ephemeral lens/docs agents too (previously leaked).
    dispose_plan_review_sessions(state, &id).await;
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

/// Set/clear the app-repo path (`app_scope`) on an existing run — so a user can add
/// or fix the path the docs-commit agent uses, on an in-flight run, anytime before
/// completion. An empty value clears it (docs commit then skips).
pub fn update_plan_app_scope(
    state: &AppState,
    id: String,
    app_scope: Option<String>,
) -> Result<AgentPlanRun, String> {
    let current = get_plan(state, &id)?;
    let normalized = normalize_optional_workspace_path(
        Path::new(&current.plan.workspace_path),
        app_scope.as_deref(),
    )?;
    state.db.with_conn(|conn| {
        conn.execute(
            "UPDATE agent_plans SET app_scope = ?1, updated_at = datetime('now') WHERE id = ?2",
            params![normalized, id],
        )
        .map_err(|e| e.to_string())
    })?;
    append_event(
        state,
        &id,
        None,
        "agent_plan_app_scope_updated",
        json!({ "appScope": normalized }),
    )?;
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
    // Also tear down any ephemeral lens/docs agents for this plan (previously leaked).
    dispose_plan_review_sessions(state, &id).await;

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
        update_plan_status_and_health(conn, &id, "blocked", Some(&reason)).map_err(|e| e.to_string())
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
        let prompt = planning_continue_prompt(&run);
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
        if run.plan.status == "planning_planner_running" {
            let planner_session_id = run
                .plan
                .worker_session_id
                .clone()
                .ok_or_else(|| "Planning run has no planner session".to_string())?;
            let prompt = planning_continue_prompt(&run);
            terminal::attach_terminal_headless(&state, planner_session_id.clone(), 120, 36).await?;
            terminal::send_terminal_input(&state, planner_session_id, format!("{}\r", prompt)).await?;
            append_event(
                &state,
                &id,
                None,
                "planning_start_nudge",
                json!({ "source": "start" }),
            )?;
        } else {
            dispatch_planning_review(&state, &run).await?;
            append_event(
                &state,
                &id,
                None,
                "planning_review_restart",
                json!({ "source": "start" }),
            )?;
        }
        spawn_coordinator_loop(state.clone(), id.clone()).await;
        return get_plan(&state, &id);
    }
    let planner_session_id = run
        .plan
        .worker_session_id
        .clone()
        .ok_or_else(|| "Planning run has no planner session".to_string())?;

    terminal::attach_terminal_headless(&state, planner_session_id.clone(), 120, 36).await?;
    // Reviewer session is optional now (fan-out uses ephemeral reviewers); only
    // attach if a legacy run still has one.
    if let Some(reviewer_session_id) = run.plan.reviewer_session_id.clone() {
        terminal::attach_terminal_headless(&state, reviewer_session_id, 120, 36).await?;
    }
    sleep(Duration::from_millis(TERMINAL_STARTUP_WAIT_MS)).await;

    state.db.with_conn(|conn| {
        conn.execute(
            "UPDATE agent_plans SET status = 'planning_planner_running', updated_at = datetime('now') WHERE id = ?1",
            params![id],
        )
        .map_err(|e| e.to_string())
    })?;
    append_event(&state, &id, None, "planning_started", json!({}))?;
    let prompt = planning_planner_prompt(&state, &run)?;
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

fn home_directory() -> Result<PathBuf, String> {
    dirs::home_dir().ok_or_else(|| "Could not determine home directory".to_string())
}

/// Resolve a user-supplied browse path into an existing directory.
///
/// Empty input, `~`, or `~/...` resolve against the host's home directory.
/// Anything that does not resolve to an existing directory falls back to home,
/// so the picker always lands somewhere navigable instead of erroring. This
/// keeps the directory browser host-agnostic — no machine-specific defaults.
fn resolve_browse_dir(input: &str) -> Result<PathBuf, String> {
    let trimmed = input.trim();
    if trimmed.is_empty() || trimmed == "~" {
        return home_directory();
    }
    let candidate = if let Some(rest) = trimmed.strip_prefix("~/") {
        home_directory()?.join(rest)
    } else {
        PathBuf::from(trimmed)
    };
    match normalize_path(&candidate) {
        Ok(resolved) if resolved.is_dir() => Ok(resolved),
        _ => home_directory(),
    }
}

pub fn browse_host_directory(path: String) -> Result<Vec<HostFileEntry>, String> {
    let base = resolve_browse_dir(&path)?;
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
                // Kept inline (not via update_plan_status_and_health) to preserve the
                // `status NOT IN (...)` guard; health is still derived via health_from_status so
                // the status+health pairing can't drift (documented in decisions.md D-inline).
                conn.execute(
                    "UPDATE agent_plans SET status = 'needs_attention', health = ?1, error = ?2, updated_at = datetime('now') WHERE id = ?3 AND status NOT IN ('approved', 'blocked', 'stopped')",
                    params![health_from_status("needs_attention"), error, plan_id],
                )
                .map_err(|e| e.to_string())
            });
            // Emit an event so the failure shows in the feed AND fires the Discord
            // alert (append_event is the single notification chokepoint).
            let _ = append_event(
                &state,
                &plan_id,
                None,
                "coordinator_failed",
                json!({ "reason": error }),
            );
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
                    wait_for_planner_ready(&state, &session_id, &plan_id).await?;
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
                    run_planning_lens_fanout_review(&state, &run).await?;
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
                wait_for_worker_ready(&state, &session_id, &plan_id, Some(&phase.phase_id)).await?;
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
                // Development review fans out into three ephemeral lens reviewers
                // (Product/QA/Lead) run in parallel; their verdicts are merged.
                let phase = current_phase(&run)?;
                run_lens_fanout_review(&state, &run, &phase).await?;
            }
            "approved" | "blocked" | "stopped" | "needs_attention" => break,
            _ => break,
        }
        sleep(Duration::from_millis(300)).await;
    }
    Ok(())
}

/// Record a structured agent report received via the host GraphQL
/// `reportAgentResult` mutation. Verifies the session belongs to a plan and the
/// report kind matches the session's role (only the worker reports `ready`, only
/// the reviewer reports a `verdict`) — an unknown session id is rejected, which is
/// the spoof guard (the id is an unguessable UUID a browser/remote caller can't
/// learn). The coordinator's wait loops drain `state.agent_reports`.
#[allow(clippy::too_many_arguments)]
pub async fn record_agent_report(
    state: &AppState,
    session_id: String,
    kind: String,
    verdict: Option<String>,
    findings: Option<String>,
    summary: Option<String>,
    severity: Option<String>,
    reason: Option<String>,
    evidence: Option<String>,
) -> Result<(), String> {
    let kind_norm = kind.trim().to_ascii_lowercase();
    // Identity is derived from the session, never self-declared (spoof guard).
    let role = plan_role_for_session(state, &session_id)?;
    let verdict_norm = match kind_norm.as_str() {
        "ready" => {
            if role != "worker" {
                return Err("'ready' may only be reported by the worker/planner session".to_string());
            }
            None
        }
        "verdict" => {
            if !matches!(role.as_str(), "reviewer" | "product" | "qa" | "lead") {
                return Err(
                    "'verdict' may only be reported by the reviewer or a lens session".to_string(),
                );
            }
            let raw = verdict.ok_or_else(|| "verdict kind requires a verdict value".to_string())?;
            Some(
                normalize_verdict_token(&raw)
                    .ok_or_else(|| format!("invalid verdict value: {}", raw))?,
            )
        }
        // An ephemeral agent (e.g. the docs agent) signalling its task is complete.
        "done" => None,
        // The agent is stuck and needs a human decision — any session may signal it.
        "blocked" => None,
        // Progress / status — any session may push these.
        "update" => None,
        other => return Err(format!("unknown report kind: {}", other)),
    };
    state.agent_reports.lock().await.insert(
        session_id.clone(),
        AgentReport {
            kind: kind_norm.clone(),
            verdict: verdict_norm,
            role: Some(role.clone()),
            findings,
            summary,
            severity: severity.map(|s| s.trim().to_ascii_lowercase()),
            reason,
            evidence,
        },
    );
    tracing::info!(session_id, role, kind = %kind_norm, "recorded structured agent report");
    Ok(())
}

/// Resolve a session's role: an ephemeral review/agent session's registered role
/// (product/qa/lead/docs/…) if present, otherwise the plan's worker/reviewer.
/// Errors if the id belongs to no plan — this is the report spoof guard.
fn plan_role_for_session(state: &AppState, session_id: &str) -> Result<String, String> {
    let registered: Option<String> = state.db.with_conn(|conn| {
        conn.query_row(
            "SELECT role FROM agent_plan_review_sessions WHERE session_id = ?1",
            params![session_id],
            |row| row.get(0),
        )
        .optional()
        .map_err(|e| e.to_string())
    })?;
    if let Some(role) = registered {
        return Ok(role);
    }
    let role: Option<String> = state.db.with_conn(|conn| {
        conn.query_row(
            "SELECT CASE WHEN worker_session_id = ?1 THEN 'worker' ELSE 'reviewer' END \
             FROM agent_plans WHERE worker_session_id = ?1 OR reviewer_session_id = ?1 LIMIT 1",
            params![session_id],
            |row| row.get(0),
        )
        .optional()
        .map_err(|e| e.to_string())
    })?;
    role.ok_or_else(|| "session id does not belong to any plan".to_string())
}

/// Register an ephemeral agent session (lens reviewer, docs agent, …) so its
/// reports are trusted and attributed to `role`.
fn register_review_session(
    state: &AppState,
    plan_id: &str,
    session_id: &str,
    role: &str,
) -> Result<(), String> {
    state.db.with_conn(|conn| {
        conn.execute(
            "INSERT OR REPLACE INTO agent_plan_review_sessions (session_id, plan_id, role) VALUES (?1, ?2, ?3)",
            params![session_id, plan_id, role],
        )
        .map_err(|e| e.to_string())
    })?;
    Ok(())
}

fn unregister_review_session(state: &AppState, session_id: &str) {
    let _ = state.db.with_conn(|conn| {
        conn.execute(
            "DELETE FROM agent_plan_review_sessions WHERE session_id = ?1",
            params![session_id],
        )
        .map_err(|e| e.to_string())
    });
}

/// Take a report only if it is of the given kind, leaving other kinds (e.g. a
/// pending `update`) in place so a ready/verdict wait doesn't swallow them.
async fn take_agent_report_kind(
    state: &AppState,
    session_id: &str,
    kind: &str,
) -> Option<AgentReport> {
    let mut map = state.agent_reports.lock().await;
    match map.get(session_id) {
        Some(report) if report.kind == kind => map.remove(session_id),
        _ => None,
    }
}

async fn clear_agent_report(state: &AppState, session_id: &str) {
    state.agent_reports.lock().await.remove(session_id);
}

/// Spawn an ephemeral agent (lens reviewer, docs agent, …) registered to a plan,
/// attach it headless, and send its task prompt. `prompt_for` receives the new
/// session id so the prompt can bake the session-specific report `curl`. Returns
/// the session id; dispose it with `dispose_ephemeral_agent` once its report is in.
async fn spawn_ephemeral_agent(
    state: &AppState,
    plan_id: &str,
    role: &str,
    provider: &str,
    model: Option<String>,
    working_directory: Option<String>,
    prompt_for: impl FnOnce(&str) -> String,
) -> Result<String, String> {
    let short = &plan_id[..plan_id.len().min(8)];
    // For a shell reviewer, run the plan's reviewer setup commands in the pane.
    let setup_commands = if provider == "shell" {
        state
            .db
            .with_conn(|conn| {
                Ok(conn
                    .query_row(
                        "SELECT reviewer_setup_commands FROM agent_plans WHERE id = ?1",
                        params![plan_id],
                        |row| row.get::<_, Option<String>>(0),
                    )
                    .ok()
                    .flatten())
            })
            .unwrap_or(None)
    } else {
        None
    };
    let session = sessions::create_session(
        state,
        CreateSessionInput {
            provider: Some(provider.to_string()),
            model,
            working_directory,
            title: Some(format!("{} · {}", role.to_uppercase(), short)),
            kind: Some("agent".to_string()),
            setup_commands,
            tmux_session_name: None,
        },
    )?;
    let session_id = session.id.clone();
    register_review_session(state, plan_id, &session_id, role)?;
    terminal::attach_terminal_headless(state, session_id.clone(), 120, 36).await?;
    sleep(Duration::from_millis(TERMINAL_STARTUP_WAIT_MS)).await;
    let prompt = prompt_for(&session_id);
    terminal::send_terminal_input(state, session_id.clone(), format!("{}\r", prompt)).await?;
    Ok(session_id)
}

/// Tear down an ephemeral agent: unregister, drop any pending report, kill its
/// terminal, and delete the session.
async fn dispose_ephemeral_agent(state: &AppState, session_id: &str) {
    unregister_review_session(state, session_id);
    state.agent_reports.lock().await.remove(session_id);
    let _ = terminal::kill_terminal_session(state, session_id).await;
    let _ = sessions::delete_session(state, session_id.to_string()).await;
}

/// Dispose EVERY ephemeral review/docs agent registered for a plan (the 3 lens
/// reviewers, the docs agent). Called on stop/delete so those transient sessions
/// never outlive the plan — they were leaking before (only T1/T2 were killed).
async fn dispose_plan_review_sessions(state: &AppState, plan_id: &str) {
    let session_ids: Vec<String> = state
        .db
        .with_conn(|conn| {
            let mut stmt = conn
                .prepare("SELECT session_id FROM agent_plan_review_sessions WHERE plan_id = ?1")
                .map_err(|e| e.to_string())?;
            let ids = stmt
                .query_map(params![plan_id], |row| row.get::<_, String>(0))
                .map_err(|e| e.to_string())?
                .filter_map(|r| r.ok())
                .collect::<Vec<_>>();
            Ok(ids)
        })
        .unwrap_or_default();
    for session_id in session_ids {
        dispose_ephemeral_agent(state, &session_id).await;
    }
}

/// Discord embed color by severity.
fn discord_color(severity: &str) -> u32 {
    match severity {
        "attention" => 16_478_597, // red
        "warn" => 16_705_372,      // amber
        "success" => 5_763_719,    // green
        _ => 3_447_003,            // blue (info)
    }
}

/// Post a notification to the configured Discord webhook (best-effort, no-op if
/// unset). Used for both attention alerts and per-phase progress updates: the
/// `headline` carries its own emoji, `severity` picks the color. Always includes a
/// deep link to the run.
async fn notify_discord(
    state: &AppState,
    plan_id: &str,
    severity: &str,
    headline: &str,
    reason: &str,
) {
    let webhook =
        settings_service::get_setting_or(state, settings_service::KEY_DISCORD_WEBHOOK_URL, "");
    let webhook = webhook.trim();
    if webhook.is_empty() {
        return;
    }
    let (title, run_type) = state
        .db
        .with_conn(|conn| {
            conn.query_row(
                "SELECT title, run_type FROM agent_plans WHERE id = ?1",
                params![plan_id],
                |row| Ok((row.get::<_, String>(0)?, row.get::<_, String>(1)?)),
            )
            .map_err(|e| e.to_string())
        })
        .unwrap_or_else(|_| ("(plan)".to_string(), "development".to_string()));
    let mode = if run_type == "planning" {
        "planning"
    } else {
        "development"
    };
    let base = settings_service::get_setting_or(
        state,
        settings_service::KEY_WEB_CLIENT_URL,
        settings_service::DEFAULT_WEB_CLIENT_URL,
    );
    let link = format!("{}/{}/{}", base.trim_end_matches('/'), mode, plan_id);
    let description = if reason.is_empty() {
        format!("[Open the run →]({})", link)
    } else {
        format!("{}\n[Open the run →]({})", reason, link)
    };
    let payload = json!({
        "username": "JohnnyOne",
        "embeds": [{
            "title": headline,
            "description": description,
            "color": discord_color(severity),
            "fields": [
                { "name": "Plan", "value": title, "inline": true },
                { "name": "Mode", "value": mode, "inline": true },
            ],
        }]
    });
    let client = reqwest::Client::new();
    if let Err(error) = client.post(webhook).json(&payload).send().await {
        tracing::warn!(%error, plan_id, "failed to POST Discord notification");
    }
}

/// Map a plan event to a Discord notification, or `None` to skip (most events stay
/// in-app only). Returns (severity, headline-with-emoji, reason).
fn discord_message_for(
    event_type: &str,
    phase_id: Option<&str>,
    payload: &serde_json::Value,
) -> Option<(&'static str, String, String)> {
    let phase = phase_id.unwrap_or("");
    let str_field = |k: &str| {
        payload
            .get(k)
            .and_then(|v| v.as_str())
            .unwrap_or("")
            .to_string()
    };
    let verdict = str_field("verdict");
    let reason = str_field("reason");
    let summary = str_field("summary");
    let or_default = |s: String, d: &str| if s.is_empty() { d.to_string() } else { s };
    // Deliberately quiet: Discord fires ONLY on (a) a block / needs-human and
    // (b) a phase or plan being DONE (a PASS). We do NOT alert on starts, or on
    // every T2 result — a non-PASS review just loops back to T1 silently and is
    // visible in the web lens panel.
    match event_type {
        // Phase done.
        "agent_phase_gate_result" if verdict == "PASS" => Some((
            "success",
            format!("✅ Phase {} passed review", phase),
            summary,
        )),
        // Plan done (all dev phases passed).
        "agent_plan_completed" => Some((
            "success",
            "🎉 Plan approved — all phases passed".to_string(),
            String::new(),
        )),
        // Plan done (planning approved).
        "planning_gate_result" if verdict == "PASS" => Some((
            "success",
            "✅ Plan passed review".to_string(),
            summary,
        )),
        // Block / needs-human / failure.
        "agent_phase_needs_attention" => Some((
            "attention",
            format!("🔴 Phase {} needs attention", phase),
            or_default(reason, "unresolved"),
        )),
        "agent_blocked" => Some((
            "attention",
            "🔴 Blocked — needs your decision".to_string(),
            "Open the run and reply to unblock the agent.".to_string(),
        )),
        "planning_needs_attention" => Some((
            "attention",
            "🔴 Plan needs attention".to_string(),
            or_default(reason, "unresolved"),
        )),
        "coordinator_failed" => Some((
            "attention",
            "🔴 Run needs attention".to_string(),
            or_default(reason, "coordinator error"),
        )),
        "agent_docs_commit_failed" => Some((
            "attention",
            "🔴 Docs commit failed".to_string(),
            or_default(reason, "the docs-commit agent did not finish"),
        )),
        _ => None,
    }
}

/// Fire-and-forget a Discord notification for notifiable plan events.
fn maybe_notify_discord(
    state: &AppState,
    plan_id: &str,
    phase_id: Option<&str>,
    event_type: &str,
    payload: &serde_json::Value,
) {
    if let Some((severity, headline, reason)) = discord_message_for(event_type, phase_id, payload) {
        let state = state.clone();
        let plan_id = plan_id.to_string();
        tokio::spawn(async move {
            notify_discord(&state, &plan_id, severity, &headline, &reason).await;
        });
    }
}

/// The development-review lenses: (registration role, display name).
const REVIEW_LENSES: [(&str, &str); 3] =
    [("product", "Product"), ("qa", "QA"), ("lead", "Lead")];

/// Self-contained prompt for one ephemeral lens reviewer. Bakes the verdict report
/// `curl` for `session_id`; the agent runs only its lens.
/// Per-session file the lens writes its GraphQL request body into, then POSTs with
/// `curl -d @file` — keeps the agent's free-text out of the shell (no quoting bugs)
/// while still going through the structured API, not the terminal.
fn review_body_path(session_id: &str) -> std::path::PathBuf {
    std::env::temp_dir()
        .join("johnnyone-reviews")
        .join(format!("{session_id}.json"))
}

/// Instruction telling a lens to report its verdict AND reasons through the
/// `reportAgentResult` API (summary + findings), never by printing to the screen.
/// Uses GraphQL variables in a body file so the agent only edits simple JSON values.
fn lens_report_instruction(session_id: &str, lens_name: &str) -> String {
    const TEMPLATE: &str = r#"

---
Report your verdict AND your reasons through the coordinator API — this is the ONLY channel that is read. Do NOT just print to the screen; screen output is ignored.
1. Write this exact JSON to the file BODY_PATH (keep "query" verbatim; fill ONLY the values inside "variables"):
{
  "query": "mutation($v:String!,$s:String,$f:String){reportAgentResult(sessionId:\"SESSION_ID\",kind:\"verdict\",verdict:$v,summary:$s,findings:$f)}",
  "variables": {
    "v": "<PASS|NEEDS_CHANGES|BLOCKED>",
    "s": "<one sentence: your LENS_NAME verdict and the core reason>",
    "f": "<if not PASS: the specific, actionable things to fix, one per line; if PASS: none>"
  }
}
2. Then run exactly this (it posts the file as the request body, so your prose needs no shell escaping):
curl -s 127.0.0.1:7788/graphql -H 'content-type: application/json' -d @BODY_PATH"#;
    TEMPLATE
        .replace("BODY_PATH", &review_body_path(session_id).to_string_lossy())
        .replace("SESSION_ID", session_id)
        .replace("LENS_NAME", lens_name)
}

/// Build `ReviewInsights` from a structured lens report (API), splitting the
/// free-text `findings` string into bullet items. No terminal scraping.
fn insights_from_report(report: &AgentReport) -> ReviewInsights {
    let findings = report
        .findings
        .as_deref()
        .map(|f| {
            f.lines()
                .map(|l| {
                    l.trim()
                        .trim_start_matches(['-', '*', '•'])
                        .trim()
                        .to_string()
                })
                .filter(|l| !l.is_empty() && !is_placeholder_bullet(l))
                .collect::<Vec<_>>()
        })
        .unwrap_or_default();
    let summary = report.summary.clone().filter(|s| !s.trim().is_empty());
    let reason = report
        .reason
        .clone()
        .filter(|s| !s.trim().is_empty())
        .or_else(|| findings.first().cloned())
        .or_else(|| summary.clone());
    ReviewInsights {
        summary,
        findings,
        next_steps: Vec::new(),
        reason,
    }
}

fn lens_reviewer_prompt(
    state: &AppState,
    run: &AgentPlanRun,
    phase: &AgentPlanPhase,
    lens_name: &str,
    session_id: &str,
) -> String {
    let phase_path = Path::new(&run.plan.plan_path)
        .join("phases")
        .join(&phase.phase_id);
    let tasks_path = phase_path.join("tasks");
    let values = phase_template_values(state, run, phase, &phase_path, &tasks_path);
    let get = |k: &str| {
        values
            .iter()
            .find(|(kk, _)| *kk == k)
            .map(|(_, v)| v.clone())
            .unwrap_or_default()
    };
    format!(
        "You are the {name} reviewer for phase {phase_id} of a development run. Run ONLY the {name} lens — do not run the other lenses.\n\n\
Read first: methodology at {methodology}; all conventions under {conventions} (especially review-lenses.md — the development-review {name} checklist); the plan overview at {plan}; this phase's overview/status at {phase_path}; the task files under {tasks}. Then inspect the actual delivered work for this phase.\n\n\
Decide a single verdict for the {name} lens: PASS, NEEDS_CHANGES, or BLOCKED.{report}",
        name = lens_name,
        phase_id = phase.phase_id,
        methodology = get("methodology_path"),
        conventions = get("conventions_path"),
        plan = get("plan_path"),
        phase_path = get("phase_path"),
        tasks = get("tasks_path"),
        report = lens_report_instruction(session_id, lens_name),
    )
}

/// Wait for one lens session to report its verdict; nudge (re-send the report
/// command) if it goes idle without one, escalating after `MAX_READY_NUDGES`.
/// Returns the verdict plus a captured snapshot (for SUMMARY/FINDINGS prose).
/// Build the `agent_lens_verdict` event payload, carrying the lens's reason (summary
/// + concrete findings) so the web T2 panel can show *why* it voted as it did — not
/// just PASS/FAIL.
fn lens_verdict_payload(
    lens: &str,
    verdict: &str,
    insights: &ReviewInsights,
) -> serde_json::Value {
    let findings: Vec<&String> = insights
        .findings
        .iter()
        .filter(|f| !is_placeholder_bullet(f))
        .collect();
    json!({
        "lens": lens,
        "verdict": verdict,
        "summary": insights.summary.clone().unwrap_or_default(),
        "findings": findings,
    })
}

/// Wait for one lens session to report its verdict via the `reportAgentResult` API.
/// Returns the full `AgentReport` (verdict + summary + findings) — the reasons come
/// from the structured report, NOT from scraping the terminal. The tmux capture is
/// used only as an idle detector (when to re-send the report instruction).
async fn wait_for_lens_verdict(
    state: &AppState,
    session_id: &str,
    lens_name: &str,
) -> Result<AgentReport, String> {
    clear_agent_report(state, session_id).await;
    let mut last_key = String::new();
    let mut last_changed_at = Instant::now();
    let mut nudges_sent: u32 = 0;
    loop {
        if let Some(report) = take_agent_report_kind(state, session_id, "verdict").await {
            if report.verdict.is_some() {
                return Ok(report);
            }
        }

        let snapshot = terminal::capture_terminal_session_with_history(state, session_id).await?;
        let key = snapshot_idle_key(&snapshot);
        if key != last_key {
            last_key = key;
            last_changed_at = Instant::now();
        }
        if last_changed_at.elapsed() >= Duration::from_millis(READY_NUDGE_IDLE_MS) {
            if nudges_sent >= MAX_READY_NUDGES {
                return Err(format!(
                    "{} lens went idle without reporting a verdict after {} reminders",
                    lens_name, MAX_READY_NUDGES
                ));
            }
            nudges_sent += 1;
            tracing::warn!(
                session_id,
                lens = lens_name,
                attempt = nudges_sent,
                "re-requesting lens verdict"
            );
            let nudge = format!(
                "You have not yet reported your {} verdict.{}",
                lens_name,
                lens_report_instruction(session_id, lens_name)
            );
            terminal::send_terminal_input(state, session_id.to_string(), format!("{}\r", nudge))
                .await?;
            last_changed_at = Instant::now();
        }
        sleep(Duration::from_millis(1_000)).await;
    }
}

/// Merge the three lens outcomes into one reviewer-footer string that
/// `handle_reviewer_output` can route on. PASS iff all pass; BLOCKED wins over
/// NEEDS_CHANGES. Findings are concatenated and labeled by lens.
/// Combine lens verdicts: BLOCKED wins over NEEDS_CHANGES wins over PASS; PASS only
/// if every lens passed. An unrecognized token is treated as NEEDS_CHANGES (safe).
fn merged_verdict<'a>(verdicts: impl IntoIterator<Item = &'a str>) -> &'static str {
    let mut needs_changes = false;
    let mut count = 0;
    for v in verdicts {
        count += 1;
        match v {
            "BLOCKED" => return "BLOCKED",
            "PASS" => {}
            _ => needs_changes = true, // NEEDS_CHANGES or anything unexpected
        }
    }
    if count == 0 || needs_changes {
        "NEEDS_CHANGES"
    } else {
        "PASS"
    }
}

/// Shared body for a merged lens review: rolls the per-lens verdicts up via
/// `merged_verdict` and concatenates their findings. Returns
/// `(summary, findings_block, merged_verdict)` so callers can prepend whatever
/// header (`PHASE:` for dev, none for planning) their output handler expects.
fn merge_lens_body(outcomes: &[(String, String, ReviewInsights)]) -> (String, String, &'static str) {
    let merged = merged_verdict(outcomes.iter().map(|(_, v, _)| v.as_str()));
    let summary = outcomes
        .iter()
        .map(|(name, verdict, _)| format!("{}: {}", name, verdict))
        .collect::<Vec<_>>()
        .join(", ");
    let mut findings = String::new();
    for (name, _verdict, insights) in outcomes {
        for finding in &insights.findings {
            if !is_placeholder_bullet(finding) {
                findings.push_str(&format!("- [{}] {}\n", name, finding));
            }
        }
    }
    if findings.is_empty() {
        findings.push_str("- none\n");
    }
    (summary, findings, merged)
}

fn merge_lens_outcomes(
    phase: &AgentPlanPhase,
    outcomes: &[(String, String, ReviewInsights)],
) -> String {
    let (summary, findings, merged) = merge_lens_body(outcomes);
    format!(
        "PHASE: {}\nSUMMARY: 3-lens review — {}\nFINDINGS:\n{}NEXT_STEPS:\n- none\nVERDICT: {}",
        phase.phase_id, summary, findings, merged
    )
}

/// Planning variant of `merge_lens_outcomes` — no `PHASE:` line (planning is a
/// whole-plan review); `handle_planning_reviewer_output` parses VERDICT/SUMMARY/
/// FINDINGS and ignores any phase header.
fn merge_planning_lens_outcomes(outcomes: &[(String, String, ReviewInsights)]) -> String {
    let (summary, findings, merged) = merge_lens_body(outcomes);
    format!(
        "SUMMARY: 3-lens plan review — {}\nFINDINGS:\n{}NEXT_STEPS:\n- none\nVERDICT: {}",
        summary, findings, merged
    )
}

/// Development phase review via lens fan-out: spawn Product/QA/Lead ephemeral
/// reviewers in parallel, collect verdicts, dispose, merge, and route through the
/// existing reviewer-output handler.
async fn run_lens_fanout_review(
    state: &AppState,
    run: &AgentPlanRun,
    phase: &AgentPlanPhase,
) -> Result<(), String> {
    let provider = run.plan.reviewer_provider.clone();
    let model = default_model_for_provider(&provider);
    let working_dir = Some(run.plan.workspace_path.clone());
    let plan_id = run.plan.id.clone();
    // Lenses POST their review body from a file under here; make sure it exists.
    let _ = std::fs::create_dir_all(std::env::temp_dir().join("johnnyone-reviews"));

    // Spawn all three (they run concurrently once their prompts are sent).
    let mut sessions: Vec<(&str, String)> = Vec::new(); // (display name, session_id)
    for (role, name) in REVIEW_LENSES {
        let sid = spawn_ephemeral_agent(
            state,
            &plan_id,
            role,
            &provider,
            model.clone(),
            working_dir.clone(),
            |sid| lens_reviewer_prompt(state, run, phase, name, sid),
        )
        .await?;
        let _ = append_event(
            state,
            &plan_id,
            Some(&phase.phase_id),
            "agent_lens_review_started",
            json!({ "lens": name }),
        );
        sessions.push((name, sid));
    }

    // Collect all three verdicts concurrently.
    let (r0, r1, r2) = tokio::join!(
        wait_for_lens_verdict(state, &sessions[0].1, sessions[0].0),
        wait_for_lens_verdict(state, &sessions[1].1, sessions[1].0),
        wait_for_lens_verdict(state, &sessions[2].1, sessions[2].0),
    );

    // Tear the reviewers down regardless of outcome.
    for (_, sid) in &sessions {
        dispose_ephemeral_agent(state, sid).await;
    }

    let results = [
        (sessions[0].0, r0),
        (sessions[1].0, r1),
        (sessions[2].0, r2),
    ];
    let mut outcomes: Vec<(String, String, ReviewInsights)> = Vec::new();
    for (name, res) in results {
        let report = res?; // any lens escalation → needs_attention
        let verdict = report
            .verdict
            .clone()
            .unwrap_or_else(|| "NEEDS_CHANGES".to_string());
        let insights = insights_from_report(&report);
        let _ = append_event(
            state,
            &plan_id,
            Some(&phase.phase_id),
            "agent_lens_verdict",
            lens_verdict_payload(name, &verdict, &insights),
        );
        outcomes.push((name.to_string(), verdict, insights));
    }

    let merged = merge_lens_outcomes(phase, &outcomes);
    handle_reviewer_output(state, run, phase, &merged).await
}

fn planning_lens_reviewer_prompt(
    state: &AppState,
    run: &AgentPlanRun,
    lens_name: &str,
    session_id: &str,
) -> String {
    let values = planning_template_values(state, run);
    let get = |k: &str| {
        values
            .iter()
            .find(|(kk, _)| *kk == k)
            .map(|(_, v)| v.clone())
            .unwrap_or_default()
    };
    format!(
        "You are the {name} reviewer for a PLAN (planning run). Run ONLY the {name} lens — do not run the other lenses.\n\n\
Read first: methodology at {methodology}; all conventions under {conventions} (especially review-lenses.md — the planning-review {name} checklist); then read the plan at {plan_output}. Judge whether the PLAN itself is ready along the {name} dimension (e.g. Product: clear scope, mocks, and a screens-to-verify inventory; QA: testable acceptance criteria per phase; Lead: a sound, reuse-aware, secure approach with phases sized right).\n\n\
Decide a single verdict for the {name} lens: PASS, NEEDS_CHANGES, or BLOCKED.{report}",
        name = lens_name,
        methodology = get("methodology_path"),
        conventions = get("conventions_path"),
        plan_output = get("plan_output_path"),
        report = lens_report_instruction(session_id, lens_name),
    )
}

/// Planning review via lens fan-out: spawn Product/QA/Lead ephemeral reviewers in
/// parallel against the whole plan, collect verdicts, dispose, merge, and route
/// through the planning reviewer-output handler. Mirrors `run_lens_fanout_review`
/// but has no phase (planning is a whole-plan review).
async fn run_planning_lens_fanout_review(
    state: &AppState,
    run: &AgentPlanRun,
) -> Result<(), String> {
    let provider = run.plan.reviewer_provider.clone();
    let model = default_model_for_provider(&provider);
    let working_dir = Some(run.plan.workspace_path.clone());
    let plan_id = run.plan.id.clone();
    let _ = std::fs::create_dir_all(std::env::temp_dir().join("johnnyone-reviews"));

    let mut sessions: Vec<(&str, String)> = Vec::new(); // (display name, session_id)
    for (role, name) in REVIEW_LENSES {
        let sid = spawn_ephemeral_agent(
            state,
            &plan_id,
            role,
            &provider,
            model.clone(),
            working_dir.clone(),
            |sid| planning_lens_reviewer_prompt(state, run, name, sid),
        )
        .await?;
        let _ = append_event(
            state,
            &plan_id,
            None,
            "agent_lens_review_started",
            json!({ "lens": name }),
        );
        sessions.push((name, sid));
    }

    let (r0, r1, r2) = tokio::join!(
        wait_for_lens_verdict(state, &sessions[0].1, sessions[0].0),
        wait_for_lens_verdict(state, &sessions[1].1, sessions[1].0),
        wait_for_lens_verdict(state, &sessions[2].1, sessions[2].0),
    );

    for (_, sid) in &sessions {
        dispose_ephemeral_agent(state, sid).await;
    }

    let results = [
        (sessions[0].0, r0),
        (sessions[1].0, r1),
        (sessions[2].0, r2),
    ];
    let mut outcomes: Vec<(String, String, ReviewInsights)> = Vec::new();
    for (name, res) in results {
        let report = res?; // any lens escalation → needs_attention
        let verdict = report
            .verdict
            .clone()
            .unwrap_or_else(|| "NEEDS_CHANGES".to_string());
        let insights = insights_from_report(&report);
        let _ = append_event(
            state,
            &plan_id,
            None,
            "agent_lens_verdict",
            lens_verdict_payload(name, &verdict, &insights),
        );
        outcomes.push((name.to_string(), verdict, insights));
    }

    let merged = merge_planning_lens_outcomes(&outcomes);
    handle_planning_reviewer_output(state, run, &merged).await
}

/// A baked, copy-paste-safe `curl` that the agent runs to report via the host
/// GraphQL endpoint. CO substitutes the session id; only the verdict token (an
/// enum) is left for the agent to fill, so there is no shell-escaping risk.
fn report_command(session_id: &str, kind: &str) -> String {
    const READY: &str = "curl -s 127.0.0.1:7788/graphql -H 'content-type: application/json' -d '{\"query\":\"mutation{reportAgentResult(sessionId:\\\"SESSION_ID\\\",kind:\\\"ready\\\")}\"}'";
    const VERDICT: &str = "curl -s 127.0.0.1:7788/graphql -H 'content-type: application/json' -d '{\"query\":\"mutation{reportAgentResult(sessionId:\\\"SESSION_ID\\\",kind:\\\"verdict\\\",verdict:\\\"<PASS|NEEDS_CHANGES|BLOCKED>\\\")}\"}'";
    const BLOCKED: &str = "curl -s 127.0.0.1:7788/graphql -H 'content-type: application/json' -d '{\"query\":\"mutation{reportAgentResult(sessionId:\\\"SESSION_ID\\\",kind:\\\"blocked\\\")}\"}'";
    const DONE: &str = "curl -s 127.0.0.1:7788/graphql -H 'content-type: application/json' -d '{\"query\":\"mutation{reportAgentResult(sessionId:\\\"SESSION_ID\\\",kind:\\\"done\\\")}\"}'";
    let tmpl = match kind {
        "verdict" => VERDICT,
        "blocked" => BLOCKED,
        "done" => DONE,
        _ => READY,
    };
    tmpl.replace("SESSION_ID", session_id)
}

/// Feature 3 — prompt for the docs-commit agent (dev-only, on full plan completion).
/// It updates the APP repo's docs to reflect what the plan delivered and commits ONLY
/// the docs (commit-only, no push).
fn docs_commit_prompt(run: &AgentPlanRun, app_repo: &str, session_id: &str) -> String {
    format!(
        "You are the documentation agent. A development plan has just COMPLETED — every phase passed review. Your ONE job is to bring this app's documentation up to date for what was delivered, then commit ONLY the docs.\n\n\
APP CODE REPOSITORY (work here): {app_repo}\n\
PLAN (read its overview.md + status.md to learn what changed/was delivered): {plan_path}\n\n\
Steps:\n\
1. FIRST read the README at the app repo root. Update it ONLY where needed and keep it CONCISE — do NOT bloat the README. Put any detailed documentation in a `docs/` directory (create `docs/` if it does not exist). If the README links to other docs, open and update the ones that are now out of date.\n\
2. Write accurate docs for what this plan delivered (new/changed behavior, config/env, APIs, flows) based on the plan AND the actual code — do not invent.\n\
3. Commit ONLY the documentation files you changed. Do NOT commit code, do NOT stage everything, and do NOT push:\n\
   git add <only the doc files you changed>   # e.g. README.md docs/whatever.md — NEVER `git add -A` or `git add .`\n\
   git commit -m \"docs: {title}\"\n\
4. When the commit is done, report completion by running this exact command:\n{report}",
        app_repo = app_repo,
        plan_path = run.plan.plan_path,
        title = run.plan.title.replace('"', "'"),
        report = report_command(session_id, "done"),
    )
}

/// Wait for an ephemeral agent to report `kind:done` via the API. Nudges (re-sends
/// the report command) if it goes idle without reporting; escalates after
/// `MAX_READY_NUDGES`. Mirrors `wait_for_lens_verdict` but for the `done` signal.
async fn wait_for_done_report(
    state: &AppState,
    session_id: &str,
    label: &str,
) -> Result<(), String> {
    clear_agent_report(state, session_id).await;
    let mut last_key = String::new();
    let mut last_changed_at = Instant::now();
    let mut nudges_sent: u32 = 0;
    loop {
        if take_agent_report_kind(state, session_id, "done").await.is_some() {
            return Ok(());
        }
        let snapshot = terminal::capture_terminal_session_with_history(state, session_id).await?;
        let key = snapshot_idle_key(&snapshot);
        if key != last_key {
            last_key = key;
            last_changed_at = Instant::now();
        }
        if last_changed_at.elapsed() >= Duration::from_millis(READY_NUDGE_IDLE_MS) {
            if nudges_sent >= MAX_READY_NUDGES {
                return Err(format!(
                    "{} agent went idle without reporting done after {} reminders",
                    label, MAX_READY_NUDGES
                ));
            }
            nudges_sent += 1;
            let nudge = format!(
                "You have not reported completion. When the docs are committed, run this exact command:\n{}",
                report_command(session_id, "done")
            );
            terminal::send_terminal_input(state, session_id.to_string(), format!("{}\r", nudge))
                .await?;
            last_changed_at = Instant::now();
        }
        sleep(Duration::from_millis(1_000)).await;
    }
}

/// Feature 3 — on full development completion, spawn an ephemeral "docs" agent that
/// updates the APP repo's docs and commits them (commit-only, no push). Dev-only.
/// Requires the run's `app_scope` (the app repo path); skips gracefully if unset.
async fn run_docs_commit_agent(state: &AppState, plan_id: &str) -> Result<(), String> {
    let run = get_plan(state, plan_id)?;
    let app_repo = match run.plan.app_scope.clone().filter(|s| !s.trim().is_empty()) {
        Some(p) => p,
        None => {
            tracing::info!(plan_id, "no app_scope set — skipping docs-commit agent");
            return append_event(
                state,
                plan_id,
                None,
                "agent_docs_commit_skipped",
                json!({ "reason": "No app repo path (app_scope) set on this run — set it to enable docs commits." }),
            );
        }
    };
    let provider = run.plan.worker_provider.clone();
    let model = default_model_for_provider(&provider);
    append_event(
        state,
        plan_id,
        None,
        "agent_docs_commit_started",
        json!({ "appRepo": app_repo }),
    )?;
    let session_id = spawn_ephemeral_agent(
        state,
        plan_id,
        "docs",
        &provider,
        model,
        Some(app_repo.clone()),
        |sid| docs_commit_prompt(&run, &app_repo, sid),
    )
    .await?;
    let result = wait_for_done_report(state, &session_id, "docs").await;
    dispose_ephemeral_agent(state, &session_id).await;
    result?;
    append_event(
        state,
        plan_id,
        None,
        "agent_docs_committed",
        json!({ "appRepo": app_repo }),
    )
}

fn worker_report_instruction(session_id: &str) -> String {
    format!(
        "\n\n---\nIMPORTANT — this is the ONLY way the coordinator knows you are done. When this phase is complete and ready for T2 review, run this exact command:\n{}\n\nIf you get genuinely stuck and need a human decision you cannot resolve yourself (e.g. a missing credential, or an ambiguous requirement with no safe default), FIRST clearly state your question/blocker in your output, THEN run this command and wait — the coordinator will alert a human, who replies right here to unblock you:\n{}\n",
        report_command(session_id, "ready"),
        report_command(session_id, "blocked"),
    )
}

fn reviewer_report_instruction(session_id: &str) -> String {
    format!(
        "\n\n---\nIMPORTANT — this is the ONLY way the coordinator receives your verdict. After you decide, run this exact command — replace <PASS|NEEDS_CHANGES|BLOCKED> with your single chosen verdict:\n{}\nThen print a short footer with SUMMARY, FINDINGS, and NEXT_STEPS for the record.",
        report_command(session_id, "verdict")
    )
}

/// Append the worker/planner (`ready`) report instruction if the session exists.
fn append_worker_report(run: &AgentPlanRun, prompt: String) -> String {
    match &run.plan.worker_session_id {
        Some(sid) => format!("{}{}", prompt, worker_report_instruction(sid)),
        None => prompt,
    }
}

/// Append the reviewer (`verdict`) report instruction if the session exists.
fn append_reviewer_report(run: &AgentPlanRun, prompt: String) -> String {
    match &run.plan.reviewer_session_id {
        Some(sid) => format!("{}{}", prompt, reviewer_report_instruction(sid)),
        None => prompt,
    }
}

/// Wait on a reviewer (T2) session. Returns `Some(snapshot)` once the reviewer has
/// reported its verdict via the `reportAgentResult` API; returns `None` if it has
/// gone idle for `READY_NUDGE_IDLE_MS` without reporting (caller clarifies/escalates).
///
/// The verdict comes solely from the structured report — no terminal scraping. The
/// reported verdict is appended as an authoritative `VERDICT:` line to the captured
/// snapshot so `handle_reviewer_output` reads it while SUMMARY/FINDINGS still come
/// from the pane. The capture-based idle check only decides *when* the reviewer has
/// stopped (so we don't clarify mid-review); it is not a signal source.
// Superseded by the lens fan-out for both dev and planning review; retained as the
// reference single-reviewer idle+verdict waiter.
#[allow(dead_code)]
async fn wait_for_reviewer_idle_or_verdict(
    state: &AppState,
    session_id: &str,
) -> Result<Option<terminal::TerminalSnapshot>, String> {
    // Discard any stale report from a previous turn so it can't trigger this one.
    clear_agent_report(state, session_id).await;
    let mut last_key = String::new();
    let mut last_changed_at = Instant::now();
    loop {
        if let Some(report) = take_agent_report_kind(state, session_id, "verdict").await {
            if let Some(verdict) = report.verdict {
                let mut snapshot =
                    terminal::capture_terminal_session_with_history(state, session_id).await?;
                snapshot.content = format!("{}\nVERDICT: {}", snapshot.content, verdict);
                return Ok(Some(snapshot));
            }
        }

        let snapshot = terminal::capture_terminal_session_with_history(state, session_id).await?;
        let key = snapshot_idle_key(&snapshot);
        if key != last_key {
            last_key = key;
            last_changed_at = Instant::now();
        }
        // Reviewer has stopped but never reported a verdict — caller clarifies.
        if last_changed_at.elapsed() >= Duration::from_millis(READY_NUDGE_IDLE_MS) {
            return Ok(None);
        }

        sleep(Duration::from_millis(1_000)).await;
    }
}

async fn wait_for_worker_ready(
    state: &AppState,
    session_id: &str,
    plan_id: &str,
    phase_id: Option<&str>,
) -> Result<(), String> {
    wait_for_agent_ready_report(state, session_id, plan_id, phase_id).await
}

async fn wait_for_planner_ready(
    state: &AppState,
    session_id: &str,
    plan_id: &str,
) -> Result<(), String> {
    wait_for_agent_ready_report(state, session_id, plan_id, None).await
}

/// Wait for a worker/planner (T1) session to report `ready` via the
/// `reportAgentResult` API — the sole completion signal (no marker scraping). If
/// the session goes idle for `READY_NUDGE_IDLE_MS` without reporting, re-send the
/// exact report command; after `MAX_READY_NUDGES` the run escalates (the caller
/// turns the `Err` into needs_attention). The capture-based idle check only decides
/// when the agent has stopped, so we don't nudge mid-work.
async fn wait_for_agent_ready_report(
    state: &AppState,
    session_id: &str,
    plan_id: &str,
    phase_id: Option<&str>,
) -> Result<(), String> {
    // Discard any stale report from a previous turn so it can't trigger this one.
    clear_agent_report(state, session_id).await;
    let mut last_key = String::new();
    let mut last_changed_at = Instant::now();
    let mut nudges_sent: u32 = 0;
    // When the agent reports it's blocked on a human, we stop nudging/escalating
    // and wait (indefinitely) for the human to reply — which resumes the agent and
    // produces the `ready` report below.
    let mut blocked = false;
    loop {
        if take_agent_report_kind(state, session_id, "ready").await.is_some() {
            return Ok(());
        }
        if take_agent_report_kind(state, session_id, "blocked").await.is_some() {
            if !blocked {
                blocked = true;
                tracing::warn!(session_id, plan_id, "agent reported blocked — needs a human");
                let _ = append_event(state, plan_id, phase_id, "agent_blocked", json!({}));
            }
        }

        let snapshot = terminal::capture_terminal_session_with_history(state, session_id).await?;
        let key = snapshot_idle_key(&snapshot);
        if key != last_key {
            last_key = key;
            last_changed_at = Instant::now();
        }

        // While blocked, never nudge/escalate — the agent is intentionally waiting
        // on the human, not stuck.
        if !blocked && last_changed_at.elapsed() >= Duration::from_millis(READY_NUDGE_IDLE_MS) {
            if nudges_sent >= MAX_READY_NUDGES {
                // Record an explaining event before escalating, so the timeout shows
                // up in the event log instead of a silent status flip (the generic
                // coordinator catch sets `needs_attention` but writes no event).
                let (event_type, reason) = match phase_id {
                    Some(_) => ("agent_phase_needs_attention", "worker_no_report"),
                    None => ("planning_needs_attention", "planner_no_report"),
                };
                let _ = append_event(
                    state,
                    plan_id,
                    phase_id,
                    event_type,
                    json!({ "reason": reason, "reminders": MAX_READY_NUDGES }),
                );
                if let Some(pid) = phase_id {
                    let _ = state.db.with_conn(|conn| {
                        conn.execute(
                            "UPDATE agent_plan_phases SET status = 'needs_attention', updated_at = datetime('now') WHERE plan_id = ?1 AND phase_id = ?2",
                            params![plan_id, pid],
                        )
                        .map_err(|e| e.to_string())
                    });
                }
                return Err(format!(
                    "Agent went idle without reporting completion after {} reminders",
                    MAX_READY_NUDGES
                ));
            }
            nudges_sent += 1;
            tracing::warn!(
                session_id,
                attempt = nudges_sent,
                "coordinator re-requesting ready report"
            );
            let nudge = format!(
                "You have not reported completion to the coordinator. If this turn is complete, run exactly:\n{}\nIf you are not finished, continue the work.",
                report_command(session_id, "ready")
            );
            terminal::send_terminal_input(state, session_id.to_string(), format!("{}\r", nudge))
                .await?;
            // Restart the idle clock so the agent has time to respond before the
            // next re-request.
            last_changed_at = Instant::now();
        }

        sleep(Duration::from_millis(1_000)).await;
    }
}

fn snapshot_idle_key(snapshot: &terminal::TerminalSnapshot) -> String {
    normalize_terminal_snapshot_for_idle(&snapshot.content)
}


fn normalize_terminal_snapshot_for_idle(content: &str) -> String {
    strip_ansi_escapes(content)
        .lines()
        .filter_map(|line| {
            let normalized = marker_search_text(line);
            let trimmed = normalized.trim();
            if trimmed.is_empty() {
                return None;
            }
            if trimmed == "█" || trimmed.ends_with('█') {
                return None;
            }
            if trimmed.starts_with("~/") && trimmed.contains('│') {
                return None;
            }
            if trimmed.starts_with("▾ Tasks") || is_volatile_grok_task_line(trimmed) {
                return None;
            }
            if trimmed.contains("Shift+Tab:")
                || trimmed.starts_with('╭')
                || trimmed.starts_with('╰')
            {
                return None;
            }
            // The persistent Grok status footer ("Grok Build · always-approve",
            // formerly "Grok Composer …"). Match on the stable token so a version
            // rename doesn't unfilter it.
            if trimmed.contains("always-approve") {
                return None;
            }
            if trimmed.starts_with("│ ❯") {
                return None;
            }
            Some(trimmed.to_string())
        })
        .collect::<Vec<_>>()
        .join("\n")
}

fn is_volatile_grok_task_line(trimmed: &str) -> bool {
    // IMPORTANT: Grok's *working* indicator — the spinner + elapsed timer + run
    // markers ([✗]/[⛶]) — is intentionally NOT filtered. While the agent works that
    // line changes every second, so the idle key keeps changing and the agent
    // registers as ACTIVE. Filtering it (as we used to) made a busy agent look idle
    // after the nudge window and fired premature ready-nudges during long
    // tool-runs. Only collapsed-list bullets are dropped here.
    trimmed.starts_with('❯') || trimmed.starts_with("⸬ ")
}

fn strip_ansi_escapes(content: &str) -> String {
    let mut out = String::with_capacity(content.len());
    let mut chars = content.chars().peekable();
    while let Some(ch) = chars.next() {
        if ch == '\x1b' {
            if chars.next_if_eq(&'[').is_some() {
                for next in chars.by_ref() {
                    if ('@'..='~').contains(&next) {
                        break;
                    }
                }
            }
            continue;
        }
        out.push(ch);
    }
    out
}

async fn dispatch_planning_review(state: &AppState, run: &AgentPlanRun) -> Result<(), String> {
    // Planning review fans out into ephemeral Product/QA/Lead reviewers, spawned by
    // `run_planning_lens_fanout_review` from the `planning_review_running` arm. Here
    // we only transition into review; the persistent reviewer session is unused.
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

/// Read the `verdict` field straight from an event's stored payload (independent of
/// enrichment), used to count review rounds.
fn event_payload_verdict(event: &AgentPlanEvent) -> Option<String> {
    serde_json::from_str::<serde_json::Value>(&event.payload_json)
        .ok()
        .and_then(|p| {
            p.get("verdict")
                .and_then(|v| v.as_str())
                .map(|s| s.to_ascii_uppercase())
        })
}

/// Consecutive non-PASS review rounds at the tail of the event log for a given gate
/// event type (optionally scoped to one phase). Stops at the most recent PASS so an
/// amend cycle / a later phase starts its count fresh.
fn count_consecutive_non_pass(
    events: &[AgentPlanEvent],
    gate_event_type: &str,
    start_event_type: &str,
    phase_id: Option<&str>,
) -> i64 {
    let mut n = 0;
    for event in events.iter().rev() {
        // Reset boundaries: a manual stop, or a fresh (re)start of this attempt, ends
        // the backward count. Rounds from before the user last intervened (stopped /
        // restarted / amended) must NOT carry over — otherwise a re-run of a stuck
        // plan re-escalates on its very first round.
        if event.event_type == "agent_plan_stopped" {
            break;
        }
        if event.event_type == start_event_type
            && phase_id.map_or(true, |pid| event.phase_id.as_deref() == Some(pid))
        {
            break;
        }
        if event.event_type != gate_event_type {
            continue;
        }
        if let Some(pid) = phase_id {
            if event.phase_id.as_deref() != Some(pid) {
                continue;
            }
        }
        match event_payload_verdict(event).as_deref() {
            Some("PASS") => break,
            Some(_) => n += 1,
            None => {}
        }
    }
    n
}

fn consecutive_non_pass_planning_rounds(run: &AgentPlanRun) -> i64 {
    count_consecutive_non_pass(
        &run.events,
        "planning_gate_result",
        "planning_started",
        None,
    )
}

fn consecutive_non_pass_phase_rounds(run: &AgentPlanRun, phase_id: &str) -> i64 {
    count_consecutive_non_pass(
        &run.events,
        "agent_phase_gate_result",
        "agent_phase_started",
        Some(phase_id),
    )
}

/// Stop a development phase that won't converge: pause the run into `needs_attention`
/// (fires the Discord attention alert) instead of looping T1↔T2 forever.
async fn escalate_phase_no_converge(
    state: &AppState,
    run: &AgentPlanRun,
    phase: &AgentPlanPhase,
    round: i64,
    verdict: &str,
    summary: &str,
) -> Result<(), String> {
    let reason = format!(
        "Phase {} still not passing after {} review rounds (last: {} — {}). Coordinator paused — needs a human decision.",
        phase.phase_id, round, verdict, summary
    );
    state.db.with_conn(|conn| {
        update_plan_status_and_health(conn, &run.plan.id, "needs_attention", Some(&reason))
            .map_err(|e| e.to_string())
    })?;
    append_event(
        state,
        &run.plan.id,
        Some(&phase.phase_id),
        "agent_phase_needs_attention",
        json!({ "reason": reason, "rounds": round }),
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
                update_plan_status_and_health(conn, &run.plan.id, "approved", None)
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
            let summary = review.summary.clone().unwrap_or_else(|| summarize_output(output));
            let round = consecutive_non_pass_planning_rounds(run) + 1;
            // Record this round's verdict regardless of what we do next.
            append_event(
                state,
                &run.plan.id,
                None,
                "planning_gate_result",
                review_payload(&verdict, Some("sent_back_to_planner"), &review),
            )?;
            // Loop guard: a review that never converges must not churn forever.
            if round >= MAX_REVISION_ROUNDS {
                let reason = format!(
                    "Plan still not passing after {} review rounds (last: {} — {}). Coordinator paused — needs a human decision.",
                    round, verdict, summary
                );
                state.db.with_conn(|conn| {
                    update_plan_status_and_health(conn, &run.plan.id, "needs_attention", Some(&reason))
                        .map_err(|e| e.to_string())
                })?;
                return append_event(
                    state,
                    &run.plan.id,
                    None,
                    "planning_needs_attention",
                    json!({ "reason": reason, "rounds": round }),
                );
            }
            state.db.with_conn(|conn| {
                conn.execute(
                    "UPDATE agent_plans SET status = 'planning_planner_running', error = ?1, updated_at = datetime('now') WHERE id = ?2",
                    params![summary, run.plan.id],
                )
                .map_err(|e| e.to_string())
            })?;
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
    let prompt = append_worker_report(
        run,
        format!(
            "T2 reviewed the plan and requested changes.\n\nReviewer feedback:\n{}\n\nUpdate only the plan files at {}.",
            feedback, run.plan.plan_path
        ),
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
            update_plan_status_and_health(
                conn,
                &run.plan.id,
                "needs_attention",
                Some("T2 did not return a parseable planning verdict after 5 clarification attempts"),
            )
            .map_err(|e| e.to_string())
        })?;
        return append_event(
            state,
            &run.plan.id,
            None,
            "planning_needs_attention",
            json!({ "reason": "T2 did not return a parseable planning verdict after 5 clarification attempts" }),
        );
    }

    let reviewer_session_id = run
        .plan
        .reviewer_session_id
        .clone()
        .ok_or_else(|| "Planning run has no reviewer session".to_string())?;
    let prompt = format!(
        "You have not reported a verdict to the coordinator. Run exactly this command — replace <PASS|NEEDS_CHANGES|BLOCKED> with your single chosen verdict:\n{}\nThen print a short footer with SUMMARY, FINDINGS, and NEXT_STEPS for the record.",
        report_command(&reviewer_session_id, "verdict")
    );
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
    // Development review fans out into ephemeral Product/QA/Lead reviewers, spawned
    // by `run_lens_fanout_review` from the `phase_review_running` arm. Here we only
    // transition the phase into review; the persistent reviewer session is not used
    // for the fan-out.
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
            let round = consecutive_non_pass_phase_rounds(run, &phase.phase_id) + 1;
            if round >= MAX_REVISION_ROUNDS {
                let summary = review.summary.clone().unwrap_or_else(|| summarize_output(output));
                return escalate_phase_no_converge(state, run, phase, round, "NEEDS_CHANGES", &summary).await;
            }
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
            let round = consecutive_non_pass_phase_rounds(run, &phase.phase_id) + 1;
            if round >= MAX_REVISION_ROUNDS {
                let summary = review.summary.clone().unwrap_or_else(|| summarize_output(output));
                return escalate_phase_no_converge(state, run, phase, round, "BLOCKED", &summary).await;
            }
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
    let prompt = append_worker_report(run, reviewer_feedback_prompt(phase, reviewer_output));
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
            update_plan_status_and_health(
                conn,
                &run.plan.id,
                "needs_attention",
                Some("T2 did not return a parseable verdict after 5 clarification attempts"),
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
            json!({ "reason": "T2 did not return a parseable verdict after 5 clarification attempts" }),
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
    let prompt = format!(
        "You have not reported a verdict to the coordinator. Run exactly this command — replace <PASS|NEEDS_CHANGES|BLOCKED> with your single chosen verdict:\n{}\nThen print a short footer with SUMMARY, FINDINGS, and NEXT_STEPS for the record.",
        report_command(&reviewer_session_id, "verdict")
    );
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
            update_plan_status_and_health(conn, plan_id, "approved", None)
                .map_err(|e| e.to_string())?;
            conn.execute(
                "UPDATE agent_plans SET phase_run_mode = 'continue', updated_at = datetime('now') WHERE id = ?1",
                params![plan_id],
            )
            .map_err(|e| e.to_string())?;
        }
        } else {
            update_plan_status_and_health(conn, plan_id, "approved", None)
                .map_err(|e| e.to_string())?;
            conn.execute(
                "UPDATE agent_plans SET phase_run_mode = 'continue', updated_at = datetime('now') WHERE id = ?1",
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
        let prompt = worker_phase_prompt(state, &refreshed, &next)?;
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
            // Feature 3: dev-only docs-commit agent — update + commit the app repo's
            // docs (commit only). Failures are logged + surfaced but never un-approve
            // the plan (it is already complete).
            if let Err(err) = run_docs_commit_agent(state, plan_id).await {
                tracing::warn!(plan_id, %err, "docs-commit agent failed");
                let _ = append_event(
                    state,
                    plan_id,
                    None,
                    "agent_docs_commit_failed",
                    json!({ "reason": err }),
                );
            }
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
    // Reject traversal on the raw input first, before any join/normalize (same message/logic as
    // create_planning_run). This is the only path guard now that plans live in the global store.
    if plan_path.split(['/', '\\']).any(|seg| seg == "..") {
        return Err("Plan path must not contain '..'".to_string());
    }
    let workspace = normalize_path(Path::new(workspace_path))?;
    if !workspace.is_dir() {
        return Err("Workspace path is not a directory".to_string());
    }
    let raw_plan = Path::new(plan_path);
    // Absolute store paths (outside the workspace) resolve as-is; relative paths join the
    // workspace. The former "plan must be inside the workspace" rejection is intentionally gone
    // (D6) — the initiatives store lives outside every repo.
    let plan = if raw_plan.is_absolute() {
        normalize_path(raw_plan)?
    } else {
        normalize_path(&workspace.join(raw_plan))?
    };
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
    match provider {
        "ollama" => Some("qwen3.5:2b".to_string()),
        // `grok-composer-2.5-fast` is the cheap/weak model — too weak for the
        // planner and the review lenses (they make real judgments). Use Grok's own
        // default, `grok-build`, the stronger model. Applies to worker, reviewer,
        // and all three ephemeral lens reviewers.
        "grok" => Some("grok-build".to_string()),
        _ => None,
    }
}

fn worker_phase_prompt(
    state: &AppState,
    run: &AgentPlanRun,
    phase: &AgentPlanPhase,
) -> Result<String, String> {
    let phase_path = Path::new(&run.plan.plan_path)
        .join("phases")
        .join(&phase.phase_id);
    let tasks_path = phase_path.join("tasks");
    let settings = planner_prompts::load_prompt_settings()?;
    let base = planner_prompts::render_template(
        &settings.development.worker,
        &phase_template_values(state, run, phase, &phase_path, &tasks_path),
    );
    Ok(append_worker_report(run, base))
}

// Single-reviewer (all-lenses) development prompt. Superseded by the lens fan-out
// (`run_lens_fanout_review`); kept as a fallback if we ever want non-fanout review.
#[allow(dead_code)]
fn reviewer_phase_prompt(
    state: &AppState,
    run: &AgentPlanRun,
    phase: &AgentPlanPhase,
) -> Result<String, String> {
    let phase_path = Path::new(&run.plan.plan_path)
        .join("phases")
        .join(&phase.phase_id);
    let tasks_path = phase_path.join("tasks");
    let settings = planner_prompts::load_prompt_settings()?;
    let base = planner_prompts::render_template(
        &settings.development.reviewer,
        &phase_template_values(state, run, phase, &phase_path, &tasks_path),
    );
    Ok(append_reviewer_report(run, base))
}

fn phase_template_values(
    state: &AppState,
    run: &AgentPlanRun,
    phase: &AgentPlanPhase,
    phase_path: &Path,
    tasks_path: &Path,
) -> Vec<(&'static str, String)> {
    let methodology_path =
        settings_service::resolve_methodology_path(state, &run.plan.workspace_path)
            .unwrap_or_else(|_| {
                Path::new(&run.plan.workspace_path)
                    .join(settings_service::DEFAULT_METHODOLOGY_REL)
                    .to_string_lossy()
                    .to_string()
            });
    let conventions_path =
        settings_service::resolve_conventions_path(state, &run.plan.workspace_path)
            .unwrap_or_else(|_| {
                Path::new(&run.plan.workspace_path)
                    .join(settings_service::DEFAULT_CONVENTIONS_REL)
                    .to_string_lossy()
                    .to_string()
            });
    vec![
        ("run_id", run.plan.id.clone()),
        ("phase_id", phase.phase_id.clone()),
        ("workspace_path", run.plan.workspace_path.clone()),
        ("plan_path", run.plan.plan_path.clone()),
        ("phase_path", phase_path.to_string_lossy().to_string()),
        ("tasks_path", tasks_path.to_string_lossy().to_string()),
        ("methodology_path", methodology_path),
        ("conventions_path", conventions_path),
    ]
}

fn planning_template_values(state: &AppState, run: &AgentPlanRun) -> Vec<(&'static str, String)> {
    let methodology_path =
        settings_service::resolve_methodology_path(state, &run.plan.workspace_path)
            .unwrap_or_else(|_| {
                Path::new(&run.plan.workspace_path)
                    .join(settings_service::DEFAULT_METHODOLOGY_REL)
                    .to_string_lossy()
                    .to_string()
            });
    let conventions_path =
        settings_service::resolve_conventions_path(state, &run.plan.workspace_path)
            .unwrap_or_else(|_| {
                Path::new(&run.plan.workspace_path)
                    .join(settings_service::DEFAULT_CONVENTIONS_REL)
                    .to_string_lossy()
                    .to_string()
            });
    vec![
        ("run_id", run.plan.id.clone()),
        ("workspace_path", run.plan.workspace_path.clone()),
        ("plan_output_path", run.plan.plan_path.clone()),
        ("methodology_path", methodology_path),
        ("conventions_path", conventions_path),
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

fn planning_continue_prompt(run: &AgentPlanRun) -> String {
    format!(
        "Continue updating the plan at {}. When ready for T2 review, say exactly {}.",
        run.plan.plan_path, PLANNER_READY_MARKER
    )
}

fn planning_planner_prompt(state: &AppState, run: &AgentPlanRun) -> Result<String, String> {
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
    let base = planner_prompts::render_template(template, &planning_template_values(state, run));
    Ok(append_worker_report(run, base))
}

// Superseded by `planning_lens_reviewer_prompt` (3-lens fan-out); retained as the
// reference single-reviewer planning prompt.
#[allow(dead_code)]
fn planning_reviewer_prompt(state: &AppState, run: &AgentPlanRun) -> Result<String, String> {
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
    let base = planner_prompts::render_template(template, &planning_template_values(state, run));
    Ok(append_reviewer_report(run, base))
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
    for line in output.lines().rev() {
        if let Some(verdict) = parse_verdict_line(line) {
            return Some(verdict);
        }
    }
    None
}

fn parse_verdict_line(line: &str) -> Option<String> {
    let normalized = marker_search_text(line);
    let upper = normalized.to_ascii_uppercase();
    let idx = upper.find("VERDICT:")?;
    let value = normalized[idx + "VERDICT:".len()..].trim();
    // Ignore the instruction/echo line that lists every option, e.g.
    // "VERDICT: PASS | NEEDS_CHANGES | BLOCKED" — only a single concrete verdict
    // counts (otherwise we'd parse our own clarification prompt as a PASS).
    let value_upper = value.to_ascii_uppercase();
    let option_count = ["PASS", "NEEDS_CHANGES", "BLOCKED"]
        .iter()
        .filter(|token| value_upper.contains(*token))
        .count();
    if option_count > 1 {
        return None;
    }
    normalize_verdict_token(value)
}

fn normalize_verdict_token(value: &str) -> Option<String> {
    let token = value
        .split_whitespace()
        .next()?
        .trim_matches(|ch: char| !ch.is_alphanumeric() && ch != '_')
        .to_ascii_uppercase();
    match token.as_str() {
        "PASS" | "NEEDS_CHANGES" | "BLOCKED" => Some(token),
        _ => None,
    }
}

/// Strip ANSI/VT escape sequences (CSI `ESC [ … final`, OSC `ESC ] … BEL/ST`). The
/// old code only dropped the ESC byte, leaving the visible `[38;2;…m` parameter text
/// behind — which then polluted scraped summaries with terminal color codes.
fn strip_ansi(input: &str) -> String {
    let mut out = String::with_capacity(input.len());
    let mut chars = input.chars().peekable();
    while let Some(c) = chars.next() {
        if c != '\x1b' {
            out.push(c);
            continue;
        }
        match chars.peek() {
            Some('[') => {
                chars.next();
                while let Some(&n) = chars.peek() {
                    chars.next();
                    if ('@'..='~').contains(&n) {
                        break;
                    }
                }
            }
            Some(']') => {
                chars.next();
                while let Some(&n) = chars.peek() {
                    chars.next();
                    if n == '\x07' {
                        break;
                    }
                    if n == '\x1b' {
                        if chars.peek() == Some(&'\\') {
                            chars.next();
                        }
                        break;
                    }
                }
            }
            _ => {}
        }
    }
    out
}

fn marker_search_text(line: &str) -> String {
    strip_ansi(line)
        .chars()
        .filter(|ch| !ch.is_control())
        .collect::<String>()
}

/// True for an agent-TUI status/shortcut bar line (e.g. Grok's
/// "Shift+Tab :mode │ Ctrl+c :cancel │ …") so it never gets scraped as a summary.
fn is_terminal_chrome_line(lower: &str) -> bool {
    lower.contains("shift+tab")
        || lower.contains(":shortcuts")
        || lower.contains(":interject")
        || lower.contains(":cancel")
        || (lower.contains("ctrl+") && lower.contains(':'))
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
        && !is_terminal_chrome_line(&lower)
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
    // Single chokepoint for out-of-band alerts: notifiable events (phase
    // started/passed/changes/attention, plan completed) fire a Discord message.
    maybe_notify_discord(state, plan_id, phase_id, event_type, &payload);
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

pub(crate) fn content_type_for_path(path: &Path) -> String {
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

pub(crate) fn is_text_content_type(content_type: &str) -> bool {
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
        initiative_id: row.get(19)?,
        initiative_status: row.get(20)?,
        health: row.get(21)?,
        created_at: row.get(22)?,
        updated_at: row.get(23)?,
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
        "agent_docs_commit_started" => "Docs agent — updating app-repo docs".to_string(),
        "agent_docs_committed" => "Docs committed to the app repo".to_string(),
        "agent_docs_commit_skipped" => {
            "Docs commit skipped (no app repo path set)".to_string()
        }
        "agent_docs_commit_failed" => "Docs commit failed".to_string(),
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

#[cfg(test)]
mod coordinator_terminal_tests {
    use super::*;

    #[test]
    fn parse_verdict_accepts_grok_style_verdict_label() {
        let output = "I reviewed the phase and updated the status files. Verdict: PASS.";
        assert_eq!(parse_verdict(output).as_deref(), Some("PASS"));
    }

    #[test]
    fn parse_verdict_reads_long_joined_footer_line() {
        // tmux -J joins the whole footer into one long line; the verdict must
        // still parse out of the prose that follows it.
        let output = "PHASE: 04-validation VERDICT: PASS SUMMARY: Re-validation confirms phase 04 \
            still meets done criteria; 9/9 E2E green, worker 185/185. FINDINGS: none NEXT_STEPS: none";
        assert_eq!(parse_verdict(output).as_deref(), Some("PASS"));
    }

    #[test]
    fn discord_message_maps_notifiable_events() {
        let p = |t: &str, ph: Option<&str>, v: serde_json::Value| discord_message_for(t, ph, &v);
        // phase pass → success
        let (sev, head, _) = p(
            "agent_phase_gate_result",
            Some("02-x"),
            serde_json::json!({ "verdict": "PASS", "summary": "all good" }),
        )
        .unwrap();
        assert_eq!(sev, "success");
        assert!(head.contains("passed"));
        // a non-PASS T2 result is NOT alerted (it just loops back to T1)
        assert!(p(
            "agent_phase_gate_result",
            Some("02-x"),
            serde_json::json!({ "verdict": "NEEDS_CHANGES", "reason": "missing test" }),
        )
        .is_none());
        assert!(p(
            "planning_gate_result",
            None,
            serde_json::json!({ "verdict": "NEEDS_CHANGES" }),
        )
        .is_none());
        // escalation → attention
        assert_eq!(
            p("agent_phase_needs_attention", Some("x"), serde_json::json!({})).unwrap().0,
            "attention"
        );
        // plan completed → success
        assert_eq!(
            p("agent_plan_completed", None, serde_json::json!({})).unwrap().0,
            "success"
        );
        // blocked → attention
        assert_eq!(
            p("agent_blocked", Some("x"), serde_json::json!({})).unwrap().0,
            "attention"
        );
        // routine / progress events are not notified (quiet mode: block or done only)
        assert!(p("agent_phase_worker_idle", Some("x"), serde_json::json!({})).is_none());
        assert!(p("agent_lens_review_started", Some("x"), serde_json::json!({})).is_none());
        assert!(p("agent_phase_started", Some("x"), serde_json::json!({})).is_none());
        assert!(p("planning_started", None, serde_json::json!({})).is_none());
    }

    #[test]
    fn merged_verdict_combines_lens_results() {
        assert_eq!(merged_verdict(["PASS", "PASS", "PASS"]), "PASS");
        assert_eq!(merged_verdict(["PASS", "NEEDS_CHANGES", "PASS"]), "NEEDS_CHANGES");
        assert_eq!(merged_verdict(["PASS", "NEEDS_CHANGES", "BLOCKED"]), "BLOCKED");
        assert_eq!(merged_verdict(["BLOCKED", "PASS", "PASS"]), "BLOCKED");
        // empty / unexpected tokens fail safe
        assert_eq!(merged_verdict(std::iter::empty::<&str>()), "NEEDS_CHANGES");
        assert_eq!(merged_verdict(["PASS", "weird", "PASS"]), "NEEDS_CHANGES");
    }

    fn gate_event(event_type: &str, phase_id: Option<&str>, verdict: &str) -> AgentPlanEvent {
        AgentPlanEvent {
            id: String::new(),
            plan_id: String::new(),
            phase_id: phase_id.map(str::to_string),
            phase_index: None,
            phase_title: None,
            event_type: event_type.to_string(),
            actor: String::new(),
            category: String::new(),
            summary: String::new(),
            status_before: None,
            status_after: None,
            reason: None,
            verdict: None,
            task_id: None,
            clarification_attempt: None,
            payload_json: format!("{{\"verdict\":\"{}\"}}", verdict),
            created_at: String::new(),
        }
    }

    fn boundary_event(event_type: &str) -> AgentPlanEvent {
        gate_event(event_type, None, "")
    }

    #[test]
    fn count_consecutive_non_pass_stops_at_last_pass_and_scopes_phase() {
        // Planning: three NEEDS_CHANGES in a row → 3.
        let planning = vec![
            gate_event("planning_gate_result", None, "NEEDS_CHANGES"),
            gate_event("planning_gate_result", None, "NEEDS_CHANGES"),
            gate_event("planning_gate_result", None, "NEEDS_CHANGES"),
        ];
        assert_eq!(
            count_consecutive_non_pass(&planning, "planning_gate_result", "planning_started", None),
            3
        );

        // A PASS earlier resets the count — only rounds after it are counted.
        let with_pass = vec![
            gate_event("planning_gate_result", None, "NEEDS_CHANGES"),
            gate_event("planning_gate_result", None, "PASS"),
            gate_event("planning_gate_result", None, "NEEDS_CHANGES"),
            gate_event("planning_gate_result", None, "BLOCKED"),
        ];
        assert_eq!(
            count_consecutive_non_pass(&with_pass, "planning_gate_result", "planning_started", None),
            2
        );

        // A manual stop / restart resets the count — pre-stop rounds don't carry over
        // (regression guard for the false "9 rounds" escalation on a re-run).
        let restarted = vec![
            gate_event("planning_gate_result", None, "NEEDS_CHANGES"),
            gate_event("planning_gate_result", None, "NEEDS_CHANGES"),
            boundary_event("agent_plan_stopped"),
            gate_event("planning_gate_result", None, "NEEDS_CHANGES"),
        ];
        assert_eq!(
            count_consecutive_non_pass(&restarted, "planning_gate_result", "planning_started", None),
            1
        );
        let reamended = vec![
            gate_event("planning_gate_result", None, "NEEDS_CHANGES"),
            gate_event("planning_gate_result", None, "NEEDS_CHANGES"),
            boundary_event("planning_started"),
            gate_event("planning_gate_result", None, "NEEDS_CHANGES"),
            gate_event("planning_gate_result", None, "NEEDS_CHANGES"),
        ];
        assert_eq!(
            count_consecutive_non_pass(&reamended, "planning_gate_result", "planning_started", None),
            2
        );

        // Dev: only the named phase's gate results count.
        let dev = vec![
            gate_event("agent_phase_gate_result", Some("01-x"), "NEEDS_CHANGES"),
            gate_event("agent_phase_gate_result", Some("02-y"), "NEEDS_CHANGES"),
            gate_event("agent_phase_gate_result", Some("02-y"), "NEEDS_CHANGES"),
        ];
        assert_eq!(
            count_consecutive_non_pass(&dev, "agent_phase_gate_result", "agent_phase_started", Some("02-y")),
            2
        );
        assert_eq!(
            count_consecutive_non_pass(&dev, "agent_phase_gate_result", "agent_phase_started", Some("01-x")),
            1
        );
    }

    #[test]
    fn parse_verdict_ignores_clarification_instruction_echo() {
        // The clarification prompt we send lists every option; its echo must not
        // be misread as a PASS verdict.
        let output = "Return only this footer:\nVERDICT: PASS | NEEDS_CHANGES | BLOCKED\nSUMMARY: <one sentence>";
        assert_eq!(parse_verdict(output), None);
    }

    #[test]
    fn parse_verdict_accepts_structured_footer() {
        let output = "PHASE: 01-or-schedule\nVERDICT: NEEDS_CHANGES\nSUMMARY: missing screenshot";
        assert_eq!(parse_verdict(output).as_deref(), Some("NEEDS_CHANGES"));
    }

    #[test]
    fn idle_key_ignores_grok_static_chrome() {
        // Static idle chrome (context bar, task header, cursor, prompt, the
        // "always-approve" footer) must not change the idle key.
        let base = "Worker finished phase 01.\nTurn completed in 3.0s.";
        let with_chrome = format!(
            "{base}\n~/Documents/Workspace │ ⸬ 4 │ 156K / 200K │ 3 ✓\n▾ Tasks 4\n█\n│ ❯\nGrok Build · always-approve"
        );
        assert_eq!(
            normalize_terminal_snapshot_for_idle(&base),
            normalize_terminal_snapshot_for_idle(&with_chrome)
        );
    }

    #[test]
    fn idle_key_treats_working_spinner_as_activity() {
        // The animated working line (spinner + elapsed timer + run marker) MUST
        // change the idle key so a busy agent is never seen as idle / nudged.
        let base = "Reading files for the phase.";
        let working = format!("{base}\n⠋ Thinking… 0.3s 2m39s ⇣145k [✗]");
        assert_ne!(
            normalize_terminal_snapshot_for_idle(&base),
            normalize_terminal_snapshot_for_idle(&working)
        );
    }
}

#[cfg(test)]
mod store_tests {
    use super::*;
    use crate::db::migrations::run_migrations;
    use crate::db::Database;
    use std::sync::atomic::{AtomicU64, Ordering};

    static SEQ: AtomicU64 = AtomicU64::new(0);

    /// Unique temp dir. `Date::now`/random are unavailable in this environment, so make the
    /// suffix unique with the pid + a static counter.
    fn tmp_dir(tag: &str) -> PathBuf {
        let d = std::env::temp_dir().join(format!(
            "j1-p1-{}-{}-{}",
            tag,
            std::process::id(),
            SEQ.fetch_add(1, Ordering::Relaxed)
        ));
        std::fs::create_dir_all(&d).unwrap();
        d
    }

    /// Full in-process harness: `Database::open` runs migrations; `AppState::new` wires the rest
    /// (broadcast channels) with no live process, relay, or Tauri handle.
    fn test_state() -> (AppState, PathBuf) {
        let root = tmp_dir("state");
        let db = Database::open(&root.join("test.db")).expect("open db");
        (AppState::new(db), root)
    }

    fn input(run_type: Option<&str>, workspace: &str, plan_path: &str) -> CreateAgentPlanInput {
        CreateAgentPlanInput {
            run_type: run_type.map(|s| s.to_string()),
            title: None,
            workspace_path: workspace.to_string(),
            plan_path: plan_path.to_string(),
            worker_provider: "claude_code".to_string(),
            reviewer_provider: "claude_code".to_string(),
            brief: None,
            app_scope: None,
            docs_scope: None,
            reference_paths: None,
            worker_setup_commands: None,
            reviewer_setup_commands: None,
        }
    }

    /// Scaffold a minimal parseable plan at `plan_dir` (overview + one phase + one task).
    fn scaffold_plan(plan_dir: &Path, title: &str) {
        std::fs::create_dir_all(plan_dir.join("phases/01-x/tasks/01-y")).unwrap();
        std::fs::write(plan_dir.join("overview.md"), format!("# {}\n", title)).unwrap();
        std::fs::write(plan_dir.join("phases/01-x/overview.md"), "# Phase X\n").unwrap();
        std::fs::write(plan_dir.join("phases/01-x/tasks/01-y/prompt.md"), "# Task Y\n").unwrap();
    }

    // ── Preflight (task 02-01): the AppState harness is constructible ─────────────────
    #[test]
    fn appstate_harness_constructs() {
        let (state, root) = test_state();
        assert!(state.db.with_conn(|_c| Ok(())).is_ok());
        let _ = std::fs::remove_dir_all(&root);
    }

    // ── Test 1: parse_plan resolves an out-of-workspace absolute store path ───────────
    #[test]
    fn parse_plan_resolves_out_of_workspace_store_path() {
        let workspace = tmp_dir("ws");
        let store_plan = tmp_dir("store").join("INIT/plan");
        scaffold_plan(&store_plan, "My Plan");

        let parsed = parse_plan(
            workspace.to_string_lossy().as_ref(),
            store_plan.to_string_lossy().as_ref(),
        )
        .expect("store path must parse even though it is outside the workspace");
        assert_eq!(parsed.title, "My Plan");
        assert_eq!(parsed.phases.len(), 1);
        assert!(
            !parsed.plan_path.starts_with(&workspace),
            "resolved plan_path must NOT be under the workspace (proves the guard is relaxed)"
        );

        let _ = std::fs::remove_dir_all(&workspace);
        let _ = std::fs::remove_dir_all(store_plan.parent().unwrap().parent().unwrap());
    }

    // ── Test 2: parse_plan rejects `..` ──────────────────────────────────────────────
    #[test]
    fn parse_plan_rejects_dotdot() {
        let workspace = tmp_dir("ws");
        let ws = workspace.to_string_lossy().to_string();
        for traversal in ["../escape", "sub/../../escape"] {
            let err = parse_plan(&ws, traversal).expect_err("traversal must be rejected");
            assert!(err.contains(".."), "error should mention '..': {}", err);
        }
        let _ = std::fs::remove_dir_all(&workspace);
    }

    // ── Test 3: store path helpers ───────────────────────────────────────────────────
    #[test]
    fn initiative_plan_path_is_pure() {
        assert_eq!(
            settings_service::initiative_plan_path(Path::new("/store"), "abc"),
            PathBuf::from("/store/abc/plan")
        );
    }

    #[test]
    fn resolve_initiatives_dir_defaults_then_overrides() {
        let (state, root) = test_state();
        assert_eq!(
            settings_service::resolve_initiatives_dir(&state),
            PathBuf::from(settings_service::DEFAULT_INITIATIVES_DIR)
        );
        settings_service::set_setting(
            &state,
            settings_service::KEY_INITIATIVES_DIR.to_string(),
            "/tmp/j1-store".to_string(),
        )
        .unwrap();
        assert_eq!(
            settings_service::resolve_initiatives_dir(&state),
            PathBuf::from("/tmp/j1-store")
        );
        let _ = std::fs::remove_dir_all(&root);
    }

    // ── Test 4: linkage query via shared find_planning_initiative_id ──────────────────
    #[test]
    fn find_planning_initiative_id_links_by_plan_path() {
        let conn = rusqlite::Connection::open_in_memory().unwrap();
        run_migrations(&conn).unwrap();
        conn.execute(
            "INSERT INTO agent_plans (id, run_type, title, workspace_path, plan_path, worker_provider, reviewer_provider, initiative_id, initiative_status) \
             VALUES ('INIT1', 'planning', 't', '/w', '/store/INIT1/plan', 'claude_code', 'claude_code', 'INIT1', 'planning')",
            [],
        )
        .unwrap();
        assert_eq!(
            find_planning_initiative_id(&conn, "/store/INIT1/plan"),
            Some("INIT1".to_string())
        );
        assert_eq!(find_planning_initiative_id(&conn, "/store/UNKNOWN/plan"), None);
    }

    // ── Test 5: health sync via update_plan_status_and_health ─────────────────────────
    #[test]
    fn update_plan_status_and_health_syncs_all_transitions() {
        let conn = rusqlite::Connection::open_in_memory().unwrap();
        run_migrations(&conn).unwrap();
        conn.execute(
            "INSERT INTO agent_plans (id, run_type, title, workspace_path, plan_path, worker_provider, reviewer_provider, initiative_id, initiative_status) \
             VALUES ('P', 'development', 't', '/w', '/p', 'claude_code', 'claude_code', 'P', 'development')",
            [],
        )
        .unwrap();

        let read = |c: &rusqlite::Connection| -> (String, String, Option<String>) {
            c.query_row(
                "SELECT status, health, error FROM agent_plans WHERE id='P'",
                [],
                |row| Ok((row.get(0)?, row.get(1)?, row.get(2)?)),
            )
            .unwrap()
        };

        update_plan_status_and_health(&conn, "P", "blocked", Some("x")).unwrap();
        let (s, h, e) = read(&conn);
        assert_eq!((s.as_str(), h.as_str()), ("blocked", "blocked"));
        assert_eq!(e.as_deref(), Some("x"));

        update_plan_status_and_health(&conn, "P", "needs_attention", Some("y")).unwrap();
        let (s, h, _) = read(&conn);
        assert_eq!((s.as_str(), h.as_str()), ("needs_attention", "needs-attention"));

        update_plan_status_and_health(&conn, "P", "approved", None).unwrap();
        let (s, h, e) = read(&conn);
        assert_eq!((s.as_str(), h.as_str()), ("approved", "in-progress"));
        assert!(e.is_none(), "approve must clear error");
    }

    // ── Test 6: create_planning_run writes to the store (end-to-end wiring) ───────────
    #[tokio::test]
    async fn create_planning_run_writes_under_store() {
        let (state, root) = test_state();
        let workspace = tmp_dir("ws");
        let store = tmp_dir("store").canonicalize().unwrap();
        settings_service::set_setting(
            &state,
            settings_service::KEY_INITIATIVES_DIR.to_string(),
            store.to_string_lossy().to_string(),
        )
        .unwrap();

        let run = create_planning_run(
            &state,
            input(Some("planning"), workspace.to_string_lossy().as_ref(), "docs/plans/whatever"),
        )
        .expect("create_planning_run");
        let plan = run.plan;

        assert!(!plan.initiative_id.is_empty());
        assert_eq!(plan.initiative_status, "planning");
        assert_eq!(plan.health, "in-progress");
        // The wiring wrote the STORE path (not the advisory input path):
        assert_eq!(
            PathBuf::from(&plan.plan_path),
            settings_service::initiative_plan_path(&store, &plan.initiative_id)
        );
        assert!(Path::new(&plan.plan_path).is_dir(), "store plan dir must exist");
        assert!(
            !Path::new(&plan.plan_path).starts_with(&workspace),
            "plan_path must not be under the workspace"
        );

        let _ = std::fs::remove_dir_all(&workspace);
        let _ = std::fs::remove_dir_all(&store);
        let _ = std::fs::remove_dir_all(&root);
    }

    // ── Test 7: dev→planning initiative_id reuse (end-to-end) ─────────────────────────
    #[tokio::test]
    async fn create_plan_dev_reuses_planning_initiative_id() {
        let (state, root) = test_state();
        let workspace = tmp_dir("ws");
        let store = tmp_dir("store").canonicalize().unwrap();
        settings_service::set_setting(
            &state,
            settings_service::KEY_INITIATIVES_DIR.to_string(),
            store.to_string_lossy().to_string(),
        )
        .unwrap();

        let planning = create_planning_run(
            &state,
            input(Some("planning"), workspace.to_string_lossy().as_ref(), "docs/plans/whatever"),
        )
        .expect("create_planning_run")
        .plan;

        // Scaffold a parseable plan at the planning run's store plan_path so create_plan parses it.
        scaffold_plan(Path::new(&planning.plan_path), "E2E");

        let dev = create_plan(
            &state,
            input(Some("development"), workspace.to_string_lossy().as_ref(), &planning.plan_path),
        )
        .expect("create_plan (development)")
        .plan;

        assert_eq!(
            dev.initiative_id, planning.initiative_id,
            "development run must share the planning run's initiative_id"
        );
        assert_eq!(dev.initiative_status, "development");

        let _ = std::fs::remove_dir_all(&workspace);
        let _ = std::fs::remove_dir_all(&store);
        let _ = std::fs::remove_dir_all(&root);
    }
}
