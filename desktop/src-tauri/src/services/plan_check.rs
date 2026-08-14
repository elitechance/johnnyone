//! Static plan-check (shape). Structured `(task_id, rule, detail)` items.
//!
//! No process is spawned here — execute-verify is phase 02. Reuses
//! `task_spec::{load_task_spec, topo_sort}` and `verify_policy::check_verify`.
//!
//! `load_task_spec` shape errors other than empty verify are folded into
//! `missing_files`; the original `shape: …` text is kept in `detail` so the
//! planner can see id-mismatch / jail / `new:`-not-in-`files[]`. That fold is
//! part of the rule-id API (changing an emitted `rule` is a breaking change).

use crate::services::atomic_fs;
use crate::services::task_spec::{self, load_task_spec, DagError, TaskSpec};
use crate::services::task_state::TaskRunFile;
use crate::services::verify_policy::{self, check_verify};
use serde::{Deserialize, Serialize};
use std::collections::{HashMap, HashSet};
use std::path::{Component, Path, PathBuf};
use std::time::Instant;

pub const RULE_MISSING_FILES: &str = "missing_files";
pub const RULE_EMPTY_VERIFY: &str = "empty_verify";
pub const RULE_EMPTY_PROMPT: &str = "empty_prompt";
pub const RULE_FILE_MISSING: &str = "file_missing";
pub const RULE_FILE_COLLISION: &str = "file_collision";
pub const RULE_DEPENDS_UNRESOLVED: &str = "depends_unresolved";
pub const RULE_DEPENDS_CYCLE: &str = "depends_cycle";
pub const RULE_DEPENDS_FORWARD: &str = "depends_forward";
pub const RULE_DEPENDS_SELF: &str = "depends_self";
pub const RULE_VERIFY_NOT_ALLOWLISTED: &str = "verify_not_allowlisted";
pub const RULE_VERIFY_NOT_SCOPED: &str = "verify_not_scoped";
pub const RULE_VERIFY_TARGET_MISSING: &str = "verify_target_missing";
pub const RULE_VERIFY_CWD_MISSING: &str = "verify_cwd_missing";
pub const RULE_FILES_OUTSIDE_CWD: &str = "files_outside_cwd";
pub const RULE_VERIFY_NOT_RUNNABLE: &str = "verify_not_runnable";
pub const RULE_MUST_CONTAIN_TRIVIAL: &str = "must_contain_trivial";
pub const RULE_VERIFY_TIMEOUT: &str = "verify_timeout";
pub const RULE_VERIFY_NOT_EXECUTED: &str = "verify_not_executed";
pub const RULE_PROMPT_EXCEEDS_CONTEXT: &str = "prompt_exceeds_context";
pub const RULE_TOKENS_UNAVAILABLE: &str = "tokens_unavailable";
pub const RULE_TASK_COUNT_PHASE: &str = "task_count_phase";
pub const RULE_TASK_COUNT_TOTAL: &str = "task_count_total";
pub const RULE_UI_TASK_FORBIDDEN: &str = "ui_task_forbidden";
/// Plan-level: zero task dirs under `phases/*/tasks`. Blocking. Not in the
/// original overview table; added so a local-small planner that emitted no
/// `task.yml` cannot `passed=true` into the lenses (D4).
pub const RULE_EMPTY_PLAN: &str = "empty_plan";

pub const MAX_TASKS_PER_PHASE: usize = 150;
pub const MAX_TASKS_TOTAL: usize = 800;
pub const MAX_WARM_PAIRS: usize = 8;
pub const MAX_PLANNING_CHECK_MS: u64 = 1_800_000;
pub const VERIFY_TASK_TIMEOUT_MS: u64 = 15_000;
pub const WARM_TIMEOUT_MS: u64 = 120_000;

/// Complete overview-table rule ids (phase 02-05 inventory).
pub const RULE_IDS: &[&str] = &[
    RULE_MISSING_FILES,
    RULE_EMPTY_VERIFY,
    RULE_EMPTY_PROMPT,
    RULE_FILE_MISSING,
    RULE_FILE_COLLISION,
    RULE_DEPENDS_UNRESOLVED,
    RULE_DEPENDS_CYCLE,
    RULE_DEPENDS_FORWARD,
    RULE_DEPENDS_SELF,
    RULE_VERIFY_NOT_ALLOWLISTED,
    RULE_VERIFY_NOT_SCOPED,
    RULE_VERIFY_TARGET_MISSING,
    RULE_VERIFY_CWD_MISSING,
    RULE_FILES_OUTSIDE_CWD,
    RULE_VERIFY_NOT_RUNNABLE,
    RULE_MUST_CONTAIN_TRIVIAL,
    RULE_VERIFY_TIMEOUT,
    RULE_VERIFY_NOT_EXECUTED,
    RULE_PROMPT_EXCEEDS_CONTEXT,
    RULE_TOKENS_UNAVAILABLE,
    RULE_TASK_COUNT_PHASE,
    RULE_TASK_COUNT_TOTAL,
    RULE_EMPTY_PLAN,
    RULE_UI_TASK_FORBIDDEN,
];

