//! Static plan-check (shape). Structured `(task_id, rule, detail)` items.
//!
//! No process is spawned here — execute-verify is phase 02. Reuses
//! `task_spec::{load_task_spec, topo_sort}` and `verify_policy::check_verify`.

use crate::services::atomic_fs;
use crate::services::task_spec::{self, load_task_spec, DagError, TaskSpec};
use crate::services::task_state::TaskRunFile;
use crate::services::verify_policy::{self, check_verify};
use serde::{Deserialize, Serialize};
use std::collections::{HashMap, HashSet};
use std::path::{Path, PathBuf};
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

pub const MAX_TASKS_PER_PHASE: usize = 150;
pub const MAX_TASKS_TOTAL: usize = 800;

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
        }
    }

    fn finish(&mut self, shape_ms: u64) {
        self.shape_ms = shape_ms;
        self.passed = !self.items.iter().any(|i| i.blocking);
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
    let started = Instant::now();
    let mut report = PlanCheckReport::empty();
    let phases_dir = plan_path.join("phases");
    let phase_dirs = list_sorted_dirs(&phases_dir);
    let mut total_tasks = 0usize;

    for phase_dir in &phase_dirs {
        let phase_id = dir_name(phase_dir);
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
            run,
            &mut report,
        );
        report.phases.push(PhaseVerifyCounts {
            phase_id,
            verify_executed: 0,
            verify_skipped: 0,
        });
    }
    if total_tasks > MAX_TASKS_TOTAL {
        report.items.push(item(
            None,
            RULE_TASK_COUNT_TOTAL,
            format!("plan has {total_tasks} tasks (max {MAX_TASKS_TOTAL})"),
        ));
    }
    report.tasks_checked = total_tasks;
    report.finish(started.elapsed().as_millis() as u64);
    report
}

struct LoadedTask {
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
            // Commercial leftover (prompt only) is not a small-mode task.
            if prompt.is_file() {
                continue;
            }
            report.items.push(item(
                Some(&id),
                RULE_MISSING_FILES,
                format!("missing task.yml for {id}"),
            ));
            out.push(LoadedTask { spec: None });
            continue;
        }
        match load_task_spec(&dir) {
            Ok(spec) => out.push(LoadedTask { spec: Some(spec) }),
            Err(e) => {
                report.items.push(map_spec_error(&id, &e));
                out.push(LoadedTask { spec: None });
            }
        }
    }
    out
}

fn map_spec_error(id: &str, err: &str) -> PlanCheckItem {
    if err.contains("empty verify") {
        item(Some(id), RULE_EMPTY_VERIFY, err)
    } else if err.contains("empty files") || err.contains("missing task.yml") {
        item(Some(id), RULE_MISSING_FILES, err)
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
            if dep != &spec.id && ids.contains(dep.as_str()) && dep.as_str() > spec.id.as_str() {
                report.items.push(item(
                    Some(&spec.id),
                    RULE_DEPENDS_FORWARD,
                    format!("{} depends_on later sibling {dep}", spec.id),
                ));
            }
        }
    }
    match task_spec::topo_sort(specs) {
        Ok(_) => {}
        Err(DagError::UnknownDep { from, to }) => {
            report.items.push(item(
                Some(&from),
                RULE_DEPENDS_UNRESOLVED,
                format!("{from} depends_on unknown id {to}"),
            ));
        }
        Err(DagError::Cycle { nodes }) => {
            let tid = nodes.first().map(String::as_str);
            report.items.push(item(
                tid,
                RULE_DEPENDS_CYCLE,
                format!("cycle in depends_on: {}", nodes.join(",")),
            ));
        }
        Err(DagError::SelfDep { id }) => {
            report.items.push(item(
                Some(&id),
                RULE_DEPENDS_SELF,
                format!("{id} depends_on itself"),
            ));
        }
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
            if let Some(other) = claimed.get(path) {
                report.items.push(item(
                    Some(&spec.id),
                    RULE_FILE_COLLISION,
                    format!("{other} and {} both claim {path}", spec.id),
                ));
            } else {
                claimed.insert(path.clone(), spec.id.clone());
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
            let rule = if e.rule == RULE_VERIFY_NOT_SCOPED {
                RULE_VERIFY_NOT_SCOPED
            } else {
                RULE_VERIFY_NOT_ALLOWLISTED
            };
            report.items.push(item(Some(&spec.id), rule, e.detail));
            return;
        }
    };
    let cwd = spec.cwd.as_deref().map(str::trim).filter(|c| !c.is_empty());
    let effective = verify_policy::effective_verify_dir(workspace, cwd)
        .unwrap_or_else(|_| workspace.join(cwd.unwrap_or(".")));
    for token in &argv {
        if !is_verify_file_target(token) {
            continue;
        }
        let resolved = effective.join(token);
        let listed = match cwd {
            Some(c) => format!("{}/{token}", c.trim_end_matches('/')),
            None => token.clone(),
        };
        if resolved.is_file() || path_created(&listed, spec, by_id) || path_created(token, spec, by_id)
        {
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
        if trimmed.len() < 4 || TRIVIAL_NEEDLES.contains(&trimmed) {
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

fn path_created(path: &str, spec: &TaskSpec, by_id: &HashMap<String, TaskSpec>) -> bool {
    if spec
        .new
        .as_ref()
        .map(|n| n.iter().any(|p| p == path))
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
            .map(|n| n.iter().any(|p| p == path))
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

fn path_under_cwd(path: &str, cwd: &str) -> bool {
    let cwd = cwd.trim_end_matches('/');
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
        ];
        for (c, s) in pairs {
            assert_eq!(c, s);
        }
        let src = include_str!("plan_check.rs");
        let impl_src = src.split("#[cfg(test)]").next().unwrap();
        assert!(
            !impl_src.contains("runner_root_missing"),
            "deleted rule must not return"
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
        let report = check_plan(&plan, &ws, None);
        assert!(has_rule(&report, RULE_DEPENDS_UNRESOLVED), "{:?}", rules_of(&report));

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
            "00-m",
            "01-triv",
            "id: 01-triv\nfiles: [src/a.rs]\nverify: \"cargo test a -- --exact\"\nmust_contain: [\"x\", \"return\"]\n",
            "triv",
        );
        let report = check_plan(&plan, &ws, None);
        assert!(has_rule(&report, RULE_MUST_CONTAIN_TRIVIAL), "{:?}", report.items);

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
}
