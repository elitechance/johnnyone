//! Mid-phase planner re-entry: `j1-task-amendment/v1` + resume helpers.
//!
//! Lives under `runs/<plan_id>/<phase_id>/amendment.json`, outside the plan
//! store. Types + persist + reset/cap helpers; the coordinator owns spawn.

use crate::services::atomic_fs;
use crate::services::task_spec::TaskSpec;
use crate::services::task_state::{TaskRow, TaskRunFile};
use serde::{Deserialize, Serialize};
use std::collections::HashSet;
use std::path::{Path, PathBuf};

pub const SCHEMA: &str = "j1-task-amendment/v1";
/// Distinct from planning's `MAX_REVISION_ROUNDS` (6). A surgical replan
/// must not burn six commercial planner calls mid-phase.
pub const MAX_REPLAN_ROUNDS: u32 = 3;

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct AttemptSnap {
    pub tier: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub failure_code: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub class: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct RoutedTask {
    pub task_id: String,
    pub route: String,
    /// `shape` or `exhausted` — mapped from the last attempt's classifier class.
    pub rule: String,
    pub blocked_dependents: Vec<String>,
    #[serde(default)]
    pub attempts: Vec<AttemptSnap>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct TaskAmendment {
    pub schema: String,
    pub plan_id: String,
    pub phase_id: String,
    pub round: u32,
    pub routed: Vec<RoutedTask>,
}

pub fn amendment_json_path(runs_dir: &Path) -> PathBuf {
    runs_dir.join("amendment.json")
}

/// One-shot: written when a replan check passes, consumed by the next
/// `run_development_worker_phase` so we skip only that immediate re-entry.
pub fn skip_preflight_marker_path(runs_dir: &Path) -> PathBuf {
    runs_dir.join("replan-skip-preflight")
}

pub fn write_skip_preflight_marker(runs_dir: &Path) -> Result<(), String> {
    std::fs::create_dir_all(runs_dir).map_err(|e| format!("mkdir {}: {e}", runs_dir.display()))?;
    atomic_fs::write_atomic(&skip_preflight_marker_path(runs_dir), b"1")
}

/// Returns true once, then the marker is gone. Later entries run preflight.
pub fn consume_skip_preflight_marker(runs_dir: &Path) -> bool {
    let path = skip_preflight_marker_path(runs_dir);
    if !path.is_file() {
        return false;
    }
    let _ = std::fs::remove_file(&path);
    true
}

/// Human Resume/Amend: restore replan capacity. `round = 0` means the next
/// episode starts at 1 (`next_episode_round`).
pub fn reset_replan_round(path: &Path) -> Result<(), String> {
    let Some(mut existing) = load_amendment_optional(path)? else {
        return Ok(());
    };
    existing.round = 0;
    save_amendment(path, &existing)
}

/// `shape` when the last attempt classified as shape; otherwise `exhausted`.
pub fn amendment_rule_for(row: &TaskRow) -> &'static str {
    match row
        .attempts
        .last()
        .and_then(|a| a.class.as_deref())
        .map(|c| c.to_ascii_lowercase())
        .as_deref()
    {
        Some("shape") => "shape",
        _ => "exhausted",
    }
}

fn snap_attempts(row: &TaskRow) -> Vec<AttemptSnap> {
    row.attempts
        .iter()
        .map(|a| AttemptSnap {
            tier: a.tier.clone(),
            failure_code: a.failure_code.clone(),
            class: a.class.clone(),
        })
        .collect()
}

fn blocked_dependents_of(file: &TaskRunFile, root_id: &str) -> Vec<String> {
    let mut ids: Vec<String> = file
        .tasks
        .iter()
        .filter(|t| t.blocked_by.as_deref() == Some(root_id))
        .map(|t| t.id.clone())
        .collect();
    ids.sort();
    ids
}

/// Collect every `route == "planner"` row plus the ids it blocked.
pub fn build_amendment(
    plan_id: &str,
    phase_id: &str,
    round: u32,
    file: &TaskRunFile,
) -> TaskAmendment {
    let mut routed: Vec<RoutedTask> = file
        .tasks
        .iter()
        .filter(|t| t.route.as_deref() == Some("planner"))
        .map(|t| RoutedTask {
            task_id: t.id.clone(),
            route: "planner".into(),
            rule: amendment_rule_for(t).to_string(),
            blocked_dependents: blocked_dependents_of(file, &t.id),
            attempts: snap_attempts(t),
        })
        .collect();
    routed.sort_by(|a, b| a.task_id.cmp(&b.task_id));
    TaskAmendment {
        schema: SCHEMA.to_string(),
        plan_id: plan_id.to_string(),
        phase_id: phase_id.to_string(),
        round,
        routed,
    }
}

pub fn save_amendment(path: &Path, amendment: &TaskAmendment) -> Result<(), String> {
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent)
            .map_err(|e| format!("mkdir {}: {e}", parent.display()))?;
    }
    let bytes = serde_json::to_vec_pretty(amendment)
        .map_err(|e| format!("serialize amendment.json: {e}"))?;
    atomic_fs::write_atomic(path, &bytes)
}