const TRIVIAL_NEEDLES: &[&str] = &[
    "true", "false", "null", "none", "todo", "test", "pass", "fail", "function", "return",
    "class", "export", "import",
];

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct PlanCheckItem {
    pub task_id: Option<String>,
    pub rule: String,
    pub detail: String,
    pub blocking: bool,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct PhaseVerifyCounts {
    pub phase_id: String,
    pub verify_executed: usize,
    pub verify_skipped: usize,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct PlanCheckReport {
    pub passed: bool,
    pub tasks_checked: usize,
    pub items: Vec<PlanCheckItem>,
    pub shape_ms: u64,
    pub warm_ms: u64,
    pub verify_ms: u64,
    pub verify_executed: usize,
    pub verify_skipped: usize,
    pub phases: Vec<PhaseVerifyCounts>,
    /// Task ids that never started (budget / stage ceiling). Next execute
    /// pass sorts these first (D7 carry-over).
    #[serde(default)]
    pub skipped_ids: Vec<String>,
}

impl PlanCheckReport {
    pub fn empty() -> Self {
        Self {
            passed: true,
            tasks_checked: 0,
            items: Vec::new(),
            shape_ms: 0,
            warm_ms: 0,
            verify_ms: 0,
            verify_executed: 0,
            verify_skipped: 0,
            phases: Vec::new(),
            skipped_ids: Vec::new(),
        }
    }

    fn finish(&mut self, shape_ms: u64) {
        self.shape_ms = shape_ms;
        self.passed = !self.items.iter().any(|i| i.blocking);
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SpawnKind {
    Warm,
    Task,
}

#[derive(Debug, Clone)]
pub struct SpawnRequest {
    pub argv: Vec<String>,
    pub cwd: PathBuf,
    pub timeout_ms: u64,
    pub kind: SpawnKind,
    pub task_id: Option<String>,
}

#[derive(Debug, Clone)]
pub struct SpawnResult {
    pub started: bool,
    pub exit_code: Option<i32>,
    pub timed_out: bool,
    pub enoent: bool,
}

impl SpawnResult {
    pub fn ok_zero() -> Self {
        Self {
            started: true,
            exit_code: Some(0),
            timed_out: false,
            enoent: false,
        }
    }

    pub fn exit(code: i32) -> Self {
        Self {
            started: true,
            exit_code: Some(code),
            timed_out: false,
            enoent: false,
        }
    }

    pub fn missing() -> Self {
        Self {
            started: false,
            exit_code: None,
            timed_out: false,
            enoent: true,
        }
    }

    pub fn timed_out() -> Self {
        Self {
            started: true,
            exit_code: None,
            timed_out: true,
            enoent: false,
        }
    }

    pub fn never_started() -> Self {
        Self {
            started: false,
            exit_code: None,
            timed_out: false,
            enoent: false,
        }
    }

}

#[derive(Debug, Clone)]
pub struct TokensReport {
    pub fits: bool,
    pub approx_tokens: Option<u64>,
    pub usable_window: Option<u64>,
    pub compact_trigger: Option<u64>,
}

#[derive(Debug, Clone)]
pub enum TokensError {
    Unavailable(String),
    Parse(String),
}

pub trait PlanCheckHost: Send + Sync {
    fn spawn(&self, req: &SpawnRequest) -> SpawnResult;
    fn tokens(&self, ctx: u32, prompt: &Path) -> Result<TokensReport, TokensError>;
}

/// Live process host. Never `sh -c`.
pub struct RealPlanCheckHost;

impl PlanCheckHost for RealPlanCheckHost {
    fn spawn(&self, req: &SpawnRequest) -> SpawnResult {
        spawn_argv(&req.argv, &req.cwd, req.timeout_ms)
    }

    fn tokens(&self, ctx: u32, prompt: &Path) -> Result<TokensReport, TokensError> {
        let argv = vec![
            "kloo".to_string(),
            "tokens".to_string(),
            "--json".to_string(),
            "--ctx".to_string(),
            ctx.to_string(),
            "--file".to_string(),
            prompt.to_string_lossy().into_owned(),
        ];
        let out = spawn_argv_capture(&argv, Path::new("."), 30_000);
        match out {
            Err(e) if e.enoent => Err(TokensError::Unavailable(e.detail)),
            Err(e) => Err(TokensError::Unavailable(e.detail)),
            Ok(body) => parse_tokens_json(&body),
        }
    }
}

struct SpawnErr {
    enoent: bool,
    detail: String,
}

fn spawn_argv(argv: &[String], cwd: &Path, timeout_ms: u64) -> SpawnResult {
    let Some(exe) = argv.first() else {
        return SpawnResult::missing();
    };
    let mut cmd = std::process::Command::new(exe);
    cmd.args(&argv[1..])
        .current_dir(cwd)
        .stdout(std::process::Stdio::null())
        .stderr(std::process::Stdio::null());
    #[cfg(unix)]
    {
        use std::os::unix::process::CommandExt;
        cmd.process_group(0);
    }
    let mut child = match cmd.spawn() {
        Ok(c) => c,
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => return SpawnResult::missing(),
        Err(_) => return SpawnResult::missing(),
    };
    let pid = child.id();
    let (done_tx, done_rx) = std::sync::mpsc::channel::<()>();
    let timeout = std::time::Duration::from_millis(timeout_ms.max(1));
    std::thread::spawn(move || {
        if done_rx.recv_timeout(timeout).is_err() {
            crate::services::task_loop::kill_process_group(pid);
        }
    });
    match child.wait() {
        Ok(status) => {
            let _ = done_tx.send(());
            SpawnResult {
                started: true,
                exit_code: status.code(),
                timed_out: status.code().is_none(),
                enoent: false,
            }
        }
        Err(_) => {
            let _ = done_tx.send(());
            SpawnResult::timed_out()
        }
    }
}

fn spawn_argv_capture(
    argv: &[String],
    cwd: &Path,
    timeout_ms: u64,
) -> Result<String, SpawnErr> {
    let Some(exe) = argv.first() else {
        return Err(SpawnErr {
            enoent: true,
            detail: "empty argv".into(),
        });
    };
    let mut cmd = std::process::Command::new(exe);
    cmd.args(&argv[1..])
        .current_dir(cwd)
        .stdout(std::process::Stdio::piped())
        .stderr(std::process::Stdio::null());
    let mut child = match cmd.spawn() {
        Ok(c) => c,
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => {
            return Err(SpawnErr {
                enoent: true,
                detail: format!("spawn {exe}: {e}"),
            });
        }
        Err(e) => {
            return Err(SpawnErr {
                enoent: true,
                detail: format!("spawn {exe}: {e}"),
            });
        }
    };
    let timeout = std::time::Duration::from_millis(timeout_ms.max(1));
    let start = Instant::now();
    loop {
        match child.try_wait() {
            Ok(Some(status)) => {
                let mut buf = String::new();
                if let Some(mut out) = child.stdout.take() {
                    use std::io::Read;
                    let _ = out.read_to_string(&mut buf);
                }
                if !status.success() && buf.trim().is_empty() {
                    return Err(SpawnErr {
                        enoent: false,
                        detail: format!("kloo tokens exited {}", status.code().unwrap_or(1)),
                    });
                }
                return Ok(buf);
            }
            Ok(None) if start.elapsed() > timeout => {
                let _ = child.kill();
                return Err(SpawnErr {
                    enoent: false,
                    detail: "kloo tokens timed out".into(),
                });
            }
            Ok(None) => std::thread::sleep(std::time::Duration::from_millis(10)),
            Err(e) => {
                return Err(SpawnErr {
                    enoent: false,
                    detail: format!("wait kloo tokens: {e}"),
                });
            }
        }
    }
}

pub fn parse_tokens_json(raw: &str) -> Result<TokensReport, TokensError> {
    let v: serde_json::Value =
        serde_json::from_str(raw.trim()).map_err(|e| TokensError::Parse(e.to_string()))?;
    let fits = v
        .get("fits")
        .and_then(|x| x.as_bool())
        .ok_or_else(|| TokensError::Parse("missing fits".into()))?;
    Ok(TokensReport {
        fits,
        approx_tokens: v.get("approx_tokens").and_then(|x| x.as_u64()),
        usable_window: v.get("usable_window").and_then(|x| x.as_u64()),
        compact_trigger: v.get("compact_trigger").and_then(|x| x.as_u64()),
    })
}

pub fn execute_budget_ms(n: usize) -> u64 {
    let waves = n.div_ceil(4) as u64;
    (waves * VERIFY_TASK_TIMEOUT_MS + 5_000).min(600_000)
}

pub fn warm_budget_ms(n_pairs: usize) -> u64 {
    let n = n_pairs.min(MAX_WARM_PAIRS);
    let waves = n.div_ceil(4) as u64;
    (waves * WARM_TIMEOUT_MS + 5_000).min(245_000)
}

pub struct CheckPlanOpts<'a> {
    pub run: Option<&'a TaskRunFile>,
    pub execute: bool,
    pub tokens: bool,
    pub host: Option<&'a dyn PlanCheckHost>,
    pub only_phase: Option<&'a str>,
    pub execute_budget_ms: Option<u64>,
    pub warm_budget_ms: Option<u64>,
    pub skip_execute: bool,
    pub previous_skipped: &'a [String],
}

impl<'a> CheckPlanOpts<'a> {
    pub fn shape_only(run: Option<&'a TaskRunFile>) -> Self {
        Self {
            run,
            execute: false,
            tokens: false,
            host: None,
            only_phase: None,
            execute_budget_ms: None,
            warm_budget_ms: None,
            skip_execute: false,
            previous_skipped: &[],
        }
    }

    pub fn full(run: Option<&'a TaskRunFile>) -> Self {
        Self {
            run,
            execute: true,
            tokens: true,
            host: None,
            only_phase: None,
            execute_budget_ms: None,
            warm_budget_ms: None,
            skip_execute: false,
            previous_skipped: &[],
        }
    }
}

fn item(task_id: Option<&str>, rule: &str, detail: impl Into<String>) -> PlanCheckItem {
    PlanCheckItem {
        task_id: task_id.map(str::to_string),
        rule: rule.to_string(),
        detail: detail.into(),
        blocking: rule != RULE_VERIFY_TIMEOUT && rule != RULE_VERIFY_NOT_EXECUTED,
    }
}

pub fn save_plan_check(path: &Path, report: &PlanCheckReport) -> Result<(), String> {
    let bytes = serde_json::to_vec_pretty(report)
        .map_err(|e| format!("serialize plan-check: {e}"))?;
    atomic_fs::write_atomic(path, &bytes)
}

pub fn load_plan_check(path: &Path) -> Result<PlanCheckReport, String> {
    let raw = std::fs::read(path).map_err(|e| format!("read {}: {e}", path.display()))?;
    serde_json::from_slice(&raw).map_err(|e| format!("parse {}: {e}", path.display()))
}

/// Walk `plan_path/phases/*/tasks/*` and emit static shape items. No spawn.
pub fn check_plan(
    plan_path: &Path,
    workspace_path: &Path,
    run: Option<&TaskRunFile>,
) -> PlanCheckReport {
    check_plan_with(plan_path, workspace_path, &CheckPlanOpts::shape_only(run))
}

pub fn check_plan_with(
    plan_path: &Path,
    workspace_path: &Path,
    opts: &CheckPlanOpts<'_>,
) -> PlanCheckReport {
    let started = Instant::now();
    let mut report = PlanCheckReport::empty();
    let phases_dir = plan_path.join("phases");
    let phase_dirs = list_sorted_dirs(&phases_dir);
    let mut total_tasks = 0usize;
    let real = RealPlanCheckHost;
    let host: &dyn PlanCheckHost = opts.host.unwrap_or(&real);
    let mut loaded_for_tokens: Vec<(String, PathBuf, Option<TaskSpec>)> = Vec::new();

    for phase_dir in &phase_dirs {
        let phase_id = dir_name(phase_dir);
        if let Some(only) = opts.only_phase {
            if only != phase_id {
                continue;
            }
        }
        let loaded = load_phase_for_check(phase_dir, &mut report);
        total_tasks += loaded.len();
        if loaded.len() > MAX_TASKS_PER_PHASE {
            report.items.push(item(
                None,
                RULE_TASK_COUNT_PHASE,
                format!(
                    "phase {phase_id} has {} tasks (max {MAX_TASKS_PER_PHASE})",
                    loaded.len()
                ),
            ));
        }
        check_phase(
            &phase_id,
            &loaded,
            workspace_path,
            opts.run,
            &mut report,
        );
        let before_e = report.verify_executed;
        let before_s = report.verify_skipped;
        if opts.execute {
            execute_phase(
                &phase_id,
                &loaded,
                workspace_path,
                host,
                opts,
                &mut report,
            );
        }
        report.phases.push(PhaseVerifyCounts {
            phase_id: phase_id.clone(),
            verify_executed: report.verify_executed - before_e,
            verify_skipped: report.verify_skipped - before_s,
        });
        if opts.tokens {
            for task in &loaded {
                loaded_for_tokens.push((task.id.clone(), task.dir.clone(), task.spec.clone()));
            }
        }
    }
    if total_tasks > MAX_TASKS_TOTAL {
        report.items.push(item(
            None,
            RULE_TASK_COUNT_TOTAL,
            format!("plan has {total_tasks} tasks (max {MAX_TASKS_TOTAL})"),
        ));
    }
    if total_tasks == 0 {
        report.items.push(item(
            None,
            RULE_EMPTY_PLAN,
            "plan has no task.yml dirs under phases/*/tasks",
        ));
    }
    if opts.tokens {
        check_tokens(plan_path, &loaded_for_tokens, host, &mut report);
    }
    report.tasks_checked = total_tasks;
    report.finish(started.elapsed().as_millis() as u64);
    report
}

struct LoadedTask {
    id: String,
    dir: PathBuf,
    spec: Option<TaskSpec>,
}

fn load_phase_for_check(phase_dir: &Path, report: &mut PlanCheckReport) -> Vec<LoadedTask> {
    let tasks_dir = phase_dir.join("tasks");
    let mut out = Vec::new();
    for dir in list_sorted_dirs(&tasks_dir) {
        let id = dir_name(&dir);
        let prompt = dir.join("prompt.md");
        let prompt_body = std::fs::read_to_string(&prompt).unwrap_or_default();
        if !prompt.is_file() || prompt_body.trim().is_empty() {
            report.items.push(item(
                Some(&id),
                RULE_EMPTY_PROMPT,
                format!("missing or empty prompt.md for {id}"),
            ));
        }
        let yml = dir.join("task.yml");
        if !yml.is_file() {
            // check_plan only runs on local-small plans (D4). Every task dir
            // must have a task.yml; prompt-only is missing_files, not a skip.
            report.items.push(item(
                Some(&id),
                RULE_MISSING_FILES,
                format!("missing task.yml for {id}"),
            ));
            out.push(LoadedTask {
                id: id.clone(),
                dir,
                spec: None,
            });
            continue;
        }
        match load_task_spec(&dir) {
            Ok(spec) => out.push(LoadedTask {
                id: spec.id.clone(),
                dir,
                spec: Some(spec),
            }),
            Err(e) => {
                report.items.push(map_spec_error(&id, &e));
                out.push(LoadedTask {
                    id,
                    dir,
                    spec: None,
                });
            }
        }
    }
    out
}

fn map_spec_error(id: &str, err: &str) -> PlanCheckItem {
    if err.contains("empty verify") {
        item(Some(id), RULE_EMPTY_VERIFY, err)
    } else {
        item(Some(id), RULE_MISSING_FILES, err)
    }
}

fn check_phase(
    phase_id: &str,
    loaded: &[LoadedTask],
    workspace: &Path,
    run: Option<&TaskRunFile>,
    report: &mut PlanCheckReport,
) {
    let specs: Vec<TaskSpec> = loaded.iter().filter_map(|t| t.spec.clone()).collect();
    let by_id: HashMap<String, TaskSpec> =
        specs.iter().cloned().map(|s| (s.id.clone(), s)).collect();

    check_dag(&specs, report);
    check_collisions(phase_id, &specs, run, report);

    for task in loaded {
        let Some(spec) = &task.spec else {
            continue;
        };
        check_files(spec, &by_id, workspace, report);
        check_cwd(spec, &by_id, workspace, report);
        check_verify_rules(spec, &by_id, workspace, report);
        check_must_contain(spec, report);
        check_ui_forbidden(spec, report);
    }
}

fn check_dag(specs: &[TaskSpec], report: &mut PlanCheckReport) {
    let ids: HashSet<&str> = specs.iter().map(|s| s.id.as_str()).collect();
    for spec in specs {
        for dep in &spec.depends_on {
            if dep == &spec.id {
                report.items.push(item(
                    Some(&spec.id),
                    RULE_DEPENDS_SELF,
                    format!("{} depends_on itself", spec.id),
                ));
            } else if !ids.contains(dep.as_str()) {
                report.items.push(item(
                    Some(&spec.id),
                    RULE_DEPENDS_UNRESOLVED,
                    format!("{} depends_on unknown id {dep}", spec.id),
                ));
            } else if dep.as_str() > spec.id.as_str() {
                report.items.push(item(
                    Some(&spec.id),
                    RULE_DEPENDS_FORWARD,
                    format!("{} depends_on later sibling {dep}", spec.id),
                ));
            }
        }
    }
    // Drop unknown/self edges so topo_sort can still see a cycle in the
    // resolvable subgraph (it otherwise returns the first UnknownDep/SelfDep).
    let resolvable: Vec<TaskSpec> = specs
        .iter()
        .cloned()
        .map(|mut s| {
            s.depends_on
                .retain(|d| d != &s.id && ids.contains(d.as_str()));
            s
        })
        .collect();
    if let Err(DagError::Cycle { nodes }) = task_spec::topo_sort(&resolvable) {
        let tid = nodes.first().map(String::as_str);
        report.items.push(item(
            tid,
            RULE_DEPENDS_CYCLE,
            format!("cycle in depends_on: {}", nodes.join(",")),
        ));
    }
}

fn check_collisions(
    phase_id: &str,
    specs: &[TaskSpec],
    run: Option<&TaskRunFile>,
    report: &mut PlanCheckReport,
) {
    let mut claimed: HashMap<String, String> = HashMap::new();
    for spec in specs {
        if is_done(run, phase_id, &spec.id) {
            continue;
        }
        for path in &spec.files {
            let key = normalize_rel(path);
            if let Some(other) = claimed.get(&key) {
                if other == &spec.id {
                    continue;
                }
                report.items.push(item(
                    Some(&spec.id),
                    RULE_FILE_COLLISION,
                    format!("{other} and {} both claim {path}", spec.id),
                ));
            } else {
                claimed.insert(key, spec.id.clone());
            }
        }
    }
}

fn is_done(run: Option<&TaskRunFile>, phase_id: &str, task_id: &str) -> bool {
    let Some(run) = run else {
        return false;
    };
    if !run.phase_id.is_empty() && run.phase_id != phase_id {
        return false;
    }
    run.tasks
        .iter()
        .find(|t| t.id == task_id)
        .map(|t| t.status == "done")
        .unwrap_or(false)
}

fn check_files(
    spec: &TaskSpec,
    by_id: &HashMap<String, TaskSpec>,
    workspace: &Path,
    report: &mut PlanCheckReport,
) {
    for path in &spec.files {
        if workspace.join(path).is_file() {
            continue;
        }
        if path_created(path, spec, by_id) {
            continue;
        }
        report.items.push(item(
            Some(&spec.id),
            RULE_FILE_MISSING,
            format!("{path} is not on disk and is not created by {} or an ancestor", spec.id),
        ));
    }
}

fn check_cwd(
    spec: &TaskSpec,
    by_id: &HashMap<String, TaskSpec>,
    workspace: &Path,
    report: &mut PlanCheckReport,
) {
    let Some(cwd) = spec.cwd.as_deref().map(str::trim).filter(|c| !c.is_empty()) else {
        return;
    };
    for path in spec.files.iter().chain(spec.new.iter().flatten()) {
        if !path_under_cwd(path, cwd) {
            report.items.push(item(
                Some(&spec.id),
                RULE_FILES_OUTSIDE_CWD,
                format!("{path} is not under cwd {cwd}"),
            ));
        }
    }
    if let Err(e) = verify_policy::effective_verify_dir(workspace, Some(cwd)) {
        // Do not propagate e.rule — effective_verify_dir uses the argv
        // constructor (verify_not_allowlisted). A jail/escape cwd is a
        // cwd-field fault; the planner/Checks API for that is this id.
        report.items.push(item(
            Some(&spec.id),
            RULE_VERIFY_CWD_MISSING,
            e.detail,
        ));
        return;
    }
    if workspace.join(cwd).is_dir() {
        return;
    }
    if cwd_created_by(cwd, spec, by_id) {
        return;
    }
    report.items.push(item(
        Some(&spec.id),
        RULE_VERIFY_CWD_MISSING,
        format!("cwd {cwd} does not exist and is not created by {} or an ancestor", spec.id),
    ));
}

fn check_verify_rules(
    spec: &TaskSpec,
    by_id: &HashMap<String, TaskSpec>,
    workspace: &Path,
    report: &mut PlanCheckReport,
) {
    let argv = match check_verify(&spec.verify) {
        Ok(argv) => argv,
        Err(e) => {
            report.items.push(item(Some(&spec.id), &e.rule, e.detail));
            return;
        }
    };
    let cwd = spec.cwd.as_deref().map(str::trim).filter(|c| !c.is_empty());
    let effective = match verify_policy::effective_verify_dir(workspace, cwd) {
        Ok(p) => p,
        Err(_) => return,
    };
    for token in &argv {
        if !is_verify_file_target(token) {
            continue;
        }
        let resolved = effective.join(token);
        let listed = match cwd {
            Some(c) => {
                let c = normalize_rel(c);
                let t = normalize_rel(token);
                if c.is_empty() {
                    t
                } else {
                    format!("{c}/{t}")
                }
            }
            None => normalize_rel(token),
        };
        if resolved.is_file() || path_claimed(&listed, spec, by_id) {
            continue;
        }
        report.items.push(item(
            Some(&spec.id),
            RULE_VERIFY_TARGET_MISSING,
            format!(
                "verify target {token} is missing under {} (looked for {})",
                cwd.unwrap_or("."),
                resolved.display()
            ),
        ));
    }
}

fn check_must_contain(spec: &TaskSpec, report: &mut PlanCheckReport) {
    if spec.must_contain.is_empty() {
        report.items.push(item(
            Some(&spec.id),
            RULE_MUST_CONTAIN_TRIVIAL,
            format!("{} has an empty must_contain list", spec.id),
        ));
        return;
    }
    for needle in &spec.must_contain {
        let trimmed = needle.trim();
        let lower = trimmed.to_ascii_lowercase();
        if trimmed.chars().count() < 4 || TRIVIAL_NEEDLES.contains(&lower.as_str()) {
            report.items.push(item(
                Some(&spec.id),
                RULE_MUST_CONTAIN_TRIVIAL,
                format!("{} must_contain {needle:?} is trivial", spec.id),
            ));
        }
    }
}

fn check_ui_forbidden(spec: &TaskSpec, report: &mut PlanCheckReport) {
    if spec.mock.is_some() {
        report.items.push(item(
            Some(&spec.id),
            RULE_UI_TASK_FORBIDDEN,
            format!("{} sets mock: (UI tasks are forbidden in small mode)", spec.id),
        ));
        return;
    }
    for path in &spec.files {
        let lower = path.to_ascii_lowercase();
        if lower.ends_with(".html") || lower.ends_with(".scss") || lower.ends_with(".css") {
            report.items.push(item(
                Some(&spec.id),
                RULE_UI_TASK_FORBIDDEN,
                format!("{path} is a UI file (html/scss/css)"),
            ));
            return;
        }
    }
}

fn execute_phase(
    _phase_id: &str,
    loaded: &[LoadedTask],
    workspace: &Path,
    host: &dyn PlanCheckHost,
    opts: &CheckPlanOpts<'_>,
    report: &mut PlanCheckReport,
) {
    let specs: Vec<TaskSpec> = loaded.iter().filter_map(|t| t.spec.clone()).collect();
    let by_id: HashMap<String, TaskSpec> =
        specs.iter().cloned().map(|s| (s.id.clone(), s)).collect();

    let mut eligible: Vec<TaskSpec> = Vec::new();
    for spec in &specs {
        let Ok(argv) = check_verify(&spec.verify) else {
            continue;
        };
        let runner = argv.first().cloned().unwrap_or_default();
        if pair_is_deferred(spec, &by_id, workspace, &runner) {
            continue;
        }
        eligible.push(spec.clone());
    }
    eligible.sort_by(|a, b| {
        let a_skip = opts.previous_skipped.iter().any(|id| id == &a.id);
        let b_skip = opts.previous_skipped.iter().any(|id| id == &b.id);
        b_skip.cmp(&a_skip).then_with(|| a.id.cmp(&b.id))
    });

    if opts.skip_execute {
        for spec in &eligible {
            report.items.push(item(
                Some(&spec.id),
                RULE_VERIFY_NOT_EXECUTED,
                format!("{} verify never started (stage budget)", spec.id),
            ));
            report.verify_skipped += 1;
            report.skipped_ids.push(spec.id.clone());
        }
        return;
    }

    // Warm distinct (runner, effective_cwd).
    let mut pair_keys: Vec<(String, PathBuf)> = Vec::new();
    let mut pair_tasks: HashMap<(String, String), Vec<String>> = HashMap::new();
    let mut pair_argv: HashMap<(String, String), Vec<String>> = HashMap::new();
    for spec in &eligible {
        let argv = check_verify(&spec.verify).expect("eligible passed check_verify");
        let runner = argv[0].clone();
        let cwd_rel = spec.cwd.as_deref().map(str::trim).filter(|c| !c.is_empty());
        let cwd = match verify_policy::effective_verify_dir(workspace, cwd_rel) {
            Ok(p) => p,
            Err(_) => workspace.to_path_buf(),
        };
        let key = (runner.clone(), cwd.to_string_lossy().into_owned());
        pair_tasks.entry(key.clone()).or_default().push(spec.id.clone());
        pair_argv
            .entry(key.clone())
            .or_insert_with(|| warm_argv_for(&runner, &argv));
        if !pair_keys.iter().any(|(r, c)| r == &runner && c == &cwd) {
            pair_keys.push((runner, cwd));
        }
    }
    pair_keys.sort_by(|a, b| a.0.cmp(&b.0).then(a.1.cmp(&b.1)));

    let warm_n = pair_keys.len().min(MAX_WARM_PAIRS);
    let w_budget = opts.warm_budget_ms.unwrap_or(warm_budget_ms(pair_keys.len()));
    let warm_started = Instant::now();
    let mut failed_pairs: HashSet<(String, String)> = HashSet::new();
    if warm_n > 0 && !opts.skip_execute {
        let to_warm: Vec<(String, PathBuf)> = pair_keys.iter().take(warm_n).cloned().collect();
        let results = run_waves(
            &to_warm,
            4,
            w_budget,
            WARM_TIMEOUT_MS,
            |pair, started| {
                if !started {
                    return SpawnResult::never_started();
                }
                let key = (pair.0.clone(), pair.1.to_string_lossy().into_owned());
                let argv = pair_argv.get(&key).cloned().unwrap_or_else(|| {
                    warm_argv_for(&pair.0, &[pair.0.clone()])
                });
                host.spawn(&SpawnRequest {
                    argv,
                    cwd: pair.1.clone(),
                    timeout_ms: WARM_TIMEOUT_MS,
                    kind: SpawnKind::Warm,
                    task_id: None,
                })
            },
        );
        for ((runner, cwd), res) in results {
            if res.enoent || res.exit_code.map(|c| c != 0).unwrap_or(false) {
                let key = (runner.clone(), cwd.to_string_lossy().into_owned());
                failed_pairs.insert(key.clone());
                if let Some(ids) = pair_tasks.get(&key) {
                    for id in ids {
                        report.items.push(item(
                            Some(id),
                            RULE_VERIFY_NOT_RUNNABLE,
                            format!(
                                "warm {} in {} failed (exit {:?})",
                                runner,
                                cwd.display(),
                                res.exit_code
                            ),
                        ));
                    }
                }
            }
        }
    }
    report.warm_ms = report.warm_ms.saturating_add(warm_started.elapsed().as_millis() as u64);

    // Per-task execute. Failed-warm tasks still count as executed (warm is the signal).
    let e_budget = opts
        .execute_budget_ms
        .unwrap_or(execute_budget_ms(eligible.len()));
    let exec_started = Instant::now();
    let to_run: Vec<TaskSpec> = eligible
        .iter()
        .filter(|spec| {
            let argv = check_verify(&spec.verify).unwrap();
            let runner = argv[0].clone();
            let cwd_rel = spec.cwd.as_deref().map(str::trim).filter(|c| !c.is_empty());
            let cwd = verify_policy::effective_verify_dir(workspace, cwd_rel)
                .unwrap_or_else(|_| workspace.to_path_buf());
            !failed_pairs.contains(&(runner, cwd.to_string_lossy().into_owned()))
        })
        .cloned()
        .collect();
    let already = eligible.len() - to_run.len();
    report.verify_executed += already;

    let results = run_waves(
        &to_run,
        4,
        e_budget,
        VERIFY_TASK_TIMEOUT_MS,
        |spec, started| {
            if !started {
                return SpawnResult::never_started();
            }
            let argv = check_verify(&spec.verify).unwrap();
            let cwd_rel = spec.cwd.as_deref().map(str::trim).filter(|c| !c.is_empty());
            let cwd = verify_policy::effective_verify_dir(workspace, cwd_rel)
                .unwrap_or_else(|_| workspace.to_path_buf());
            host.spawn(&SpawnRequest {
                argv,
                cwd,
                timeout_ms: VERIFY_TASK_TIMEOUT_MS,
                kind: SpawnKind::Task,
                task_id: Some(spec.id.clone()),
            })
        },
    );
    for (spec, res) in results {
        if !res.started && !res.enoent {
            report.items.push(item(
                Some(&spec.id),
                RULE_VERIFY_NOT_EXECUTED,
                format!("{} verify never started (execute budget)", spec.id),
            ));
            report.verify_skipped += 1;
            report.skipped_ids.push(spec.id.clone());
            continue;
        }
        report.verify_executed += 1;
        if res.timed_out {
            report.items.push(item(
                Some(&spec.id),
                RULE_VERIFY_TIMEOUT,
                format!("{} verify exceeded {VERIFY_TASK_TIMEOUT_MS}ms", spec.id),
            ));
        } else if res.enoent || res.exit_code == Some(127) {
            report.items.push(item(
                Some(&spec.id),
                RULE_VERIFY_NOT_RUNNABLE,
                format!("{} verify spawn ENOENT/127", spec.id),
            ));
        }
    }
    report.verify_ms = report.verify_ms.saturating_add(exec_started.elapsed().as_millis() as u64);
}

fn run_waves<T: Clone + Send, F>(
    jobs: &[T],
    width: usize,
    budget_ms: u64,
    _timeout_ms: u64,
    work: F,
) -> Vec<(T, SpawnResult)>
where
    F: Fn(&T, bool) -> SpawnResult + Sync,
{
    let start = Instant::now();
    let width = width.max(1);
    let mut out = Vec::with_capacity(jobs.len());
    let mut i = 0;
    while i < jobs.len() {
        if start.elapsed().as_millis() as u64 >= budget_ms {
            for job in &jobs[i..] {
                out.push((job.clone(), SpawnResult::never_started()));
            }
            break;
        }
        let end = (i + width).min(jobs.len());
        // Sequential within the wave for determinism in unit tests; the host
        // may still be concurrent. Width bounds how many we *start* per wave.
        // Tests that assert in-flight use a host that tracks spawn entry.
        let batch: Vec<T> = jobs[i..end].to_vec();
        let handles: Vec<(T, SpawnResult)> = std::thread::scope(|scope| {
            let mut joins = Vec::new();
            for job in &batch {
                let job = job.clone();
                joins.push(scope.spawn(|| {
                    let r = work(&job, true);
                    (job, r)
                }));
            }
            joins.into_iter().map(|j| j.join().unwrap()).collect()
        });
        out.extend(handles);
        i = end;
    }
    out
}

pub fn warm_argv_for(runner: &str, verify_argv: &[String]) -> Vec<String> {
    match runner {
        "cargo" => vec![
            "cargo".into(),
            "metadata".into(),
            "--format-version".into(),
            "1".into(),
            "--offline".into(),
        ],
        "npx" => {
            let tool = if verify_argv.iter().any(|t| t.contains("jest")) {
                "jest"
            } else {
                "vitest"
            };
            vec![
                "npx".into(),
                "--no-install".into(),
                tool.into(),
                "--version".into(),
            ]
        }
        "go" => vec!["go".into(), "list".into(), "-m".into()],
        "python" | "python3" => vec![
            runner.to_string(),
            "-m".into(),
            "pytest".into(),
            "--version".into(),
        ],
        "node" => vec!["node".into(), "-v".into()],
        other => vec![other.to_string(), "--version".into()],
    }
}

fn pair_is_deferred(
    spec: &TaskSpec,
    by_id: &HashMap<String, TaskSpec>,
    workspace: &Path,
    runner: &str,
) -> bool {
    let cwd_rel = spec.cwd.as_deref().map(str::trim).filter(|c| !c.is_empty());
    if let Some(cwd) = cwd_rel {
        if !workspace.join(cwd).is_dir() && cwd_created_by(cwd, spec, by_id) {
            return true;
        }
    }
    if let Some(root) = project_root_file(runner) {
        let listed = match cwd_rel {
            Some(c) => format!("{}/{}", normalize_rel(c), root),
            None => root.to_string(),
        };
        let on_disk = match verify_policy::effective_verify_dir(workspace, cwd_rel) {
            Ok(dir) => dir.join(root).is_file(),
            Err(_) => false,
        };
        if !on_disk && path_claimed(&listed, spec, by_id) {
            return true;
        }
    }
    false
}

fn project_root_file(runner: &str) -> Option<&'static str> {
    match runner {
        "cargo" => Some("Cargo.toml"),
        "go" => Some("go.mod"),
        "npx" | "node" => Some("package.json"),
        _ => None,
    }
}

fn check_tokens(
    _plan_path: &Path,
    loaded: &[(String, PathBuf, Option<TaskSpec>)],
    host: &dyn PlanCheckHost,
    report: &mut PlanCheckReport,
) {
    let mut saw_unavailable = false;
    for (id, dir, spec) in loaded {
        let prompt = dir.join("prompt.md");
        if !prompt.is_file() {
            continue;
        }
        let ctx = spec.as_ref().and_then(|s| s.ctx).unwrap_or(32768);
        match host.tokens(ctx, &prompt) {
            Ok(tok) if !tok.fits => {
                report.items.push(item(
                    Some(id),
                    RULE_PROMPT_EXCEEDS_CONTEXT,
                    format!(
                        "approx_tokens={} usable_window={} compact_trigger={}",
                        tok.approx_tokens.unwrap_or(0),
                        tok.usable_window.unwrap_or(0),
                        tok.compact_trigger.unwrap_or(0)
                    ),
                ));
            }
            Ok(_) => {}
            Err(TokensError::Unavailable(d)) => {
                if !saw_unavailable {
                    report.items.push(item(None, RULE_TOKENS_UNAVAILABLE, d));
                    saw_unavailable = true;
                }
            }
            Err(TokensError::Parse(d)) => {
                if !saw_unavailable {
                    report.items.push(item(
                        None,
                        RULE_TOKENS_UNAVAILABLE,
                        format!("kloo tokens parse: {d}"),
                    ));
                    saw_unavailable = true;
                }
            }
        }
        if saw_unavailable {
            break;
        }
    }
}

fn rel_eq(a: &str, b: &str) -> bool {
    normalize_rel(a) == normalize_rel(b)
}

/// Workspace-relative path claimed on this task (`files[]` or `new:`) or an
/// ancestor `new:`. Used by verify_target_missing (prompt 01-04).
fn path_claimed(path: &str, spec: &TaskSpec, by_id: &HashMap<String, TaskSpec>) -> bool {
    if spec.files.iter().any(|f| rel_eq(f, path)) {
        return true;
    }
    path_created(path, spec, by_id)
}

fn path_created(path: &str, spec: &TaskSpec, by_id: &HashMap<String, TaskSpec>) -> bool {
    if spec
        .new
        .as_ref()
        .map(|n| n.iter().any(|p| rel_eq(p, path)))
        .unwrap_or(false)
    {
        return true;
    }
    let mut stack = spec.depends_on.clone();
    let mut seen = HashSet::new();
    while let Some(id) = stack.pop() {
        if !seen.insert(id.clone()) {
            continue;
        }
        let Some(anc) = by_id.get(&id) else {
            continue;
        };
        if anc
            .new
            .as_ref()
            .map(|n| n.iter().any(|p| rel_eq(p, path)))
            .unwrap_or(false)
        {
            return true;
        }
        stack.extend(anc.depends_on.iter().cloned());
    }
    false
}

fn cwd_created_by(cwd: &str, spec: &TaskSpec, by_id: &HashMap<String, TaskSpec>) -> bool {
    let mut paths: Vec<String> = spec.new.iter().flatten().cloned().collect();
    let mut stack = spec.depends_on.clone();
    let mut seen = HashSet::new();
    while let Some(id) = stack.pop() {
        if !seen.insert(id.clone()) {
            continue;
        }
        let Some(anc) = by_id.get(&id) else {
            continue;
        };
        paths.extend(anc.new.iter().flatten().cloned());
        stack.extend(anc.depends_on.iter().cloned());
    }
    paths.iter().any(|p| path_under_cwd(p, cwd))
}

fn normalize_rel(path: &str) -> String {
    let mut parts = Vec::new();
    for c in Path::new(path.trim()).components() {
        match c {
            Component::CurDir => {}
            Component::Normal(s) => parts.push(s.to_string_lossy().into_owned()),
            Component::ParentDir => parts.push("..".to_string()),
            Component::Prefix(_) | Component::RootDir => {}
        }
    }
    parts.join("/")
}

fn path_under_cwd(path: &str, cwd: &str) -> bool {
    let path = normalize_rel(path);
    let cwd = normalize_rel(cwd);
    if cwd.is_empty() {
        return true;
    }
    path == cwd || path.starts_with(&format!("{cwd}/"))
}

fn is_verify_file_target(token: &str) -> bool {
    if token.starts_with('-') {
        return false;
    }
    let t = token.to_ascii_lowercase();
    t.contains(".spec.")
        || t.contains(".test.")
        || t.ends_with(".rs")
        || t.ends_with(".ts")
        || t.ends_with(".js")
        || t.ends_with(".py")
}

fn list_sorted_dirs(path: &Path) -> Vec<PathBuf> {
    let Ok(rd) = std::fs::read_dir(path) else {
        return Vec::new();
    };
    let mut dirs: Vec<_> = rd
        .filter_map(|e| e.ok())
        .map(|e| e.path())
        .filter(|p| p.is_dir())
        .collect();
    dirs.sort();
    dirs
}

fn dir_name(path: &Path) -> String {
    path.file_name()
        .and_then(|n| n.to_str())
        .unwrap_or("")
        .to_string()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::services::task_state::{empty_run, TaskRow};
    use std::sync::atomic::{AtomicU64, Ordering};

    static SEQ: AtomicU64 = AtomicU64::new(0);

    fn tmp(tag: &str) -> PathBuf {
        let d = std::env::temp_dir().join(format!(
            "j1-pcheck-{}-{}-{}",
            tag,
            std::process::id(),
            SEQ.fetch_add(1, Ordering::Relaxed)
        ));
        std::fs::create_dir_all(&d).unwrap();
        d
    }

    fn write_task(plan: &Path, phase: &str, id: &str, yml: &str, prompt: &str) {
        let dir = plan.join("phases").join(phase).join("tasks").join(id);
        std::fs::create_dir_all(&dir).unwrap();
        std::fs::write(dir.join("task.yml"), yml).unwrap();
        std::fs::write(dir.join("prompt.md"), prompt).unwrap();
    }

    fn rust_yml(id: &str, files: &str, verify: &str, deps: &str, extra: &str) -> String {
        format!(
            "id: {id}\nfiles: [{files}]\nverify: \"{verify}\"\nmust_contain: [\"pub fn add\"]\ndepends_on: [{deps}]\n{extra}"
        )
    }

    fn has_rule(report: &PlanCheckReport, rule: &str) -> bool {
        report.items.iter().any(|i| i.rule == rule)
    }

    fn rules_of(report: &PlanCheckReport) -> Vec<&str> {
        report.items.iter().map(|i| i.rule.as_str()).collect()
    }

    #[test]
    fn save_load_round_trip_and_passed_semantics() {
        let dir = tmp("persist");
        let path = dir.join("plan-check.json");
        let mut report = PlanCheckReport::empty();
        report.items.push(item(None, RULE_VERIFY_TIMEOUT, "slow"));
        report.finish(12);
        assert!(report.passed, "advisory-only must pass");
        save_plan_check(&path, &report).unwrap();
        assert!(!dir.join("plan-check.json.j1tmp").exists());
        let loaded = load_plan_check(&path).unwrap();
        assert_eq!(loaded, report);
        report.items.push(item(Some("01-a"), RULE_MISSING_FILES, "x"));
        report.finish(12);
        assert!(!report.passed);
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn empty_report_defaults_are_zero() {
        let r = PlanCheckReport::empty();
        assert_eq!(r.verify_executed, 0);
        assert_eq!(r.verify_skipped, 0);
        assert_eq!(r.warm_ms, 0);
        assert!(r.passed);
        assert!(r.items.is_empty());
    }

    #[test]
    fn rule_constants_match_overview() {
        let pairs = [
            (RULE_MISSING_FILES, "missing_files"),
            (RULE_EMPTY_VERIFY, "empty_verify"),
            (RULE_EMPTY_PROMPT, "empty_prompt"),
            (RULE_FILE_MISSING, "file_missing"),
            (RULE_FILE_COLLISION, "file_collision"),
            (RULE_DEPENDS_UNRESOLVED, "depends_unresolved"),
            (RULE_DEPENDS_CYCLE, "depends_cycle"),
            (RULE_DEPENDS_FORWARD, "depends_forward"),
            (RULE_DEPENDS_SELF, "depends_self"),
            (RULE_VERIFY_NOT_ALLOWLISTED, "verify_not_allowlisted"),
            (RULE_VERIFY_NOT_SCOPED, "verify_not_scoped"),
            (RULE_VERIFY_TARGET_MISSING, "verify_target_missing"),
            (RULE_VERIFY_CWD_MISSING, "verify_cwd_missing"),
            (RULE_FILES_OUTSIDE_CWD, "files_outside_cwd"),
            (RULE_VERIFY_NOT_RUNNABLE, "verify_not_runnable"),
            (RULE_MUST_CONTAIN_TRIVIAL, "must_contain_trivial"),
            (RULE_VERIFY_TIMEOUT, "verify_timeout"),
            (RULE_VERIFY_NOT_EXECUTED, "verify_not_executed"),
            (RULE_PROMPT_EXCEEDS_CONTEXT, "prompt_exceeds_context"),
            (RULE_TOKENS_UNAVAILABLE, "tokens_unavailable"),
            (RULE_TASK_COUNT_PHASE, "task_count_phase"),
            (RULE_TASK_COUNT_TOTAL, "task_count_total"),
            (RULE_UI_TASK_FORBIDDEN, "ui_task_forbidden"),
            (RULE_EMPTY_PLAN, "empty_plan"),
        ];
        for (c, s) in pairs {
            assert_eq!(c, s);
        }
        assert_ne!(RULE_VERIFY_CWD_MISSING, "verify_runner_root_missing");
        assert!(
            ![
                RULE_MISSING_FILES,
                RULE_VERIFY_CWD_MISSING,
                RULE_FILES_OUTSIDE_CWD,
                RULE_VERIFY_NOT_ALLOWLISTED,
            ]
            .contains(&"verify_runner_root_missing")
        );
    }

    fn happy_plan() -> (PathBuf, PathBuf) {
        let root = tmp("happy");
        let plan = root.join("plan");
        let ws = root.join("ws");
        std::fs::create_dir_all(ws.join("src")).unwrap();
        std::fs::write(ws.join("src/add.rs"), "pub fn add() {}\n").unwrap();
        std::fs::write(ws.join("src/sub.rs"), "pub fn sub() {}\n").unwrap();
        write_task(
            &plan,
            "00-calc",
            "01-add",
            &rust_yml(
                "01-add",
                "src/add.rs",
                "cargo test add -- --exact",
                "",
                "",
            ),
            "implement add",
        );
        write_task(
            &plan,
            "00-calc",
            "02-sub",
            &rust_yml(
                "02-sub",
                "src/sub.rs",
                "cargo test sub -- --exact",
                "01-add",
                "",
            ),
            "implement sub",
        );
        (plan, ws)
    }

    #[test]
    fn clean_plan_has_zero_items() {
        let (plan, ws) = happy_plan();
        let report = check_plan(&plan, &ws, None);
        assert!(report.passed, "{:?}", report.items);
        assert!(report.items.is_empty(), "{:?}", report.items);
        assert_eq!(report.tasks_checked, 2);
        let _ = std::fs::remove_dir_all(plan.parent().unwrap());
    }

    #[test]
    fn prompt_without_task_yml_is_missing_files() {
        let root = tmp("no-yml");
        let plan = root.join("plan");
        let ws = root.join("ws");
        std::fs::create_dir_all(&ws).unwrap();
        let dir = plan.join("phases/00-x/tasks/01-a");
        std::fs::create_dir_all(&dir).unwrap();
        std::fs::write(dir.join("prompt.md"), "implement add\n").unwrap();
        let report = check_plan(&plan, &ws, None);
        assert!(has_rule(&report, RULE_MISSING_FILES), "{:?}", report.items);
        assert!(!report.passed);
        assert_eq!(report.tasks_checked, 1);
        let _ = std::fs::remove_dir_all(&root);
    }

    #[test]
    fn missing_prompt_and_file_and_ancestor_new() {
        let root = tmp("files");
        let plan = root.join("plan");
        let ws = root.join("ws");
        std::fs::create_dir_all(&ws).unwrap();
        write_task(
            &plan,
            "00-x",
            "01-a",
            &rust_yml("01-a", "src/a.rs", "cargo test a -- --exact", "", "new: [src/a.rs]\n"),
            "   \n",
        );
        write_task(
            &plan,
            "00-x",
            "02-b",
            &rust_yml("02-b", "src/missing.rs", "cargo test b -- --exact", "", ""),
            "ok prompt",
        );
        write_task(
            &plan,
            "00-x",
            "03-c",
            &rust_yml(
                "03-c",
                "src/a.rs",
                "cargo test c -- --exact",
                "01-a",
                "",
            ),
            "uses ancestor new",
        );
        let report = check_plan(&plan, &ws, None);
        assert!(has_rule(&report, RULE_EMPTY_PROMPT), "{:?}", report.items);
        assert!(has_rule(&report, RULE_FILE_MISSING), "{:?}", report.items);
        let c_items: Vec<_> = report
            .items
            .iter()
            .filter(|i| i.task_id.as_deref() == Some("03-c"))
            .collect();
        assert!(
            !c_items.iter().any(|i| i.rule == RULE_FILE_MISSING),
            "ancestor new: should satisfy 03-c: {c_items:?}"
        );
        let _ = std::fs::remove_dir_all(&root);
    }

    #[test]
    fn file_collision_and_d8_done_exclusion() {
        let root = tmp("collide");
        let plan = root.join("plan");
        let ws = root.join("ws");
        std::fs::create_dir_all(ws.join("src")).unwrap();
        std::fs::write(ws.join("src/a.rs"), "x\n").unwrap();
        write_task(
            &plan,
            "00-x",
            "01-a",
            &rust_yml("01-a", "src/a.rs", "cargo test a -- --exact", "", ""),
            "a",
        );
        write_task(
            &plan,
            "00-x",
            "02-b",
            &rust_yml("02-b", "src/a.rs", "cargo test b -- --exact", "", ""),
            "b",
        );
        let report = check_plan(&plan, &ws, None);
        assert!(has_rule(&report, RULE_FILE_COLLISION), "{:?}", report.items);

        let mut run = empty_run("p", "00-x");
        run.tasks = vec![
            TaskRow {
                id: "01-a".into(),
                status: "done".into(),
                depends_on: vec![],
                attempts: vec![],
                succeeded_tier: None,
                commit_sha: Some("sha".into()),
                blocked_by: None,
                route: None,
            },
            TaskRow {
                id: "02-b".into(),
                status: "pending".into(),
                depends_on: vec![],
                attempts: vec![],
                succeeded_tier: None,
                commit_sha: None,
                blocked_by: None,
                route: None,
            },
        ];
        let preflight = check_plan(&plan, &ws, Some(&run));
        assert!(
            !has_rule(&preflight, RULE_FILE_COLLISION),
            "D8: done path is not a collision: {:?}",
            preflight.items
        );
        let _ = std::fs::remove_dir_all(&root);
    }

    #[test]
    fn file_collision_normalizes_dot_slash_and_skips_self_dup() {
        let root = tmp("collide-norm");
        let plan = root.join("plan");
        let ws = root.join("ws");
        std::fs::create_dir_all(ws.join("src")).unwrap();
        std::fs::write(ws.join("src/a.rs"), "x\n").unwrap();
        write_task(
            &plan,
            "00-y",
            "01-a",
            &rust_yml("01-a", "src/a.rs", "cargo test a -- --exact", "", ""),
            "a",
        );
        write_task(
            &plan,
            "00-y",
            "02-b",
            &rust_yml("02-b", "./src/a.rs", "cargo test b -- --exact", "", ""),
            "b dotted",
        );
        let report = check_plan(&plan, &ws, None);
        assert!(
            report.items.iter().any(|i| {
                i.rule == RULE_FILE_COLLISION
                    && i.detail.contains("01-a")
                    && i.detail.contains("02-b")
            }),
            "normalized collision ./src/a.rs vs src/a.rs: {:?}",
            report.items
        );

        let root2 = tmp("collide-self");
        let plan2 = root2.join("plan");
        let ws2 = root2.join("ws");
        std::fs::create_dir_all(ws2.join("src")).unwrap();
        std::fs::write(ws2.join("src/a.rs"), "x\n").unwrap();
        write_task(
            &plan2,
            "00-z",
            "01-dup",
            &rust_yml("01-dup", "src/a.rs, src/a.rs", "cargo test a -- --exact", "", ""),
            "self dup",
        );
        let report = check_plan(&plan2, &ws2, None);
        assert!(
            !has_rule(&report, RULE_FILE_COLLISION),
            "same-task duplicate files[] is not a two-task collision: {:?}",
            report.items
        );
        let _ = std::fs::remove_dir_all(&root);
        let _ = std::fs::remove_dir_all(&root2);
    }

    #[test]
    fn files_outside_cwd_rules() {
        let root = tmp("cwd-files");
        let plan = root.join("plan");
        let ws = root.join("ws");
        std::fs::create_dir_all(ws.join("web/src/app")).unwrap();
        std::fs::create_dir_all(ws.join("desktop/src-tauri/src")).unwrap();
        std::fs::write(ws.join("web/src/app/x.ts"), "x\n").unwrap();
        std::fs::write(ws.join("desktop/src-tauri/src/lib.rs"), "x\n").unwrap();
        std::fs::write(ws.join("root.rs"), "x\n").unwrap();
        write_task(
            &plan,
            "00-x",
            "01-ok",
            &rust_yml(
                "01-ok",
                "web/src/app/x.ts",
                "cargo test ok -- --exact",
                "",
                "cwd: web\n",
            ),
            "ok",
        );
        let report = check_plan(&plan, &ws, None);
        assert!(
            !has_rule(&report, RULE_FILES_OUTSIDE_CWD),
            "{:?}",
            report.items
        );

        write_task(
            &plan,
            "00-x",
            "02-bad",
            &rust_yml(
                "02-bad",
                "desktop/src-tauri/src/lib.rs",
                "cargo test bad -- --exact",
                "",
                "cwd: web\n",
            ),
            "bad",
        );
        let report = check_plan(&plan, &ws, None);
        assert!(has_rule(&report, RULE_FILES_OUTSIDE_CWD), "{:?}", report.items);

        write_task(
            &plan,
            "00-y",
            "01-new",
            "id: 01-new\nfiles: [src/app/x.ts]\nnew: [src/app/x.ts]\nverify: \"cargo test n -- --exact\"\nmust_contain: [\"pub fn add\"]\ncwd: web\n",
            "new outside",
        );
        let report = check_plan(&plan, &ws, None);
        assert!(
            report
                .items
                .iter()
                .any(|i| i.rule == RULE_FILES_OUTSIDE_CWD && i.task_id.as_deref() == Some("01-new")),
            "{:?}",
            report.items
        );

        write_task(
            &plan,
            "00-z",
            "01-root",
            &rust_yml("01-root", "root.rs", "cargo test r -- --exact", "", ""),
            "unset cwd",
        );
        let report = check_plan(&plan, &ws, None);
        assert!(
            !report
                .items
                .iter()
                .any(|i| i.rule == RULE_FILES_OUTSIDE_CWD && i.task_id.as_deref() == Some("01-root")),
            "{:?}",
            report.items
        );

        write_task(
            &plan,
            "00-dot",
            "01-dotcwd",
            &rust_yml(
                "01-dotcwd",
                "web/src/app/x.ts",
                "cargo test d -- --exact",
                "",
                "cwd: ./web\n",
            ),
            "dot cwd",
        );
        write_task(
            &plan,
            "00-dot",
            "02-dotfile",
            &rust_yml(
                "02-dotfile",
                "./web/src/app/x.ts",
                "cargo test e -- --exact",
                "",
                "cwd: web\n",
            ),
            "dot file",
        );
        let report = check_plan(&plan, &ws, None);
        assert!(
            !report.items.iter().any(|i| {
                i.rule == RULE_FILES_OUTSIDE_CWD
                    && matches!(i.task_id.as_deref(), Some("01-dotcwd") | Some("02-dotfile"))
            }),
            "./web vs web must resolve the same: {:?}",
            report.items
        );
        let _ = std::fs::remove_dir_all(&root);
    }

    #[test]
    fn empty_plan_or_empty_phase_is_blocking() {
        let root = tmp("empty");
        let plan = root.join("plan");
        let ws = root.join("ws");
        std::fs::create_dir_all(plan.join("phases")).unwrap();
        std::fs::create_dir_all(&ws).unwrap();
        let report = check_plan(&plan, &ws, None);
        assert!(has_rule(&report, RULE_EMPTY_PLAN), "{:?}", report.items);
        assert!(!report.passed);
        assert_eq!(report.tasks_checked, 0);

        std::fs::create_dir_all(plan.join("phases/00-x/tasks")).unwrap();
        let report = check_plan(&plan, &ws, None);
        assert!(has_rule(&report, RULE_EMPTY_PLAN), "{:?}", report.items);
        assert!(!report.passed);
        let _ = std::fs::remove_dir_all(&root);
    }

    #[test]
    fn depends_rules() {
        let root = tmp("dag");
        let plan = root.join("plan");
        let ws = root.join("ws");
        std::fs::create_dir_all(ws.join("src")).unwrap();
        std::fs::write(ws.join("src/a.rs"), "x\n").unwrap();
        write_task(
            &plan,
            "00-x",
            "01-ok",
            &rust_yml("01-ok", "src/a.rs", "cargo test a -- --exact", "", ""),
            "ok",
        );
        write_task(
            &plan,
            "00-x",
            "03-mul",
            &rust_yml("03-mul", "src/a.rs", "cargo test m -- --exact", "01-ok", ""),
            "ok dep",
        );
        let report = check_plan(&plan, &ws, None);
        assert!(
            !report.items.iter().any(|i| i.rule.starts_with("depends_")),
            "{:?}",
            report.items
        );

        write_task(
            &plan,
            "00-u",
            "01-a",
            &rust_yml("01-a", "src/a.rs", "cargo test a -- --exact", "nope", ""),
            "unknown",
        );
        write_task(
            &plan,
            "00-u",
            "02-b",
            &rust_yml("02-b", "src/a.rs", "cargo test b -- --exact", "also-missing", ""),
            "unknown 2",
        );
        let report = check_plan(&plan, &ws, None);
        let unresolved: Vec<_> = report
            .items
            .iter()
            .filter(|i| i.rule == RULE_DEPENDS_UNRESOLVED)
            .collect();
        assert!(
            unresolved.len() >= 2,
            "every unknown dep in one pass: {:?}",
            report.items
        );

        write_task(
            &plan,
            "00-c",
            "01-a",
            &rust_yml("01-a", "src/a.rs", "cargo test a -- --exact", "02-b", ""),
            "cycle a",
        );
        write_task(
            &plan,
            "00-c",
            "02-b",
            &rust_yml("02-b", "src/a.rs", "cargo test b -- --exact", "01-a", ""),
            "cycle b",
        );
        let report = check_plan(&plan, &ws, None);
        assert!(has_rule(&report, RULE_DEPENDS_CYCLE), "{:?}", rules_of(&report));

        write_task(
            &plan,
            "00-s",
            "01-a",
            &rust_yml("01-a", "src/a.rs", "cargo test a -- --exact", "01-a", ""),
            "self",
        );
        let report = check_plan(&plan, &ws, None);
        assert!(has_rule(&report, RULE_DEPENDS_SELF), "{:?}", rules_of(&report));

        write_task(
            &plan,
            "00-f",
            "07-foo",
            &rust_yml("07-foo", "src/a.rs", "cargo test a -- --exact", "09-bar", ""),
            "fwd",
        );
        write_task(
            &plan,
            "00-f",
            "09-bar",
            &rust_yml("09-bar", "src/a.rs", "cargo test b -- --exact", "", ""),
            "later",
        );
        let report = check_plan(&plan, &ws, None);
        assert!(has_rule(&report, RULE_DEPENDS_FORWARD), "{:?}", rules_of(&report));
        let _ = std::fs::remove_dir_all(&root);
    }

    /// Isolated plan: a real 01-a ↔ 02-b cycle plus an unrelated 03-c → zz-nope.
    /// Must emit both `depends_cycle` and `depends_unresolved` in one pass.
    #[test]
    fn depends_cycle_and_unresolved_in_same_phase() {
        let root = tmp("dag-mix");
        let plan = root.join("plan");
        let ws = root.join("ws");
        std::fs::create_dir_all(ws.join("src")).unwrap();
        std::fs::write(ws.join("src/a.rs"), "x\n").unwrap();
        write_task(
            &plan,
            "00-mix",
            "01-a",
            &rust_yml("01-a", "src/a.rs", "cargo test a -- --exact", "02-b", ""),
            "cycle+unknown a",
        );
        write_task(
            &plan,
            "00-mix",
            "02-b",
            &rust_yml("02-b", "src/a.rs", "cargo test b -- --exact", "01-a", ""),
            "cycle+unknown b",
        );
        write_task(
            &plan,
            "00-mix",
            "03-c",
            &rust_yml("03-c", "src/a.rs", "cargo test c -- --exact", "zz-nope", ""),
            "unrelated unknown",
        );
        let report = check_plan(&plan, &ws, None);
        assert!(
            has_rule(&report, RULE_DEPENDS_CYCLE),
            "cycle must still emit when an unknown dep exists in the same phase: {:?}",
            report.items
        );
        assert!(
            report.items.iter().any(|i| {
                i.rule == RULE_DEPENDS_UNRESOLVED && i.task_id.as_deref() == Some("03-c")
            }),
            "unknown dep in the same phase: {:?}",
            report.items
        );
        assert!(
            !report.items.iter().any(|i| i.rule == RULE_DEPENDS_SELF),
            "{:?}",
            report.items
        );
        let _ = std::fs::remove_dir_all(&root);
    }

    #[test]
    fn verify_and_misc_rules() {
        let root = tmp("verify");
        let plan = root.join("plan");
        let ws = root.join("ws");
        std::fs::create_dir_all(ws.join("src/app")).unwrap();
        std::fs::create_dir_all(ws.join("desktop/src-tauri")).unwrap();
        std::fs::create_dir_all(ws.join("web/src/app")).unwrap();
        std::fs::write(ws.join("src/a.rs"), "x\n").unwrap();
        std::fs::write(ws.join("web/src/app/x.spec.ts"), "x\n").unwrap();
        std::fs::write(ws.join("src/app/x.spec.ts"), "root only\n").unwrap();

        write_task(
            &plan,
            "00-x",
            "01-bare",
            &rust_yml("01-bare", "src/a.rs", "cargo test", "", ""),
            "bare",
        );
        let report = check_plan(&plan, &ws, None);
        assert!(has_rule(&report, RULE_VERIFY_NOT_SCOPED), "{:?}", report.items);

        write_task(
            &plan,
            "00-t",
            "01-miss",
            &rust_yml(
                "01-miss",
                "src/a.rs",
                "npx vitest run missing.spec.ts",
                "",
                "",
            ),
            "missing spec",
        );
        let report = check_plan(&plan, &ws, None);
        assert!(has_rule(&report, RULE_VERIFY_TARGET_MISSING), "{:?}", report.items);

        write_task(
            &plan,
            "00-listed",
            "01-listed",
            &rust_yml(
                "01-listed",
                "src/claimed.spec.ts",
                "npx vitest run src/claimed.spec.ts",
                "",
                "",
            ),
            "listed in files[] but not on disk",
        );
        let report = check_plan(&plan, &ws, None);
        assert!(
            report.items.iter().any(|i| {
                i.rule == RULE_FILE_MISSING && i.task_id.as_deref() == Some("01-listed")
            }),
            "{:?}",
            report.items
        );
        assert!(
            !report.items.iter().any(|i| {
                i.rule == RULE_VERIFY_TARGET_MISSING && i.task_id.as_deref() == Some("01-listed")
            }),
            "files[] listing satisfies verify target: {:?}",
            report.items
        );

        write_task(
            &plan,
            "00-m",
            "01-triv",
            "id: 01-triv\nfiles: [src/a.rs]\nverify: \"cargo test a -- --exact\"\nmust_contain: [\"x\", \"return\", \"TODO\"]\n",
            "triv",
        );
        let report = check_plan(&plan, &ws, None);
        assert!(has_rule(&report, RULE_MUST_CONTAIN_TRIVIAL), "{:?}", report.items);
        assert!(
            report.items.iter().any(|i| i.detail.contains("TODO")),
            "case-insensitive trivial: {:?}",
            report.items
        );
        write_task(
            &plan,
            "00-jp",
            "01-jp",
            "id: 01-jp\nfiles: [src/a.rs]\nverify: \"cargo test a -- --exact\"\nmust_contain: [\"日本語\"]\n",
            "three chars is trivial even if nine bytes",
        );
        let report = check_plan(&plan, &ws, None);
        assert!(
            report.items.iter().any(|i| {
                i.rule == RULE_MUST_CONTAIN_TRIVIAL && i.task_id.as_deref() == Some("01-jp")
            }),
            "3 unicode chars is < 4 chars: {:?}",
            report.items
        );

        write_task(
            &plan,
            "00-u",
            "01-html",
            &rust_yml("01-html", "src/a.html", "cargo test a -- --exact", "", ""),
            "html",
        );
        std::fs::write(ws.join("src/a.html"), "<p></p>\n").unwrap();
        let report = check_plan(&plan, &ws, None);
        assert!(has_rule(&report, RULE_UI_TASK_FORBIDDEN), "{:?}", report.items);

        write_task(
            &plan,
            "00-k",
            "01-mock",
            &rust_yml(
                "01-mock",
                "src/a.rs",
                "cargo test a -- --exact",
                "",
                "mock: artifacts/x.png\n",
            ),
            "mock missing",
        );
        let report = check_plan(&plan, &ws, None);
        assert!(
            report
                .items
                .iter()
                .any(|i| i.rule == RULE_UI_TASK_FORBIDDEN && i.task_id.as_deref() == Some("01-mock")),
            "{:?}",
            report.items
        );

        std::fs::create_dir_all(plan.join("artifacts")).unwrap();
        std::fs::write(plan.join("artifacts/exists.png"), "png").unwrap();
        write_task(
            &plan,
            "00-e",
            "01-mockex",
            &rust_yml(
                "01-mockex",
                "src/a.rs",
                "cargo test a -- --exact",
                "",
                "mock: artifacts/exists.png\n",
            ),
            "mock exists still forbidden",
        );
        let report = check_plan(&plan, &ws, None);
        assert!(
            report.items.iter().any(|i| {
                i.rule == RULE_UI_TASK_FORBIDDEN && i.task_id.as_deref() == Some("01-mockex")
            }),
            "{:?}",
            report.items
        );

        write_task(
            &plan,
            "00-c",
            "01-cwdok",
            &rust_yml(
                "01-cwdok",
                "desktop/src-tauri/ok.rs",
                "cargo test a -- --exact",
                "",
                "cwd: desktop/src-tauri\n",
            ),
            "cwd exists",
        );
        std::fs::write(ws.join("desktop/src-tauri/ok.rs"), "x\n").unwrap();
        let report = check_plan(&plan, &ws, None);
        assert!(
            !report.items.iter().any(|i| {
                i.rule == RULE_VERIFY_CWD_MISSING && i.task_id.as_deref() == Some("01-cwdok")
            }),
            "{:?}",
            report.items
        );

        write_task(
            &plan,
            "00-d",
            "01-cwdmiss",
            &rust_yml(
                "01-cwdmiss",
                "src/a.rs",
                "cargo test a -- --exact",
                "",
                "cwd: does-not-exist\n",
            ),
            "cwd missing",
        );
        let report = check_plan(&plan, &ws, None);
        assert!(has_rule(&report, RULE_VERIFY_CWD_MISSING), "{:?}", report.items);

        write_task(
            &plan,
            "00-w",
            "01-web",
            &rust_yml(
                "01-web",
                "web/src/app/x.spec.ts",
                "npx --no-install vitest run src/app/x.spec.ts",
                "",
                "cwd: web\n",
            ),
            "cwd web target ok",
        );
        let report = check_plan(&plan, &ws, None);
        assert!(
            !report.items.iter().any(|i| {
                i.rule == RULE_VERIFY_TARGET_MISSING && i.task_id.as_deref() == Some("01-web")
            }),
            "target under cwd: {:?}",
            report.items
        );

        std::fs::write(ws.join("src/app/rootonly.spec.ts"), "root only\n").unwrap();
        write_task(
            &plan,
            "00-r",
            "01-rootonly",
            &rust_yml(
                "01-rootonly",
                "web/src/app/x.spec.ts",
                "npx --no-install vitest run src/app/rootonly.spec.ts",
                "",
                "cwd: web\n",
            ),
            "file only at workspace root",
        );
        let report = check_plan(&plan, &ws, None);
        assert!(
            report.items.iter().any(|i| {
                i.rule == RULE_VERIFY_TARGET_MISSING && i.task_id.as_deref() == Some("01-rootonly")
            }),
            "{:?}",
            report.items
        );
        assert!(
            !report
                .items
                .iter()
                .any(|i| i.rule == "verify_runner_root_missing"),
            "{:?}",
            report.items
        );
        let _ = std::fs::remove_dir_all(&root);
    }

    #[cfg(unix)]
    #[test]
    fn symlink_cwd_is_blocking_not_silently_joined() {
        let root = tmp("cwd-link");
        let plan = root.join("plan");
        let ws = root.join("ws");
        std::fs::create_dir_all(ws.join("src")).unwrap();
        std::fs::write(ws.join("src/a.rs"), "x\n").unwrap();
        std::os::unix::fs::symlink("/etc", ws.join("link")).unwrap();
        write_task(
            &plan,
            "00-x",
            "01-a",
            &rust_yml(
                "01-a",
                "src/a.rs",
                "cargo test a -- --exact",
                "",
                "cwd: link\n",
            ),
            "symlink cwd",
        );
        let report = check_plan(&plan, &ws, None);
        assert!(
            report.items.iter().any(|i| {
                i.task_id.as_deref() == Some("01-a") && i.rule == RULE_VERIFY_CWD_MISSING
            }),
            "symlink cwd must be verify_cwd_missing, not verify_not_allowlisted: {:?}",
            report.items
        );
        assert!(
            !report.items.iter().any(|i| i.rule == RULE_VERIFY_NOT_ALLOWLISTED),
            "cwd jail/escape is not an argv allowlist miss: {:?}",
            report.items
        );
        assert!(!report.passed);
        let _ = std::fs::remove_dir_all(&root);
    }

    #[test]
    fn task_count_phase_over_150() {
        let root = tmp("n151");
        let plan = root.join("plan");
        let ws = root.join("ws");
        std::fs::create_dir_all(ws.join("src")).unwrap();
        std::fs::write(ws.join("src/a.rs"), "x\n").unwrap();
        for i in 0..151 {
            let id = format!("t{i:03}");
            write_task(
                &plan,
                "00-big",
                &id,
                &format!(
                    "id: {id}\nfiles: [src/a.rs]\nverify: \"cargo test a -- --exact\"\nmust_contain: [\"pub fn add\"]\n"
                ),
                "p",
            );
        }
        let report = check_plan(&plan, &ws, None);
        assert!(has_rule(&report, RULE_TASK_COUNT_PHASE), "{:?}", rules_of(&report));
        let _ = std::fs::remove_dir_all(&root);
    }

    fn builder_for(rule: &str, plan: &Path, ws: &Path) {
        std::fs::create_dir_all(ws.join("src")).unwrap();
        std::fs::write(ws.join("src/a.rs"), "x\n").unwrap();
        match rule {
            RULE_MISSING_FILES => write_task(
                plan,
                "00-x",
                "01-a",
                "id: 01-a\nfiles: []\nverify: \"cargo test a -- --exact\"\n",
                "p",
            ),
            RULE_EMPTY_VERIFY => write_task(
                plan,
                "00-x",
                "01-a",
                "id: 01-a\nfiles: [src/a.rs]\nverify: \"\"\nmust_contain: [\"pub fn add\"]\n",
                "p",
            ),
            RULE_EMPTY_PROMPT => write_task(
                plan,
                "00-x",
                "01-a",
                &rust_yml("01-a", "src/a.rs", "cargo test a -- --exact", "", ""),
                "  \n",
            ),
            RULE_FILE_MISSING => write_task(
                plan,
                "00-x",
                "01-a",
                &rust_yml("01-a", "src/gone.rs", "cargo test a -- --exact", "", ""),
                "p",
            ),
            RULE_FILE_COLLISION => {
                write_task(
                    plan,
                    "00-x",
                    "01-a",
                    &rust_yml("01-a", "src/a.rs", "cargo test a -- --exact", "", ""),
                    "p",
                );
                write_task(
                    plan,
                    "00-x",
                    "02-b",
                    &rust_yml("02-b", "src/a.rs", "cargo test b -- --exact", "", ""),
                    "p",
                );
            }
            RULE_DEPENDS_UNRESOLVED => write_task(
                plan,
                "00-x",
                "01-a",
                &rust_yml("01-a", "src/a.rs", "cargo test a -- --exact", "nope", ""),
                "p",
            ),
            RULE_DEPENDS_CYCLE => {
                write_task(
                    plan,
                    "00-x",
                    "01-a",
                    &rust_yml("01-a", "src/a.rs", "cargo test a -- --exact", "02-b", ""),
                    "p",
                );
                write_task(
                    plan,
                    "00-x",
                    "02-b",
                    &rust_yml("02-b", "src/a.rs", "cargo test b -- --exact", "01-a", ""),
                    "p",
                );
            }
            RULE_DEPENDS_FORWARD => {
                write_task(
                    plan,
                    "00-x",
                    "07-foo",
                    &rust_yml("07-foo", "src/a.rs", "cargo test a -- --exact", "09-bar", ""),
                    "p",
                );
                write_task(
                    plan,
                    "00-x",
                    "09-bar",
                    &rust_yml("09-bar", "src/a.rs", "cargo test b -- --exact", "", ""),
                    "p",
                );
            }
            RULE_DEPENDS_SELF => write_task(
                plan,
                "00-x",
                "01-a",
                &rust_yml("01-a", "src/a.rs", "cargo test a -- --exact", "01-a", ""),
                "p",
            ),
            RULE_VERIFY_NOT_ALLOWLISTED => write_task(
                plan,
                "00-x",
                "01-a",
                &rust_yml("01-a", "src/a.rs", "rm -rf /", "", ""),
                "p",
            ),
            RULE_VERIFY_NOT_SCOPED => write_task(
                plan,
                "00-x",
                "01-a",
                &rust_yml("01-a", "src/a.rs", "cargo test", "", ""),
                "p",
            ),
            RULE_VERIFY_TARGET_MISSING => write_task(
                plan,
                "00-x",
                "01-a",
                &rust_yml(
                    "01-a",
                    "src/a.rs",
                    "npx vitest run missing.spec.ts",
                    "",
                    "",
                ),
                "p",
            ),
            RULE_VERIFY_CWD_MISSING => write_task(
                plan,
                "00-x",
                "01-a",
                &rust_yml(
                    "01-a",
                    "src/a.rs",
                    "cargo test a -- --exact",
                    "",
                    "cwd: no-such-dir\n",
                ),
                "p",
            ),
            RULE_FILES_OUTSIDE_CWD => {
                std::fs::create_dir_all(ws.join("web")).unwrap();
                write_task(
                    plan,
                    "00-x",
                    "01-a",
                    &rust_yml(
                        "01-a",
                        "src/a.rs",
                        "cargo test a -- --exact",
                        "",
                        "cwd: web\n",
                    ),
                    "p",
                );
            }
            RULE_MUST_CONTAIN_TRIVIAL => write_task(
                plan,
                "00-x",
                "01-a",
                "id: 01-a\nfiles: [src/a.rs]\nverify: \"cargo test a -- --exact\"\nmust_contain: []\n",
                "p",
            ),
            RULE_TASK_COUNT_PHASE => {
                for i in 0..151 {
                    let id = format!("n{i:03}");
                    write_task(
                        plan,
                        "00-x",
                        &id,
                        &format!(
                            "id: {id}\nfiles: [src/a.rs]\nverify: \"cargo test a -- --exact\"\nmust_contain: [\"pub fn add\"]\n"
                        ),
                        "p",
                    );
                }
            }
            RULE_TASK_COUNT_TOTAL => {
                for phase in 0..6 {
                    for i in 0..134 {
                        let id = format!("n{i:03}");
                        write_task(
                            plan,
                            &format!("p{phase:02}"),
                            &id,
                            &format!(
                                "id: {id}\nfiles: [src/a.rs]\nverify: \"cargo test a -- --exact\"\nmust_contain: [\"pub fn add\"]\n"
                            ),
                            "p",
                        );
                    }
                }
            }
            RULE_EMPTY_PLAN => {
                std::fs::create_dir_all(plan.join("phases")).unwrap();
            }
            RULE_UI_TASK_FORBIDDEN => write_task(
                plan,
                "00-x",
                "01-a",
                &rust_yml(
                    "01-a",
                    "src/a.rs",
                    "cargo test a -- --exact",
                    "",
                    "mock: artifacts/x.png\n",
                ),
                "p",
            ),
            other => panic!("no builder for {other}"),
        }
    }

    #[test]
    fn rejects_each_static_rule() {
        let static_rules = [
            RULE_MISSING_FILES,
            RULE_EMPTY_VERIFY,
            RULE_EMPTY_PROMPT,
            RULE_FILE_MISSING,
            RULE_FILE_COLLISION,
            RULE_DEPENDS_UNRESOLVED,
            RULE_DEPENDS_CYCLE,
            RULE_DEPENDS_FORWARD,
            RULE_DEPENDS_SELF,
            RULE_VERIFY_NOT_ALLOWLISTED,
            RULE_VERIFY_NOT_SCOPED,
            RULE_VERIFY_TARGET_MISSING,
            RULE_VERIFY_CWD_MISSING,
            RULE_FILES_OUTSIDE_CWD,
            RULE_MUST_CONTAIN_TRIVIAL,
            RULE_TASK_COUNT_PHASE,
            RULE_TASK_COUNT_TOTAL,
            RULE_EMPTY_PLAN,
            RULE_UI_TASK_FORBIDDEN,
        ];
        for rule in static_rules {
            let root = tmp(&format!("rule-{rule}"));
            let plan = root.join("plan");
            let ws = root.join("ws");
            builder_for(rule, &plan, &ws);
            let report = check_plan(&plan, &ws, None);
            assert!(
                has_rule(&report, rule),
                "expected {rule}, got {:?}",
                rules_of(&report)
            );
            let _ = std::fs::remove_dir_all(&root);
        }
    }

    struct ScriptedHost {
        warm: std::sync::Mutex<Vec<(Vec<String>, PathBuf)>>,
        tasks: std::sync::Mutex<Vec<(Vec<String>, PathBuf)>>,
        in_flight: std::sync::atomic::AtomicUsize,
        max_in_flight: std::sync::atomic::AtomicUsize,
        warm_exit: i32,
        task_exit: i32,
        task_timeout: bool,
        refuse_after: Option<usize>,
        tokens: Result<TokensReport, TokensError>,
    }

    impl ScriptedHost {
        fn instant() -> Self {
            Self {
                warm: std::sync::Mutex::new(Vec::new()),
                tasks: std::sync::Mutex::new(Vec::new()),
                in_flight: std::sync::atomic::AtomicUsize::new(0),
                max_in_flight: std::sync::atomic::AtomicUsize::new(0),
                warm_exit: 0,
                task_exit: 1,
                task_timeout: false,
                refuse_after: None,
                tokens: Ok(TokensReport {
                    fits: true,
                    approx_tokens: Some(10),
                    usable_window: Some(26214),
                    compact_trigger: Some(1),
                }),
            }
        }
    }

    impl PlanCheckHost for ScriptedHost {
        fn spawn(&self, req: &SpawnRequest) -> SpawnResult {
            let n = self.in_flight.fetch_add(1, Ordering::SeqCst) + 1;
            self.max_in_flight.fetch_max(n, Ordering::SeqCst);
            let result = match req.kind {
                SpawnKind::Warm => {
                    self.warm
                        .lock()
                        .unwrap()
                        .push((req.argv.clone(), req.cwd.clone()));
                    if self.warm_exit == 127 {
                        SpawnResult::exit(127)
                    } else if self.warm_exit < 0 {
                        SpawnResult::missing()
                    } else {
                        SpawnResult::exit(self.warm_exit)
                    }
                }
                SpawnKind::Task => {
                    let started = {
                        let mut g = self.tasks.lock().unwrap();
                        if let Some(limit) = self.refuse_after {
                            if g.len() >= limit {
                                self.in_flight.fetch_sub(1, Ordering::SeqCst);
                                return SpawnResult::never_started();
                            }
                        }
                        g.push((req.argv.clone(), req.cwd.clone()));
                        true
                    };
                    if !started {
                        SpawnResult::never_started()
                    } else if self.task_timeout {
                        SpawnResult::timed_out()
                    } else if self.task_exit == 127 {
                        SpawnResult::exit(127)
                    } else if self.task_exit < 0 {
                        SpawnResult::missing()
                    } else {
                        SpawnResult::exit(self.task_exit)
                    }
                }
            };
            self.in_flight.fetch_sub(1, Ordering::SeqCst);
            result
        }

        fn tokens(&self, _ctx: u32, _prompt: &Path) -> Result<TokensReport, TokensError> {
            self.tokens.clone()
        }
    }

    fn write_ok_task(plan: &Path, phase: &str, id: &str, extra: &str) {
        write_task(
            plan,
            phase,
            id,
            &rust_yml(id, "src/a.rs", "cargo test a -- --exact", "", extra),
            "implement add",
        );
    }

    #[test]
    fn execute_path_missing_is_not_runnable() {
        let root = tmp("exec-127");
        let plan = root.join("plan");
        let ws = root.join("ws");
        std::fs::create_dir_all(ws.join("src")).unwrap();
        std::fs::write(ws.join("src/a.rs"), "x\n").unwrap();
        write_ok_task(&plan, "00-x", "01-a", "");
        let mut host = ScriptedHost::instant();
        host.warm_exit = 0;
        host.task_exit = 127;
        let report = check_plan_with(
            &plan,
            &ws,
            &CheckPlanOpts {
                host: Some(&host),
                execute: true,
                ..CheckPlanOpts::shape_only(None)
            },
        );
        assert!(has_rule(&report, RULE_VERIFY_NOT_RUNNABLE), "{:?}", report.items);
        assert!(!report.passed);
        let _ = std::fs::remove_dir_all(&root);
    }

    #[test]
    fn execute_per_task_exit_1_is_not_an_item() {
        let root = tmp("exec-1");
        let plan = root.join("plan");
        let ws = root.join("ws");
        std::fs::create_dir_all(ws.join("src")).unwrap();
        std::fs::write(ws.join("src/a.rs"), "x\n").unwrap();
        write_ok_task(&plan, "00-x", "01-a", "");
        let host = ScriptedHost::instant(); // warm 0, task 1
        let report = check_plan_with(
            &plan,
            &ws,
            &CheckPlanOpts {
                host: Some(&host),
                execute: true,
                ..CheckPlanOpts::shape_only(None)
            },
        );
        assert!(
            !has_rule(&report, RULE_VERIFY_NOT_RUNNABLE),
            "{:?}",
            report.items
        );
        assert_eq!(report.verify_executed, 1);
        assert_eq!(report.verify_skipped, 0);
        assert!(report.passed);
        let _ = std::fs::remove_dir_all(&root);
    }

    #[test]
    fn warm_nonzero_is_not_runnable_per_runner() {
        for (runner, verify, extra) in [
            ("cargo", "cargo test a -- --exact", ""),
            ("go", "go test ./pkg", ""),
            ("python3", "python3 -m pytest tests/test_a.py", ""),
            ("npx", "npx vitest run a.spec.ts", ""),
            ("node", "node a.test.js", ""),
        ] {
            let root = tmp(&format!("warm-{runner}"));
            let plan = root.join("plan");
            let ws = root.join("ws");
            std::fs::create_dir_all(ws.join("src")).unwrap();
            std::fs::write(ws.join("src/a.rs"), "x\n").unwrap();
            std::fs::write(ws.join("a.spec.ts"), "x\n").unwrap();
            std::fs::write(ws.join("a.test.js"), "x\n").unwrap();
            std::fs::create_dir_all(ws.join("tests")).unwrap();
            std::fs::write(ws.join("tests/test_a.py"), "x\n").unwrap();
            std::fs::create_dir_all(ws.join("pkg")).unwrap();
            write_task(
                &plan,
                "00-x",
                "01-a",
                &format!(
                    "id: 01-a\nfiles: [src/a.rs]\nverify: \"{verify}\"\nmust_contain: [\"pub fn add\"]\n{extra}"
                ),
                "p",
            );
            let mut host = ScriptedHost::instant();
            host.warm_exit = 101;
            let report = check_plan_with(
                &plan,
                &ws,
                &CheckPlanOpts {
                    host: Some(&host),
                    execute: true,
                    ..CheckPlanOpts::shape_only(None)
                },
            );
            assert!(
                has_rule(&report, RULE_VERIFY_NOT_RUNNABLE),
                "{runner}: {:?}",
                report.items
            );
            let warms = host.warm.lock().unwrap();
            assert!(
                warms.iter().any(|(argv, _)| argv[0] == runner),
                "{runner} warm argv: {warms:?}"
            );
            assert_eq!(
                warms[0].0,
                warm_argv_for(runner, &[runner.to_string()]),
                "{runner} pinned warm"
            );
            let _ = std::fs::remove_dir_all(&root);
        }
    }

    #[test]
    fn execute_timeout_is_advisory() {
        let root = tmp("exec-to");
        let plan = root.join("plan");
        let ws = root.join("ws");
        std::fs::create_dir_all(ws.join("src")).unwrap();
        std::fs::write(ws.join("src/a.rs"), "x\n").unwrap();
        write_ok_task(&plan, "00-x", "01-a", "");
        let mut host = ScriptedHost::instant();
        host.task_timeout = true;
        let report = check_plan_with(
            &plan,
            &ws,
            &CheckPlanOpts {
                host: Some(&host),
                execute: true,
                ..CheckPlanOpts::shape_only(None)
            },
        );
        assert!(has_rule(&report, RULE_VERIFY_TIMEOUT), "{:?}", report.items);
        assert!(
            report.items.iter().any(|i| i.rule == RULE_VERIFY_TIMEOUT && !i.blocking),
            "{:?}",
            report.items
        );
        assert!(report.passed);
        let _ = std::fs::remove_dir_all(&root);
    }

    #[test]
    fn execute_budget_skip_is_not_executed() {
        let root = tmp("exec-skip");
        let plan = root.join("plan");
        let ws = root.join("ws");
        std::fs::create_dir_all(ws.join("src")).unwrap();
        std::fs::write(ws.join("src/a.rs"), "x\n").unwrap();
        for i in 0..8 {
            write_ok_task(&plan, "00-x", &format!("t{i:02}"), "");
        }
        let mut host = ScriptedHost::instant();
        host.refuse_after = Some(4);
        let report = check_plan_with(
            &plan,
            &ws,
            &CheckPlanOpts {
                host: Some(&host),
                execute: true,
                execute_budget_ms: Some(1),
                ..CheckPlanOpts::shape_only(None)
            },
        );
        // 1ms budget: first wave may start; remainder never-started.
        assert!(
            report.verify_executed + report.verify_skipped == 8,
            "e={} s={} items={:?}",
            report.verify_executed,
            report.verify_skipped,
            report.items
        );
        assert_eq!(
            report
                .items
                .iter()
                .filter(|i| i.rule == RULE_VERIFY_NOT_EXECUTED)
                .count(),
            report.verify_skipped
        );
        let _ = std::fs::remove_dir_all(&root);
    }

    #[test]
    fn execute_150_instant_exit_covers_all() {
        let root = tmp("exec-150");
        let plan = root.join("plan");
        let ws = root.join("ws");
        std::fs::create_dir_all(ws.join("src")).unwrap();
        std::fs::write(ws.join("src/a.rs"), "x\n").unwrap();
        for i in 0..150 {
            let id = format!("n{i:03}");
            let f = format!("src/{id}.rs");
            std::fs::write(ws.join(&f), "x\n").unwrap();
            write_task(
                &plan,
                "00-x",
                &id,
                &rust_yml(&id, &f, "cargo test a -- --exact", "", ""),
                "p",
            );
        }
        let host = ScriptedHost::instant();
        let report = check_plan_with(
            &plan,
            &ws,
            &CheckPlanOpts {
                host: Some(&host),
                execute: true,
                ..CheckPlanOpts::shape_only(None)
            },
        );
        assert_eq!(report.verify_executed, 150);
        assert_eq!(report.verify_skipped, 0);
        assert!(report.passed, "{:?}", report.items);
        assert!(report.verify_ms <= execute_budget_ms(150) + 5_000);
        let tasks = host.tasks.lock().unwrap();
        assert_eq!(tasks.len(), 150);
        for (_, cwd) in tasks.iter() {
            assert_eq!(cwd, &ws);
        }
        let _ = std::fs::remove_dir_all(&root);
    }

    #[test]
    fn warm_capped_at_eight_pairs_and_four_wide() {
        let root = tmp("warm-cap");
        let plan = root.join("plan");
        let ws = root.join("ws");
        std::fs::create_dir_all(ws.join("src")).unwrap();
        std::fs::write(ws.join("src/a.rs"), "x\n").unwrap();
        let runners = [
            ("cargo", "cargo test a -- --exact"),
            ("go", "go test ./pkg"),
            ("python3", "python3 -m pytest tests/test_a.py"),
            ("python", "python -m pytest tests/test_a.py"),
            ("npx", "npx vitest run a.spec.ts"),
            ("node", "node a.test.js"),
        ];
        std::fs::write(ws.join("a.spec.ts"), "x\n").unwrap();
        std::fs::write(ws.join("a.test.js"), "x\n").unwrap();
        std::fs::create_dir_all(ws.join("tests")).unwrap();
        std::fs::write(ws.join("tests/test_a.py"), "x\n").unwrap();
        std::fs::create_dir_all(ws.join("pkg")).unwrap();
        // 6 runners × 2 cwds = 12 pairs
        for (i, (_runner, verify)) in runners.iter().enumerate() {
            write_task(
                &plan,
                "00-x",
                &format!("a{i}"),
                &format!(
                    "id: a{i}\nfiles: [src/a.rs]\nverify: \"{verify}\"\nmust_contain: [\"pub fn add\"]\n"
                ),
                "p",
            );
            write_task(
                &plan,
                "00-x",
                &format!("b{i}"),
                &format!(
                    "id: b{i}\nfiles: [src/a.rs]\nverify: \"{verify}\"\nmust_contain: [\"pub fn add\"]\ncwd: src\n"
                ),
                "p",
            );
        }
        let host = ScriptedHost::instant();
        let report = check_plan_with(
            &plan,
            &ws,
            &CheckPlanOpts {
                host: Some(&host),
                execute: true,
                ..CheckPlanOpts::shape_only(None)
            },
        );
        let warms = host.warm.lock().unwrap();
        assert!(
            warms.len() <= MAX_WARM_PAIRS,
            "warm spawns {} > {MAX_WARM_PAIRS}",
            warms.len()
        );
        assert!(
            host.max_in_flight.load(Ordering::SeqCst) <= 4,
            "in flight {}",
            host.max_in_flight.load(Ordering::SeqCst)
        );
        assert!(report.warm_ms > 0 || !warms.is_empty());
        // warm_ms is not folded into verify_ms (they are independent counters)
        assert!(report.verify_ms < u64::MAX);
        let _ = std::fs::remove_dir_all(&root);
    }

    #[test]
    fn deferred_cwd_skips_warm_and_is_not_eligible() {
        let root = tmp("defer");
        let plan = root.join("plan");
        let ws = root.join("ws");
        std::fs::create_dir_all(&ws).unwrap();
        write_task(
            &plan,
            "00-x",
            "01-new",
            "id: 01-new\nfiles: [crate/src/lib.rs]\nnew: [crate/src/lib.rs]\nverify: \"cargo test a -- --exact\"\nmust_contain: [\"pub fn add\"]\ncwd: crate\n",
            "new crate",
        );
        let host = ScriptedHost::instant();
        let report = check_plan_with(
            &plan,
            &ws,
            &CheckPlanOpts {
                host: Some(&host),
                execute: true,
                ..CheckPlanOpts::shape_only(None)
            },
        );
        assert!(
            !has_rule(&report, RULE_VERIFY_NOT_RUNNABLE),
            "{:?}",
            report.items
        );
        assert!(host.warm.lock().unwrap().is_empty());
        assert_eq!(report.verify_executed, 0);
        assert_eq!(report.verify_skipped, 0);
        let _ = std::fs::remove_dir_all(&root);
    }

    #[test]
    fn tokens_fits_false_emits_numbers() {
        let root = tmp("tok-no");
        let plan = root.join("plan");
        let ws = root.join("ws");
        std::fs::create_dir_all(ws.join("src")).unwrap();
        std::fs::write(ws.join("src/a.rs"), "x\n").unwrap();
        write_ok_task(&plan, "00-x", "01-a", "");
        let mut host = ScriptedHost::instant();
        host.tokens = Ok(TokensReport {
            fits: false,
            approx_tokens: Some(40000),
            usable_window: Some(26214),
            compact_trigger: Some(18000),
        });
        let report = check_plan_with(
            &plan,
            &ws,
            &CheckPlanOpts {
                host: Some(&host),
                tokens: true,
                ..CheckPlanOpts::shape_only(None)
            },
        );
        let item = report
            .items
            .iter()
            .find(|i| i.rule == RULE_PROMPT_EXCEEDS_CONTEXT)
            .expect("item");
        assert!(item.detail.contains("40000"), "{}", item.detail);
        assert!(item.detail.contains("26214"), "{}", item.detail);
        assert!(item.detail.contains("18000"), "{}", item.detail);
        let _ = std::fs::remove_dir_all(&root);
    }

    #[test]
    fn tokens_fits_true_is_silent() {
        let root = tmp("tok-yes");
        let plan = root.join("plan");
        let ws = root.join("ws");
        std::fs::create_dir_all(ws.join("src")).unwrap();
        std::fs::write(ws.join("src/a.rs"), "x\n").unwrap();
        write_ok_task(&plan, "00-x", "01-a", "");
        let host = ScriptedHost::instant();
        let report = check_plan_with(
            &plan,
            &ws,
            &CheckPlanOpts {
                host: Some(&host),
                tokens: true,
                ..CheckPlanOpts::shape_only(None)
            },
        );
        assert!(!has_rule(&report, RULE_PROMPT_EXCEEDS_CONTEXT));
        assert!(!has_rule(&report, RULE_TOKENS_UNAVAILABLE));
        let _ = std::fs::remove_dir_all(&root);
    }

    #[test]
    fn tokens_enoent_is_single_plan_item() {
        let root = tmp("tok-miss");
        let plan = root.join("plan");
        let ws = root.join("ws");
        std::fs::create_dir_all(ws.join("src")).unwrap();
        std::fs::write(ws.join("src/a.rs"), "x\n").unwrap();
        write_ok_task(&plan, "00-x", "01-a", "");
        write_ok_task(&plan, "00-x", "02-b", "");
        let mut host = ScriptedHost::instant();
        host.tokens = Err(TokensError::Unavailable("no kloo".into()));
        let report = check_plan_with(
            &plan,
            &ws,
            &CheckPlanOpts {
                host: Some(&host),
                tokens: true,
                ..CheckPlanOpts::shape_only(None)
            },
        );
        let n = report
            .items
            .iter()
            .filter(|i| i.rule == RULE_TOKENS_UNAVAILABLE)
            .count();
        assert_eq!(n, 1, "{:?}", report.items);
        assert!(report.items.iter().any(|i| i.task_id.is_none()));
        let _ = std::fs::remove_dir_all(&root);
    }

    #[test]
    fn complete_rule_id_set_is_the_overview_table() {
        assert!(RULE_IDS.contains(&RULE_VERIFY_NOT_RUNNABLE));
        assert!(RULE_IDS.contains(&RULE_VERIFY_TIMEOUT));
        assert!(RULE_IDS.contains(&RULE_VERIFY_NOT_EXECUTED));
        assert!(RULE_IDS.contains(&RULE_PROMPT_EXCEEDS_CONTEXT));
        assert!(RULE_IDS.contains(&RULE_TOKENS_UNAVAILABLE));
        assert!(RULE_IDS.contains(&RULE_VERIFY_CWD_MISSING));
        assert!(RULE_IDS.contains(&RULE_FILES_OUTSIDE_CWD));
        assert!(RULE_IDS.contains(&RULE_UI_TASK_FORBIDDEN));
        assert!(RULE_IDS.contains(&RULE_EMPTY_PLAN));
        assert_eq!(RULE_IDS.len(), 24);
    }

    #[test]
    fn every_overview_rule_is_emitted_by_a_fixture() {
        let mut seen = std::collections::HashSet::new();
        for rule in [
            RULE_MISSING_FILES,
            RULE_EMPTY_VERIFY,
            RULE_EMPTY_PROMPT,
            RULE_FILE_MISSING,
            RULE_FILE_COLLISION,
            RULE_DEPENDS_UNRESOLVED,
            RULE_DEPENDS_CYCLE,
            RULE_DEPENDS_FORWARD,
            RULE_DEPENDS_SELF,
            RULE_VERIFY_NOT_ALLOWLISTED,
            RULE_VERIFY_NOT_SCOPED,
            RULE_VERIFY_TARGET_MISSING,
            RULE_VERIFY_CWD_MISSING,
            RULE_FILES_OUTSIDE_CWD,
            RULE_MUST_CONTAIN_TRIVIAL,
            RULE_TASK_COUNT_PHASE,
            RULE_TASK_COUNT_TOTAL,
            RULE_EMPTY_PLAN,
            RULE_UI_TASK_FORBIDDEN,
        ] {
            let root = tmp(&format!("all-{rule}"));
            let plan = root.join("plan");
            let ws = root.join("ws");
            builder_for(rule, &plan, &ws);
            let report = check_plan(&plan, &ws, None);
            for i in &report.items {
                seen.insert(i.rule.clone());
            }
            let _ = std::fs::remove_dir_all(&root);
        }
        // Execute / token rules from dedicated fixtures.
        seen.insert(RULE_VERIFY_NOT_RUNNABLE.to_string());
        seen.insert(RULE_VERIFY_TIMEOUT.to_string());
        seen.insert(RULE_VERIFY_NOT_EXECUTED.to_string());
        seen.insert(RULE_PROMPT_EXCEEDS_CONTEXT.to_string());
        seen.insert(RULE_TOKENS_UNAVAILABLE.to_string());
        for id in RULE_IDS {
            assert!(
                seen.contains(*id),
                "overview rule {id} has no fixture producing it; seen={seen:?}"
            );
        }
    }
}
