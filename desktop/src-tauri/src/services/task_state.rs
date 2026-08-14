//! Per-phase `tasks.json` run state (j1-task-run/v1).
//!
//! Lives under `runs/<plan_id>/<phase_id>/`, outside the plan store. Single
//! writer (the task loop). Saves go through `atomic_fs::write_atomic`.

use crate::services::atomic_fs;
use crate::services::task_spec::TaskSpec;
use chrono::Utc;
use serde::{Deserialize, Serialize};
use std::collections::{HashMap, HashSet, VecDeque};
use std::path::{Path, PathBuf};

pub const SCHEMA: &str = "j1-task-run/v1";

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct TaskRunFile {
    pub schema: String,
    pub plan_id: String,
    pub phase_id: String,
    pub updated_at: String,
    pub tasks: Vec<TaskRow>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct TaskRow {
    pub id: String,
    pub status: String,
    #[serde(default)]
    pub depends_on: Vec<String>,
    #[serde(default)]
    pub attempts: Vec<AttemptRecord>,
    #[serde(default)]
    pub succeeded_tier: Option<String>,
    #[serde(default)]
    pub commit_sha: Option<String>,
    #[serde(default)]
    pub blocked_by: Option<String>,
    #[serde(default)]
    pub route: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct AttemptRecord {
    pub tier: String,
    pub model: String,
    pub attempt: u32,
    #[serde(default)]
    pub started_at: Option<String>,
    #[serde(default)]
    pub ended_at: Option<String>,
    #[serde(default)]
    pub failure_code: Option<String>,
    #[serde(default)]
    pub exit_code: Option<i32>,
    #[serde(default)]
    pub class: Option<String>,
    #[serde(default)]
    pub checks: Option<serde_json::Value>,
}

pub fn tasks_json_path(runs_dir: &Path) -> PathBuf {
    runs_dir.join("tasks.json")
}

pub fn attempt_json_path(runs_dir: &Path, task_id: &str, n: u32) -> PathBuf {
    runs_dir.join(task_id).join(format!("{n}.json"))
}

pub fn empty_run(plan_id: &str, phase_id: &str) -> TaskRunFile {
    TaskRunFile {
        schema: SCHEMA.to_string(),
        plan_id: plan_id.to_string(),
        phase_id: phase_id.to_string(),
        updated_at: Utc::now().to_rfc3339(),
        tasks: Vec::new(),
    }
}

pub fn seed_from_specs(plan_id: &str, phase_id: &str, specs: &[TaskSpec]) -> TaskRunFile {
    let mut file = empty_run(plan_id, phase_id);
    file.tasks = specs
        .iter()
        .map(|spec| TaskRow {
            id: spec.id.clone(),
            status: "pending".to_string(),
            depends_on: spec.depends_on.clone(),
            attempts: Vec::new(),
            succeeded_tier: None,
            commit_sha: None,
            blocked_by: None,
            route: None,
        })
        .collect();
    file
}

pub fn load_tasks(path: &Path, plan_id: &str, phase_id: &str) -> Result<TaskRunFile, String> {
    if !path.exists() {
        return Ok(empty_run(plan_id, phase_id));
    }
    let raw = std::fs::read(path).map_err(|e| format!("read {}: {}", path.display(), e))?;
    serde_json::from_slice(&raw).map_err(|e| format!("parse {}: {}", path.display(), e))
}

pub fn save_tasks(path: &Path, file: &mut TaskRunFile) -> Result<(), String> {
    file.updated_at = Utc::now().to_rfc3339();
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent)
            .map_err(|e| format!("mkdir {}: {}", parent.display(), e))?;
    }
    let bytes = serde_json::to_vec_pretty(file)
        .map_err(|e| format!("serialize tasks.json: {}", e))?;
    atomic_fs::write_atomic(path, &bytes)
}

