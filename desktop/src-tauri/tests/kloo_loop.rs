//! Integration tests for the kloo task loop (phase 04).
//! These run under `cargo test` (all targets), not `cargo test --lib`.
//!
//! Isolation: each test writes a private wrapper that bakes `J1_FAKE_KLOO_*`
//! and is passed as `cli_path`. Nothing mutates process-global PATH or env.

use johnnyone_desktop_lib::db::models::{AgentPlan, AgentPlanPhase};
use johnnyone_desktop_lib::db::Database;
use johnnyone_desktop_lib::services::agent_plans::AgentPlanRun;
use johnnyone_desktop_lib::services::atomic_fs;
use johnnyone_desktop_lib::services::settings::{self, KEY_INITIATIVES_DIR};
use johnnyone_desktop_lib::services::task_loop::{load_phase_specs, run_kloo_phase_ex, PhaseLoopOutcome};
use johnnyone_desktop_lib::services::task_state::{
    load_tasks, save_tasks, seed_from_specs, tasks_json_path,
};
use johnnyone_desktop_lib::services::workspace_git::{self, index_task_commits};
use johnnyone_desktop_lib::state::app_state::AppState;
use std::collections::HashSet;
use std::path::{Path, PathBuf};
use std::process::Command;
use std::sync::atomic::{AtomicU64, Ordering};
use std::time::Duration;

static SEQ: AtomicU64 = AtomicU64::new(0);

fn fixture_root() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures/small-mode-spine")
}

fn tmp(tag: &str) -> PathBuf {
    let d = std::env::temp_dir().join(format!(
        "j1-kloop-{}-{}-{}",
        tag,
        std::process::id(),
        SEQ.fetch_add(1, Ordering::Relaxed)
    ));
    std::fs::create_dir_all(&d).unwrap();
    d
}

fn sh_quote(s: &str) -> String {
    format!("'{}'", s.replace('\'', "'\\''"))
}

#[derive(Default)]
struct FakeKlooOpts {
    tasks_json: Option<PathBuf>,
    assert_running: bool,
    sleep_secs: Option<String>,
    sleep_allow: Option<String>,
    pid_file: Option<PathBuf>,
    no_rewrite: bool,
    model_log: Option<PathBuf>,
}

/// Write a per-test `kloo` wrapper that exports only this test's config and
/// execs the committed fake-kloo.sh. Returns the wrapper path for `cli_path`.
fn write_fake_kloo(bin_dir: &Path, opts: &FakeKlooOpts) -> PathBuf {
    let dest = bin_dir.join("kloo");
    std::fs::create_dir_all(bin_dir).unwrap();
    let script = fixture_root().join("fake-kloo.sh");
    let expected = fixture_root().join("workspace-expected");
    let mut body = String::from("#!/bin/sh\nset -eu\n");
    body.push_str(&format!(
        "export J1_FAKE_KLOO_EXPECTED={}\n",
        sh_quote(&expected.to_string_lossy())
    ));
    if let Some(p) = &opts.tasks_json {
        body.push_str(&format!(
            "export J1_FAKE_KLOO_TASKS_JSON={}\n",
            sh_quote(&p.to_string_lossy())
        ));
    }
    if opts.assert_running {
        body.push_str("export J1_FAKE_KLOO_ASSERT_RUNNING=1\n");
    }
    if let Some(s) = &opts.sleep_secs {
        body.push_str(&format!(
            "export J1_FAKE_KLOO_SLEEP_SECS={}\n",
            sh_quote(s)
        ));
    }
    if let Some(a) = &opts.sleep_allow {
        body.push_str(&format!(
            "export J1_FAKE_KLOO_SLEEP_ALLOW={}\n",
            sh_quote(a)
        ));
    }
    if let Some(p) = &opts.pid_file {
        body.push_str(&format!(
            "export J1_FAKE_KLOO_PID_FILE={}\n",
            sh_quote(&p.to_string_lossy())
        ));
    }
    if opts.no_rewrite {
        body.push_str("export J1_FAKE_KLOO_NO_REWRITE=1\n");
    }
    if let Some(p) = &opts.model_log {
        body.push_str(&format!(
            "export J1_FAKE_KLOO_MODEL_LOG={}\n",
            sh_quote(&p.to_string_lossy())
        ));
    }
    body.push_str(&format!(
        "exec {} \"$@\"\n",
        sh_quote(&script.to_string_lossy())
    ));
    std::fs::write(&dest, body).unwrap();
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        let mut p = std::fs::metadata(&dest).unwrap().permissions();
        p.set_mode(0o755);
        std::fs::set_permissions(&dest, p).unwrap();
    }
    dest
}

