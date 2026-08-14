use crate::db::migrations::health_from_status;
use crate::db::models::{
    AgentPlan, AgentPlanEvent, AgentPlanPhase, AgentPlanTask, CreateAgentPlanInput,
    CreateBriefingInput, CreateSessionInput, ValidationLens,
};
use crate::providers::CliProvider;
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

/// One changed file in a whole-tree diff (overhaul P7, D7). `additions`/`deletions` come from
/// `git diff HEAD --numstat`; `diff` is this file's unified-diff section (empty for a pure rename or
/// a binary file). `old_path` is set only on a rename.
#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct GitDiffFile {
    pub path: String,
    pub old_path: Option<String>,
    pub additions: u32,
    pub deletions: u32,
    pub binary: bool,
    pub diff: String,
}

/// The working-tree diff of a repo (overhaul P7, D7/D10/D11): tracked changes vs HEAD, with per-file
/// +/- counts and hunks in one round-trip. A non-repo path (or a clean repo) yields `clean:true` /
/// `files:[]` and a `None` `repo_root` (benign empty state, D11).
#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct GitDiffView {
    pub repo_root: Option<String>,
    pub branch: Option<String>,
    pub clean: bool,
    pub files: Vec<GitDiffFile>,
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
    reject_oneshot_plan_provider(&input.worker_provider)?;
    reject_oneshot_plan_provider(&input.reviewer_provider)?;
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
            "INSERT INTO agent_plans (id, run_type, title, workspace_path, plan_path, status, worker_session_id, reviewer_session_id, worker_provider, reviewer_provider, current_phase_id, current_phase_index, app_scope, reviewer_setup_commands, initiative_id, initiative_status, health, validation_config)
             VALUES (?1, 'development', ?2, ?3, ?4, 'draft', ?5, NULL, ?6, ?7, ?8, 0, ?9, ?10, ?11, 'development', 'in-progress', NULL)",
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
    reject_oneshot_plan_provider(&input.worker_provider)?;
    reject_oneshot_plan_provider(&input.reviewer_provider)?;
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
            "INSERT INTO agent_plans (id, run_type, title, workspace_path, plan_path, status, worker_session_id, reviewer_session_id, worker_provider, reviewer_provider, current_phase_index, brief, app_scope, docs_scope, reference_paths, reviewer_setup_commands, initiative_id, initiative_status, health, validation_config)
             VALUES (?1, 'planning', ?2, ?3, ?4, 'draft', ?5, NULL, ?6, ?7, 0, ?8, ?9, ?10, ?11, ?12, ?1, 'planning', 'in-progress', NULL)",
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

/// Create an Initiative *in briefing* (overhaul P4, D1). Mirrors [`create_planning_run`] but:
/// the row lands at `initiative_status='briefing'` with `worker_session_id` NULL (no planner yet),
/// its conversation is a **`kind='user'`** chat session (routed through the headless
/// `claude --print --resume` chat path, D2 — NOT a `kind='agent'` planner terminal), and both the
/// `<id>/plan` and `<id>/attachments` store dirs are created. No agent is spawned here — the first
/// briefing turn is sent later by the UI (Phase 04). Accept (Phase 02) flips this same row to
/// `planning` and starts the planner.
pub async fn create_briefing_run(
    state: &AppState,
    input: CreateBriefingInput,
) -> Result<AgentPlanRun, String> {
    reject_oneshot_plan_provider(&input.worker_provider)?;
    reject_oneshot_plan_provider(&input.reviewer_provider)?;
    let workspace = normalize_path(Path::new(&input.workspace_path))?;
    if !workspace.is_dir() {
        return Err("Workspace path is not a directory".to_string());
    }
    // A briefing Initiative's id IS its initiative id (D1); the row will flip briefing→planning
    // on the same id at Accept, so no fresh id is minted then.
    let initiative_id = Uuid::new_v4().to_string();
    let plan_id = initiative_id.clone();
    // Provision both store dirs: the plan dir (populated on the later planning kickoff) and the
    // attachments dir (📎/⤒ uploads land here via `initiative_upload_chunk`, D5).
    let store = settings_service::resolve_initiatives_dir(state);
    let plan = settings_service::initiative_plan_path(&store, &initiative_id);
    let attachments = settings_service::initiative_attachments_path(&store, &initiative_id);
    std::fs::create_dir_all(&plan)
        .map_err(|e| format!("Failed to create initiative plan dir {}: {}", plan.display(), e))?;
    std::fs::create_dir_all(&attachments).map_err(|e| {
        format!(
            "Failed to create initiative attachments dir {}: {}",
            attachments.display(),
            e
        )
    })?;
    let title = input
        .title
        .clone()
        .filter(|t| !t.trim().is_empty())
        .unwrap_or_else(|| "Briefing".to_string());

    // Placeholder session linked as briefing_session_id (kept for schema compatibility). The briefing
    // STEP was removed — creation now goes straight to planning via an immediate accept — so no agent
    // is spawned here.
    let chat_session = sessions::create_session(
        state,
        CreateSessionInput {
            provider: Some(input.worker_provider.clone()),
            model: input
                .model
                .clone()
                .or_else(|| default_model_for_provider(&input.worker_provider)),
            working_directory: Some(workspace.to_string_lossy().to_string()),
            title: Some(format!("Briefing - {}", title)),
            kind: Some("user".to_string()),
            setup_commands: None,
            tmux_session_name: None,
        },
    )?;

    state.db.with_conn(|conn| {
        conn.execute(
            // Fresh briefing Initiative: id == initiative_id (?1); plan_path (?4) is the store path;
            // initiative_status='briefing', worker/reviewer sessions NULL (no planner yet),
            // briefing_session_id (?7) links the chat session, health seeded 'in-progress'.
            "INSERT INTO agent_plans (id, run_type, title, workspace_path, plan_path, status, worker_session_id, reviewer_session_id, worker_provider, reviewer_provider, current_phase_index, brief, initiative_id, initiative_status, health, briefing_session_id, validation_config)
             VALUES (?1, 'planning', ?2, ?3, ?4, 'draft', NULL, NULL, ?5, ?6, 0, ?8, ?1, 'briefing', 'in-progress', ?7, NULL)",
            params![
                plan_id,
                title,
                workspace.to_string_lossy(),
                plan.to_string_lossy(),
                input.worker_provider,
                input.reviewer_provider,
                chat_session.id,
                input.brief.clone().unwrap_or_default(),
            ],
        )
        .map_err(|e| format!("Failed to create briefing run: {}", e))
    })?;

    append_event(
        state,
        &plan_id,
        None,
        "briefing_run_created",
        json!({ "briefingSessionId": chat_session.id }),
    )?;

    get_plan(state, &plan_id)
}

/// Compose the accepted brief (overhaul P4, D6): the draft, then an `## Attached files` section of
/// absolute store paths, then a `## Referenced host paths` section. Each trailing section is omitted
/// when its slice is empty (no dangling header). Pure — no state/DB/FS, so its test needs no fixtures.
/// (Absolute store paths are readable by the agent CLI directly — design §5b.)
pub fn compose_accepted_brief(
    draft: &str,
    attachment_paths: &[String],
    reference_paths: &[String],
) -> String {
    let mut out = draft.trim_end().to_string();
    if !attachment_paths.is_empty() {
        out.push_str("\n\n## Attached files\n");
        for p in attachment_paths {
            out.push_str(&format!("- {}\n", p));
        }
    }
    if !reference_paths.is_empty() {
        out.push_str("\n\n## Referenced host paths\n");
        for p in reference_paths {
            out.push_str(&format!("- {}\n", p));
        }
    }
    out
}

/// Flip the row `briefing → planning` and store the composed brief — the SAME row (D1). DB-only so it
/// is unit-testable against in-memory SQLite without spawning an agent (D8). `updated_at` uses the
/// same `datetime('now')` idiom as the other UPDATEs in this file.
fn apply_brief_acceptance(
    conn: &rusqlite::Connection,
    id: &str,
    composed: &str,
) -> Result<(), String> {
    conn.execute(
        "UPDATE agent_plans SET brief = ?2, initiative_status = 'planning', updated_at = datetime('now') WHERE id = ?1",
        params![id, composed],
    )
    .map_err(|e| format!("Failed to apply brief acceptance: {}", e))?;
    Ok(())
}

/// Record a ▤ Reference host path on a briefing Initiative (overhaul P4, D6): append `path` to the
/// row's `reference_paths` and re-normalize the whole block with the SAME `normalize_reference_paths`
/// the planner create uses (validation/dedup live there — not re-implemented here). Returns the
/// refreshed run.
pub fn add_initiative_reference_path(
    state: &AppState,
    id: &str,
    path: &str,
) -> Result<AgentPlanRun, String> {
    let plan = get_plan(state, id)?.plan;
    let workspace = Path::new(&plan.workspace_path);
    let combined = match plan.reference_paths.as_deref() {
        Some(existing) if !existing.trim().is_empty() => format!("{}\n{}", existing, path),
        _ => path.to_string(),
    };
    let normalized = normalize_reference_paths(workspace, Some(&combined))?;
    state.db.with_conn(|conn| {
        conn.execute(
            "UPDATE agent_plans SET reference_paths = ?2, updated_at = datetime('now') WHERE id = ?1",
            params![id, normalized],
        )
        .map_err(|e| format!("Failed to update reference paths: {}", e))
    })?;
    get_plan(state, id)
}

/// Accept a briefing brief (overhaul P4, D1/D4): the one transition out of `briefing`. Guards that the
/// row is in `briefing`, composes the accepted brief (draft + attachment refs + reference paths),
/// flips the **same** row `briefing → planning` storing that brief, provisions the T1 planner session
/// + plan dir on the same row (reusing `create_planning_run`'s provisioning primitives — NOT its
/// INSERT wrapper), and starts the planner via the existing **`start_planning_run`**. The compose +
/// DB flip happen before the terminal-attaching spawn so they are assertable without a live agent (D8).
pub async fn accept_brief(
    state: &AppState,
    id: &str,
    final_brief: Option<String>,
) -> Result<AgentPlanRun, String> {
    let plan = get_plan(state, id)?.plan;
    if plan.initiative_status != "briefing" {
        return Err("Initiative is not in briefing".to_string());
    }
    let store = settings_service::resolve_initiatives_dir(state);

    // Attachment absolute paths (files only, sorted for determinism); [] if the dir is absent.
    let attach_dir = settings_service::initiative_attachments_path(&store, id);
    let mut attachments: Vec<String> = Vec::new();
    if let Ok(entries) = std::fs::read_dir(&attach_dir) {
        for entry in entries.flatten() {
            if entry.file_type().map(|t| t.is_file()).unwrap_or(false) {
                attachments.push(entry.path().to_string_lossy().to_string());
            }
        }
    }
    attachments.sort();

    // Reference paths recorded during briefing (one per non-empty line).
    let refs: Vec<String> = plan
        .reference_paths
        .as_deref()
        .unwrap_or("")
        .lines()
        .map(str::trim)
        .filter(|l| !l.is_empty())
        .map(str::to_string)
        .collect();

    // Draft precedence: an explicit finalBrief (edited in the UI) wins; otherwise the brief the
    // interactive briefing agent wrote to brief.md; otherwise the original stored ask. This is what
    // carries the TERMINAL briefing conversation forward — the agent consolidates it into brief.md.
    let agent_brief = std::fs::read_to_string(Path::new(&plan.plan_path).join("brief.md"))
        .ok()
        .map(|s| s.trim().to_string())
        .filter(|s| !s.is_empty());
    let draft = final_brief
        .as_deref()
        .or(agent_brief.as_deref())
        .or(plan.brief.as_deref())
        .unwrap_or("");
    let composed = compose_accepted_brief(draft, &attachments, &refs);

    // Flip the SAME row briefing→planning and store the composed brief BEFORE any spawn (D8).
    state
        .db
        .with_conn(|conn| apply_brief_acceptance(conn, id, &composed))?;

    // Provision the planner on THIS row — reuse create_planning_run's primitives (309-336): ensure the
    // plan dir exists and create the T1 planner (kind='agent') session, then point worker_session_id
    // at it. `start_planning_run`'s INSERT wrapper is deliberately NOT called (would duplicate — D1).
    let plan_dir = settings_service::initiative_plan_path(&store, id);
    std::fs::create_dir_all(&plan_dir).map_err(|e| {
        format!(
            "Failed to create initiative plan dir {}: {}",
            plan_dir.display(),
            e
        )
    })?;
    // Persist the accepted brief as `brief.md` in the (shared) plan dir so EVERY agent on this
    // initiative can read it from disk — the planner, and crucially the development validation lenses
    // (their prompt points here). The DB `brief` column drives the planner prompt; this file is the
    // durable, agent-readable source of the original intent for planning AND review.
    if let Err(e) = write_brief_md(&plan_dir, &composed) {
        tracing::warn!(%id, error=%e, "Failed to write brief.md (non-fatal)");
    }
    let planner_session = sessions::create_session(
        state,
        CreateSessionInput {
            provider: Some(plan.worker_provider.clone()),
            model: default_model_for_provider(&plan.worker_provider),
            working_directory: Some(plan.workspace_path.clone()),
            title: Some(format!("T1 Planner - {}", plan.title)),
            kind: Some("agent".to_string()),
            setup_commands: None,
            tmux_session_name: None,
        },
    )?;
    state.db.with_conn(|conn| {
        conn.execute(
            "UPDATE agent_plans SET worker_session_id = ?2, updated_at = datetime('now') WHERE id = ?1",
            params![id, planner_session.id],
        )
        .map_err(|e| format!("Failed to set planner session: {}", e))
    })?;

    append_event(
        state,
        id,
        None,
        "brief_accepted",
        json!({ "plannerSessionId": planner_session.id }),
    )?;

    // Existing kickoff (start_planning_run, 819): attaches the planner terminal + sends the prompt
    // built from `user_brief = run.plan.brief` — now the composed accepted brief.
    start_planning_run(state.clone(), id.to_string()).await?;
    get_plan(state, id)
}