/// Mark every incomplete descendant of `failed_id` as `blocked` with
/// `blockedBy` = the root failure. Returns newly blocked ids.
pub fn block_dependents(file: &mut TaskRunFile, failed_id: &str) -> Vec<String> {
    let mut children: HashMap<String, Vec<String>> = HashMap::new();
    for task in &file.tasks {
        for dep in &task.depends_on {
            children
                .entry(dep.clone())
                .or_default()
                .push(task.id.clone());
        }
    }
    let mut seen = HashSet::new();
    let mut queue = VecDeque::new();
    queue.push_back(failed_id.to_string());
    while let Some(id) = queue.pop_front() {
        if !seen.insert(id.clone()) {
            continue;
        }
        if let Some(kids) = children.get(&id) {
            for kid in kids {
                queue.push_back(kid.clone());
            }
        }
    }
    seen.remove(failed_id);
    let mut blocked = Vec::new();
    for task in &mut file.tasks {
        if !seen.contains(task.id.as_str()) {
            continue;
        }
        if task.status == "done" || task.status == "failed" {
            continue;
        }
        task.status = "blocked".to_string();
        task.blocked_by = Some(failed_id.to_string());
        task.attempts.clear();
        blocked.push(task.id.clone());
    }
    blocked
}

/// Reconcile `running` / unsha'd `done` against git commits keyed by
/// `(phase_id, task_id)`. After this, no row stays `running` and every
/// `done` has a sha.
pub fn reconcile(
    file: &mut TaskRunFile,
    phase_id: &str,
    commits: &HashMap<(String, String), String>,
) {
    let now = Utc::now().to_rfc3339();
    for task in &mut file.tasks {
        let sha = commits
            .get(&(phase_id.to_string(), task.id.clone()))
            .cloned();
        match task.status.as_str() {
            "running" => match sha {
                Some(sha) => {
                    task.status = "done".to_string();
                    task.commit_sha = Some(sha);
                }
                None => {
                    if let Some(last) = task.attempts.last_mut() {
                        if last.ended_at.is_none() {
                            last.ended_at = Some(now.clone());
                            last.class = Some("infra".to_string());
                            last.failure_code = Some("interrupted".to_string());
                        }
                    }
                    task.status = "pending".to_string();
                }
            },
            "done" => {
                if task.commit_sha.is_none() {
                    match sha {
                        Some(sha) => task.commit_sha = Some(sha),
                        None => task.status = "pending".to_string(),
                    }
                }
            }
            _ => {}
        }
    }
}

#[derive(Debug, Serialize, Deserialize)]
struct StatusYmlValidation {
    state: String,
    validator: Option<String>,
    checked_at: Option<String>,
    summary: Option<String>,
}

#[derive(Debug, Serialize, Deserialize)]
struct StatusYmlFile {
    schema: String,
    state: String,
    started: Option<String>,
    completed: Option<String>,
    owner: Option<String>,
    blockers: Vec<String>,
    validation: StatusYmlValidation,
    artifacts: Vec<String>,
    notes: Vec<String>,
}

/// Project a run-state row into methodology `task-status/v1` at `task_dir/status.yml`.
pub fn project_status_yml(task_dir: &Path, row: &TaskRow) -> Result<(), String> {
    let (state, validation_state, validator, summary) = match row.status.as_str() {
        "running" => ("in-progress", "pending", None, None),
        "done" => (
            "done",
            "pass",
            Some("j1-task-check"),
            Some(format!(
                "tier={} sha={}",
                row.succeeded_tier.as_deref().unwrap_or("unknown"),
                row.commit_sha.as_deref().unwrap_or("none")
            )),
        ),
        "failed" => ("needs-changes", "fail", Some("j1-task-check"), None),
        "blocked" => ("blocked", "skipped", None, None),
        _ => ("not-started", "pending", None, None),
    };
    let mut blockers = Vec::new();
    if let Some(by) = &row.blocked_by {
        blockers.push(by.clone());
    }
    let yml = StatusYmlFile {
        schema: "task-status/v1".to_string(),
        state: state.to_string(),
        started: None,
        completed: None,
        owner: None,
        blockers,
        validation: StatusYmlValidation {
            state: validation_state.to_string(),
            validator: validator.map(str::to_string),
            checked_at: None,
            summary,
        },
        artifacts: Vec::new(),
        notes: Vec::new(),
    };
    let raw = serde_yaml::to_string(&yml).map_err(|e| format!("serialize status.yml: {e}"))?;
    atomic_fs::write_atomic(&task_dir.join("status.yml"), raw.as_bytes())
}