fn git_init_and_commit(ws: &Path, msg: &str) {
    let run = |args: &[&str]| {
        let o = Command::new("git").args(args).current_dir(ws).output().unwrap();
        assert!(
            o.status.success(),
            "git {:?} failed: {}",
            args,
            String::from_utf8_lossy(&o.stderr)
        );
    };
    run(&["init"]);
    run(&["config", "user.email", "johnnyone@local"]);
    run(&["config", "user.name", "JohnnyOne"]);
    run(&["add", "-A"]);
    run(&["commit", "-m", msg]);
}

fn dummy_run(ws: &Path, plan: &Path, plan_id: &str, initiative: &str) -> AgentPlanRun {
    let now = "2026-08-14T00:00:00Z".to_string();
    let phase = dummy_phase(plan_id, "00-calc");
    AgentPlanRun {
        plan: AgentPlan {
            id: plan_id.to_string(),
            run_type: "development".into(),
            title: "spine".into(),
            workspace_path: ws.to_string_lossy().into_owned(),
            plan_path: plan.to_string_lossy().into_owned(),
            status: "phase_worker_running".into(),
            worker_session_id: None,
            reviewer_session_id: None,
            worker_provider: "claude_code".into(),
            reviewer_provider: "claude_code".into(),
            current_phase_id: Some("00-calc".into()),
            current_phase_index: 0,
            error: None,
            brief: None,
            app_scope: None,
            docs_scope: None,
            reference_paths: None,
            amend_brief: None,
            phase_run_mode: "continue".into(),
            initiative_id: initiative.to_string(),
            initiative_status: "development".into(),
            health: "in-progress".into(),
            briefing_session_id: None,
            created_at: now.clone(),
            updated_at: now,
            validation_config: None,
            executor_config: None,
            consecutive_non_pass_planning_rounds: None,
        },
        phases: vec![phase],
        tasks: vec![],
        events: vec![],
    }
}

fn dummy_phase(plan_id: &str, phase_id: &str) -> AgentPlanPhase {
    AgentPlanPhase {
        id: format!("{plan_id}-{phase_id}"),
        plan_id: plan_id.to_string(),
        phase_id: phase_id.to_string(),
        phase_title: "calc".into(),
        phase_index: 0,
        status: "worker_running".into(),
        worker_started_at: None,
        worker_idle_at: None,
        reviewer_started_at: None,
        reviewer_idle_at: None,
        gate_verdict: "none".into(),
        clarification_attempts: 0,
        summary: None,
        findings_json: "[]".into(),
        created_at: String::new(),
        updated_at: String::new(),
    }
}

struct Harness {
    root: PathBuf,
    ws: PathBuf,
    plan: PathBuf,
    runs: PathBuf,
    state: AppState,
    plan_id: &'static str,
    initiative: &'static str,
}

fn setup_harness(tag: &str, plan_id: &'static str, initiative: &'static str) -> Harness {
    let root = tmp(tag);
    let ws = root.join("ws");
    let plan = root.join("plan");
    atomic_fs::copy_dir_skipping_target(&fixture_root().join("workspace"), &ws).unwrap();
    atomic_fs::copy_dir_skipping_target(&fixture_root().join("plan"), &plan).unwrap();
    git_init_and_commit(&ws, "stubs");
    let initiatives = root.join("initiatives");
    let db = Database::open(&root.join("db.sqlite")).unwrap();
    let state = AppState::new(db);
    settings::set_setting(
        &state,
        KEY_INITIATIVES_DIR.to_string(),
        initiatives.to_string_lossy().into_owned(),
    )
    .unwrap();
    let runs = settings::initiative_runs_path(&initiatives, initiative, plan_id, "00-calc");
    Harness {
        root,
        ws,
        plan,
        runs,
        state,
        plan_id,
        initiative,
    }
}

