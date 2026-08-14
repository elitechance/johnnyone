//! Phase 06 acceptance: planner-emitted plan (no checked-in task.yml) +
//! fake-kloo loop + routed-task amendment reaching AllDone.

use johnnyone_desktop_lib::db::models::{AgentPlan, AgentPlanPhase};
use johnnyone_desktop_lib::db::Database;
use johnnyone_desktop_lib::services::agent_plans::AgentPlanRun;
use johnnyone_desktop_lib::services::atomic_fs;
use johnnyone_desktop_lib::services::plan_check::{
    check_plan, check_plan_with, CheckPlanOpts, PlanCheckHost, SpawnRequest, SpawnResult,
    TokensError, TokensReport, RULE_FILE_COLLISION,
};
use johnnyone_desktop_lib::services::settings::{self, KEY_INITIATIVES_DIR};
use johnnyone_desktop_lib::services::task_loop::{load_phase_specs, run_kloo_phase_ex, PhaseLoopOutcome};
use johnnyone_desktop_lib::services::task_replan::{
    amendment_json_path, build_amendment, reset_for_resume, save_amendment,
};
use johnnyone_desktop_lib::services::task_state::{
    load_tasks, save_tasks, seed_from_specs, tasks_json_path, AttemptRecord,
};
use johnnyone_desktop_lib::services::workspace_git::{commit_task, index_task_commits, task_slug};
use johnnyone_desktop_lib::state::app_state::AppState;
use std::path::{Path, PathBuf};
use std::process::Command;
use std::sync::atomic::{AtomicU64, Ordering};

static SEQ: AtomicU64 = AtomicU64::new(0);

const PHASE: &str = "00-calc";

const TASKS: &[(&str, &str, &str, &str)] = &[
    ("01-add", "src/add.rs", "add_works", "pub fn add(a: i32, b: i32) -> i32"),
    ("02-sub", "src/sub.rs", "sub_works", "pub fn sub(a: i32, b: i32) -> i32"),
    ("03-mul", "src/mul.rs", "mul_works", "pub fn mul(a: i32, b: i32) -> i32"),
    ("04-div", "src/div.rs", "div_works", "pub fn div(a: i32, b: i32) -> i32"),
    ("05-clamp", "src/clamp.rs", "clamp_works", "pub fn clamp(x: i32, lo: i32, hi: i32) -> i32"),
];

fn spine_workspace() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures/small-mode-spine/workspace")
}

fn usable_fixture() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures/small-mode-usable")
}

fn spine_expected() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("tests/fixtures/small-mode-spine/workspace-expected")
}

fn collect_yml(dir: &Path, out: &mut Vec<PathBuf>) {
    let Ok(rd) = std::fs::read_dir(dir) else {
        return;
    };
    for e in rd.flatten() {
        let p = e.path();
        if p.is_dir() {
            collect_yml(&p, out);
        } else if p.file_name().and_then(|n| n.to_str()) == Some("task.yml") {
            out.push(p);
        }
    }
}

struct TempRoot(PathBuf);

impl TempRoot {
    fn new(tag: &str) -> Self {
        let d = std::env::temp_dir().join(format!(
            "j1-smacc-{}-{}-{}",
            tag,
            std::process::id(),
            SEQ.fetch_add(1, Ordering::Relaxed)
        ));
        std::fs::create_dir_all(&d).unwrap();
        Self(d)
    }
}

impl std::ops::Deref for TempRoot {
    type Target = PathBuf;
    fn deref(&self) -> &PathBuf {
        &self.0
    }
}

impl Drop for TempRoot {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.0);
    }
}

fn tmp(tag: &str) -> TempRoot {
    TempRoot::new(tag)
}

fn sh_quote(s: &str) -> String {
    format!("'{}'", s.replace('\'', "'\\''"))
}