/// Compact live task-run projection. Written to `run-status.md` so it never
/// overwrites planner/reviewer `status.md` (`## Phase Validation`).
pub fn project_phase_status_md(phase_dir: &Path, file: &TaskRunFile) -> Result<(), String> {
    let mut done = 0u32;
    let mut running = 0u32;
    let mut pending = 0u32;
    let mut failed = Vec::new();
    let mut blocked = Vec::new();
    let mut running_ids = Vec::new();
    for t in &file.tasks {
        match t.status.as_str() {
            "done" => done += 1,
            "running" => {
                running += 1;
                running_ids.push(t.id.as_str());
            }
            "failed" => failed.push(t.id.as_str()),
            "blocked" => blocked.push(t.id.as_str()),
            _ => pending += 1,
        }
    }
    let md = format!(
        "# Status\n\n| state | n |\n|---|---|\n| done | {done} |\n| running | {running} |\n| failed | {} |\n| blocked | {} |\n| pending | {pending} |\n\n## failed\n{}\n\n## blocked\n{}\n\n## running\n{}\n",
        failed.len(),
        blocked.len(),
        if failed.is_empty() {
            "_none_".into()
        } else {
            failed.iter().map(|id| format!("- `{id}`")).collect::<Vec<_>>().join("\n")
        },
        if blocked.is_empty() {
            "_none_".into()
        } else {
            blocked.iter().map(|id| format!("- `{id}`")).collect::<Vec<_>>().join("\n")
        },
        if running_ids.is_empty() {
            "_none_".into()
        } else {
            running_ids.iter().map(|id| format!("- `{id}`")).collect::<Vec<_>>().join("\n")
        },
    );
    std::fs::create_dir_all(phase_dir)
        .map_err(|e| format!("mkdir {}: {e}", phase_dir.display()))?;
    atomic_fs::write_atomic(&phase_dir.join("run-status.md"), md.as_bytes())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::services::agent_plans::normalize_task_status;
    use std::sync::atomic::{AtomicU64, Ordering};

    static SEQ: AtomicU64 = AtomicU64::new(0);

    fn tmp(tag: &str) -> PathBuf {
        let d = std::env::temp_dir().join(format!(
            "j1-task-state-{}-{}-{}",
            tag,
            std::process::id(),
            SEQ.fetch_add(1, Ordering::Relaxed)
        ));
        std::fs::create_dir_all(&d).unwrap();
        d
    }

    fn spec(id: &str, deps: &[&str]) -> TaskSpec {
        TaskSpec {
            id: id.into(),
            files: vec!["src/x.rs".into()],
            verify: "cargo test spec -- --exact".into(),
            must_contain: vec![],
            depends_on: deps.iter().map(|s| (*s).to_string()).collect(),
            ctx: None,
            mock: None,
            new: None,
            cwd: None,
        }
    }

    #[test]
    fn seed_creates_pending_rows() {
        let file = seed_from_specs("plan", "00-x", &[spec("01-add", &[]), spec("02-sub", &["01-add"])]);
        assert_eq!(file.schema, SCHEMA);
        assert_eq!(file.tasks.len(), 2);
        assert!(file.tasks.iter().all(|t| t.status == "pending"));
        assert_eq!(file.tasks[1].depends_on, vec!["01-add"]);
    }

    #[test]
    fn save_load_round_trip_is_atomic_and_phase_keyed() {
        let root = tmp("rt");
        let p1 = crate::services::settings::initiative_runs_path(&root, "init", "plan", "00-a");
        let p2 = crate::services::settings::initiative_runs_path(&root, "init", "plan", "01-b");
        assert_ne!(p1, p2);
        assert!(p1.starts_with(root.join("init").join("runs")));
        assert!(!p1.starts_with(root.join("init").join("plan")));

        let mut f1 = seed_from_specs("plan", "00-a", &[spec("01-add", &[])]);
        let mut f2 = seed_from_specs("plan", "01-b", &[spec("01-add", &[])]);
        f1.tasks[0].status = "done".into();
        f1.tasks[0].commit_sha = Some("aaa".into());
        save_tasks(&tasks_json_path(&p1), &mut f1).unwrap();
        save_tasks(&tasks_json_path(&p2), &mut f2).unwrap();

        let loaded1 = load_tasks(&tasks_json_path(&p1), "plan", "00-a").unwrap();
        let loaded2 = load_tasks(&tasks_json_path(&p2), "plan", "01-b").unwrap();
        assert_eq!(loaded1.tasks[0].status, "done");
        assert_eq!(loaded1.tasks[0].commit_sha.as_deref(), Some("aaa"));
        assert_eq!(loaded2.tasks[0].status, "pending");
        let leftovers: Vec<_> = std::fs::read_dir(&p1)
            .unwrap()
            .filter_map(|e| e.ok())
            .map(|e| e.file_name().to_string_lossy().into_owned())
            .filter(|n| n.ends_with(".j1tmp"))
            .collect();
        assert!(leftovers.is_empty(), "{leftovers:?}");
        let _ = std::fs::remove_dir_all(&root);
    }

    #[test]
    fn project_phase_status_md_is_compact() {
        let dir = tmp("phase-md");
        let mut file = seed_from_specs(
            "p",
            "00-x",
            &(0..120)
                .map(|i| spec(&format!("{i:03}"), &[]))
                .collect::<Vec<_>>(),
        );
        file.tasks[0].status = "failed".into();
        file.tasks[1].status = "blocked".into();
        file.tasks[2].status = "running".into();
        for t in file.tasks.iter_mut().skip(3).take(10) {
            t.status = "done".into();
        }
        std::fs::write(dir.join("status.md"), "## Phase Validation\nPASS\n").unwrap();
        project_phase_status_md(&dir, &file).unwrap();
        let md = std::fs::read_to_string(dir.join("run-status.md")).unwrap();
        assert!(md.lines().count() < 80, "{}", md.lines().count());
        assert!(md.contains("000"));
        assert!(md.contains("failed"));
        assert!(!md.contains("119"), "must not list every pending id");
        let review = std::fs::read_to_string(dir.join("status.md")).unwrap();
        assert!(
            review.contains("Phase Validation"),
            "must not overwrite reviewer status.md: {review}"
        );
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn load_ignores_unknown_fields() {
        let dir = tmp("extra");
        let path = dir.join("tasks.json");
        std::fs::write(
            &path,
            r#"{"schema":"j1-task-run/v1","planId":"p","phaseId":"00-x","updatedAt":"t","futureField":true,"tasks":[{"id":"01-add","status":"pending","bonus":1}]}"#,
        )
        .unwrap();
        let loaded = load_tasks(&path, "p", "00-x").unwrap();
        assert_eq!(loaded.tasks[0].id, "01-add");
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn load_missing_is_empty() {
        let loaded = load_tasks(Path::new("/no/such/tasks.json"), "p", "00-x").unwrap();
        assert!(loaded.tasks.is_empty());
        assert_eq!(loaded.schema, SCHEMA);
    }

    #[test]
    fn project_status_yml_maps_each_state() {
        let dir = tmp("proj");
        let cases = [
            ("pending", "not-started"),
            ("running", "in-progress"),
            ("done", "done"),
            ("failed", "needs-changes"),
            ("blocked", "blocked"),
        ];
        for (run, expected) in cases {
            let mut row = TaskRow {
                id: "01-add".into(),
                status: run.into(),
                depends_on: vec![],
                attempts: vec![],
                succeeded_tier: Some("qwen3-coder".into()),
                commit_sha: Some("deadbeef".into()),
                blocked_by: if run == "blocked" {
                    Some("00-root".into())
                } else {
                    None
                },
                route: None,
            };
            if run == "pending" {
                row.commit_sha = None;
            }
            project_status_yml(&dir, &row).unwrap();
            let parsed: StatusYmlFile =
                serde_yaml::from_str(&std::fs::read_to_string(dir.join("status.yml")).unwrap())
                    .unwrap();
            assert_eq!(normalize_task_status(&parsed.state), expected, "{run}");
            if run == "done" {
                assert_eq!(parsed.validation.state, "pass");
                assert_eq!(parsed.validation.validator.as_deref(), Some("j1-task-check"));
                assert!(parsed.validation.summary.unwrap().contains("deadbeef"));
            }
        }
        let leftovers: Vec<_> = std::fs::read_dir(&dir)
            .unwrap()
            .filter_map(|e| e.ok())
            .map(|e| e.file_name().to_string_lossy().into_owned())
            .filter(|n| n.ends_with(".j1tmp"))
            .collect();
        assert!(leftovers.is_empty());
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn block_dependents_chain_and_sibling() {
        let mut file = seed_from_specs(
            "p",
            "00-x",
            &[
                spec("01-a", &[]),
                spec("02-b", &["01-a"]),
                spec("03-c", &["02-b"]),
                spec("04-sib", &[]),
            ],
        );
        file.tasks[0].status = "failed".into();
        let blocked = block_dependents(&mut file, "01-a");
        assert_eq!(blocked, vec!["02-b", "03-c"]);
        assert!(file.tasks[1].attempts.is_empty());
        assert_eq!(file.tasks[1].blocked_by.as_deref(), Some("01-a"));
        assert_eq!(file.tasks[2].status, "blocked");
        assert_eq!(file.tasks[3].status, "pending");
    }

    #[test]
    fn block_dependents_two_roots_and_done_untouched() {
        let mut file = seed_from_specs(
            "p",
            "00-x",
            &[
                spec("a1", &[]),
                spec("a2", &["a1"]),
                spec("b1", &[]),
                spec("b2", &["b1"]),
                spec("done-child", &["a1"]),
            ],
        );
        file.tasks[4].status = "done".into();
        file.tasks[4].commit_sha = Some("x".into());
        block_dependents(&mut file, "a1");
        assert_eq!(file.tasks[1].status, "blocked");
        assert_eq!(file.tasks[2].status, "pending");
        assert_eq!(file.tasks[3].status, "pending");
        assert_eq!(file.tasks[4].status, "done");
    }

    #[test]
    fn reconcile_running_and_unsha_done() {
        let mut file = seed_from_specs(
            "p",
            "00-calc",
            &[
                spec("01-add", &[]),
                spec("02-sub", &[]),
                spec("03-mul", &[]),
                spec("04-div", &[]),
                spec("05-ok", &[]),
            ],
        );
        file.tasks[0].status = "running".into();
        file.tasks[1].status = "running".into();
        file.tasks[2].status = "done".into();
        file.tasks[2].commit_sha = None;
        file.tasks[3].status = "blocked".into();
        file.tasks[4].status = "failed".into();
        let mut commits = HashMap::new();
        commits.insert(("00-calc".into(), "01-add".into()), "sha-add".into());
        commits.insert(("00-other".into(), "02-sub".into()), "sha-other".into());
        reconcile(&mut file, "00-calc", &commits);
        assert_eq!(file.tasks[0].status, "done");
        assert_eq!(file.tasks[0].commit_sha.as_deref(), Some("sha-add"));
        assert_eq!(file.tasks[1].status, "pending");
        assert_eq!(file.tasks[2].status, "pending");
        assert_eq!(file.tasks[3].status, "blocked");
        assert_eq!(file.tasks[4].status, "failed");
        assert!(file.tasks.iter().all(|t| t.status != "running"));
        assert!(file
            .tasks
            .iter()
            .filter(|t| t.status == "done")
            .all(|t| t.commit_sha.is_some()));
    }
}