pub fn list_plans(
    state: &AppState,
    status: Option<String>,
    run_type: Option<String>,
    only_existing: bool,
) -> Result<Vec<AgentPlan>, String> {
    state.db.with_conn(|conn| {
        let sql = "SELECT id, run_type, title, workspace_path, plan_path, status, worker_session_id, reviewer_session_id, worker_provider, reviewer_provider, current_phase_id, current_phase_index, error, brief, app_scope, docs_scope, reference_paths, amend_brief, phase_run_mode, initiative_id, initiative_status, health, briefing_session_id, created_at, updated_at, validation_config FROM agent_plans
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
            "SELECT id, run_type, title, workspace_path, plan_path, status, worker_session_id, reviewer_session_id, worker_provider, reviewer_provider, current_phase_id, current_phase_index, error, brief, app_scope, docs_scope, reference_paths, amend_brief, phase_run_mode, initiative_id, initiative_status, health, briefing_session_id, created_at, updated_at, validation_config FROM agent_plans WHERE id = ?1",
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
    let phase_path = Path::new(&run.plan.plan_path)
        .join("phases")
        .join(&phase.phase_id);
    if crate::services::task_loop::phase_is_kloo_mode(&phase_path) {
        // Kloo-mode: no persistent T1, no worker_phase_prompt, no ready-curl.
        // Coordinator re-enters run_kloo_phase which reconciles first (D5).
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
        spawn_coordinator_loop(state.clone(), id.clone()).await;
        return get_plan(&state, &id);
    }
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

/// Validate an optional human comment/guidance attached to a run/resume.
///
/// - `None` or an empty string → `Ok(None)`: a pure re-run/resume — no event, no
///   injected feedback (brief: "Resume with no comment → resumes, no injected
///   feedback, no human_comment event").
/// - A non-empty string that trims to empty (whitespace-only) → `Err`.
/// - Otherwise → `Ok(Some(text))` with the ORIGINAL text preserved (trimmed only for
///   the blank check) so the guidance is recorded and injected verbatim.
fn validate_comment(comment: Option<&str>) -> Result<Option<String>, String> {
    match comment {
        None => Ok(None),
        Some(text) if text.is_empty() => Ok(None),
        Some(text) if text.trim().is_empty() => {
            Err("comment must not be whitespace-only".to_string())
        }
        Some(text) => Ok(Some(text.to_string())),
    }
}

/// Unified run/resume entry point: optionally record + inject a human comment, clear a
/// paused (`needs_attention`/`blocked`) run, then (re)start development at `phase_id`
/// via the existing `start_plan` path. Composes existing helpers — no new coordinator
/// logic (overhaul §Key decisions D1).
pub async fn run_initiative_from_phase(
    state: AppState,
    id: String,
    phase_id: Option<String>,
    phase_run_mode: Option<String>,
    comment: Option<String>,
) -> Result<AgentPlanRun, String> {
    // Reject a whitespace-only comment up front (early return before any side effect).
    let comment = validate_comment(comment.as_deref())?;
    let run = get_plan(&state, &id)?;

    if let Some(text) = comment.as_deref() {
        // Record the comment as a timeline event, then inject it into the run's active
        // session so the guidance enters the agent's context BEFORE the phase directive
        // start_plan sends — awaited sequentially, no interleave. `worker_session_id`
        // holds the planner session for a planning run and the worker/T1 session for a
        // development run, exactly as `send_feedback_to_worker` resolves it.
        append_event(
            &state,
            &id,
            phase_id.as_deref(),
            "human_comment",
            json!({ "text": text, "phaseId": phase_id.clone(), "mode": phase_run_mode.clone() }),
        )?;
        let session_id = run
            .plan
            .worker_session_id
            .clone()
            .ok_or_else(|| "Run has no active session".to_string())?;
        terminal::send_terminal_input(&state, session_id, format!("{}\r", text)).await?;
    }

    // Clear a paused run before delegating: start_plan early-returns for `blocked`
    // (and start_planning_run_inner for `blocked`), so move off the terminal status and
    // NULL the error (mirrors the error-clearing in amend_plan). `needs_attention` also
    // breaks the coordinator loop, so it must be cleared too. start_plan then re-enters
    // the full (re)start path and spawns the coordinator loop (overhaul §Key decisions D2).
    if matches!(run.plan.status.as_str(), "needs_attention" | "blocked") {
        let resume_status = if run.plan.run_type == "planning" {
            "planning_planner_running"
        } else {
            "needs_changes"
        };
        state.db.with_conn(|conn| {
            update_plan_status_and_health(conn, &id, resume_status, None)
                .map_err(|e| e.to_string())
        })?;
    }

    start_plan(state, id, phase_id, phase_run_mode).await
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
    // Write `stopped` before killing kloo so the loop's post-wait cancel
    // check cannot record Stop as a model failure (T2).
    state.db.with_conn(|conn| {
        conn.execute(
            "UPDATE agent_plans SET status = 'stopped', updated_at = datetime('now') WHERE id = ?1",
            params![id],
        )
        .map_err(|e| e.to_string())
    })?;
    crate::services::task_loop::kill_registered_kloo_child(state, &id);
    if let Some(session_id) = &run.plan.worker_session_id {
        let _ = terminal::kill_terminal_session(state, session_id).await;
        let _ = sessions::archive_session(state, session_id.clone()).await;
    }
    if let Some(session_id) = &run.plan.reviewer_session_id {
        let _ = terminal::kill_terminal_session(state, session_id).await;
        let _ = sessions::archive_session(state, session_id.clone()).await;
    }
    dispose_plan_review_sessions(state, &id).await;
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

/// Set/clear the Initiative's `validation_config` (overhaul P7, D1/D13). Mirrors
/// [`update_plan_app_scope`]: a `None`/empty value clears the column so `resolve_validation_lenses`
/// falls back to the default template. When `Some`, the JSON must parse as a `Vec<ValidationLens>`
/// (a malformed string is rejected, not silently written) and each lens's `provider` must be a
/// real `CliProvider` (D13 — a typo/hostile config can't smuggle an unrunnable provider into the
/// review fan-out). An empty array is allowed and stored as-is (→ resolve falls back to default).
pub fn update_plan_validation_config(
    state: &AppState,
    id: String,
    config: Option<String>,
) -> Result<AgentPlanRun, String> {
    // Treat blank/whitespace-only as a clear (→ NULL → default resolve).
    let config = config.filter(|s| !s.trim().is_empty());
    if let Some(json) = config.as_deref() {
        let lenses: Vec<ValidationLens> = serde_json::from_str(json)
            .map_err(|e| format!("Invalid validation config JSON: {}", e))?;
        for lens in &lenses {
            reject_non_review_lens(&lens.provider, &lens.name)?;
        }
    }
    // Ensure the plan exists (clear error if not) before writing.
    get_plan(state, &id)?;
    state.db.with_conn(|conn| {
        conn.execute(
            "UPDATE agent_plans SET validation_config = ?1, updated_at = datetime('now') WHERE id = ?2",
            params![config, id],
        )
        .map_err(|e| e.to_string())
    })?;
    append_event(
        state,
        &id,
        None,
        "agent_plan_validation_config_updated",
        json!({ "hasConfig": config.is_some() }),
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
        // Local-small must re-run the plan-check. Dispatching review here
        // would skip the gate the same way the old pre-check status write did.
        if is_local_small(&state, &run.plan.id)? {
            gate_planning_lenses(&state, &run, &PlanningCheckCtrl::live()).await?;
        } else {
            dispatch_planning_review(&state, &run).await?;
        }
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

/// One parsed `git diff --numstat -z` record.
struct NumstatRecord {
    additions: u32,
    deletions: u32,
    binary: bool,
    path: String,
    old_path: Option<String>,
}

/// Parse `git diff --numstat -z` output. The `-z` numstat format is NUL-terminated: a normal record
/// is `add\tdel\t<path>\0`; a rename is `add\tdel\t\0<old>\0<new>\0` (an empty path right after the
/// second tab signals the two following NUL fields are old/new). `-`/`-` counts mean a binary file.
fn parse_numstat_z(output: &str) -> Vec<NumstatRecord> {
    let fields: Vec<&str> = output.split('\0').collect();
    let mut records = Vec::new();
    let mut i = 0;
    while i < fields.len() {
        let field = fields[i];
        if field.is_empty() {
            i += 1;
            continue;
        }
        // field = "add\tdel\t<path-or-empty>"
        let mut parts = field.splitn(3, '\t');
        let add_tok = parts.next().unwrap_or("");
        let del_tok = parts.next().unwrap_or("");
        let path_part = parts.next().unwrap_or("");
        let binary = add_tok == "-" || del_tok == "-";
        let additions = add_tok.parse::<u32>().unwrap_or(0);
        let deletions = del_tok.parse::<u32>().unwrap_or(0);
        if path_part.is_empty() {
            // Rename: the next two NUL fields are the old and new paths.
            let old_path = fields.get(i + 1).map(|s| s.to_string());
            let new_path = fields.get(i + 2).map(|s| s.to_string()).unwrap_or_default();
            records.push(NumstatRecord {
                additions,
                deletions,
                binary,
                path: new_path,
                old_path,
            });
            i += 3;
        } else {
            records.push(NumstatRecord {
                additions,
                deletions,
                binary,
                path: path_part.to_string(),
                old_path: None,
            });
            i += 1;
        }
    }
    records
}

/// Split a unified `git diff` blob into per-file sections keyed by the file's new (`b/`) path. The
/// `+++ b/<path>` line is the authoritative path source when present (content changes); the
/// `diff --git a/… b/…` header is the fallback (pure renames / binary, which have no `+++`).
fn split_diff_sections(diff: &str) -> HashMap<String, String> {
    let mut sections: HashMap<String, String> = HashMap::new();
    let mut current_path: Option<String> = None;
    let mut current = String::new();
    let flush = |sections: &mut HashMap<String, String>,
                 path: &mut Option<String>,
                 buf: &mut String| {
        if let Some(p) = path.take() {
            sections.insert(p, std::mem::take(buf));
        } else {
            buf.clear();
        }
    };
    for line in diff.lines() {
        if line.starts_with("diff --git ") {
            flush(&mut sections, &mut current_path, &mut current);
            current.push_str(line);
            current.push('\n');
            // Fallback path from the header (overridden by a later `+++ b/` when present).
            current_path = line.split(" b/").nth(1).map(|s| s.to_string());
        } else {
            current.push_str(line);
            current.push('\n');
            if let Some(rest) = line.strip_prefix("+++ b/") {
                current_path = Some(rest.to_string());
            }
        }
    }
    flush(&mut sections, &mut current_path, &mut current);
    sections
}

/// Run `git diff HEAD <extra>`, falling back to `git diff --cached <extra>` when the repo has no
/// HEAD yet (no commits) — matching `get_workspace_file_diff`'s staged fallback (D11). Read-only.
fn git_diff_output(repo_root: &Path, extra: &[&str]) -> Result<String, String> {
    let mut head_args: Vec<&str> = vec!["diff", "HEAD"];
    head_args.extend_from_slice(extra);
    let output = std::process::Command::new("git")
        .args(&head_args)
        .current_dir(repo_root)
        .output()
        .map_err(|e| format!("Failed to run git diff: {}", e))?;
    if output.status.success() {
        return Ok(String::from_utf8_lossy(&output.stdout).to_string());
    }
    let mut cached_args: Vec<&str> = vec!["diff", "--cached"];
    cached_args.extend_from_slice(extra);
    let staged = std::process::Command::new("git")
        .args(&cached_args)
        .current_dir(repo_root)
        .output()
        .map_err(|e| format!("Failed to run staged git diff: {}", e))?;
    if staged.status.success() {
        return Ok(String::from_utf8_lossy(&staged.stdout).to_string());
    }
    Err(String::from_utf8_lossy(&staged.stderr).trim().to_string())
}

/// Whole-tree working-tree diff of the repo at `path` (overhaul P7, D7/D10/D11/D13). Path-keyed off a
/// session's `workingDirectory`: guards the path under the configured `files_root` (rejects traversal
/// — D13), resolves the repo via `git rev-parse --show-toplevel`, then runs `git diff HEAD --numstat`
/// (per-file +/- counts, rename-aware, `-` for binary) + `git diff HEAD` (hunks) in ONE call (no
/// N+1). Tracked changes vs HEAD only; untracked files excluded (D11). A non-repo/clean path returns
/// a benign empty view. Only ever runs READ git commands (`rev-parse`/`branch`/`diff`).
pub fn git_diff(state: &AppState, path: String) -> Result<GitDiffView, String> {
    let root = settings_service::resolve_files_root(state);
    let dir = resolve_workspace_file_path(&root, &path)?;
    let git_root = match git_root_for_path(&dir) {
        Ok(root) => root,
        // Not a git repository → benign empty state (D11), no error toast in the UI.
        Err(_) => {
            return Ok(GitDiffView {
                repo_root: None,
                branch: None,
                clean: true,
                files: vec![],
            });
        }
    };
    let branch = git_current_branch(&git_root).ok();

    let numstat = git_diff_output(&git_root, &["--numstat", "-z"])?;
    let records = parse_numstat_z(&numstat);
    let hunks = git_diff_output(&git_root, &[])?;
    let sections = split_diff_sections(&hunks);

    let files: Vec<GitDiffFile> = records
        .into_iter()
        .map(|rec| {
            let diff = sections.get(&rec.path).cloned().unwrap_or_default();
            GitDiffFile {
                // A file is only "binary" when numstat gave `-`/`-` AND there is no textual hunk.
                binary: rec.binary && diff.trim().is_empty(),
                path: rec.path,
                old_path: rec.old_path,
                additions: rec.additions,
                deletions: rec.deletions,
                diff,
            }
        })
        .collect();

    Ok(GitDiffView {
        repo_root: Some(git_root.to_string_lossy().to_string()),
        branch,
        clean: files.is_empty(),
        files,
    })
}

/// J1-owned write of the accepted brief. Thin wrapper so accept stays
/// non-fatal-on-error while tests can exercise the atomic path directly.
fn write_brief_md(plan_dir: &Path, contents: &str) -> Result<(), String> {
    crate::services::atomic_fs::write_atomic(&plan_dir.join("brief.md"), contents.as_bytes())
}

async fn spawn_coordinator_loop(state: AppState, plan_id: String) {
    let slots = state.coordinator_loops.clone();
    let id = plan_id.clone();
    let _ = crate::services::coordinator_flight::spawn_if_idle(&slots, id.clone(), move || {
        let state = state;
        let plan_id = id;
        async move {
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
        }
    });
}

/// Development `phase_worker_running` arm. Kloo-mode runs the per-task loop;
/// the `else` is the **commercial** persistent-worker + ready-curl path and
/// must stay (D2 / acceptance 7).
async fn run_development_worker_phase(state: &AppState, run: &AgentPlanRun) -> Result<(), String> {
    let phase = current_phase(run)?;
    let phase_path = Path::new(&run.plan.plan_path)
        .join("phases")
        .join(&phase.phase_id);
    if development_arm(&phase_path) == DevArm::KlooPreflight {
        if !run_phase_preflight(state, run, &phase).await? {
            return Ok(());
        }
        let outcome =
            crate::services::task_loop::run_kloo_phase(state, run, &phase).await?;
        return follow_kloo_outcome(state, run, &phase, outcome).await;
    }
    // COMMERCIAL PATH — persistent worker + ready-curl + three-lens review.
    // Do not change this branch; kloo-mode is the `if` above (D2 / D5).
    let session_id = run
        .plan
        .worker_session_id
        .clone()
        .ok_or_else(|| "Plan has no worker session".to_string())?;
    wait_for_worker_ready(state, &session_id, &run.plan.id, Some(&phase.phase_id)).await?;
    enter_phase_review(state, &run.plan.id, &phase).await
}

async fn follow_kloo_outcome(
    state: &AppState,
    run: &AgentPlanRun,
    phase: &AgentPlanPhase,
    outcome: crate::services::task_loop::PhaseLoopOutcome,
) -> Result<(), String> {
    use crate::services::task_loop::{followup_after_kloo_phase, KlooPhaseFollowup};
    match followup_after_kloo_phase(&outcome) {
        KlooPhaseFollowup::DispatchReview => {
            enter_phase_review(state, &run.plan.id, phase).await
        }
        KlooPhaseFollowup::NeedsAttention { task_id, route } => {
            state.db.with_conn(|conn| {
                update_plan_status_and_health(
                    conn,
                    &run.plan.id,
                    "needs_attention",
                    Some("task_failed"),
                )
                .map_err(|e| e.to_string())
            })?;
            append_event(
                state,
                &run.plan.id,
                Some(&phase.phase_id),
                "agent_phase_needs_attention",
                json!({ "reason": "task_failed", "taskId": task_id, "route": route }),
            )?;
            Ok(())
        }
        KlooPhaseFollowup::LeaveAsIs => Ok(()),
    }
}

async fn enter_phase_review(
    state: &AppState,
    plan_id: &str,
    phase: &AgentPlanPhase,
) -> Result<(), String> {
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
        state,
        plan_id,
        Some(&phase.phase_id),
        "agent_phase_worker_idle",
        json!({}),
    )?;
    let refreshed = get_plan(state, plan_id)?;
    dispatch_review(state, &refreshed, phase).await
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
                    // Stay on planning_planner_running until the check passes.
                    // Writing planning_review_running here used to let a
                    // restart mid-check fan out lenses on an unchecked plan.
                    append_event(&state, &plan_id, None, "planning_planner_ready", json!({}))?;
                    let refreshed = get_plan(&state, &plan_id)?;
                    gate_planning_lenses(&state, &refreshed, &PlanningCheckCtrl::live()).await?;
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
                run_development_worker_phase(&state, &run).await?;
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
            if !verdict_role_allowed(&role) {
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

/// A `verdict` report is legitimate from the plan's `reviewer` or any lens session.
/// Lens sessions are registered (in `agent_plan_review_sessions`) under their
/// *configured* lens name — `product`/`qa`/`lead` by default, but a run may configure
/// custom names (e.g. `pr`/`le`), so the set is open-ended. The only roles that may
/// NOT report a verdict are the `worker`/planner (it reports `ready`) and the `docs`
/// agent (it reports `done`). Gating by exclusion — rather than a hardcoded lens
/// allow-list — keeps custom lens names working instead of silently stranding them
/// (a custom-named lens could never land its verdict and the review would time out).
/// Scope/least-privilege rationale recorded in the Phase 00 review record:
/// `.johnnyone/initiatives/.../plan/phases/00-backend-run-from-phase/decisions.md`.
fn verdict_role_allowed(role: &str) -> bool {
    !matches!(role, "worker" | "docs")
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

/// The per-lens decision the fan-out needs to spawn one ephemeral reviewer: which provider/model to
/// launch and the lens's identity/rubric. Pure so it can be unit-pinned (D2/D6) without live agents.
#[derive(Debug, Clone, PartialEq)]
struct LensSpawnDescriptor {
    name: String,
    provider: String,
    model: Option<String>,
    prompt: Option<String>,
    vision: bool,
    blocking: bool,
}

/// Resolve one lens's spawn descriptor: its own `provider`, and `model` falling back to the
/// provider default when the lens leaves it unset (mirrors the ephemeral-reviewer model choice).
fn lens_spawn_descriptor(lens: &ValidationLens) -> LensSpawnDescriptor {
    let provider = lens.provider.clone();
    let model = lens
        .model
        .clone()
        .or_else(|| default_model_for_provider(&provider));
    LensSpawnDescriptor {
        name: lens.name.clone(),
        provider,
        model,
        prompt: lens.prompt.clone(),
        vision: lens.vision,
        blocking: lens.blocking,
    }
}

/// Build every lens's spawn descriptor, validating each `provider` against the real `CliProvider`
/// registry first (D13) — an unknown/typo provider is rejected up front rather than spawned. Pure:
/// the N-arity + per-lens provider/model test seam (A1/A2). Order is preserved so `descriptors[i]`
/// aligns with `lenses[i]` (and, after `join_all`, with `outcomes[i]`).
fn lens_spawn_descriptors(lenses: &[ValidationLens]) -> Result<Vec<LensSpawnDescriptor>, String> {
    lenses
        .iter()
        .map(|lens| {
            reject_non_review_lens(&lens.provider, &lens.name)?;
            Ok(lens_spawn_descriptor(lens))
        })
        .collect()
}

/// Kloo/Shell cannot be a planner, worker, or reviewer on a commercial plan.
fn reject_oneshot_plan_provider(provider: &str) -> Result<(), String> {
    match CliProvider::from_str(provider) {
        Some(CliProvider::Kloo) | Some(CliProvider::Shell) => Err(format!(
            "Provider '{}' is a oneshot executor, not a chat provider",
            provider
        )),
        _ => Ok(()),
    }
}

/// Unknown names keep the D13 "Unknown provider" error. Kloo/Shell parse as
/// known providers but cannot run a chat-style review lens.
fn reject_non_review_lens(provider: &str, lens_name: &str) -> Result<(), String> {
    match CliProvider::from_str(provider) {
        Some(CliProvider::Kloo) | Some(CliProvider::Shell) => Err(format!(
            "Provider '{}' is a oneshot executor, not a review lens",
            provider
        )),
        Some(_) => Ok(()),
        None => Err(format!(
            "Unknown provider '{}' for lens '{}'",
            provider, lens_name
        )),
    }
}

/// The lens-specific tail appended to a reviewer prompt (after the standard lens instructions,
/// before the report protocol): the lens's own rubric (`lens.prompt`, for a custom lens) and the
/// design-authority §3 vision clause when the lens is vision-capable (D14). Empty for a default,
/// non-vision lens — so the default template's prompt is byte-for-byte today's prompt plus nothing.
fn lens_prompt_extras(lens: &ValidationLens) -> String {
    let mut out = String::new();
    if let Some(p) = lens
        .prompt
        .as_deref()
        .map(str::trim)
        .filter(|s| !s.is_empty())
    {
        out.push_str("\n\nAdditional rubric for this lens (supplied by the validation config):\n");
        out.push_str(p);
    }
    if lens.vision {
        out.push_str(
            "\n\nYou may be running on a vision-capable model. If you cannot read a screenshot for \
this phase, approve on functional grounds rather than blocking.",
        );
    }
    out
}

fn lens_reviewer_prompt(
    state: &AppState,
    run: &AgentPlanRun,
    phase: &AgentPlanPhase,
    lens: &ValidationLens,
    session_id: &str,
) -> String {
    let lens_name = lens.name.as_str();
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
    let brief_path = Path::new(&run.plan.plan_path).join("brief.md");
    format!(
        "You are the {name} reviewer for phase {phase_id} of a development run. Run ONLY the {name} lens — do not run the other lenses.\n\n\
Read first: the ACCEPTED BRIEF (the user's finalized intent) at {brief} — validate the delivered work against it; methodology at {methodology}; all conventions under {conventions} (especially review-lenses.md — the development-review {name} checklist); the plan overview at {plan}; this phase's overview/status at {phase_path}; the task files under {tasks}. Then inspect the actual delivered work for this phase.\n\n\
Decide a single verdict for the {name} lens: PASS, NEEDS_CHANGES, or BLOCKED.{extras}{report}",
        name = lens_name,
        phase_id = phase.phase_id,
        brief = brief_path.display(),
        methodology = get("methodology_path"),
        conventions = get("conventions_path"),
        plan = get("plan_path"),
        phase_path = get("phase_path"),
        tasks = get("tasks_path"),
        extras = lens_prompt_extras(lens),
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

/// Gate promotion on the **blocking** lenses only (overhaul P7, D4). Pairs each outcome with its
/// lens by index (ordering preserved by `join_all`), filters to `blocking == true`, and feeds ONLY
/// those verdicts to the existing `merged_verdict` — which stays the single verdict authority (this
/// is a filter on its input, not a second gate). When there are no blocking lenses nothing can halt
/// promotion, so the gate is PASS (warn findings still surface via `merge_lens_body`). With the
/// default all-blocking template this is identical to feeding `merged_verdict` every outcome (A5).
fn gate_verdict_over_blocking(
    outcomes: &[(String, String, ReviewInsights)],
    lenses: &[ValidationLens],
) -> &'static str {
    let blocking: Vec<&str> = outcomes
        .iter()
        .zip(lenses.iter())
        .filter(|(_, lens)| lens.blocking)
        .map(|((_, verdict, _), _)| verdict.as_str())
        .collect();
    if blocking.is_empty() {
        return "PASS";
    }
    merged_verdict(blocking)
}

/// One-line "name: VERDICT" roll-up for the merged reviewer footer, marking warn-only lenses with a
/// `(warn)` tag so a reviewer reading the summary can see a non-PASS from a warn lens did not gate.
fn lens_summary_line(
    outcomes: &[(String, String, ReviewInsights)],
    lenses: &[ValidationLens],
) -> String {
    outcomes
        .iter()
        .enumerate()
        .map(|(i, (name, verdict, _))| {
            let blocking = lenses.get(i).map(|l| l.blocking).unwrap_or(true);
            if blocking {
                format!("{}: {}", name, verdict)
            } else {
                format!("{}: {} (warn)", name, verdict)
            }
        })
        .collect::<Vec<_>>()
        .join(", ")
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

/// Compose the development reviewer footer. Findings fold in **all** lenses (blocking + warn) via
/// `merge_lens_body` so warnings surface; the `VERDICT:` line is the **gate** verdict (blocking
/// lenses only, D4) — the promotion authority `handle_reviewer_output` routes on. The summary marks
/// warn lenses so a non-PASS from a warn lens is visibly non-gating.
fn merge_lens_outcomes(
    phase: &AgentPlanPhase,
    outcomes: &[(String, String, ReviewInsights)],
    lenses: &[ValidationLens],
    gate_verdict: &str,
) -> String {
    let (_summary, findings, _merged) = merge_lens_body(outcomes);
    let summary = lens_summary_line(outcomes, lenses);
    format!(
        "PHASE: {}\nSUMMARY: {}-lens review — {}\nFINDINGS:\n{}NEXT_STEPS:\n- none\nVERDICT: {}",
        phase.phase_id,
        outcomes.len(),
        summary,
        findings,
        gate_verdict
    )
}

/// Planning variant of `merge_lens_outcomes` — no `PHASE:` line (planning is a
/// whole-plan review); `handle_planning_reviewer_output` parses VERDICT/SUMMARY/
/// FINDINGS and ignores any phase header. Same blocking/warn split (D4).
fn merge_planning_lens_outcomes(
    outcomes: &[(String, String, ReviewInsights)],
    lenses: &[ValidationLens],
    gate_verdict: &str,
) -> String {
    let (_summary, findings, _merged) = merge_lens_body(outcomes);
    let summary = lens_summary_line(outcomes, lenses);
    format!(
        "SUMMARY: {}-lens plan review — {}\nFINDINGS:\n{}NEXT_STEPS:\n- none\nVERDICT: {}",
        outcomes.len(),
        summary,
        findings,
        gate_verdict
    )
}

/// Development phase review via lens fan-out (overhaul P7, D2): spawn one ephemeral reviewer per
/// **configured** lens (`resolve_validation_lenses`, default = product/qa/lead), each on its own
/// provider/model, await them with `join_all` (N-arity — no hand-unrolled join), gate over the
/// blocking lenses (D4), and route through the existing reviewer-output handler. EXTENDS the loop —
/// the spawn/dispose/merge/verdict machinery is unchanged, only the arity and per-lens plumbing.
async fn run_lens_fanout_review(
    state: &AppState,
    run: &AgentPlanRun,
    phase: &AgentPlanPhase,
) -> Result<(), String> {
    let lenses = resolve_validation_lenses(run);
    let descriptors = lens_spawn_descriptors(&lenses)?; // validates each provider (D13)
    let working_dir = Some(run.plan.workspace_path.clone());
    let plan_id = run.plan.id.clone();
    // Lenses POST their review body from a file under here; make sure it exists.
    let _ = std::fs::create_dir_all(std::env::temp_dir().join("johnnyone-reviews"));

    // Spawn one reviewer per lens (they run concurrently once their prompts are sent).
    let mut sessions: Vec<(String, String)> = Vec::new(); // (lens name, session_id)
    for (lens, desc) in lenses.iter().zip(descriptors.iter()) {
        let sid = spawn_ephemeral_agent(
            state,
            &plan_id,
            &desc.name,
            &desc.provider,
            desc.model.clone(),
            working_dir.clone(),
            |sid| lens_reviewer_prompt(state, run, phase, lens, sid),
        )
        .await?;
        let _ = append_event(
            state,
            &plan_id,
            Some(&phase.phase_id),
            "agent_lens_review_started",
            json!({ "lens": desc.name }),
        );
        sessions.push((desc.name.clone(), sid));
    }

    // Collect every verdict concurrently — order preserved so results[i] aligns with lenses[i].
    let futures = sessions
        .iter()
        .map(|(name, sid)| wait_for_lens_verdict(state, sid, name));
    let results = futures_util::future::join_all(futures).await;

    // Tear the reviewers down regardless of outcome.
    for (_, sid) in &sessions {
        dispose_ephemeral_agent(state, sid).await;
    }

    let mut outcomes: Vec<(String, String, ReviewInsights)> = Vec::new();
    for ((name, _sid), res) in sessions.iter().zip(results.into_iter()) {
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
        outcomes.push((name.clone(), verdict, insights));
    }

    let gate = gate_verdict_over_blocking(&outcomes, &lenses);
    let merged = merge_lens_outcomes(phase, &outcomes, &lenses, gate);
    handle_reviewer_output(state, run, phase, &merged).await
}

fn planning_lens_reviewer_prompt(
    state: &AppState,
    run: &AgentPlanRun,
    lens: &ValidationLens,
    session_id: &str,
) -> String {
    let lens_name = lens.name.as_str();
    let values = planning_template_values(state, run);
    let get = |k: &str| {
        values
            .iter()
            .find(|(kk, _)| *kk == k)
            .map(|(_, v)| v.clone())
            .unwrap_or_default()
    };
    let brief_path = Path::new(&run.plan.plan_path).join("brief.md");
    format!(
        "You are the {name} reviewer for a PLAN (planning run). Run ONLY the {name} lens — do not run the other lenses.\n\n\
Read first: the ACCEPTED BRIEF (the user's finalized intent) at {brief} — the plan must not narrow, drift from, or exceed it; methodology at {methodology}; all conventions under {conventions} (especially review-lenses.md — the planning-review {name} checklist); then read the plan at {plan_output}. Judge whether the PLAN itself is ready along the {name} dimension AND faithful to the brief (e.g. Product: clear scope, mocks, and a screens-to-verify inventory; QA: testable acceptance criteria per phase, not narrower than the brief; Lead: a sound, reuse-aware, secure approach with phases sized right).\n\n\
Decide a single verdict for the {name} lens: PASS, NEEDS_CHANGES, or BLOCKED.{extras}{report}",
        name = lens_name,
        brief = brief_path.display(),
        methodology = get("methodology_path"),
        conventions = get("conventions_path"),
        plan_output = get("plan_output_path"),
        extras = lens_prompt_extras(lens),
        report = lens_report_instruction(session_id, lens_name),
    )
}

/// Planning review via lens fan-out (overhaul P7, D2): one ephemeral reviewer per **configured**
/// lens against the whole plan, each on its own provider/model, awaited with `join_all`, gated over
/// the blocking lenses (D4). Mirrors `run_lens_fanout_review` but has no phase (planning is a
/// whole-plan review). EXTENDS the loop — only the arity and per-lens plumbing changed.
async fn run_planning_lens_fanout_review(
    state: &AppState,
    run: &AgentPlanRun,
) -> Result<(), String> {
    let lenses = resolve_validation_lenses(run);
    let descriptors = lens_spawn_descriptors(&lenses)?; // validates each provider (D13)
    let working_dir = Some(run.plan.workspace_path.clone());
    let plan_id = run.plan.id.clone();
    let _ = std::fs::create_dir_all(std::env::temp_dir().join("johnnyone-reviews"));

    let mut sessions: Vec<(String, String)> = Vec::new(); // (lens name, session_id)
    for (lens, desc) in lenses.iter().zip(descriptors.iter()) {
        let sid = spawn_ephemeral_agent(
            state,
            &plan_id,
            &desc.name,
            &desc.provider,
            desc.model.clone(),
            working_dir.clone(),
            |sid| planning_lens_reviewer_prompt(state, run, lens, sid),
        )
        .await?;
        let _ = append_event(
            state,
            &plan_id,
            None,
            "agent_lens_review_started",
            json!({ "lens": desc.name }),
        );
        sessions.push((desc.name.clone(), sid));
    }

    let futures = sessions
        .iter()
        .map(|(name, sid)| wait_for_lens_verdict(state, sid, name));
    let results = futures_util::future::join_all(futures).await;

    for (_, sid) in &sessions {
        dispose_ephemeral_agent(state, sid).await;
    }

    let mut outcomes: Vec<(String, String, ReviewInsights)> = Vec::new();
    for ((name, _sid), res) in sessions.iter().zip(results.into_iter()) {
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
        outcomes.push((name.clone(), verdict, insights));
    }

    let gate = gate_verdict_over_blocking(&outcomes, &lenses);
    let merged = merge_planning_lens_outcomes(&outcomes, &lenses, gate);
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

/// Mark a plan as `blocked` because its agent asked for a human, returning the
/// status it had before so [`clear_plan_blocked`] can put it back.
///
/// Without this the run keeps its `*_running` status while the agent sits idle at a
/// prompt, so every status reader — `listAgentPlans`, `getAgentPlan`, the console —
/// reports "running" indefinitely. That is exactly how run `4187e055` looked busy
/// for three days after reporting `blocked` (its `agent_blocked` event recorded a
/// `None -> None` status transition). The status must tell the truth, not just the
/// event log and a best-effort Discord ping.
fn mark_plan_blocked_conn(
    conn: &rusqlite::Connection,
    plan_id: &str,
    reason: &str,
) -> rusqlite::Result<String> {
    let previous: String = conn.query_row(
        "SELECT status FROM agent_plans WHERE id = ?1",
        params![plan_id],
        |row| row.get(0),
    )?;
    update_plan_status_and_health(conn, plan_id, "blocked", Some(reason))?;
    Ok(previous)
}

/// Restore the status [`mark_plan_blocked_conn`] replaced. Guarded on the plan still
/// being `blocked`, so a human who stopped or closed the run while it waited is
/// never overridden by the resuming agent.
fn clear_plan_blocked_conn(
    conn: &rusqlite::Connection,
    plan_id: &str,
    previous: &str,
) -> rusqlite::Result<usize> {
    conn.execute(
        "UPDATE agent_plans SET status = ?1, health = ?2, error = NULL, updated_at = datetime('now') \
         WHERE id = ?3 AND status = 'blocked'",
        params![previous, health_from_status(previous), plan_id],
    )
}

async fn mark_plan_blocked(state: &AppState, plan_id: &str, reason: &str) -> Option<String> {
    let plan_id = plan_id.to_string();
    let reason = reason.to_string();
    state
        .db
        .with_conn(move |conn| {
            mark_plan_blocked_conn(conn, &plan_id, &reason).map_err(|e| e.to_string())
        })
        .ok()
}

/// Restore the status [`mark_plan_blocked`] replaced, once the human has replied and
/// the agent has resumed.
async fn clear_plan_blocked(state: &AppState, plan_id: &str, previous: Option<&str>) {
    let Some(previous) = previous else { return };
    let plan_id = plan_id.to_string();
    let previous = previous.to_string();
    let _ = state.db.with_conn(move |conn| {
        clear_plan_blocked_conn(conn, &plan_id, &previous).map_err(|e| e.to_string())
    });
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
    // produces the `ready` report below. Because that wait is unbounded, the plan's
    // status is flipped to `blocked` for its duration: an unbounded wait under a
    // `*_running` status is indistinguishable from a healthy run, which is how a
    // blocked run once sat unnoticed for three days.
    let mut blocked = false;
    let mut status_before_block: Option<String> = None;
    loop {
        if take_agent_report_kind(state, session_id, "ready").await.is_some() {
            if blocked {
                // Human replied and the agent resumed — hand the status back before
                // returning, so the caller's `status = 'phase_worker_running'`
                // guarded transition still matches.
                clear_plan_blocked(state, plan_id, status_before_block.as_deref()).await;
                let _ = append_event(state, plan_id, phase_id, "agent_unblocked", json!({}));
                tracing::info!(session_id, plan_id, "agent unblocked — resuming");
            }
            return Ok(());
        }
        if take_agent_report_kind(state, session_id, "blocked").await.is_some() {
            if !blocked {
                blocked = true;
                tracing::warn!(session_id, plan_id, "agent reported blocked — needs a human");
                status_before_block =
                    mark_plan_blocked(state, plan_id, "agent reported blocked — needs a human")
                        .await;
                let _ = append_event(
                    state,
                    plan_id,
                    phase_id,
                    "agent_blocked",
                    json!({ "statusBefore": status_before_block }),
                );
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

pub(crate) struct PlanningCheckCtrl<'a> {
    pub host: Option<&'a dyn crate::services::plan_check::PlanCheckHost>,
    pub stage_budget_ms: u64,
    pub send_planner: bool,
    /// Injected elapsed-ms clock for the stage ceiling. `None` uses wall Instant.
    pub elapsed_ms: Option<std::sync::Arc<dyn Fn() -> u64 + Send + Sync>>,
}

impl PlanningCheckCtrl<'static> {
    fn live() -> Self {
        Self {
            host: None,
            stage_budget_ms: crate::services::plan_check::MAX_PLANNING_CHECK_MS,
            send_planner: true,
            elapsed_ms: None,
        }
    }
}

pub(crate) fn is_local_small(state: &AppState, plan_id: &str) -> Result<bool, String> {
    let raw: Option<String> = state.db.with_conn(|conn| {
        conn.query_row(
            "SELECT executor_config FROM agent_plans WHERE id = ?1",
            params![plan_id],
            |row| row.get::<_, Option<String>>(0),
        )
        .map_err(|e| format!("read executor_config: {e}"))
    })?;
    Ok(executor_mode_is_local_small(raw.as_deref()))
}

pub(crate) fn executor_mode_is_local_small(raw: Option<&str>) -> bool {
    let Some(raw) = raw.map(str::trim).filter(|s| !s.is_empty()) else {
        return false;
    };
    serde_json::from_str::<serde_json::Value>(raw)
        .ok()
        .and_then(|v| {
            v.get("mode")
                .and_then(|m| m.as_str())
                .map(|m| m == "local-small")
        })
        .unwrap_or(false)
}

#[cfg(test)]
pub(crate) fn set_executor_config(
    state: &AppState,
    plan_id: &str,
    json: &str,
) -> Result<(), String> {
    state.db.with_conn(|conn| {
        conn.execute(
            "UPDATE agent_plans SET executor_config = ?1, updated_at = datetime('now') WHERE id = ?2",
            params![json, plan_id],
        )
        .map_err(|e| e.to_string())?;
        Ok(())
    })
}

fn plan_check_json_path(state: &AppState, run: &AgentPlanRun) -> PathBuf {
    settings_service::resolve_initiatives_dir(state)
        .join(&run.plan.initiative_id)
        .join("runs")
        .join(&run.plan.id)
        .join("plan-check.json")
}

fn check_findings_path(plan_path: &str) -> PathBuf {
    Path::new(plan_path).join("check-findings.json")
}

struct CheckPlanCall<'a> {
    only_phase: Option<&'a str>,
    skip_execute: bool,
    previous_skipped: &'a [String],
    host: Option<&'a dyn crate::services::plan_check::PlanCheckHost>,
    tokens: bool,
    tokens_only: bool,
    run: Option<&'a crate::services::task_state::TaskRunFile>,
    tokens_budget_ms: Option<u64>,
}

async fn run_check_plan_blocking(
    plan_path: &Path,
    workspace: &Path,
    call: CheckPlanCall<'_>,
) -> Result<crate::services::plan_check::PlanCheckReport, String> {
    if call.host.is_some() {
        let mut opts = crate::services::plan_check::CheckPlanOpts::shape_only(call.run);
        opts.host = call.host;
        opts.only_phase = call.only_phase;
        opts.skip_execute = call.skip_execute;
        opts.previous_skipped = call.previous_skipped;
        apply_check_mode(&mut opts, call.tokens, call.tokens_only);
        opts.tokens_budget_ms = call.tokens_budget_ms;
        return Ok(crate::services::plan_check::check_plan_with(
            plan_path, workspace, &opts,
        ));
    }
    let plan_path = plan_path.to_path_buf();
    let workspace = workspace.to_path_buf();
    let only = call.only_phase.map(str::to_string);
    let prev = call.previous_skipped.to_vec();
    let tokens = call.tokens;
    let tokens_only = call.tokens_only;
    let skip_execute = call.skip_execute;
    let tokens_budget_ms = call.tokens_budget_ms;
    let run_owned = call.run.cloned();
    tokio::task::spawn_blocking(move || {
        let mut opts = crate::services::plan_check::CheckPlanOpts::shape_only(run_owned.as_ref());
        let only_ref = only.as_deref();
        opts.only_phase = only_ref;
        opts.tokens_budget_ms = tokens_budget_ms;
        opts.skip_execute = skip_execute;
        opts.previous_skipped = &prev;
        apply_check_mode(&mut opts, tokens, tokens_only);
        crate::services::plan_check::check_plan_with(&plan_path, &workspace, &opts)
    })
    .await
    .map_err(|e| format!("plan-check join: {e}"))
}

fn apply_check_mode(
    opts: &mut crate::services::plan_check::CheckPlanOpts<'_>,
    tokens: bool,
    tokens_only: bool,
) {
    opts.tokens_only = tokens_only;
    if tokens_only {
        opts.execute = false;
        opts.tokens = true;
    } else if tokens {
        opts.execute = true;
        opts.tokens = true;
    } else {
        // Always enter execute_phase so skip_execute can emit verify_not_executed.
        opts.execute = true;
        opts.tokens = false;
    }
}

struct CheckFindingsSummary {
    prompt: String,
    preview: Vec<serde_json::Value>,
    counts_by_rule: serde_json::Map<String, serde_json::Value>,
}

fn summarize_check_findings(
    items: &[crate::services::plan_check::PlanCheckItem],
    cap: usize,
) -> CheckFindingsSummary {
    let mut counts: std::collections::BTreeMap<String, usize> = std::collections::BTreeMap::new();
    for i in items {
        *counts.entry(i.rule.clone()).or_default() += 1;
    }
    let counts_by_rule: serde_json::Map<String, serde_json::Value> = counts
        .iter()
        .map(|(k, v)| (k.clone(), json!(*v)))
        .collect();
    let preview: Vec<serde_json::Value> = items
        .iter()
        .take(cap)
        .map(|i| {
            json!({
                "task_id": i.task_id,
                "rule": i.rule,
                "detail": i.detail,
            })
        })
        .collect();
    let mut prompt = String::new();
    prompt.push_str("Counts by rule:\n");
    for (rule, n) in &counts {
        prompt.push_str(&format!("- {rule}: {n}\n"));
    }
    prompt.push('\n');
    for i in items.iter().take(cap) {
        prompt.push_str(&format!(
            "- [{}] {} — {}\n",
            i.rule,
            i.task_id.as_deref().unwrap_or("(plan)"),
            i.detail
        ));
    }
    if items.len() > cap {
        prompt.push_str(&format!(
            "… and {} more (see check-findings.json)\n",
            items.len() - cap
        ));
    }
    CheckFindingsSummary {
        prompt,
        preview,
        counts_by_rule,
    }
}

fn persist_check_findings(
    plan_path: &str,
    report: &crate::services::plan_check::PlanCheckReport,
) -> Result<(), String> {
    let items: Vec<serde_json::Value> = report
        .items
        .iter()
        .map(|i| {
            json!({
                "task_id": i.task_id,
                "rule": i.rule,
                "detail": i.detail,
            })
        })
        .collect();
    let bytes = serde_json::to_vec_pretty(&items).map_err(|e| format!("serialize findings: {e}"))?;
    crate::services::atomic_fs::write_atomic(&check_findings_path(plan_path), &bytes)
}

fn blocking_items_are_only_env_tokens(report: &crate::services::plan_check::PlanCheckReport) -> bool {
    let blocking: Vec<_> = report.items.iter().filter(|i| i.blocking).collect();
    !blocking.is_empty()
        && blocking.iter().all(|i| {
            i.rule == crate::services::plan_check::RULE_TOKENS_UNAVAILABLE
                && !crate::services::plan_check::is_tokens_budget_skip(&i.rule, &i.detail)
        })
}

/// After `planning_planner_ready`: local-small runs plan-check; commercial dispatches review.
pub(crate) async fn gate_planning_lenses(
    state: &AppState,
    run: &AgentPlanRun,
    ctrl: &PlanningCheckCtrl<'_>,
) -> Result<(), String> {
    if !is_local_small(state, &run.plan.id)? {
        return dispatch_planning_review(state, run).await;
    }

    let plan_path = PathBuf::from(&run.plan.plan_path);
    let workspace = PathBuf::from(&run.plan.workspace_path);
    let prev_path = plan_check_json_path(state, run);
    let previous_skipped = crate::services::plan_check::load_plan_check(&prev_path)
        .map(|r| r.skipped_ids)
        .unwrap_or_default();

    let mut phase_ids: Vec<String> = crate::services::plan_check::list_sorted_dirs(
        &plan_path.join("phases"),
    )
    .into_iter()
    .map(|p| {
        p.file_name()
            .and_then(|n| n.to_str())
            .unwrap_or("")
            .to_string()
    })
    .collect();
    if phase_ids.is_empty() {
        phase_ids.push(String::new());
    }

    let mut acc = crate::services::plan_check::PlanCheckReport::empty();
    let stage_start = Instant::now();
    for phase_id in &phase_ids {
        let elapsed = ctrl
            .elapsed_ms
            .as_ref()
            .map(|f| f())
            .unwrap_or_else(|| stage_start.elapsed().as_millis() as u64);
        let skip = elapsed >= ctrl.stage_budget_ms;
        let only = if phase_id.is_empty() {
            None
        } else {
            Some(phase_id.as_str())
        };
        let report = run_check_plan_blocking(
            &plan_path,
            &workspace,
            CheckPlanCall {
                only_phase: only,
                skip_execute: skip,
                previous_skipped: &previous_skipped,
                host: ctrl.host,
                tokens: false,
                tokens_only: false,
                run: None,
                tokens_budget_ms: None,
            },
        )
        .await?;

        acc.items.extend(report.items.clone());
        acc.verify_executed += report.verify_executed;
        acc.verify_skipped += report.verify_skipped;
        acc.warm_ms += report.warm_ms;
        acc.verify_ms += report.verify_ms;
        acc.shape_ms += report.shape_ms;
        acc.tasks_checked += report.tasks_checked;
        acc.skipped_ids.extend(report.skipped_ids.clone());
        acc.phases.extend(report.phases.clone());

        if !phase_id.is_empty() {
            append_event(
                state,
                &run.plan.id,
                Some(phase_id),
                "planning_check_phase",
                json!({
                    "executed": report.verify_executed,
                    "skipped": report.verify_skipped,
                }),
            )?;
        }
    }

    if acc.tasks_checked > crate::services::plan_check::MAX_TASKS_TOTAL {
        acc.items.push(crate::services::plan_check::task_count_total_item(
            acc.tasks_checked,
        ));
    }

    // Tokens once over the whole plan (no second shape walk). Same stage ceiling.
    {
        let elapsed = ctrl
            .elapsed_ms
            .as_ref()
            .map(|f| f())
            .unwrap_or_else(|| stage_start.elapsed().as_millis() as u64);
        if elapsed >= ctrl.stage_budget_ms {
            acc.items.push(crate::services::plan_check::item(
                None,
                crate::services::plan_check::RULE_TOKENS_UNAVAILABLE,
                "stage budget exhausted before tokens",
            ));
        } else {
        let tok = run_check_plan_blocking(
            &plan_path,
            &workspace,
            CheckPlanCall {
                only_phase: None,
                skip_execute: true,
                previous_skipped: &previous_skipped,
                host: ctrl.host,
                tokens: true,
                tokens_only: true,
                run: None,
                tokens_budget_ms: Some(ctrl.stage_budget_ms.saturating_sub(elapsed)),
            },
        )
        .await?;
        for item in tok.items {
            if item.rule == crate::services::plan_check::RULE_PROMPT_EXCEEDS_CONTEXT
                || item.rule == crate::services::plan_check::RULE_TOKENS_UNAVAILABLE
            {
                acc.items.push(item);
            }
        }
        }
    }

    acc.passed = !acc.items.iter().any(|i| i.blocking);
    if let Some(parent) = prev_path.parent() {
        std::fs::create_dir_all(parent)
            .map_err(|e| format!("create plan-check dir: {e}"))?;
    }
    crate::services::plan_check::save_plan_check(&prev_path, &acc)?;
    persist_check_findings(&run.plan.plan_path, &acc)?;

    if acc.passed {
        append_event(
            state,
            &run.plan.id,
            None,
            "planning_check_passed",
            json!({
                "verifyExecuted": acc.verify_executed,
                "verifySkipped": acc.verify_skipped,
            }),
        )?;
        return dispatch_planning_review(state, run).await;
    }

    if blocking_items_are_only_env_tokens(&acc) {
        let reason = "kloo tokens is unavailable — environment, not a plan defect".to_string();
        state.db.with_conn(|conn| {
            update_plan_status_and_health(conn, &run.plan.id, "needs_attention", Some(&reason))
                .map_err(|e| e.to_string())
        })?;
        return append_event(
            state,
            &run.plan.id,
            None,
            "planning_needs_attention",
            json!({ "reason": reason }),
        );
    }

    let summary = summarize_check_findings(&acc.items, 8);
    append_event(
        state,
        &run.plan.id,
        None,
        "planning_check_failed",
        json!({
            "verdict": "NEEDS_CHANGES",
            "total": acc.items.len(),
            "countsByRule": summary.counts_by_rule,
            "items": summary.preview,
            "findingsPath": check_findings_path(&run.plan.plan_path).display().to_string(),
        }),
    )?;

    let refreshed = get_plan(state, &run.plan.id)?;
    let round = consecutive_non_pass_planning_rounds(&refreshed);
    if round >= MAX_REVISION_ROUNDS {
        let reason = format!(
            "Plan still not passing after {round} plan-check rounds. Coordinator paused — needs a human decision."
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
            params!["plan-check failed", run.plan.id],
        )
        .map_err(|e| e.to_string())
    })?;

    if ctrl.send_planner {
        send_plan_check_findings_to_planner(state, run, &acc).await?;
    }
    Ok(())
}

async fn send_plan_check_findings_to_planner(
    state: &AppState,
    run: &AgentPlanRun,
    report: &crate::services::plan_check::PlanCheckReport,
) -> Result<(), String> {
    let Some(session_id) = run.plan.worker_session_id.clone() else {
        return Err("Planning run has no planner session".to_string());
    };
    let findings_path = check_findings_path(&run.plan.plan_path);
    let summary = summarize_check_findings(&report.items, 8);
    let mut body = String::from("Plan-check failed. Fix every item (task_id + rule).\n\n");
    body.push_str(&summary.prompt);
    body.push_str(&format!(
        "\nFull structured findings: {}\n",
        findings_path.display()
    ));
    let prompt = append_worker_report(run, body);
    terminal::send_terminal_input(state, session_id, format!("{}\r", prompt)).await?;
    append_event(
        state,
        &run.plan.id,
        None,
        "planning_feedback_sent_to_planner",
        json!({ "reason": "plan_check_failed" }),
    )
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum DevArm {
    KlooPreflight,
    PersistentWorker,
}

pub(crate) fn development_arm(phase_path: &Path) -> DevArm {
    if crate::services::task_loop::phase_is_kloo_mode(phase_path) {
        DevArm::KlooPreflight
    } else {
        DevArm::PersistentWorker
    }
}

/// Returns `false` when kloo must not spawn (preflight failed).
pub(crate) async fn run_phase_preflight(
    state: &AppState,
    run: &AgentPlanRun,
    phase: &AgentPlanPhase,
) -> Result<bool, String> {
    run_phase_preflight_with(state, run, phase, None, None).await
}

pub(crate) async fn run_phase_preflight_with(
    state: &AppState,
    run: &AgentPlanRun,
    phase: &AgentPlanPhase,
    host: Option<&dyn crate::services::plan_check::PlanCheckHost>,
    tokens_budget_ms: Option<u64>,
) -> Result<bool, String> {
    let initiatives = settings_service::resolve_initiatives_dir(state);
    let runs_dir = settings_service::initiative_runs_path(
        &initiatives,
        &run.plan.initiative_id,
        &run.plan.id,
        &phase.phase_id,
    );
    let tasks_path = crate::services::task_state::tasks_json_path(&runs_dir);
    let task_run = crate::services::task_state::load_tasks(
        &tasks_path,
        &run.plan.id,
        &phase.phase_id,
    )
    .ok();
    let plan_path = PathBuf::from(&run.plan.plan_path);
    let workspace = PathBuf::from(&run.plan.workspace_path);
    let report = run_check_plan_blocking(
        &plan_path,
        &workspace,
        CheckPlanCall {
            only_phase: Some(&phase.phase_id),
            skip_execute: false,
            previous_skipped: &[],
            host,
            tokens: true,
            tokens_only: false,
            run: task_run.as_ref(),
            tokens_budget_ms: Some(
                tokens_budget_ms.unwrap_or(crate::services::plan_check::MAX_PLANNING_CHECK_MS),
            ),
        },
    )
    .await?;

    std::fs::create_dir_all(&runs_dir).map_err(|e| format!("create preflight dir: {e}"))?;
    let preflight_path = runs_dir.join("preflight.json");
    crate::services::plan_check::save_plan_check(&preflight_path, &report)?;

    let nn = phase_nn(Some(&phase.phase_id));
    if report.passed {
        append_event(
            state,
            &run.plan.id,
            Some(&phase.phase_id),
            "agent_phase_preflight_passed",
            json!({ "tasks": report.tasks_checked }),
        )?;
        return Ok(true);
    }
    let n = report.items.iter().filter(|i| i.blocking).count();
    append_event(
        state,
        &run.plan.id,
        Some(&phase.phase_id),
        "agent_phase_preflight_failed",
        json!({ "violations": n, "nn": nn }),
    )?;
    state.db.with_conn(|conn| {
        update_plan_status_and_health(
            conn,
            &run.plan.id,
            "needs_attention",
            Some("phase preflight failed"),
        )
        .map_err(|e| e.to_string())
    })?;
    Ok(false)
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
    count_consecutive_non_pass_kinds(
        events,
        gate_event_type,
        start_event_type,
        phase_id,
        &[],
        &[],
    )
}

fn count_consecutive_non_pass_kinds(
    events: &[AgentPlanEvent],
    gate_event_type: &str,
    start_event_type: &str,
    phase_id: Option<&str>,
    extra_fail: &[&str],
    extra_pass: &[&str],
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
        if extra_pass.contains(&event.event_type.as_str()) {
            break;
        }
        if extra_fail.contains(&event.event_type.as_str()) {
            n += 1;
            continue;
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
    count_consecutive_non_pass_kinds(
        &run.events,
        "planning_gate_result",
        "planning_started",
        None,
        &["planning_check_failed"],
        &[],
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
            )?;
            // Continuous SDLC (D-auto): hand the approved plan straight to development — no manual step.
            // `auto_start_development` returns a boxed future (breaks the async-recursion type cycle).
            auto_start_development(state, run).await
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

/// Continuous SDLC: when a planning run PASSes review, create the development stage-run from the
/// approved plan and start it — the initiative flows planning → development → review → done with no
/// manual step. Best-effort: any failure (most likely the plan is off-spec and `parse_plan` finds no
/// `overview.md`/`phases/`) is recorded as an event and leaves the initiative at planning-approved for
/// a human to resolve, rather than breaking the coordinator.
fn auto_start_development<'a>(
    state: &'a AppState,
    planning: &'a AgentPlanRun,
) -> std::pin::Pin<Box<dyn std::future::Future<Output = Result<(), String>> + Send + 'a>> {
    // Returns a BOXED future (rather than an `async fn`) to break the async-recursion type cycle:
    // this transitively calls start_plan → spawn_coordinator_loop → coordinator_loop →
    // handle_planning_reviewer_output → back here. A concrete boxed return type stops the opaque-type
    // cycle the compiler otherwise can't resolve.
    Box::pin(async move {
    let input = CreateAgentPlanInput {
        run_type: Some("development".to_string()),
        title: Some(planning.plan.title.clone()),
        workspace_path: planning.plan.workspace_path.clone(),
        plan_path: planning.plan.plan_path.clone(),
        worker_provider: planning.plan.worker_provider.clone(),
        reviewer_provider: planning.plan.reviewer_provider.clone(),
        brief: planning.plan.brief.clone(),
        app_scope: planning.plan.app_scope.clone(),
        docs_scope: planning.plan.docs_scope.clone(),
        reference_paths: planning.plan.reference_paths.clone(),
        worker_setup_commands: None,
        reviewer_setup_commands: None,
    };
    let dev = match create_plan(state, input) {
        Ok(dev) => dev,
        Err(err) => {
            let reason = format!(
                "Could not auto-start development from the approved plan: {}. The plan may not follow \
                 the methodology's overview.md + phases/ structure.",
                err
            );
            tracing::warn!(planning_id = %planning.plan.id, %err, "auto_start_development: create_plan failed");
            let _ = append_event(
                state,
                &planning.plan.id,
                None,
                "development_autostart_failed",
                json!({ "reason": reason }),
            );
            return Ok(()); // leave the initiative at planning-approved; a human resolves it
        }
    };
    // Carry the initiative's configured validation lenses onto the development run — `create_plan`
    // seeds validation_config NULL, so without this the review fan-out would fall back to the default
    // template instead of the lenses the user set at creation.
    if let Some(cfg) = planning.plan.validation_config.clone() {
        let _ = state.db.with_conn(|conn| {
            conn.execute(
                "UPDATE agent_plans SET validation_config = ?1, updated_at = datetime('now') WHERE id = ?2",
                params![cfg, dev.plan.id],
            )
            .map_err(|e| e.to_string())
        });
    }
    let _ = append_event(
        state,
        &dev.plan.id,
        None,
        "development_autostarted",
        json!({ "fromPlanningRun": planning.plan.id }),
    );
    // Kick the development coordinator loop on its OWN task — not inline. `start_plan` runs the worker
    // attach + phase kickoff and must not be folded into the (tokio::spawn'd, Send-bound) planning
    // coordinator future; spawning it also lets planning-review completion return promptly. (The
    // async-recursion type cycle is broken by boxing this fn's future at the call site.)
    let state_clone = state.clone();
    let dev_id = dev.plan.id.clone();
    tokio::spawn(async move {
        if let Err(err) = start_plan(state_clone, dev_id, None, None).await {
            tracing::error!(%err, "auto_start_development: start_plan failed");
        }
    });
    Ok(())
    })
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
        // Enter the `review` lifecycle stage while the configured lenses fan out. This is the single
        // per-phase entry into review, co-located with the `agent_phase_review_started` marker below,
        // so the lifecycle bar pulses development → review each phase (reverted on loop-back / advance).
        conn.execute(
            "UPDATE agent_plans SET status = 'phase_review_running', initiative_status = 'review', updated_at = datetime('now') WHERE id = ?1 AND run_type = 'development'",
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
        // Review requested changes → loop back to the worker: leave the `review` stage, return to
        // `development` for the same phase (the bar reverts review → development).
        conn.execute(
            "UPDATE agent_plans SET status = 'phase_worker_running', initiative_status = 'development', updated_at = datetime('now') WHERE id = ?1 AND run_type = 'development'",
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
            // Phase passed, more phases remain → leave `review`, return to `development` as the next
            // phase's worker starts (the bar reverts review → development for the next iteration).
            conn.execute(
                "UPDATE agent_plans SET status = 'phase_worker_running', initiative_status = 'development', current_phase_id = ?1, current_phase_index = ?2, updated_at = datetime('now') WHERE id = ?3 AND run_type = 'development'",
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
            // Final phase passed → the development run is complete. Advance the
            // initiative lifecycle to 'done' (health is derived to 'complete' above);
            // otherwise the stepper stays stuck on 'development' forever.
            conn.execute(
                "UPDATE agent_plans SET phase_run_mode = 'continue', initiative_status = 'done', updated_at = datetime('now') WHERE id = ?1 AND run_type = 'development'",
                params![plan_id],
            )
            .map_err(|e| e.to_string())?;
        }
        } else {
            update_plan_status_and_health(conn, plan_id, "approved", None)
                .map_err(|e| e.to_string())?;
            // Single-phase run approved → development complete; advance to 'done'.
            conn.execute(
                "UPDATE agent_plans SET phase_run_mode = 'continue', initiative_status = 'done', updated_at = datetime('now') WHERE id = ?1 AND run_type = 'development'",
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
        let next_path = Path::new(&run.plan.plan_path)
            .join("phases")
            .join(&next.phase_id);
        if !crate::services::task_loop::phase_is_kloo_mode(&next_path) {
            let refreshed = get_plan(state, plan_id)?;
            let worker_session_id = refreshed
                .plan
                .worker_session_id
                .clone()
                .ok_or_else(|| "Plan has no worker session".to_string())?;
            let prompt = worker_phase_prompt(state, &refreshed, &next)?;
            terminal::send_terminal_input(state, worker_session_id, format!("{}\r", prompt)).await?;
        }
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

pub(crate) fn normalize_task_status(status: &str) -> String {
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

/// The default validation template (overhaul P7, D1/D6/D14): today's fixed review triad —
/// `product` / `qa` / `lead`, in that order, every lens `blocking:true` and `vision:false`,
/// all on the plan's reviewer provider/model. Reproduces the hardcoded `REVIEW_LENSES` + the
/// single-provider choice the fan-out makes today, so an unconfigured Initiative behaves EXACTLY
/// as before this feature. `model` falls back to `default_model_for_provider(provider)` when the
/// caller passes `None` (mirrors the ephemeral-reviewer model choice).
pub fn default_validation_config(reviewer_provider: &str, model: Option<String>) -> Vec<ValidationLens> {
    let model = model.or_else(|| default_model_for_provider(reviewer_provider));
    ["product", "qa", "lead"]
        .iter()
        .map(|name| ValidationLens {
            name: name.to_string(),
            provider: reviewer_provider.to_string(),
            model: model.clone(),
            prompt: None,
            vision: false,
            blocking: true,
        })
        .collect()
}

/// Resolve the ordered lens list a review fan-out should iterate (overhaul P7, D1). Parses the
/// Initiative's persisted `validation_config` JSON array; when it is absent, malformed, or an
/// empty array, falls back to `default_validation_config` on the plan's reviewer provider. This
/// is the single contract Phase 02's fan-out reads — an unconfigured/empty config is today's
/// product/qa/lead triad.
pub fn resolve_validation_lenses(run: &AgentPlanRun) -> Vec<ValidationLens> {
    if let Some(json) = run.plan.validation_config.as_deref() {
        if let Ok(lenses) = serde_json::from_str::<Vec<ValidationLens>>(json) {
            if !lenses.is_empty() {
                return lenses;
            }
        }
    }
    default_validation_config(
        &run.plan.reviewer_provider,
        default_model_for_provider(&run.plan.reviewer_provider),
    )
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

/// Events for a whole Initiative — the UNION of every plan-run's events (planning-run + development-run
/// share one `initiative_id`), enriched with phase index/title and returned oldest-first so the console
/// can render the full SDLC timeline (planning → approval → development → review loop → done). Mirrors
/// `list_events` but keys on `initiative_id` via a join instead of a single `plan_id`.
pub fn list_initiative_events(
    state: &AppState,
    initiative_id: &str,
    limit: i64,
) -> Result<Vec<AgentPlanEvent>, String> {
    state.db.with_conn(|conn| {
        let mut stmt = conn
            .prepare(
                "SELECT e.id, e.plan_id, e.phase_id, e.type, e.payload_json, e.created_at \
                 FROM agent_plan_events e JOIN agent_plans p ON e.plan_id = p.id \
                 WHERE p.initiative_id = ?1 ORDER BY e.created_at ASC, e.rowid ASC LIMIT ?2",
            )
            .map_err(|e| e.to_string())?;
        let mut rows: Vec<AgentPlanEvent> = stmt
            .query_map(params![initiative_id, limit], agent_plan_event_from_row)
            .map_err(|e| e.to_string())?
            .filter_map(|row| row.ok())
            .collect();
        // Phase metadata spans every run of the initiative (each run owns its own phases).
        let mut phase_meta = HashMap::new();
        let mut phase_stmt = conn
            .prepare(
                "SELECT phase_id, phase_index, phase_title FROM agent_plan_phases \
                 WHERE plan_id IN (SELECT id FROM agent_plans WHERE initiative_id = ?1)",
            )
            .map_err(|e| e.to_string())?;
        let phase_rows = phase_stmt
            .query_map(params![initiative_id], |row| {
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

/// Emit a kloo-mode task event with the contracted payload fields.
pub(crate) fn emit_task_event(
    state: &AppState,
    plan_id: &str,
    event_type: &str,
    task_id: &str,
    phase_id: &str,
    tier: Option<&str>,
    attempt: Option<u32>,
    failure_code: Option<&str>,
    commit_sha: Option<&str>,
    route: Option<&str>,
    checks: Option<serde_json::Value>,
) -> Result<(), String> {
    debug_assert!(
        event_type.starts_with("agent_phase_task_"),
        "task events must use the agent_phase_ prefix"
    );
    append_event(
        state,
        plan_id,
        Some(phase_id),
        event_type,
        serde_json::json!({
            "taskId": task_id,
            "phaseId": phase_id,
            "tier": tier,
            "attempt": attempt,
            "failureCode": failure_code,
            "commitSha": commit_sha,
            "route": route,
            "checks": checks,
        }),
    )
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

pub(crate) fn git_changed_files(workspace_path: &str, path_filter: Option<&str>) -> Result<Vec<HostFileEntry>, String> {
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
        briefing_session_id: row.get(22)?,
        created_at: row.get(23)?,
        updated_at: row.get(24)?,
        // Appended last (P7, D3) — order is load-bearing; every SELECT feeding this mapper
        // ends with validation_config.
        validation_config: row.get(25)?,
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
        // A human's run/resume guidance — a NEW actor value (the timeline renders it as
        // the "you" chip). Distinct from `user` (system-attributed lifecycle events).
        "human_comment" => "human",
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
        | "planning_check_passed"
        | "planning_check_failed"
        | "planning_check_phase"
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
        | "agent_single_phase_completed"
        | "agent_phase_task_done"
        | "agent_phase_task_failed"
        | "agent_phase_task_escalated"
        | "agent_phase_preflight_passed"
        | "agent_phase_preflight_failed"
        // A human comment on a run/resume is phase-scoped guidance; the "phase" category
        // maps to the console's development stage (frontend `stageOf`) — see decisions.md.
        | "human_comment" => "phase",
        _ => "run",
    }
}

fn event_status_transition(
    event_type: &str,
    payload: &serde_json::Value,
) -> (Option<&'static str>, Option<&'static str>) {
    match event_type {
        "planning_started" => (None, Some("planning_planner_running")),
        "planning_planner_ready" => (Some("planning_planner_running"), None),
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
        // The coordinator's own block/unblock, emitted by `wait_for_agent_ready_report`.
        // The pre-block status is dynamic (`phase_worker_running` vs
        // `planning_planner_running`) so it cannot be a `&'static str` here; it is
        // carried in the event payload as `statusBefore` instead.
        "agent_blocked" => (None, Some("blocked")),
        "agent_unblocked" => (Some("blocked"), None),
        _ => (None, None),
    }
}

fn phase_nn(phase_id: Option<&str>) -> &str {
    phase_id
        .and_then(|p| p.split('-').next())
        .unwrap_or("??")
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
        "planning_check_passed" => "Planning plan-check passed".to_string(),
        "planning_check_failed" => "Planning plan-check failed".to_string(),
        "planning_check_phase" => {
            let nn = phase_nn(event.phase_id.as_deref());
            let e = payload
                .get("executed")
                .and_then(|v| v.as_u64())
                .unwrap_or(0);
            let s = payload
                .get("skipped")
                .and_then(|v| v.as_u64())
                .unwrap_or(0);
            format!("Checking phase {nn} — {e} executed, {s} skipped.")
        }
        "agent_phase_preflight_passed" => {
            let nn = phase_nn(event.phase_id.as_deref());
            let n = payload
                .get("tasks")
                .and_then(|v| v.as_u64())
                .unwrap_or(0);
            format!("Phase {nn} preflight passed — {n} tasks, 0 shape violations.")
        }
        "agent_phase_preflight_failed" => {
            let nn = payload_str(payload, "nn")
                .unwrap_or_else(|| phase_nn(event.phase_id.as_deref()).to_string());
            let n = payload
                .get("violations")
                .and_then(|v| v.as_u64())
                .unwrap_or(0);
            format!("Phase {nn} preflight failed — {n} violations.")
        }
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
        // Configured validation lenses (the "review"). Surface WHICH lens and WHAT it decided so the
        // console Events timeline can show "qa lens → NEEDS_CHANGES: <reason>" per the user's ask.
        "agent_lens_review_started" => match payload_str(payload, "lens") {
            Some(lens) => format!("{} lens started review", lens),
            None => "Lens started review".to_string(),
        },
        "agent_lens_verdict" => {
            let lens = payload_str(payload, "lens").unwrap_or_else(|| "lens".to_string());
            let verdict = payload_str(payload, "verdict").unwrap_or_else(|| "reviewed".to_string());
            let detail = payload_str(payload, "summary").or_else(|| {
                payload
                    .get("findings")
                    .and_then(|f| f.as_array())
                    .and_then(|a| a.first())
                    .and_then(|v| v.as_str())
                    .map(str::to_string)
            });
            match detail {
                Some(detail) => format!("{} lens → {}: {}", lens, verdict, detail),
                None => format!("{} lens → {}", lens, verdict),
            }
        }
        "development_autostarted" => "Development started automatically".to_string(),
        "development_autostart_failed" => match payload_str(payload, "reason") {
            Some(reason) => format!("Auto-start development failed — {}", reason),
            None => "Auto-start development failed".to_string(),
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
        "agent_phase_task_done" => {
            let id = payload_str(payload, "taskId").unwrap_or_else(|| "?".into());
            let tier = payload_str(payload, "tier").unwrap_or_else(|| "unknown".into());
            format!("Task {id} passed ({tier})")
        }
        "agent_phase_task_failed" => {
            let id = payload_str(payload, "taskId").unwrap_or_else(|| "?".into());
            let rule = payload_str(payload, "failureCode")
                .or_else(|| payload_str(payload, "route"))
                .unwrap_or_else(|| "failed".into());
            format!("Task {id} failed ({rule})")
        }
        "agent_phase_task_escalated" => {
            let id = payload_str(payload, "taskId").unwrap_or_else(|| "?".into());
            let tier = payload_str(payload, "tier").unwrap_or_else(|| "next".into());
            format!("Task {id} escalated to {tier}")
        }
        "agent_phase_unlocked" => "Unlocked the next phase".to_string(),
        "agent_plan_completed" => "Completed the full run".to_string(),
        "agent_docs_commit_started" => "Docs agent — updating app-repo docs".to_string(),
        "agent_docs_committed" => "Docs committed to the app repo".to_string(),
        "agent_docs_commit_skipped" => {
            "Docs commit skipped (no app repo path set)".to_string()
        }
        "agent_docs_commit_failed" => "Docs commit failed".to_string(),
        "agent_single_phase_completed" => "Completed the selected phase only".to_string(),
        // Human run/resume guidance. When the comment rode a resume that named a target
        // phase, surface it: "Resume · <PHASE> + comment: <text>"; otherwise "Comment: <text>".
        "human_comment" => {
            let text = payload_str(payload, "text").unwrap_or_default();
            match payload_str(payload, "phaseId") {
                Some(phase) => format!("Resume · {} + comment: {}", phase, text),
                None => format!("Comment: {}", text),
            }
        }
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
    fn verdict_role_allows_reviewer_and_any_lens_name() {
        // The plan reviewer and every lens — default or custom-named — may report a verdict.
        for role in ["reviewer", "product", "qa", "lead", "pr", "le", "perf", "security"] {
            assert!(
                verdict_role_allowed(role),
                "expected lens/reviewer role '{role}' to be allowed to report a verdict"
            );
        }
    }

    #[test]
    fn verdict_role_rejects_worker_and_docs() {
        // The worker/planner reports `ready`; the docs agent reports `done` — neither votes a verdict.
        assert!(!verdict_role_allowed("worker"));
        assert!(!verdict_role_allowed("docs"));
    }

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
    fn agent_phase_task_events_are_phase_scoped() {
        for ty in [
            "agent_phase_task_done",
            "agent_phase_task_failed",
            "agent_phase_task_escalated",
        ] {
            assert!(ty.starts_with("agent_phase_"));
            assert_eq!(event_category(ty), "phase");
            assert_eq!(event_actor(ty), "coordinator");
            assert_eq!(event_status_transition(ty, &serde_json::json!({})), (None, None));
        }
        let ev = |ty: &str| AgentPlanEvent {
            id: "e".into(),
            plan_id: "p".into(),
            phase_id: Some("00-x".into()),
            phase_index: None,
            phase_title: None,
            event_type: ty.into(),
            actor: "coordinator".into(),
            category: "phase".into(),
            summary: String::new(),
            status_before: None,
            status_after: None,
            reason: None,
            verdict: None,
            task_id: Some("01-add".into()),
            clarification_attempt: None,
            payload_json: "{}".into(),
            created_at: String::new(),
        };
        assert_eq!(
            event_summary(
                &ev("agent_phase_task_done"),
                &serde_json::json!({ "taskId": "01-add", "tier": "qwen3-coder" })
            ),
            "Task 01-add passed (qwen3-coder)"
        );
        assert_eq!(
            event_summary(
                &ev("agent_phase_task_failed"),
                &serde_json::json!({ "taskId": "01-add", "failureCode": "off_scope_edit" })
            ),
            "Task 01-add failed (off_scope_edit)"
        );
        assert_eq!(
            event_summary(
                &ev("agent_phase_task_escalated"),
                &serde_json::json!({ "taskId": "01-add", "tier": "claude" })
            ),
            "Task 01-add escalated to claude"
        );
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

    /// A blocked agent waits for a human indefinitely. While it waits the plan must
    /// NOT keep a `*_running` status — run `4187e055` sat at `phase_worker_running`
    /// for three days after reporting `blocked`, so every status reader said
    /// "running" while nothing was happening.
    #[test]
    fn block_flips_status_and_unblock_restores_it() {
        use crate::db::migrations::run_migrations;
        let conn = rusqlite::Connection::open_in_memory().unwrap();
        run_migrations(&conn).unwrap();
        conn.execute(
            "INSERT INTO agent_plans (id, run_type, title, workspace_path, plan_path, worker_provider, reviewer_provider, initiative_id, initiative_status) \
             VALUES ('P', 'development', 't', '/w', '/p', 'claude_code', 'claude_code', 'P', 'development')",
            [],
        )
        .unwrap();
        let status = |c: &rusqlite::Connection| -> (String, String) {
            c.query_row("SELECT status, health FROM agent_plans WHERE id='P'", [], |r| {
                Ok((r.get(0)?, r.get(1)?))
            })
            .unwrap()
        };

        update_plan_status_and_health(&conn, "P", "phase_worker_running", None).unwrap();

        let previous = mark_plan_blocked_conn(&conn, "P", "needs a human").unwrap();
        assert_eq!(previous, "phase_worker_running", "must report the pre-block status");
        assert_eq!(status(&conn).0, "blocked", "status must stop claiming it is running");

        clear_plan_blocked_conn(&conn, "P", &previous).unwrap();
        assert_eq!(
            status(&conn),
            ("phase_worker_running".to_string(), health_from_status("phase_worker_running").to_string()),
            "unblock must hand the exact previous status back so the coordinator's guarded transition still matches"
        );
    }

    /// If a human stops the run while the agent is blocked, the agent resuming must
    /// not resurrect it.
    #[test]
    fn unblock_does_not_override_a_human_stop() {
        use crate::db::migrations::run_migrations;
        let conn = rusqlite::Connection::open_in_memory().unwrap();
        run_migrations(&conn).unwrap();
        conn.execute(
            "INSERT INTO agent_plans (id, run_type, title, workspace_path, plan_path, worker_provider, reviewer_provider, initiative_id, initiative_status) \
             VALUES ('P', 'development', 't', '/w', '/p', 'claude_code', 'claude_code', 'P', 'development')",
            [],
        )
        .unwrap();
        update_plan_status_and_health(&conn, "P", "phase_worker_running", None).unwrap();
        let previous = mark_plan_blocked_conn(&conn, "P", "needs a human").unwrap();

        // Human stops the run while it is blocked.
        update_plan_status_and_health(&conn, "P", "stopped", None).unwrap();

        let rows = clear_plan_blocked_conn(&conn, "P", &previous).unwrap();
        assert_eq!(rows, 0, "guard must refuse to write when the plan is no longer blocked");
        let (s, _) = conn
            .query_row("SELECT status, health FROM agent_plans WHERE id='P'", [], |r| {
                Ok((r.get::<_, String>(0)?, r.get::<_, String>(1)?))
            })
            .unwrap();
        assert_eq!(s, "stopped", "a human stop must survive the agent resuming");
    }

    #[test]
    fn blocked_and_unblocked_events_carry_a_status_transition() {
        // The `agent_blocked` event previously recorded None -> None, so the run
        // timeline showed a block that changed nothing.
        assert_eq!(
            event_status_transition("agent_blocked", &serde_json::json!({})),
            (None, Some("blocked"))
        );
        assert_eq!(
            event_status_transition("agent_unblocked", &serde_json::json!({})),
            (Some("blocked"), None)
        );
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

    // ── P7 Phase 02: dynamic fan-out plumbing + blocking/warn gate (pure) ─────────────
    fn lens(name: &str, provider: &str, blocking: bool) -> ValidationLens {
        ValidationLens {
            name: name.into(),
            provider: provider.into(),
            model: None,
            prompt: None,
            vision: false,
            blocking,
        }
    }

    fn outcome(name: &str, verdict: &str) -> (String, String, ReviewInsights) {
        (
            name.into(),
            verdict.into(),
            ReviewInsights {
                summary: None,
                findings: vec![format!("{} finding", name)],
                next_steps: vec![],
                reason: None,
            },
        )
    }

    // A1/A2 — lens_spawn_descriptors: N-arity, order, per-lens provider/model, provider validation.
    #[test]
    fn lens_spawn_descriptors_honor_one_and_five_lens_configs() {
        // 1-lens config → exactly one descriptor.
        let one = vec![lens("solo", "grok", true)];
        let d1 = lens_spawn_descriptors(&one).unwrap();
        assert_eq!(d1.len(), 1);
        assert_eq!(d1[0].name, "solo");
        assert_eq!(d1[0].provider, "grok");
        // grok seeds its default model when the lens leaves model None.
        assert_eq!(d1[0].model.as_deref(), Some("grok-build"));

        // 5-lens config → five descriptors, order + per-lens provider preserved.
        let five = vec![
            lens("one", "claude_code", true),
            lens("two", "codex", false),
            lens("three", "cline", true),
            lens("four", "ollama", false),
            lens("five", "grok", true),
        ];
        let d5 = lens_spawn_descriptors(&five).unwrap();
        assert_eq!(d5.len(), 5);
        assert_eq!(
            d5.iter().map(|d| d.name.as_str()).collect::<Vec<_>>(),
            ["one", "two", "three", "four", "five"]
        );
        assert_eq!(
            d5.iter().map(|d| d.provider.as_str()).collect::<Vec<_>>(),
            ["claude_code", "codex", "cline", "ollama", "grok"]
        );
    }

    #[test]
    fn lens_spawn_descriptors_two_providers_are_distinct() {
        let cfg = vec![lens("a", "claude_code", true), lens("b", "grok", true)];
        let d = lens_spawn_descriptors(&cfg).unwrap();
        assert_ne!(d[0].provider, d[1].provider);
        assert_eq!(d[0].provider, "claude_code");
        assert_eq!(d[1].provider, "grok");
    }

    #[test]
    fn lens_spawn_descriptor_prefers_explicit_model() {
        let mut l = lens("x", "grok", true);
        l.model = Some("grok-custom".into());
        assert_eq!(lens_spawn_descriptor(&l).model.as_deref(), Some("grok-custom"));
    }

    #[test]
    fn lens_spawn_descriptors_reject_unknown_provider() {
        let cfg = vec![lens("bad", "claude_cod", true)];
        let err = lens_spawn_descriptors(&cfg).unwrap_err();
        assert!(
            err.contains("Unknown provider") && err.contains("claude_cod"),
            "{err}"
        );
    }

    #[test]
    fn lens_config_rejects_oneshot_providers() {
        for provider in ["kloo", "shell"] {
            let err = lens_spawn_descriptors(&[lens("x", provider, true)]).unwrap_err();
            assert!(
                err.contains("oneshot executor") && err.contains(provider),
                "{err}"
            );
            assert!(!err.contains("Unknown provider"), "{err}");
        }
    }

    // A3v — vision clause + custom rubric appear only when set (pure half of the prompt builders).
    #[test]
    fn lens_prompt_extras_appends_vision_and_custom_prompt() {
        let plain = lens("product", "claude_code", true);
        assert_eq!(lens_prompt_extras(&plain), "", "default non-vision lens adds nothing");

        let mut vision = lens("security", "grok", true);
        vision.vision = true;
        let vx = lens_prompt_extras(&vision);
        assert!(vx.contains("approve on functional grounds"));
        assert!(vx.contains("cannot read a screenshot"));

        let mut custom = lens("custom", "codex", false);
        custom.prompt = Some("Check the licence headers.".into());
        let cx = lens_prompt_extras(&custom);
        assert!(cx.contains("Check the licence headers."));
        assert!(!cx.contains("functional grounds"), "no vision clause when vision=false");
    }

    // A4 — blocking gates, warn only annotates.
    #[test]
    fn gate_over_blocking_warn_does_not_halt() {
        // Two blocking lenses PASS, a warn lens BLOCKED → gate PASS (warn cannot halt).
        let lenses = vec![
            lens("product", "claude_code", true),
            lens("qa", "claude_code", true),
            lens("perf", "claude_code", false),
        ];
        let outcomes = vec![
            outcome("product", "PASS"),
            outcome("qa", "PASS"),
            outcome("perf", "BLOCKED"),
        ];
        assert_eq!(gate_verdict_over_blocking(&outcomes, &lenses), "PASS");
        // …but the warn lens's finding still surfaces in the merged body.
        let (_s, findings, _m) = merge_lens_body(&outcomes);
        assert!(findings.contains("[perf] perf finding"));
    }

    #[test]
    fn gate_over_blocking_blocking_lens_halts() {
        let lenses = vec![
            lens("product", "claude_code", true),
            lens("perf", "claude_code", false),
        ];
        // Blocking NEEDS_CHANGES → gate NEEDS_CHANGES (warn PASS is irrelevant).
        let nc = vec![outcome("product", "NEEDS_CHANGES"), outcome("perf", "PASS")];
        assert_eq!(gate_verdict_over_blocking(&nc, &lenses), "NEEDS_CHANGES");
        // Blocking BLOCKED → gate BLOCKED.
        let bl = vec![outcome("product", "BLOCKED"), outcome("perf", "PASS")];
        assert_eq!(gate_verdict_over_blocking(&bl, &lenses), "BLOCKED");
    }

    #[test]
    fn gate_over_blocking_all_warn_is_pass() {
        let lenses = vec![
            lens("a", "claude_code", false),
            lens("b", "claude_code", false),
        ];
        let outcomes = vec![outcome("a", "BLOCKED"), outcome("b", "NEEDS_CHANGES")];
        assert_eq!(gate_verdict_over_blocking(&outcomes, &lenses), "PASS");
    }

    // A5 — no regression: default 3 blocking lenses → gate == merged_verdict over all three.
    #[test]
    fn gate_over_blocking_default_matches_merged_verdict() {
        let lenses = default_validation_config("claude_code", None); // 3 blocking
        for verdicts in [
            ["PASS", "PASS", "PASS"],
            ["PASS", "NEEDS_CHANGES", "PASS"],
            ["PASS", "NEEDS_CHANGES", "BLOCKED"],
            ["BLOCKED", "PASS", "PASS"],
        ] {
            let outcomes: Vec<_> = lenses
                .iter()
                .zip(verdicts.iter())
                .map(|(l, v)| outcome(&l.name, v))
                .collect();
            assert_eq!(
                gate_verdict_over_blocking(&outcomes, &lenses),
                merged_verdict(verdicts),
                "gate must equal today's merged_verdict for the all-blocking default"
            );
        }
    }

    // The summary line marks warn lenses so a non-PASS from a warn lens reads as non-gating.
    #[test]
    fn lens_summary_line_marks_warn_lenses() {
        let lenses = vec![
            lens("product", "claude_code", true),
            lens("perf", "claude_code", false),
        ];
        let outcomes = vec![outcome("product", "PASS"), outcome("perf", "NEEDS_CHANGES")];
        let line = lens_summary_line(&outcomes, &lenses);
        assert!(line.contains("product: PASS"));
        assert!(line.contains("perf: NEEDS_CHANGES (warn)"));
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

    // --- Task 00-01: comment validation ---

    #[test]
    fn validate_comment_treats_missing_or_empty_as_none() {
        // Pure re-run/resume: no feedback, no event.
        assert_eq!(validate_comment(None).unwrap(), None);
        assert_eq!(validate_comment(Some("")).unwrap(), None);
    }

    #[test]
    fn validate_comment_rejects_whitespace_only() {
        // A field that IS used but carries only whitespace is a user error.
        assert!(validate_comment(Some("   \t\n ")).is_err());
    }

    #[test]
    fn validate_comment_preserves_non_blank_text() {
        // Non-blank text is accepted and kept verbatim (stored/injected as guidance).
        assert_eq!(
            validate_comment(Some("recompute total")).unwrap(),
            Some("recompute total".to_string())
        );
    }

    // --- Task 00-02: human_comment read-time enrichment ---

    #[test]
    fn human_comment_actor_is_human() {
        assert_eq!(event_actor("human_comment"), "human");
    }

    #[test]
    fn human_comment_category_groups_under_development_stage() {
        // "phase" maps to the console's development stage in the frontend `stageOf`.
        assert_eq!(event_category("human_comment"), "phase");
    }

    #[test]
    fn human_comment_summary_composes_resume_with_phase() {
        let event = gate_event("human_comment", None, "");
        let payload = json!({ "text": "recompute total", "phaseId": "P02", "mode": "continue" });
        let summary = event_summary(&event, &payload);
        assert!(summary.contains("Resume · P02"), "got: {summary}");
        assert!(summary.contains("recompute total"), "got: {summary}");
    }

    #[test]
    fn human_comment_summary_falls_back_to_comment_form() {
        let event = gate_event("human_comment", None, "");
        let payload = json!({ "text": "looks good" });
        assert_eq!(event_summary(&event, &payload), "Comment: looks good");
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
    fn initiative_runs_path_is_phase_keyed() {
        let path = settings_service::initiative_runs_path(
            Path::new("/store"),
            "init-1",
            "plan-1",
            "00-atomic-plan-store",
        );
        assert_eq!(
            path,
            PathBuf::from("/store/init-1/runs/plan-1/00-atomic-plan-store")
        );
        assert!(
            !path.to_string_lossy().contains("snapshots"),
            "runs path must not include a snapshots segment"
        );
    }

    #[test]
    fn write_brief_md_leaves_full_composed_bytes() {
        let dir = tmp_dir("brief");
        let composed = "# Accepted brief\n\nfull composed contents\n";
        write_brief_md(&dir, composed).unwrap();
        let landed = std::fs::read_to_string(dir.join("brief.md")).unwrap();
        assert_eq!(landed, composed);
        let leftovers: Vec<_> = std::fs::read_dir(&dir)
            .unwrap()
            .filter_map(|e| e.ok())
            .map(|e| e.file_name().to_string_lossy().into_owned())
            .filter(|n| n.ends_with(".j1tmp"))
            .collect();
        assert!(leftovers.is_empty(), "no temp sibling: {:?}", leftovers);
        let _ = std::fs::remove_dir_all(&dir);
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

    // ── Task 01-01: briefing_session_id column round-trips through the mapper ─────────
    #[test]
    fn briefing_session_id_column_round_trips() {
        let conn = rusqlite::Connection::open_in_memory().unwrap();
        run_migrations(&conn).unwrap();

        // The migration must have added the column…
        let cols: Vec<String> = {
            let mut stmt = conn.prepare("PRAGMA table_info(agent_plans)").unwrap();
            stmt.query_map([], |row| row.get::<_, String>(1))
                .unwrap()
                .map(|r| r.unwrap())
                .collect()
        };
        assert!(
            cols.iter().any(|c| c == "briefing_session_id"),
            "agent_plans must have briefing_session_id (have: {:?})",
            cols
        );

        // …and a row inserted with it set reads back through the shared row mapper.
        conn.execute(
            "INSERT INTO agent_plans (id, run_type, title, workspace_path, plan_path, status, worker_provider, reviewer_provider, initiative_id, initiative_status, briefing_session_id) \
             VALUES ('B', 'planning', 't', '/w', '/p', 'draft', 'claude_code', 'claude_code', 'B', 'briefing', 'CHAT-SESSION')",
            [],
        )
        .unwrap();

        let plan = conn
            .query_row(
                "SELECT id, run_type, title, workspace_path, plan_path, status, worker_session_id, reviewer_session_id, worker_provider, reviewer_provider, current_phase_id, current_phase_index, error, brief, app_scope, docs_scope, reference_paths, amend_brief, phase_run_mode, initiative_id, initiative_status, health, briefing_session_id, created_at, updated_at, validation_config FROM agent_plans WHERE id='B'",
                [],
                agent_plan_from_row,
            )
            .unwrap();
        assert_eq!(plan.briefing_session_id.as_deref(), Some("CHAT-SESSION"));
        assert_eq!(plan.initiative_status, "briefing");
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
        assert_eq!((s.as_str(), h.as_str()), ("approved", "complete"));
        assert!(e.is_none(), "approve must clear error");
    }

    #[test]
    fn create_plan_rejects_oneshot_providers() {
        let (state, root) = test_state();
        let workspace = tmp_dir("ws");
        for (worker, reviewer) in [
            ("kloo", "claude_code"),
            ("claude_code", "kloo"),
            ("shell", "claude_code"),
            ("claude_code", "shell"),
        ] {
            let mut inp = input(
                Some("planning"),
                workspace.to_string_lossy().as_ref(),
                "docs/plans/whatever",
            );
            inp.worker_provider = worker.to_string();
            inp.reviewer_provider = reviewer.to_string();
            let err = create_plan(&state, inp).unwrap_err();
            assert!(
                err.contains("oneshot executor")
                    && (err.contains(worker) || err.contains(reviewer)),
                "{err}"
            );
        }
        let _ = std::fs::remove_dir_all(&workspace);
        let _ = std::fs::remove_dir_all(&root);
    }

    #[tokio::test]
    async fn create_briefing_run_rejects_oneshot_providers() {
        let (state, root) = test_state();
        let workspace = tmp_dir("ws");
        let err = create_briefing_run(
            &state,
            CreateBriefingInput {
                title: Some("x".into()),
                workspace_path: workspace.to_string_lossy().to_string(),
                brief: None,
                worker_provider: "kloo".into(),
                reviewer_provider: "claude_code".into(),
                model: None,
            },
        )
        .await
        .unwrap_err();
        assert!(err.contains("oneshot") && err.contains("kloo"), "{err}");
        let _ = std::fs::remove_dir_all(&workspace);
        let _ = std::fs::remove_dir_all(&root);
    }

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

    // ── Task 01-03: create_briefing_run makes a briefing Initiative ───────────────────
    #[tokio::test]
    async fn create_briefing_run_makes_briefing_initiative() {
        let (state, root) = test_state();
        let workspace = tmp_dir("ws");
        let store = tmp_dir("store").canonicalize().unwrap();
        settings_service::set_setting(
            &state,
            settings_service::KEY_INITIATIVES_DIR.to_string(),
            store.to_string_lossy().to_string(),
        )
        .unwrap();

        let run = create_briefing_run(
            &state,
            CreateBriefingInput {
                title: Some("My Feature".to_string()),
                workspace_path: workspace.to_string_lossy().to_string(),
                brief: Some("raw ask".to_string()),
                worker_provider: "claude_code".to_string(),
                reviewer_provider: "claude_code".to_string(),
                model: None,
            },
        )
        .await
        .expect("create_briefing_run");
        let plan = run.plan;

        // Row shape: a briefing Initiative with no planner session yet.
        assert_eq!(plan.initiative_status, "briefing");
        assert_eq!(plan.health, "in-progress");
        assert!(plan.worker_session_id.is_none(), "no planner session during briefing");
        assert!(plan.reviewer_session_id.is_none());
        assert_eq!(plan.brief.as_deref(), Some("raw ask"), "brief holds the raw ask");
        let chat_id = plan
            .briefing_session_id
            .clone()
            .expect("briefing_session_id must be set");

        // The linked conversation session is a chat-lane (kind='user') session.
        let kind: String = state
            .db
            .with_conn(|conn| {
                conn.query_row(
                    "SELECT kind FROM sessions WHERE id = ?1",
                    params![chat_id],
                    |r| r.get(0),
                )
                .map_err(|e| e.to_string())
            })
            .unwrap();
        assert_eq!(kind, "user", "briefing conversation must be a kind='user' chat session");

        // Both store dirs exist under the initiative id.
        assert!(
            settings_service::initiative_plan_path(&store, &plan.initiative_id).is_dir(),
            "<id>/plan dir must exist"
        );
        assert!(
            settings_service::initiative_attachments_path(&store, &plan.initiative_id).is_dir(),
            "<id>/attachments dir must exist"
        );

        let _ = std::fs::remove_dir_all(&workspace);
        let _ = std::fs::remove_dir_all(&store);
        let _ = std::fs::remove_dir_all(&root);
    }

    // ── Task 02-01: compose_accepted_brief folds draft + attachments + reference paths ─
    #[test]
    fn compose_accepted_brief_folds_sections() {
        // Draft-only → the trimmed draft, no dangling headers.
        let only = compose_accepted_brief("just the ask\n\n", &[], &[]);
        assert_eq!(only, "just the ask");
        assert!(!only.contains("## Attached files"));
        assert!(!only.contains("## Referenced host paths"));

        // With both sections → draft + each path listed under its header.
        let full = compose_accepted_brief(
            "the ask",
            &["/store/INIT/attachments/a.png".to_string(), "/store/INIT/attachments/b.pdf".to_string()],
            &["/home/u/proj/docs".to_string()],
        );
        assert!(full.contains("the ask"));
        assert!(full.contains("## Attached files"));
        assert!(full.contains("- /store/INIT/attachments/a.png"));
        assert!(full.contains("- /store/INIT/attachments/b.pdf"));
        assert!(full.contains("## Referenced host paths"));
        assert!(full.contains("- /home/u/proj/docs"));

        // Only attachments → no reference-paths header.
        let attach_only = compose_accepted_brief("x", &["/p/f".to_string()], &[]);
        assert!(attach_only.contains("## Attached files"));
        assert!(!attach_only.contains("## Referenced host paths"));

        // Only reference paths → no attachments header.
        let ref_only = compose_accepted_brief("x", &[], &["/p/d".to_string()]);
        assert!(!ref_only.contains("## Attached files"));
        assert!(ref_only.contains("## Referenced host paths"));
    }

    // ── Task 02-01: apply_brief_acceptance flips the SAME row briefing→planning ────────
    #[test]
    fn apply_brief_acceptance_flips_same_row() {
        let conn = rusqlite::Connection::open_in_memory().unwrap();
        run_migrations(&conn).unwrap();
        conn.execute(
            "INSERT INTO agent_plans (id, run_type, title, workspace_path, plan_path, status, worker_provider, reviewer_provider, brief, reference_paths, initiative_id, initiative_status) \
             VALUES ('INIT', 'planning', 't', '/w', '/p', 'draft', 'claude_code', 'claude_code', 'draft ask', '/w/docs', 'INIT', 'briefing')",
            [],
        )
        .unwrap();

        apply_brief_acceptance(&conn, "INIT", "COMPOSED BRIEF").unwrap();

        let (status, brief): (String, Option<String>) = conn
            .query_row(
                "SELECT initiative_status, brief FROM agent_plans WHERE id='INIT'",
                [],
                |row| Ok((row.get(0)?, row.get(1)?)),
            )
            .unwrap();
        assert_eq!(status, "planning", "same row must flip to planning");
        assert_eq!(brief.as_deref(), Some("COMPOSED BRIEF"));
        // The id is unchanged — one initiative, not a second row (D1).
        let count: i64 = conn
            .query_row("SELECT COUNT(*) FROM agent_plans", [], |r| r.get(0))
            .unwrap();
        assert_eq!(count, 1, "no new row created on accept");
    }

    // ── Task 02-01: add_initiative_reference_path appends a normalized host path ───────
    #[tokio::test]
    async fn add_initiative_reference_path_appends() {
        let (state, root) = test_state();
        let workspace = tmp_dir("ws");
        let store = tmp_dir("store").canonicalize().unwrap();
        settings_service::set_setting(
            &state,
            settings_service::KEY_INITIATIVES_DIR.to_string(),
            store.to_string_lossy().to_string(),
        )
        .unwrap();

        let plan = create_briefing_run(
            &state,
            CreateBriefingInput {
                title: Some("Ref".to_string()),
                workspace_path: workspace.to_string_lossy().to_string(),
                brief: Some("ask".to_string()),
                worker_provider: "claude_code".to_string(),
                reviewer_provider: "claude_code".to_string(),
                model: None,
            },
        )
        .await
        .expect("create_briefing_run")
        .plan;

        // A real directory inside the (canonical) workspace — reference paths must stay in-workspace.
        let ref_dir = Path::new(&plan.workspace_path).join("docs");
        std::fs::create_dir_all(&ref_dir).unwrap();

        let updated = add_initiative_reference_path(
            &state,
            &plan.id,
            ref_dir.to_string_lossy().as_ref(),
        )
        .expect("add_initiative_reference_path");

        let refs = updated.plan.reference_paths.expect("reference_paths set");
        assert!(
            refs.lines().any(|l| l == ref_dir.canonicalize().unwrap().to_string_lossy()),
            "reference_paths must contain the appended dir (have: {:?})",
            refs
        );

        let _ = std::fs::remove_dir_all(&workspace);
        let _ = std::fs::remove_dir_all(&store);
        let _ = std::fs::remove_dir_all(&root);
    }

    // ── Task 02-02: accept_brief guards non-briefing rows (no mutation) ────────────────
    #[tokio::test]
    async fn accept_brief_rejects_non_briefing() {
        let (state, root) = test_state();
        // A planning-status row (raw insert): accept must refuse and leave it untouched.
        state
            .db
            .with_conn(|conn| {
                conn.execute(
                    "INSERT INTO agent_plans (id, run_type, title, workspace_path, plan_path, status, worker_provider, reviewer_provider, brief, initiative_id, initiative_status) \
                     VALUES ('P', 'planning', 't', '/w', '/p', 'draft', 'claude_code', 'claude_code', 'brief', 'P', 'planning')",
                    [],
                )
                .map_err(|e| e.to_string())
            })
            .unwrap();

        let err = accept_brief(&state, "P", None)
            .await
            .expect_err("accept on a non-briefing row must error");
        assert!(err.contains("not in briefing"), "unexpected error: {}", err);

        // The row is unchanged — still planning, brief intact, no planner session provisioned.
        let (status, brief, worker): (String, Option<String>, Option<String>) = state
            .db
            .with_conn(|conn| {
                conn.query_row(
                    "SELECT initiative_status, brief, worker_session_id FROM agent_plans WHERE id='P'",
                    [],
                    |row| Ok((row.get(0)?, row.get(1)?, row.get(2)?)),
                )
                .map_err(|e| e.to_string())
            })
            .unwrap();
        assert_eq!(status, "planning", "guard must not mutate the row");
        assert_eq!(brief.as_deref(), Some("brief"));
        assert!(worker.is_none(), "no planner session on a rejected accept");

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

    // ── P7 Task 01-02: update_plan_validation_config persists / clears / validates ────
    #[test]
    fn update_plan_validation_config_persists_clears_and_validates() {
        let (state, root) = test_state();
        let workspace = tmp_dir("ws");
        let store = tmp_dir("store").canonicalize().unwrap();
        settings_service::set_setting(
            &state,
            settings_service::KEY_INITIATIVES_DIR.to_string(),
            store.to_string_lossy().to_string(),
        )
        .unwrap();

        let plan = create_planning_run(
            &state,
            input(Some("planning"), workspace.to_string_lossy().as_ref(), "docs/plans/vc"),
        )
        .expect("create_planning_run")
        .plan;

        // A valid 2-lens JSON persists and reloads; resolve returns those two (not the default 3).
        let two = r#"[{"name":"a","provider":"claude_code","blocking":true},{"name":"b","provider":"grok","blocking":false}]"#;
        let run = update_plan_validation_config(&state, plan.id.clone(), Some(two.to_string()))
            .expect("persist 2-lens config");
        assert!(run.plan.validation_config.is_some());
        let lenses = resolve_validation_lenses(&run);
        assert_eq!(lenses.len(), 2);
        assert_eq!(lenses[0].name, "a");
        assert_eq!(lenses[1].name, "b");
        assert!(!lenses[1].blocking);

        // Clearing (None) drops the column → resolve falls back to the 3-lens default.
        let run = update_plan_validation_config(&state, plan.id.clone(), None)
            .expect("clear config");
        assert!(run.plan.validation_config.is_none());
        assert_eq!(resolve_validation_lenses(&run).len(), 3);

        // Malformed JSON is rejected (not silently written).
        assert!(update_plan_validation_config(&state, plan.id.clone(), Some("{not json".into())).is_err());

        // A lens with an unknown/typo provider is rejected (D13).
        let bad = r#"[{"name":"x","provider":"claude_cod","blocking":true}]"#;
        let err = update_plan_validation_config(&state, plan.id.clone(), Some(bad.to_string()))
            .unwrap_err();
        assert!(
            err.contains("Unknown provider") && err.contains("claude_cod"),
            "{err}"
        );

        for provider in ["kloo", "shell"] {
            let json = format!(
                r#"[{{"name":"x","provider":"{}","blocking":true}}]"#,
                provider
            );
            let err = update_plan_validation_config(&state, plan.id.clone(), Some(json)).unwrap_err();
            assert!(
                err.contains("oneshot executor") && err.contains(provider),
                "{err}"
            );
        }

        // The column is still cleared (the two rejected writes never touched it).
        let reloaded = get_plan(&state, &plan.id).unwrap();
        assert!(reloaded.plan.validation_config.is_none());

        let _ = std::fs::remove_dir_all(&workspace);
        let _ = std::fs::remove_dir_all(&store);
        let _ = std::fs::remove_dir_all(&root);
    }

    // ── P7 Phase 02 (A3/A3v): the prompt builders emit the lens name, keep the
    // review-lenses.md pointer for a default lens, and append the custom rubric + vision clause. ──
    fn sample_run_for_prompt() -> AgentPlanRun {
        AgentPlanRun {
            plan: AgentPlan {
                id: "PLAN".into(),
                run_type: "development".into(),
                title: "t".into(),
                workspace_path: "/w".into(),
                plan_path: "/w/docs/plans/x".into(),
                status: "in_progress".into(),
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
                initiative_status: "development".into(),
                health: "in-progress".into(),
                briefing_session_id: None,
                created_at: "".into(),
                updated_at: "".into(),
                validation_config: None,
            },
            phases: vec![],
            tasks: vec![],
            events: vec![],
        }
    }

    fn sample_phase_for_prompt() -> AgentPlanPhase {
        AgentPlanPhase {
            id: "PH".into(),
            plan_id: "PLAN".into(),
            phase_id: "01-thing".into(),
            phase_title: "Thing".into(),
            phase_index: 0,
            status: "in_progress".into(),
            worker_started_at: None,
            worker_idle_at: None,
            reviewer_started_at: None,
            reviewer_idle_at: None,
            gate_verdict: "pending".into(),
            clarification_attempts: 0,
            summary: None,
            findings_json: "[]".into(),
            created_at: "".into(),
            updated_at: "".into(),
        }
    }

    #[test]
    fn lens_reviewer_prompts_carry_name_pointer_rubric_and_vision() {
        let (state, root) = test_state();
        let run = sample_run_for_prompt();
        let phase = sample_phase_for_prompt();

        // Default lens: name substituted, review-lenses.md pointer kept, no vision clause.
        let default_lens = ValidationLens {
            name: "product".into(),
            provider: "claude_code".into(),
            model: None,
            prompt: None,
            vision: false,
            blocking: true,
        };
        let dev = lens_reviewer_prompt(&state, &run, &phase, &default_lens, "SESS");
        assert!(dev.contains("Run ONLY the product lens"));
        assert!(dev.contains("review-lenses.md"));
        assert!(!dev.contains("functional grounds"));
        let plan = planning_lens_reviewer_prompt(&state, &run, &default_lens, "SESS");
        assert!(plan.contains("Run ONLY the product lens"));
        assert!(plan.contains("review-lenses.md"));
        assert!(!plan.contains("functional grounds"));

        // Vision + custom-rubric lens: clause + rubric present in BOTH builders (A3v).
        let vision_lens = ValidationLens {
            name: "security".into(),
            provider: "grok".into(),
            model: None,
            prompt: Some("Audit for leaked secrets.".into()),
            vision: true,
            blocking: false,
        };
        let devv = lens_reviewer_prompt(&state, &run, &phase, &vision_lens, "SESS");
        assert!(devv.contains("Run ONLY the security lens"));
        assert!(devv.contains("Audit for leaked secrets."));
        assert!(devv.contains("approve on functional grounds"));
        let planv = planning_lens_reviewer_prompt(&state, &run, &vision_lens, "SESS");
        assert!(planv.contains("Run ONLY the security lens"));
        assert!(planv.contains("Audit for leaked secrets."));
        assert!(planv.contains("approve on functional grounds"));

        let _ = std::fs::remove_dir_all(&root);
    }
}

// ── P7 Task 01-01: default template + resolve (pure — no DB) ──────────────────────────
#[cfg(test)]
mod validation_lens_tests {
    use super::*;

    /// Build a minimal `AgentPlanRun` carrying only the fields `resolve_validation_lenses` reads
    /// (`validation_config`, `reviewer_provider`); everything else is filler.
    fn plan_run(validation_config: Option<&str>, reviewer_provider: &str) -> AgentPlanRun {
        AgentPlanRun {
            plan: AgentPlan {
                id: "id".into(),
                run_type: "development".into(),
                title: "t".into(),
                workspace_path: "/w".into(),
                plan_path: "/p".into(),
                status: "draft".into(),
                worker_session_id: None,
                reviewer_session_id: None,
                worker_provider: "claude_code".into(),
                reviewer_provider: reviewer_provider.into(),
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
                initiative_status: "development".into(),
                health: "in-progress".into(),
                briefing_session_id: None,
                created_at: "".into(),
                updated_at: "".into(),
                validation_config: validation_config.map(|s| s.to_string()),
            },
            phases: vec![],
            tasks: vec![],
            events: vec![],
        }
    }

    #[test]
    fn default_validation_config_is_todays_three_lenses() {
        let lenses = default_validation_config("claude_code", None);
        assert_eq!(lenses.len(), 3);
        assert_eq!(
            lenses.iter().map(|l| l.name.as_str()).collect::<Vec<_>>(),
            ["product", "qa", "lead"]
        );
        for lens in &lenses {
            assert!(lens.blocking, "default lenses are all blocking");
            assert!(!lens.vision, "today's lenses are non-vision (D14)");
            assert_eq!(lens.prompt, None);
            assert_eq!(lens.provider, "claude_code");
            assert_eq!(lens.model, default_model_for_provider("claude_code"));
        }
    }

    #[test]
    fn default_validation_config_threads_provider_model() {
        // Grok seeds its stronger default model even when `model` is None.
        let lenses = default_validation_config("grok", None);
        assert!(lenses.iter().all(|l| l.provider == "grok"));
        assert!(lenses
            .iter()
            .all(|l| l.model.as_deref() == Some("grok-build")));
    }

    #[test]
    fn resolve_falls_back_to_default_when_none() {
        let run = plan_run(None, "claude_code");
        assert_eq!(
            resolve_validation_lenses(&run),
            default_validation_config("claude_code", None)
        );
    }

    #[test]
    fn resolve_falls_back_to_default_when_empty_array() {
        let run = plan_run(Some("[]"), "claude_code");
        assert_eq!(resolve_validation_lenses(&run).len(), 3);
    }

    #[test]
    fn resolve_falls_back_to_default_when_malformed() {
        let run = plan_run(Some("{ not json"), "claude_code");
        assert_eq!(resolve_validation_lenses(&run).len(), 3);
    }

    #[test]
    fn resolve_honors_one_lens_config() {
        let run = plan_run(
            Some(r#"[{"name":"solo","provider":"grok","blocking":true}]"#),
            "claude_code",
        );
        let lenses = resolve_validation_lenses(&run);
        assert_eq!(lenses.len(), 1);
        assert_eq!(lenses[0].name, "solo");
        // `vision` key absent → #[serde(default)] false.
        assert!(!lenses[0].vision);
    }

    #[test]
    fn resolve_honors_five_lens_config_in_order() {
        let cfg = r#"[
            {"name":"one","provider":"claude_code","blocking":true},
            {"name":"two","provider":"codex","blocking":false},
            {"name":"three","provider":"cline","blocking":true,"vision":true},
            {"name":"four","provider":"ollama","blocking":false},
            {"name":"five","provider":"grok","blocking":true}
        ]"#;
        let run = plan_run(Some(cfg), "claude_code");
        let lenses = resolve_validation_lenses(&run);
        assert_eq!(lenses.len(), 5);
        assert_eq!(
            lenses.iter().map(|l| l.name.as_str()).collect::<Vec<_>>(),
            ["one", "two", "three", "four", "five"]
        );
        // The vision:true lens round-trips its flag.
        assert!(lenses[2].vision);
        assert!(!lenses[0].vision);
    }

    #[test]
    fn validation_lens_json_round_trips_camelcase() {
        let lens = ValidationLens {
            name: "sec".into(),
            provider: "grok".into(),
            model: Some("grok-build".into()),
            prompt: Some("check secrets".into()),
            vision: true,
            blocking: false,
        };
        let v = serde_json::to_value(&lens).unwrap();
        assert_eq!(v.get("vision").and_then(|x| x.as_bool()), Some(true));
        assert_eq!(v.get("blocking").and_then(|x| x.as_bool()), Some(false));
        let back: ValidationLens = serde_json::from_value(v).unwrap();
        assert_eq!(back, lens);
    }
}

// ── P7 Phase 03: git_diff whole-tree diff (temp-repo) ─────────────────────────────────
#[cfg(test)]
mod git_diff_tests {
    use super::*;
    use crate::test_support::{test_state, tmp_dir};

    /// Run a git command in `dir`, asserting success (git identity/signing are set locally so the
    /// test never depends on the host's global git config).
    fn git(dir: &Path, args: &[&str]) {
        let out = std::process::Command::new("git")
            .args(args)
            .current_dir(dir)
            .output()
            .unwrap_or_else(|e| panic!("git {:?} failed to spawn: {}", args, e));
        assert!(
            out.status.success(),
            "git {:?} failed: {}",
            args,
            String::from_utf8_lossy(&out.stderr)
        );
    }

    fn init_repo(dir: &Path) {
        git(dir, &["init", "-q"]);
        git(dir, &["config", "user.email", "t@example.com"]);
        git(dir, &["config", "user.name", "Test"]);
        git(dir, &["config", "commit.gpgsign", "false"]);
    }

    /// Point `files_root` at `root` so the path guard accepts paths beneath it.
    fn set_files_root(state: &AppState, root: &Path) {
        settings_service::set_setting(
            state,
            settings_service::KEY_FILES_ROOT.to_string(),
            root.to_string_lossy().to_string(),
        )
        .unwrap();
    }

    #[test]
    fn git_diff_parses_counts_hunks_and_aggregates_files() {
        let (state, state_root) = test_state();
        let base = tmp_dir("gitdiff");
        set_files_root(&state, &base);
        let repo = base.join("repo");
        std::fs::create_dir_all(&repo).unwrap();
        init_repo(&repo);

        // Commit a.txt with 3 lines.
        std::fs::write(repo.join("a.txt"), "line1\nline2\nline3\n").unwrap();
        git(&repo, &["add", "-A"]);
        git(&repo, &["commit", "-q", "-m", "init"]);

        // Modify a.txt: delete line2 (-1), append newA + newB (+2).
        std::fs::write(repo.join("a.txt"), "line1\nline3\nnewA\nnewB\n").unwrap();

        let view = git_diff(&state, repo.to_string_lossy().to_string()).unwrap();
        assert!(!view.clean, "modified repo is not clean");
        assert!(view.branch.is_some(), "branch should resolve");
        assert!(view.repo_root.is_some());
        assert_eq!(view.files.len(), 1, "one changed file");
        let f = &view.files[0];
        assert_eq!(f.path, "a.txt");
        assert_eq!(f.additions, 2, "two additions");
        assert_eq!(f.deletions, 1, "one deletion");
        assert!(!f.binary);
        assert!(f.diff.contains("@@"), "diff carries a hunk header: {}", f.diff);

        // Commit b.txt ONLY (stage just b.txt so a.txt's working change stays dirty), then edit
        // b.txt → two files aggregate with per-file counts.
        std::fs::write(repo.join("b.txt"), "x\ny\n").unwrap();
        git(&repo, &["add", "b.txt"]);
        git(&repo, &["commit", "-q", "-m", "add b"]);
        std::fs::write(repo.join("b.txt"), "x\ny\nz\n").unwrap();

        let view2 = git_diff(&state, repo.to_string_lossy().to_string()).unwrap();
        assert_eq!(view2.files.len(), 2, "a.txt (still dirty) + b.txt");
        let by_path: std::collections::HashMap<_, _> =
            view2.files.iter().map(|f| (f.path.as_str(), f)).collect();
        assert_eq!(by_path["a.txt"].additions, 2);
        assert_eq!(by_path["a.txt"].deletions, 1);
        assert_eq!(by_path["b.txt"].additions, 1, "b.txt: one line appended");
        assert_eq!(by_path["b.txt"].deletions, 0);

        let _ = std::fs::remove_dir_all(&base);
        let _ = std::fs::remove_dir_all(&state_root);
    }

    #[test]
    fn git_diff_clean_repo_has_no_files() {
        let (state, state_root) = test_state();
        let base = tmp_dir("gitdiff-clean");
        set_files_root(&state, &base);
        let repo = base.join("repo");
        std::fs::create_dir_all(&repo).unwrap();
        init_repo(&repo);
        std::fs::write(repo.join("a.txt"), "hello\n").unwrap();
        git(&repo, &["add", "-A"]);
        git(&repo, &["commit", "-q", "-m", "init"]);

        // No working-tree changes → clean, no files, but it IS a repo (repo_root Some).
        let view = git_diff(&state, repo.to_string_lossy().to_string()).unwrap();
        assert!(view.clean);
        assert!(view.files.is_empty());
        assert!(view.repo_root.is_some());

        let _ = std::fs::remove_dir_all(&base);
        let _ = std::fs::remove_dir_all(&state_root);
    }

    #[test]
    fn git_diff_non_repo_is_benign_empty() {
        let (state, state_root) = test_state();
        let base = tmp_dir("gitdiff-norepo");
        set_files_root(&state, &base);
        let plain = base.join("plain");
        std::fs::create_dir_all(&plain).unwrap();
        std::fs::write(plain.join("readme.txt"), "not a repo\n").unwrap();

        let view = git_diff(&state, plain.to_string_lossy().to_string()).unwrap();
        assert!(view.clean, "non-repo path is clean");
        assert!(view.files.is_empty());
        assert!(view.repo_root.is_none(), "no repo_root for a non-repo path");
        assert!(view.branch.is_none());

        let _ = std::fs::remove_dir_all(&base);
        let _ = std::fs::remove_dir_all(&state_root);
    }

    #[test]
    fn git_diff_rejects_path_outside_files_root() {
        // Path guard (D13): a path above the configured files_root is rejected, not diffed.
        let (state, state_root) = test_state();
        let base = tmp_dir("gitdiff-guard");
        set_files_root(&state, &base.join("inner"));
        std::fs::create_dir_all(base.join("inner")).unwrap();
        // `base` is the parent of the configured root → outside it.
        let outside = base.to_string_lossy().to_string();
        assert!(git_diff(&state, outside).is_err());
        let _ = std::fs::remove_dir_all(&base);
        let _ = std::fs::remove_dir_all(&state_root);
    }

    #[test]
    fn parse_numstat_z_handles_normal_and_rename_records() {
        // Normal file: add\tdel\tpath\0
        let normal = "2\t1\ta.txt\0";
        let recs = parse_numstat_z(normal);
        assert_eq!(recs.len(), 1);
        assert_eq!(recs[0].path, "a.txt");
        assert_eq!(recs[0].additions, 2);
        assert_eq!(recs[0].deletions, 1);
        assert!(recs[0].old_path.is_none());
        assert!(!recs[0].binary);

        // Rename: add\tdel\t\0old\0new\0
        let rename = "0\t0\t\0old.txt\0new.txt\0";
        let recs = parse_numstat_z(rename);
        assert_eq!(recs.len(), 1);
        assert_eq!(recs[0].path, "new.txt");
        assert_eq!(recs[0].old_path.as_deref(), Some("old.txt"));

        // Binary: -\t-\tbin.png\0
        let binary = "-\t-\tbin.png\0";
        let recs = parse_numstat_z(binary);
        assert_eq!(recs.len(), 1);
        assert!(recs[0].binary);
        assert_eq!(recs[0].additions, 0);
        assert_eq!(recs[0].deletions, 0);
    }
}

#[cfg(test)]
mod plan_check_gate_tests {
    use super::*;
    use crate::services::plan_check::{
        PlanCheckHost, RULE_IDS, RULE_MISSING_FILES, SpawnKind, SpawnRequest,
        SpawnResult, TokensError, TokensReport, MAX_PLANNING_CHECK_MS,
    };
    use crate::services::task_loop::{followup_after_kloo_phase, KlooPhaseFollowup, PhaseLoopOutcome};

    fn gate_state() -> (AppState, PathBuf, PathBuf, PathBuf) {
        let (state, root) = {
            let root = {
                let d = std::env::temp_dir().join(format!(
                    "j1-gate-{}-{}",
                    std::process::id(),
                    std::time::SystemTime::now()
                        .duration_since(std::time::UNIX_EPOCH)
                        .unwrap()
                        .as_nanos()
                ));
                std::fs::create_dir_all(&d).unwrap();
                d
            };
            let db = crate::db::Database::open(&root.join("test.db")).expect("open db");
            (AppState::new(db), root)
        };
        let store = root.join("store");
        let ws = root.join("ws");
        std::fs::create_dir_all(&store).unwrap();
        std::fs::create_dir_all(ws.join("src")).unwrap();
        std::fs::write(ws.join("src/a.rs"), "x\n").unwrap();
        settings_service::set_setting(
            &state,
            settings_service::KEY_INITIATIVES_DIR.to_string(),
            store.to_string_lossy().to_string(),
        )
        .unwrap();
        (state, root, store, ws)
    }

    fn write_small_task(plan: &Path, phase: &str, id: &str, yml: &str) {
        if !plan.join("overview.md").is_file() {
            std::fs::create_dir_all(plan).unwrap();
            std::fs::write(plan.join("overview.md"), "# Plan\n").unwrap();
        }
        let pov = plan.join("phases").join(phase).join("overview.md");
        if !pov.is_file() {
            std::fs::create_dir_all(pov.parent().unwrap()).unwrap();
            std::fs::write(&pov, format!("# {phase}\n")).unwrap();
        }
        let dir = plan.join("phases").join(phase).join("tasks").join(id);
        std::fs::create_dir_all(&dir).unwrap();
        std::fs::write(dir.join("task.yml"), yml).unwrap();
        std::fs::write(dir.join("prompt.md"), "implement add\n").unwrap();
    }

    struct SilentHost;

    impl PlanCheckHost for SilentHost {
        fn spawn(&self, req: &SpawnRequest) -> SpawnResult {
            match req.kind {
                SpawnKind::Warm => SpawnResult::ok_zero(),
                SpawnKind::Task => SpawnResult::exit(1),
            }
        }
        fn tokens(&self, _ctx: u32, _p: &Path) -> Result<TokensReport, TokensError> {
            Ok(TokensReport {
                fits: true,
                approx_tokens: Some(1),
                usable_window: Some(26214),
                compact_trigger: Some(1),
            })
        }
    }

    #[tokio::test]
    async fn commercial_planning_ready_skips_plan_check() {
        let (state, root, _store, ws) = gate_state();
        let plan_dir = root.join("plan");
        std::fs::create_dir_all(plan_dir.join("phases/01-x/tasks/01-y")).unwrap();
        std::fs::write(plan_dir.join("overview.md"), "# t\n").unwrap();
        std::fs::write(plan_dir.join("phases/01-x/overview.md"), "# p\n").unwrap();
        std::fs::write(plan_dir.join("phases/01-x/tasks/01-y/prompt.md"), "# t\n").unwrap();
        let run = create_plan(
            &state,
            CreateAgentPlanInput {
                run_type: Some("planning".into()),
                title: Some("c".into()),
                workspace_path: ws.to_string_lossy().into(),
                plan_path: plan_dir.to_string_lossy().into(),
                worker_provider: "claude_code".into(),
                reviewer_provider: "claude_code".into(),
                brief: None,
                app_scope: None,
                docs_scope: None,
                reference_paths: None,
                worker_setup_commands: None,
                reviewer_setup_commands: None,
            },
        )
        .unwrap();
        assert!(!is_local_small(&state, &run.plan.id).unwrap());
        let host = SilentHost;
        gate_planning_lenses(
            &state,
            &run,
            &PlanningCheckCtrl {
                host: Some(&host),
                stage_budget_ms: MAX_PLANNING_CHECK_MS,
                send_planner: false,
                elapsed_ms: None,
            },
        )
        .await
        .unwrap();
        let after = get_plan(&state, &run.plan.id).unwrap();
        assert_eq!(after.plan.status, "planning_review_running");
        assert!(after
            .events
            .iter()
            .any(|e| e.event_type == "planning_review_started"));
        assert!(!plan_check_json_path(&state, &after).exists());
        let _ = std::fs::remove_dir_all(&root);
    }

    #[tokio::test]
    async fn local_small_failing_check_does_not_dispatch_review() {
        let (state, root, _store, ws) = gate_state();
        let run = create_plan(
            &state,
            CreateAgentPlanInput {
                run_type: Some("planning".into()),
                title: Some("ls".into()),
                workspace_path: ws.to_string_lossy().into(),
                plan_path: root.join("unused").to_string_lossy().into(),
                worker_provider: "claude_code".into(),
                reviewer_provider: "claude_code".into(),
                brief: None,
                app_scope: None,
                docs_scope: None,
                reference_paths: None,
                worker_setup_commands: None,
                reviewer_setup_commands: None,
            },
        )
        .unwrap();
        set_executor_config(
            &state,
            &run.plan.id,
            r#"{"mode":"local-small"}"#,
        )
        .unwrap();
        // Plant two distinct static failures in the store plan path.
        write_small_task(
            Path::new(&run.plan.plan_path),
            "00-x",
            "01-a",
            "id: 01-a\nfiles: []\nverify: \"cargo test a -- --exact\"\n",
        );
        write_small_task(
            Path::new(&run.plan.plan_path),
            "00-x",
            "01-b",
            "id: 01-b\nfiles: [src/a.rs]\nverify: \"rm -rf /\"\nmust_contain: [\"pub fn add\"]\n",
        );
        let host = SilentHost;
        gate_planning_lenses(
            &state,
            &run,
            &PlanningCheckCtrl {
                host: Some(&host),
                stage_budget_ms: MAX_PLANNING_CHECK_MS,
                send_planner: false,
                elapsed_ms: None,
            },
        )
        .await
        .unwrap();
        let after = get_plan(&state, &run.plan.id).unwrap();
        assert_eq!(after.plan.status, "planning_planner_running");
        assert!(after
            .events
            .iter()
            .any(|e| e.event_type == "planning_check_failed"));
        assert!(!after
            .events
            .iter()
            .any(|e| e.event_type == "planning_review_started"));
        let findings = std::fs::read_to_string(check_findings_path(&after.plan.plan_path)).unwrap();
        let parsed: Vec<serde_json::Value> = serde_json::from_str(&findings).unwrap();
        let rules: std::collections::HashSet<_> = parsed
            .iter()
            .filter_map(|v| v.get("rule").and_then(|r| r.as_str()))
            .collect();
        assert!(rules.len() >= 2, "{parsed:?}");
        assert!(plan_check_json_path(&state, &after).exists());
        let ev = after
            .events
            .iter()
            .find(|e| e.event_type == "planning_check_failed")
            .unwrap();
        let payload: serde_json::Value = serde_json::from_str(&ev.payload_json).unwrap();
        assert!(payload.get("findingsPath").is_some());
        assert!(payload.get("countsByRule").is_some());
        let _ = std::fs::remove_dir_all(&root);
    }

    #[tokio::test]
    async fn empty_sibling_phase_names_the_phase() {
        let (state, root, _store, ws) = gate_state();
        let run = create_plan(
            &state,
            CreateAgentPlanInput {
                run_type: Some("planning".into()),
                title: Some("mix".into()),
                workspace_path: ws.to_string_lossy().into(),
                plan_path: root.join("unused").to_string_lossy().into(),
                worker_provider: "claude_code".into(),
                reviewer_provider: "claude_code".into(),
                brief: None,
                app_scope: None,
                docs_scope: None,
                reference_paths: None,
                worker_setup_commands: None,
                reviewer_setup_commands: None,
            },
        )
        .unwrap();
        set_executor_config(&state, &run.plan.id, r#"{"mode":"local-small"}"#).unwrap();
        write_small_task(
            Path::new(&run.plan.plan_path),
            "00-ok",
            "01-a",
            "id: 01-a\nfiles: [src/a.rs]\nverify: \"cargo test a -- --exact\"\nmust_contain: [\"pub fn add\"]\n",
        );
        std::fs::create_dir_all(Path::new(&run.plan.plan_path).join("phases/01-empty/tasks"))
            .unwrap();
        std::fs::write(
            Path::new(&run.plan.plan_path).join("phases/01-empty/overview.md"),
            "# empty\n",
        )
        .unwrap();
        let host = SilentHost;
        gate_planning_lenses(
            &state,
            &run,
            &PlanningCheckCtrl {
                host: Some(&host),
                stage_budget_ms: MAX_PLANNING_CHECK_MS,
                send_planner: false,
                elapsed_ms: None,
            },
        )
        .await
        .unwrap();
        let after = get_plan(&state, &run.plan.id).unwrap();
        assert!(!after
            .events
            .iter()
            .any(|e| e.event_type == "planning_review_started"));
        let findings = std::fs::read_to_string(check_findings_path(&after.plan.plan_path)).unwrap();
        assert!(
            findings.contains("01-empty"),
            "empty phase named in findings: {findings}"
        );
        let _ = std::fs::remove_dir_all(&root);
    }

    #[tokio::test]
    async fn rerun_reviewer_local_small_does_not_skip_check() {
        let (state, root, _store, ws) = gate_state();
        let run = create_plan(
            &state,
            CreateAgentPlanInput {
                run_type: Some("planning".into()),
                title: Some("rerun".into()),
                workspace_path: ws.to_string_lossy().into(),
                plan_path: root.join("unused").to_string_lossy().into(),
                worker_provider: "claude_code".into(),
                reviewer_provider: "claude_code".into(),
                brief: None,
                app_scope: None,
                docs_scope: None,
                reference_paths: None,
                worker_setup_commands: None,
                reviewer_setup_commands: None,
            },
        )
        .unwrap();
        set_executor_config(&state, &run.plan.id, r#"{"mode":"local-small"}"#).unwrap();
        write_small_task(
            Path::new(&run.plan.plan_path),
            "00-x",
            "01-a",
            "id: 01-a\nfiles: []\nverify: \"cargo test a -- --exact\"\n",
        );
        // Live ctrl would spawn real processes; the failing shape check
        // does not need a host. gate via rerun_reviewer uses live() which
        // is host-less — shape-only failure (empty files) still blocks.
        let after = rerun_reviewer(state.clone(), run.plan.id.clone())
            .await
            .unwrap();
        assert_ne!(after.plan.status, "planning_review_running");
        assert!(!after
            .events
            .iter()
            .any(|e| e.event_type == "planning_review_started"));
        let _ = std::fs::remove_dir_all(&root);
    }

    #[tokio::test]
    async fn local_small_clean_plan_dispatches_review() {
        let (state, root, _store, ws) = gate_state();
        let run = create_plan(
            &state,
            CreateAgentPlanInput {
                run_type: Some("planning".into()),
                title: Some("ok".into()),
                workspace_path: ws.to_string_lossy().into(),
                plan_path: root.join("unused").to_string_lossy().into(),
                worker_provider: "claude_code".into(),
                reviewer_provider: "claude_code".into(),
                brief: None,
                app_scope: None,
                docs_scope: None,
                reference_paths: None,
                worker_setup_commands: None,
                reviewer_setup_commands: None,
            },
        )
        .unwrap();
        set_executor_config(&state, &run.plan.id, r#"{"mode":"local-small"}"#).unwrap();
        write_small_task(
            Path::new(&run.plan.plan_path),
            "00-x",
            "01-a",
            "id: 01-a\nfiles: [src/a.rs]\nverify: \"cargo test a -- --exact\"\nmust_contain: [\"pub fn add\"]\n",
        );
        let host = SilentHost;
        gate_planning_lenses(
            &state,
            &run,
            &PlanningCheckCtrl {
                host: Some(&host),
                stage_budget_ms: MAX_PLANNING_CHECK_MS,
                send_planner: false,
                elapsed_ms: None,
            },
        )
        .await
        .unwrap();
        let after = get_plan(&state, &run.plan.id).unwrap();
        assert!(after
            .events
            .iter()
            .any(|e| e.event_type == "planning_check_passed"));
        assert!(after
            .events
            .iter()
            .any(|e| e.event_type == "planning_review_started"));
        assert_eq!(after.plan.status, "planning_review_running");
        let _ = std::fs::remove_dir_all(&root);
    }

    #[tokio::test]
    async fn planning_check_ceiling_skips_later_phases() {
        let (state, root, _store, ws) = gate_state();
        let run = create_plan(
            &state,
            CreateAgentPlanInput {
                run_type: Some("planning".into()),
                title: Some("ceil".into()),
                workspace_path: ws.to_string_lossy().into(),
                plan_path: root.join("unused").to_string_lossy().into(),
                worker_provider: "claude_code".into(),
                reviewer_provider: "claude_code".into(),
                brief: None,
                app_scope: None,
                docs_scope: None,
                reference_paths: None,
                worker_setup_commands: None,
                reviewer_setup_commands: None,
            },
        )
        .unwrap();
        set_executor_config(&state, &run.plan.id, r#"{"mode":"local-small"}"#).unwrap();
        for phase in ["00-a", "01-b", "02-c"] {
            for i in 0..40 {
                let id = format!("t{i:02}");
                std::fs::write(ws.join(format!("src/{phase}-{id}.rs")), "x\n").ok();
                write_small_task(
                    Path::new(&run.plan.plan_path),
                    phase,
                    &id,
                    &format!(
                        "id: {id}\nfiles: [src/{phase}-{id}.rs]\nverify: \"cargo test a -- --exact\"\nmust_contain: [\"pub fn add\"]\n"
                    ),
                );
            }
        }
        let host = SilentHost;
        let ticks = std::sync::Arc::new(std::sync::atomic::AtomicU64::new(0));
        let ticks_c = ticks.clone();
        let clock: std::sync::Arc<dyn Fn() -> u64 + Send + Sync> = std::sync::Arc::new(move || {
            let n = ticks_c.fetch_add(1, std::sync::atomic::Ordering::SeqCst);
            if n == 0 {
                0
            } else {
                10
            }
        });
        gate_planning_lenses(
            &state,
            &run,
            &PlanningCheckCtrl {
                host: Some(&host),
                stage_budget_ms: 1,
                send_planner: false,
                elapsed_ms: Some(clock),
            },
        )
        .await
        .unwrap();
        let after = get_plan(&state, &run.plan.id).unwrap();
        let phases: Vec<_> = after
            .events
            .iter()
            .filter(|e| e.event_type == "planning_check_phase")
            .collect();
        assert_eq!(phases.len(), 3, "one progress event per phase");
        let report =
            crate::services::plan_check::load_plan_check(&plan_check_json_path(&state, &after))
                .unwrap();
        let later: usize = report
            .phases
            .iter()
            .filter(|p| p.phase_id != "00-a")
            .map(|p| p.verify_skipped)
            .sum();
        assert!(later >= 40, "later phases skipped: {:?}", report.phases);
        assert!(
            report
                .items
                .iter()
                .any(|i| i.rule == crate::services::plan_check::RULE_TOKENS_UNAVAILABLE
                    && i.detail.contains("stage budget")
                    && !i.blocking),
            "tokens skip is advisory: {:?}",
            report.items
        );
        assert!(report.passed, "ceiling-only must not fail the plan: {:?}", report.items);
        assert!(after
            .events
            .iter()
            .any(|e| e.event_type == "planning_check_passed"));
        assert!(after
            .events
            .iter()
            .any(|e| e.event_type == "planning_review_started"));
        assert_eq!(after.plan.status, "planning_review_running");
        let _ = std::fs::remove_dir_all(&root);
    }

    #[tokio::test]
    async fn planning_check_three_phases_cover_all() {
        let (state, root, _store, ws) = gate_state();
        let run = create_plan(
            &state,
            CreateAgentPlanInput {
                run_type: Some("planning".into()),
                title: Some("3x40".into()),
                workspace_path: ws.to_string_lossy().into(),
                plan_path: root.join("unused").to_string_lossy().into(),
                worker_provider: "claude_code".into(),
                reviewer_provider: "claude_code".into(),
                brief: None,
                app_scope: None,
                docs_scope: None,
                reference_paths: None,
                worker_setup_commands: None,
                reviewer_setup_commands: None,
            },
        )
        .unwrap();
        set_executor_config(&state, &run.plan.id, r#"{"mode":"local-small"}"#).unwrap();
        for phase in ["00-a", "01-b", "02-c"] {
            for i in 0..40 {
                let id = format!("t{i:02}");
                std::fs::write(ws.join(format!("src/{phase}-{id}.rs")), "x\n").ok();
                write_small_task(
                    Path::new(&run.plan.plan_path),
                    phase,
                    &id,
                    &format!(
                        "id: {id}\nfiles: [src/{phase}-{id}.rs]\nverify: \"cargo test a -- --exact\"\nmust_contain: [\"pub fn add\"]\n"
                    ),
                );
            }
        }
        let host = SilentHost;
        gate_planning_lenses(
            &state,
            &run,
            &PlanningCheckCtrl {
                host: Some(&host),
                stage_budget_ms: MAX_PLANNING_CHECK_MS,
                send_planner: false,
                elapsed_ms: None,
            },
        )
        .await
        .unwrap();
        let after = get_plan(&state, &run.plan.id).unwrap();
        let report =
            crate::services::plan_check::load_plan_check(&plan_check_json_path(&state, &after))
                .unwrap();
        assert_eq!(report.verify_skipped, 0, "{:?}", report.phases);
        assert_eq!(report.verify_executed, 120, "{:?}", report.phases);
        let _ = std::fs::remove_dir_all(&root);
    }

    #[tokio::test]
    async fn planning_check_exhausts_revision_rounds() {
        let (state, root, _store, ws) = gate_state();
        let run = create_plan(
            &state,
            CreateAgentPlanInput {
                run_type: Some("planning".into()),
                title: Some("cap".into()),
                workspace_path: ws.to_string_lossy().into(),
                plan_path: root.join("unused").to_string_lossy().into(),
                worker_provider: "claude_code".into(),
                reviewer_provider: "claude_code".into(),
                brief: None,
                app_scope: None,
                docs_scope: None,
                reference_paths: None,
                worker_setup_commands: None,
                reviewer_setup_commands: None,
            },
        )
        .unwrap();
        set_executor_config(&state, &run.plan.id, r#"{"mode":"local-small"}"#).unwrap();
        write_small_task(
            Path::new(&run.plan.plan_path),
            "00-x",
            "01-a",
            "id: 01-a\nfiles: []\nverify: \"cargo test a -- --exact\"\n",
        );
        let host = SilentHost;
        let ctrl = PlanningCheckCtrl {
            host: Some(&host),
            stage_budget_ms: MAX_PLANNING_CHECK_MS,
            send_planner: false,
            elapsed_ms: None,
        };
        for _ in 0..MAX_REVISION_ROUNDS {
            let current = get_plan(&state, &run.plan.id).unwrap();
            gate_planning_lenses(&state, &current, &ctrl).await.unwrap();
        }
        let after = get_plan(&state, &run.plan.id).unwrap();
        assert_eq!(after.plan.status, "needs_attention");
        assert!(after
            .events
            .iter()
            .any(|e| e.event_type == "planning_needs_attention"));
        let _ = std::fs::remove_dir_all(&root);
    }

    #[tokio::test]
    async fn lens_needs_changes_after_check_pass_climbs_to_cap() {
        let (state, root, _store, ws) = gate_state();
        let run = create_plan(
            &state,
            CreateAgentPlanInput {
                run_type: Some("planning".into()),
                title: Some("mix-cap".into()),
                workspace_path: ws.to_string_lossy().into(),
                plan_path: root.join("unused").to_string_lossy().into(),
                worker_provider: "claude_code".into(),
                reviewer_provider: "claude_code".into(),
                brief: None,
                app_scope: None,
                docs_scope: None,
                reference_paths: None,
                worker_setup_commands: None,
                reviewer_setup_commands: None,
            },
        )
        .unwrap();
        set_executor_config(&state, &run.plan.id, r#"{"mode":"local-small"}"#).unwrap();
        write_small_task(
            Path::new(&run.plan.plan_path),
            "00-x",
            "01-a",
            "id: 01-a\nfiles: [src/a.rs]\nverify: \"cargo test a -- --exact\"\nmust_contain: [\"pub fn add\"]\n",
        );
        // Five (check-pass + lens NEEDS_CHANGES) pairs. A check-pass must not
        // reset the lens counter (that was the r2 hole).
        for i in 1..=5 {
            append_event(
                &state,
                &run.plan.id,
                None,
                "planning_check_passed",
                json!({}),
            )
            .unwrap();
            append_event(
                &state,
                &run.plan.id,
                None,
                "planning_gate_result",
                json!({ "verdict": "NEEDS_CHANGES" }),
            )
            .unwrap();
            let current = get_plan(&state, &run.plan.id).unwrap();
            assert_eq!(
                consecutive_non_pass_planning_rounds(&current),
                i,
                "round should climb, not reset at check-pass"
            );
        }
        append_event(
            &state,
            &run.plan.id,
            None,
            "planning_check_passed",
            json!({}),
        )
        .unwrap();
        let current = get_plan(&state, &run.plan.id).unwrap();
        handle_planning_reviewer_output(
            &state,
            &current,
            "VERDICT: NEEDS_CHANGES\nSUMMARY: still failing\nFINDINGS: x\n",
        )
        .await
        .unwrap();
        let after = get_plan(&state, &run.plan.id).unwrap();
        assert_eq!(after.plan.status, "needs_attention");
        assert!(after
            .events
            .iter()
            .any(|e| e.event_type == "planning_needs_attention"));
        let _ = std::fs::remove_dir_all(&root);
    }

    #[tokio::test]
    async fn preflight_fail_persists_and_does_not_pass() {
        let (state, root, _store, ws) = gate_state();
        let plan_dir = root.join("plan");
        write_small_task(
            &plan_dir,
            "00-x",
            "01-a",
            "id: 01-a\nfiles: [src/a.rs]\nverify: \"cargo test a -- --exact\"\nmust_contain: [\"pub fn add\"]\n",
        );
        write_small_task(
            &plan_dir,
            "00-x",
            "02-b",
            "id: 02-b\nfiles: [src/a.rs]\nverify: \"cargo test b -- --exact\"\nmust_contain: [\"pub fn add\"]\n",
        );
        let run = create_plan(
            &state,
            CreateAgentPlanInput {
                run_type: Some("development".into()),
                title: Some("pf".into()),
                workspace_path: ws.to_string_lossy().into(),
                plan_path: plan_dir.to_string_lossy().into(),
                worker_provider: "claude_code".into(),
                reviewer_provider: "claude_code".into(),
                brief: None,
                app_scope: None,
                docs_scope: None,
                reference_paths: None,
                worker_setup_commands: None,
                reviewer_setup_commands: None,
            },
        )
        .unwrap();
        let phase = run.phases.first().cloned().unwrap();
        let host = SilentHost;
        let ok = run_phase_preflight_with(&state, &run, &phase, Some(&host), None)
            .await
            .unwrap();
        assert!(!ok);
        let after = get_plan(&state, &run.plan.id).unwrap();
        assert_eq!(after.plan.status, "needs_attention");
        let ev = after
            .events
            .iter()
            .find(|e| e.event_type == "agent_phase_preflight_failed")
            .unwrap();
        let payload: serde_json::Value = serde_json::from_str(&ev.payload_json).unwrap();
        assert_eq!(
            event_summary(ev, &payload),
            "Phase 00 preflight failed — 1 violations."
        );
        let runs = settings_service::initiative_runs_path(
            &settings_service::resolve_initiatives_dir(&state),
            &run.plan.initiative_id,
            &run.plan.id,
            &phase.phase_id,
        );
        assert!(runs.join("preflight.json").exists());
        let _ = std::fs::remove_dir_all(&root);
    }

    #[tokio::test]
    async fn preflight_clean_passes() {
        let (state, root, _store, ws) = gate_state();
        let plan_dir = root.join("plan");
        write_small_task(
            &plan_dir,
            "00-x",
            "01-a",
            "id: 01-a\nfiles: [src/a.rs]\nverify: \"cargo test a -- --exact\"\nmust_contain: [\"pub fn add\"]\n",
        );
        let run = create_plan(
            &state,
            CreateAgentPlanInput {
                run_type: Some("development".into()),
                title: Some("pfok".into()),
                workspace_path: ws.to_string_lossy().into(),
                plan_path: plan_dir.to_string_lossy().into(),
                worker_provider: "claude_code".into(),
                reviewer_provider: "claude_code".into(),
                brief: None,
                app_scope: None,
                docs_scope: None,
                reference_paths: None,
                worker_setup_commands: None,
                reviewer_setup_commands: None,
            },
        )
        .unwrap();
        let phase = run.phases.first().cloned().unwrap();
        let host = SilentHost;
        let ok = run_phase_preflight_with(&state, &run, &phase, Some(&host), None)
            .await
            .unwrap();
        assert!(ok);
        let after = get_plan(&state, &run.plan.id).unwrap();
        assert!(after
            .events
            .iter()
            .any(|e| e.event_type == "agent_phase_preflight_passed"));
        let _ = std::fs::remove_dir_all(&root);
    }

    #[tokio::test]
    async fn missing_kloo_tokens_is_environment_not_planner() {
        let (state, root, _store, ws) = gate_state();
        let run = create_plan(
            &state,
            CreateAgentPlanInput {
                run_type: Some("planning".into()),
                title: Some("nokloo".into()),
                workspace_path: ws.to_string_lossy().into(),
                plan_path: root.join("unused").to_string_lossy().into(),
                worker_provider: "claude_code".into(),
                reviewer_provider: "claude_code".into(),
                brief: None,
                app_scope: None,
                docs_scope: None,
                reference_paths: None,
                worker_setup_commands: None,
                reviewer_setup_commands: None,
            },
        )
        .unwrap();
        set_executor_config(&state, &run.plan.id, r#"{"mode":"local-small"}"#).unwrap();
        write_small_task(
            Path::new(&run.plan.plan_path),
            "00-x",
            "01-a",
            "id: 01-a\nfiles: [src/a.rs]\nverify: \"cargo test a -- --exact\"\nmust_contain: [\"pub fn add\"]\n",
        );
        struct NoKloo;
        impl PlanCheckHost for NoKloo {
            fn spawn(&self, req: &SpawnRequest) -> SpawnResult {
                match req.kind {
                    SpawnKind::Warm => SpawnResult::ok_zero(),
                    SpawnKind::Task => SpawnResult::exit(1),
                }
            }
            fn tokens(&self, _c: u32, _p: &Path) -> Result<TokensReport, TokensError> {
                Err(TokensError::Unavailable("no kloo on PATH".into()))
            }
        }
        let host = NoKloo;
        gate_planning_lenses(
            &state,
            &run,
            &PlanningCheckCtrl {
                host: Some(&host),
                stage_budget_ms: MAX_PLANNING_CHECK_MS,
                send_planner: false,
                elapsed_ms: None,
            },
        )
        .await
        .unwrap();
        let after = get_plan(&state, &run.plan.id).unwrap();
        assert_eq!(after.plan.status, "needs_attention");
        assert!(!after
            .events
            .iter()
            .any(|e| e.event_type == "planning_review_started"));
        assert!(after
            .events
            .iter()
            .any(|e| e.event_type == "planning_needs_attention"));
        let _ = std::fs::remove_dir_all(&root);
    }

    #[tokio::test]
    async fn preflight_tokens_budget_is_advisory() {
        let (state, root, _store, ws) = gate_state();
        let plan_dir = root.join("plan");
        write_small_task(
            &plan_dir,
            "00-x",
            "01-a",
            "id: 01-a\nfiles: [src/a.rs]\nverify: \"cargo test a -- --exact\"\nmust_contain: [\"pub fn add\"]\n",
        );
        let run = create_plan(
            &state,
            CreateAgentPlanInput {
                run_type: Some("development".into()),
                title: Some("pftok".into()),
                workspace_path: ws.to_string_lossy().into(),
                plan_path: plan_dir.to_string_lossy().into(),
                worker_provider: "claude_code".into(),
                reviewer_provider: "claude_code".into(),
                brief: None,
                app_scope: None,
                docs_scope: None,
                reference_paths: None,
                worker_setup_commands: None,
                reviewer_setup_commands: None,
            },
        )
        .unwrap();
        let phase = run.phases.first().cloned().unwrap();
        let host = SilentHost;
        let ok = run_phase_preflight_with(&state, &run, &phase, Some(&host), Some(0))
            .await
            .unwrap();
        assert!(ok, "budget-skip tokens must not fail preflight");
        let after = get_plan(&state, &run.plan.id).unwrap();
        assert!(after
            .events
            .iter()
            .any(|e| e.event_type == "agent_phase_preflight_passed"));
        let runs = settings_service::initiative_runs_path(
            &settings_service::resolve_initiatives_dir(&state),
            &run.plan.initiative_id,
            &run.plan.id,
            &phase.phase_id,
        );
        let pf = crate::services::plan_check::load_plan_check(&runs.join("preflight.json")).unwrap();
        assert!(
            pf.items.iter().any(|i| {
                i.rule == crate::services::plan_check::RULE_TOKENS_UNAVAILABLE && !i.blocking
            }),
            "{:?}",
            pf.items
        );
        let _ = std::fs::remove_dir_all(&root);
    }

    #[test]
    fn commercial_development_without_task_yml_is_not_kloo() {
        let root = std::env::temp_dir().join(format!(
            "j1-comm-{}-{}",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        ));
        let phase = root.join("phases/00-x");
        std::fs::create_dir_all(phase.join("tasks/01-y")).unwrap();
        std::fs::write(phase.join("tasks/01-y/prompt.md"), "p\n").unwrap();
        assert_eq!(development_arm(&phase), DevArm::PersistentWorker);
        assert!(!crate::services::task_loop::phase_is_kloo_mode(&phase));
        let _ = std::fs::remove_dir_all(&root);
    }

    #[test]
    fn kloo_all_done_still_dispatches_review() {
        assert_eq!(
            followup_after_kloo_phase(&PhaseLoopOutcome::AllDone),
            KlooPhaseFollowup::DispatchReview
        );
    }

    #[test]
    fn planning_planner_ready_does_not_claim_review() {
        assert_eq!(
            event_status_transition("planning_planner_ready", &json!({})),
            (Some("planning_planner_running"), None)
        );
        assert_eq!(
            event_status_transition("planning_review_started", &json!({})),
            (Some("planning_planner_running"), Some("planning_review_running"))
        );
    }

    #[test]
    fn planning_check_event_names_and_prefix() {
        for ty in [
            "planning_check_passed",
            "planning_check_failed",
            "planning_check_phase",
        ] {
            assert!(ty.starts_with("planning_"));
            assert_eq!(event_category(ty), "planning");
            assert_eq!(event_actor(ty), "coordinator");
        }
        assert_eq!(event_category("agent_phase_preflight_failed"), "phase");
        let ev = AgentPlanEvent {
            id: "e".into(),
            plan_id: "p".into(),
            phase_id: Some("00-x".into()),
            phase_index: None,
            phase_title: None,
            event_type: "agent_phase_preflight_failed".into(),
            actor: "coordinator".into(),
            category: "phase".into(),
            summary: String::new(),
            status_before: None,
            status_after: None,
            reason: None,
            verdict: None,
            task_id: None,
            clarification_attempt: None,
            payload_json: r#"{"violations":3,"nn":"00"}"#.into(),
            created_at: String::new(),
        };
        let payload: serde_json::Value = serde_json::from_str(&ev.payload_json).unwrap();
        assert_eq!(
            event_summary(&ev, &payload),
            "Phase 00 preflight failed — 3 violations."
        );
    }

    #[test]
    fn every_overview_rule_id_is_produced_by_a_test() {
        // The inventory that actually runs fixtures lives in
        // plan_check::every_overview_rule_is_emitted_by_a_fixture.
        assert!(RULE_IDS.contains(&RULE_MISSING_FILES));
        assert!(RULE_IDS.contains(&crate::services::plan_check::RULE_VERIFY_NOT_RUNNABLE));
        assert!(RULE_IDS.contains(&crate::services::plan_check::RULE_PROMPT_EXCEEDS_CONTEXT));
    }
}
