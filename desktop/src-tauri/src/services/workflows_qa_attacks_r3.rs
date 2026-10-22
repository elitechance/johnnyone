//! QA A's ROUND-3 adversarial tests: the executor, the identity key, the SQL guard, and the
//! cross-branch `fleet_busy` contract.
//!
//! A third file, for the same reason as the second: the two earlier ones are now co-authored, and
//! keeping each round separable means neither author range-edits the other's work.
//!
//! Same discipline. A test that documents a hole says `DEFECT PIN` in its message and asserts the
//! CURRENT behaviour, so closing the hole turns it red. A test that pins intended behaviour names
//! the mutation used to prove it is not vacuous.

#![cfg(test)]

use crate::services::workflows::*;
use crate::state::app_state::AppState;
use crate::test_support::{test_state, tmp_dir};
use rusqlite::params;
use std::path::Path;

const KEY: (&str, &str, &str) = ("repo", "dev-phase", "glm");

/// A logical repo name as a path the host can derive an identity from. Mirrors Dev A's own test
/// helper so both files spell the key the same way.
fn repo_path(name: &str) -> String {
    format!("/nonexistent-arena-qa-r3/{}", name)
}

fn repo_id(name: &str) -> String {
    repo_identity(&repo_path(name)).expect("derive the repo identity")
}

fn version_with(repo: &str, phase_type: &str, family: &str, fan_out: i64) -> NewWorkflowVersion {
    NewWorkflowVersion {
        workspace_path: repo_path(repo),
        phase_type: phase_type.to_string(),
        family: family.to_string(),
        parent_id: None,
        role: "role".to_string(),
        instruction: "instruction".to_string(),
        contract: "contract".to_string(),
        hop: r#"{"steps":["edit"]}"#.to_string(),
        fan_out,
    }
}

/// Shims over the three service functions that take the DERIVED identity, so every call site below
/// can keep naming the logical repo. An explicit local item wins over the glob import.
fn workflow_search_state(
    state: &AppState,
    repo: &str,
    phase_type: &str,
    family: &str,
) -> Result<WorkflowSearchState, String> {
    crate::services::workflows::workflow_search_state(state, &repo_id(repo), phase_type, family)
}

fn resume_workflow_search(
    state: &AppState,
    repo: &str,
    phase_type: &str,
    family: &str,
    reason: &str,
    confirmed: bool,
) -> Result<WorkflowSearchState, String> {
    crate::services::workflows::resume_workflow_search(
        state,
        &repo_id(repo),
        phase_type,
        family,
        reason,
        confirmed,
    )
}

fn workflow(state: &AppState, repo: &str, phase_type: &str, family: &str) -> Workflow {
    create_workflow_version(state, version_with(repo, phase_type, family, 2)).expect("create")
}

/// A plan row with a declared gate, pointing at `workspace`.
fn seed_plan(state: &AppState, workspace: &Path, test_commands: Option<&str>) -> String {
    let id = uuid::Uuid::new_v4().to_string();
    state
        .db
        .with_conn(|conn| {
            conn.execute(
                "INSERT INTO agent_plans (id, run_type, title, workspace_path, plan_path, \
                 worker_provider, reviewer_provider, initiative_id, initiative_status, \
                 test_commands) \
                 VALUES (?1, 'development', 'arena', ?2, ?3, 'claude_code', 'claude_code', \
                 ?1, 'development', ?4)",
                params![
                    id,
                    workspace.to_string_lossy(),
                    workspace.join("plan").to_string_lossy(),
                    test_commands
                ],
            )
            .map_err(|e| e.to_string())
        })
        .expect("seed plan");
    id
}

fn start_against(state: &AppState, workflow_id: &str, plan_id: Option<&str>) -> WorkflowRun {
    start_workflow_run(
        state,
        NewWorkflowRun {
            workflow_id: workflow_id.to_string(),
            plan_id: plan_id.map(|p| p.to_string()),
            seed: 0,
            seats: None,
        },
    )
    .expect("start run")
}

fn halt(state: &AppState, workflow_id: &str, repo: &str, phase: &str, family: &str) {
    for strike in 1..=3 {
        let run = start_against(state, workflow_id, None);
        reject_workflow_run(state, &run.id, &format!("strike {strike}")).expect("reject");
    }
    assert!(
        workflow_search_state(state, repo, phase, family)
            .expect("state")
            .halted
    );
}

// =================================================================================================
// ITEM 1 — the executor
// =================================================================================================

/// **DEFECT (documented, not fixed). The halt check is a TOCTOU and the halt leaks through it.**
///
/// `execute_gate` checks `search.halted` in one `with_conn`, then AWAITS the gate — a whole test
/// suite, seconds to minutes — then writes the score in a different `with_conn`. The mutex is not
/// held across the await, so three rejections landing during the gate run halt the family and the
/// score is still recorded.
///
/// That is precisely the leak the refusal's own doc comment says it prevents: *"Scoring one would
/// hand a halted family fresh evidence to promote on the moment it resumes."* The check stops a
/// score from STARTING on a halted family; it does not stop one from LANDING on one. Re-checking
/// the halt inside `set_gate_result`'s transaction closes it, and `set_gate_result` already opens a
/// transaction and already reads the workflow for the champion lock, so the fact is one query away
/// from where the decision is made.
#[tokio::test]
async fn qa_a3_a_halt_arriving_during_the_gate_run_discards_the_score() {
    let (state, _root) = test_state();
    let workspace = tmp_dir("arena-r3-toctou");
    let w = workflow(&state, KEY.0, KEY.1, KEY.2);
    // **Round 5, Dev A: the timing assumption is replaced by a SENTINEL, and the reason is a
    // measured flake.** The fixture was `["sleep 1"]` raced against rejections at +250ms, and under
    // full-suite load (`--test-threads=1`, 806 other tests) the rejections landed AFTER the gate's
    // write once in two runs: the family was not halted at write time, the score landed, and the
    // test failed — for the wrong reason, because a score landing on a family that has not halted
    // yet is correct behaviour. Measured: 8/8 alone, 1 failure in 2 full-suite runs.
    //
    // The gate now BLOCKS until the rejecting side touches a file, so the three rejections are
    // guaranteed to have committed before the gate can finish and therefore before the write. That
    // is strictly stronger than the original: it proves the halt is read at WRITE time with no
    // assumption about scheduling at all, which is the only thing this test is for. The wait is
    // bounded at ~10s and exits non-zero if the sentinel never arrives, so a regression here fails
    // loudly instead of hanging the suite.
    let sentinel = workspace.join("rejections-committed");
    let plan = seed_plan(
        &state,
        &workspace,
        Some(&format!(
            r#"["for _ in $(seq 1 200); do [ -f {} ] && exit 0; sleep 0.05; done; exit 1"]"#,
            sentinel.to_string_lossy()
        )),
    );
    let run = start_against(&state, &w.id, Some(&plan));
    assert_eq!(run.gate_exit, None);

    // Three runs to reject, created BEFORE the halt exists so `start_workflow_run` allows them.
    let victims: Vec<String> = (0..3)
        .map(|_| start_against(&state, &w.id, None).id)
        .collect();

    assert!(
        !workflow_search_state(&state, KEY.0, KEY.1, KEY.2)
            .expect("state")
            .halted,
        "not halted when the score begins"
    );

    let scoring = score_workflow_run(&state, &run.id, None);
    let rejecting = async {
        for (index, victim) in victims.iter().enumerate() {
            reject_workflow_run(&state, victim, &format!("mid-gate strike {index}"))
                .expect("reject");
        }
        // The family is halted NOW, and only now is the gate allowed to finish. Everything after
        // this point is the write racing nothing.
        assert!(
            workflow_search_state(&state, KEY.0, KEY.1, KEY.2)
                .expect("state")
                .halted,
            "the fixture must halt the family BEFORE releasing the gate"
        );
        std::fs::write(&sentinel, b"go").expect("release the gate");
    };
    let (scored, ()) = tokio::join!(scoring, rejecting);

    let state_after = workflow_search_state(&state, KEY.0, KEY.1, KEY.2).expect("state");
    assert!(
        state_after.halted,
        "the fixture must really halt the family while the gate runs"
    );
    // FIXED. The halt is re-read INSIDE `set_gate_result`'s write transaction, so a halt that
    // lands while the gate is running discards the result instead of letting it through. A check
    // before a multi-minute await is not a check.
    let error = scored.expect_err(
        "a score gathered while the search gave up must not land",
    );
    assert!(error.contains("is halted"), "got: {error}");
    assert!(
        error.contains("its result is discarded"),
        "the refusal must say what happened to the work: {error}"
    );
    assert_eq!(
        get_workflow_run(&state, &run.id).unwrap().gate_exit,
        None,
        "and NOTHING was written: no partial score, no ended_at"
    );

    // The consequence that made the leak matter is gone too: after a resume there is no stale
    // evidence to promote on, so a new run has to be scored.
    resume_workflow_search(&state, KEY.0, KEY.1, KEY.2, "resuming", true).expect("resume");
    let still_unscored = promote_workflow(&state, &run.id, None, true)
        .expect_err("there is no evidence to promote on");
    assert!(still_unscored.contains("no gate result"), "got: {still_unscored}");

    // And scoring AGAIN now that the halt is lifted works, which is what keeps this a discard
    // rather than a dead end.
    score_workflow_run(&state, &run.id, None)
        .await
        .expect("a resumed family can score the same run");
    assert_eq!(
        get_workflow_run(&state, &run.id).unwrap().gate_exit,
        Some(0)
    );
}

