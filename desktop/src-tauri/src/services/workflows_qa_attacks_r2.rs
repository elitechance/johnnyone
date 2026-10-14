//! QA A's ROUND-2 adversarial tests: the fixes for round 1, plus the surfaces the fixes added.
//!
//! A second file rather than more of `workflows_qa_attacks.rs`, for two reasons. That file now has
//! two authors — Dev A inverted and renamed nine of its tests when the defects they pinned were
//! closed — and keeping round 2 separate means neither of us has to range-edit the other's work.
//! It also keeps the audit trail readable: everything here was written against the fixes, not
//! against the original feature.
//!
//! The same discipline as round 1. A test that documents a hole says `DEFECT PIN` in its assertion
//! message and asserts the CURRENT behaviour, so closing the hole turns it red. A test that pins
//! intended behaviour says what mutation was used to prove it is not vacuous.

#![cfg(test)]

use crate::services::workflows::*;
use crate::state::app_state::AppState;
use crate::test_support::test_state;
use rusqlite::params;

const KEY: (&str, &str, &str) = ("repo", "dev-phase", "glm");

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

fn workflow(state: &AppState, repo: &str, phase_type: &str, family: &str) -> Workflow {
    create_workflow_version(state, version_with(repo, phase_type, family, 2)).expect("create")
}

/// A run started through the REAL entry point, so `seats_held` / `fleet_busy` are whatever the host
/// measures today — which is the configuration that actually ships.
fn measured_run(state: &AppState, workflow_id: &str) -> WorkflowRun {
    start_workflow_run(
        state,
        NewWorkflowRun {
            workflow_id: workflow_id.to_string(),
            plan_id: None,
            seed: 0,
            seats: None,
        },
    )
    .expect("start run")
}

/// A run with contention the test chooses, via the host-internal entry point. This is the only way
/// to exercise the contention rule at all until the lease pool lands.
fn run_under(state: &AppState, workflow_id: &str, seats_held: &str, fleet_busy: &str) -> WorkflowRun {
    start_workflow_run_with_budget(
        state,
        NewWorkflowRun {
            workflow_id: workflow_id.to_string(),
            plan_id: None,
            seed: 0,
            seats: None,
        },
        RunBudget {
            seats_held: seats_held.to_string(),
            fleet_busy: fleet_busy.to_string(),
        },
    )
    .expect("start run with budget")
}

fn green(state: &AppState, run: &WorkflowRun) -> WorkflowRun {
    record_workflow_run_gate(state, &run.id, 0, Some(7)).expect("record gate")
}

fn green_measured(state: &AppState, workflow_id: &str) -> WorkflowRun {
    let run = measured_run(state, workflow_id);
    green(state, &run)
}

fn champion_of(state: &AppState, repo: &str, phase_type: &str, family: &str) -> Option<String> {
    list_workflows(state, Some(repo), Some(phase_type), Some(family))
        .expect("list")
        .into_iter()
        .find(|w| w.is_champion)
        .map(|w| w.id)
}

fn decisions(state: &AppState) -> Vec<(String, String)> {
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
        .expect("decisions")
}

/// The `promote` row the service would treat as the incumbent's evidence: the latest one in the
/// family. Read here with the same query shape the service uses, so the test and the rule agree
/// about what "latest" means.
fn latest_promote_run(state: &AppState, repo: &str, phase: &str, family: &str) -> Option<String> {
    state
        .db
        .with_conn(|conn| {
            Ok(conn
                .query_row(
                    "SELECT d.run_id FROM workflow_decisions d \
                     JOIN workflow_runs r ON r.id = d.run_id \
                     JOIN workflows w ON w.id = r.workflow_id \
                     WHERE w.repo = ?1 AND w.phase_type = ?2 AND w.family = ?3 \
                       AND d.kind = 'promote' ORDER BY d.seq DESC LIMIT 1",
                    params![repo_id(repo), phase, family],
                    |row| row.get::<_, String>(0),
                )
                .ok())
        })
        .expect("latest promote")
}

/// Three distinct rejected candidates — a real halt, under the DISTINCT-run counting rule.
fn halt(state: &AppState, workflow_id: &str) -> Vec<WorkflowRun> {
    let mut runs = Vec::new();
    for strike in 1..=3 {
        let run = measured_run(state, workflow_id);
        reject_workflow_run(state, &run.id, &format!("strike {strike}")).expect("reject");
        runs.push(run);
    }
    let s = workflow_search_state(state, KEY.0, KEY.1, KEY.2).expect("state");
    assert!(s.halted, "the fixture must actually halt the search");
    runs
}

// =================================================================================================
// The two round-1 escapes, re-attacked independently of Dev A's inversions
// =================================================================================================

/// FIX CONFIRMED. Re-promoting the sitting champion no longer escapes a halt, by two independent
/// mechanisms: a promotion is refused outright while a halt is in force, and a `promote` row is
/// skipped by the streak walk rather than ending it.
#[test]
fn qa_a2_repromoting_the_champion_no_longer_escapes_a_halt() {
    let (state, _root) = test_state();
    let champion = workflow(&state, KEY.0, KEY.1, KEY.2);
    let champion_run = green_measured(&state, &champion.id);
    promote_workflow(&state, &champion_run.id, None, true).expect("seed the first champion");
    let log_before = decisions(&state);

    halt(&state, &champion.id);

    let blocked = promote_workflow(&state, &champion_run.id, None, true);
    assert!(blocked.is_err(), "a promotion must be refused while halted");
    let error = blocked.unwrap_err();
    assert!(error.contains("is halted"), "got: {error}");
    assert!(
        error.contains("resume"),
        "the refusal must name the way out: {error}"
    );

    let after = workflow_search_state(&state, KEY.0, KEY.1, KEY.2).expect("state");
    assert!(after.halted, "the halt survives the attempt");
    assert_eq!(after.consecutive_rejections, 3);
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
        "and the search is still refused"
    );

    // Independently: even with the halt lifted, a re-promotion writes NOTHING, so it cannot
    // manufacture the `promote` row that used to be the escape.
    resume_workflow_search(&state, KEY.0, KEY.1, KEY.2, "trying again", true).expect("resume");
    promote_workflow(&state, &champion_run.id, None, true).expect("a no-op re-promotion");
    let promotes_now = decisions(&state)
        .into_iter()
        .filter(|(kind, _)| kind == "promote")
        .count();
    let promotes_before = log_before
        .into_iter()
        .filter(|(kind, _)| kind == "promote")
        .count();
    assert_eq!(
        promotes_now, promotes_before,
        "a promotion that promotes nothing must append nothing"
    );
}