fn insert_plan_row(state: &AppState, run: &AgentPlanRun) {
    state
        .db
        .with_conn(|conn| {
            conn.execute(
                "INSERT INTO agent_plans (
                    id, run_type, title, workspace_path, plan_path, status,
                    worker_provider, reviewer_provider, initiative_id, initiative_status
                ) VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10)",
                rusqlite::params![
                    run.plan.id,
                    run.plan.run_type,
                    run.plan.title,
                    run.plan.workspace_path,
                    run.plan.plan_path,
                    run.plan.status,
                    run.plan.worker_provider,
                    run.plan.reviewer_provider,
                    run.plan.initiative_id,
                    run.plan.initiative_status,
                ],
            )
            .map_err(|e| e.to_string())?;
            Ok(())
        })
        .unwrap();
}

fn set_plan_status(state: &AppState, plan_id: &str, status: &str) {
    state
        .db
        .with_conn(|conn| {
            conn.execute(
                "UPDATE agent_plans SET status = ?1, updated_at = datetime('now') WHERE id = ?2",
                rusqlite::params![status, plan_id],
            )
            .map_err(|e| e.to_string())?;
            Ok(())
        })
        .unwrap();
}

async fn run_happy(
    state: &AppState,
    ws: &Path,
    plan: &Path,
    plan_id: &str,
    initiative: &str,
    kloo_cli: Option<String>,
) -> PhaseLoopOutcome {
    let run = dummy_run(ws, plan, plan_id, initiative);
    let phase = dummy_phase(plan_id, "00-calc");
    run_kloo_phase_ex(state, &run, &phase, kloo_cli)
        .await
        .expect("loop")
}

fn spawn_ids_from_log(log: &str) -> HashSet<String> {
    log.lines()
        .filter_map(|l| {
            let id = l.split('\t').next().unwrap_or("").trim();
            if id.is_empty() {
                None
            } else {
                Some(id.to_string())
            }
        })
        .collect()
}

fn resolve_kloo_on_path() -> PathBuf {
    let out = Command::new("sh")
        .args(["-c", "command -v kloo"])
        .output()
        .expect("command -v kloo");
    assert!(
        out.status.success(),
        "J1_KLOO_LIVE=1 but kloo is not on PATH: {}",
        String::from_utf8_lossy(&out.stderr)
    );
    let p = String::from_utf8_lossy(&out.stdout).trim().to_string();
    assert!(!p.is_empty(), "J1_KLOO_LIVE=1 but `command -v kloo` is empty");
    PathBuf::from(p)
}

#[tokio::test(flavor = "multi_thread")]
async fn fixture_loop_with_fake_kloo_completes_all_tasks() {
    let h = setup_harness("fake-all", "plan-fake", "init-fake");
    let cli = write_fake_kloo(
        &h.root.join("bin"),
        &FakeKlooOpts {
            tasks_json: Some(h.runs.join("tasks.json")),
            assert_running: true,
            ..Default::default()
        },
    );
    let out = run_happy(
        &h.state,
        &h.ws,
        &h.plan,
        h.plan_id,
        h.initiative,
        Some(cli.to_string_lossy().into_owned()),
    )
    .await;
    assert_eq!(out, PhaseLoopOutcome::AllDone);
    let file = load_tasks(&tasks_json_path(&h.runs), h.plan_id, "00-calc").unwrap();
    let done: Vec<_> = file.tasks.iter().filter(|t| t.status == "done").collect();
    assert!(done.len() >= 10, "done={}", done.len());
    assert!(done.iter().all(|t| t.commit_sha.is_some()));
    let idx = index_task_commits(&h.ws, "00-calc", None).unwrap();
    for t in &done {
        let sha = t.commit_sha.as_ref().unwrap();
        assert_eq!(
            idx.get(&("00-calc".into(), t.id.clone())),
            Some(sha),
            "sha mismatch {}",
            t.id
        );
    }
    let log = Command::new("git")
        .args(["-C", h.ws.to_str().unwrap(), "log", "--oneline"])
        .output()
        .unwrap();
    let log_text = String::from_utf8_lossy(&log.stdout).into_owned();
    let lines: Vec<_> = log_text
        .lines()
        .filter(|l| l.contains("task 00-calc/"))
        .collect();
    assert_eq!(lines.len(), done.len(), "{lines:?}");
    let _ = std::fs::remove_dir_all(&h.root);
}