/// Writes a methodology-shaped plan (≥5 task.yml dirs). The committed fixture
/// has none of these files.
fn fake_planner_emit(plan: &Path) {
    std::fs::create_dir_all(plan.join("phases").join(PHASE).join("tasks")).unwrap();
    std::fs::write(
        plan.join("overview.md"),
        "# usable\n\nFive arithmetic tasks, one phase.\n",
    )
    .unwrap();
    std::fs::write(
        plan.join("status.md"),
        "# Status\n\n| id | goal | tasks |\n|---|---|---|\n| 00-calc | implement arithmetic | 5 |\n",
    )
    .unwrap();
    std::fs::write(
        plan.join("phases").join(PHASE).join("overview.md"),
        "# 00-calc\n\n| id | goal |\n|---|---|\n| 01-add | add |\n| 02-sub | sub |\n| 03-mul | mul |\n| 04-div | div |\n| 05-clamp | clamp |\n",
    )
    .unwrap();
    for (id, file, filter, needle) in TASKS {
        let dir = plan.join("phases").join(PHASE).join("tasks").join(id);
        std::fs::create_dir_all(&dir).unwrap();
        std::fs::write(
            dir.join("task.yml"),
            format!(
                "id: {id}\nfiles: [{file}]\nverify: \"cargo test {filter} -- --exact\"\nmust_contain: [\"{needle}\"]\ndepends_on: []\nctx: 32768\n"
            ),
        )
        .unwrap();
        std::fs::write(dir.join("prompt.md"), format!("Implement {id} in {file}.\n")).unwrap();
    }
}

struct InstantHost;

impl PlanCheckHost for InstantHost {
    fn spawn(&self, _req: &SpawnRequest) -> SpawnResult {
        SpawnResult::ok_zero()
    }
    fn tokens(&self, ctx: u32, _prompt_text: &str) -> Result<TokensReport, TokensError> {
        Ok(TokensReport {
            fits: true,
            approx_tokens: Some(8),
            usable_window: Some(ctx as u64),
            compact_trigger: Some(1),
        })
    }
}

fn check_emitted(plan: &Path, ws: &Path) -> johnnyone_desktop_lib::services::plan_check::PlanCheckReport {
    let host = InstantHost;
    let mut opts = CheckPlanOpts::full(None);
    opts.host = Some(&host);
    check_plan_with(plan, ws, &opts)
}

fn write_fake_kloo(bin_dir: &Path, tasks_json: &Path) -> PathBuf {
    write_fake_kloo_ex(bin_dir, tasks_json, None, None)
}

fn write_simple_expected(dir: &Path) {
    std::fs::create_dir_all(dir.join("src")).unwrap();
    std::fs::write(
        dir.join("src/add.rs"),
        "pub fn add(a: i32, b: i32) -> i32 { a + b }\n",
    )
    .unwrap();
    std::fs::write(
        dir.join("src/sub.rs"),
        "pub fn sub(a: i32, b: i32) -> i32 { a - b }\n",
    )
    .unwrap();
    std::fs::write(
        dir.join("src/mul.rs"),
        "pub fn mul(a: i32, b: i32) -> i32 { a * b }\n",
    )
    .unwrap();
    std::fs::write(
        dir.join("src/div.rs"),
        "pub fn div(a: i32, b: i32) -> i32 { a / b }\n",
    )
    .unwrap();
    std::fs::write(
        dir.join("src/clamp.rs"),
        "pub fn clamp(x: i32, lo: i32, hi: i32) -> i32 { if x < lo { lo } else if x > hi { hi } else { x } }\n",
    )
    .unwrap();
}

fn write_fake_kloo_ex(
    bin_dir: &Path,
    tasks_json: &Path,
    id_map: Option<&Path>,
    expected: Option<&Path>,
) -> PathBuf {
    let dest = bin_dir.join("kloo");
    std::fs::create_dir_all(bin_dir).unwrap();
    let script = PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("tests/fixtures/small-mode-spine/fake-kloo.sh");
    let default_expected = spine_expected();
    let expected = expected.unwrap_or(&default_expected);
    let mut body = String::from("#!/bin/sh\nset -eu\n");
    body.push_str(&format!(
        "export J1_FAKE_KLOO_EXPECTED={}\n",
        sh_quote(&expected.to_string_lossy())
    ));
    body.push_str(&format!(
        "export J1_FAKE_KLOO_TASKS_JSON={}\n",
        sh_quote(&tasks_json.to_string_lossy())
    ));
    if let Some(map) = id_map {
        body.push_str(&format!(
            "export J1_FAKE_KLOO_ID_MAP={}\n",
            sh_quote(&map.to_string_lossy())
        ));
    }
    body.push_str("export J1_FAKE_KLOO_ASSERT_RUNNING=1\n");
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
    dummy_run_for(ws, plan, plan_id, initiative, PHASE)
}