/// FIX CONFIRMED. The design's own step 8 — a human rejecting a GREEN run — can no longer be
/// overturned with the operation the search was refusing. The run stays green and stays promotable
/// in principle; it is the HALT that refuses, and only a `resume` lifts it.
#[test]
fn qa_a2_a_thrice_rejected_green_run_cannot_promote_until_a_human_resumes() {
    let (state, _root) = test_state();
    let w = workflow(&state, KEY.0, KEY.1, KEY.2);

    // Three distinct GREEN runs, each rejected: gates passed, the human said no anyway.
    let mut greens = Vec::new();
    for strike in 1..=3 {
        let run = green_measured(&state, &w.id);
        assert_eq!(run.gate_exit, Some(0));
        reject_workflow_run(&state, &run.id, &format!("green but wrong, {strike}"))
            .expect("reject");
        greens.push(run);
    }
    let halted = workflow_search_state(&state, KEY.0, KEY.1, KEY.2).expect("state");
    assert!(halted.halted && halted.champion_workflow_id.is_none());

    for run in &greens {
        let blocked = promote_workflow(&state, &run.id, None, true);
        assert!(
            blocked.is_err(),
            "run {} must not be promotable while halted",
            run.id
        );
        assert!(blocked.unwrap_err().contains("is halted"));
    }
    assert_eq!(champion_of(&state, KEY.0, KEY.1, KEY.2), None);

    // A human resume is the only way forward, and then the run is promotable again — which is
    // correct: the halt was the refusal, not a verdict on the run.
    resume_workflow_search(&state, KEY.0, KEY.1, KEY.2, "overriding the three noes", true)
        .expect("resume");
    promote_workflow(&state, &greens[0].id, None, true).expect("promotable once resumed");
    assert_eq!(
        champion_of(&state, KEY.0, KEY.1, KEY.2).as_deref(),
        Some(w.id.as_str())
    );
}

// =================================================================================================
// `resume` — the new decision kind
// =================================================================================================

/// A resume with nothing to resume is refused at 0, 1 and 2 strikes, and the refusal names the
/// count so a human can see how far off a halt they are.
#[test]
fn qa_a2_a_resume_with_nothing_halted_is_refused_at_every_strike_count() {
    let (state, _root) = test_state();
    let w = workflow(&state, KEY.0, KEY.1, KEY.2);

    for expected_strikes in 0..3 {
        let blocked = resume_workflow_search(&state, KEY.0, KEY.1, KEY.2, "let me through", true);
        assert!(
            blocked.is_err(),
            "a resume at {expected_strikes} strikes must be refused"
        );
        let error = blocked.unwrap_err();
        assert!(error.contains("is not halted"), "got: {error}");
        assert!(
            error.contains(&format!("{} of 3 strikes", expected_strikes)),
            "the refusal must state the strike count: {error}"
        );
        assert!(
            decisions(&state).iter().all(|(kind, _)| kind != "resume"),
            "a refused resume must write nothing"
        );
        let run = measured_run(&state, &w.id);
        reject_workflow_run(&state, &run.id, "no").expect("reject");
    }
    // The third rejection halts, and NOW a resume is allowed.
    assert!(
        workflow_search_state(&state, KEY.0, KEY.1, KEY.2)
            .expect("state")
            .halted
    );
    resume_workflow_search(&state, KEY.0, KEY.1, KEY.2, "now there is something to resume", true)
        .expect("resume");
}

/// A resume cannot be replayed: the second call sees an unhalted search and is refused. A fresh
/// halt is resumable again, which is the point — this is a per-halt permission, not a one-off.
#[test]
fn qa_a2_a_resume_cannot_be_replayed() {
    let (state, _root) = test_state();
    let w = workflow(&state, KEY.0, KEY.1, KEY.2);
    halt(&state, &w.id);

    resume_workflow_search(&state, KEY.0, KEY.1, KEY.2, "first", true).expect("resume");
    for attempt in 0..3 {
        let replay = resume_workflow_search(&state, KEY.0, KEY.1, KEY.2, "again", true);
        assert!(replay.is_err(), "replay {attempt} must be refused");
        assert!(replay.unwrap_err().contains("is not halted"));
    }
    assert_eq!(
        decisions(&state)
            .into_iter()
            .filter(|(kind, _)| kind == "resume")
            .count(),
        1,
        "exactly one resume row for one halt"
    );

    // Three more distinct candidates, a new halt, a second legitimate resume.
    for strike in 1..=3 {
        let run = measured_run(&state, &w.id);
        reject_workflow_run(&state, &run.id, &format!("round two, {strike}")).expect("reject");
    }
    assert!(
        workflow_search_state(&state, KEY.0, KEY.1, KEY.2)
            .expect("state")
            .halted
    );
    resume_workflow_search(&state, KEY.0, KEY.1, KEY.2, "second", true).expect("resume");
    assert_eq!(
        decisions(&state)
            .into_iter()
            .filter(|(kind, _)| kind == "resume")
            .count(),
        2
    );
}

/// Confirmation and a non-blank reason are both mandatory, and neither failure writes a row.
#[test]
fn qa_a2_a_resume_needs_confirmation_and_a_real_reason() {
    let (state, _root) = test_state();
    let w = workflow(&state, KEY.0, KEY.1, KEY.2);
    halt(&state, &w.id);

    let unconfirmed = resume_workflow_search(&state, KEY.0, KEY.1, KEY.2, "a good reason", false);
    assert!(unconfirmed.is_err());
    assert!(unconfirmed.unwrap_err().contains("human-confirmed"));

    for blank in ["", " ", "   ", "\t", "\n", " \t\r\n "] {
        let refused = resume_workflow_search(&state, KEY.0, KEY.1, KEY.2, blank, true);
        assert!(
            refused.is_err(),
            "a resume reason of {blank:?} must be refused"
        );
        assert!(refused.unwrap_err().contains("must record a reason"));
    }
    assert!(
        decisions(&state).iter().all(|(kind, _)| kind != "resume"),
        "no refused resume wrote a row"
    );
    assert!(
        workflow_search_state(&state, KEY.0, KEY.1, KEY.2)
            .expect("state")
            .halted,
        "and the halt is intact"
    );
}