#[tokio::test(flavor = "multi_thread")]
async fn live_spine_calc_qwen3() {
    let live = std::env::var("J1_KLOO_LIVE").unwrap_or_default();
    if live.trim().is_empty() {
        eprintln!("skip: J1_KLOO_LIVE not set");
        return;
    }
    let resolved = resolve_kloo_on_path();
    let resolved_s = resolved.to_string_lossy();
    assert!(
        !resolved_s.contains("j1-kloop-"),
        "live test must not use a test fake kloo at {resolved_s}"
    );
    let artifact = std::env::var("J1_PLAN_ARTIFACTS")
        .map(PathBuf::from)
        .expect("J1_PLAN_ARTIFACTS must be set when J1_KLOO_LIVE=1");

    let h = setup_harness("live", "plan-live", "init-live");
    let out = tokio::time::timeout(
        Duration::from_secs(3600),
        run_happy(
            &h.state,
            &h.ws,
            &h.plan,
            h.plan_id,
            h.initiative,
            Some(resolved_s.into_owned()),
        ),
    )
    .await
    .expect("live loop timed out");
    let file = load_tasks(&tasks_json_path(&h.runs), h.plan_id, "00-calc").unwrap();
    std::fs::create_dir_all(&artifact).unwrap();
    let log = Command::new("git")
        .args(["-C", h.ws.to_str().unwrap(), "log", "--oneline"])
        .output()
        .unwrap();
    let mut body = String::from("upper tiers unverified live.\n\n");
    body.push_str("## git log --oneline\n");
    body.push_str(&String::from_utf8_lossy(&log.stdout));
    body.push_str("\n## tasks\n");
    body.push_str("id\tsucceededTier\tcommitSha\tattempts\n");
    for t in &file.tasks {
        body.push_str(&format!(
            "{}\t{}\t{}\t{}\n",
            t.id,
            t.succeeded_tier.as_deref().unwrap_or("-"),
            t.commit_sha.as_deref().unwrap_or("-"),
            t.attempts.len()
        ));
    }
    std::fs::write(artifact.join("live-run.txt"), &body).unwrap();

    match out {
        PhaseLoopOutcome::AllDone => {}
        other => panic!("live loop not AllDone: {other:?}"),
    }
    let happy: Vec<_> = file
        .tasks
        .iter()
        .filter(|t| t.id.chars().next().unwrap().is_ascii_digit())
        .collect();
    assert!(happy.len() >= 10, "need ≥10 tasks, got {}", happy.len());
    for t in &happy {
        assert_eq!(t.status, "done", "{}", t.id);
        assert_eq!(
            t.succeeded_tier.as_deref(),
            Some("qwen3-coder"),
            "{} escalated — acceptance 3 fail",
            t.id
        );
        assert!(t.commit_sha.is_some(), "{} missing sha", t.id);
    }
    let idx = index_task_commits(&h.ws, "00-calc", None).unwrap();
    for t in &happy {
        assert_eq!(
            idx.get(&("00-calc".into(), t.id.clone())),
            t.commit_sha.as_ref()
        );
    }
    let _ = std::fs::remove_dir_all(&h.root);
}