pub fn load_amendment(path: &Path) -> Result<TaskAmendment, String> {
    let raw = std::fs::read(path).map_err(|e| format!("read {}: {e}", path.display()))?;
    serde_json::from_slice(&raw).map_err(|e| format!("parse {}: {e}", path.display()))
}

/// `Ok(None)` only when the file is absent (first episode). A present but
/// unreadable file is `Err` — the cap must not reset to 1 by deleting/corrupting
/// the file the planner was pointed at.
pub fn load_amendment_optional(path: &Path) -> Result<Option<TaskAmendment>, String> {
    if !path.exists() {
        return Ok(None);
    }
    load_amendment(path).map(Some)
}

/// Next episode round from a persisted amendment. `Err` when the phase has
/// already spent `MAX_REPLAN_ROUNDS` (pass → resume → re-route still counts).
pub fn next_episode_round(existing: Option<&TaskAmendment>) -> Result<u32, ()> {
    match existing {
        None => Ok(1),
        Some(a) if a.round == 0 => Ok(1),
        Some(a) if replan_cap_reached(a.round) => Err(()),
        Some(a) => Ok(a.round.saturating_add(1)),
    }
}

/// After a passing post-replan check: keep `done` (+ sha); flip failed/blocked
/// to pending; seed any new sibling dirs the planner added. Never deletes
/// commits or done rows.
pub fn reset_for_resume(file: &mut TaskRunFile, specs: &[TaskSpec]) {
    let existing: HashSet<String> = file.tasks.iter().map(|t| t.id.clone()).collect();
    for task in &mut file.tasks {
        if task.status != "done" {
            if let Some(spec) = specs.iter().find(|s| s.id == task.id) {
                task.depends_on = spec.depends_on.clone();
            }
        }
        match task.status.as_str() {
            "failed" | "blocked" => {
                task.status = "pending".to_string();
                task.route = None;
                task.blocked_by = None;
                // The loop rebuilds the ladder from attempts. A routed
                // exhausted row that kept its 4 slots would hit
                // Step::Exhausted and never spawn kloo (QA-1 / Lead).
                task.attempts.clear();
                task.succeeded_tier = None;
            }
            _ => {}
        }
    }
    for spec in specs {
        if existing.contains(&spec.id) {
            continue;
        }
        file.tasks.push(TaskRow {
            id: spec.id.clone(),
            status: "pending".to_string(),
            depends_on: spec.depends_on.clone(),
            attempts: Vec::new(),
            succeeded_tier: None,
            commit_sha: None,
            blocked_by: None,
            route: None,
        });
    }
}

/// Done ids whose task dirs the planner deleted. Caller treats this as a
/// plan-check failure and must not reset.
pub fn missing_done_task_ids(file: &TaskRunFile, specs: &[TaskSpec]) -> Vec<String> {
    let spec_ids: HashSet<&str> = specs.iter().map(|s| s.id.as_str()).collect();
    file.tasks
        .iter()
        .filter(|t| t.status == "done" && !spec_ids.contains(t.id.as_str()))
        .map(|t| t.id.clone())
        .collect()
}

/// True when a failed post-replan check at `round` must park (no further spawn).
pub fn replan_cap_reached(round: u32) -> bool {
    round >= MAX_REPLAN_ROUNDS
}