/// The TABLE's half of the resume guard, driven with raw SQL so a future second writer is covered.
///
/// **It is only half.** The `CHECK` enforces a non-blank reason and a known `kind`, but the
/// "something must actually be halted" precondition lives only in `resume_workflow_search`. A
/// writer that bypasses the service can therefore insert a well-formed `resume` with nothing to
/// resume, and the streak walk will honour it — which is the free streak reset the service refuses.
/// Pinned because the migration's own comment claims the table "cannot be made useless by a future
/// second writer", and for this precondition that is not so.
#[test]
fn qa_a2_the_decision_table_guards_the_resume_reason_but_not_the_halt() {
    let (state, _root) = test_state();
    let w = workflow(&state, KEY.0, KEY.1, KEY.2);
    let run = measured_run(&state, &w.id);

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

    assert!(insert("r1", "resume", None).is_err(), "NULL resume reason");
    assert!(insert("r2", "resume", Some("  ")).is_err(), "blank reason");
    // **FIXED, and wider than reported.** Single-argument `trim()` strips U+0020 only, so the
    // CHECK admitted a reason made entirely of tabs and newlines. The replacement strips EVERY C0
    // control plus DEL via the trim set, and the zero-width family, the BOM and NUL via nested
    // `replace()` — because `trim(X, Y)` does not remove multi-byte members of Y, and a `char(…,
    // 0, …)` argument list truncates at the zero. Both were measured, not assumed.
    for (label, reason) in [
        ("tab+newline", "\t\n"),
        ("tab", "\t"),
        ("vertical tab", "\u{0b}"),
        ("form feed", "\u{0c}"),
        ("DEL", "\u{7f}"),
        ("zero width space", "\u{200b}"),
        ("BOM", "\u{feff}"),
        ("NUL then tab", "\0\t"),
    ] {
        assert!(
            insert(&format!("r3-{label}"), "resume", Some(reason)).is_err(),
            "a {label}-only resume reason must be refused by the CHECK"
        );
        assert!(
            insert(&format!("r3b-{label}"), "reject", Some(reason)).is_err(),
            "and on the rejection reason, which is the one the design calls the only thing a \
             halted search has to show"
        );
    }
    // Visible text with an invisible character in it is still fine: the rule is "contains
    // something visible", not "contains nothing invisible".
    assert!(insert("r3-ok", "reject", Some("too slow\u{200b}")).is_ok());
    assert!(insert("r4", "abstain", Some("x")).is_err(), "unknown kind");
    // A promote still may have a NULL reason — it is the one kind that needs none.
    assert!(insert("r5", "promote", None).is_ok());

    // One rejection, then a bypassing resume with nothing to resume.
    reject_workflow_run(&state, &run.id, "one strike").expect("reject");
    assert_eq!(
        workflow_search_state(&state, KEY.0, KEY.1, KEY.2)
            .expect("state")
            .consecutive_rejections,
        1
    );
    assert!(
        insert("r6", "resume", Some("inserted behind the service's back")).is_ok(),
        "DEFECT PIN: the table accepts a resume with nothing halted. The halted precondition is \
         enforced ONLY in resume_workflow_search, so the migration's claim that the table cannot \
         be made useless by a second writer does not cover it."
    );
    assert_eq!(
        workflow_search_state(&state, KEY.0, KEY.1, KEY.2)
            .expect("state")
            .consecutive_rejections,
        0,
        "DEFECT PIN: and the streak walk honours it, so the strike is gone"
    );
}

/// A resume ends the streak for the walk, and rejections after it count from zero — including
/// re-rejections of candidates that were already rejected before the resume. Three fresh strikes
/// after a resume halt the search again.
#[test]
fn qa_a2_rejections_after_a_resume_count_from_zero() {
    let (state, _root) = test_state();
    let w = workflow(&state, KEY.0, KEY.1, KEY.2);
    let runs = halt(&state, &w.id);
    resume_workflow_search(&state, KEY.0, KEY.1, KEY.2, "continue", true).expect("resume");

    let after = workflow_search_state(&state, KEY.0, KEY.1, KEY.2).expect("state");
    assert_eq!(after.consecutive_rejections, 0);
    assert!(after.recent_rejections.is_empty(), "and the reasons reset too");

    // Re-reject the SAME three candidates. Each is a distinct run, so each is a strike again, and
    // the pre-resume rows do not double-count.
    for (index, run) in runs.iter().enumerate() {
        let state_after =
            reject_workflow_run(&state, &run.id, &format!("again {index}")).expect("reject");
        assert_eq!(
            state_after.consecutive_rejections,
            index as i64 + 1,
            "strike {} after the resume",
            index + 1
        );
    }
    let halted = workflow_search_state(&state, KEY.0, KEY.1, KEY.2).expect("state");
    assert!(halted.halted);
    assert_eq!(halted.consecutive_rejections, 3);
    assert_eq!(
        halted.recent_rejections.len(),
        3,
        "only the post-resume reasons: {:?}",
        halted.recent_rejections
    );
    assert!(
        halted
            .recent_rejections
            .iter()
            .all(|reason| reason.starts_with("again")),
        "no pre-resume reason leaked through: {:?}",
        halted.recent_rejections
    );
}

/// A resume is scoped to one `(repo, phase_type, family)` — it does not lift another family's halt.
#[test]
fn qa_a2_a_resume_does_not_cross_a_family_boundary() {
    let (state, _root) = test_state();
    let glm = workflow(&state, "repo", "dev-phase", "glm");
    let qwen = workflow(&state, "repo", "dev-phase", "qwen");

    for w in [&glm, &qwen] {
        for strike in 1..=3 {
            let run = start_workflow_run(
                &state,
                NewWorkflowRun {
                    workflow_id: w.id.clone(),
                    plan_id: None,
                    seed: 0,
                    seats: None,
                },
            )
            .expect("start");
            reject_workflow_run(&state, &run.id, &format!("s{strike}")).expect("reject");
        }
    }
    resume_workflow_search(&state, "repo", "dev-phase", "glm", "glm only", true).expect("resume");
    assert!(
        !workflow_search_state(&state, "repo", "dev-phase", "glm")
            .expect("state")
            .halted
    );
    assert!(
        workflow_search_state(&state, "repo", "dev-phase", "qwen")
            .expect("state")
            .halted,
        "qwen's halt is untouched"
    );
}

