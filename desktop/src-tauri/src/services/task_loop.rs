//! Per-task kloo `--benchmark` loop (phase 03).
//!
//! A phase uses this loop iff any task directory contains `task.yml` (D5).
//! Commercial phases (prompt.md only) stay on the persistent worker path.

use crate::providers::kloo_cli::{
    build_config, parse_line, parse_result_from_stdout, KlooResult, KlooRunRequest,
};
use crate::providers::cli_runner::{self, CliProcess};
use crate::providers::ChunkType;
use tokio::sync::mpsc;
use crate::db::models::AgentPlanPhase;
use crate::services::agent_plans::{self, AgentPlanRun};
use crate::services::atomic_fs;
use crate::services::settings as settings_service;
use crate::services::task_check::{
    classify, digest_bytes, next_attempt, Attempt, CheckInputs, CheckReport, FailureClass,
    SpawnFailure, Step,
};
use crate::services::task_spec::{self, load_task_spec, TaskSpec};
use crate::services::verify_policy::{plan_task_verify, PlannedVerify, VerifyPolicyError};
use crate::services::task_state::{
    attempt_json_path, block_dependents, load_tasks, project_status_yml, reconcile,
    save_tasks, seed_from_specs, tasks_json_path, AttemptRecord, TaskRow, TaskRunFile,
};
use crate::services::workspace_git;
use crate::state::app_state::AppState;
use chrono::Utc;
use std::collections::HashMap;
use std::future::Future;
use std::path::{Path, PathBuf};
use std::pin::Pin;
use std::process::Stdio;
use std::time::Duration;

/// kloo `--benchmark` wall-clock default (3600s) plus 30s slack.
pub const DEFAULT_KLOO_TIMEOUT: Duration = Duration::from_secs(3630);
pub const VERIFY_TIMEOUT: Duration = Duration::from_secs(120);

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum PhaseLoopOutcome {
    AllDone,
    Partial {
        failed: Vec<String>,
        blocked: Vec<String>,
        first_route: Option<String>,
        first_task_id: Option<String>,
    },
    /// User Stop / plan `blocked`. Not a task failure.
    Cancelled,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum KlooPhaseFollowup {
    DispatchReview,
    NeedsAttention { task_id: String, route: String },
    /// Leave `stopped` / `blocked` as `stop_plan` already wrote it.
    LeaveAsIs,
}



pub fn followup_after_kloo_phase(outcome: &PhaseLoopOutcome) -> KlooPhaseFollowup {
    match outcome {
        PhaseLoopOutcome::AllDone => KlooPhaseFollowup::DispatchReview,
        PhaseLoopOutcome::Partial {
            first_task_id,
            first_route,
            ..
        } => KlooPhaseFollowup::NeedsAttention {
            task_id: first_task_id.clone().unwrap_or_default(),
            route: first_route.clone().unwrap_or_else(|| "planner".into()),
        },
        PhaseLoopOutcome::Cancelled => KlooPhaseFollowup::LeaveAsIs,
    }
}

/// D5: kloo-mode iff any task subdirectory contains `task.yml`.
/// Filesystem gate only — not the plan's worker provider id.
pub fn phase_is_kloo_mode(phase_path: &Path) -> bool {
    let tasks_dir = phase_path.join("tasks");
    let Ok(rd) = std::fs::read_dir(&tasks_dir) else {
        return false;
    };
    rd.flatten().any(|entry| {
        let p = entry.path();
        p.is_dir() && p.join("task.yml").is_file()
    })
}

/// Load every `prompt.md` task dir. In kloo-mode a sibling without `task.yml`
/// is a shape error naming that id (do not silently skip).
pub fn load_phase_specs(phase_path: &Path) -> Result<Vec<TaskSpec>, String> {
    let tasks_dir = phase_path.join("tasks");
    if !tasks_dir.is_dir() {
        return Ok(Vec::new());
    }
    let kloo = phase_is_kloo_mode(phase_path);
    let mut dirs: Vec<_> = std::fs::read_dir(&tasks_dir)
        .map_err(|e| format!("read {}: {e}", tasks_dir.display()))?
        .filter_map(|e| e.ok())
        .filter(|e| e.path().is_dir())
        .collect();
    dirs.sort_by_key(|e| e.file_name());

    let mut specs = Vec::new();
    let mut missing_yml = Vec::new();
    let mut missing_prompt = Vec::new();
    for entry in dirs {
        let path = entry.path();
        let id = entry.file_name().to_string_lossy().to_string();
        let has_yml = path.join("task.yml").is_file();
        let has_prompt = path.join("prompt.md").is_file();
        if has_yml && !has_prompt {
            missing_prompt.push(id);
            continue;
        }
        if !has_prompt {
            continue;
        }
        if has_yml {
            specs.push(load_task_spec(&path)?);
        } else if kloo {
            missing_yml.push(id);
        }
    }
    if !missing_prompt.is_empty() {
        return Err(format!(
            "shape: kloo-mode phase missing prompt.md for {}",
            missing_prompt.join(", ")
        ));
    }
    if !missing_yml.is_empty() {
        return Err(format!(
            "shape: kloo-mode phase missing task.yml for {}",
            missing_yml.join(", ")
        ));
    }
    Ok(specs)
}

#[derive(Debug, Clone)]
pub struct SpawnOutcome {
    pub stdout: String,
    pub exit_code: i32,
}

pub trait KlooSpawner: Send {
    fn spawn<'a>(
        &'a mut self,
        req: &'a KlooRunRequest,
    ) -> Pin<Box<dyn Future<Output = Result<SpawnOutcome, String>> + Send + 'a>>;
}

pub trait LoopHost: Send {
    fn spawn_kloo<'a>(
        &'a mut self,
        req: &'a KlooRunRequest,
    ) -> Pin<Box<dyn Future<Output = Result<SpawnOutcome, String>> + Send + 'a>>;
    fn git_changed_files(&self, workspace: &Path) -> Result<Vec<String>, String>;
    fn hash_file(&self, workspace: &Path, rel: &str) -> Option<String>;
    fn read_file(&self, workspace: &Path, rel: &str) -> Option<String>;
    fn file_exists(&self, workspace: &Path, rel: &str) -> bool;
    fn commit_task(
        &mut self,
        workspace: &Path,
        phase_id: &str,
        id: &str,
        files: &[String],
    ) -> Result<Option<String>, String>;
    fn index_task_commits(
        &self,
        workspace: &Path,
        phase_id: &str,
    ) -> Result<HashMap<(String, String), String>, String>;
    fn run_task_verify(
        &self,
        workspace: &Path,
        cmd: &str,
        cwd: Option<&str>,
    ) -> Result<i32, String>;
    fn plan_status(&self) -> String;
    fn emit(
        &mut self,
        event_type: &str,
        task_id: &str,
        tier: Option<&str>,
        attempt: Option<u32>,
        failure_code: Option<&str>,
        commit_sha: Option<&str>,
        route: Option<&str>,
        checks: Option<serde_json::Value>,
    );
}

pub struct KlooPhaseCtx {
    pub plan_id: String,
    pub phase_id: String,
    pub phase_dir: PathBuf,
    pub workspace: PathBuf,
    pub runs_dir: PathBuf,
    /// Optional absolute path to the kloo binary. Tests pass a per-test
    /// wrapper so they never mutate process-global `PATH` / `J1_FAKE_KLOO_*`.
    /// Production leaves this `None` and resolves `kloo` from PATH.
    pub kloo_cli: Option<String>,
}

pub fn register_kloo_pid(state: &AppState, plan_id: &str, pid: u32) {
    if let Ok(mut map) = state.kloo_pids.lock() {
        map.insert(plan_id.to_string(), pid);
    }
}

pub fn unregister_kloo_pid(state: &AppState, plan_id: &str) {
    if let Ok(mut map) = state.kloo_pids.lock() {
        map.remove(plan_id);
    }
}

/// Bounded TERM → wait → KILL of a process group (and the pid itself if
/// the group signal fails). Shared by Stop, spawn timeout, and verify watchdog.
pub fn kill_process_group(pid: u32) -> bool {
    if !pid_is_alive(pid) {
        return false;
    }
    signal_pid_or_group(pid, "-TERM");
    if wait_until_dead(pid, Duration::from_millis(400)) {
        return true;
    }
    signal_pid_or_group(pid, "-KILL");
    wait_until_dead(pid, Duration::from_millis(200))
}

fn signal_pid_or_group(pid: u32, sig: &str) {
    let group = format!("-{pid}");
    let sent = std::process::Command::new("kill")
        .args([sig, &group])
        .status()
        .map(|s| s.success())
        .unwrap_or(false);
    if !sent {
        let _ = std::process::Command::new("kill")
            .args([sig, &pid.to_string()])
            .status();
    }
}

fn wait_until_dead(pid: u32, budget: Duration) -> bool {
    let start = std::time::Instant::now();
    while start.elapsed() < budget {
        if !pid_is_alive(pid) {
            return true;
        }
        std::thread::sleep(Duration::from_millis(20));
    }
    !pid_is_alive(pid)
}

/// Kill a registered kloo process group (no tmux pane). The map entry is
/// kept until the pid is confirmed dead so a second Stop can still signal.
pub fn kill_registered_kloo_child(state: &AppState, plan_id: &str) -> bool {
    let pid = {
        let Ok(map) = state.kloo_pids.lock() else {
            return false;
        };
        map.get(plan_id).copied()
    };
    let Some(pid) = pid else {
        return false;
    };
    let dead = kill_process_group(pid) || !pid_is_alive(pid);
    if dead {
        unregister_kloo_pid(state, plan_id);
    }
    dead
}

fn pid_is_alive(pid: u32) -> bool {
    let stat = format!("/proc/{pid}/stat");
    match std::fs::read_to_string(&stat) {
        Ok(body) => {
            // comm is in parens; the next token is state. Zombies cannot edit
            // the workspace — treat them as dead so Stop can drop the map entry.
            let state = body
                .rsplit(')')
                .next()
                .and_then(|rest| rest.trim().chars().next());
            !matches!(state, Some('Z') | None)
        }
        Err(_) => std::process::Command::new("kill")
            .args(["-0", &pid.to_string()])
            .status()
            .map(|s| s.success())
            .unwrap_or(false),
    }
}

/// Parse last `KLOO_RESULT_JSON` and persist the attempt object. Used by the
/// live loop and the `#[cfg(test)]` spawn helper so both share one write path.
fn parse_and_record_attempt(
    stdout: &str,
    runs_dir: &Path,
    task_id: &str,
    attempt_n: u32,
) -> Result<KlooResult, String> {
    let parsed = parse_result_from_stdout(stdout)?;
    write_attempt_json(runs_dir, task_id, attempt_n, &parsed)?;
    Ok(parsed)
}

#[cfg(test)]
pub async fn spawn_kloo_task<S: KlooSpawner>(
    req: &KlooRunRequest,
    spawner: &mut S,
    runs_dir: &Path,
    task_id: &str,
    attempt_n: u32,
) -> Result<(KlooResult, i32, crate::providers::CliSpawnConfig), String> {
    let cfg = build_config(req)?;
    let outcome = spawner.spawn(req).await?;
    let parsed = parse_and_record_attempt(&outcome.stdout, runs_dir, task_id, attempt_n)?;
    Ok((parsed, outcome.exit_code, cfg))
}

fn write_attempt_json(
    runs_dir: &Path,
    task_id: &str,
    attempt_n: u32,
    result: &KlooResult,
) -> Result<(), String> {
    let path = attempt_json_path(runs_dir, task_id, attempt_n);
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent)
            .map_err(|e| format!("mkdir {}: {e}", parent.display()))?;
    }
    let bytes = serde_json::to_vec_pretty(result)
        .map_err(|e| format!("serialize attempt json: {e}"))?;
    atomic_fs::write_atomic(&path, &bytes)
}