#[tokio::test(flavor = "multi_thread")]
async fn resume_after_killed_child_reruns_only_uncommitted() {
    let h = setup_harness("kill-resume", "plan-kill", "init-kill");
    let pid_file = h.root.join("kloo.pid");
    let model_log = h.root.join("models.log");
    let cli = write_fake_kloo(
        &h.root.join("bin"),
        &FakeKlooOpts {
            tasks_json: Some(h.runs.join("tasks.json")),
            assert_running: true,
            sleep_allow: Some("src/saturate.rs".into()),
            sleep_secs: Some("30".into()),
            pid_file: Some(pid_file.clone()),
            model_log: Some(model_log.clone()),
            ..Default::default()
        },
    );
    let run_row = dummy_run(&h.ws, &h.plan, h.plan_id, h.initiative);
    insert_plan_row(&h.state, &run_row);

    let state2 = h.state.clone();
    let ws2 = h.ws.clone();
    let plan2 = h.plan.clone();
    let plan_id = h.plan_id;
    let initiative = h.initiative;
    let cli2 = cli.to_string_lossy().into_owned();
    let handle = tokio::spawn(async move {
        run_happy(&state2, &ws2, &plan2, plan_id, initiative, Some(cli2)).await
    });

    let deadline = std::time::Instant::now() + Duration::from_secs(60);
    while !pid_file.exists() && std::time::Instant::now() < deadline {
        tokio::time::sleep(Duration::from_millis(50)).await;
    }
    assert!(pid_file.exists(), "fake-kloo never wrote pid file");
    let pid: u32 = std::fs::read_to_string(&pid_file)
        .unwrap()
        .trim()
        .parse()
        .unwrap();
    // Mark stopped first so the post-wait cancel check returns Cancelled
    // instead of recording the kill as an infra failure and retrying 06.
    set_plan_status(&h.state, h.plan_id, "stopped");
    let killed = johnnyone_desktop_lib::services::task_loop::kill_process_group(pid);
    assert!(
        killed,
        "kill_process_group({pid}) returned false — child already gone"
    );
    let first = handle.await.unwrap();
    assert!(
        !matches!(first, PhaseLoopOutcome::AllDone),
        "first entry finished AllDone — kill did not interrupt: {first:?}"
    );
    assert!(
        matches!(first, PhaseLoopOutcome::Cancelled),
        "first entry should be Cancelled after stop+kill, got {first:?}"
    );

    let after_kill = if model_log.exists() {
        std::fs::read_to_string(&model_log).unwrap()
    } else {
        String::new()
    };

    // Re-enter with the same wrapper minus the sleep so 06 finishes.
    let cli_resume = write_fake_kloo(
        &h.root.join("bin"),
        &FakeKlooOpts {
            tasks_json: Some(h.runs.join("tasks.json")),
            assert_running: true,
            model_log: Some(model_log.clone()),
            ..Default::default()
        },
    );
    set_plan_status(&h.state, h.plan_id, "phase_worker_running");
    let out2 = run_happy(
        &h.state,
        &h.ws,
        &h.plan,
        h.plan_id,
        h.initiative,
        Some(cli_resume.to_string_lossy().into_owned()),
    )
    .await;
    assert_eq!(out2, PhaseLoopOutcome::AllDone);
    let after_resume = std::fs::read_to_string(&model_log).unwrap_or_default();
    let new_log = after_resume
        .strip_prefix(&after_kill)
        .unwrap_or(&after_resume);
    let first_ids = spawn_ids_from_log(&after_kill);
    let spawned = spawn_ids_from_log(new_log);
    assert!(
        spawned.contains("06-saturate"),
        "interrupted task 06 must be spawned again, got {spawned:?}\nnew_log={new_log:?}"
    );
    for id in first_ids.iter().filter(|id| id.as_str() != "06-saturate") {
        assert!(
            !spawned.contains(id),
            "{id} was already committed/spawned and must not be re-spawned; second={spawned:?}"
        );
    }
    let file = load_tasks(&tasks_json_path(&h.runs), h.plan_id, "00-calc").unwrap();
    assert!(file.tasks.iter().all(|t| t.status == "done"));
    let _ = std::fs::remove_dir_all(&h.root);
}

#[tokio::test(flavor = "multi_thread")]
async fn resume_already_correct_residue_commits_without_escalate() {
    let h = setup_harness("residue", "plan-res", "init-res");
    std::fs::copy(
        fixture_root().join("workspace-expected/src/saturate.rs"),
        h.ws.join("src/saturate.rs"),
    )
    .unwrap();
    let cli = write_fake_kloo(
        &h.root.join("bin"),
        &FakeKlooOpts {
            tasks_json: Some(h.runs.join("tasks.json")),
            ..Default::default()
        },
    );
    let _ = run_happy(
        &h.state,
        &h.ws,
        &h.plan,
        h.plan_id,
        h.initiative,
        Some(cli.to_string_lossy().into_owned()),
    )
    .await;
    let path = tasks_json_path(&h.runs);
    let mut file = load_tasks(&path, h.plan_id, "00-calc").unwrap();
    let six = file.tasks.iter_mut().find(|t| t.id == "06-saturate").unwrap();
    six.status = "running".into();
    six.commit_sha = None;
    six.succeeded_tier = None;
    six.route = None;
    save_tasks(&path, &mut file).unwrap();
    // Residue on 06 only: later tasks stay done. Re-copy expected saturate
    // so verify is green (first run already wrote it).
    std::fs::copy(
        fixture_root().join("workspace-expected/src/saturate.rs"),
        h.ws.join("src/saturate.rs"),
    )
    .unwrap();
    let model_log = h.root.join("models.log");
    let cli_residue = write_fake_kloo(
        &h.root.join("bin"),
        &FakeKlooOpts {
            tasks_json: Some(h.runs.join("tasks.json")),
            assert_running: true,
            no_rewrite: true,
            model_log: Some(model_log.clone()),
            ..Default::default()
        },
    );
    let before = if model_log.exists() {
        std::fs::read_to_string(&model_log).unwrap()
    } else {
        String::new()
    };
    let out = run_happy(
        &h.state,
        &h.ws,
        &h.plan,
        h.plan_id,
        h.initiative,
        Some(cli_residue.to_string_lossy().into_owned()),
    )
    .await;
    assert_eq!(out, PhaseLoopOutcome::AllDone);
    let after = if model_log.exists() {
        std::fs::read_to_string(&model_log).unwrap()
    } else {
        String::new()
    };
    assert_eq!(after, before, "residue must not spawn kloo");
    let loaded = load_tasks(&path, h.plan_id, "00-calc").unwrap();
    let six = loaded.tasks.iter().find(|t| t.id == "06-saturate").unwrap();
    assert_eq!(six.status, "done");
    assert!(six.commit_sha.is_some());
    let _ = std::fs::remove_dir_all(&h.root);
}