// =================================================================================================
// The loser check, and `champion_promotion_run_id`'s reasoned invariant
// =================================================================================================

/// FIX CONFIRMED. A displacement must beat the run the incumbent was promoted on — not merely some
/// comparable run in the same family.
#[test]
fn qa_a2_a_displacement_must_beat_the_incumbents_own_run() {
    let (state, _root) = test_state();
    let incumbent = workflow(&state, KEY.0, KEY.1, KEY.2);
    let incumbent_run = green_measured(&state, &incumbent.id);
    let decoy = green_measured(&state, &incumbent.id); // same family, same budget, NOT the evidence
    promote_workflow(&state, &incumbent_run.id, None, true).expect("seed champion");

    let challenger = workflow(&state, KEY.0, KEY.1, KEY.2);
    let challenger_run = green_measured(&state, &challenger.id);

    let decoy_pref =
        record_workflow_pref(&state, &challenger_run.id, &decoy.id, None).expect("pref");
    assert!(
        decoy_pref.comparable,
        "the decoy pair IS comparable — which is why the loser check has to be separate"
    );
    let blocked = promote_workflow(&state, &challenger_run.id, Some(&decoy_pref.id), true);
    assert!(blocked.is_err(), "beating a decoy must not displace");
    let error = blocked.unwrap_err();
    assert!(error.contains("not won against the incumbent"), "got: {error}");
    assert!(
        error.contains(&incumbent_run.id),
        "the refusal must name the run that has to be beaten: {error}"
    );
    assert_eq!(
        champion_of(&state, KEY.0, KEY.1, KEY.2).as_deref(),
        Some(incumbent.id.as_str())
    );

    // Beating the right run works.
    let real = record_workflow_pref(&state, &challenger_run.id, &incumbent_run.id, None)
        .expect("pref");
    assert!(real.comparable);
    promote_workflow(&state, &challenger_run.id, Some(&real.id), true).expect("displaces");
    assert_eq!(
        champion_of(&state, KEY.0, KEY.1, KEY.2).as_deref(),
        Some(challenger.id.as_str())
    );
}

/// FIX CONFIRMED. The reverse pair `(loser, winner)` is deliberately still recordable, and it
/// cannot launder a displacement: the loser check reads the pair's own `loser_run_id`, so a
/// reversed preference simply names the wrong winner or the wrong loser.
#[test]
fn qa_a2_the_reverse_pair_cannot_launder_a_displacement() {
    let (state, _root) = test_state();
    let incumbent = workflow(&state, KEY.0, KEY.1, KEY.2);
    let incumbent_run = green_measured(&state, &incumbent.id);
    promote_workflow(&state, &incumbent_run.id, None, true).expect("seed champion");

    let challenger = workflow(&state, KEY.0, KEY.1, KEY.2);
    let challenger_run = green_measured(&state, &challenger.id);

    // The incumbent "wins" first, then the reverse pair is recorded as a contradiction.
    let forward = record_workflow_pref(&state, &incumbent_run.id, &challenger_run.id, Some("a".into()))
        .expect("forward");
    let reverse = record_workflow_pref(&state, &challenger_run.id, &incumbent_run.id, Some("b".into()))
        .expect("reverse is a contradiction, not a duplicate");
    assert_ne!(forward.id, reverse.id, "the reverse pair is its own row");

    // The forward pref names the incumbent as winner, so it cannot promote the challenger.
    let wrong_winner = promote_workflow(&state, &challenger_run.id, Some(&forward.id), true);
    assert!(wrong_winner.is_err());
    assert!(wrong_winner
        .unwrap_err()
        .contains("as the winner, not run"));

    // The reverse one is a legitimate head-to-head against the incumbent's own run and DOES
    // promote — which is correct: a human recorded that judgement second, and `workflow_prefs` is
    // a log of what humans decided, not a referee of whether they changed their minds.
    promote_workflow(&state, &challenger_run.id, Some(&reverse.id), true).expect("promotes");
    assert_eq!(
        champion_of(&state, KEY.0, KEY.1, KEY.2).as_deref(),
        Some(challenger.id.as_str())
    );
}

/// `champion_promotion_run_id`'s invariant — "the latest `promote` in a family is exactly the one
/// that put the sitting champion in the slot" — is labelled reasoned rather than measured. MEASURED
/// here, through the service only, across every sequence that could break it: seeding,
/// displacement, no-op re-promotion on the original run, no-op re-promotion on a DIFFERENT run of
/// the same workflow, interleaved rejections, a halt, and a resume.
#[test]
fn qa_a2_the_latest_promotion_really_is_the_sitting_champions() {
    let (state, _root) = test_state();
    let a = workflow(&state, KEY.0, KEY.1, KEY.2);
    let b = workflow(&state, KEY.0, KEY.1, KEY.2);
    let c = workflow(&state, KEY.0, KEY.1, KEY.2);

    let check = |label: &str| {
        let champion = champion_of(&state, KEY.0, KEY.1, KEY.2).expect(label);
        let promote_run = latest_promote_run(&state, KEY.0, KEY.1, KEY.2).expect(label);
        let run = get_workflow_run(&state, &promote_run).expect("run");
        assert_eq!(
            run.workflow_id, champion,
            "{label}: the latest promote must belong to the sitting champion"
        );
    };

    let a1 = green_measured(&state, &a.id);
    promote_workflow(&state, &a1.id, None, true).expect("seed");
    check("after seeding");

    // A no-op re-promotion on a DIFFERENT run of the same workflow: must not move the evidence.
    let a2 = green_measured(&state, &a.id);
    promote_workflow(&state, &a2.id, None, true).expect("no-op");
    assert_eq!(
        latest_promote_run(&state, KEY.0, KEY.1, KEY.2).as_deref(),
        Some(a1.id.as_str()),
        "a no-op re-promotion on another run must not rewrite which run the champion rests on"
    );
    check("after a no-op re-promotion on a different run");

    // Displace A with B, beating A's promoting run.
    let b1 = green_measured(&state, &b.id);
    let pref = record_workflow_pref(&state, &b1.id, &a1.id, None).expect("pref");
    promote_workflow(&state, &b1.id, Some(&pref.id), true).expect("displace");
    check("after a displacement");
    assert_eq!(
        latest_promote_run(&state, KEY.0, KEY.1, KEY.2).as_deref(),
        Some(b1.id.as_str())
    );

    // A rejection, a halt and a resume in between must not disturb it.
    for strike in 1..=3 {
        let run = measured_run(&state, &c.id);
        reject_workflow_run(&state, &run.id, &format!("s{strike}")).expect("reject");
    }
    check("while halted");
    resume_workflow_search(&state, KEY.0, KEY.1, KEY.2, "continue", true).expect("resume");
    check("after a resume");

    // And C displaces B.
    let c1 = green_measured(&state, &c.id);
    let pref = record_workflow_pref(&state, &c1.id, &b1.id, None).expect("pref");
    promote_workflow(&state, &c1.id, Some(&pref.id), true).expect("displace");
    check("after the second displacement");
}