/// After `proc.wait()`, keep reading until the channel closes. Bound the
/// drain: a grandchild that inherited stdout can hold the pipe open.
const STDOUT_DRAIN_SLACK: Duration = Duration::from_secs(5);
/// Only the last `KLOO_RESULT_JSON` line is parsed; cap coordinator RAM.
const STDOUT_TAIL_BYTES: usize = 64 * 1024;

struct StdoutTail {
    buf: String,
}

impl StdoutTail {
    fn new() -> Self {
        Self {
            buf: String::new(),
        }
    }
    fn push_line(&mut self, line: &str) {
        self.buf.push_str(line);
        self.buf.push('\n');
        if self.buf.len() > STDOUT_TAIL_BYTES {
            let excess = self.buf.len() - STDOUT_TAIL_BYTES;
            let drop_to = self.buf[excess..]
                .find('\n')
                .map(|i| excess + i + 1)
                .unwrap_or(excess);
            self.buf.drain(..drop_to);
        }
    }
}

async fn wait_and_collect_stdout(
    proc: &mut CliProcess,
    rx: &mut mpsc::Receiver<crate::providers::StreamChunk>,
) -> Result<(std::process::ExitStatus, String), String> {
    let mut stdout = StdoutTail::new();
    let wait_fut = proc.wait();
    tokio::pin!(wait_fut);
    let status = loop {
        tokio::select! {
            chunk = rx.recv() => {
                match chunk {
                    Some(c) if c.chunk_type == ChunkType::Text => {
                        stdout.push_line(&c.content);
                    }
                    Some(_) => {}
                    None => break wait_fut.await?,
                }
            }
            status = &mut wait_fut => break status?,
        }
    };
    let drain = async {
        while let Some(c) = rx.recv().await {
            if c.chunk_type == ChunkType::Text {
                stdout.push_line(&c.content);
            }
        }
    };
    let _ = tokio::time::timeout(STDOUT_DRAIN_SLACK, drain).await;
    Ok((status, stdout.buf))
}

/// Live piped spawn via `cli_runner::spawn_cli_process_group`. Only
/// `ChunkType::Text` (stdout) is concatenated for `KLOO_RESULT_JSON` parse.
pub async fn spawn_kloo_live(
    state: &AppState,
    plan_id: &str,
    req: &KlooRunRequest,
    timeout: Duration,
) -> Result<SpawnOutcome, String> {
    let cfg = build_config(req)?;
    let session_id = format!("kloo-{plan_id}");
    let (mut proc, mut rx) =
        cli_runner::spawn_cli_process_group(cfg, session_id, parse_line).await?;
    let pid = proc.id();
    if let Some(p) = pid {
        register_kloo_pid(state, plan_id, p);
    }
    let timed = tokio::time::timeout(timeout, wait_and_collect_stdout(&mut proc, &mut rx)).await;
    match timed {
        Ok(Ok((status, stdout))) => {
            unregister_kloo_pid(state, plan_id);
            Ok(SpawnOutcome {
                stdout,
                exit_code: status.code().unwrap_or(1),
            })
        }
        Ok(Err(e)) => {
            unregister_kloo_pid(state, plan_id);
            Err(e)
        }
        Err(_) => {
            if let Some(p) = pid {
                tokio::task::block_in_place(|| kill_process_group(p));
            }
            let _ = proc.kill().await;
            unregister_kloo_pid(state, plan_id);
            Err("kloo timed out".into())
        }
    }
}

pub async fn run_kloo_phase(
    state: &AppState,
    run: &AgentPlanRun,
    phase: &AgentPlanPhase,
) -> Result<PhaseLoopOutcome, String> {
    run_kloo_phase_ex(state, run, phase, None).await
}

/// Same as [`run_kloo_phase`] but pins the kloo binary (`cli_path`) so
/// integration tests can spawn a per-test wrapper without touching `PATH`.
pub async fn run_kloo_phase_ex(
    state: &AppState,
    run: &AgentPlanRun,
    phase: &AgentPlanPhase,
    kloo_cli: Option<String>,
) -> Result<PhaseLoopOutcome, String> {
    let phase_dir = Path::new(&run.plan.plan_path)
        .join("phases")
        .join(&phase.phase_id);
    let initiatives = settings_service::resolve_initiatives_dir(state);
    let runs_dir = settings_service::initiative_runs_path(
        &initiatives,
        &run.plan.initiative_id,
        &run.plan.id,
        &phase.phase_id,
    );
    let ctx = KlooPhaseCtx {
        plan_id: run.plan.id.clone(),
        phase_id: phase.phase_id.clone(),
        phase_dir,
        workspace: PathBuf::from(&run.plan.workspace_path),
        runs_dir,
        kloo_cli,
    };
    let mut host = RealHost {
        state: state.clone(),
        plan_id: run.plan.id.clone(),
        phase_id: phase.phase_id.clone(),
        timeout: DEFAULT_KLOO_TIMEOUT,
    };
    run_kloo_phase_with(&ctx, &mut host).await
}

pub async fn run_kloo_phase_with<H: LoopHost>(
    ctx: &KlooPhaseCtx,
    host: &mut H,
) -> Result<PhaseLoopOutcome, String> {
    if is_cancelled(&host.plan_status()) {
        return Ok(PhaseLoopOutcome::Cancelled);
    }
    let specs = load_phase_specs(&ctx.phase_dir)?;
    if specs.is_empty() {
        return Ok(PhaseLoopOutcome::AllDone);
    }
    let spec_by_id: HashMap<String, TaskSpec> =
        specs.iter().cloned().map(|s| (s.id.clone(), s)).collect();

    let tasks_path = tasks_json_path(&ctx.runs_dir);
    let mut file = load_tasks(&tasks_path, &ctx.plan_id, &ctx.phase_id)?;
    if file.tasks.is_empty() {
        file = seed_from_specs(&ctx.plan_id, &ctx.phase_id, &specs);
    }
    ensure_spec_rows(&mut file, &specs);
    let residue_ids = running_or_attempted_ids(&file);
    let commits = host.index_task_commits(&ctx.workspace, &ctx.phase_id)?;
    reconcile(&mut file, &ctx.phase_id, &commits);
    save_and_project(ctx, &mut file)?;

    loop {
        if is_cancelled(&host.plan_status()) {
            return Ok(PhaseLoopOutcome::Cancelled);
        }

        let order = task_spec::topo_sort(&specs).map_err(|e| e.to_string())?;
        let next_id = pick_next(&file, &order, &spec_by_id)?;
        let Some(task_id) = next_id else {
            return Ok(terminal_outcome(&file));
        };
        if is_cancelled(&host.plan_status()) {
            return Ok(PhaseLoopOutcome::Cancelled);
        }

        let spec = spec_by_id
            .get(&task_id)
            .ok_or_else(|| format!("missing spec {task_id}"))?
            .clone();
        if let Some(reason) = missing_required(host, &ctx.workspace, &spec, &file, &spec_by_id) {
            fail_task(
                ctx,
                host,
                &mut file,
                &task_id,
                "planner",
                Some("missing_file"),
                Some(&reason),
            )?;
            continue;
        }

        let residue_eligible = residue_ids.contains(&task_id);
        {
            let row = row_mut(&mut file, &task_id)?;
            row.status = "running".into();
        }
        save_and_project(ctx, &mut file)?;

        let prompt = match load_prompt(&ctx.phase_dir, &task_id) {
            Ok(p) => p,
            Err(reason) => {
                fail_task(
                    ctx,
                    host,
                    &mut file,
                    &task_id,
                    "planner",
                    Some("missing_prompt"),
                    Some(&reason),
                )?;
                continue;
            }
        };

        if residue_eligible && residue_commits(ctx, host, &mut file, &spec).await? {
            continue;
        }

        let mut last_class: Option<FailureClass> = None;
        loop {
            if is_cancelled(&host.plan_status()) {
                return Ok(PhaseLoopOutcome::Cancelled);
            }
            let history = attempts_to_history(row_ref(&file, &task_id)?.attempts.as_slice());
            let step = next_attempt(&history, last_class);
            let (model, label) = match step {
                Step::Run { model, label, .. } | Step::RetryInfra { model, label } => {
                    (model, label)
                }
                Step::StopShape => {
                    fail_task(ctx, host, &mut file, &task_id, "planner", None, None)?;
                    break;
                }
                Step::Exhausted => {
                    let route = if last_class == Some(FailureClass::Infra) {
                        "infra"
                    } else {
                        "planner"
                    };
                    fail_task(ctx, host, &mut file, &task_id, route, None, None)?;
                    break;
                }
            };

            let attempt_n = row_ref(&file, &task_id)?.attempts.len() as u32 + 1;
            let started = Utc::now().to_rfc3339();
            let req = KlooRunRequest {
                workspace: ctx.workspace.to_string_lossy().into_owned(),
                prompt: prompt.clone(),
                files: spec.files.clone(),
                verify: spec.verify.clone(),
                cwd: spec.cwd.clone(),
                model: model.clone(),
                ctx: spec.ctx,
                profile: None,
                provider: None,
                cli_path: ctx.kloo_cli.clone(),
                status_file: None,
            };

            let pre_spawn_dirty = host.git_changed_files(&ctx.workspace)?;
            let pre_hashes = hash_files(host, &ctx.workspace, &spec.files);

            let spawn_res = host.spawn_kloo(&req).await;
            if is_cancelled(&host.plan_status()) {
                return Ok(PhaseLoopOutcome::Cancelled);
            }

            let (result, exit_code, spawn_fail) = match spawn_res {
                Ok(outcome) => {
                    match parse_and_record_attempt(
                        &outcome.stdout,
                        &ctx.runs_dir,
                        &task_id,
                        attempt_n,
                    ) {
                        Ok(parsed) => (parsed, outcome.exit_code, None),
                        Err(e) => (
                            KlooResult::default(),
                            outcome.exit_code,
                            Some(SpawnFailure::Other(e)),
                        ),
                    }
                }
                Err(e) => {
                    let fail = if is_missing_binary(&e) {
                        SpawnFailure::MissingBinary
                    } else if VerifyPolicyError::message_is_policy(&e) {
                        SpawnFailure::Policy(e)
                    } else {
                        SpawnFailure::Other(e)
                    };
                    (KlooResult::default(), 1, Some(fail))
                }
            };

            let post_hashes = hash_files(host, &ctx.workspace, &spec.files);
            let post_contents = read_files(host, &ctx.workspace, &spec.files);
            let inputs = CheckInputs {
                pre_spawn_dirty,
                pre_hashes,
                post_hashes,
                post_contents,
            };
            let report = crate::services::task_check::run_checks(&spec, &result, exit_code, &inputs);
            // A missing/garbled KLOO_RESULT_JSON is Infra (retry same tier),
            // not Model — do not pass the defaulted report into classify.
            // Passing attempts must not be labelled "infra" (classify default).
            let class = if report.passed && spawn_fail.is_none() {
                None
            } else if spawn_fail.is_some() {
                Some(classify(None, None, spawn_fail.as_ref(), Some(exit_code), None))
            } else {
                Some(classify(
                    Some(&report),
                    Some(&result),
                    None,
                    Some(exit_code),
                    None,
                ))
            };

            {
                let row = row_mut(&mut file, &task_id)?;
                row.attempts.push(AttemptRecord {
                    tier: label.clone(),
                    model: model.clone(),
                    attempt: attempt_n,
                    started_at: Some(started),
                    ended_at: Some(Utc::now().to_rfc3339()),
                    failure_code: result.failure_code.clone(),
                    exit_code: Some(exit_code),
                    class: class.map(class_str).map(str::to_string),
                    checks: Some(checks_json(&report)),
                });
            }
            save_and_project(ctx, &mut file)?;

            if report.passed && spawn_fail.is_none() {
                let Some(sha) = host.commit_task(
                    &ctx.workspace,
                    &ctx.phase_id,
                    &task_id,
                    &spec.files,
                )?
                else {
                    // Record no-sha as a real infra attempt so the ladder
                    // retry-once then exhausts (not RetryInfra forever).
                    {
                        let row = row_mut(&mut file, &task_id)?;
                        if let Some(last) = row.attempts.last_mut() {
                            last.class = Some("infra".into());
                            last.failure_code = Some("no_commit".into());
                        }
                    }
                    save_and_project(ctx, &mut file)?;
                    last_class = Some(FailureClass::Infra);
                    let hist =
                        attempts_to_history(row_ref(&file, &task_id)?.attempts.as_slice());
                    match next_attempt(&hist, Some(FailureClass::Infra)) {
                        Step::RetryInfra { .. } => continue,
                        _ => {
                            fail_task(
                                ctx,
                                host,
                                &mut file,
                                &task_id,
                                "infra",
                                Some("no_commit"),
                                Some("commit_task returned no sha"),
                            )?;
                            break;
                        }
                    }
                };
                {
                    let row = row_mut(&mut file, &task_id)?;
                    row.status = "done".into();
                    row.succeeded_tier = Some(label.clone());
                    row.commit_sha = Some(sha.clone());
                    row.route = None;
                }
                save_and_project(ctx, &mut file)?;
                host.emit(
                    "agent_phase_task_done",
                    &task_id,
                    Some(&label),
                    Some(attempt_n),
                    None,
                    Some(&sha),
                    None,
                    Some(checks_json(&report)),
                );
                break;
            }

            last_class = class;
            match class {
                Some(FailureClass::Shape) => {
                    fail_task(
                        ctx,
                        host,
                        &mut file,
                        &task_id,
                        "planner",
                        result.failure_code.as_deref(),
                        None,
                    )?;
                    break;
                }
                Some(FailureClass::Model) => {
                    let hist = attempts_to_history(row_ref(&file, &task_id)?.attempts.as_slice());
                    match next_attempt(&hist, Some(FailureClass::Model)) {
                        Step::Run { label: next_label, .. } => {
                            if next_label != label {
                                host.emit(
                                    "agent_phase_task_escalated",
                                    &task_id,
                                    Some(&next_label),
                                    Some(attempt_n),
                                    result.failure_code.as_deref(),
                                    None,
                                    None,
                                    Some(checks_json(&report)),
                                );
                            }
                        }
                        Step::RetryInfra { .. } => {}
                        Step::Exhausted | Step::StopShape => {
                            fail_task(
                                ctx,
                                host,
                                &mut file,
                                &task_id,
                                "planner",
                                result.failure_code.as_deref(),
                                None,
                            )?;
                            break;
                        }
                    }
                }
                Some(FailureClass::Infra) | None => {
                    let hist = attempts_to_history(row_ref(&file, &task_id)?.attempts.as_slice());
                    match next_attempt(&hist, Some(FailureClass::Infra)) {
                        Step::RetryInfra { .. } => {}
                        _ => {
                            fail_task(
                                ctx,
                                host,
                                &mut file,
                                &task_id,
                                "infra",
                                result.failure_code.as_deref(),
                                None,
                            )?;
                            break;
                        }
                    }
                }
            }
        }
    }
}