fn dummy_run_for(
    ws: &Path,
    plan: &Path,
    plan_id: &str,
    initiative: &str,
    phase: &str,
) -> AgentPlanRun {
    let now = "2026-08-14T00:00:00Z".to_string();
    AgentPlanRun {
        plan: AgentPlan {
            id: plan_id.to_string(),
            run_type: "development".into(),
            title: "usable".into(),
            workspace_path: ws.to_string_lossy().into_owned(),
            plan_path: plan.to_string_lossy().into_owned(),
            status: "phase_worker_running".into(),
            worker_session_id: None,
            reviewer_session_id: None,
            worker_provider: "claude_code".into(),
            reviewer_provider: "claude_code".into(),
            current_phase_id: Some(phase.into()),
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
            executor_config: Some(r#"{"mode":"local-small"}"#.into()),
            consecutive_non_pass_planning_rounds: None,
        },
        phases: vec![AgentPlanPhase {
            id: format!("{plan_id}-{phase}"),
            plan_id: plan_id.to_string(),
            phase_id: phase.to_string(),
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
        }],
        tasks: vec![],
        events: vec![],
    }
}

fn setup(tag: &str) -> (TempRoot, PathBuf, PathBuf, AppState, PathBuf) {
    let root = tmp(tag);
    let ws = root.join("ws");
    let plan = root.join("plan");
    atomic_fs::copy_dir_skipping_target(&spine_workspace(), &ws).unwrap();
    fake_planner_emit(&plan);
    git_init_and_commit(&ws, "stubs");
    let db = Database::open(&root.join("db.sqlite")).unwrap();
    let state = AppState::new(db);
    settings::set_setting(
        &state,
        KEY_INITIATIVES_DIR.to_string(),
        root.join("initiatives").to_string_lossy().into_owned(),
    )
    .unwrap();
    let initiatives = root.join("initiatives");
    (root, ws, plan, state, initiatives)
}

#[test]
fn fixture_repo_has_no_task_yml() {
    // Trees the tests actually copy (usable README-only + spine workspace).
    // Initiative-1's sibling `small-mode-spine/plan` is intentionally not
    // copied; walking it would hide a regression in the copied trees.
    for root in [usable_fixture(), spine_workspace()] {
        assert!(root.is_dir(), "{}", root.display());
        let mut found = Vec::new();
        collect_yml(&root, &mut found);
        assert!(
            found.is_empty(),
            "copied fixture tree has task.yml: {found:?}"
        );
    }
}

#[test]
fn fake_planner_writes_allowlisted_specs() {
    let root = tmp("emit");
    let ws = root.join("ws");
    let plan = root.join("plan");
    atomic_fs::copy_dir_skipping_target(&spine_workspace(), &ws).unwrap();
    std::fs::create_dir_all(&plan).unwrap();
    let mut before = Vec::new();
    collect_yml(&plan, &mut before);
    assert!(before.is_empty(), "plan already had task.yml: {before:?}");
    fake_planner_emit(&plan);
    let report = check_emitted(&plan, &ws);
    assert!(report.passed, "{:?}", report.items);
    assert!(
        !report.items.iter().any(|i| i.blocking),
        "{:?}",
        report.items
    );
    let status = std::fs::read_to_string(plan.join("status.md")).unwrap();
    assert!(status.contains("00-calc"), "{status}");
    assert!(
        !status.contains("01-add"),
        "top-level status.md must list phases, not tasks: {status}"
    );
    let overview = std::fs::read_to_string(plan.join("phases").join(PHASE).join("overview.md")).unwrap();
    assert!(overview.contains("01-add"));
    assert!(!overview.contains("src/add.rs"), "{overview}");
    assert!(!overview.contains("cargo test"), "{overview}");
}

#[tokio::test(flavor = "multi_thread")]
async fn emitted_plan_loop_completes() {
    let (root, ws, plan, state, initiatives) = setup("loop");
    let plan_id = "plan-emit";
    let initiative = "init-emit";
    let runs = settings::initiative_runs_path(&initiatives, initiative, plan_id, PHASE);
    let cli = write_fake_kloo(&root.join("bin"), &runs.join("tasks.json"));
    let run = dummy_run(&ws, &plan, plan_id, initiative);
    let phase = run.phases[0].clone();
    let out = run_kloo_phase_ex(&state, &run, &phase, Some(cli.to_string_lossy().into_owned()))
        .await
        .expect("loop");
    assert_eq!(out, PhaseLoopOutcome::AllDone);
    let file = load_tasks(&tasks_json_path(&runs), plan_id, PHASE).unwrap();
    assert_eq!(file.tasks.len(), 5);
    assert!(file.tasks.iter().all(|t| t.status == "done"));
    assert!(file.tasks.iter().all(|t| t.commit_sha.is_some()));
    let log = Command::new("git")
        .args(["-C", ws.to_str().unwrap(), "log", "--oneline"])
        .output()
        .unwrap();
    let text = String::from_utf8_lossy(&log.stdout);
    for t in &file.tasks {
        assert!(
            text.contains(&format!("task {PHASE}/{}:", t.id)),
            "missing commit for {}: {text}",
            t.id
        );
    }
}

fn commit_expected_file(ws: &Path, id: &str, rel: &str) -> String {
    let src = spine_expected().join(rel);
    std::fs::copy(&src, ws.join(rel)).unwrap();
    let slug = task_slug(id);
    commit_task(ws, PHASE, id, &slug, &[Path::new(rel)])
        .unwrap()
        .expect("sibling commit")
}

fn git_rev_exists(ws: &Path, sha: &str) -> bool {
    Command::new("git")
        .args(["-C", ws.to_str().unwrap(), "cat-file", "-t", sha])
        .output()
        .map(|o| o.status.success())
        .unwrap_or(false)
}

#[tokio::test(flavor = "multi_thread")]
async fn routed_task_amendment_reaches_done_and_alldone() {
    let (root, ws, plan, state, initiatives) = setup("replan");
    let plan_id = "plan-replan";
    let initiative = "init-replan";
    let sha_add = commit_expected_file(&ws, "01-add", "src/add.rs");
    let sha_sub = commit_expected_file(&ws, "02-sub", "src/sub.rs");
    let runs = settings::initiative_runs_path(&initiatives, initiative, plan_id, PHASE);
    let specs = load_phase_specs(&plan.join("phases").join(PHASE)).unwrap();
    let mut file = seed_from_specs(plan_id, PHASE, &specs);
    file.tasks[0].status = "done".into();
    file.tasks[0].commit_sha = Some(sha_add.clone());
    file.tasks[1].status = "done".into();
    file.tasks[1].commit_sha = Some(sha_sub.clone());
    file.tasks[2].status = "failed".into();
    file.tasks[2].route = Some("planner".into());
    file.tasks[2].attempts.push(AttemptRecord {
        tier: "opus".into(),
        model: "opus".into(),
        attempt: 1,
        started_at: None,
        ended_at: None,
        failure_code: Some("verify_failed".into()),
        exit_code: Some(1),
        class: Some("model".into()),
        checks: None,
    });
    file.tasks[3].status = "blocked".into();
    file.tasks[3].blocked_by = Some("03-mul".into());
    file.tasks[4].status = "blocked".into();
    file.tasks[4].blocked_by = Some("03-mul".into());
    save_tasks(&tasks_json_path(&runs), &mut file).unwrap();

    let amendment = build_amendment(plan_id, PHASE, 1, &file);
    assert_eq!(amendment.routed.len(), 1);
    assert_eq!(amendment.routed[0].task_id, "03-mul");
    save_amendment(&amendment_json_path(&runs), &amendment).unwrap();

    // D8: routed task may also claim a done sibling's file.
    let mul_dir = plan.join("phases").join(PHASE).join("tasks/03-mul");
    std::fs::write(
        mul_dir.join("task.yml"),
        "id: 03-mul\nfiles: [src/mul.rs, src/add.rs]\nverify: \"cargo test mul_works -- --exact\"\nmust_contain: [\"pub fn mul(a: i32, b: i32) -> i32\"]\ndepends_on: []\nctx: 32768\n",
    )
    .unwrap();
    let file = load_tasks(&tasks_json_path(&runs), plan_id, PHASE).unwrap();
    let report = check_plan(&plan, &ws, Some(&file));
    assert!(
        !report.items.iter().any(|i| i.rule == RULE_FILE_COLLISION),
        "D8 collision: {:?}",
        report.items
    );

    let specs = load_phase_specs(&plan.join("phases").join(PHASE)).unwrap();
    let mut file = load_tasks(&tasks_json_path(&runs), plan_id, PHASE).unwrap();
    reset_for_resume(&mut file, &specs);
    save_tasks(&tasks_json_path(&runs), &mut file).unwrap();
    assert_eq!(file.tasks[0].commit_sha.as_deref(), Some(sha_add.as_str()));
    assert_eq!(file.tasks[1].commit_sha.as_deref(), Some(sha_sub.as_str()));
    assert_eq!(file.tasks[2].status, "pending");

    let cli = write_fake_kloo(&root.join("bin"), &runs.join("tasks.json"));
    let run = dummy_run(&ws, &plan, plan_id, initiative);
    let phase = run.phases[0].clone();
    let out = run_kloo_phase_ex(&state, &run, &phase, Some(cli.to_string_lossy().into_owned()))
        .await
        .expect("loop");
    assert_eq!(out, PhaseLoopOutcome::AllDone);
    let after = load_tasks(&tasks_json_path(&runs), plan_id, PHASE).unwrap();
    let mul = after.tasks.iter().find(|t| t.id == "03-mul").unwrap();
    assert_eq!(mul.status, "done");
    assert!(mul.commit_sha.is_some());
    assert_ne!(mul.commit_sha.as_deref(), Some(sha_add.as_str()));
    assert_eq!(
        after.tasks.iter().find(|t| t.id == "01-add").unwrap().commit_sha.as_deref(),
        Some(sha_add.as_str())
    );
    assert_eq!(
        after.tasks.iter().find(|t| t.id == "02-sub").unwrap().commit_sha.as_deref(),
        Some(sha_sub.as_str())
    );
    assert!(amendment_json_path(&runs).is_file());
    assert!(git_rev_exists(&ws, &sha_add), "01-add commit missing: {sha_add}");
    assert!(git_rev_exists(&ws, &sha_sub), "02-sub commit missing: {sha_sub}");
    let indexed = index_task_commits(&ws, PHASE, None).unwrap();
    assert_eq!(
        indexed.get(&(PHASE.to_string(), "01-add".into())),
        Some(&sha_add),
        "git log rewritten 01-add: {indexed:?}"
    );
    assert_eq!(
        indexed.get(&(PHASE.to_string(), "02-sub".into())),
        Some(&sha_sub),
        "git log rewritten 02-sub: {indexed:?}"
    );
}

#[test]
fn done_sibling_sha_unchanged() {
    let root = tmp("sha");
    let ws = root.join("ws");
    let plan = root.join("plan");
    atomic_fs::copy_dir_skipping_target(&spine_workspace(), &ws).unwrap();
    fake_planner_emit(&plan);
    git_init_and_commit(&ws, "stubs");
    let sha_add = commit_expected_file(&ws, "01-add", "src/add.rs");
    let specs = load_phase_specs(&plan.join("phases").join(PHASE)).unwrap();
    let mut file = seed_from_specs("p", PHASE, &specs);
    file.tasks[0].status = "done".into();
    file.tasks[0].commit_sha = Some(sha_add.clone());
    file.tasks[2].status = "failed".into();
    reset_for_resume(&mut file, &specs);
    assert_eq!(file.tasks[0].commit_sha.as_deref(), Some(sha_add.as_str()));
    assert_eq!(file.tasks[0].status, "done");
    assert!(git_rev_exists(&ws, &sha_add));
    let indexed = index_task_commits(&ws, PHASE, None).unwrap();
    assert_eq!(
        indexed.get(&(PHASE.to_string(), "01-add".into())),
        Some(&sha_add)
    );
}

fn first_phase_id(plan: &Path) -> String {
    let mut dirs: Vec<_> = std::fs::read_dir(plan.join("phases"))
        .unwrap()
        .filter_map(|e| e.ok())
        .filter(|e| e.path().is_dir())
        .map(|e| e.file_name().to_string_lossy().into_owned())
        .collect();
    dirs.sort();
    dirs.into_iter()
        .next()
        .expect("planner emitted no phase dirs")
}

fn write_id_map_from_plan(plan: &Path, phase: &str, dest: &Path) {
    let specs = load_phase_specs(&plan.join("phases").join(phase)).unwrap_or_default();
    let mut body = String::new();
    for spec in specs {
        if let Some(f) = spec.files.first() {
            body.push_str(&format!("{}={}\n", f, spec.id));
        }
    }
    std::fs::write(dest, body).unwrap();
}

fn check_live(plan: &Path, ws: &Path) -> johnnyone_desktop_lib::services::plan_check::PlanCheckReport {
    // Real spawn + real kloo tokens. No InstantHost.
    check_plan_with(plan, ws, &CheckPlanOpts::full(None))
}

/// Required 06-05 / acceptance 4. Spawn the commercial planner (claude) with
/// the real small-mode planner prompt. Fake-kloo is allowed for the loop step.
#[tokio::test(flavor = "multi_thread")]
async fn live_small_mode_planner_emits_and_loop() {
    if std::env::var("J1_SMALL_MODE_LIVE").unwrap_or_default().trim().is_empty() {
        eprintln!("skip: J1_SMALL_MODE_LIVE not set");
        return;
    }
    let artifact_dir = std::env::var("J1_PLAN_ARTIFACTS")
        .map(PathBuf::from)
        .expect("J1_PLAN_ARTIFACTS must be set when J1_SMALL_MODE_LIVE=1");
    let claude = Command::new("sh")
        .args(["-c", "command -v claude"])
        .output()
        .expect("command -v claude");
    assert!(
        claude.status.success(),
        "planner CLI cannot be spawned: claude not on PATH"
    );
    let kloo_ok = Command::new("sh")
        .args(["-c", "command -v kloo"])
        .output()
        .map(|o| o.status.success())
        .unwrap_or(false);
    assert!(kloo_ok, "kloo not on PATH — live check needs real tokens");

    let root = tmp("live-plan");
    let ws = root.join("ws");
    let plan = root.join("plan");
    atomic_fs::copy_dir_skipping_target(&spine_workspace(), &ws).unwrap();
    std::fs::create_dir_all(&plan).unwrap();
    git_init_and_commit(&ws, "stubs");
    let mut pre = Vec::new();
    collect_yml(&plan, &mut pre);
    assert!(pre.is_empty(), "scratch plan already had task.yml: {pre:?}");

    let settings = johnnyone_desktop_lib::services::planner_prompts::load_prompt_settings()
        .unwrap_or_else(|_| {
            johnnyone_desktop_lib::services::planner_prompts::PlannerPromptSettings::default()
        });
    let template = settings.small_mode.planner;
    let user_brief = "Plan ONE phase 00-calc for this rust crate. Implement the five stub functions add, sub, mul, div, and clamp under src/ — one task per file, each independently verifiable with that file's existing unit test. Do not implement saturate, gcd, lcm, min, or max. Write overview.md, status.md (phases not tasks), a phase overview (id + one-line goal only), and each task.yml + prompt.md. Then say READY_FOR_T2_PLAN_REVIEW.".to_string();
    let prompt = johnnyone_desktop_lib::services::planner_prompts::render_template(
        &template,
        &[
            ("run_id", "live-a4".into()),
            ("workspace_path", ws.to_string_lossy().into_owned()),
            ("app_scope", ws.to_string_lossy().into_owned()),
            ("docs_scope", String::new()),
            ("plan_output_path", plan.to_string_lossy().into_owned()),
            ("methodology_path", String::new()),
            ("conventions_path", String::new()),
            ("user_brief", user_brief.clone()),
            ("reference_paths", String::new()),
        ],
    );

    let claude_out = Command::new("claude")
        .args([
            "-p",
            &prompt,
            "--add-dir",
            &plan.to_string_lossy(),
            "--allowedTools",
            "Read,Write,Edit,Bash",
        ])
        .current_dir(&ws)
        .output()
        .expect("spawn claude planner");
    assert!(
        claude_out.status.success(),
        "planner CLI failed: {}\nstdout: {}\nstderr: {}",
        claude_out.status,
        String::from_utf8_lossy(&claude_out.stdout),
        String::from_utf8_lossy(&claude_out.stderr)
    );

    let mut ymls = Vec::new();
    collect_yml(&plan, &mut ymls);
    assert!(!ymls.is_empty(), "planner emitted no task.yml under {}", plan.display());
    for p in &ymls {
        let text = p.to_string_lossy();
        assert!(
            text.contains("/tasks/"),
            "planner wrote task.yml outside phases/*/tasks: {}",
            p.display()
        );
    }

    let report = check_live(&plan, &ws);
    assert!(report.passed, "check_plan failed: {:?}", report.items);
    assert!(
        report.verify_executed > 0,
        "live check_plan executed no verify: {:?}",
        report
    );

    let phase_id = first_phase_id(&plan);
    let db = Database::open(&root.join("db.sqlite")).unwrap();
    let state = AppState::new(db);
    settings::set_setting(
        &state,
        KEY_INITIATIVES_DIR.to_string(),
        root.join("initiatives").to_string_lossy().into_owned(),
    )
    .unwrap();
    let plan_id = "plan-live-a4";
    let initiative = "init-live-a4";
    let runs = settings::initiative_runs_path(&root.join("initiatives"), initiative, plan_id, &phase_id);
    let map_path = root.join("id-map.txt");
    write_id_map_from_plan(&plan, &phase_id, &map_path);
    let expected_dir = root.join("expected-simple");
    write_simple_expected(&expected_dir);
    let cli = write_fake_kloo_ex(
        &root.join("bin"),
        &runs.join("tasks.json"),
        Some(&map_path),
        Some(&expected_dir),
    );
    let run = dummy_run_for(&ws, &plan, plan_id, initiative, &phase_id);
    let phase = run.phases[0].clone();
    let out = run_kloo_phase_ex(&state, &run, &phase, Some(cli.to_string_lossy().into_owned()))
        .await
        .expect("loop");
    assert_eq!(out, PhaseLoopOutcome::AllDone);
    let file = load_tasks(&tasks_json_path(&runs), plan_id, &phase_id).unwrap();
    assert!(file.tasks.iter().all(|t| t.status == "done"));
    assert!(file.tasks.iter().all(|t| t.commit_sha.is_some()));
    let log = Command::new("git")
        .args(["-C", ws.to_str().unwrap(), "log", "--oneline"])
        .output()
        .unwrap();
    let log_text = String::from_utf8_lossy(&log.stdout);
    for t in &file.tasks {
        assert!(
            log_text.contains(&format!("task {phase_id}/{}:", t.id)),
            "missing commit for {}: {log_text}",
            t.id
        );
    }

    let claude_ver = Command::new("claude")
        .arg("--version")
        .output()
        .ok()
        .map(|o| String::from_utf8_lossy(&o.stdout).trim().to_string())
        .unwrap_or_else(|| "unknown".into());
    let kloo_ver = Command::new("kloo")
        .arg("--version")
        .output()
        .ok()
        .map(|o| String::from_utf8_lossy(&o.stdout).trim().to_string())
        .unwrap_or_else(|| "unknown".into());
    let sample = ymls
        .first()
        .and_then(|p| std::fs::read_to_string(p).ok())
        .unwrap_or_default();

    std::fs::create_dir_all(&artifact_dir).unwrap();
    let stamp = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_secs().to_string())
        .unwrap_or_else(|_| "0".into());
    let blocking = report.items.iter().filter(|i| i.blocking).count();
    let mut body = String::new();
    body.push_str("# Live planner run (acceptance 4)\n\n");
    body.push_str(&format!(
        "1. Planner provider / model / timestamp: claude_code / {claude_ver} / unix {stamp}\n"
    ));
    body.push_str("2. Emitted task.yml paths (none pre-existed in the fixture):\n");
    for p in &ymls {
        body.push_str(&format!("   - {}\n", p.display()));
    }
    body.push_str(&format!(
        "3. check_plan: passed: {}, blocking items = {}\n",
        report.passed, blocking
    ));
    body.push_str(&format!(
        "   verify_executed: {}, verify_skipped: {}\n",
        report.verify_executed, report.verify_skipped
    ));
    body.push_str(&format!(
        "   host: RealPlanCheckHost (default). seams stubbed: none. spawn=real cargo; tokens=kloo ({kloo_ver}).\n"
    ));
    body.push_str("4. Loop result:\n");
    for t in &file.tasks {
        body.push_str(&format!(
            "   - {} status={} sha={} subject=`task {phase_id}/{}:`\n",
            t.id,
            t.status,
            t.commit_sha.as_deref().unwrap_or("-"),
            t.id
        ));
    }
    body.push_str("\n## user_brief (intent only — no per-task field dictation)\n");
    body.push_str(&user_brief);
    body.push('\n');
    body.push_str("\n## git log\n");
    body.push_str(&log_text);
    if !sample.is_empty() {
        body.push_str("\n## sample emitted task.yml\n```yaml\n");
        body.push_str(&sample);
        body.push_str("```\n");
    }
    std::fs::write(artifact_dir.join("live-planner-run.md"), body).unwrap();
}