/// FIX CONFIRMED. A champion row with NO promotion behind it refuses displacement rather than
/// guessing — the case Dev A's comment claims to handle.
#[test]
fn qa_a2_a_champion_with_no_promotion_behind_it_refuses_displacement() {
    let (state, _root) = test_state();
    let squatter = workflow(&state, KEY.0, KEY.1, KEY.2);
    let challenger = workflow(&state, KEY.0, KEY.1, KEY.2);

    // A writer that bypasses the service puts `squatter` in the slot with no decision row.
    state
        .db
        .with_conn(|conn| {
            conn.execute(
                "UPDATE workflows SET is_champion = 1 WHERE id = ?1",
                params![squatter.id],
            )
            .map_err(|e| e.to_string())
        })
        .expect("raw champion");
    assert!(latest_promote_run(&state, KEY.0, KEY.1, KEY.2).is_none());

    let challenger_run = green_measured(&state, &challenger.id);
    let victim = green_measured(&state, &squatter.id);
    let pref = record_workflow_pref(&state, &challenger_run.id, &victim.id, None).expect("pref");
    let blocked = promote_workflow(&state, &challenger_run.id, Some(&pref.id), true);
    assert!(blocked.is_err(), "no recorded promotion => refuse");
    let error = blocked.unwrap_err();
    assert!(error.contains("no promotion is recorded"), "got: {error}");
    assert!(error.contains("Refusing rather than guessing"), "got: {error}");
}

/// **FIXED. Inverted from the defect pin of the same shape.** The STALE case is refused too, not
/// just the absent one.
///
/// What was wrong: `champion_promotion_run_id` took the latest `promote` row in the FAMILY, and
/// `promotion_block` never checked that the run it named belonged to the workflow actually holding
/// the slot. When the two separately-stored facts disagreed, a challenger could displace the
/// sitting champion by beating a run of a completely different workflow — and, symmetrically, a
/// preference won against the sitting champion's OWN run was refused. The rule pointed at the wrong
/// thing in both directions.
///
/// The fix is QA's one line, applied at the query instead of the comparison: the promote row is
/// looked up BY THE INCUMBENT'S WORKFLOW, so a slot whose holder has no promotion behind it yields
/// `None` and `promotion_block` refuses rather than guessing. Both directions now fail closed,
/// which is the right answer for a state only a bypassing writer can produce.
#[test]
fn qa_a2_a_stale_promote_row_cannot_launder_a_displacement() {
    let (state, _root) = test_state();
    let promoted = workflow(&state, KEY.0, KEY.1, KEY.2);
    let squatter = workflow(&state, KEY.0, KEY.1, KEY.2);
    let challenger = workflow(&state, KEY.0, KEY.1, KEY.2);

    // A legitimate promotion first, so the family HAS a promote row.
    let promoted_run = green_measured(&state, &promoted.id);
    promote_workflow(&state, &promoted_run.id, None, true).expect("seed");
    assert_eq!(
        latest_promote_run(&state, KEY.0, KEY.1, KEY.2).as_deref(),
        Some(promoted_run.id.as_str())
    );

    // A bypassing writer moves the slot to `squatter`. The promote row still points at
    // `promoted`'s run. Demote first, or the partial unique index refuses — which is itself worth
    // noting: the index stops two champions, not a wrong one.
    state
        .db
        .with_conn(|conn| {
            conn.execute(
                "UPDATE workflows SET is_champion = 0 WHERE id = ?1",
                params![promoted.id],
            )
            .map_err(|e| e.to_string())?;
            conn.execute(
                "UPDATE workflows SET is_champion = 1 WHERE id = ?1",
                params![squatter.id],
            )
            .map_err(|e| e.to_string())
        })
        .expect("raw swap");
    assert_eq!(
        champion_of(&state, KEY.0, KEY.1, KEY.2).as_deref(),
        Some(squatter.id.as_str()),
        "the slot and the promote row now disagree"
    );

    // Beating `promoted`'s run — which has nothing to do with the sitting champion — displaces the
    // sitting champion.
    let challenger_run = green_measured(&state, &challenger.id);
    let pref = record_workflow_pref(&state, &challenger_run.id, &promoted_run.id, None)
        .expect("pref");
    assert!(pref.comparable);
    let laundered = promote_workflow(&state, &challenger_run.id, Some(&pref.id), true);
    let error = laundered.expect_err(
        "a stale promote row must not let a challenger displace a champion by beating a run of a \
         DIFFERENT workflow",
    );
    assert!(
        error.contains("no promotion is recorded for it"),
        "the refusal must name the disagreement rather than guessing: {error}"
    );
    assert_eq!(
        champion_of(&state, KEY.0, KEY.1, KEY.2).as_deref(),
        Some(squatter.id.as_str()),
        "the squatter keeps the slot — a bypassing writer's state is not evidence either way"
    );

    // The symmetric consequence: a preference against the ACTUAL sitting champion's own run is
    // refused, because the stale row says the evidence is elsewhere. The rule is not merely weak
    // here, it is pointing at the wrong thing in both directions.
    let (state2, _root2) = test_state();
    let p = workflow(&state2, KEY.0, KEY.1, KEY.2);
    let s = workflow(&state2, KEY.0, KEY.1, KEY.2);
    let ch = workflow(&state2, KEY.0, KEY.1, KEY.2);
    let p_run = green_measured(&state2, &p.id);
    promote_workflow(&state2, &p_run.id, None, true).expect("seed");
    let s_run = green_measured(&state2, &s.id);
    state2
        .db
        .with_conn(|conn| {
            conn.execute("UPDATE workflows SET is_champion = 0 WHERE id = ?1", params![p.id])
                .map_err(|e| e.to_string())?;
            conn.execute("UPDATE workflows SET is_champion = 1 WHERE id = ?1", params![s.id])
                .map_err(|e| e.to_string())
        })
        .expect("raw swap");
    let ch_run = green_measured(&state2, &ch.id);
    let honest = record_workflow_pref(&state2, &ch_run.id, &s_run.id, None).expect("pref");
    // The symmetric direction: still refused, but now for the RIGHT reason — the squatter has no
    // promotion behind it, so there is no incumbent run to beat. Fail-closed in both directions.
    let symmetric = promote_workflow(&state2, &ch_run.id, Some(&honest.id), true)
        .expect_err("a slot with no promotion behind it cannot be displaced");
    assert!(
        symmetric.contains("no promotion is recorded for it"),
        "got: {symmetric}"
    );

    // Note what is NOT asserted here: promoting the squatter's own run. `s` already holds the slot,
    // so that is the idempotent no-op path and it succeeds — which is correct, and is how a
    // bypassing writer's state gets a real promotion behind it again.
}