fn is_cancelled(status: &str) -> bool {
    matches!(status, "stopped" | "blocked")
}

fn is_missing_binary(err: &str) -> bool {
    let lower = err.to_ascii_lowercase();
    lower.contains("no such file") || lower.contains("os error 2") || lower.contains("enoent")
}

fn class_str(c: FailureClass) -> &'static str {
    match c {
        FailureClass::Shape => "shape",
        FailureClass::Model => "model",
        FailureClass::Infra => "infra",
    }
}

fn checks_json(report: &CheckReport) -> serde_json::Value {
    serde_json::json!({
        "passed": report.passed,
        "failed": report.failed.iter().map(|f| {
            serde_json::json!({ "check": f.check, "reason": f.reason })
        }).collect::<Vec<_>>(),
    })
}

fn load_prompt(phase_dir: &Path, task_id: &str) -> Result<String, String> {
    let p = phase_dir.join("tasks").join(task_id).join("prompt.md");
    let text = std::fs::read_to_string(&p).map_err(|e| {
        format!("shape: missing prompt.md for {task_id}: {e}")
    })?;
    if text.trim().is_empty() {
        return Err(format!("shape: empty prompt.md for {task_id}"));
    }
    Ok(text)
}

fn save_and_project(ctx: &KlooPhaseCtx, file: &mut TaskRunFile) -> Result<(), String> {
    save_tasks(&tasks_json_path(&ctx.runs_dir), file)?;
    for row in &file.tasks {
        let dir = ctx.phase_dir.join("tasks").join(&row.id);
        if dir.is_dir() {
            project_status_yml(&dir, row)?;
        }
    }
    Ok(())
}

fn hash_files<H: LoopHost>(
    host: &H,
    workspace: &Path,
    files: &[String],
) -> HashMap<String, Option<String>> {
    files
        .iter()
        .map(|f| (f.clone(), host.hash_file(workspace, f)))
        .collect()
}

fn read_files<H: LoopHost>(
    host: &H,
    workspace: &Path,
    files: &[String],
) -> HashMap<String, String> {
    files
        .iter()
        .filter_map(|f| host.read_file(workspace, f).map(|c| (f.clone(), c)))
        .collect()
}

fn row_mut<'a>(file: &'a mut TaskRunFile, id: &str) -> Result<&'a mut TaskRow, String> {
    file.tasks
        .iter_mut()
        .find(|t| t.id == id)
        .ok_or_else(|| format!("unknown task {id}"))
}

fn row_ref<'a>(file: &'a TaskRunFile, id: &str) -> Result<&'a TaskRow, String> {
    file.tasks
        .iter()
        .find(|t| t.id == id)
        .ok_or_else(|| format!("unknown task {id}"))
}

fn pick_next(
    file: &TaskRunFile,
    order: &[String],
    specs: &HashMap<String, TaskSpec>,
) -> Result<Option<String>, String> {
    for id in order {
        let Some(row) = file.tasks.iter().find(|t| t.id == *id) else {
            return Err(format!("tasks.json missing row for spec {id}"));
        };
        if matches!(row.status.as_str(), "done" | "blocked" | "failed") {
            continue;
        }
        let spec = specs
            .get(id)
            .ok_or_else(|| format!("no spec for tasks.json row {id}"))?;
        let deps_ok = spec.depends_on.iter().all(|dep| {
            file.tasks
                .iter()
                .find(|t| t.id == *dep)
                .map(|t| t.status == "done")
                .unwrap_or(false)
        });
        if deps_ok {
            return Ok(Some(id.clone()));
        }
    }
    Ok(None)
}

fn ensure_spec_rows(file: &mut TaskRunFile, specs: &[TaskSpec]) {
    let existing: std::collections::HashSet<String> =
        file.tasks.iter().map(|t| t.id.clone()).collect();
    for spec in specs {
        if existing.contains(&spec.id) {
            continue;
        }
        file.tasks.push(TaskRow {
            id: spec.id.clone(),
            status: "pending".into(),
            depends_on: spec.depends_on.clone(),
            attempts: Vec::new(),
            succeeded_tier: None,
            commit_sha: None,
            blocked_by: None,
            route: None,
        });
    }
}

fn running_or_attempted_ids(file: &TaskRunFile) -> std::collections::HashSet<String> {
    file.tasks
        .iter()
        .filter(|t| t.status == "running" || !t.attempts.is_empty())
        .map(|t| t.id.clone())
        .collect()
}

fn missing_required<H: LoopHost>(
    host: &H,
    workspace: &Path,
    spec: &TaskSpec,
    file: &TaskRunFile,
    specs: &HashMap<String, TaskSpec>,
) -> Option<String> {
    let new_set: std::collections::HashSet<&str> = spec
        .new
        .as_ref()
        .map(|v| v.iter().map(String::as_str).collect())
        .unwrap_or_default();
    for path in &spec.files {
        if new_set.contains(path.as_str()) {
            continue;
        }
        if host.file_exists(workspace, path) {
            continue;
        }
        if ancestor_created(file, spec, path, specs) {
            continue;
        }
        return Some(format!("shape: required file '{path}' missing"));
    }
    None
}

fn ancestor_created(
    file: &TaskRunFile,
    spec: &TaskSpec,
    path: &str,
    specs: &HashMap<String, TaskSpec>,
) -> bool {
    let mut seen = std::collections::HashSet::new();
    let mut stack = spec.depends_on.clone();
    while let Some(dep) = stack.pop() {
        if !seen.insert(dep.clone()) {
            continue;
        }
        let Some(row) = file.tasks.iter().find(|t| t.id == dep) else {
            continue;
        };
        // Not-yet-terminal ancestors still "own" the path — B must not
        // shape-fail before A has had a chance to create it.
        if matches!(row.status.as_str(), "failed" | "blocked") {
            stack.extend(row.depends_on.iter().cloned());
            continue;
        }
        if let Some(ancestor) = specs.get(&dep) {
            let in_files = ancestor.files.iter().any(|f| f == path);
            let in_new = ancestor
                .new
                .as_ref()
                .map(|n| n.iter().any(|f| f == path))
                .unwrap_or(false);
            if in_files || in_new {
                return true;
            }
        }
        stack.extend(row.depends_on.iter().cloned());
    }
    false
}

async fn residue_commits<H: LoopHost>(
    ctx: &KlooPhaseCtx,
    host: &mut H,
    file: &mut TaskRunFile,
    spec: &TaskSpec,
) -> Result<bool, String> {
    let exit = match host.run_task_verify(&ctx.workspace, &spec.verify, spec.cwd.as_deref()) {
        Ok(code) => code,
        Err(e) if VerifyPolicyError::message_is_policy(&e) => {
            fail_task(
                ctx,
                host,
                file,
                &spec.id,
                "planner",
                Some("verify_not_allowlisted"),
                Some(&e),
            )?;
            return Ok(true);
        }
        Err(e) => return Err(e),
    };
    if exit == 127 {
        fail_task(
            ctx,
            host,
            file,
            &spec.id,
            "planner",
            Some("verify_127"),
            Some("verify command not found (exit 127)"),
        )?;
        return Ok(true);
    }
    if exit != 0 {
        return Ok(false);
    }
    let contents = read_files(host, &ctx.workspace, &spec.files);
    let needles_ok = spec.must_contain.iter().all(|n| {
        spec.files
            .iter()
            .any(|f| contents.get(f).map(|c| c.contains(n)).unwrap_or(false))
    });
    if !needles_ok {
        return Ok(false);
    }
    // Clean tree → Ok(None). Do not mark done without a sha (reconcile
    // would flip it back to pending). Fall through to spawn.
    let Some(sha) = host.commit_task(&ctx.workspace, &ctx.phase_id, &spec.id, &spec.files)? else {
        return Ok(false);
    };
    {
        let row = row_mut(file, &spec.id)?;
        row.status = "done".into();
        row.succeeded_tier = row
            .attempts
            .last()
            .map(|a| a.tier.clone())
            .or_else(|| row.succeeded_tier.clone());
        row.commit_sha = Some(sha.clone());
        row.route = None;
    }
    save_and_project(ctx, file)?;
    host.emit(
        "agent_phase_task_done",
        &spec.id,
        row_ref(file, &spec.id)?.succeeded_tier.as_deref(),
        None,
        None,
        Some(&sha),
        None,
        None,
    );
    Ok(true)
}

fn fail_task<H: LoopHost>(
    ctx: &KlooPhaseCtx,
    host: &mut H,
    file: &mut TaskRunFile,
    task_id: &str,
    route: &str,
    failure_code: Option<&str>,
    detail: Option<&str>,
) -> Result<(), String> {
    let attempt_n;
    let tier;
    {
        let row = row_mut(file, task_id)?;
        row.status = "failed".into();
        row.route = Some(route.into());
        attempt_n = row.attempts.len() as u32;
        tier = row.attempts.last().map(|a| a.tier.clone());
    }
    block_dependents(file, task_id);
    save_and_project(ctx, file)?;
    let checks = detail.map(|reason| serde_json::json!({ "reason": reason }));
    host.emit(
        "agent_phase_task_failed",
        task_id,
        tier.as_deref(),
        Some(attempt_n),
        failure_code,
        None,
        Some(route),
        checks,
    );
    Ok(())
}

