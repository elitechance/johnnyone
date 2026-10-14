//! QA A's adversarial tests against the workflow arena (`services::workflows`, migration 026).
//!
//! A separate module from the author's own `workflows::tests` on purpose: these exist to FAIL, and
//! several of them pass only because the implementation has a hole. Each test names, in its own
//! assertion message, whether it is pinning intended behaviour or documenting a defect — so a
//! later reader is never left guessing which side of the line a green test is on.
//!
//! Nothing here is a fix. Where a test documents a hole it asserts the CURRENT behaviour and says
//! so loudly, so that closing the hole turns the test red and forces a deliberate update rather
//! than letting the hole quietly persist behind a green suite.

#![cfg(test)]

use crate::services::workflows::*;
use crate::state::app_state::AppState;
use crate::test_support::test_state;
use rusqlite::params;

/// Round 3: `workflows.repo` is no longer a caller-supplied string — the host DERIVES it from a
/// workspace path (`repo_identity`). These tests still want to talk about "repo" and "other-repo"
/// as logical names, so this maps a logical name to a stable synthetic path and lets the host
/// derive the identity from it exactly as it would in production.
///
/// The path deliberately does not exist: `repo_identity` then finds no git remote and falls back to
/// the absolute path, which is deterministic and distinct per name. No filesystem, no git, no
/// dependence on the machine this runs on.
fn repo_path(name: &str) -> String {
    format!("/nonexistent-arena-tests/{}", name)
}

fn repo_id(name: &str) -> String {
    repo_identity(&repo_path(name)).expect("derive the repo identity")
}

// --- Test shims over the three service functions that take the derived identity. ---
//
// They SHADOW the glob-imported originals deliberately, so that every call site in this file can go
// on naming the logical repo. An explicit local item wins over a glob import, so `workflow_search_state("repo", …)`
// below is this shim, which translates and then calls the real one. Nothing about the rule under
// test changes — only how the key is spelled at the call site.

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

fn list_workflows(
    state: &AppState,
    repo: Option<&str>,
    phase_type: Option<&str>,
    family: Option<&str>,
) -> Result<Vec<Workflow>, String> {
    let repo = repo.map(repo_id);
    crate::services::workflows::list_workflows(state, repo.as_deref(), phase_type, family)
}

fn version(repo: &str, phase_type: &str, family: &str) -> NewWorkflowVersion {
    NewWorkflowVersion {
        workspace_path: repo_path(repo),
        phase_type: phase_type.to_string(),
        family: family.to_string(),
        parent_id: None,
        role: "role".to_string(),
        instruction: "instruction".to_string(),
        contract: "contract".to_string(),
        hop: r#"{"steps":["edit"]}"#.to_string(),
        fan_out: 1,
    }
}

fn workflow(state: &AppState, repo: &str, phase_type: &str, family: &str) -> Workflow {
    create_workflow_version(state, version(repo, phase_type, family)).expect("create workflow")
}

/// Round 2: `seats_held` / `fleet_busy` are no longer caller parameters — the HOST measures both,
/// because the caller is a browser that can measure neither. Tests still need to construct specific
/// contention in order to exercise the comparability rule at all, so this helper goes through
/// `start_workflow_run_with_budget`, the host-internal seam the lease pool will fill. That seam is
/// deliberately NOT reachable over RPC or GraphQL.
fn run_with(state: &AppState, workflow_id: &str, seats: &str, fleet: &str) -> WorkflowRun {
    start_workflow_run_with_budget(
        state,
        NewWorkflowRun {
            workflow_id: workflow_id.to_string(),
            plan_id: None,
            seed: 0,
            seats: None,
        },
        RunBudget {
            seats_held: seats.to_string(),
            fleet_busy: fleet.to_string(),
        },
    )
    .expect("start run")
}

/// A green run: gate exit 0, with an advisory collected count.
fn green(state: &AppState, workflow_id: &str, seats: &str) -> WorkflowRun {
    let run = run_with(state, workflow_id, seats, "{}");
    record_workflow_run_gate(state, &run.id, 0, Some(7)).expect("record gate")
}

fn champion_of(state: &AppState, repo: &str, phase_type: &str, family: &str) -> Option<String> {
    state
        .db
        .with_conn(|conn| {
            conn.query_row(
                "SELECT id FROM workflows WHERE repo = ?1 AND phase_type = ?2 AND family = ?3 \
                 AND is_champion = 1",
                params![repo_id(repo), phase_type, family],
                |row| row.get::<_, String>(0),
            )
            .map_err(|e| e.to_string())
            .ok()
            .into_iter()
            .next()
            .map(Ok)
            .unwrap_or(Ok(String::new()))
        })
        .ok()
        .filter(|id: &String| !id.is_empty())
}

/// Every row in `workflow_decisions`, oldest first, as `(kind, run_id)`.
fn decision_log(state: &AppState) -> Vec<(String, String)> {
    state
        .db
        .with_conn(|conn| {
            let mut stmt = conn
                .prepare("SELECT kind, run_id FROM workflow_decisions ORDER BY seq ASC")
                .map_err(|e| e.to_string())?;
            let rows = stmt
                .query_map([], |row| {
                    Ok((row.get::<_, String>(0)?, row.get::<_, String>(1)?))
                })
                .map_err(|e| e.to_string())?;
            let mut out = Vec::new();
            for row in rows {
                out.push(row.map_err(|e| e.to_string())?);
            }
            Ok(out)
        })
        .expect("decision log")
}

// =================================================================================================
// CLAIM 3 — "three consecutive rejections halt the search"
// =================================================================================================