// =================================================================================================
// Comparability — attacking whichever rule is in the tree
// =================================================================================================

/// **Which comparability rule is in the tree, established by its OBSERVABLE truth table rather
/// than by the names of its helpers — and the finding that falls out of it.**
///
/// Two rules have been written for this. `7067027` ships strict `fleet_busy` EQUALITY. An
/// uncommitted rewrite in the working tree replaces it with "a run that started under foreign
/// contention is UNSCOREABLE, plus an escape for two runs that saw IDENTICAL contention".
///
/// The table below is the whole input space, and **both rules produce every row of it**:
///
/// | winner / loser `fleet_busy` | equality | exclude + identity escape |
/// |---|---|---|
/// | `{}` vs `{}`                | equal -> comparable     | both quiet -> comparable |
/// | `{}` vs `{"qwen":0}`        | equal -> comparable     | both quiet -> comparable |
/// | `{}` vs `{"qwen":1}`        | unequal -> refuse       | loser unscoreable, not identical -> refuse |
/// | `{"qwen":1}` vs `{"qwen":2}`| unequal -> refuse       | both unscoreable, not identical -> refuse |
/// | `{"qwen":9}` vs `{"qwen":9}`| equal -> comparable     | both unscoreable BUT identical -> escape -> comparable |
/// | unreadable, either side     | refuse (fail closed)    | refuse (fail closed) |
///
/// The identity escape re-admits exactly the pairs equality admitted and no others, so the rewrite
/// does not change a single comparability outcome. The stated reason for it was that equality
/// "collapses to never once contention is real" — and the replacement collapses to never in
/// precisely the same cases. What actually changed is the refusal TEXT and a new per-run
/// `scoreable` reason a client can display. Both are worth having; neither is the behavioural
/// change the rewrite was asked for.
///
/// This test therefore asserts outcomes only, and passing against both implementations IS the
/// evidence.
#[test]
fn qa_a2_both_comparability_rules_admit_exactly_the_same_pairs() {
    let (state, _root) = test_state();
    let w = workflow(&state, KEY.0, KEY.1, KEY.2);
    let seats = r#"{"glm":2}"#;

    let pair = |left_fleet: &str, right_fleet: &str| -> bool {
        let a = run_under(&state, &w.id, seats, left_fleet);
        let b = run_under(&state, &w.id, seats, right_fleet);
        let a = green(&state, &a);
        let b = green(&state, &b);
        record_workflow_pref(&state, &a.id, &b.id, None)
            .expect("pref")
            .comparable
    };

    assert!(pair("{}", "{}"), "quiet vs quiet");
    assert!(pair("{}", r#"{"glm":0}"#), "an explicit zero normalises to quiet");
    assert!(!pair("{}", r#"{"qwen":1}"#), "quiet vs one foreign seat");
    assert!(!pair(r#"{"qwen":1}"#, "{}"), "and symmetrically");
    assert!(!pair(r#"{"qwen":1}"#, r#"{"qwen":2}"#), "different loads");
    assert!(
        pair(r#"{"qwen":4096,"glm":77}"#, r#"{"glm":77,"qwen":4096}"#),
        "identical heavy contention is comparable under BOTH rules — key order is normalised, and \
         this is the row that shows the rewrite changed no outcome"
    );
    // The unreadable rows of the table are UNREACHABLE through the service, which is a stronger
    // guarantee than failing closed: `start_workflow_run_with_budget` validates both maps before
    // the insert, so no run can be stored with a contention map that cannot be parsed.
    for bad in ["{", "null", "[]", r#"{"q":-1}"#, r#"{"q":1.5}"#] {
        assert!(
            start_workflow_run_with_budget(
                &state,
                NewWorkflowRun {
                    workflow_id: w.id.clone(),
                    plan_id: None,
                    seed: 0,
                    seats: None,
                },
                RunBudget {
                    seats_held: seats.to_string(),
                    fleet_busy: bad.to_string(),
                },
            )
            .is_err(),
            "an unreadable fleet_busy of {bad:?} must never be stored"
        );
    }
    // And if one ever did get in by another route, the rule fails closed. Driven at the predicate,
    // since no row can hold it.
    assert!(!contention_was_identical("{", "{}"));
    assert!(!contention_was_identical("{}", "{"));
    assert!(
        !contention_was_identical("{", "{"),
        "two identically-unreadable maps are not rescued by the identity escape: both rules parse \
         first and fail closed"
    );
}

/// FIX CONFIRMED, for the case that matters. A run taken under foreign contention cannot promote,
/// and the refusal says why in words naming the contention.
#[test]
fn qa_a2_a_run_under_foreign_contention_cannot_promote() {
    let (state, _root) = test_state();
    let seats = r#"{"glm":2}"#;
    let incumbent = workflow(&state, KEY.0, KEY.1, KEY.2);
    let incumbent_run = green(&state, &run_under(&state, &incumbent.id, seats, "{}"));
    promote_workflow(&state, &incumbent_run.id, None, true).expect("seed champion");

    let challenger = workflow(&state, KEY.0, KEY.1, KEY.2);
    let busy = green(
        &state,
        &run_under(&state, &challenger.id, seats, r#"{"qwen":3}"#),
    );
    let pref = record_workflow_pref(&state, &busy.id, &incumbent_run.id, None).expect("pref");
    assert!(!pref.comparable, "a loaded challenger is not comparable");
    // The refusal text differs between the two rules; what must hold either way is that the
    // stored reason names CONTENTION as the cause, so a human is not left guessing.
    let reason = pref.reason.unwrap_or_default();
    assert!(
        reason.contains("contention") || reason.contains("unscoreable") || reason.contains("fleet"),
        "the stored reason must name the contention: {reason}"
    );
    // Round 3+: `promotion_block` refuses an unscoreable run BEFORE it looks at the preference,
    // including on the seeding path that has no preference at all, so the refusal now names the
    // contention directly rather than the downstream "not comparable".
    let refusal = promote_workflow(&state, &busy.id, Some(&pref.id), true).unwrap_err();
    assert!(
        refusal.contains("cannot be scored") && refusal.contains("quiet fleet"),
        "the refusal must name the contention: {refusal}"
    );
    assert_eq!(
        champion_of(&state, KEY.0, KEY.1, KEY.2).as_deref(),
        Some(incumbent.id.as_str())
    );
}

/// **DEFECT (documented, not fixed) — the one Dev A pinned, verified end to end rather than at the
/// measurement function.**
///
/// Dev A's own pin asserts only that `measure_run_budget` returns `fleet_busy == "{}"`. That is the
/// cause; this is the consequence, which is the part that matters: through the REAL
/// `start_workflow_run` — the only path a relay client can reach — every run is reported quiet,
/// every run is therefore scoreable, and the contention half of the comparability rule has nothing
/// to read. An unmeasured fleet is indistinguishable from a quiet one, and the system cannot tell a
/// human which it was.
///
/// Both halves must break when the lease pool lands. Dev A's pin breaks on the measurement; this
/// one breaks on the promotion that the measurement was supposed to prevent.
#[test]
fn qa_a2_an_unmeasured_fleet_is_indistinguishable_from_a_quiet_one() {
    let (state, _root) = test_state();
    let incumbent = workflow(&state, KEY.0, KEY.1, KEY.2);
    let challenger = workflow(&state, KEY.0, KEY.1, KEY.2);

    let incumbent_run = green_measured(&state, &incumbent.id);
    let challenger_run = green_measured(&state, &challenger.id);
    assert_eq!(
        (incumbent_run.fleet_busy.as_str(), challenger_run.fleet_busy.as_str()),
        ("{}", "{}"),
        "DEFECT PIN: every run started through the relay path reports a quiet fleet"
    );
    assert_eq!(
        measure_run_budget(&state, "glm", 2).expect("measure").fleet_busy,
        "{}",
        "DEFECT PIN: the measurement itself reports quiet for everything"
    );

    promote_workflow(&state, &incumbent_run.id, None, true).expect("seed champion");
    let pref = record_workflow_pref(&state, &challenger_run.id, &incumbent_run.id, None)
        .expect("pref");
    assert!(
        pref.comparable,
        "DEFECT PIN: comparability is seats-only until the lease pool lands"
    );
    assert!(
        promote_workflow(&state, &challenger_run.id, Some(&pref.id), true).is_ok(),
        "DEFECT PIN: and a champion is crowned on a comparison that measured nothing about the \
         fleet. Wiring services::leases::LeasePool must break this line as well as Dev A's \
         measurement pin."
    );
}

/// **A consequence of deriving `seats_held` from `fan_out`, which nothing else states.** Two
/// workflows in one family that declare different `fan_out` can never produce a comparable
/// preference, so a challenger with a different seat cost can never displace the champion — the
/// arena cannot search over `fan_out` at all. Not obviously wrong (different budgets are genuinely
/// different experiments) but it is a hard limit on what the search can explore, and it is not
/// written down anywhere.
#[test]
fn qa_a2_a_challenger_with_a_different_fan_out_can_never_displace() {
    let (state, _root) = test_state();
    let incumbent =
        create_workflow_version(&state, version_with(KEY.0, KEY.1, KEY.2, 2)).expect("create");
    let cheaper =
        create_workflow_version(&state, version_with(KEY.0, KEY.1, KEY.2, 1)).expect("create");

    let incumbent_run = green_measured(&state, &incumbent.id);
    promote_workflow(&state, &incumbent_run.id, None, true).expect("seed champion");
    let cheaper_run = green_measured(&state, &cheaper.id);
    assert_ne!(
        incumbent_run.seats_held, cheaper_run.seats_held,
        "fan_out flows straight into seats_held"
    );

    let pref = record_workflow_pref(&state, &cheaper_run.id, &incumbent_run.id, None).expect("pref");
    assert!(!pref.comparable);
    assert_eq!(
        pref.reason.as_deref(),
        Some("the two runs held different seat budgets")
    );
    assert!(promote_workflow(&state, &cheaper_run.id, Some(&pref.id), true).is_err());

    // And there is no retry that helps: `seats_held` is immutable, and overriding `seats` at start
    // time only moves which of the two is odd one out.
    let matched = start_workflow_run(
        &state,
        NewWorkflowRun {
            workflow_id: cheaper.id.clone(),
            plan_id: None,
            seed: 0,
            seats: Some(2),
        },
    )
    .expect("start with an override");
    let matched = green(&state, &matched);
    let pref2 = record_workflow_pref(&state, &matched.id, &incumbent_run.id, None).expect("pref");
    assert!(
        pref2.comparable,
        "the only route is to run the challenger on the INCUMBENT's seat budget, which means the \
         fan_out being tested is not the fan_out that ran"
    );
}

// =================================================================================================
// The re-score lock and the host's new bounds
// =================================================================================================

/// FIX CONFIRMED. A second gate result needs the named entry point, and the run a sitting champion
/// was promoted on cannot be re-scored even with it.
#[test]
fn qa_a2_the_rescore_lock_holds_and_releases_correctly() {
    let (state, _root) = test_state();
    let champion = workflow(&state, KEY.0, KEY.1, KEY.2);
    let champion_run = green_measured(&state, &champion.id);

    // A second plain record is refused and changes nothing.
    let blocked = record_workflow_run_gate(&state, &champion_run.id, 1, Some(0));
    assert!(blocked.is_err());
    let error = blocked.unwrap_err();
    assert!(error.contains("already has a gate result"), "got: {error}");
    assert!(error.contains("rescore_workflow_run_gate"), "got: {error}");
    let unchanged = get_workflow_run(&state, &champion_run.id).expect("run");
    assert_eq!((unchanged.gate_exit, unchanged.gate_collected), (Some(0), Some(7)));

    // Before promotion the explicit re-score is allowed.
    let rescored = rescore_workflow_run_gate(&state, &champion_run.id, 0, Some(9))
        .expect("an explicit rescore is allowed before promotion");
    assert_eq!(rescored.gate_collected, Some(9));

    promote_workflow(&state, &champion_run.id, None, true).expect("promote");

    // After promotion it is refused by BOTH entry points.
    assert!(record_workflow_run_gate(&state, &champion_run.id, 1, None).is_err());
    let locked = rescore_workflow_run_gate(&state, &champion_run.id, 1, None);
    assert!(locked.is_err(), "the champion's evidence must be immutable");
    let error = locked.unwrap_err();
    assert!(error.contains("cannot be rewritten underneath it"), "got: {error}");
    assert_eq!(
        get_workflow_run(&state, &champion_run.id).unwrap().gate_exit,
        Some(0),
        "and the score really did not move"
    );

    // A non-champion run in the same family is still re-scorable.
    let other = green_measured(&state, &champion.id);
    rescore_workflow_run_gate(&state, &other.id, 1, None).expect("other runs are not locked");

    // Once displaced, the former champion's run is re-scorable again — correct, it is no longer
    // the evidence anything rests on.
    let challenger = workflow(&state, KEY.0, KEY.1, KEY.2);
    let challenger_run = green_measured(&state, &challenger.id);
    let pref =
        record_workflow_pref(&state, &challenger_run.id, &champion_run.id, None).expect("pref");
    promote_workflow(&state, &challenger_run.id, Some(&pref.id), true).expect("displace");
    rescore_workflow_run_gate(&state, &champion_run.id, 1, None)
        .expect("a former champion's run is no longer locked");
    // …and the new champion's is.
    assert!(rescore_workflow_run_gate(&state, &challenger_run.id, 1, None).is_err());
}

/// FIX CONFIRMED, with the one deliberate exception measured. `fan_out`, `seed` and `gate_exit` are
/// bounded by the host; `gate_collected` is deliberately NOT, which is the field that then has to
/// survive the wire.
#[test]
fn qa_a2_the_host_bounds_everything_except_gate_collected() {
    let (state, _root) = test_state();

    // fan_out
    for bad in [0, -1, MAX_FAN_OUT + 1, i64::MAX] {
        assert!(
            create_workflow_version(&state, version_with(KEY.0, KEY.1, KEY.2, bad)).is_err(),
            "fan_out {bad} must be refused"
        );
    }
    let w = create_workflow_version(&state, version_with(KEY.0, KEY.1, KEY.2, MAX_FAN_OUT))
        .expect("the boundary itself is allowed");
    assert_eq!(w.fan_out, MAX_FAN_OUT);
    // …but a run of it cannot be started, because the seat count is bounded by the same constant
    // and the boundary is inclusive on both sides. Pinned so the two bounds cannot drift apart.
    assert!(
        start_workflow_run(
            &state,
            NewWorkflowRun {
                workflow_id: w.id.clone(),
                plan_id: None,
                seed: 0,
                seats: None,
            },
        )
        .is_ok(),
        "MAX_FAN_OUT seats must be startable, or the two bounds disagree"
    );

    let normal = workflow(&state, "repo2", KEY.1, KEY.2);
    // seed
    for bad in [-1, MAX_SEED + 1, i64::MAX] {
        assert!(
            start_workflow_run(
                &state,
                NewWorkflowRun {
                    workflow_id: normal.id.clone(),
                    plan_id: None,
                    seed: bad,
                    seats: None,
                },
            )
            .is_err(),
            "seed {bad} must be refused"
        );
    }
    let at_bound = start_workflow_run(
        &state,
        NewWorkflowRun {
            workflow_id: normal.id.clone(),
            plan_id: None,
            seed: MAX_SEED,
            seats: None,
        },
    )
    .expect("the boundary is allowed");
    assert_eq!(at_bound.seed, MAX_SEED);

    // seats override is bounded too, so the override is not a way round MAX_FAN_OUT.
    for bad in [0, -5, MAX_FAN_OUT + 1] {
        assert!(
            start_workflow_run(
                &state,
                NewWorkflowRun {
                    workflow_id: normal.id.clone(),
                    plan_id: None,
                    seed: 0,
                    seats: Some(bad),
                },
            )
            .is_err(),
            "a seats override of {bad} must be refused"
        );
    }

    // gate_exit: a process exit code, 32 bits.
    let run = measured_run(&state, &normal.id);
    for bad in [i32::MAX as i64 + 1, i32::MIN as i64 - 1, i64::MAX, i64::MIN] {
        assert!(
            record_workflow_run_gate(&state, &run.id, bad, None).is_err(),
            "gate_exit {bad} must be refused"
        );
    }
    let scored = record_workflow_run_gate(&state, &run.id, i32::MAX as i64, None)
        .expect("the boundary is allowed");
    assert_eq!(scored.gate_exit, Some(i32::MAX as i64));

    // gate_collected: deliberately unbounded, and it round-trips as an i64 beyond 2^53 — where a
    // JSON number stops being exactly representable as an IEEE-754 double. The wire consequence is
    // measured in worker/lib/ai-scope-tests/workflow-arena-int32-qa.test.ts.
    let run2 = measured_run(&state, &normal.id);
    let huge = 9_007_199_254_740_993_i64; // 2^53 + 1, the first integer a double cannot hold
    let scored2 =
        record_workflow_run_gate(&state, &run2.id, 0, Some(huge)).expect("unbounded on purpose");
    assert_eq!(
        scored2.gate_collected,
        Some(huge),
        "the host stores 2^53+1 exactly"
    );
    let json = serde_json::to_value(&scored2).expect("serialise");
    assert_eq!(
        json["gateCollected"].as_i64(),
        Some(huge),
        "and serialises it exactly: the precision is lost later, on the wire, not here"
    );
}