fn terminal_outcome(file: &TaskRunFile) -> PhaseLoopOutcome {
    let failed: Vec<String> = file
        .tasks
        .iter()
        .filter(|t| t.status == "failed")
        .map(|t| t.id.clone())
        .collect();
    let blocked: Vec<String> = file
        .tasks
        .iter()
        .filter(|t| t.status == "blocked")
        .map(|t| t.id.clone())
        .collect();
    if failed.is_empty() && blocked.is_empty() {
        PhaseLoopOutcome::AllDone
    } else {
        let first = file.tasks.iter().find(|t| t.status == "failed");
        PhaseLoopOutcome::Partial {
            first_task_id: first.map(|t| t.id.clone()),
            first_route: first.and_then(|t| t.route.clone()),
            failed,
            blocked,
        }
    }
}

fn attempts_to_history(records: &[AttemptRecord]) -> Vec<Attempt> {
    let mut seen_infra: std::collections::HashSet<String> = std::collections::HashSet::new();
    records
        .iter()
        .map(|r| {
            let class = match r.class.as_deref() {
                Some("shape") => Some(FailureClass::Shape),
                Some("model") => Some(FailureClass::Model),
                Some("infra") => Some(FailureClass::Infra),
                _ => None,
            };
            let infra_retry = class == Some(FailureClass::Infra) && seen_infra.contains(&r.tier);
            if class == Some(FailureClass::Infra) {
                seen_infra.insert(r.tier.clone());
            }
            Attempt {
                label: r.tier.clone(),
                model: r.model.clone(),
                class,
                infra_retry,
            }
        })
        .collect()
}

pub fn hash_workspace_file(workspace: &Path, rel: &str) -> Option<String> {
    let bytes = std::fs::read(workspace.join(rel)).ok()?;
    Some(digest_bytes(&bytes))
}

pub fn run_task_verify(
    workspace: &Path,
    verify: &str,
    cwd: Option<&str>,
) -> Result<i32, String> {
    let plan = plan_task_verify(workspace, verify, cwd)
        .map_err(|e| format!("shape: {e}"))?;
    spawn_planned_verify(&plan)
}

fn spawn_planned_verify(plan: &PlannedVerify) -> Result<i32, String> {
    let exe = plan
        .argv
        .first()
        .ok_or_else(|| "shape: empty authorised argv".to_string())?;
    let mut cmd = std::process::Command::new(exe);
    cmd.args(&plan.argv[1..])
        .current_dir(&plan.current_dir)
        .stdout(Stdio::null())
        .stderr(Stdio::null());
    #[cfg(unix)]
    {
        use std::os::unix::process::CommandExt;
        cmd.process_group(0);
    }
    let mut child = cmd.spawn().map_err(|e| format!("spawn verify: {e}"))?;
    let pid = child.id();
    let (done_tx, done_rx) = std::sync::mpsc::channel::<()>();
    std::thread::spawn(move || {
        if done_rx.recv_timeout(VERIFY_TIMEOUT).is_err() {
            kill_process_group(pid);
        }
    });
    let status = child.wait().map_err(|e| format!("wait verify: {e}"))?;
    let _ = done_tx.send(());
    Ok(status.code().unwrap_or(1))
}

struct RealHost {
    state: AppState,
    plan_id: String,
    phase_id: String,
    timeout: Duration,
}

impl LoopHost for RealHost {
    fn spawn_kloo<'a>(
        &'a mut self,
        req: &'a KlooRunRequest,
    ) -> Pin<Box<dyn Future<Output = Result<SpawnOutcome, String>> + Send + 'a>> {
        Box::pin(async move { spawn_kloo_live(&self.state, &self.plan_id, req, self.timeout).await })
    }

    fn git_changed_files(&self, workspace: &Path) -> Result<Vec<String>, String> {
        tokio::task::block_in_place(|| {
            let entries = agent_plans::git_changed_files(&workspace.to_string_lossy(), None)?;
            Ok(entries
                .into_iter()
                .filter(|e| e.kind != "info")
                .map(|e| e.path)
                .collect())
        })
    }

    fn hash_file(&self, workspace: &Path, rel: &str) -> Option<String> {
        hash_workspace_file(workspace, rel)
    }

    fn read_file(&self, workspace: &Path, rel: &str) -> Option<String> {
        std::fs::read_to_string(workspace.join(rel)).ok()
    }

    fn file_exists(&self, workspace: &Path, rel: &str) -> bool {
        workspace.join(rel).exists()
    }

    fn commit_task(
        &mut self,
        workspace: &Path,
        phase_id: &str,
        id: &str,
        files: &[String],
    ) -> Result<Option<String>, String> {
        tokio::task::block_in_place(|| {
            let slug = workspace_git::task_slug(id);
            let paths: Vec<PathBuf> = files.iter().map(PathBuf::from).collect();
            workspace_git::commit_task(workspace, phase_id, id, &slug, &paths)
        })
    }

    fn index_task_commits(
        &self,
        workspace: &Path,
        phase_id: &str,
    ) -> Result<HashMap<(String, String), String>, String> {
        tokio::task::block_in_place(|| {
            workspace_git::index_task_commits(workspace, phase_id, None)
        })
    }

    fn run_task_verify(
        &self,
        workspace: &Path,
        cmd: &str,
        cwd: Option<&str>,
    ) -> Result<i32, String> {
        tokio::task::block_in_place(|| run_task_verify(workspace, cmd, cwd))
    }

    fn plan_status(&self) -> String {
        agent_plans::get_plan(&self.state, &self.plan_id)
            .map(|r| r.plan.status)
            .unwrap_or_default()
    }

    fn emit(
        &mut self,
        event_type: &str,
        task_id: &str,
        tier: Option<&str>,
        attempt: Option<u32>,
        failure_code: Option<&str>,
        commit_sha: Option<&str>,
        route: Option<&str>,
        checks: Option<serde_json::Value>,
    ) {
        let _ = agent_plans::emit_task_event(
            &self.state,
            &self.plan_id,
            event_type,
            task_id,
            &self.phase_id,
            tier,
            attempt,
            failure_code,
            commit_sha,
            route,
            checks,
        );
    }
}