#[tokio::test(flavor = "multi_thread")]
async fn resume_preseed_skips_committed() {
    let h = setup_harness("preseed", "plan-pre", "init-pre");
    let expected = fixture_root().join("workspace-expected");
    let seeded = [
        ("01-add", "src/add.rs"),
        ("02-sub", "src/sub.rs"),
        ("03-mul", "src/mul.rs"),
        ("04-div", "src/div.rs"),
        ("05-clamp", "src/clamp.rs"),
    ];
    let mut shas = std::collections::HashMap::new();
    for (id, rel) in seeded {
        std::fs::copy(expected.join(rel), h.ws.join(rel)).unwrap();
        let sha = workspace_git::commit_task(
            &h.ws,
            "00-calc",
            id,
            &workspace_git::task_slug(id),
            &[PathBuf::from(rel)],
        )
        .unwrap()
        .expect("commit seeded task");
        shas.insert(id.to_string(), sha);
    }

    let specs = load_phase_specs(&h.plan.join("phases/00-calc")).unwrap();
    let mut file = seed_from_specs(h.plan_id, "00-calc", &specs);
    for t in &mut file.tasks {
        if let Some(sha) = shas.get(&t.id) {
            t.status = "done".into();
            t.commit_sha = Some(sha.clone());
            t.succeeded_tier = Some("qwen3-coder".into());
        } else if t.id == "06-saturate" {
            t.status = "running".into();
            t.commit_sha = None;
        }
    }
    save_tasks(&tasks_json_path(&h.runs), &mut file).unwrap();

    let model_log = h.root.join("models.log");
    let cli = write_fake_kloo(
        &h.root.join("bin"),
        &FakeKlooOpts {
            tasks_json: Some(h.runs.join("tasks.json")),
            assert_running: true,
            model_log: Some(model_log.clone()),
            ..Default::default()
        },
    );
    let out = run_happy(
        &h.state,
        &h.ws,
        &h.plan,
        h.plan_id,
        h.initiative,
        Some(cli.to_string_lossy().into_owned()),
    )
    .await;
    assert_eq!(out, PhaseLoopOutcome::AllDone);
    let spawned = spawn_ids_from_log(&std::fs::read_to_string(&model_log).unwrap_or_default());
    for id in ["01-add", "02-sub", "03-mul", "04-div", "05-clamp"] {
        assert!(
            !spawned.contains(id),
            "pre-seeded {id} must not be spawned, got {spawned:?}"
        );
    }
    assert!(
        spawned.contains("06-saturate"),
        "running 06 with no commit must be spawned, got {spawned:?}"
    );
    let loaded = load_tasks(&tasks_json_path(&h.runs), h.plan_id, "00-calc").unwrap();
    assert!(loaded.tasks.iter().all(|t| t.status == "done"));
    let _ = std::fs::remove_dir_all(&h.root);
}

#[test]
fn commercial_phase_without_task_yml() {
    use johnnyone_desktop_lib::services::task_loop::phase_is_kloo_mode;
    let d = tmp("commercial");
    std::fs::create_dir_all(d.join("tasks/01-y")).unwrap();
    std::fs::write(d.join("tasks/01-y/prompt.md"), "# Y\n").unwrap();
    assert!(!phase_is_kloo_mode(&d));
    let _ = std::fs::remove_dir_all(&d);
}

#[test]
fn fake_kloo_script_has_no_https() {
    let s = std::fs::read_to_string(fixture_root().join("fake-kloo.sh")).unwrap();
    assert!(!s.contains("https://"));
    assert!(
        !s.contains("python3"),
        "fake-kloo must not shell out to python3"
    );
}