/// A fresh score is refused outright while the family is ALREADY halted, before anything runs —
/// the half of the rule that does work. Pinned separately from the TOCTOU so the two cannot be
/// confused.
#[tokio::test]
async fn qa_a3_scoring_is_refused_when_the_family_is_already_halted() {
    let (state, _root) = test_state();
    let workspace = tmp_dir("arena-r3-halted");
    let w = workflow(&state, KEY.0, KEY.1, KEY.2);
    let plan = seed_plan(&state, &workspace, Some(r#"["true"]"#));
    let run = start_against(&state, &w.id, Some(&plan));
    // Created BEFORE the halt: once halted, `start_workflow_run` refuses, so a run to test the
    // no-plan ordering with has to exist already.
    let planless = start_against(&state, &w.id, None);
    halt(&state, &w.id, KEY.0, KEY.1, KEY.2);

    let refused = score_workflow_run(&state, &run.id, None).await;
    assert!(refused.is_err(), "a halted family must not score");
    let error = refused.unwrap_err();
    assert!(error.contains("neither starts nor scores runs"), "got: {error}");
    assert!(error.contains("resume"), "the way out must be named: {error}");
    assert_eq!(
        get_workflow_run(&state, &run.id).unwrap().gate_exit,
        None,
        "and nothing was written"
    );
    // The halt is reported BEFORE the run's own wiring, as documented: a run with no plan AND a
    // halted family reports the halt, not the missing plan.
    assert!(score_workflow_run(&state, &planless.id, None)
        .await
        .unwrap_err()
        .contains("is halted"));
}

/// The three reserved negative exit codes reach the row as themselves. A signal is the one that is
/// cheap to produce deterministically: a gate that kills its own process group has no exit status,
/// so `status.code()` is `None` and the gate reports `TEST_GATE_EXIT_SIGNAL`.
///
/// What matters is that the stored score is the one the gate PRODUCED, not a 0 and not a value
/// synthesised on an error path — and that each is non-zero, so none can promote.
#[tokio::test]
async fn qa_a3_a_signalled_and_a_malformed_gate_record_their_real_codes() {
    use crate::services::agent_plans::{TEST_GATE_EXIT_MALFORMED, TEST_GATE_EXIT_SIGNAL};
    let (state, _root) = test_state();
    let workspace = tmp_dir("arena-r3-codes");
    let w = workflow(&state, KEY.0, KEY.1, KEY.2);

    // (a) killed by a signal, not exited.
    let killed_plan = seed_plan(&state, &workspace, Some(r#"["kill -9 $$"]"#));
    let killed_run = start_against(&state, &w.id, Some(&killed_plan));
    let killed = score_workflow_run(&state, &killed_run.id, None)
        .await
        .expect("a signalled gate is a score, not an error");
    assert_eq!(
        killed.gate_exit,
        Some(TEST_GATE_EXIT_SIGNAL as i64),
        "a signal records -1, the reserved code, not 0 and not a synthesised value"
    );

    // (b) a gate DECLARED but unreadable: a configuration error, which is a failure rather than an
    // absence, so it scores rather than refusing.
    let bad_plan = seed_plan(&state, &workspace, Some("not json at all"));
    let bad_run = start_against(&state, &w.id, Some(&bad_plan));
    let malformed = score_workflow_run(&state, &bad_run.id, None)
        .await
        .expect("a malformed gate is a score, not a refusal");
    assert_eq!(
        malformed.gate_exit,
        Some(TEST_GATE_EXIT_MALFORMED as i64),
        "-2, distinct from both a pass and an absent gate"
    );

    // Neither can promote, and both are distinguishable from the ungated refusal.
    for run in [&killed, &malformed] {
        assert_ne!(run.gate_exit, Some(0));
        assert!(promote_workflow(&state, &run.id, None, true)
            .unwrap_err()
            .contains("failed its gate"));
    }
    // The reserved codes are inside the stored range, so the bound does not reject them.
    assert!(TEST_GATE_EXIT_SIGNAL as i64 >= i32::MIN as i64);
}

/// A workspace that no longer exists is an INFRASTRUCTURE error, not a red score: the gate cannot
/// spawn, `execute_gate` propagates the error, and the run keeps `gate_exit = NULL`.
///
/// That is the right call — recording a red score would blame the workflow for the environment —
/// and it is worth pinning because the difference between "returns Err" and "records non-zero" is
/// invisible at the call site and decides whether a workflow is permanently tainted by one bad
/// checkout.
#[tokio::test]
async fn qa_a3_a_missing_workspace_errors_without_recording_a_score() {
    let (state, _root) = test_state();
    let w = workflow(&state, KEY.0, KEY.1, KEY.2);
    let gone = tmp_dir("arena-r3-gone");
    let plan = seed_plan(&state, &gone, Some(r#"["true"]"#));
    let run = start_against(&state, &w.id, Some(&plan));
    std::fs::remove_dir_all(&gone).expect("remove the workspace");

    let failed = score_workflow_run(&state, &run.id, None).await;
    assert!(failed.is_err(), "a vanished workspace must not score");
    let error = failed.unwrap_err();
    assert!(
        error.contains("Could not run the test command"),
        "the error must name the spawn failure, not a gate verdict: {error}"
    );
    assert_eq!(
        get_workflow_run(&state, &run.id).unwrap().gate_exit,
        None,
        "and the run is still unscored, not scored red"
    );
    // So it is still promotable once the workspace comes back, rather than permanently tainted.
    std::fs::create_dir_all(&gone).expect("restore");
    let scored = score_workflow_run(&state, &run.id, None)
        .await
        .expect("scores once the workspace exists again");
    assert_eq!(scored.gate_exit, Some(0));
}

/// A phase deleted between start and score is a hard error naming the phase — NOT a silent
/// fall-back to the plan-level gate, which would score the run on a different gate than the one
/// asked for and look like a real result.
#[tokio::test]
async fn qa_a3_a_deleted_phase_errors_rather_than_falling_back_to_the_plan_gate() {
    let (state, _root) = test_state();
    let workspace = tmp_dir("arena-r3-phase");
    let w = workflow(&state, KEY.0, KEY.1, KEY.2);
    // The PLAN-level gate passes. If a missing phase silently fell back to it, the run would be
    // scored green on a gate nobody asked for.
    let plan = seed_plan(&state, &workspace, Some(r#"["true"]"#));
    let run = start_against(&state, &w.id, Some(&plan));

    let refused = score_workflow_run(&state, &run.id, Some("no-such-phase".to_string())).await;
    assert!(refused.is_err(), "a missing phase must not fall back");
    assert!(
        refused.unwrap_err().contains("Phase not found"),
        "the error must name the phase"
    );
    assert_eq!(
        get_workflow_run(&state, &run.id).unwrap().gate_exit,
        None,
        "and nothing was scored"
    );
}

/// A run with no plan, and a run whose plan was DELETED after it started (`plan_id` is
/// `ON DELETE SET NULL`, so the column goes null underneath the run). Both refuse with the
/// no-plan message rather than scoring 0.
#[tokio::test]
async fn qa_a3_a_run_whose_plan_is_null_or_deleted_cannot_be_scored() {
    let (state, _root) = test_state();
    let workspace = tmp_dir("arena-r3-null");
    let w = workflow(&state, KEY.0, KEY.1, KEY.2);

    let planless = start_against(&state, &w.id, None);
    let refused = score_workflow_run(&state, &planless.id, None).await;
    assert!(refused.is_err());
    assert!(refused.unwrap_err().contains("has no plan"));

    // Now one that HAD a plan, scored once, and then lost it.
    let plan = seed_plan(&state, &workspace, Some(r#"["true"]"#));
    let run = start_against(&state, &w.id, Some(&plan));
    assert_eq!(run.plan_id.as_deref(), Some(plan.as_str()));
    state
        .db
        .with_conn(|conn| {
            conn.execute("PRAGMA foreign_keys=ON", []).ok();
            conn.execute("DELETE FROM agent_plans WHERE id = ?1", params![plan])
                .map_err(|e| e.to_string())
        })
        .expect("delete the plan");
    assert_eq!(
        get_workflow_run(&state, &run.id).unwrap().plan_id,
        None,
        "ON DELETE SET NULL really nulled the column"
    );
    let orphaned = score_workflow_run(&state, &run.id, None).await;
    assert!(orphaned.is_err(), "an orphaned run must not score");
    assert!(orphaned.unwrap_err().contains("has no plan"));
    assert_eq!(get_workflow_run(&state, &run.id).unwrap().gate_exit, None);
}

/// The executor honours the re-score guards: a second `score_workflow_run` is refused and names the
/// explicit door, `rescore_workflow_run` goes through, and the sitting champion's promoting run is
/// refused by BOTH doors. The suite's test for this exists; this adds the part it does not cover —
/// that a refused re-score does not leave a NEW score behind after actually running the gate.
#[tokio::test]
async fn qa_a3_a_refused_rescore_runs_the_gate_but_keeps_the_old_score() {
    let (state, _root) = test_state();
    let workspace = tmp_dir("arena-r3-rescore");
    let w = workflow(&state, KEY.0, KEY.1, KEY.2);
    let green = seed_plan(&state, &workspace, Some(r#"["true"]"#));
    let run = start_against(&state, &w.id, Some(&green));
    score_workflow_run(&state, &run.id, None).await.expect("first");
    assert_eq!(get_workflow_run(&state, &run.id).unwrap().gate_exit, Some(0));

    // A second plain score is refused …
    let refused = score_workflow_run(&state, &run.id, None).await;
    assert!(refused.is_err());
    let error = refused.unwrap_err();
    assert!(error.contains("already has a gate result"), "got: {error}");
    assert!(error.contains("rescore_workflow_run_gate"), "got: {error}");
    assert_eq!(
        get_workflow_run(&state, &run.id).unwrap().gate_exit,
        Some(0),
        "and the stored score is untouched"
    );

    // … the explicit door works, and replaces the score with what the gate really produced.
    let red = seed_plan(&state, &workspace, Some(r#"["exit 3"]"#));
    let red_run = start_against(&state, &w.id, Some(&red));
    score_workflow_run(&state, &red_run.id, None).await.expect("score");
    let replaced = rescore_workflow_run(&state, &red_run.id, None)
        .await
        .expect("explicit rescore");
    assert_eq!(replaced.gate_exit, Some(3), "the gate's real code, again");

    // … and the sitting champion's promoting run is refused by both doors even though the gate
    // would have passed.
    promote_workflow(&state, &run.id, None, true).expect("promote");
    for attempt in [
        score_workflow_run(&state, &run.id, None).await,
        rescore_workflow_run(&state, &run.id, None).await,
    ] {
        assert!(attempt.is_err(), "the champion's evidence is immutable");
    }
    assert!(rescore_workflow_run(&state, &run.id, None)
        .await
        .unwrap_err()
        .contains("cannot be rewritten underneath it"));
    assert_eq!(get_workflow_run(&state, &run.id).unwrap().gate_exit, Some(0));
}

/// **A stale doc comment contradicting the code beneath it.** `set_gate_result`'s comment says
/// *"`gate_collected` is deliberately NOT bounded: it is advisory, and refusing a real score
/// because an advisory count is large would be the wrong trade"* — and the next ten lines bound it
/// and refuse exactly that. The bound is the right call (it is what stops the silent 2^53
/// rounding), but through the EXECUTOR it means a gate that ran for minutes and passed can have its
/// real result discarded over the advisory count, which is the trade the comment says was rejected.
///
/// Measured at the recorder, since no real parser produces a count that large.
#[test]
fn qa_a3_the_collected_bound_keeps_the_score_and_drops_the_count() {
    let (state, _root) = test_state();
    let w = workflow(&state, KEY.0, KEY.1, KEY.2);
    let run = start_against(&state, &w.id, None);

    // FIXED, and the contradiction is resolved in the direction that keeps the score. An
    // out-of-range advisory count is DROPPED to None with a warning, not refused: the exit code is
    // the only score the arena has, and a gate that ran for minutes and passed must not lose it
    // over a number nothing branches on.
    //
    // None rather than a clamp to MAX_GATE_COLLECTED, because None already means "no count the host
    // could trust", whereas clamping would assert a precise number that was never measured — the
    // quiet-wrong-value failure the bound exists to prevent, just relocated.
    let kept = record_workflow_run_gate(&state, &run.id, 0, Some(MAX_GATE_COLLECTED + 1))
        .expect("an advisory count must never cost the score");
    assert_eq!(kept.gate_exit, Some(0), "the score is kept");
    assert_eq!(kept.gate_collected, None, "and the untrustworthy count is dropped");
    assert_eq!(
        get_workflow_run(&state, &run.id).unwrap().gate_exit,
        Some(0),
        "persisted, not just returned"
    );
    // A negative count goes the same way, for the same reason.
    let negative = rescore_workflow_run_gate(&state, &run.id, 0, Some(-1)).expect("kept");
    assert_eq!((negative.gate_exit, negative.gate_collected), (Some(0), None));
    // The boundary itself round-trips, so the bound is inclusive and exact.
    let at_bound = rescore_workflow_run_gate(&state, &run.id, 0, Some(MAX_GATE_COLLECTED))
        .expect("the boundary is allowed");
    assert_eq!(at_bound.gate_collected, Some(MAX_GATE_COLLECTED));
    assert_eq!(MAX_GATE_COLLECTED, 9_007_199_254_740_991);
}

// =================================================================================================
// ITEM 2 — is the manual path complete?
// =================================================================================================

/// **The whole manual journey, with no host gap: unscored -> scored -> compared -> promoted.**
///
/// The seam Dev A left open is that nothing calls `score_workflow_run` automatically. This proves a
/// human (or the eventual coordinator call) can get from a fresh workflow to a champion using only
/// published entry points, with every intermediate fact readable back.
#[tokio::test]
async fn qa_a3_a_human_can_drive_a_run_from_unscored_to_champion_with_no_gap() {
    let (state, _root) = test_state();
    let workspace = tmp_dir("arena-r3-journey");
    let incumbent = workflow(&state, KEY.0, KEY.1, KEY.2);
    let challenger = create_workflow_version(
        &state,
        NewWorkflowVersion {
            parent_id: Some(incumbent.id.clone()),
            ..version_with(KEY.0, KEY.1, KEY.2, 2)
        },
    )
    .expect("a second version");

    let plan = seed_plan(&state, &workspace, Some(r#"["true"]"#));

    // 1. start, 2. score by EXECUTING the gate — the step that did not exist before this round.
    let incumbent_run = start_against(&state, &incumbent.id, Some(&plan));
    let incumbent_run = score_workflow_run(&state, &incumbent_run.id, None).await.expect("score");
    assert_eq!(incumbent_run.gate_exit, Some(0));
    // 3. seed the first champion.
    promote_workflow(&state, &incumbent_run.id, None, true).expect("seed");

    let challenger_run = start_against(&state, &challenger.id, Some(&plan));
    let challenger_run = score_workflow_run(&state, &challenger_run.id, None).await.expect("score");

    // 4. compare, 5. read the pref back (the read added for exactly this), 6. promote.
    let pref = record_workflow_pref(&state, &challenger_run.id, &incumbent_run.id, None)
        .expect("compare");
    assert!(pref.comparable, "reason: {:?}", pref.reason);
    let readable = list_workflow_prefs(&state, &challenger.id)
        .expect("the pref is readable back without having been remembered")
        .into_iter()
        .find(|p| p.id == pref.id);
    assert!(readable.is_some(), "a reloaded client can recover the prefId");
    promote_workflow(&state, &challenger_run.id, Some(&pref.id), true).expect("displace");

    // Every fact a UI needs is readable: who is champion, the search state, the runs, the prefs.
    let search = workflow_search_state(&state, KEY.0, KEY.1, KEY.2).expect("state");
    assert_eq!(search.champion_workflow_id.as_deref(), Some(challenger.id.as_str()));
    assert!(!search.halted);
    assert_eq!(list_workflow_runs(&state, &challenger.id).expect("runs").len(), 1);
    assert!(get_workflow(&state, &incumbent.id).unwrap().is_champion == false);
}

// =================================================================================================
// ITEM 4 — the SQL reason guard, re-measured, hunting an eighteenth case
// =================================================================================================

/// The decision log's `reason` CHECK, re-measured from scratch, including Dev A's own two failure
/// modes: a `char(...)` set silently truncated at a zero, and visible text that happens to contain
/// an invisible character being wrongly refused.
///
/// `has_visible_content` is the real guard; this is the table's half, which exists so a second
/// writer cannot make the log useless.
#[test]
fn qa_a3_the_reason_check_refuses_every_invisible_reason_i_could_build() {
    let (state, _root) = test_state();
    let w = workflow(&state, KEY.0, KEY.1, KEY.2);
    let run = start_against(&state, &w.id, None);
    let mut next = 0;
    let insert = |label: &str, kind: &str, reason: &str, n: &mut i32| -> Result<usize, String> {
        *n += 1;
        let id = format!("{label}-{n}");
        state.db.with_conn(|conn| {
            conn.execute(
                "INSERT INTO workflow_decisions (id, run_id, kind, reason, created_at) \
                 VALUES (?1, ?2, ?3, ?4, '2026-01-01T00:00:00Z')",
                params![id, run.id, kind, reason],
            )
            .map_err(|e| e.to_string())
        })
    };

    // Everything the shipped CHECK is supposed to catch, plus every invisible character I could
    // find that it is NOT told about. The ones it cannot know about are the point of
    // `has_visible_content`, so each is also run through that.
    let invisible: &[(&str, &str)] = &[
        ("empty", ""),
        ("space", " "),
        ("spaces", "     "),
        ("tab", "\t"),
        ("newline", "\n"),
        ("crlf", "\r\n"),
        ("mixed-ascii-ws", " \t\r\n "),
        ("vertical-tab", "\u{000B}"),
        ("form-feed", "\u{000C}"),
        ("nul", "\u{0000}"),
        ("c0-soh", "\u{0001}"),
        ("c0-us", "\u{001F}"),
        ("delete", "\u{007F}"),
        ("zwsp", "\u{200B}"),
        ("zwnj", "\u{200C}"),
        ("zwj", "\u{200D}"),
        ("lrm", "\u{200E}"),
        ("rlm", "\u{200F}"),
        ("bom", "\u{FEFF}"),
        ("soft-hyphen", "\u{00AD}"),
        // Beyond the set the CHECK names — the eighteenth case onwards.
        ("nbsp", "\u{00A0}"),
        ("nel", "\u{0085}"),
        ("ogham-space", "\u{1680}"),
        ("mongolian-vowel-sep", "\u{180E}"),
        ("en-quad", "\u{2000}"),
        ("hair-space", "\u{200A}"),
        ("line-separator", "\u{2028}"),
        ("para-separator", "\u{2029}"),
        ("narrow-nbsp", "\u{202F}"),
        ("medium-math-space", "\u{205F}"),
        ("word-joiner", "\u{2060}"),
        ("invisible-times", "\u{2062}"),
        ("ideographic-space", "\u{3000}"),
        ("braille-blank", "\u{2800}"),
        ("hangul-filler", "\u{3164}"),
        ("halfwidth-hangul-filler", "\u{FFA0}"),
        ("zwnbsp-variation", "\u{FE00}"),
        ("variation-selector-16", "\u{FE0F}"),
        ("hangul-choseong-filler", "\u{115F}"),
        ("hangul-jungseong-filler", "\u{1160}"),
        ("khmer-inherent-aq", "\u{17B4}"),
        ("khmer-inherent-aa", "\u{17B5}"),
        ("combinations", "\u{200B}\u{FEFF}\u{00AD}\t \n"),
        ("nul-then-tab", "\u{0000}\t"),
        ("tab-then-nul", "\t\u{0000}"),
    ];

    let mut table_holes = Vec::new();
    let mut service_holes = Vec::new();
    for (label, reason) in invisible {
        // The SERVICE guard must refuse all of them — it is the real one.
        if reject_workflow_run(&state, &run.id, reason).is_ok() {
            service_holes.push(*label);
        }
        if resume_workflow_search(&state, KEY.0, KEY.1, KEY.2, reason, true)
            .err()
            .map(|e| !e.contains("must record a reason") && !e.contains("is not halted"))
            .unwrap_or(true)
        {
            // Not halted here, so the expected refusal is either one; a reason refusal is what we
            // are checking and "not halted" means the reason passed. Only the latter is a hole.
            if resume_workflow_search(&state, KEY.0, KEY.1, KEY.2, reason, true)
                .unwrap_err()
                .contains("is not halted")
            {
                service_holes.push(*label);
            }
        }
        // The TABLE guard catches what SQL can express.
        for kind in ["reject", "resume"] {
            if insert(label, kind, reason, &mut next).is_ok() {
                table_holes.push(format!("{kind}/{label}"));
            }
        }
    }

    // **DEFECT (documented, not fixed). `has_visible_content` is a DENYLIST and the list is
    // incomplete.** Dev A measured 17 cases with 0 holes; these are the eighteenth onwards. Each of
    // these code points renders as NOTHING, is not Unicode `White_Space`, is not `is_control`, and
    // is not in the denylist — so a rejection or resume reason made only of them is accepted and
    // stored, which is the exact hole the guard was written to close.
    //
    // Dev A's own doc comment anticipates this: *"If this list needs to grow, growing it is the
    // fix."* It needs to grow. The durable fix is `Default_Ignorable_Code_Point` plus the
    // Braille blank, which needs a Unicode table; the cheap fix is adding these ranges.
    // FIXED: all nine were added to `has_visible_content`, so the expected-holes list is now
    // EMPTY and the assertion below becomes "no input in this corpus passes the guard". The
    // Default_Ignorable table was considered and deliberately not taken as a dependency — the
    // reasoning is recorded on the function itself, including that a third batch would flip it.
    let expected_service_holes: [&str; 0] = [];
    let mut unexpected: Vec<&str> = service_holes
        .iter()
        .copied()
        .filter(|hole| !expected_service_holes.contains(hole))
        .collect();
    unexpected.sort_unstable();
    assert!(
        unexpected.is_empty(),
        "a NEW hole in has_visible_content beyond the nine already known: {unexpected:?}"
    );
    let mut missing: Vec<&str> = expected_service_holes
        .iter()
        .copied()
        .filter(|hole| !service_holes.contains(hole))
        .collect();
    missing.sort_unstable();
    assert!(
        missing.is_empty(),
        "these were listed as known holes but the guard now refuses them; shrink \
         `expected_service_holes` to match: {missing:?}"
    );
    // The guard still works for everything it DOES know about, which is most of the space.
    assert!(
        service_holes.len() < invisible.len() / 2,
        "most invisible reasons are still caught: {} of {} got through",
        service_holes.len(),
        invisible.len()
    );
    // The table cannot express Unicode visibility, so holes here are expected and bounded — what
    // must not happen is a hole in the characters it DOES name.
    let named_by_the_check = [
        "empty", "space", "spaces", "tab", "newline", "crlf", "mixed-ascii-ws", "vertical-tab",
        "form-feed", "nul", "c0-soh", "c0-us", "delete", "zwsp", "zwnj", "zwj", "lrm", "rlm",
        "bom", "soft-hyphen", "nul-then-tab", "tab-then-nul", "combinations",
    ];
    // Compared on the EXACT label after the slash, not a suffix: an earlier draft used
    // `ends_with`, and "reject/ogham-space".ends_with("space") made the check report a hole in
    // `space` that did not exist. Caught by the assertion firing with a list that disagreed with
    // itself.
    for name in named_by_the_check {
        let hole = table_holes
            .iter()
            .find(|hole| hole.split('/').nth(1) == Some(name));
        assert!(
            hole.is_none(),
            "the CHECK names {name} and must refuse it — a hole here is the char() truncation bug \
             returning. holes: {table_holes:?}"
        );
    }
    // What the table CANNOT express is bounded and known: every remaining hole is a non-ASCII
    // invisible, which is precisely why `has_visible_content` is the real guard.
    for hole in &table_holes {
        let label = hole.split('/').nth(1).unwrap_or_default();
        assert!(
            !named_by_the_check.contains(&label),
            "an unexpected table hole in a named character: {hole}"
        );
    }

    // Dev A's second failure mode, pinned directly: the trim set must not be truncated at the zero.
    // If it were, tab and newline would pass, which the loop above would have caught — assert the
    // set's own length so the cause is named, not just the symptom.
    let trim_set_len: i64 = state
        .db
        .with_conn(|conn| {
            conn.query_row(
                "SELECT length(char(32, 127, 1, 2, 3, 4, 5, 6, 7, 8, 9, 10, 11, 12, 13, 14, 15, \
                 16, 17, 18, 19, 20, 21, 22, 23, 24, 25, 26, 27, 28, 29, 30, 31))",
                [],
                |row| row.get(0),
            )
            .map_err(|e| e.to_string())
        })
        .expect("measure the trim set");
    assert_eq!(
        trim_set_len, 33,
        "the trim set must contain all 33 named characters; a shorter set means char() truncated \
         at a zero and the guard is silently narrower than it reads"
    );
    // And the control: NUL really does truncate char(), which is why it is handled by replace().
    let truncated_len: i64 = state
        .db
        .with_conn(|conn| {
            conn.query_row("SELECT length(char(32, 0, 9, 10))", [], |row| row.get(0))
                .map_err(|e| e.to_string())
        })
        .expect("measure");
    assert_eq!(
        truncated_len, 1,
        "char(32, 0, 9, 10) is one character, not four — the truncation Dev A measured twice"
    );
}

/// The other direction, which a guard this aggressive can easily get wrong: VISIBLE text that
/// contains an invisible character must still be ACCEPTED, at the table and in the service.
#[test]
fn qa_a3_visible_text_containing_invisible_characters_is_still_accepted() {
    let (state, _root) = test_state();
    let w = workflow(&state, KEY.0, KEY.1, KEY.2);
    let run = start_against(&state, &w.id, None);

    let acceptable: &[(&str, &str)] = &[
        ("plain", "too slow"),
        ("leading-zwsp", "\u{200B}too slow"),
        ("trailing-bom", "too slow\u{FEFF}"),
        ("internal-soft-hyphen", "too\u{00AD}slow"),
        ("internal-nul", "too\u{0000}slow"),
        ("wrapped-in-whitespace", "  \t too slow \n "),
        ("emoji-only", "\u{1F6A9}"),
        ("cjk", "\u{592A}\u{6162}"),
        ("rtl-text", "\u{0628}\u{0637}\u{064A}\u{0621}"),
        ("combining-accent", "e\u{0301}"),
        ("digit-only", "7"),
        ("punctuation-only", "?"),
        ("zwj-emoji-sequence", "\u{1F469}\u{200D}\u{1F4BB}"),
        ("at-the-length-bound", "x"),
    ];
    for (label, reason) in acceptable {
        reject_workflow_run(&state, &run.id, reason)
            .unwrap_or_else(|e| panic!("the service must accept {label}: {e}"));
        state
            .db
            .with_conn(|conn| {
                conn.execute(
                    "INSERT INTO workflow_decisions (id, run_id, kind, reason, created_at) \
                     VALUES (?1, ?2, 'reject', ?3, '2026-01-01T00:00:00Z')",
                    params![format!("ok-{label}"), run.id, reason],
                )
                .map_err(|e| e.to_string())
            })
            .unwrap_or_else(|e| panic!("the table must accept {label}: {e}"));
    }

    // The length bound bites at the table as well as the service, and the boundary is inclusive.
    let at_bound = "x".repeat(MAX_REASON_CHARS);
    let over_bound = "x".repeat(MAX_REASON_CHARS + 1);
    reject_workflow_run(&state, &run.id, &at_bound).expect("2000 chars is allowed");
    assert!(
        reject_workflow_run(&state, &run.id, &over_bound).is_err(),
        "2001 is not"
    );
}

// =================================================================================================
// ITEM 6 — the identity key
// =================================================================================================

fn git(dir: &Path, args: &[&str]) {
    let status = std::process::Command::new("git")
        .args(args)
        .current_dir(dir)
        .output()
        .expect("git");
    assert!(
        status.status.success(),
        "git {:?} failed: {}",
        args,
        String::from_utf8_lossy(&status.stderr)
    );
}

fn init_repo(dir: &Path) {
    std::fs::create_dir_all(dir).expect("mkdir");
    git(dir, &["init", "-q"]);
    git(dir, &["config", "user.email", "qa@example.com"]);
    git(dir, &["config", "user.name", "qa"]);
    std::fs::write(dir.join("f.txt"), "x").expect("write");
    git(dir, &["add", "."]);
    git(dir, &["commit", "-qm", "one"]);
}

/// Normalisation: SSH and HTTPS forms of one repo are ONE identity, and two repos that differ only
/// by host are TWO. Pure-function level, so no git is needed.
#[test]
fn qa_a3_remote_normalisation_folds_the_forms_that_are_one_repo() {
    let (state, _root) = test_state();
    let _ = &state;
    // Every spelling of the same repo.
    let same = [
        "git@github.com:Owner/Repo.git",
        "git@github.com:owner/repo",
        "https://github.com/owner/repo.git",
        "https://github.com/owner/repo",
        "https://user:token@github.com/owner/repo.git",
        "ssh://git@github.com/owner/repo.git",
        "git://github.com/owner/repo.git",
        "http://github.com/Owner/Repo/",
        "  https://github.com/owner/repo  ",
    ];
    let identities: std::collections::BTreeSet<String> =
        same.iter().map(|url| normalise_remote_for_test(url)).collect();
    assert_eq!(
        identities.len(),
        1,
        "every spelling of one repo must be one identity: {identities:?}"
    );
    assert_eq!(identities.into_iter().next().unwrap(), "git:github.com/owner/repo");

    // Different host, same path: must stay distinct. This is the coordinator's case.
    assert_ne!(
        normalise_remote_for_test("https://github.com/owner/repo"),
        normalise_remote_for_test("https://gitlab.com/owner/repo"),
        "two different hosts are two repos"
    );
    assert_ne!(
        normalise_remote_for_test("git@github.com:owner/repo"),
        normalise_remote_for_test("git@github.example.internal:owner/repo"),
    );
    // Different owner, different repo: distinct.
    assert_ne!(
        normalise_remote_for_test("https://github.com/a/repo"),
        normalise_remote_for_test("https://github.com/b/repo"),
    );

    // FIXED: a `:` followed by digits is a PORT, not the scp-form separator, and a port is not
    // part of which repository this is — so it is dropped rather than folded into the path.
    assert_eq!(
        normalise_remote_for_test("ssh://git@host.example/owner/repo.git"),
        normalise_remote_for_test("ssh://git@host.example:2222/owner/repo.git"),
        "one repo on a custom port is ONE identity"
    );
    assert_eq!(
        normalise_remote_for_test("ssh://git@host.example:2222/owner/repo.git"),
        "git:host.example/owner/repo"
    );
    // And the scp form, where the `:` separates a PATH rather than a port, still folds correctly —
    // the two cases are told apart by what follows the colon, not by guessing.
    assert_eq!(
        normalise_remote_for_test("git@host.example:owner/repo.git"),
        "git:host.example/owner/repo"
    );
}

/// `normalise_remote` is private; exercise it through the one public door that reaches it, by
/// building a real repo with that remote. Kept as a helper so the table above stays readable.
fn normalise_remote_for_test(url: &str) -> String {
    let dir = tmp_dir("arena-r3-norm");
    init_repo(&dir);
    git(&dir, &["remote", "add", "origin", url.trim()]);
    repo_identity(&dir.to_string_lossy()).expect("identity")
}

/// Two WORKTREES of one repository are one identity — the case the whole design decision rests on,
/// because this project lives in worktrees and a path key would give a worktree its own champion.
#[test]
fn qa_a3_two_worktrees_of_one_repo_are_one_identity() {
    let root = tmp_dir("arena-r3-wt");
    let main = root.join("main");
    init_repo(&main);
    git(&main, &["remote", "add", "origin", "git@github.com:owner/repo.git"]);
    let linked = root.join("linked");
    git(&main, &["worktree", "add", "-q", linked.to_str().unwrap(), "-b", "side"]);

    let a = repo_identity(&main.to_string_lossy()).expect("main");
    let b = repo_identity(&linked.to_string_lossy()).expect("worktree");
    assert_eq!(a, b, "a worktree shares its repo's champion");
    assert_eq!(a, "git:github.com/owner/repo");

    // A detached head does not change the identity — config is unaffected by HEAD.
    git(&linked, &["checkout", "-q", "--detach"]);
    assert_eq!(
        repo_identity(&linked.to_string_lossy()).expect("detached"),
        a,
        "a detached head is the same repo"
    );
}

/// **DEFECT (documented, not fixed). A repo with remotes but no `origin` falls back to the PATH,
/// which re-opens the very collision the remote key exists to close.**
///
/// `git_remote_url` reads `remote.origin.url` and nothing else. A checkout whose remote is called
/// `upstream` (a fork workflow, or any repo where origin was renamed) has a remote — so it is not
/// the "no remote at all" case the path fallback was written for — yet it gets a path identity. Two
/// worktrees of such a repo then have DIFFERENT identities and therefore different champions, which
/// is exactly the failure the design doc names as the reason not to key on the path.
///
/// Reading the first remote alphabetically, or `git remote get-url --all` with origin preferred,
/// would close it.
#[test]
fn qa_a3_a_repo_whose_remote_is_not_named_origin_still_uses_the_remote() {
    let root = tmp_dir("arena-r3-upstream");
    let main = root.join("main");
    init_repo(&main);
    git(&main, &["remote", "add", "upstream", "git@github.com:owner/repo.git"]);
    // FIXED: `git remote` is listed and `origin` merely PREFERRED, so a fork-style checkout with
    // only an `upstream` remote uses that remote rather than falling through to the path.
    let identity = repo_identity(&main.to_string_lossy()).expect("identity");
    assert_eq!(
        identity, "git:github.com/owner/repo",
        "a repo with a remote named `upstream` has a remote, and the identity must use it"
    );

    // And the consequence the design explicitly set out to avoid is gone with it.
    let linked = root.join("linked");
    git(&main, &["worktree", "add", "-q", linked.to_str().unwrap(), "-b", "side"]);
    assert_eq!(
        repo_identity(&main.to_string_lossy()).expect("main"),
        repo_identity(&linked.to_string_lossy()).expect("worktree"),
        "two worktrees of ONE repo are ONE champion, whatever the remote is called"
    );

    // Renaming it to origin fixes it, which is what makes this a lookup gap rather than a design
    // problem.
    git(&main, &["remote", "rename", "upstream", "origin"]);
    assert_eq!(
        repo_identity(&main.to_string_lossy()).expect("renamed"),
        "git:github.com/owner/repo"
    );
    // …and the identity is UNCHANGED by the rename, which is now the better outcome: the remote
    // URL is what the key is made of, so renaming `upstream` to `origin` moves no champion. The
    // thing that WOULD orphan a champion is changing the remote's URL, which is a different and
    // much rarer act than renaming it.
    assert_eq!(
        identity,
        repo_identity(&main.to_string_lossy()).expect("after rename"),
        "renaming a remote must not orphan the family's champion"
    );
}

/// When origin exists alongside other remotes, origin wins — so multiple remotes are not ambiguous
/// in the common case.
#[test]
fn qa_a3_origin_wins_when_several_remotes_exist() {
    let dir = tmp_dir("arena-r3-multi");
    init_repo(&dir);
    git(&dir, &["remote", "add", "upstream", "git@github.com:upstream/repo.git"]);
    git(&dir, &["remote", "add", "origin", "git@github.com:mine/repo.git"]);
    git(&dir, &["remote", "add", "zzz", "git@github.com:zzz/repo.git"]);
    assert_eq!(
        repo_identity(&dir.to_string_lossy()).expect("identity"),
        "git:github.com/mine/repo",
        "origin is the answer regardless of the order the others were added"
    );
}

/// A submodule has its own identity, not its parent's — correct, since it is a different repo with
/// its own champion.
#[test]
fn qa_a3_a_submodule_has_its_own_identity() {
    let root = tmp_dir("arena-r3-sub");
    let parent = root.join("parent");
    let child = root.join("child");
    init_repo(&parent);
    init_repo(&child);
    git(&parent, &["remote", "add", "origin", "git@github.com:owner/parent.git"]);
    git(&child, &["remote", "add", "origin", "git@github.com:owner/child.git"]);
    let nested = parent.join("vendor");
    std::fs::create_dir_all(&nested).expect("mkdir");
    // A real `git submodule add` needs network-free file protocol permission; a nested repo is the
    // same thing for identity purposes — `git config` resolves to the innermost repo.
    let inner = nested.join("child");
    init_repo(&inner);
    git(&inner, &["remote", "add", "origin", "git@github.com:owner/child.git"]);

    assert_eq!(
        repo_identity(&parent.to_string_lossy()).expect("parent"),
        "git:github.com/owner/parent"
    );
    assert_eq!(
        repo_identity(&inner.to_string_lossy()).expect("nested"),
        "git:github.com/owner/child",
        "a nested repo is its own identity, not the parent's"
    );
}

/// The PATH fallback: a symlinked path and a `.` -laden path are one identity, and a blank path is
/// refused. A path that does not exist cannot be canonicalised, so it stays as given — which is a
/// real (documented) gap worth pinning so it is not discovered as a surprise.
#[test]
fn qa_a3_the_path_fallback_canonicalises_what_it_can() {
    let root = tmp_dir("arena-r3-path");
    let real = root.join("real");
    std::fs::create_dir_all(&real).expect("mkdir");
    let link = root.join("link");
    #[cfg(unix)]
    std::os::unix::fs::symlink(&real, &link).expect("symlink");

    let direct = repo_identity(&real.to_string_lossy()).expect("direct");
    assert!(direct.starts_with("path:"), "no git here: {direct}");
    #[cfg(unix)]
    assert_eq!(
        repo_identity(&link.to_string_lossy()).expect("via symlink"),
        direct,
        "a symlinked path resolves to the same identity"
    );
    let dotted = format!("{}/./.", real.to_string_lossy());
    assert_eq!(
        repo_identity(&dotted).expect("dotted"),
        direct,
        "`/a/./b` and `/a/b` are one identity"
    );

    assert!(repo_identity("").is_err(), "a blank path is refused");
    assert!(repo_identity("   ").is_err(), "and so is whitespace");

    // A path that does not exist: canonicalize fails, the raw string is used, so two spellings of
    // one absent directory are two identities.
    let absent = format!("{}/not-there", root.to_string_lossy());
    let absent_dotted = format!("{}/./not-there", root.to_string_lossy());
    assert_ne!(
        repo_identity(&absent).expect("absent"),
        repo_identity(&absent_dotted).expect("absent dotted"),
        "DEFECT PIN (documented in the fn's own comment): a non-existent path is not \
         canonicalised, so its spellings do not fold. Low impact — a run needs a real workspace — \
         but it is the fallback's one soft edge."
    );
}

/// The phase-type validator closes the key's other half: free text cannot start a second search
/// that looks like the real one.
#[test]
fn qa_a3_the_phase_type_validator_accepts_only_the_closed_set() {
    for good in KNOWN_PHASE_TYPES {
        assert_eq!(&validate_phase_type(good).expect("known"), good);
    }
    assert_eq!(validate_phase_type("  dev-phase  ").expect("trimmed"), "dev-phase");
    for bad in [
        "", "   ", "dev", "devphase", "dev_phase", "Dev-Phase", "dev-phase ,ui-phase",
        "planning", "review-phase", "../dev-phase", "dev-phase\u{200B}",
    ] {
        let refused = validate_phase_type(bad);
        assert!(refused.is_err(), "{bad:?} must be refused");
        assert!(
            refused.unwrap_err().contains("is not a phase type"),
            "{bad:?}"
        );
    }
    // The closed set is the one the design names, and nothing else.
    assert_eq!(KNOWN_PHASE_TYPES, &["planning-phase", "dev-phase", "ui-phase"]);
}

// =================================================================================================
// ITEM 7 — the cross-branch `fleet_busy` contract. Settled against Dev B's real output.
// =================================================================================================

/// **DEFECT. Dev B's pool does NOT produce what Dev A's rule expects, and the mismatch is an
/// ordering question with a one-line answer.**
///
/// These are not invented strings. They are the exact literals Dev B's own test
/// (`leases.rs::the_snapshot_columns_are_json_a_run_can_store` on `feat/arena-b`) asserts
/// `LeasePool::fleet_busy_json()` returns, before and after a lease is acquired, on the measured
/// cap table.
///
/// `LeasePool::busy()` returns TOTAL seats held per family. There is no exclusion of the caller's
/// own lease and no `busy_excluding(..)` variant anywhere in that module. So:
///
/// * snapshot taken BEFORE `acquire` → all families at 0 → Dev A's `parse_seat_map` drops the zeros
///   → empty → QUIET → scoreable. **This is the contract.**
/// * snapshot taken AFTER `acquire` → the run's own seats appear → non-empty → UNSCOREABLE.
///   Every run, forever, so nothing ever promotes.
///
/// Dev A's own integration note prescribes the second order: *"the lease pool, once it is wired,
/// will take a lease and then start the run with the snapshot it just took."* That is the losing
/// order. The fix is to snapshot first and acquire second — which is also correct on its own terms,
/// because a concurrent run's seats SHOULD appear in this run's snapshot while its own should not.
///
/// The direction is safe (a refusal, never a false crowning), exactly as Dev A reasoned. But a
/// feature that silently promotes nothing is the failure mode they themselves called worse than a
/// loud one, so this needs the order fixed at integration, not discovered afterwards.
#[test]
fn qa_a3_dev_bs_pool_matches_the_contract_only_if_snapshotted_before_acquire() {
    // Verbatim from feat/arena-b leases.rs.
    const BEFORE_ACQUIRE: &str =
        r#"{"deepseek-v4.1-flash":0,"glm-5.3-flash":0,"qwen3.8-flash-next":0}"#;
    const AFTER_ACQUIRE: &str =
        r#"{"deepseek-v4.1-flash":0,"glm-5.3-flash":2,"qwen3.8-flash-next":0}"#;

    // Snapshot-before-acquire satisfies the contract: all-zero reads as quiet, because
    // `parse_seat_map` drops zero entries.
    assert!(
        parse_seat_map(BEFORE_ACQUIRE).expect("parses").is_empty(),
        "an all-zero snapshot must normalise to the empty map"
    );
    let quiet = run_with_contention(BEFORE_ACQUIRE);
    assert!(
        run_is_scoreable(&quiet).is_none(),
        "snapshot-before-acquire is SCOREABLE — the contract holds in this order"
    );

    // Snapshot-after-acquire does not.
    let own_seats_included = run_with_contention(AFTER_ACQUIRE);
    let block = run_is_scoreable(&own_seats_included);
    assert!(
        block.is_some(),
        "DEFECT PIN: the run's own 2 seats read as foreign contention, so it is unscoreable"
    );
    let block = block.unwrap();
    assert!(
        block.contains("2 foreign seats were held"),
        "and the refusal blames the run for its own lease: {block}"
    );

    // Two such runs are not rescued by the identity escape either, as soon as their seat counts
    // differ at all — which is the normal case, since fan_out is what the arena varies.
    let four_seats = r#"{"deepseek-v4.1-flash":0,"glm-5.3-flash":4,"qwen3.8-flash-next":0}"#;
    assert!(
        !contention_was_identical(AFTER_ACQUIRE, four_seats),
        "DEFECT PIN: a 2-seat run and a 4-seat run are not comparable under the wrong order, so \
         the arena cannot compare anything"
    );

    // The one case the wrong order does NOT break: two runs of identical width, via the escape.
    assert!(
        contention_was_identical(AFTER_ACQUIRE, AFTER_ACQUIRE),
        "identical widths survive by accident, which is why this could pass a shallow smoke test"
    );
}

/// Dev B's pool deliberately keeps idle families at `0` so that "quiet" stays distinguishable from
/// "not recorded" — and Dev A's `parse_seat_map` deliberately DROPS zero entries, erasing exactly
/// that distinction. Two documented, opposite intentions about the same bytes.
///
/// Behaviourally the erasure is what makes the contract work at all (it is what turns Dev B's
/// all-zero snapshot into "quiet"), so this is not a live bug. It is a guarantee Dev B's module
/// documents and provides, which the only consumer destroys on read — so anyone who later relies on
/// it will be wrong. Worth reconciling in prose at integration.
#[test]
fn qa_a3_the_zero_entry_distinction_dev_b_preserves_is_erased_on_read() {
    let all_zero = r#"{"deepseek-v4.1-flash":0,"glm-5.3-flash":0,"qwen3.8-flash-next":0}"#;
    let one_family_zero = r#"{"glm-5.3-flash":0}"#;
    let absent = "{}";
    for snapshot in [all_zero, one_family_zero, absent] {
        assert!(
            parse_seat_map(snapshot).expect("parses").is_empty(),
            "{snapshot} normalises to empty, so the three are indistinguishable after reading"
        );
    }
    assert!(
        contention_was_identical(all_zero, absent),
        "an all-zero snapshot and an empty one compare EQUAL, which is the distinction Dev B's \
         doc comment says has to survive for the comparability check"
    );
}

/// A run with contention the test chooses, so the contract can be exercised without the pool.
fn run_with_contention(fleet_busy: &str) -> WorkflowRun {
    WorkflowRun {
        id: "r".to_string(),
        workflow_id: "w".to_string(),
        plan_id: None,
        seed: 0,
        gate_exit: Some(0),
        gate_collected: None,
        seats_held: r#"{"glm-5.3-flash":2}"#.to_string(),
        fleet_busy: fleet_busy.to_string(),
        started_at: "now".to_string(),
        ended_at: None,
        scoreable: true,
        not_scoreable_reason: None,
    }
}

// =================================================================================================
// ITEM 8 — pin the collapsed-literal grep as a test
// =================================================================================================

/// Dev A verified by a one-off grep that no collapsed string literals remain in the refusal text a
/// human reads. A grep run once is not a guard, and this is the second time a multi-line literal
/// without a `\` continuation has put a run of spaces into a user-facing message — so it is pinned.
///
/// Every refusal `promotion_block` and `preference_comparability` can produce is generated here and
/// checked for an internal run of whitespace, which is what a missing continuation looks like.
#[test]
fn qa_a3_no_refusal_a_human_reads_contains_a_collapsed_literal() {
    let (state, _root) = test_state();
    let seats = r#"{"glm":2}"#;
    let incumbent = workflow(&state, KEY.0, KEY.1, KEY.2);
    let challenger = workflow(&state, KEY.0, KEY.1, KEY.2);
    let other_family = workflow(&state, KEY.0, KEY.1, "qwen");

    let run = |w: &Workflow, fleet: &str| -> WorkflowRun {
        let r = start_workflow_run_with_budget(
            &state,
            NewWorkflowRun {
                workflow_id: w.id.clone(),
                plan_id: None,
                seed: 0,
                seats: None,
            },
            RunBudget {
                seats_held: seats.to_string(),
                fleet_busy: fleet.to_string(),
            },
        )
        .expect("start");
        record_workflow_run_gate(&state, &r.id, 0, None).expect("gate")
    };

    let mut messages: Vec<String> = Vec::new();

    // Every `preference_comparability` refusal, via the stored `reason`.
    let a = run(&incumbent, "{}");
    let b = run(&challenger, "{}");
    let cross_family = run(&other_family, "{}");
    let busy = run(&challenger, r#"{"qwen":3}"#);
    for (winner, loser) in [(&a, &cross_family), (&a, &busy), (&busy, &a)] {
        if let Some(reason) = record_workflow_pref(&state, &winner.id, &loser.id, None)
            .expect("pref")
            .reason
        {
            messages.push(reason);
        }
    }
    // A mismatched seat budget, which needs a different width.
    let narrow = start_workflow_run_with_budget(
        &state,
        NewWorkflowRun {
            workflow_id: challenger.id.clone(),
            plan_id: None,
            seed: 0,
            seats: None,
        },
        RunBudget {
            seats_held: r#"{"glm":9}"#.to_string(),
            fleet_busy: "{}".to_string(),
        },
    )
    .expect("start");
    let narrow = record_workflow_run_gate(&state, &narrow.id, 0, None).expect("gate");
    if let Some(reason) = record_workflow_pref(&state, &a.id, &narrow.id, None)
        .expect("pref")
        .reason
    {
        messages.push(reason);
    }

    // Every `promote_workflow` refusal.
    promote_workflow(&state, &a.id, None, true).expect("seed champion");
    let pref_cross = record_workflow_pref(&state, &b.id, &cross_family.id, Some("x".into()))
        .expect("pref");
    let pref_wrong_loser = record_workflow_pref(&state, &b.id, &narrow.id, Some("y".into()))
        .expect("pref");
    let pref_self = record_workflow_pref(&state, &b.id, &run(&challenger, "{}").id, Some("z".into()))
        .expect("pref");
    let unscored = start_against(&state, &challenger.id, None);
    for attempt in [
        promote_workflow(&state, &b.id, None, true),
        promote_workflow(&state, &b.id, Some(&pref_cross.id), true),
        promote_workflow(&state, &b.id, Some(&pref_wrong_loser.id), true),
        promote_workflow(&state, &b.id, Some(&pref_self.id), true),
        promote_workflow(&state, &unscored.id, None, true),
        promote_workflow(&state, &a.id, None, false),
        promote_workflow(&state, &b.id, Some(&pref_cross.id), false),
    ] {
        if let Err(message) = attempt {
            messages.push(message);
        }
    }
    // And the refusals from the other human-facing guards.
    messages.push(validate_phase_type("nope").unwrap_err());
    messages.push(reject_workflow_run(&state, &a.id, " ").unwrap_err());
    messages.push(resume_workflow_search(&state, KEY.0, KEY.1, KEY.2, "x", true).unwrap_err());
    messages.push(resume_workflow_search(&state, KEY.0, KEY.1, KEY.2, "x", false).unwrap_err());
    messages.push(record_workflow_pref(&state, &a.id, &a.id, None).unwrap_err());
    messages.push(record_workflow_run_gate(&state, &a.id, 0, None).unwrap_err());
    messages.push(record_workflow_run_gate(&state, &b.id, i64::MAX, None).unwrap_err());
    messages.push(
        create_workflow_version(&state, version_with(KEY.0, KEY.1, KEY.2, 0)).unwrap_err(),
    );

    assert!(
        messages.len() >= 15,
        "the sweep must actually reach the refusals: only {} collected",
        messages.len()
    );
    let collapsed: Vec<&String> = messages
        .iter()
        .filter(|message| {
            // A missing `\` continuation leaves the source indentation inside the string: a run of
            // spaces with visible text on both sides. Three or more, so ". Two" style is fine.
            message
                .split_whitespace()
                .count()
                .checked_sub(1)
                .is_some_and(|_| message.contains("   "))
        })
        .collect();
    assert!(
        collapsed.is_empty(),
        "a refusal a human reads contains a collapsed literal (a run of 3+ spaces from a \
         multi-line string without a `\\` continuation): {collapsed:#?}"
    );
    // Non-vacuous: the detector fires on the shape it is looking for.
    let broken = "a champion is keyed on              that triple";
    assert!(broken.contains("   "), "the detector's own premise");
}