/// **FIXED (round 2). Inverted from the defect pin of the same shape.** Re-promoting the workflow
/// that is already champion does NOT escape a halt.
///
/// What was wrong: `promote_workflow` filtered the incumbent out when the incumbent IS the run's
/// own workflow, so `promotion_block` saw `incumbent = None, pref = None` and took the "seeding the
/// first champion" branch. The row change was a no-op but a `promote` decision was appended, and
/// the streak walk counted rejections only back to the last `promote` — so one phantom promotion
/// reset a three-strike halt. The halt was a speed bump any caller holding one already-green
/// champion run could step over in a single call.
///
/// Two independent changes close it, and this test pins both: a promotion is REFUSED while a halt
/// is in force, and only an explicit, human-confirmed `resume` resets the streak.
#[test]
fn qa_a_repromoting_the_sitting_champion_does_not_escape_the_halt() {
    let (state, _root) = test_state();
    let champion = workflow(&state, "repo", "dev-phase", "glm");
    let champion_run = green(&state, &champion.id, r#"{"glm":1}"#);
    promote_workflow(&state, &champion_run.id, None, true).expect("seed the first champion");

    // Three rejections, each on its own fresh run, halt the family.
    for strike in 1..=3 {
        let run = run_with(&state, &champion.id, r#"{"glm":1}"#, "{}");
        reject_workflow_run(&state, &run.id, &format!("strike {strike}")).expect("reject");
    }
    let halted = workflow_search_state(&state, "repo", "dev-phase", "glm").expect("state");
    assert!(halted.halted, "three rejections must halt the search");
    assert!(
        start_workflow_run(
            &state,
            NewWorkflowRun {
                workflow_id: champion.id.clone(),
                plan_id: None,
                seed: 0,
                seats: None,
            },
        )
        .is_err(),
        "a halted family must refuse a new run"
    );

    // THE CLOSED HOLE: re-promote the sitting champion using the SAME run that seeded it. No
    // preference, no new gate, nothing proven — and it is refused.
    let repromoted = promote_workflow(&state, &champion_run.id, None, true);
    let error = repromoted.expect_err(
        "re-promoting the sitting champion must not be a way out of a halt",
    );
    assert!(
        error.contains("is halted"),
        "the refusal must name the halt as the reason: {error}"
    );
    let after = workflow_search_state(&state, "repo", "dev-phase", "glm").expect("state");
    assert_eq!(
        after.consecutive_rejections, 3,
        "the streak survives a refused promotion"
    );
    assert!(after.halted, "and so does the halt");
    // The search really is still blocked, which is what makes this more than a counter.
    assert!(
        start_workflow_run(
            &state,
            NewWorkflowRun {
                workflow_id: champion.id.clone(),
                plan_id: None,
                seed: 0,
                seats: None,
            },
        )
        .is_err(),
        "still halted, so still no new runs"
    );
    // The champion is untouched: a refused promotion changes nothing.
    assert_eq!(
        champion_of(&state, "repo", "dev-phase", "glm").as_deref(),
        Some(champion.id.as_str())
    );

    // And the ONE way out: an explicit, human-confirmed resume carrying a reason.
    let resumed = resume_workflow_search(
        &state,
        "repo",
        "dev-phase",
        "glm",
        "the gate was misconfigured for all three",
        true,
    )
    .expect("resume");
    assert!(!resumed.halted && resumed.consecutive_rejections == 0);
    run_with(&state, &champion.id, r#"{"glm":1}"#, "{}");
}

/// **FIXED (round 2). Inverted from the defect pin of the same shape.** Rejecting ONE run three
/// times is ONE strike.
///
/// What was wrong: nothing deduplicated by `run_id`, so "three consecutive rejections" was really
/// "three rejection ROWS" — a caller that retried a rejection, or a UI with a double-submitting
/// button, halted the search on a single human verdict. `workflow_search_state` now counts DISTINCT
/// `run_id`s in the trailing rejections.
///
/// The rows are still all written: the log records that the human pressed three times, which is
/// true and worth keeping. It is the COUNTING that changed, not the audit trail.
#[test]
fn qa_a_rejecting_one_run_three_times_is_one_strike() {
    let (state, _root) = test_state();
    let w = workflow(&state, "repo", "dev-phase", "glm");
    let run = run_with(&state, &w.id, "{}", "{}");

    for strike in 1..=3 {
        let state_after =
            reject_workflow_run(&state, &run.id, &format!("same run, strike {strike}"))
                .expect("reject");
        assert_eq!(
            state_after.consecutive_rejections, 1,
            "strike {strike}: one candidate is one strike however many times it is rejected"
        );
        assert!(!state_after.halted);
    }
    let after = workflow_search_state(&state, "repo", "dev-phase", "glm").expect("state");
    assert!(
        !after.halted,
        "three rejections of ONE run must not halt the search"
    );
    assert_eq!(
        after.recent_rejections.len(),
        1,
        "and the reasons are per candidate, not per tap: {:?}",
        after.recent_rejections
    );
    // Every row is still recorded — the audit trail keeps all three taps.
    let log = decision_log(&state);
    assert_eq!(log.len(), 3);
    assert!(
        log.iter().all(|(kind, id)| kind == "reject" && id == &run.id),
        "all three rows are the same run: {log:?}"
    );
    // THREE DISTINCT candidates do halt it, which is what makes the dedup a fix and not a hole.
    for strike in 2..=3 {
        let other = run_with(&state, &w.id, "{}", "{}");
        let after = reject_workflow_run(&state, &other.id, &format!("candidate {strike}"))
            .expect("reject");
        assert_eq!(after.consecutive_rejections, strike);
    }
    assert!(
        workflow_search_state(&state, "repo", "dev-phase", "glm")
            .expect("state")
            .halted,
        "three DISTINCT candidates must halt the search"
    );
}

/// Rejections in DIFFERENT families do not accumulate against each other. Pinned because the
/// decision log is one global table and the family filter lives in a JOIN that a future edit could
/// drop without any other test noticing.
#[test]
fn qa_a_rejections_do_not_cross_family_or_repo_boundaries() {
    let (state, _root) = test_state();
    let glm = workflow(&state, "repo", "dev-phase", "glm");
    let qwen = workflow(&state, "repo", "dev-phase", "qwen");
    let other_repo = workflow(&state, "other-repo", "dev-phase", "glm");
    let other_phase = workflow(&state, "repo", "ui-phase", "glm");

    for w in [&glm, &qwen, &other_repo, &other_phase] {
        let run = run_with(&state, &w.id, "{}", "{}");
        reject_workflow_run(&state, &run.id, "no").expect("reject");
    }
    for (repo, phase, family) in [
        ("repo", "dev-phase", "glm"),
        ("repo", "dev-phase", "qwen"),
        ("other-repo", "dev-phase", "glm"),
        ("repo", "ui-phase", "glm"),
    ] {
        let s = workflow_search_state(&state, repo, phase, family).expect("state");
        assert_eq!(
            s.consecutive_rejections, 1,
            "({repo}, {phase}, {family}) must count only its own rejections"
        );
        assert!(!s.halted);
    }
}

/// Forced `created_at` TIES. The author's argument is that `seq AUTOINCREMENT` makes ordering safe
/// when timestamps collide; this proves it by rewriting every `created_at` to one identical value
/// and checking that the decision recorded LAST still ends the streak.
///
/// **Round 2: rewritten, because the round-1 version became VACUOUS.** It rejected ONE run three
/// times with a promotion in the middle, and asserted a count of 1. Once rejections were counted by
/// DISTINCT run and a promotion stopped ending a streak, that count was still 1 — but for an
/// entirely different reason (dedup, not ordering), so the test no longer touched `seq` at all. It
/// now uses three DISTINCT runs and a `resume`, which is the decision that ends a streak, so the
/// assertion is load-bearing on the ordering again.
#[test]
fn qa_a_identical_created_at_does_not_break_the_consecutive_rule() {
    let (state, _root) = test_state();
    let w = workflow(&state, "repo", "dev-phase", "glm");
    let a = run_with(&state, &w.id, "{}", "{}");
    let b = run_with(&state, &w.id, "{}", "{}");
    let c = run_with(&state, &w.id, "{}", "{}");

    reject_workflow_run(&state, &a.id, "a").expect("reject");
    reject_workflow_run(&state, &b.id, "b").expect("reject");
    reject_workflow_run(&state, &c.id, "c").expect("reject");
    // Three distinct candidates: halted, so there is something to resume.
    resume_workflow_search(&state, "repo", "dev-phase", "glm", "resumed", true).expect("resume");
    let d = run_with(&state, &w.id, "{}", "{}");
    reject_workflow_run(&state, &d.id, "d").expect("reject");

    // Every decision now shares one timestamp — the exact tie `seq` exists for.
    state
        .db
        .with_conn(|conn| {
            conn.execute(
                "UPDATE workflow_decisions SET created_at = '2026-01-01T00:00:00Z'",
                [],
            )
            .map_err(|e| e.to_string())
        })
        .expect("force the tie");
    let distinct: i64 = state
        .db
        .with_conn(|conn| {
            conn.query_row(
                "SELECT COUNT(DISTINCT created_at) FROM workflow_decisions",
                [],
                |row| row.get(0),
            )
            .map_err(|e| e.to_string())
        })
        .expect("count");
    assert_eq!(distinct, 1, "the tie must really be forced");

    let s = workflow_search_state(&state, "repo", "dev-phase", "glm").expect("state");
    assert_eq!(
        s.consecutive_rejections, 1,
        "with all timestamps identical, only the rejection AFTER the resume may count"
    );
    assert_eq!(s.recent_rejections, vec!["d".to_string()]);
}

/// Creating a new workflow VERSION is not a way out of a halt: the halt is keyed on
/// `(repo, phase_type, family)` and a new version is in the same family.
#[test]
fn qa_a_a_new_version_does_not_escape_the_halt() {
    let (state, _root) = test_state();
    let v1 = workflow(&state, "repo", "dev-phase", "glm");
    for strike in 1..=3 {
        let run = run_with(&state, &v1.id, "{}", "{}");
        reject_workflow_run(&state, &run.id, &format!("s{strike}")).expect("reject");
    }

    // Creating v2 is allowed (it writes no run) …
    let v2 = create_workflow_version(
        &state,
        NewWorkflowVersion {
            parent_id: Some(v1.id.clone()),
            ..version("repo", "dev-phase", "glm")
        },
    )
    .expect("a halted family may still gain a version");
    assert_eq!(v2.version, 2);

    // … but running it is not.
    let blocked = start_workflow_run(
        &state,
        NewWorkflowRun {
            workflow_id: v2.id.clone(),
            plan_id: None,
            seed: 0,
            seats: None,
        },
    );
    assert!(
        blocked.is_err(),
        "a new version must not be a way around the halt"
    );
    assert!(blocked.unwrap_err().contains("is halted after 3"));
}

// =================================================================================================
// CLAIM 2 / 5 — champion uniqueness and "not comparable cannot promote"
// =================================================================================================

/// **FIXED (round 2). Inverted from the defect pin of the same shape.** A preference whose loser is
/// a run of a DIFFERENT `(repo, phase_type, family)` cannot displace the incumbent.
///
/// What was wrong: `promotion_block` checked three things about the preference — it exists, it names
/// this run as winner, and it is `comparable` — and never what the LOSER was.
/// `record_workflow_pref` only compared seat maps. So "displacing an incumbent needs a comparable
/// head-to-head" was satisfied by a head-to-head against a run that was never the incumbent, in a
/// family the design says is a different IDENTITY. `family` is the design's central claim: a hop
/// tuned for one context window does not transfer, so a challenger that beat a Qwen run must not
/// thereby become the GLM champion.
///
/// Closed in two independent places, and this test pins both: `preference_comparability` marks a
/// cross-key pair NOT comparable when the preference is recorded, and `promotion_block` refuses a
/// loser outside the run's own key even if something else had set the flag.
#[test]
fn qa_a_a_pref_against_another_family_cannot_displace_the_champion() {
    let (state, _root) = test_state();
    let seats = r#"{"glm":2}"#;
    let incumbent = workflow(&state, "repo", "dev-phase", "glm");
    let incumbent_run = green(&state, &incumbent.id, seats);
    promote_workflow(&state, &incumbent_run.id, None, true).expect("seed champion");

    let challenger = workflow(&state, "repo", "dev-phase", "glm");
    let challenger_run = green(&state, &challenger.id, seats);

    // A run in a DIFFERENT family, on the same seat budget so the budgets compare equal.
    let unrelated = workflow(&state, "repo", "dev-phase", "qwen");
    let unrelated_run = green(&state, &unrelated.id, seats);

    // A cross-family preference is still RECORDABLE — it happened, and `workflow_prefs` is the
    // project-level measurement — but it is not comparable, and the reason says why.
    let pref = record_workflow_pref(&state, &challenger_run.id, &unrelated_run.id, None)
        .expect("a cross-family preference is still data");
    assert!(
        !pref.comparable,
        "a cross-key pair measures two different things rather than comparing one"
    );
    assert!(
        pref.reason
            .as_deref()
            .unwrap_or_default()
            .contains("different (repo, phase_type, family)"),
        "the recorded reason must say why it is not comparable: {:?}",
        pref.reason
    );

    let error = promote_workflow(&state, &challenger_run.id, Some(&pref.id), true)
        .expect_err("beating a QWEN run must not make a workflow the GLM champion");
    assert!(error.contains("not comparable"), "got: {error}");
    assert_eq!(
        champion_of(&state, "repo", "dev-phase", "glm").as_deref(),
        Some(incumbent.id.as_str()),
        "the incumbent keeps the slot"
    );

    // And the second, independent gate: even with the flag forced to 1 behind the service's back,
    // `promotion_block` still refuses a loser outside the run's own champion key. Belt and braces
    // on the design's central identity claim.
    state
        .db
        .with_conn(|conn| {
            conn.execute(
                "UPDATE workflow_prefs SET comparable = 1 WHERE id = ?1",
                rusqlite::params![pref.id],
            )
            .map_err(|e| e.to_string())
        })
        .expect("force the flag");
    let forced = promote_workflow(&state, &challenger_run.id, Some(&pref.id), true)
        .expect_err("the loser's key is checked independently of the flag");
    assert!(
        forced.contains("different (repo, phase_type, family)"),
        "got: {forced}"
    );
    assert_eq!(
        champion_of(&state, "repo", "dev-phase", "glm").as_deref(),
        Some(incumbent.id.as_str())
    );
}

/// **FIXED (round 2). Inverted from the defect pin of the same shape.** A workflow cannot displace
/// the champion by beating ITSELF.
///
/// This was the sharpest form of the hole above: two runs of the same workflow on the same seat
/// budget made a `comparable` preference and `promotion_block` accepted it, with the incumbent not
/// involved at all. `promotion_block` now refuses a loser that is a run of the same workflow, and
/// separately requires a displacing loser to be the run the incumbent was promoted on.
#[test]
fn qa_a_a_workflow_cannot_displace_the_champion_by_beating_itself() {
    let (state, _root) = test_state();
    let seats = r#"{"glm":2}"#;
    let incumbent = workflow(&state, "repo", "dev-phase", "glm");
    let incumbent_run = green(&state, &incumbent.id, seats);
    promote_workflow(&state, &incumbent_run.id, None, true).expect("seed champion");

    let challenger = workflow(&state, "repo", "dev-phase", "glm");
    let win = green(&state, &challenger.id, seats);
    let lose = green(&state, &challenger.id, seats);
    // Two runs of one workflow in one key ARE comparable as measurements — same seats, same
    // contention, same family. What they are not is a head-to-head.
    let pref = record_workflow_pref(&state, &win.id, &lose.id, None).expect("self head-to-head");
    assert!(pref.comparable, "the two measurements really are comparable");

    let error = promote_workflow(&state, &win.id, Some(&pref.id), true)
        .expect_err("a workflow beating its own other run is not a head-to-head");
    assert!(error.contains("SAME workflow"), "got: {error}");
    assert_eq!(
        champion_of(&state, "repo", "dev-phase", "glm").as_deref(),
        Some(incumbent.id.as_str()),
        "the incumbent keeps the slot"
    );
    assert!(get_workflow(&state, &incumbent.id).unwrap().is_champion);

    // The legitimate route still works: beat the INCUMBENT's run.
    let honest = record_workflow_pref(&state, &win.id, &incumbent_run.id, None).expect("pref");
    assert!(honest.comparable);
    promote_workflow(&state, &win.id, Some(&honest.id), true).expect("a real head-to-head");
    assert_eq!(
        champion_of(&state, "repo", "dev-phase", "glm").as_deref(),
        Some(challenger.id.as_str())
    );
    // And now the atomic swap is visible: the old champion is demoted in the same transaction.
    assert!(!get_workflow(&state, &incumbent.id).unwrap().is_champion);
}

/// **FIXED (round 2). Inverted from the defect pin of the same shape.** `comparable` now requires
/// matching `fleet_busy` as well as matching `seats_held`.
///
/// The design's stated reason for `fleet_busy` is: "Two concurrent runs contend, there is no 429 to
/// notice, and the slower run looks like the worse workflow. Without these, the first thing a busy
/// fleet does is promote a champion because the other box was quiet." The rule read `seats_held`
/// ONLY, so two runs that held the same seats — one on an idle fleet, one on a saturated one —
/// were `comparable: true` and the preference between them promoted: exactly the failure the column
/// was added to prevent.
///
/// `budgets_are_comparable` keeps its name and its meaning (the SEAT half, and these two runs do
/// pass it). The flag that gets stored is now decided by `preference_comparability`, which also
/// consults `contention_is_comparable`. Equality, not a tolerance — a tolerance needs a threshold
/// and a threshold is an opinion.
#[test]
fn qa_a_comparable_requires_matching_fleet_contention() {
    let (state, _root) = test_state();
    let seats = r#"{"glm":2}"#;
    let w = workflow(&state, "repo", "dev-phase", "glm");

    let quiet = run_with(&state, &w.id, seats, r#"{"glm":0}"#);
    let quiet = record_workflow_run_gate(&state, &quiet.id, 0, None).expect("gate");
    let saturated = run_with(&state, &w.id, seats, r#"{"glm":4096,"qwen":512}"#);
    let saturated = record_workflow_run_gate(&state, &saturated.id, 1, None).expect("gate");

    // The SEAT half still passes: both runs held the same seats. That is the point — seat equality
    // was never the thing that was missing.
    assert!(
        budgets_are_comparable(&quiet.seats_held, &saturated.seats_held),
        "the seat budgets really are identical"
    );
    // The CONTENTION half does not.
    assert!(
        run_is_scoreable(&saturated).is_some(),
        "an idle fleet and a saturated one are not the same contention"
    );

    let pref = record_workflow_pref(&state, &quiet.id, &saturated.id, None).expect("pref");
    assert!(
        !pref.comparable,
        "a run on an idle fleet and a run on a saturated fleet must not be comparable"
    );
    // Round 3: the rule EXCLUDES rather than compares, so the recorded reason names the
    // unscoreable run and the contention it saw, rather than "the two differed".
    assert!(
        pref.reason
            .as_deref()
            .unwrap_or_default()
            .contains("unscoreable"),
        "the recorded reason must name the unscoreable run: {:?}",
        pref.reason
    );
    assert!(
        pref.reason
            .as_deref()
            .unwrap_or_default()
            .contains("the fleet was not quiet"),
        "and the contention it saw: {:?}",
        pref.reason
    );
    let error = promote_workflow(&state, &quiet.id, Some(&pref.id), true)
        .expect_err("and that preference must not promote");
    assert!(error.contains("not comparable"), "got: {error}");
    assert_eq!(champion_of(&state, "repo", "dev-phase", "glm"), None);

    // ROUND 4: identical contention is NO LONGER an escape. Two runs that saw the same foreign
    // load are both unscoreable and the pair is REFUSED — this is the one row that differs from
    // both earlier rules, and therefore the whole behavioural change the final ruling asked for.
    let a = run_with(&state, &w.id, seats, r#"{"glm":4}"#);
    let a = record_workflow_run_gate(&state, &a.id, 0, None).expect("gate");
    let b = run_with(&state, &w.id, seats, r#"{"glm":4}"#);
    let b = record_workflow_run_gate(&state, &b.id, 1, None).expect("gate");
    assert!(
        !record_workflow_pref(&state, &a.id, &b.id, None)
            .expect("pref")
            .comparable,
        "identical foreign contention is not a like-for-like escape: the arena scores on a QUIET fleet"
    );
    // And a QUIET pair IS comparable, so the rule discriminates on contention rather than simply
    // refusing everything.
    let quiet_a = run_with(&state, &w.id, seats, "{}");
    let quiet_a = record_workflow_run_gate(&state, &quiet_a.id, 0, None).expect("gate");
    let quiet_b = run_with(&state, &w.id, seats, "{}");
    let quiet_b = record_workflow_run_gate(&state, &quiet_b.id, 1, None).expect("gate");
    assert!(record_workflow_pref(&state, &quiet_a.id, &quiet_b.id, None)
        .expect("pref")
        .comparable);
}

/// A promotion that is REFUSED must leave the incumbent intact and write nothing — no demotion, no
/// decision row. This is the "does the incumbent survive a promotion that fails" question, and it
/// is the only reachable rollback case: every validation in `promote_workflow` runs before every
/// write, so there is no public path on which a partial write has to be rolled back.
#[test]
fn qa_a_a_refused_promotion_leaves_the_incumbent_and_the_log_untouched() {
    let (state, _root) = test_state();
    let seats = r#"{"glm":1}"#;
    let incumbent = workflow(&state, "repo", "dev-phase", "glm");
    let incumbent_run = green(&state, &incumbent.id, seats);
    promote_workflow(&state, &incumbent_run.id, None, true).expect("seed champion");
    let log_before = decision_log(&state);

    let challenger = workflow(&state, "repo", "dev-phase", "glm");

    // (a) a run whose gate never ran
    let unscored = run_with(&state, &challenger.id, seats, "{}");
    assert!(promote_workflow(&state, &unscored.id, None, true)
        .unwrap_err()
        .contains("no gate result"));

    // (b) a run whose gate failed
    let red = run_with(&state, &challenger.id, seats, "{}");
    let red = record_workflow_run_gate(&state, &red.id, 1, Some(3)).expect("gate");
    assert!(promote_workflow(&state, &red.id, None, true)
        .unwrap_err()
        .contains("failed its gate"));

    // (c) a green run with no preference, against a sitting incumbent
    let ok = green(&state, &challenger.id, seats);
    assert!(promote_workflow(&state, &ok.id, None, true)
        .unwrap_err()
        .contains("already the champion"));

    // (d) a non-comparable preference
    let other = green(&state, &challenger.id, r#"{"glm":9}"#);
    let bad = record_workflow_pref(&state, &ok.id, &other.id, None).expect("pref");
    assert!(!bad.comparable, "different seat budgets are not comparable");
    assert!(promote_workflow(&state, &ok.id, Some(&bad.id), true)
        .unwrap_err()
        .contains("not comparable"));

    // (e) a preference naming someone else as the winner
    let good = record_workflow_pref(&state, &other.id, &ok.id, None).expect("pref");
    assert!(promote_workflow(&state, &ok.id, Some(&good.id), true)
        .unwrap_err()
        .contains("as the winner, not run"));

    // (f) unconfirmed
    assert!(promote_workflow(&state, &ok.id, None, false)
        .unwrap_err()
        .contains("human-confirmed"));

    assert_eq!(
        champion_of(&state, "repo", "dev-phase", "glm").as_deref(),
        Some(incumbent.id.as_str()),
        "six refused promotions must leave the incumbent in place"
    );
    assert_eq!(
        decision_log(&state),
        log_before,
        "a refused promotion must append nothing to the decision log"
    );
    // The connection is not wedged by the six rolled-back transactions.
    assert_eq!(
        list_workflows(&state, Some("repo"), None, None)
            .expect("reads still work after six rolled-back transactions")
            .len(),
        2
    );
}

/// Promotion is scoped to the run's OWN `(repo, phase_type, family)` — it cannot reach across a
/// boundary, because `promote_workflow` derives the key from the run's workflow rather than taking
/// it as a parameter. Four families each keep their own champion.
#[test]
fn qa_a_promotion_cannot_cross_a_champion_key_boundary() {
    let (state, _root) = test_state();
    let keys = [
        ("repo", "dev-phase", "glm"),
        ("repo", "dev-phase", "qwen"),
        ("repo", "ui-phase", "glm"),
        ("other", "dev-phase", "glm"),
    ];
    let mut ids = Vec::new();
    for (repo, phase, family) in keys {
        let w = workflow(&state, repo, phase, family);
        let run = green(&state, &w.id, "{}");
        promote_workflow(&state, &run.id, None, true).expect("seed champion");
        ids.push(w.id);
    }
    for (index, (repo, phase, family)) in keys.iter().enumerate() {
        assert_eq!(
            champion_of(&state, repo, phase, family).as_deref(),
            Some(ids[index].as_str()),
            "({repo}, {phase}, {family}) kept its own champion"
        );
    }
    let all_champions: i64 = state
        .db
        .with_conn(|conn| {
            conn.query_row(
                "SELECT COUNT(*) FROM workflows WHERE is_champion = 1",
                [],
                |row| row.get(0),
            )
            .map_err(|e| e.to_string())
        })
        .expect("count");
    assert_eq!(all_champions, 4, "four keys, four champions, no bleed");
}

// =================================================================================================
// CLAIM 1 — the fourth table, and whether the log can disagree with `is_champion`
// =================================================================================================

/// **FIXED (round 2). Inverted from the defect pin of the same shape.** The decision log records
/// only promotions that actually promoted something, so it cannot disagree with
/// `workflows.is_champion` about how many there were.
///
/// What was wrong: re-promoting the sitting champion changed no row — `is_champion` was already 1
/// and the demote matched the same row the promote then set — but it appended a `promote` decision.
/// QA measured five rows for one promotion. That is the drift a second representation of one fact
/// invites, and while a promotion still cleared a halt, each phantom row was a free escape from
/// one. `promote_workflow` now returns early, writing nothing, when the run's workflow already
/// holds the slot.
#[test]
fn qa_a_the_decision_log_records_only_promotions_that_promoted() {
    let (state, _root) = test_state();
    let w = workflow(&state, "repo", "dev-phase", "glm");
    let run = green(&state, &w.id, "{}");
    promote_workflow(&state, &run.id, None, true).expect("real promotion");
    let champion_after_first = champion_of(&state, "repo", "dev-phase", "glm");

    for _ in 0..4 {
        promote_workflow(&state, &run.id, None, true).expect("re-promotion is accepted");
    }

    assert_eq!(
        champion_of(&state, "repo", "dev-phase", "glm"),
        champion_after_first,
        "four re-promotions changed no row"
    );
    let promotes = decision_log(&state)
        .into_iter()
        .filter(|(kind, _)| kind == "promote")
        .count();
    assert_eq!(
        promotes, 1,
        "five calls, one actual promotion, ONE row: the log and is_champion must agree about how \
         many promotions there were"
    );
}

/// The champion keeps `is_champion = 1` while its own runs are rejected to a halt. The log's
/// trailing decisions say the search gave up; `is_champion` still says this workflow is what J1
/// executes. Both are arguably intended, but they are the two facts a reader will read as one, so
/// the combination is pinned here explicitly.
#[test]
fn qa_a_a_halted_family_still_has_a_champion() {
    let (state, _root) = test_state();
    let w = workflow(&state, "repo", "dev-phase", "glm");
    let run = green(&state, &w.id, "{}");
    promote_workflow(&state, &run.id, None, true).expect("promote");
    for strike in 1..=3 {
        let r = run_with(&state, &w.id, "{}", "{}");
        reject_workflow_run(&state, &r.id, &format!("s{strike}")).expect("reject");
    }
    let s = workflow_search_state(&state, "repo", "dev-phase", "glm").expect("state");
    assert!(s.halted);
    assert_eq!(
        s.champion_workflow_id.as_deref(),
        Some(w.id.as_str()),
        "a halted search still reports a champion, and J1 still executes it"
    );
    assert!(get_workflow(&state, &w.id).unwrap().is_champion);
}

/// The migration's own guard: `kind` is constrained and a `reject` with a blank reason is refused
/// by the TABLE, not only by `reject_workflow_run`. Driven with raw SQL so a future second writer
/// cannot make the log useless.
#[test]
fn qa_a_the_decision_table_refuses_a_reasonless_rejection_and_a_bogus_kind() {
    let (state, _root) = test_state();
    let w = workflow(&state, "repo", "dev-phase", "glm");
    let run = run_with(&state, &w.id, "{}", "{}");

    let insert = |id: &str, kind: &str, reason: Option<&str>| -> Result<usize, String> {
        state.db.with_conn(|conn| {
            conn.execute(
                "INSERT INTO workflow_decisions (id, run_id, kind, reason, created_at) \
                 VALUES (?1, ?2, ?3, ?4, '2026-01-01T00:00:00Z')",
                params![id, run.id, kind, reason],
            )
            .map_err(|e| e.to_string())
        })
    };

    assert!(insert("d1", "reject", None).is_err(), "NULL reason");
    assert!(insert("d2", "reject", Some("   ")).is_err(), "blank reason");
    assert!(insert("d3", "abstain", Some("x")).is_err(), "bogus kind");
    assert!(insert("d4", "reject", Some("real")).is_ok());
    // …and the id is unique, so a replayed write cannot double-count a strike.
    assert!(insert("d4", "reject", Some("replay")).is_err(), "id reuse");

    // The FK is live: a decision for a run that does not exist is refused.
    assert!(state
        .db
        .with_conn(|conn| conn
            .execute(
                "INSERT INTO workflow_decisions (id, run_id, kind, reason, created_at) \
                 VALUES ('d5', 'no-such-run', 'promote', NULL, '2026-01-01T00:00:00Z')",
                [],
            )
            .map_err(|e| e.to_string()))
        .is_err());
}

// =================================================================================================
// CLAIM 5 — `parse_seat_map`
// =================================================================================================

#[test]
fn qa_a_parse_seat_map_rejects_everything_that_is_not_a_seat_budget() {
    for bad in [
        "",
        "   ",
        "null",
        "[]",
        r#"["glm",3]"#,
        "3",
        "\"{}\"",
        "true",
        "{",
        r#"{"glm":}"#,
        r#"{"glm":"3"}"#,      // a stringly count
        r#"{"glm":3.0}"#,      // a float, even a whole one
        r#"{"glm":2.5}"#,
        r#"{"glm":-1}"#,       // negative
        r#"{"glm":{"a":1}}"#,  // nested
        r#"{"glm":null}"#,
        r#"{"glm":1e20}"#,     // beyond i64, and a float literal besides
        r#"{"glm":9223372036854775808}"#, // i64::MAX + 1
    ] {
        assert!(
            parse_seat_map(bad).is_err(),
            "parse_seat_map must refuse {bad:?} rather than read it as an empty budget"
        );
    }
}

#[test]
fn qa_a_parse_seat_map_normalises_what_it_accepts() {
    // Zero-seat entries are dropped, so these four are all the same budget.
    for raw in [
        "{}",
        r#"{"qwen":0}"#,
        r#"{"qwen":0,"deepseek":0}"#,
        "  {}  ",
    ] {
        assert!(
            parse_seat_map(raw).expect(raw).is_empty(),
            "{raw} must normalise to the empty budget"
        );
    }
    // Key ORDER does not matter: a BTreeMap normalises it.
    assert!(budgets_are_comparable(
        r#"{"glm":3,"qwen":1}"#,
        r#"{"qwen":1,"glm":3}"#
    ));
    // DUPLICATE keys: serde_json takes the LAST one. Pinned because it is silent either way, and
    // `{"glm":1,"glm":3}` vs `{"glm":3}` comparing equal is a decision, not an accident.
    assert_eq!(
        parse_seat_map(r#"{"glm":1,"glm":3}"#).expect("dup keys"),
        parse_seat_map(r#"{"glm":3}"#).expect("single"),
        "duplicate keys resolve last-wins"
    );
    // i64::MAX is a legal seat count as far as this parser is concerned.
    assert_eq!(
        parse_seat_map(r#"{"glm":9223372036854775807}"#)
            .expect("i64::MAX")
            .get("glm"),
        Some(&i64::MAX),
        "there is no upper bound on a seat count"
    );
    // Model names are case-sensitive, so a casing difference silently makes two runs
    // non-comparable rather than erroring.
    assert!(
        !budgets_are_comparable(r#"{"glm":3}"#, r#"{"GLM":3}"#),
        "casing difference => not comparable (silently)"
    );
    // An unreadable budget on either side is never comparable.
    assert!(!budgets_are_comparable("{", r#"{"glm":3}"#));
    assert!(!budgets_are_comparable(r#"{"glm":3}"#, "{"));
    assert!(!budgets_are_comparable("{", "{"));
    // Two EMPTY budgets are comparable, which is how the "no seats recorded" case behaves.
    assert!(budgets_are_comparable("{}", r#"{"qwen":0}"#));
}

/// The claimed bypass — "record a second, comparable preference for the same pair" — is CLOSED.
/// `comparable` is derived from the two runs' stored `seats_held`, and no code path updates
/// `seats_held` after the insert, so every preference over one pair gets the same answer however
/// many times it is recorded.
#[test]
fn qa_a_a_second_preference_for_the_same_pair_cannot_become_comparable() {
    let (state, _root) = test_state();
    let w = workflow(&state, "repo", "dev-phase", "glm");
    let a = green(&state, &w.id, r#"{"glm":1}"#);
    let b = green(&state, &w.id, r#"{"glm":2}"#);

    for attempt in 0..5 {
        let pref = record_workflow_pref(&state, &a.id, &b.id, Some(format!("try {attempt}")))
            .expect("pref");
        assert!(
            !pref.comparable,
            "attempt {attempt}: a mismatched pair must stay non-comparable"
        );
        assert!(promote_workflow(&state, &a.id, Some(&pref.id), true)
            .unwrap_err()
            .contains("not comparable"));
    }
    assert_eq!(champion_of(&state, "repo", "dev-phase", "glm"), None);
    // And `seats_held` is immutable through the public API — there is no setter to flip the answer.
    assert_eq!(
        get_workflow(&state, &w.id).unwrap().is_champion,
        false,
        "no promotion happened"
    );
}

// =================================================================================================
// CLAIM 4 — `gate_collected` nullability, both directions
// =================================================================================================

/// `None` must never arrive as `0`, and a real `0` must never arrive as `null`. `0` collected is
/// the design's "this gate discriminates nothing" signal and is a different, stronger claim than
/// "nothing was parseable".
#[test]
fn qa_a_gate_collected_round_trips_none_and_zero_distinctly() {
    let (state, _root) = test_state();
    let w = workflow(&state, "repo", "dev-phase", "glm");

    // Round 4: `i64::MAX` is refused now — above MAX_GATE_COLLECTED a count would be silently
    // ROUNDED by JSON.parse before GraphQL saw it. The boundary value itself still round-trips,
    // which keeps this test about NULLABILITY rather than about the bound.
    for expected in [None, Some(0), Some(1), Some(MAX_GATE_COLLECTED)] {
        let run = run_with(&state, &w.id, "{}", "{}");
        let scored = record_workflow_run_gate(&state, &run.id, 0, expected).expect("gate");
        assert_eq!(scored.gate_collected, expected, "the write's return value");
        // Re-read from SQLite rather than trusting the returned struct.
        let reread = list_workflow_runs(&state, &w.id)
            .expect("list")
            .into_iter()
            .find(|candidate| candidate.id == run.id)
            .expect("the run");
        assert_eq!(reread.gate_collected, expected, "after a SQLite round trip");
        // And through serde, which is what the RPC layer serialises.
        let json = serde_json::to_value(&reread).expect("serialise");
        match expected {
            None => assert!(
                json["gateCollected"].is_null(),
                "None must serialise as null, not 0: {json}"
            ),
            Some(value) => assert_eq!(
                json["gateCollected"].as_i64(),
                Some(value),
                "a real {value} must survive: {json}"
            ),
        }
    }
}

/// **FIXED (round 2). Inverted from the defect pin of the same shape.** A second gate result has to
/// be asked for by NAME.
///
/// What was wrong: `record_workflow_run_gate` was an unconditional UPDATE with no "already scored"
/// guard, so a second call silently replaced a green score with a red one and erased the advisory
/// count, with only `ended_at` recording that it had happened twice. There are now two entry
/// points over one core: `record_workflow_run_gate` refuses a run that already has a score, and
/// `rescore_workflow_run_gate` is the deliberate one. The separate name IS the guard — there is no
/// flag to forget and no default to get wrong.
#[test]
fn qa_a_a_second_gate_record_is_refused_unless_asked_for_by_name() {
    let (state, _root) = test_state();
    let w = workflow(&state, "repo", "dev-phase", "glm");
    let run = run_with(&state, &w.id, "{}", "{}");

    let first = record_workflow_run_gate(&state, &run.id, 0, Some(400)).expect("gate");
    assert_eq!((first.gate_exit, first.gate_collected), (Some(0), Some(400)));

    let refused = record_workflow_run_gate(&state, &run.id, 1, None)
        .expect_err("a silent overwrite must be refused");
    assert!(
        refused.contains("already has a gate result (exit 0)"),
        "got: {refused}"
    );
    // The refusal leaves the original score completely intact, advisory count included.
    let kept = get_workflow_run(&state, &run.id).expect("read back");
    assert_eq!((kept.gate_exit, kept.gate_collected), (Some(0), Some(400)));
    assert_eq!(kept.ended_at, first.ended_at, "and does not even touch ended_at");

    // The deliberate form goes through, and is the only thing that does.
    let second = rescore_workflow_run_gate(&state, &run.id, 1, None).expect("deliberate rescore");
    assert_eq!((second.gate_exit, second.gate_collected), (Some(1), None));
    assert!(
        second.ended_at.is_some() && second.ended_at != first.ended_at,
        "ended_at still records that it happened twice"
    );

    // And a promoted champion's own evidence cannot be rewritten at all — not even deliberately.
    let promoted_w = workflow(&state, "repo", "ui-phase", "glm");
    let promoted_run = green(&state, &promoted_w.id, "{}");
    promote_workflow(&state, &promoted_run.id, None, true).expect("seed champion");
    let locked = rescore_workflow_run_gate(&state, &promoted_run.id, 1, None)
        .expect_err("a champion's evidence must not be rewritable underneath it");
    assert!(locked.contains("cannot be rewritten underneath it"), "got: {locked}");
    assert_eq!(
        get_workflow_run(&state, &promoted_run.id).unwrap().gate_exit,
        Some(0)
    );
}

// =================================================================================================
// CLAIM 7 — Int32. This project has already shipped an outage from an i64 served as GraphQL `Int`.
// =================================================================================================

/// **FIXED (round 2). Inverted from the defect pin of the same shape.** The host refuses the
/// integers it can genuinely bound, and the wire fields are `Float` for the one it cannot.
///
/// What was wrong: every arena integer was an unbounded `i64` on the host while the schema typed
/// them as 32-bit `Int`, and one over-i32 `seed` inside `workflowRuns: [WorkflowRun!]!` nulled the
/// response ROOT — worse than the `listAiSessions` outage, because unrelated fields in the same
/// document went with it.
///
/// Two halves to the fix, and this test covers the host half:
///
/// * `fan_out` is bounded by `MAX_FAN_OUT` (it is a seat cost), `seed` by `MAX_SEED` (it is a
///   replay index), and `gate_exit` to 32 bits (it is a process exit code).
/// * `gate_collected` is deliberately NOT bounded. It is advisory, and refusing a real gate result
///   because an advisory count is large would be the wrong trade — so it stays unbounded here and
///   the schema carries it as `Float`.
///
/// The GraphQL half is `worker/lib/ai-scope-tests/workflow-arena-int32-qa.test.ts`.
#[test]
fn qa_a_the_host_refuses_the_integers_it_can_bound() {
    let (state, _root) = test_state();
    let over: i64 = i32::MAX as i64 + 1; // 2147483648
    let way_over: i64 = 9_007_199_254_740_991; // Number.MAX_SAFE_INTEGER

    let fan_out_error = create_workflow_version(
        &state,
        NewWorkflowVersion {
            fan_out: way_over,
            ..version("repo", "dev-phase", "glm")
        },
    )
    .expect_err("fan_out must have an upper bound");
    assert!(
        fan_out_error.contains("exceeds the stand-in cap"),
        "got: {fan_out_error}"
    );
    // And the bound is the documented one, not an accident.
    assert!(create_workflow_version(
        &state,
        NewWorkflowVersion {
            fan_out: MAX_FAN_OUT + 1,
            ..version("repo", "dev-phase", "glm")
        },
    )
    .is_err());
    let w = create_workflow_version(
        &state,
        NewWorkflowVersion {
            fan_out: MAX_FAN_OUT,
            ..version("repo", "dev-phase", "glm")
        },
    )
    .expect("the bound itself is allowed");

    let seed_error = start_workflow_run(
        &state,
        NewWorkflowRun {
            workflow_id: w.id.clone(),
            plan_id: None,
            seed: way_over,
            seats: None,
        },
    )
    .expect_err("seed must have an upper bound");
    assert!(seed_error.contains("at most"), "got: {seed_error}");

    let run = start_workflow_run(
        &state,
        NewWorkflowRun {
            workflow_id: w.id.clone(),
            plan_id: None,
            seed: MAX_SEED,
            seats: None,
        },
    )
    .expect("the bound itself is allowed");

    // A gate exit outside 32 bits is not a process exit code.
    let exit_error = record_workflow_run_gate(&state, &run.id, over, None)
        .expect_err("gate_exit must be bounded to a process exit code");
    assert!(
        exit_error.contains("outside the range of a process exit code"),
        "got: {exit_error}"
    );

    // `gate_collected` stays UNBOUNDED on purpose: it is advisory, and the wire field is Float.
    let scored = record_workflow_run_gate(&state, &run.id, 0, Some(way_over))
        .expect("an advisory count must never cost a real score");
    assert_eq!(scored.gate_collected, Some(way_over));

    // Everything the host DOES serve now fits i32, except the deliberately-unbounded advisory one.
    let reread = list_workflow_runs(&state, &w.id).expect("list").remove(0);
    let run_json = serde_json::to_value(&reread).expect("serialise run");
    let workflow_json = serde_json::to_value(get_workflow(&state, &w.id).unwrap()).expect("wf");
    for (field, value) in [
        ("seed", run_json["seed"].as_i64()),
        ("gateExit", run_json["gateExit"].as_i64()),
        ("fanOut", workflow_json["fanOut"].as_i64()),
        ("version", workflow_json["version"].as_i64()),
    ] {
        let value = value.expect("a value");
        assert!(
            value <= i32::MAX as i64,
            "{field} = {value} must fit i32 now that the host bounds it"
        );
    }
    assert_eq!(
        run_json["gateCollected"].as_i64(),
        Some(way_over),
        "and the advisory count is still served whole, on a Float field"
    );
}

// =================================================================================================
// CLAIM 8 — `unchecked_transaction`
// =================================================================================================

/// `unchecked_transaction` rolls back on an early return, and does not wedge the single shared
/// connection.
///
/// `Database::with_conn` holds one `std::sync::Mutex` around one `Connection` for the whole
/// closure, so every arena call is fully serialised — there is no concurrent-promotion window to
/// race, and no second connection to deadlock against. The real risk the pattern carries here is a
/// NESTED `with_conn` inside a closure, which would self-deadlock on a non-reentrant mutex; no
/// arena function does that (they all pass `&tx` down to connection-level helpers), and this test
/// drives the deepest nesting that exists — `start_workflow_run`, which consults
/// `search_state_on` inside its own transaction — to prove it completes.
#[test]
fn qa_a_a_failed_transaction_rolls_back_and_releases_the_connection() {
    let (state, _root) = test_state();
    let w = workflow(&state, "repo", "dev-phase", "glm");

    let rows = |table: &str| -> i64 {
        state
            .db
            .with_conn(|conn| {
                conn.query_row(&format!("SELECT COUNT(*) FROM {table}"), [], |row| row.get(0))
                    .map_err(|e| e.to_string())
            })
            .expect("count")
    };
    let before = (rows("workflows"), rows("workflow_runs"), rows("workflow_prefs"));

    // Each of these aborts inside a transaction, after it has opened one.
    assert!(create_workflow_version(
        &state,
        NewWorkflowVersion {
            parent_id: Some("no-such-parent".to_string()),
            ..version("repo", "dev-phase", "glm")
        },
    )
    .is_err());
    assert!(create_workflow_version(
        &state,
        NewWorkflowVersion {
            parent_id: Some(workflow(&state, "repo", "dev-phase", "qwen").id),
            ..version("repo", "dev-phase", "glm")
        },
    )
    .is_err_and(|e| e.contains("lineage does not cross families")));
    assert!(start_workflow_run(
        &state,
        NewWorkflowRun {
            workflow_id: "no-such-workflow".to_string(),
            plan_id: None,
            seed: 0,
            seats: None,
        },
    )
    .is_err());
    assert!(record_workflow_run_gate(&state, "no-such-run", 0, None).is_err());
    assert!(record_workflow_pref(&state, "no-such-run", "also-not-a-run", None).is_err());
    assert!(reject_workflow_run(&state, "no-such-run", "why").is_err());
    assert!(promote_workflow(&state, "no-such-run", None, true).is_err());

    // One extra `workflows` row: the qwen parent above was created on purpose.
    assert_eq!(
        (rows("workflows"), rows("workflow_runs"), rows("workflow_prefs")),
        (before.0 + 1, before.1, before.2),
        "every aborted transaction rolled back"
    );
    assert_eq!(rows("workflow_decisions"), 0);

    // The connection is not held, poisoned or left inside a transaction: the deepest nesting in
    // the module still completes, twice.
    let run = run_with(&state, &w.id, r#"{"glm":1}"#, r#"{"glm":2}"#);
    let run2 = run_with(&state, &w.id, r#"{"glm":1}"#, r#"{"glm":2}"#);
    assert_ne!(run.id, run2.id);
    assert_eq!(rows("workflow_runs"), 2);
    // An explicit BEGIN now proves no transaction was left open by the seven aborts.
    state
        .db
        .with_conn(|conn| {
            conn.execute_batch("BEGIN; ROLLBACK;")
                .map_err(|e| e.to_string())
        })
        .expect("no transaction was left open");
}

/// `next_version_in_family` is read-then-written inside one transaction, and the single shared
/// mutexed connection serialises every caller, so two creates can never claim one version number
/// even though the schema has no unique index on `(repo, phase_type, family, version)`.
#[test]
fn qa_a_versions_are_dense_and_unique_within_a_family_under_interleaving() {
    let (state, _root) = test_state();
    let mut glm = Vec::new();
    let mut qwen = Vec::new();
    for _ in 0..10 {
        glm.push(workflow(&state, "repo", "dev-phase", "glm").version);
        qwen.push(workflow(&state, "repo", "dev-phase", "qwen").version);
    }
    assert_eq!(glm, (1..=10).collect::<Vec<i64>>());
    assert_eq!(
        qwen, glm,
        "version is per-family, so an interleaved family does not consume numbers"
    );
}

/// **DEFECT (documented, not fixed).** The sharpest halt escape, and the one that needs no champion
/// at all: reject the SAME green run three times, then promote it.
///
/// This is not a contrived sequence — it is the design's own step 8. A rejection carries a reason
/// precisely because a human says no to a run whose gate was GREEN (too slow, wrong approach,
/// unreadable diff). Such a run is therefore both rejected and promotable. Three rejections halt
/// the family, `start_workflow_run` refuses, and then `promote_workflow` on that same run is
/// accepted — the family has no champion, so `promotion_block`'s "seeding the first champion"
/// branch applies — the streak resets, and the rejected run becomes the champion J1 executes.
///
/// `promote_workflow` never consults `workflow_search_state`. Only `start_workflow_run` does.
#[test]
fn qa_a_a_thrice_rejected_green_run_cannot_be_promoted_while_halted() {
    let (state, _root) = test_state();
    let w = workflow(&state, "repo", "dev-phase", "glm");
    assert_eq!(REJECTION_STRIKE_LIMIT, 3, "this test is written for three strikes");

    // THREE DISTINCT green candidates, each refused by a human. A rejection carries a reason
    // precisely because a human says no to a GREEN run, so every one of these is promotable on its
    // gate alone — which is what made the hole reachable.
    let mut last = None;
    for strike in 1..=3 {
        let run = green(&state, &w.id, r#"{"glm":1}"#);
        assert_eq!(run.gate_exit, Some(0), "the run's gate is green");
        reject_workflow_run(&state, &run.id, &format!("rejected despite a green gate #{strike}"))
            .expect("reject");
        last = Some(run);
    }
    let run = last.expect("a run");
    let halted = workflow_search_state(&state, "repo", "dev-phase", "glm").expect("state");
    assert!(halted.halted && halted.champion_workflow_id.is_none());

    let error = promote_workflow(&state, &run.id, None, true)
        .expect_err("a halted search must refuse to promote the run it just refused");
    assert!(error.contains("is halted"), "got: {error}");
    let after = workflow_search_state(&state, "repo", "dev-phase", "glm").expect("state");
    assert!(
        after.halted && after.consecutive_rejections == 3,
        "the refused promotion changed nothing about the halt"
    );
    assert_eq!(
        after.champion_workflow_id, None,
        "and the thrice-rejected workflow is NOT what J1 executes"
    );
    assert!(
        decision_log(&state).iter().all(|(kind, _)| kind == "reject"),
        "a refused promotion appends nothing: {:?}",
        decision_log(&state)
    );
    // The search is still blocked.
    assert!(start_workflow_run(
        &state,
        NewWorkflowRun {
            workflow_id: w.id.clone(),
            plan_id: None,
            seed: 0,
            seats: None,
        },
    )
    .is_err());

    // A human can still overrule the search — but only by SAYING SO, with a reason, as its own
    // auditable decision. That is the whole point of `resume`.
    let resumed = resume_workflow_search(
        &state,
        "repo",
        "dev-phase",
        "glm",
        "all three were refused for a reason that no longer applies",
        true,
    )
    .expect("resume");
    assert!(!resumed.halted);
    promote_workflow(&state, &run.id, None, true).expect("now it may be promoted");
    assert_eq!(
        champion_of(&state, "repo", "dev-phase", "glm").as_deref(),
        Some(w.id.as_str())
    );
    run_with(&state, &w.id, r#"{"glm":1}"#, "{}");
}