/// Advance `round` for a retry. `Err` when the cap is already reached.
pub fn increment_replan_round(amendment: &mut TaskAmendment) -> Result<u32, ()> {
    if replan_cap_reached(amendment.round) {
        Err(())
    } else {
        amendment.round = amendment.round.saturating_add(1);
        Ok(amendment.round)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::services::task_spec::TaskSpec;
    use crate::services::task_state::{
        empty_run, save_tasks, seed_from_specs, tasks_json_path, AttemptRecord,
    };
    use std::sync::atomic::{AtomicU64, Ordering};

    static SEQ: AtomicU64 = AtomicU64::new(0);

    fn tmp(tag: &str) -> PathBuf {
        let d = std::env::temp_dir().join(format!(
            "j1-replan-{}-{}-{}",
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

    fn exhausted_ladder() -> Vec<AttemptRecord> {
        ["qwen3-coder", "qwen3-coder", "claude", "opus"]
            .into_iter()
            .enumerate()
            .map(|(i, tier)| AttemptRecord {
                tier: tier.into(),
                model: tier.into(),
                attempt: (i as u32) + 1,
                started_at: Some("t".into()),
                ended_at: Some("t".into()),
                failure_code: Some("verify_failed".into()),
                exit_code: Some(1),
                class: Some("model".into()),
                checks: None,
            })
            .collect()
    }

    fn failed_planner(id: &str, class: &str, code: &str) -> TaskRow {
        TaskRow {
            id: id.into(),
            status: "failed".into(),
            depends_on: vec![],
            attempts: vec![AttemptRecord {
                tier: "opus".into(),
                model: "anthropic/claude-opus-5".into(),
                attempt: 1,
                started_at: Some("t0".into()),
                ended_at: Some("t1".into()),
                failure_code: Some(code.into()),
                exit_code: Some(1),
                class: Some(class.into()),
                checks: None,
            }],
            succeeded_tier: None,
            commit_sha: None,
            blocked_by: None,
            route: Some("planner".into()),
        }
    }

    #[test]
    fn one_failed_three_blocked_is_one_routed_entry() {
        let mut file = empty_run("plan", "00-x");
        file.tasks = vec![
            failed_planner("60-queue", "model", "verify_failed"),
            TaskRow {
                id: "61-a".into(),
                status: "blocked".into(),
                depends_on: vec!["60-queue".into()],
                attempts: vec![],
                succeeded_tier: None,
                commit_sha: None,
                blocked_by: Some("60-queue".into()),
                route: None,
            },
            TaskRow {
                id: "62-b".into(),
                status: "blocked".into(),
                depends_on: vec!["61-a".into()],
                attempts: vec![],
                succeeded_tier: None,
                commit_sha: None,
                blocked_by: Some("60-queue".into()),
                route: None,
            },
            TaskRow {
                id: "63-c".into(),
                status: "blocked".into(),
                depends_on: vec!["62-b".into()],
                attempts: vec![],
                succeeded_tier: None,
                commit_sha: None,
                blocked_by: Some("60-queue".into()),
                route: None,
            },
        ];
        let a = build_amendment("plan", "00-x", 1, &file);
        assert_eq!(a.schema, SCHEMA);
        assert_eq!(a.round, 1);
        assert_eq!(a.routed.len(), 1);
        assert_eq!(a.routed[0].task_id, "60-queue");
        assert_eq!(a.routed[0].route, "planner");
        assert_eq!(a.routed[0].rule, "exhausted");
        assert_eq!(
            a.routed[0].blocked_dependents,
            vec![
                String::from("61-a"),
                String::from("62-b"),
                String::from("63-c")
            ]
        );
        assert_eq!(a.routed[0].attempts[0].tier, "opus");
        assert_eq!(
            a.routed[0].attempts[0].failure_code.as_deref(),
            Some("verify_failed")
        );
        assert_eq!(a.routed[0].attempts[0].class.as_deref(), Some("model"));
    }

    #[test]
    fn shape_class_maps_to_shape_rule() {
        let mut file = empty_run("plan", "00-x");
        file.tasks = vec![failed_planner("01-a", "shape", "missing_file")];
        let a = build_amendment("plan", "00-x", 1, &file);
        assert_eq!(a.routed[0].rule, "shape");
    }

    #[test]
    fn no_routed_tasks_is_empty() {
        let file = seed_from_specs("plan", "00-x", &[spec("01-a", &[]), spec("02-b", &["01-a"])]);
        let a = build_amendment("plan", "00-x", 1, &file);
        assert!(a.routed.is_empty());
    }

    #[test]
    fn amendment_json_round_trip_outside_plan_store() {
        let root = tmp("rt");
        let runs = root.join("init").join("runs").join("plan").join("00-x");
        let plan_store = root.join("init").join("plan");
        std::fs::create_dir_all(&plan_store).unwrap();
        let mut file = empty_run("plan", "00-x");
        file.tasks = vec![failed_planner("01-a", "model", "verify_failed")];
        let a = build_amendment("plan", "00-x", 2, &file);
        let path = amendment_json_path(&runs);
        assert!(path.starts_with(&runs));
        assert!(!path.starts_with(&plan_store));
        save_amendment(&path, &a).unwrap();
        let loaded = load_amendment(&path).unwrap();
        assert_eq!(loaded, a);
        assert_eq!(loaded.round, 2);
        let leftovers: Vec<_> = std::fs::read_dir(&runs)
            .unwrap()
            .filter_map(|e| e.ok())
            .map(|e| e.file_name().to_string_lossy().into_owned())
            .filter(|n| n.ends_with(".j1tmp"))
            .collect();
        assert!(leftovers.is_empty(), "{leftovers:?}");
        let _ = std::fs::remove_dir_all(&root);
    }

    #[test]
    fn reset_keeps_done_shas_and_pending_failed_blocked() {
        let mut file = seed_from_specs(
            "p",
            "00-x",
            &[
                spec("01-a", &[]),
                spec("02-b", &["01-a"]),
                spec("03-c", &["02-b"]),
                spec("04-d", &[]),
                spec("05-e", &["04-d"]),
                spec("06-f", &["05-e"]),
            ],
        );
        file.tasks[0].status = "done".into();
        file.tasks[0].commit_sha = Some("sha-a".into());
        file.tasks[1].status = "done".into();
        file.tasks[1].commit_sha = Some("sha-b".into());
        file.tasks[2].status = "done".into();
        file.tasks[2].commit_sha = Some("sha-c".into());
        file.tasks[3].status = "failed".into();
        file.tasks[3].route = Some("planner".into());
        file.tasks[3].succeeded_tier = Some("opus".into());
        file.tasks[3].attempts = exhausted_ladder();
        file.tasks[4].status = "blocked".into();
        file.tasks[4].blocked_by = Some("04-d".into());
        file.tasks[5].status = "blocked".into();
        file.tasks[5].blocked_by = Some("04-d".into());
        reset_for_resume(&mut file, &[]);
        assert_eq!(file.tasks[0].status, "done");
        assert_eq!(file.tasks[0].commit_sha.as_deref(), Some("sha-a"));
        assert_eq!(file.tasks[1].status, "done");
        assert_eq!(file.tasks[1].commit_sha.as_deref(), Some("sha-b"));
        assert_eq!(file.tasks[2].status, "done");
        assert_eq!(file.tasks[2].commit_sha.as_deref(), Some("sha-c"));
        for i in 3..6 {
            assert_eq!(file.tasks[i].status, "pending", "{}", file.tasks[i].id);
            assert!(file.tasks[i].route.is_none());
            assert!(file.tasks[i].blocked_by.is_none());
            assert!(
                file.tasks[i].attempts.is_empty(),
                "{} must re-enter the ladder",
                file.tasks[i].id
            );
            assert!(file.tasks[i].succeeded_tier.is_none());
        }
        assert_eq!(
            file.tasks.iter().filter(|t| t.status == "done").count(),
            3
        );
        assert_eq!(
            file.tasks.iter().filter(|t| t.status == "pending").count(),
            3
        );
    }

    #[test]
    fn reset_refreshes_depends_on_from_specs() {
        let mut file = seed_from_specs(
            "p",
            "00-x",
            &[spec("01-a", &[]), spec("02-b", &["01-a"])],
        );
        file.tasks[0].status = "done".into();
        file.tasks[0].commit_sha = Some("sha".into());
        file.tasks[1].status = "failed".into();
        file.tasks[1].route = Some("planner".into());
        let amended = [spec("01-a", &["ghost"]), spec("02-b", &[])];
        reset_for_resume(&mut file, &amended);
        assert!(
            file.tasks[0].depends_on.is_empty(),
            "done rows keep original depends_on"
        );
        assert!(file.tasks[1].depends_on.is_empty());
    }

    #[test]
    fn reset_seeds_new_sibling_pending() {
        let mut file = seed_from_specs("p", "00-x", &[spec("01-a", &[])]);
        file.tasks[0].status = "done".into();
        file.tasks[0].commit_sha = Some("sha".into());
        let specs = [spec("01-a", &[]), spec("02-new", &["01-a"])];
        reset_for_resume(&mut file, &specs);
        assert_eq!(file.tasks.len(), 2);
        assert_eq!(file.tasks[0].status, "done");
        assert_eq!(file.tasks[1].id, "02-new");
        assert_eq!(file.tasks[1].status, "pending");
        assert_eq!(file.tasks[1].depends_on, vec![String::from("01-a")]);
    }

    #[test]
    fn missing_done_id_is_detected() {
        let mut file = seed_from_specs("p", "00-x", &[spec("01-a", &[]), spec("02-b", &[])]);
        file.tasks[0].status = "done".into();
        file.tasks[0].commit_sha = Some("keep".into());
        let remaining = [spec("02-b", &[])];
        assert_eq!(
            missing_done_task_ids(&file, &remaining),
            vec![String::from("01-a")]
        );
        reset_for_resume(&mut file, &remaining);
        assert_eq!(file.tasks[0].status, "done");
        assert_eq!(file.tasks[0].commit_sha.as_deref(), Some("keep"));
    }

    #[test]
    fn cap_is_three_not_six() {
        assert_eq!(MAX_REPLAN_ROUNDS, 3);
        assert_ne!(MAX_REPLAN_ROUNDS, 6);
        assert!(!replan_cap_reached(1));
        assert!(!replan_cap_reached(2));
        assert!(replan_cap_reached(3));
        assert!(replan_cap_reached(4));
        let mut a = TaskAmendment {
            schema: SCHEMA.into(),
            plan_id: "p".into(),
            phase_id: "00-x".into(),
            round: 1,
            routed: vec![],
        };
        assert_eq!(increment_replan_round(&mut a), Ok(2));
        assert_eq!(increment_replan_round(&mut a), Ok(3));
        assert_eq!(increment_replan_round(&mut a), Err(()));
        assert_eq!(a.round, 3);
        assert_eq!(next_episode_round(None), Ok(1));
        assert_eq!(next_episode_round(Some(&a)), Err(()));
        a.round = 1;
        assert_eq!(next_episode_round(Some(&a)), Ok(2));
        a.round = 0;
        assert_eq!(next_episode_round(Some(&a)), Ok(1));
    }

    #[test]
    fn skip_preflight_marker_is_one_shot() {
        let root = tmp("skip-once");
        assert!(!consume_skip_preflight_marker(&root));
        write_skip_preflight_marker(&root).unwrap();
        assert!(consume_skip_preflight_marker(&root));
        assert!(!consume_skip_preflight_marker(&root));
        let _ = std::fs::remove_dir_all(&root);
    }

    #[test]
    fn reset_replan_round_restores_budget() {
        let root = tmp("reset-round");
        let path = amendment_json_path(&root);
        let a = TaskAmendment {
            schema: SCHEMA.into(),
            plan_id: "p".into(),
            phase_id: "00-x".into(),
            round: 3,
            routed: vec![],
        };
        save_amendment(&path, &a).unwrap();
        assert_eq!(next_episode_round(Some(&a)), Err(()));
        reset_replan_round(&path).unwrap();
        let loaded = load_amendment(&path).unwrap();
        assert_eq!(loaded.round, 0);
        assert_eq!(next_episode_round(Some(&loaded)), Ok(1));
        let _ = std::fs::remove_dir_all(&root);
    }

    #[test]
    fn reset_exhausted_row_reenters_ladder_at_slot_one() {
        use crate::services::task_check::{next_attempt, Step};
        let mut file = seed_from_specs("p", "00-x", &[spec("04-d", &[])]);
        file.tasks[0].status = "failed".into();
        file.tasks[0].route = Some("planner".into());
        file.tasks[0].attempts = exhausted_ladder();
        // Sanity: a kept full ladder is exhausted (the bug).
        let kept: Vec<_> = file.tasks[0]
            .attempts
            .iter()
            .map(|r| crate::services::task_check::Attempt {
                label: r.tier.clone(),
                model: r.model.clone(),
                class: Some(crate::services::task_check::FailureClass::Model),
                infra_retry: false,
            })
            .collect();
        assert_eq!(next_attempt(&kept, None), Step::Exhausted);
        reset_for_resume(&mut file, &[]);
        assert!(file.tasks[0].attempts.is_empty());
        match next_attempt(&[], None) {
            Step::Run { label, .. } => assert_eq!(label, "qwen3-coder"),
            other => panic!("expected first slot, got {other:?}"),
        }
    }

    #[test]
    fn missing_amendment_is_none_corrupt_is_err() {
        let root = tmp("amend-opt");
        let path = root.join("amendment.json");
        assert!(load_amendment_optional(&path).unwrap().is_none());
        std::fs::write(&path, "{not-json").unwrap();
        assert!(load_amendment_optional(&path).is_err());
        let _ = std::fs::remove_dir_all(&root);
    }

    #[test]
    fn save_tasks_path_stays_outside_plan_store() {
        let root = tmp("runs");
        let runs = crate::services::settings::initiative_runs_path(&root, "init", "plan", "00-x");
        let mut file = seed_from_specs("plan", "00-x", &[spec("01-a", &[])]);
        save_tasks(&tasks_json_path(&runs), &mut file).unwrap();
        assert!(runs.starts_with(root.join("init").join("runs")));
        assert!(!runs.starts_with(root.join("init").join("plan")));
        let _ = std::fs::remove_dir_all(&root);
    }
}
