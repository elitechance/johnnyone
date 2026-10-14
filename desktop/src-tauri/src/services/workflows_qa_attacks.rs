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

fn version(repo: &str, phase_type: &str, family: &str) -> NewWorkflowVersion {
    NewWorkflowVersion {
        repo: repo.to_string(),
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

fn run_with(state: &AppState, workflow_id: &str, seats: &str, fleet: &str) -> WorkflowRun {
    start_workflow_run(
        state,
        NewWorkflowRun {
            workflow_id: workflow_id.to_string(),
            plan_id: None,
            seed: 0,
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
                params![repo, phase_type, family],
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

/// **DEFECT (documented, not fixed).** A halt is escaped by re-promoting the workflow that is
/// ALREADY champion, with no preference, no new run and no new evidence of any kind.
///
/// `promote_workflow` filters the incumbent out when the incumbent IS the run's own workflow
/// (`incumbent.as_deref().filter(|id| *id != workflow.id)`), so `promotion_block` sees
/// `incumbent = None` and `pref = None` and returns `None` — the "seeding the first champion"
/// branch. The row change is a no-op, but a `promote` decision is appended, and
/// `workflow_search_state` counts rejections only back to the last `promote`. One phantom
/// promotion therefore resets a three-strike halt.
///
/// The halt is meant to be the search admitting defeat. As implemented it is a speed bump that any
/// caller holding one already-green champion run can step over in a single call.
#[test]
fn qa_a_halt_is_escaped_by_repromoting_the_sitting_champion() {
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
                seats_held: r#"{"glm":1}"#.to_string(),
                fleet_busy: "{}".to_string(),
            },
        )
        .is_err(),
        "a halted family must refuse a new run"
    );

    // THE HOLE: re-promote the sitting champion using the SAME run that seeded it. No preference,
    // no new gate, nothing proven — and the halt is gone.
    let repromoted = promote_workflow(&state, &champion_run.id, None, true);
    assert!(
        repromoted.is_ok(),
        "DEFECT PIN: re-promoting the sitting champion is currently accepted. If this line starts \
         failing, promote_workflow has been tightened and this test should be inverted. error: {:?}",
        repromoted.err()
    );
    let after = workflow_search_state(&state, "repo", "dev-phase", "glm").expect("state");
    assert_eq!(
        after.consecutive_rejections, 0,
        "DEFECT PIN: a no-op re-promotion resets the rejection streak"
    );
    assert!(
        !after.halted,
        "DEFECT PIN: the three-strike halt is escapable with no new evidence"
    );
    // And the search really is runnable again, which is what makes this a defect rather than a
    // cosmetic counter reset.
    run_with(&state, &champion.id, r#"{"glm":1}"#, "{}");
}

/// **DEFECT (documented, not fixed).** The same run can be rejected three times and that counts as
/// three strikes. Nothing in `reject_workflow_run` or `workflow_search_state` deduplicates by
/// `run_id`, so "three consecutive rejections" is really "three rejection ROWS", and a caller that
/// retries a rejection — or a UI with a double-submitting button — halts the search on one verdict.
#[test]
fn qa_a_rejecting_one_run_three_times_halts_the_search() {
    let (state, _root) = test_state();
    let w = workflow(&state, "repo", "dev-phase", "glm");
    let run = run_with(&state, &w.id, "{}", "{}");

    for strike in 1..=3 {
        let state_after =
            reject_workflow_run(&state, &run.id, &format!("same run, strike {strike}"))
                .expect("reject");
        assert_eq!(state_after.consecutive_rejections, strike);
    }
    let after = workflow_search_state(&state, "repo", "dev-phase", "glm").expect("state");
    assert!(
        after.halted,
        "DEFECT PIN: three rejections of ONE run halt the search — the rule counts rows, not \
         candidates"
    );
    let log = decision_log(&state);
    assert_eq!(log.len(), 3);
    assert!(
        log.iter().all(|(kind, id)| kind == "reject" && id == &run.id),
        "all three strikes are the same run: {log:?}"
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
/// and checking that a promotion recorded LAST still resets the streak.
#[test]
fn qa_a_identical_created_at_does_not_break_the_consecutive_rule() {
    let (state, _root) = test_state();
    let w = workflow(&state, "repo", "dev-phase", "glm");
    let run = green(&state, &w.id, "{}");

    reject_workflow_run(&state, &run.id, "a").expect("reject");
    reject_workflow_run(&state, &run.id, "b").expect("reject");
    promote_workflow(&state, &run.id, None, true).expect("promote");
    reject_workflow_run(&state, &run.id, "c").expect("reject");

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
        "with all timestamps identical, only the rejection AFTER the promotion may count"
    );
    assert_eq!(s.recent_rejections, vec!["c".to_string()]);
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
            seats_held: "{}".to_string(),
            fleet_busy: "{}".to_string(),
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

/// **DEFECT (documented, not fixed).** The incumbent champion is displaced by a preference whose
/// loser is a run of an entirely DIFFERENT `(repo, phase_type, family)`.
///
/// `promotion_block` checks three things about the preference — it exists, it names this run as
/// winner, and it is `comparable`. It never checks what the LOSER was. `record_workflow_pref`
/// does not check it either: it reads both runs and compares their seat maps, nothing more. So the
/// documented rule "displacing an incumbent needs a comparable head-to-head" is satisfied by a
/// head-to-head against a run that was never the incumbent, in a family the design says is a
/// different IDENTITY.
///
/// This matters because `family` is the design's central claim: a hop tuned for one context window
/// does not transfer. A challenger that beat a Qwen run must not thereby become the GLM champion.
#[test]
fn qa_a_a_pref_against_another_family_displaces_the_champion() {
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

    let pref = record_workflow_pref(
        &state,
        &challenger_run.id,
        &unrelated_run.id,
        Some("beat a qwen run".to_string()),
    )
    .expect("DEFECT PIN: a cross-family preference is currently recordable");
    assert!(
        pref.comparable,
        "DEFECT PIN: comparability is seat-map equality only, so a cross-family pair is comparable"
    );

    let promoted = promote_workflow(&state, &challenger_run.id, Some(&pref.id), true);
    assert!(
        promoted.is_ok(),
        "DEFECT PIN: the GLM champion is displaced on the strength of beating a QWEN run. If this \
         starts failing, promotion_block has gained a same-family check on the loser and this test \
         should be inverted. error: {:?}",
        promoted.err()
    );
    assert_eq!(
        champion_of(&state, "repo", "dev-phase", "glm").as_deref(),
        Some(challenger.id.as_str()),
        "DEFECT PIN: the displacement really happened"
    );
}

/// **DEFECT (documented, not fixed).** A workflow displaces the champion by beating ITSELF: two
/// runs of the same workflow, on the same seat budget, make a `comparable` preference, and
/// `promotion_block` accepts it.
///
/// This is the sharpest form of the hole above. "A comparable head-to-head" is supposed to mean
/// the challenger out-performed the incumbent. Here the incumbent is not involved at all.
#[test]
fn qa_a_a_workflow_displaces_the_champion_by_beating_itself() {
    let (state, _root) = test_state();
    let seats = r#"{"glm":2}"#;
    let incumbent = workflow(&state, "repo", "dev-phase", "glm");
    let incumbent_run = green(&state, &incumbent.id, seats);
    promote_workflow(&state, &incumbent_run.id, None, true).expect("seed champion");

    let challenger = workflow(&state, "repo", "dev-phase", "glm");
    let win = green(&state, &challenger.id, seats);
    let lose = green(&state, &challenger.id, seats);
    let pref = record_workflow_pref(&state, &win.id, &lose.id, None).expect("self head-to-head");
    assert!(pref.comparable);

    assert!(
        promote_workflow(&state, &win.id, Some(&pref.id), true).is_ok(),
        "DEFECT PIN: a workflow beating its own other run is accepted as the head-to-head that \
         displaces the incumbent"
    );
    assert_eq!(
        champion_of(&state, "repo", "dev-phase", "glm").as_deref(),
        Some(challenger.id.as_str())
    );
    // The old champion is correctly demoted — the atomic swap itself works.
    assert!(!get_workflow(&state, &incumbent.id).unwrap().is_champion);
}

/// **DEFECT (documented, not fixed).** `comparable` ignores `fleet_busy` entirely.
///
/// The design's stated reason for `fleet_busy` is: "Two concurrent runs contend, there is no 429 to
/// notice, and the slower run looks like the worse workflow. Without these, the first thing a busy
/// fleet does is promote a champion because the other box was quiet." `budgets_are_comparable`
/// compares `seats_held` ONLY, so two runs that held the same seats — one on an idle fleet, one on
/// a saturated one — are `comparable: true` and the preference between them promotes.
///
/// Identical seat budgets do not imply identical contention: `seats_held` is what a run was
/// GRANTED, `fleet_busy` is what everything else was holding at the same moment. The column is
/// stored and served, so a human can see it; the rule the design says it enables is not
/// implemented.
#[test]
fn qa_a_comparable_ignores_fleet_contention() {
    let (state, _root) = test_state();
    let seats = r#"{"glm":2}"#;
    let w = workflow(&state, "repo", "dev-phase", "glm");

    let quiet = run_with(&state, &w.id, seats, r#"{"glm":0}"#);
    let quiet = record_workflow_run_gate(&state, &quiet.id, 0, None).expect("gate");
    let saturated = run_with(&state, &w.id, seats, r#"{"glm":4096,"qwen":512}"#);
    let saturated = record_workflow_run_gate(&state, &saturated.id, 1, None).expect("gate");

    assert!(
        budgets_are_comparable(&quiet.seats_held, &saturated.seats_held),
        "DEFECT PIN: the comparability rule reads seats_held alone"
    );
    let pref = record_workflow_pref(&state, &quiet.id, &saturated.id, None).expect("pref");
    assert!(
        pref.comparable,
        "DEFECT PIN: a run on an idle fleet and a run on a saturated fleet are 'comparable', which \
         is the exact failure the design says fleet_busy exists to prevent"
    );
    assert!(
        promote_workflow(&state, &quiet.id, Some(&pref.id), true).is_ok(),
        "DEFECT PIN: and that preference promotes"
    );
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

/// **DEFECT (documented, not fixed).** The decision log and `workflows.is_champion` disagree about
/// whether anything happened.
///
/// Re-promoting the sitting champion changes no row — `is_champion` was already 1 and the demote
/// matches the same row the promote then sets — but it appends a `promote` decision. The log
/// therefore records promotions that never promoted anything, and `workflow_search_state` cannot
/// tell them apart from real ones. That is the drift the fourth table introduces: it is a second
/// representation of the same fact, and the two can diverge.
#[test]
fn qa_a_the_decision_log_records_promotions_that_promoted_nothing() {
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
        promotes, 5,
        "DEFECT PIN: five promote decisions, one actual promotion. The log and is_champion \
         disagree about how many promotions there were, and each phantom one resets a halt."
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

    for expected in [None, Some(0), Some(1), Some(i64::MAX)] {
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

/// A recorded gate result is overwritable, including back to `None`. Pinned because
/// `record_workflow_run_gate` is an unconditional UPDATE with no "already scored" guard, so a
/// second call silently replaces a green score with a red one — or erases the advisory count.
#[test]
fn qa_a_a_second_gate_record_silently_overwrites_the_first() {
    let (state, _root) = test_state();
    let w = workflow(&state, "repo", "dev-phase", "glm");
    let run = run_with(&state, &w.id, "{}", "{}");

    let first = record_workflow_run_gate(&state, &run.id, 0, Some(400)).expect("gate");
    assert_eq!((first.gate_exit, first.gate_collected), (Some(0), Some(400)));
    let second = record_workflow_run_gate(&state, &run.id, 1, None).expect("re-record");
    assert_eq!(
        (second.gate_exit, second.gate_collected),
        (Some(1), None),
        "DEFECT PIN: re-recording a gate overwrites the score and erases the collected count, \
         with no guard and no audit trail. A promoted champion's run can be rescored to red after \
         the fact."
    );
    assert!(
        second.ended_at.is_some() && second.ended_at != first.ended_at,
        "only ended_at records that it happened twice"
    );
}

// =================================================================================================
// CLAIM 7 — Int32. This project has already shipped an outage from an i64 served as GraphQL `Int`.
// =================================================================================================

/// Every arena integer the host will serve is an `i64` with no upper bound, while the GraphQL
/// schema declares `seed: Int!`, `fanOut: Int!`, `version: Int!`, `gateExit: Int` and
/// `gateCollected: Int` — all 32-bit. This test measures the HOST half: values above
/// `i32::MAX` are accepted, stored, re-read and serialised intact, so nothing on this side stops
/// them reaching the GraphQL layer.
#[test]
fn qa_a_the_host_accepts_and_serves_integers_above_i32_max() {
    let (state, _root) = test_state();
    let over: i64 = i32::MAX as i64 + 1; // 2147483648
    let way_over: i64 = 9_007_199_254_740_991; // Number.MAX_SAFE_INTEGER

    let w = create_workflow_version(
        &state,
        NewWorkflowVersion {
            fan_out: way_over,
            ..version("repo", "dev-phase", "glm")
        },
    )
    .expect("DEFECT PIN: fan_out has no upper bound — only `< 1` is refused");
    assert_eq!(w.fan_out, way_over);

    let run = start_workflow_run(
        &state,
        NewWorkflowRun {
            workflow_id: w.id.clone(),
            plan_id: None,
            seed: way_over,
            seats_held: "{}".to_string(),
            fleet_busy: "{}".to_string(),
        },
    )
    .expect("DEFECT PIN: seed has no upper bound — only negatives are refused");
    assert_eq!(run.seed, way_over);

    let scored = record_workflow_run_gate(&state, &run.id, over, Some(way_over)).expect("gate");
    assert_eq!(scored.gate_exit, Some(over));
    assert_eq!(scored.gate_collected, Some(way_over));

    // Re-read, then serialise exactly as the RPC layer does. Every one of these lands on a
    // 32-bit GraphQL `Int`.
    let reread = list_workflow_runs(&state, &w.id).expect("list").remove(0);
    let run_json = serde_json::to_value(&reread).expect("serialise run");
    let workflow_json = serde_json::to_value(get_workflow(&state, &w.id).unwrap()).expect("wf");
    for (field, value) in [
        ("seed", run_json["seed"].as_i64()),
        ("gateExit", run_json["gateExit"].as_i64()),
        ("gateCollected", run_json["gateCollected"].as_i64()),
        ("fanOut", workflow_json["fanOut"].as_i64()),
    ] {
        let value = value.unwrap_or_default();
        assert!(
            value > i32::MAX as i64,
            "{field} = {value} must exceed i32::MAX for this measurement to mean anything"
        );
    }
    assert_eq!(
        run_json["seed"].as_i64(),
        Some(way_over),
        "DEFECT PIN: the host serves seed = {way_over} on a field the schema types as Int (32-bit)"
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
            seats_held: "{}".to_string(),
            fleet_busy: "{}".to_string(),
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
fn qa_a_a_thrice_rejected_green_run_can_still_be_promoted_and_clears_the_halt() {
    let (state, _root) = test_state();
    let w = workflow(&state, "repo", "dev-phase", "glm");
    let run = green(&state, &w.id, r#"{"glm":1}"#);
    assert_eq!(run.gate_exit, Some(0), "the run's gate is green");

    for strike in 1..=3 {
        reject_workflow_run(&state, &run.id, &format!("rejected despite a green gate #{strike}"))
            .expect("reject");
    }
    let halted = workflow_search_state(&state, "repo", "dev-phase", "glm").expect("state");
    assert!(halted.halted && halted.champion_workflow_id.is_none());

    let promoted = promote_workflow(&state, &run.id, None, true);
    assert!(
        promoted.is_ok(),
        "DEFECT PIN: promote_workflow never consults the halt, so a thrice-rejected run is \
         promotable. error: {:?}",
        promoted.err()
    );
    let after = workflow_search_state(&state, "repo", "dev-phase", "glm").expect("state");
    assert!(
        !after.halted && after.consecutive_rejections == 0,
        "DEFECT PIN: the halt is cleared by promoting the very run that was rejected three times"
    );
    assert_eq!(
        after.champion_workflow_id.as_deref(),
        Some(w.id.as_str()),
        "DEFECT PIN: and the thrice-rejected workflow is now what J1 executes"
    );
    // The search is genuinely runnable again.
    run_with(&state, &w.id, r#"{"glm":1}"#, "{}");
}