// ── Tests ────────────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;
    use crate::providers::kloo_cli::build_config;
    use crate::services::verify_policy::{check_verify, quote_argv_for_shell};
    use std::collections::VecDeque;
    use std::sync::atomic::{AtomicU64, Ordering};
    use std::sync::{Arc, Mutex};

    static SEQ: AtomicU64 = AtomicU64::new(0);

    fn tmp(tag: &str) -> PathBuf {
        let d = std::env::temp_dir().join(format!(
            "j1-tloop-{}-{}-{}",
            tag,
            std::process::id(),
            SEQ.fetch_add(1, Ordering::Relaxed)
        ));
        std::fs::create_dir_all(&d).unwrap();
        d
    }

    fn write_task(phase: &Path, id: &str, yml: Option<&str>, prompt: &str) {
        let dir = phase.join("tasks").join(id);
        std::fs::create_dir_all(&dir).unwrap();
        std::fs::write(dir.join("prompt.md"), prompt).unwrap();
        if let Some(body) = yml {
            std::fs::write(dir.join("task.yml"), body).unwrap();
        }
    }

    fn yml(id: &str, files: &str, verify: &str, deps: &str, new: Option<&str>, needle: &str) -> String {
        let mut s = format!(
            "id: {id}\nfiles: [{files}]\nverify: \"{verify}\"\nmust_contain: [\"{needle}\"]\ndepends_on: [{deps}]\n"
        );
        if let Some(n) = new {
            s.push_str(&format!("new: [{n}]\n"));
        }
        s
    }

    struct FakeSpawner {
        outcomes: VecDeque<Result<SpawnOutcome, String>>,
        reqs: Vec<KlooRunRequest>,
    }

    impl FakeSpawner {
        fn new(outcomes: Vec<Result<SpawnOutcome, String>>) -> Self {
            Self {
                outcomes: outcomes.into(),
                reqs: vec![],
            }
        }
    }

    impl KlooSpawner for FakeSpawner {
        fn spawn<'a>(
            &'a mut self,
            req: &'a KlooRunRequest,
        ) -> Pin<Box<dyn Future<Output = Result<SpawnOutcome, String>> + Send + 'a>> {
            self.reqs.push(req.clone());
            let next = self
                .outcomes
                .pop_front()
                .unwrap_or_else(|| Err("no canned spawn".into()));
            Box::pin(async move { next })
        }
    }

    fn ok_json(path: &str) -> String {
        format!(
            "KLOO_RESULT_JSON {{\"success\":true,\"files_changed\":{{\"count\":1,\"paths\":[\"{path}\"]}},\"off_scope_edits\":0,\"verify\":{{\"command\":\"true\",\"passed\":true,\"exit_code\":0}}}}"
        )
    }

    fn fail_json(path: &str, code: &str) -> String {
        format!(
            "KLOO_RESULT_JSON {{\"success\":false,\"failure_code\":\"{code}\",\"files_changed\":{{\"count\":1,\"paths\":[\"{path}\"]}},\"off_scope_edits\":0,\"verify\":{{\"command\":\"true\",\"passed\":false,\"exit_code\":1}}}}"
        )
    }

    struct ScriptedHost {
        workspace: PathBuf,
        status: Arc<Mutex<String>>,
        spawn_scripts: VecDeque<Box<dyn FnMut(&KlooRunRequest) -> Result<SpawnOutcome, String> + Send>>,
        spawned_files: Vec<Vec<String>>,
        spawned_models: Vec<String>,
        commits: Vec<String>,
        events: Vec<String>,
        index: HashMap<(String, String), String>,
        stop_after_commits: Option<usize>,
        commit_returns_none: bool,
        verify_err: Option<String>,
    }

    impl ScriptedHost {
        fn new(workspace: PathBuf) -> Self {
            Self {
                workspace,
                status: Arc::new(Mutex::new("phase_worker_running".into())),
                spawn_scripts: VecDeque::new(),
                spawned_files: vec![],
                spawned_models: vec![],
                commits: vec![],
                events: vec![],
                index: HashMap::new(),
                stop_after_commits: None,
                commit_returns_none: false,
                verify_err: None,
            }
        }

        fn push_pass(&mut self, rel: &str, body: &str) {
            let ws = self.workspace.clone();
            let rel = rel.to_string();
            let body = body.to_string();
            self.spawn_scripts.push_back(Box::new(move |_req| {
                if let Some(parent) = Path::new(&rel).parent() {
                    let _ = std::fs::create_dir_all(ws.join(parent));
                }
                std::fs::write(ws.join(&rel), &body).unwrap();
                Ok(SpawnOutcome {
                    stdout: ok_json(&rel),
                    exit_code: 0,
                })
            }));
        }

        fn push_stdout(&mut self, stdout: &str, exit: i32) {
            let stdout = stdout.to_string();
            self.spawn_scripts.push_back(Box::new(move |_req| {
                Ok(SpawnOutcome {
                    stdout: stdout.clone(),
                    exit_code: exit,
                })
            }));
        }

        fn push_fail(&mut self, rel: &str, body: &str, code: &str) {
            let ws = self.workspace.clone();
            let rel = rel.to_string();
            let body = body.to_string();
            let code = code.to_string();
            self.spawn_scripts.push_back(Box::new(move |_req| {
                if let Some(parent) = Path::new(&rel).parent() {
                    let _ = std::fs::create_dir_all(ws.join(parent));
                }
                std::fs::write(ws.join(&rel), &body).unwrap();
                Ok(SpawnOutcome {
                    stdout: fail_json(&rel, &code),
                    exit_code: 10,
                })
            }));
        }
    }

    impl LoopHost for ScriptedHost {
        fn spawn_kloo<'a>(
            &'a mut self,
            req: &'a KlooRunRequest,
        ) -> Pin<Box<dyn Future<Output = Result<SpawnOutcome, String>> + Send + 'a>> {
            self.spawned_files.push(req.files.clone());
            self.spawned_models.push(req.model.clone());
            let next = self
                .spawn_scripts
                .pop_front()
                .map(|mut f| f(req))
                .unwrap_or_else(|| Err("no canned spawn".into()));
            Box::pin(async move { next })
        }

        fn git_changed_files(&self, _workspace: &Path) -> Result<Vec<String>, String> {
            Ok(vec![])
        }

        fn hash_file(&self, workspace: &Path, rel: &str) -> Option<String> {
            hash_workspace_file(workspace, rel)
        }

        fn read_file(&self, workspace: &Path, rel: &str) -> Option<String> {
            std::fs::read_to_string(workspace.join(rel)).ok()
        }

        fn file_exists(&self, workspace: &Path, rel: &str) -> bool {
            workspace.join(rel).exists()
        }

        fn commit_task(
            &mut self,
            _workspace: &Path,
            phase_id: &str,
            id: &str,
            _files: &[String],
        ) -> Result<Option<String>, String> {
            self.commits.push(id.to_string());
            if self.commit_returns_none {
                return Ok(None);
            }
            let sha = format!("sha-{id}");
            self.index
                .insert((phase_id.to_string(), id.to_string()), sha.clone());
            if let Some(n) = self.stop_after_commits {
                if self.commits.len() >= n {
                    *self.status.lock().unwrap() = "stopped".into();
                }
            }
            Ok(Some(sha))
        }

        fn index_task_commits(
            &self,
            _workspace: &Path,
            _phase_id: &str,
        ) -> Result<HashMap<(String, String), String>, String> {
            Ok(self.index.clone())
        }

        fn run_task_verify(
            &self,
            _workspace: &Path,
            _cmd: &str,
            _cwd: Option<&str>,
        ) -> Result<i32, String> {
            if let Some(e) = &self.verify_err {
                return Err(e.clone());
            }
            Ok(0)
        }

        fn plan_status(&self) -> String {
            self.status.lock().unwrap().clone()
        }

        fn emit(
            &mut self,
            event_type: &str,
            _task_id: &str,
            _tier: Option<&str>,
            _attempt: Option<u32>,
            _failure_code: Option<&str>,
            _commit_sha: Option<&str>,
            _route: Option<&str>,
            _checks: Option<serde_json::Value>,
        ) {
            self.events.push(event_type.to_string());
        }
    }

    fn ctx_for(root: &Path, phase_id: &str) -> (KlooPhaseCtx, PathBuf) {
        let phase_dir = root.join("plan").join("phases").join(phase_id);
        std::fs::create_dir_all(phase_dir.join("tasks")).unwrap();
        let workspace = root.join("ws");
        std::fs::create_dir_all(&workspace).unwrap();
        let runs_dir = root.join("runs").join("plan").join(phase_id);
        std::fs::create_dir_all(&runs_dir).unwrap();
        (
            KlooPhaseCtx {
                plan_id: "plan-1".into(),
                phase_id: phase_id.into(),
                phase_dir,
                workspace,
                runs_dir,
                kloo_cli: None,
            },
            root.to_path_buf(),
        )
    }

    #[test]
    fn phase_is_kloo_mode_false_without_task_yml() {
        let root = tmp("mode0");
        let phase = root.join("phases/00-x");
        write_task(&phase, "01-a", None, "# A\n");
        assert!(!phase_is_kloo_mode(&phase));
        let _ = std::fs::remove_dir_all(&root);
    }

    #[test]
    fn phase_is_kloo_mode_true_with_one_task_yml() {
        let root = tmp("mode1");
        let phase = root.join("phases/00-x");
        write_task(
            &phase,
            "01-a",
            Some(&yml("01-a", "a.rs", "cargo test spec -- --exact", "", None, "fn")),
            "# A\n",
        );
        assert!(phase_is_kloo_mode(&phase));
        let _ = std::fs::remove_dir_all(&root);
    }

    #[test]
    fn phase_is_kloo_mode_missing_sibling_is_shape_error() {
        let root = tmp("mode-miss");
        let phase = root.join("phases/00-x");
        write_task(
            &phase,
            "01-a",
            Some(&yml("01-a", "a.rs", "cargo test spec -- --exact", "", None, "fn")),
            "# A\n",
        );
        write_task(&phase, "02-b", None, "# B\n");
        let err = load_phase_specs(&phase).unwrap_err();
        assert!(err.contains("02-b"), "{err}");
        assert!(err.contains("shape"), "{err}");
        let _ = std::fs::remove_dir_all(&root);
    }

    #[test]
    fn phase_without_task_yml_is_commercial() {
        let root = tmp("commercial");
        let phase = root.join("phases/01-x");
        write_task(&phase, "01-y", None, "# Task Y\n");
        assert!(!phase_is_kloo_mode(&phase));
        let specs = load_phase_specs(&phase).unwrap();
        assert!(specs.is_empty());
        let _ = std::fs::remove_dir_all(&root);
    }

    #[test]
    fn detection_does_not_look_at_worker_provider() {
        let src = std::fs::read_to_string(file!()).unwrap();
        let impl_src = src.split("#[cfg(test)]").next().unwrap();
        assert!(
            !impl_src.contains("worker_provider"),
            "phase_is_kloo_mode must be a filesystem gate"
        );
    }

    #[tokio::test]
    async fn spawn_kloo_uses_benchmark_no_status_file() {
        let root = tmp("spawn1");
        let runs = root.join("runs");
        let req = KlooRunRequest {
            workspace: root.to_string_lossy().into_owned(),
            prompt: "do it".into(),
            files: vec!["src/add.rs".into()],
            verify: "cargo test spec -- --exact".into(),
            cwd: None,
            model: "qwen/qwen3-coder".into(),
            ctx: None,
            profile: None,
            provider: None,
            cli_path: None,
            status_file: None,
        };
        let cfg = build_config(&req).unwrap();
        assert!(cfg.args.iter().any(|a| a == "--benchmark"));
        assert!(!cfg.args.iter().any(|a| a == "--status-file"));
        assert_eq!(cfg.working_directory, req.workspace);

        let mut spawner = FakeSpawner::new(vec![Ok(SpawnOutcome {
            stdout: ok_json("src/add.rs"),
            exit_code: 0,
        })]);
        let (result, exit, _) = spawn_kloo_task(&req, &mut spawner, &runs, "01-add", 1)
            .await
            .unwrap();
        assert!(result.success);
        assert_eq!(exit, 0);
        assert!(runs.join("01-add").join("1.json").is_file());
        let _ = std::fs::remove_dir_all(&root);
    }

    #[tokio::test]
    async fn spawn_kloo_disposes_when_parse_fails() {
        let root = tmp("spawn-fail");
        let runs = root.join("runs");
        let req = KlooRunRequest {
            workspace: root.to_string_lossy().into_owned(),
            prompt: "x".into(),
            files: vec!["a.rs".into()],
            verify: "cargo test spec -- --exact".into(),
            cwd: None,
            model: "qwen/qwen3-coder".into(),
            ctx: None,
            profile: None,
            provider: None,
            cli_path: None,
            status_file: None,
        };
        let mut spawner = FakeSpawner::new(vec![Ok(SpawnOutcome {
            stdout: "no json here\n".into(),
            exit_code: 0,
        })]);
        let err = spawn_kloo_task(&req, &mut spawner, &runs, "01-a", 1)
            .await
            .unwrap_err();
        assert!(err.contains("KLOO_RESULT_JSON"), "{err}");
        assert!(
            !runs.join("01-a").join("1.json").exists(),
            "parse fail must not write attempt json"
        );
        let src = std::fs::read_to_string(file!()).unwrap();
        let impl_src = src.split("#[cfg(test)]").next().unwrap();
        assert!(
            !impl_src.contains("wait_for_agent_ready_report"),
            "kloo path must not wait for ready-curl"
        );
        let _ = std::fs::remove_dir_all(&root);
    }

    #[test]
    fn spawn_kloo_never_calls_attach_oneshot() {
        let src = std::fs::read_to_string(file!()).unwrap();
        let impl_src = src.split("#[cfg(test)]").next().unwrap();
        assert!(!impl_src.contains("attach_oneshot"));
        assert!(impl_src.contains("spawn_cli") || impl_src.contains("cli_runner"));
    }

    #[tokio::test]
    async fn spawn_kloo_live_keeps_result_line_after_stdout_burst() {
        let (state, root) = crate::test_support::test_state();
        let script = root.join("fake-kloo");
        let mut body = String::from("#!/bin/sh\n");
        body.push_str("i=0\nwhile [ \"$i\" -lt 4000 ]; do echo noise$i; i=$((i+1)); done\n");
        body.push_str("printf '%s\\n' 'KLOO_RESULT_JSON {\"success\":true,\"files_changed\":{\"count\":1,\"paths\":[\"a.rs\"]},\"off_scope_edits\":0,\"verify\":{\"command\":\"true\",\"passed\":true,\"exit_code\":0}}'\n");
        std::fs::write(&script, body).unwrap();
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            let mut p = std::fs::metadata(&script).unwrap().permissions();
            p.set_mode(0o755);
            std::fs::set_permissions(&script, p).unwrap();
        }
        let req = KlooRunRequest {
            workspace: root.to_string_lossy().into_owned(),
            prompt: "do it".into(),
            files: vec!["a.rs".into()],
            verify: "cargo test spec -- --exact".into(),
            cwd: None,
            model: "qwen/qwen3-coder".into(),
            ctx: None,
            profile: None,
            provider: None,
            cli_path: Some(script.to_string_lossy().into_owned()),
            status_file: None,
        };
        let out = spawn_kloo_live(&state, "plan-burst", &req, Duration::from_secs(15))
            .await
            .expect("spawn live");
        assert_eq!(out.exit_code, 0);
        let parsed = parse_result_from_stdout(&out.stdout).expect("result line must survive");
        assert!(parsed.success);
        assert!(out.stdout.contains("noise3999"), "burst must be captured");
        let _ = std::fs::remove_dir_all(&root);
    }

    #[tokio::test]
    async fn missing_result_json_is_infra_not_model() {
        let root = tmp("parse-infra");
        let (ctx, _) = ctx_for(&root, "00-x");
        write_task(
            &ctx.phase_dir,
            "01-a",
            Some(&yml("01-a", "a.rs", "cargo test spec -- --exact", "", Some("a.rs"), "fn a")),
            "01-a",
        );
        let mut host = ScriptedHost::new(ctx.workspace.clone());
        host.push_stdout("lots of logs\nno result line\n", 0);
        host.push_pass("a.rs", "fn a() {}\n");
        let out = run_kloo_phase_with(&ctx, &mut host).await.unwrap();
        assert_eq!(out, PhaseLoopOutcome::AllDone);
        assert_eq!(host.spawned_models.len(), 2);
        assert_eq!(host.spawned_models[0], "qwen/qwen3-coder");
        assert_eq!(host.spawned_models[1], "qwen/qwen3-coder");
        let file = load_tasks(&tasks_json_path(&ctx.runs_dir), &ctx.plan_id, &ctx.phase_id).unwrap();
        assert_eq!(file.tasks[0].succeeded_tier.as_deref(), Some("qwen3-coder"));
        assert_eq!(file.tasks[0].attempts[0].class.as_deref(), Some("infra"));
        let _ = std::fs::remove_dir_all(&root);
    }

    #[test]
    fn stop_plan_kills_registered_kloo_child() {
        let (state, root) = crate::test_support::test_state();
        let mut cmd = std::process::Command::new("sleep");
        cmd.arg("30");
        #[cfg(unix)]
        {
            use std::os::unix::process::CommandExt;
            cmd.process_group(0);
        }
        let mut child = cmd.spawn().expect("spawn sleep");
        let pid = child.id();
        register_kloo_pid(&state, "plan-kill", pid);
        assert!(state.kloo_pids.lock().unwrap().contains_key("plan-kill"));
        assert!(kill_registered_kloo_child(&state, "plan-kill"));
        let _ = child.wait();
        assert!(state.kloo_pids.lock().unwrap().get("plan-kill").is_none());
        assert!(
            !state.kloo_pids.lock().unwrap().contains_key("plan-kill"),
            "map entry must be gone"
        );
        let _ = std::fs::remove_dir_all(&root);
    }

    #[tokio::test]
    async fn run_kloo_phase_two_independent_pass() {
        let root = tmp("two-pass");
        let (ctx, _) = ctx_for(&root, "00-x");
        write_task(
            &ctx.phase_dir,
            "01-a",
            Some(&yml("01-a", "a.rs", "cargo test spec -- --exact", "", Some("a.rs"), "fn a")),
            "01-a",
        );
        write_task(
            &ctx.phase_dir,
            "02-b",
            Some(&yml("02-b", "b.rs", "cargo test spec -- --exact", "", Some("b.rs"), "fn b")),
            "02-b",
        );
        let mut host = ScriptedHost::new(ctx.workspace.clone());
        host.push_pass("a.rs", "fn a() {}\n");
        host.push_pass("b.rs", "fn b() {}\n");
        let out = run_kloo_phase_with(&ctx, &mut host).await.unwrap();
        assert_eq!(out, PhaseLoopOutcome::AllDone);
        assert_eq!(host.commits, vec!["01-a", "02-b"]);
        let file = load_tasks(&tasks_json_path(&ctx.runs_dir), &ctx.plan_id, &ctx.phase_id).unwrap();
        assert!(file.tasks.iter().all(|t| t.status == "done"));
        assert!(file.tasks.iter().all(|t| t.commit_sha.is_some()));
        assert!(file.tasks.iter().all(|t| t.succeeded_tier.is_some()));
        assert!(
            ctx.runs_dir.join("01-a").join("1.json").is_file(),
            "loop must write attempt JSON"
        );
        assert!(ctx.runs_dir.join("02-b").join("1.json").is_file());
        assert!(
            file.tasks.iter().all(|t| t.attempts.last().and_then(|a| a.class.as_deref()).is_none()),
            "passing attempts must not be labelled infra"
        );
        let _ = std::fs::remove_dir_all(&root);
    }

    #[tokio::test]
    async fn shape_missing_file_does_not_escalate() {
        let root = tmp("shape-dep");
        let (ctx, _) = ctx_for(&root, "00-x");
        write_task(
            &ctx.phase_dir,
            "01-a",
            Some(&yml("01-a", "missing.rs", "cargo test spec -- --exact", "", None, "fn")),
            "01-a",
        );
        write_task(
            &ctx.phase_dir,
            "02-b",
            Some(&yml("02-b", "b.rs", "cargo test spec -- --exact", "01-a", Some("b.rs"), "fn b")),
            "02-b",
        );
        let mut host = ScriptedHost::new(ctx.workspace.clone());
        host.push_pass("b.rs", "fn b() {}\n");
        let out = run_kloo_phase_with(&ctx, &mut host).await.unwrap();
        match out {
            PhaseLoopOutcome::Partial { failed, blocked, .. } => {
                assert_eq!(failed, vec!["01-a"]);
                assert_eq!(blocked, vec!["02-b"]);
            }
            other => panic!("{other:?}"),
        }
        assert!(
            !host.spawned_files.iter().any(|f| f.iter().any(|p| p == "b.rs")),
            "B must not spawn: {:?}",
            host.spawned_files
        );
        assert!(host.spawned_files.is_empty(), "A missing file is preflight, spawn 0");
        let file = load_tasks(&tasks_json_path(&ctx.runs_dir), &ctx.plan_id, &ctx.phase_id).unwrap();
        assert!(file.tasks.iter().find(|t| t.id == "02-b").unwrap().attempts.is_empty());
        let _ = std::fs::remove_dir_all(&root);
    }

    #[tokio::test]
    async fn no_commit_sha_retries_once_then_fails_infra() {
        let root = tmp("no-sha");
        let (ctx, _) = ctx_for(&root, "00-x");
        write_task(
            &ctx.phase_dir,
            "01-a",
            Some(&yml("01-a", "a.rs", "cargo test spec -- --exact", "", Some("a.rs"), "fn a")),
            "01-a",
        );
        write_task(
            &ctx.phase_dir,
            "02-b",
            Some(&yml("02-b", "b.rs", "cargo test spec -- --exact", "01-a", Some("b.rs"), "fn b")),
            "02-b",
        );
        let mut host = ScriptedHost::new(ctx.workspace.clone());
        host.commit_returns_none = true;
        host.push_pass("a.rs", "fn a() {}\n");
        host.push_pass("a.rs", "fn a() { 1 }\n");
        host.push_pass("b.rs", "fn b() {}\n");
        let out = run_kloo_phase_with(&ctx, &mut host).await.unwrap();
        match out {
            PhaseLoopOutcome::Partial { failed, blocked, first_route, .. } => {
                assert_eq!(failed, vec!["01-a"]);
                assert_eq!(blocked, vec!["02-b"]);
                assert_eq!(first_route.as_deref(), Some("infra"));
            }
            other => panic!("{other:?}"),
        }
        assert_eq!(host.spawned_files.len(), 2, "exactly one infra retry: {:?}", host.spawned_files);
        assert!(
            !host.spawned_files.iter().any(|f| f.iter().any(|p| p == "b.rs")),
            "B must not spawn"
        );
        let file = load_tasks(&tasks_json_path(&ctx.runs_dir), &ctx.plan_id, &ctx.phase_id).unwrap();
        assert_eq!(file.tasks[0].status, "failed");
        assert_eq!(file.tasks[0].route.as_deref(), Some("infra"));
        let _ = std::fs::remove_dir_all(&root);
    }

    #[tokio::test]
    async fn empty_prompt_is_shape_fail() {
        let root = tmp("empty-prompt");
        let (ctx, _) = ctx_for(&root, "00-x");
        write_task(
            &ctx.phase_dir,
            "01-a",
            Some(&yml("01-a", "a.rs", "cargo test spec -- --exact", "", Some("a.rs"), "fn a")),
            "   \n",
        );
        let mut host = ScriptedHost::new(ctx.workspace.clone());
        host.push_pass("a.rs", "fn a() {}\n");
        let out = run_kloo_phase_with(&ctx, &mut host).await.unwrap();
        match out {
            PhaseLoopOutcome::Partial { failed, .. } => assert_eq!(failed, vec!["01-a"]),
            other => panic!("{other:?}"),
        }
        assert!(host.spawned_files.is_empty());
        let _ = std::fs::remove_dir_all(&root);
    }

    #[test]
    fn task_yml_without_prompt_is_shape_error() {
        let root = tmp("yml-no-prompt");
        let phase = root.join("phases/00-x");
        let dir = phase.join("tasks").join("01-a");
        std::fs::create_dir_all(&dir).unwrap();
        std::fs::write(dir.join("task.yml"), yml("01-a", "a.rs", "cargo test spec -- --exact", "", None, "fn")).unwrap();
        let err = load_phase_specs(&phase).unwrap_err();
        assert!(err.contains("prompt.md"), "{err}");
        assert!(err.contains("01-a"), "{err}");
        let _ = std::fs::remove_dir_all(&root);
    }

    #[test]
    fn kill_process_group_reaps_sleep() {
        let mut cmd = std::process::Command::new("sleep");
        cmd.arg("30");
        #[cfg(unix)]
        {
            use std::os::unix::process::CommandExt;
            cmd.process_group(0);
        }
        let mut child = cmd.spawn().expect("spawn sleep");
        let pid = child.id();
        assert!(kill_process_group(pid));
        let _ = child.wait();
        assert!(!pid_is_alive(pid));
    }

    #[tokio::test]
    async fn dependent_edits_ancestor_new_file_is_not_shape_fail() {
        let root = tmp("dep-new");
        let (ctx, _) = ctx_for(&root, "00-x");
        write_task(
            &ctx.phase_dir,
            "01-a",
            Some(&yml("01-a", "a.rs", "cargo test spec -- --exact", "", Some("a.rs"), "fn a")),
            "01-a",
        );
        write_task(
            &ctx.phase_dir,
            "02-b",
            Some(&yml("02-b", "a.rs, b.rs", "cargo test spec -- --exact", "01-a", Some("b.rs"), "fn b")),
            "02-b",
        );
        let mut host = ScriptedHost::new(ctx.workspace.clone());
        host.push_pass("a.rs", "fn a() {}\n");
        host.push_pass("b.rs", "fn a() {}\nfn b() {}\n");
        let out = run_kloo_phase_with(&ctx, &mut host).await.unwrap();
        assert_eq!(out, PhaseLoopOutcome::AllDone);
        assert_eq!(host.spawned_files.len(), 2, "both A and B must spawn: {:?}", host.spawned_files);
        let file = load_tasks(&tasks_json_path(&ctx.runs_dir), &ctx.plan_id, &ctx.phase_id).unwrap();
        assert!(file.tasks.iter().all(|t| t.status == "done"), "{:?}", file.tasks);
        let _ = std::fs::remove_dir_all(&root);
    }

    #[tokio::test]
    async fn task_added_after_seed_is_run_not_silently_done() {
        let root = tmp("late-task");
        let (ctx, _) = ctx_for(&root, "00-x");
        write_task(
            &ctx.phase_dir,
            "01-a",
            Some(&yml("01-a", "a.rs", "cargo test spec -- --exact", "", Some("a.rs"), "fn a")),
            "01-a",
        );
        let mut file = seed_from_specs(
            &ctx.plan_id,
            &ctx.phase_id,
            &load_phase_specs(&ctx.phase_dir).unwrap(),
        );
        file.tasks[0].status = "done".into();
        file.tasks[0].commit_sha = Some("sha-a".into());
        file.tasks[0].succeeded_tier = Some("qwen3-coder".into());
        save_tasks(&tasks_json_path(&ctx.runs_dir), &mut file).unwrap();
        write_task(
            &ctx.phase_dir,
            "02-b",
            Some(&yml("02-b", "b.rs", "cargo test spec -- --exact", "", Some("b.rs"), "fn b")),
            "02-b",
        );
        let mut host = ScriptedHost::new(ctx.workspace.clone());
        host.index
            .insert(("00-x".into(), "01-a".into()), "sha-a".into());
        host.push_pass("b.rs", "fn b() {}\n");
        let out = run_kloo_phase_with(&ctx, &mut host).await.unwrap();
        assert_eq!(out, PhaseLoopOutcome::AllDone);
        assert_eq!(host.spawned_files, vec![vec!["b.rs".to_string()]]);
        let loaded = load_tasks(&tasks_json_path(&ctx.runs_dir), &ctx.plan_id, &ctx.phase_id).unwrap();
        assert_eq!(loaded.tasks.len(), 2);
        assert_eq!(loaded.tasks[1].id, "02-b");
        assert_eq!(loaded.tasks[1].status, "done");
        let _ = std::fs::remove_dir_all(&root);
    }

    #[tokio::test]
    async fn run_kloo_phase_model_then_claude() {
        let root = tmp("esc");
        let (ctx, _) = ctx_for(&root, "00-x");
        write_task(
            &ctx.phase_dir,
            "01-a",
            Some(&yml("01-a", "a.rs", "cargo test spec -- --exact", "", Some("a.rs"), "pub fn add")),
            "01-a",
        );
        std::fs::write(ctx.workspace.join("a.rs"), "old\n").unwrap();
        let mut host = ScriptedHost::new(ctx.workspace.clone());
        host.push_fail("a.rs", "fn nope() {}\n", "verify_failed");
        host.push_fail("a.rs", "fn still() {}\n", "verify_failed");
        host.push_pass("a.rs", "pub fn add() {}\n");
        let out = run_kloo_phase_with(&ctx, &mut host).await.unwrap();
        assert_eq!(out, PhaseLoopOutcome::AllDone);
        assert_eq!(
            host.spawned_models,
            vec![
                "qwen/qwen3-coder",
                "qwen/qwen3-coder",
                "anthropic/claude-sonnet-5"
            ]
        );
        let escalated = host
            .events
            .iter()
            .filter(|e| *e == "agent_phase_task_escalated")
            .count();
        assert_eq!(escalated, 1, "only qwen→claude, not same-tier retry: {:?}", host.events);
        let file = load_tasks(&tasks_json_path(&ctx.runs_dir), &ctx.plan_id, &ctx.phase_id).unwrap();
        assert_eq!(file.tasks[0].succeeded_tier.as_deref(), Some("claude"));
        let _ = std::fs::remove_dir_all(&root);
    }

    #[tokio::test]
    async fn exhausted_model_routes_planner_and_blocks() {
        let root = tmp("exh");
        let (ctx, _) = ctx_for(&root, "00-x");
        write_task(
            &ctx.phase_dir,
            "01-a",
            Some(&yml("01-a", "a.rs", "cargo test spec -- --exact", "", Some("a.rs"), "pub fn add")),
            "01-a",
        );
        write_task(
            &ctx.phase_dir,
            "02-b",
            Some(&yml("02-b", "b.rs", "cargo test spec -- --exact", "01-a", Some("b.rs"), "fn b")),
            "02-b",
        );
        std::fs::write(ctx.workspace.join("a.rs"), "old\n").unwrap();
        let mut host = ScriptedHost::new(ctx.workspace.clone());
        for _ in 0..4 {
            host.push_fail("a.rs", "fn nope() {}\n", "verify_failed");
        }
        let out = run_kloo_phase_with(&ctx, &mut host).await.unwrap();
        match out {
            PhaseLoopOutcome::Partial { failed, blocked, first_route, .. } => {
                assert_eq!(failed, vec!["01-a"]);
                assert_eq!(blocked, vec!["02-b"]);
                assert_eq!(first_route.as_deref(), Some("planner"));
            }
            other => panic!("{other:?}"),
        }
        assert_eq!(host.spawned_models.len(), 4);
        let file = load_tasks(&tasks_json_path(&ctx.runs_dir), &ctx.plan_id, &ctx.phase_id).unwrap();
        assert!(file.tasks.iter().find(|t| t.id == "02-b").unwrap().attempts.is_empty());
        let _ = std::fs::remove_dir_all(&root);
    }

    #[tokio::test]
    async fn run_kloo_phase_new_missing_is_not_preflight_fail() {
        let root = tmp("new-miss");
        let (ctx, _) = ctx_for(&root, "00-x");
        write_task(
            &ctx.phase_dir,
            "01-a",
            Some(&yml("01-a", "new.rs", "cargo test spec -- --exact", "", Some("new.rs"), "hello")),
            "01-a",
        );
        let mut host = ScriptedHost::new(ctx.workspace.clone());
        host.push_pass("new.rs", "hello world\n");
        let out = run_kloo_phase_with(&ctx, &mut host).await.unwrap();
        assert_eq!(out, PhaseLoopOutcome::AllDone);
        assert_eq!(host.spawned_files.len(), 1);
        let _ = std::fs::remove_dir_all(&root);
    }

    #[tokio::test]
    async fn run_kloo_phase_resume_residue_no_spawn() {
        let root = tmp("residue");
        let (ctx, _) = ctx_for(&root, "00-x");
        write_task(
            &ctx.phase_dir,
            "01-a",
            Some(&yml("01-a", "a.rs", "cargo test spec -- --exact", "", None, "pub fn add")),
            "01-a",
        );
        std::fs::write(ctx.workspace.join("a.rs"), "pub fn add() {}\n").unwrap();
        let mut file = seed_from_specs(&ctx.plan_id, &ctx.phase_id, &load_phase_specs(&ctx.phase_dir).unwrap());
        file.tasks[0].status = "running".into();
        file.tasks[0].attempts.push(AttemptRecord {
            tier: "qwen3-coder".into(),
            model: "qwen/qwen3-coder".into(),
            attempt: 1,
            started_at: Some("t".into()),
            ended_at: None,
            failure_code: None,
            exit_code: None,
            class: None,
            checks: None,
        });
        save_tasks(&tasks_json_path(&ctx.runs_dir), &mut file).unwrap();
        let mut host = ScriptedHost::new(ctx.workspace.clone());
        let out = run_kloo_phase_with(&ctx, &mut host).await.unwrap();
        assert_eq!(out, PhaseLoopOutcome::AllDone);
        assert!(host.spawned_files.is_empty(), "residue must not spawn");
        assert!(!host.events.iter().any(|e| e.contains("escalat")));
        let loaded = load_tasks(&tasks_json_path(&ctx.runs_dir), &ctx.plan_id, &ctx.phase_id).unwrap();
        assert_eq!(loaded.tasks[0].status, "done");
        assert!(loaded.tasks[0].commit_sha.is_some());
        let _ = std::fs::remove_dir_all(&root);
    }

    #[tokio::test]
    async fn new_file_fail_then_pass_is_done_not_planner() {
        let root = tmp("new-retry");
        let (ctx, _) = ctx_for(&root, "00-x");
        write_task(
            &ctx.phase_dir,
            "01-a",
            Some(&yml("01-a", "new.rs", "cargo test spec -- --exact", "", Some("new.rs"), "pub fn add")),
            "01-a",
        );
        let mut host = ScriptedHost::new(ctx.workspace.clone());
        host.push_fail("new.rs", "fn nope() {}\n", "verify_failed");
        host.push_pass("new.rs", "pub fn add() {}\n");
        let out = run_kloo_phase_with(&ctx, &mut host).await.unwrap();
        assert_eq!(out, PhaseLoopOutcome::AllDone);
        let loaded = load_tasks(&tasks_json_path(&ctx.runs_dir), &ctx.plan_id, &ctx.phase_id).unwrap();
        assert_eq!(loaded.tasks[0].status, "done");
        assert_eq!(loaded.tasks[0].route, None);
        assert_eq!(loaded.tasks[0].succeeded_tier.as_deref(), Some("qwen3-coder"));
        assert_eq!(host.spawned_models.len(), 2);
        let _ = std::fs::remove_dir_all(&root);
    }

    #[tokio::test]
    async fn run_kloo_phase_mid_stop_is_cancelled() {
        let root = tmp("stop");
        let (ctx, _) = ctx_for(&root, "00-x");
        for (id, f, needle) in [("01-a", "a.rs", "fn a"), ("02-b", "b.rs", "fn b"), ("03-c", "c.rs", "fn c")] {
            write_task(
                &ctx.phase_dir,
                id,
                Some(&yml(id, f, "cargo test spec -- --exact", "", Some(f), needle)),
                id,
            );
        }
        let mut host = ScriptedHost::new(ctx.workspace.clone());
        host.stop_after_commits = Some(1);
        host.push_pass("a.rs", "fn a() {}\n");
        host.push_pass("b.rs", "fn b() {}\n");
        host.push_pass("c.rs", "fn c() {}\n");
        let out = run_kloo_phase_with(&ctx, &mut host).await.unwrap();
        assert_eq!(out, PhaseLoopOutcome::Cancelled);
        assert_eq!(host.commits, vec!["01-a"]);
        assert_eq!(host.spawned_files.len(), 1);
        let _ = std::fs::remove_dir_all(&root);
    }

    #[tokio::test]
    async fn resume_skips_committed_task() {
        let root = tmp("resume-skip");
        let (ctx, _) = ctx_for(&root, "00-x");
        write_task(
            &ctx.phase_dir,
            "01-a",
            Some(&yml("01-a", "a.rs", "cargo test spec -- --exact", "", Some("a.rs"), "fn a")),
            "01-a",
        );
        write_task(
            &ctx.phase_dir,
            "02-b",
            Some(&yml("02-b", "b.rs", "cargo test spec -- --exact", "", Some("b.rs"), "fn b")),
            "02-b",
        );
        let mut file = seed_from_specs(&ctx.plan_id, &ctx.phase_id, &load_phase_specs(&ctx.phase_dir).unwrap());
        file.tasks[0].status = "done".into();
        file.tasks[0].commit_sha = Some("sha-01-a".into());
        file.tasks[0].succeeded_tier = Some("qwen3-coder".into());
        file.tasks[1].status = "running".into();
        save_tasks(&tasks_json_path(&ctx.runs_dir), &mut file).unwrap();
        let mut host = ScriptedHost::new(ctx.workspace.clone());
        host.index
            .insert(("00-x".into(), "01-a".into()), "sha-01-a".into());
        host.push_pass("b.rs", "fn b() {}\n");
        let out = run_kloo_phase_with(&ctx, &mut host).await.unwrap();
        assert_eq!(out, PhaseLoopOutcome::AllDone);
        assert_eq!(host.spawned_files, vec![vec!["b.rs".to_string()]]);
        assert!(!host.spawned_files.iter().any(|f| f.iter().any(|p| p == "a.rs")));
        let _ = std::fs::remove_dir_all(&root);
    }

    #[tokio::test]
    async fn resume_reruns_running_without_commit() {
        let root = tmp("resume-rerun");
        let (ctx, _) = ctx_for(&root, "00-x");
        write_task(
            &ctx.phase_dir,
            "01-a",
            Some(&yml("01-a", "a.rs", "cargo test spec -- --exact", "", Some("a.rs"), "fn a")),
            "01-a",
        );
        write_task(
            &ctx.phase_dir,
            "02-b",
            Some(&yml("02-b", "b.rs", "cargo test spec -- --exact", "", Some("b.rs"), "fn b")),
            "02-b",
        );
        write_task(
            &ctx.phase_dir,
            "03-c",
            Some(&yml("03-c", "c.rs", "cargo test spec -- --exact", "", Some("c.rs"), "fn c")),
            "03-c",
        );
        let mut file = seed_from_specs(&ctx.plan_id, &ctx.phase_id, &load_phase_specs(&ctx.phase_dir).unwrap());
        file.tasks[0].status = "done".into();
        file.tasks[0].commit_sha = Some("sha-a".into());
        file.tasks[1].status = "running".into();
        file.tasks[1].commit_sha = None;
        save_tasks(&tasks_json_path(&ctx.runs_dir), &mut file).unwrap();
        let mut host = ScriptedHost::new(ctx.workspace.clone());
        host.index.insert(("00-x".into(), "01-a".into()), "sha-a".into());
        // B running WITH commit → reconcile marks done; spawn C.
        host.index.insert(("00-x".into(), "02-b".into()), "sha-b".into());
        host.push_pass("c.rs", "fn c() {}\n");
        let out = run_kloo_phase_with(&ctx, &mut host).await.unwrap();
        assert_eq!(out, PhaseLoopOutcome::AllDone);
        assert_eq!(host.spawned_files, vec![vec!["c.rs".to_string()]]);
        let loaded = load_tasks(&tasks_json_path(&ctx.runs_dir), &ctx.plan_id, &ctx.phase_id).unwrap();
        assert_eq!(loaded.tasks[1].status, "done");
        assert_eq!(loaded.tasks[1].commit_sha.as_deref(), Some("sha-b"));
        let _ = std::fs::remove_dir_all(&root);
    }

    #[test]
    fn cancelled_leaves_stopped_no_review() {
        let outcome = PhaseLoopOutcome::Cancelled;
        assert_eq!(
            followup_after_kloo_phase(&outcome),
            KlooPhaseFollowup::LeaveAsIs
        );
        assert_ne!(
            followup_after_kloo_phase(&outcome),
            KlooPhaseFollowup::DispatchReview
        );
        match followup_after_kloo_phase(&PhaseLoopOutcome::Partial {
            failed: vec!["01-a".into()],
            blocked: vec!["02-b".into()],
            first_route: Some("planner".into()),
            first_task_id: Some("01-a".into()),
        }) {
            KlooPhaseFollowup::NeedsAttention { task_id, route } => {
                assert_eq!(task_id, "01-a");
                assert_eq!(route, "planner");
            }
            other => panic!("{other:?}"),
        }
        assert_eq!(
            followup_after_kloo_phase(&PhaseLoopOutcome::AllDone),
            KlooPhaseFollowup::DispatchReview
        );
    }

    #[tokio::test]
    async fn residue_policy_error_fails_task_as_planner_not_phase() {
        let root = tmp("residue-policy");
        let (ctx, _) = ctx_for(&root, "00-x");
        write_task(
            &ctx.phase_dir,
            "01-a",
            Some(&yml("01-a", "a.rs", "cargo test spec -- --exact", "", None, "pub fn add")),
            "01-a",
        );
        std::fs::write(ctx.workspace.join("a.rs"), "pub fn add() {}\n").unwrap();
        let mut file = seed_from_specs(
            &ctx.plan_id,
            &ctx.phase_id,
            &load_phase_specs(&ctx.phase_dir).unwrap(),
        );
        file.tasks[0].status = "running".into();
        file.tasks[0].attempts.push(AttemptRecord {
            tier: "qwen3-coder".into(),
            model: "qwen/qwen3-coder".into(),
            attempt: 1,
            started_at: Some("t".into()),
            ended_at: None,
            failure_code: None,
            exit_code: None,
            class: None,
            checks: None,
        });
        save_tasks(&tasks_json_path(&ctx.runs_dir), &mut file).unwrap();
        let mut host = ScriptedHost::new(ctx.workspace.clone());
        host.verify_err = Some("shape: verify_not_allowlisted: denied runner".into());
        let out = run_kloo_phase_with(&ctx, &mut host).await.unwrap();
        match out {
            PhaseLoopOutcome::Partial { failed, first_route, .. } => {
                assert_eq!(failed, vec!["01-a"]);
                assert_eq!(first_route.as_deref(), Some("planner"));
            }
            other => panic!("{other:?}"),
        }
        let loaded = load_tasks(&tasks_json_path(&ctx.runs_dir), &ctx.plan_id, &ctx.phase_id).unwrap();
        assert_eq!(loaded.tasks[0].route.as_deref(), Some("planner"));
        assert!(host.spawned_files.is_empty());
        let _ = std::fs::remove_dir_all(&root);
    }

    #[tokio::test]
    async fn kloo_policy_spawn_error_is_shape_not_escalated() {
        let root = tmp("spawn-policy");
        let (ctx, _) = ctx_for(&root, "00-x");
        write_task(
            &ctx.phase_dir,
            "01-a",
            Some(&yml("01-a", "a.rs", "cargo test spec -- --exact", "", Some("a.rs"), "fn a")),
            "01-a",
        );
        let mut host = ScriptedHost::new(ctx.workspace.clone());
        host.spawn_scripts.push_back(Box::new(|_| {
            Err("shape: verify_not_allowlisted: node --eval".into())
        }));
        let out = run_kloo_phase_with(&ctx, &mut host).await.unwrap();
        match out {
            PhaseLoopOutcome::Partial { failed, first_route, .. } => {
                assert_eq!(failed, vec!["01-a"]);
                assert_eq!(first_route.as_deref(), Some("planner"));
            }
            other => panic!("{other:?}"),
        }
        assert_eq!(host.spawned_models.len(), 1, "must not climb the ladder");
        let _ = std::fs::remove_dir_all(&root);
    }

    #[tokio::test]
    async fn verify_127_is_shape() {
        let root = tmp("v127");
        let (ctx, _) = ctx_for(&root, "00-x");
        write_task(
            &ctx.phase_dir,
            "01-a",
            Some(&yml("01-a", "a.rs", "cargo test spec -- --exact", "", Some("a.rs"), "fn a")),
            "01-a",
        );
        write_task(
            &ctx.phase_dir,
            "02-b",
            Some(&yml("02-b", "b.rs", "cargo test spec -- --exact", "01-a", Some("b.rs"), "fn b")),
            "02-b",
        );
        let mut host = ScriptedHost::new(ctx.workspace.clone());
        host.push_stdout(
            r#"KLOO_RESULT_JSON {"success":false,"failure_code":"verify_failed","files_changed":{"count":0,"paths":[]},"verify":{"command":"missing","passed":false,"exit_code":127}}"#,
            10,
        );
        let out = run_kloo_phase_with(&ctx, &mut host).await.unwrap();
        match out {
            PhaseLoopOutcome::Partial { failed, blocked, .. } => {
                assert_eq!(failed, vec!["01-a"]);
                assert_eq!(blocked, vec!["02-b"]);
            }
            other => panic!("{other:?}"),
        }
        assert_eq!(host.spawned_models.len(), 1);
        let _ = std::fs::remove_dir_all(&root);
    }

    #[tokio::test]
    async fn postcheck_failed_climbs_ladder_then_planner() {
        let root = tmp("postcheck");
        let (ctx, _) = ctx_for(&root, "00-x");
        write_task(
            &ctx.phase_dir,
            "01-a",
            Some(&yml("01-a", "a.rs", "cargo test spec -- --exact", "", Some("a.rs"), "fn a")),
            "01-a",
        );
        write_task(
            &ctx.phase_dir,
            "02-b",
            Some(&yml("02-b", "b.rs", "cargo test spec -- --exact", "01-a", Some("b.rs"), "fn b")),
            "02-b",
        );
        std::fs::write(ctx.workspace.join("a.rs"), "old\n").unwrap();
        let mut host = ScriptedHost::new(ctx.workspace.clone());
        for body in ["fn a() {}\n", "fn a() {1}\n", "fn a() {2}\n", "fn a() {3}\n"] {
            let ws = host.workspace.clone();
            let body = body.to_string();
            host.spawn_scripts.push_back(Box::new(move |_req| {
                std::fs::write(ws.join("a.rs"), &body).unwrap();
                Ok(SpawnOutcome {
                    stdout: r#"KLOO_RESULT_JSON {"success":false,"failure_code":"postcheck_failed","files_changed":{"count":1,"paths":["a.rs"]},"verify":{"command":"true","passed":true,"exit_code":0},"postchecks":[{"command":"lint","passed":false,"exit_code":1}]}"#.into(),
                    exit_code: 10,
                })
            }));
        }
        let out = run_kloo_phase_with(&ctx, &mut host).await.unwrap();
        match out {
            PhaseLoopOutcome::Partial { failed, blocked, first_route, .. } => {
                assert_eq!(failed, vec!["01-a"]);
                assert_eq!(blocked, vec!["02-b"]);
                assert_eq!(first_route.as_deref(), Some("planner"));
            }
            other => panic!("{other:?}"),
        }
        assert_eq!(host.spawned_models.len(), 4);
        assert_eq!(host.spawned_models[3], "anthropic/claude-opus-5");
        let _ = std::fs::remove_dir_all(&root);
    }

    #[test]
    fn run_task_verify_returns_exit_code_for_allowlisted_filter() {
        let root = tmp("verify-exit");
        let code = run_task_verify(&root, "cargo test mul -- --exact", None).unwrap();
        assert_ne!(code, 127, "cargo must have been spawned (exit {code})");
        let _ = std::fs::remove_dir_all(&root);
    }

    #[test]
    fn run_task_verify_cwd_is_chdir_not_argv_rewrite() {
        let root = tmp("verify-cwd");
        let pkg = root.join("pkg");
        std::fs::create_dir_all(&pkg).unwrap();
        let plan = plan_task_verify(&root, "cargo test mul -- --exact", Some("pkg")).unwrap();
        assert!(
            plan.current_dir.ends_with("pkg"),
            "{:?}",
            plan.current_dir
        );
        assert_eq!(
            plan.argv,
            vec!["cargo", "test", "mul", "--", "--exact"]
        );
        assert!(!plan.argv.iter().any(|a| a == "--manifest-path" || a == "--prefix"));
        let _ = std::fs::remove_dir_all(&root);
    }

    #[test]
    fn run_task_verify_and_kloo_share_cwd_and_authorised_argv() {
        let root = tmp("verify-shared");
        std::fs::create_dir_all(root.join("pkg")).unwrap();
        let verify = "cargo test mul -- --exact";
        let plan = plan_task_verify(&root, verify, Some("pkg")).unwrap();
        let req = KlooRunRequest {
            workspace: root.to_string_lossy().into_owned(),
            prompt: "x".into(),
            files: vec!["pkg/src/lib.rs".into()],
            verify: verify.into(),
            cwd: Some("pkg".into()),
            model: "qwen/qwen3-coder".into(),
            ctx: None,
            profile: None,
            provider: None,
            cli_path: None,
            status_file: None,
        };
        let cfg = build_config(&req).unwrap();
        assert_eq!(
            cfg.working_directory,
            plan.current_dir.to_string_lossy().as_ref()
        );
        let quoted = quote_argv_for_shell(&plan.argv);
        assert_eq!(
            cfg.args.windows(2).find(|w| w[0] == "--verify").map(|w| w[1].as_str()),
            Some(quoted.as_str())
        );
        assert_eq!(plan.argv, check_verify(verify).unwrap());
        let _ = std::fs::remove_dir_all(&root);
    }

    #[test]
    fn run_task_verify_does_not_spawn_rm() {
        let root = tmp("verify-rm");
        let sentinel = root.join("keep");
        std::fs::write(&sentinel, "safe").unwrap();
        let err = run_task_verify(&root, "rm -rf /", None).unwrap_err();
        assert!(err.contains("verify_not_allowlisted") || err.contains("shape"), "{err}");
        assert_eq!(std::fs::read_to_string(&sentinel).unwrap(), "safe");
        let _ = std::fs::remove_dir_all(&root);
    }

    #[test]
    fn run_task_verify_records_literal_brace_and_semicolon_argv() {
        let root = tmp("verify-lit");
        let plan = plan_task_verify(&root, "cargo test {a,b}", None).unwrap();
        assert_eq!(plan.argv, vec!["cargo", "test", "{a,b}"]);
        let code = run_task_verify(&root, "cargo test {a,b}", None).unwrap();
        assert_ne!(code, 127, "cargo must spawn with literal {{a,b}} (exit {code})");
        let plan = plan_task_verify(&root, r#"cargo test "a;b""#, None).unwrap();
        assert_eq!(plan.argv, vec!["cargo", "test", "a;b"]);
        let _ = std::fs::remove_dir_all(&root);
    }

    #[test]
    fn verify_execution_modules_never_construct_a_shell() {
        let manifest = PathBuf::from(env!("CARGO_MANIFEST_DIR"));
        let files = [
            "src/services/task_loop.rs",
            "src/providers/kloo_cli.rs",
            "src/services/verify_policy.rs",
        ];
        for rel in files {
            let src = std::fs::read_to_string(manifest.join(rel)).unwrap();
            let impl_src = src.split("#[cfg(test)]").next().unwrap_or(&src);
            assert!(
                !impl_src.contains("Command::new(\"sh\")")
                    && !impl_src.contains("Command::new(\"bash\")"),
                "{rel} must not construct sh/bash"
            );
            assert!(
                !impl_src.contains(".args([\"-c\"")
                    && !impl_src.contains(".arg(\"-c\")"),
                "{rel} must not pass -c to a shell"
            );
        }
        let plan_check = manifest.join("src/services/plan_check.rs");
        if plan_check.is_file() {
            let src = std::fs::read_to_string(&plan_check).unwrap();
            let impl_src = src.split("#[cfg(test)]").next().unwrap_or(&src);
            assert!(!impl_src.contains("Command::new(\"sh\")"));
            assert!(!impl_src.contains("Command::new(\"bash\")"));
        }
    }
}
