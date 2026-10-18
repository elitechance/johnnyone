//! QA A's ROUND-4 tests: the final verification pass, and the one hunt that turned something up.
//!
//! A fourth file for the reason the second and third exist — the earlier ones are co-authored now,
//! and keeping each round separable means neither author range-edits the other's work.
//!
//! Three of these confirm fixes. One documents a third batch of invisible code points, which by the
//! condition written on `has_visible_content` is the trigger for a Unicode property table rather
//! than a tenth hand-added range. One settles a disagreement between a claim of mine and a fact of
//! Dev A's, in Dev A's favour.

#![cfg(test)]

use crate::services::workflows::*;
use crate::state::app_state::AppState;
use crate::test_support::{test_state, tmp_dir};
use rusqlite::params;
use std::path::Path;

const KEY: (&str, &str, &str) = ("repo", "dev-phase", "glm");

fn repo_path(name: &str) -> String {
    format!("/nonexistent-arena-qa-r4/{}", name)
}

fn repo_id(name: &str) -> String {
    repo_identity(&repo_path(name)).expect("derive the repo identity")
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
        fan_out: 2,
    }
}

fn workflow(state: &AppState, repo: &str, phase_type: &str, family: &str) -> Workflow {
    create_workflow_version(state, version(repo, phase_type, family)).expect("create")
}

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

fn search(state: &AppState, repo: &str, phase: &str, family: &str) -> WorkflowSearchState {
    workflow_search_state(state, &repo_id(repo), phase, family).expect("state")
}

fn git(dir: &Path, args: &[&str]) {
    let out = std::process::Command::new("git")
        .args(args)
        .current_dir(dir)
        .output()
        .expect("git");
    assert!(
        out.status.success(),
        "git {:?} failed: {}",
        args,
        String::from_utf8_lossy(&out.stderr)
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

// =================================================================================================
// PRIORITY 2 — the tenth hole. There is a third batch, and it is bigger than the first two.
// =================================================================================================

/// **DEFECT (documented, not fixed). A THIRD batch of invisible code points, ~370 of them, which is
/// larger than the entire current denylist.**
///
/// Round 3 found nine, all of which were added. `has_visible_content` carries the condition that
/// would flip the decision, in its own words: *"If a third audit finds a third batch, that calculus
/// flips and the table is the right call."* This is that audit. Every representative below renders
/// as nothing, is not Unicode `White_Space`, is not `is_control`, and is not in the denylist — so a
/// rejection or resume reason made only of it is accepted and stored.
///
/// These are RANGE representatives, not nineteen code points:
///
/// | range | count | what |
/// |---|---|---|
/// | `U+034F` | 1 | combining grapheme joiner |
/// | `U+180B`–`U+180D`, `U+180F` | 4 | mongolian free variation selectors 1–4 |
/// | `U+2065` | 1 | unassigned, `Default_Ignorable` |
/// | `U+FFF0`–`U+FFF8` | 9 | unassigned, `Default_Ignorable` |
/// | `U+110BD`, `U+110CD` | 2 | kaithi number signs (prepended concatenation marks) |
/// | `U+13430`–`U+13438` | 9 | egyptian hieroglyph format controls |
/// | `U+1D173`–`U+1D17A` | 8 | musical symbol beam/phrase controls |
/// | `U+E0001`, `U+E0020`–`U+E007F` | 97 | language tag, tag characters |
/// | `U+E0100`–`U+E01EF` | 240 | variation selectors supplement |
///
/// The supplement alone is 240 — more than every range the denylist currently names put together.
/// The denylist is chasing `Default_Ignorable_Code_Point` one batch at a time and is not converging,
/// which is the argument the function's own comment says would settle it. The decision is the
/// coordinator's, not mine and not the author's; the cost of a miss is still bounded and
/// unglamorous (one reason that renders blank in a field nothing branches on), so this is a
/// direction call, not an emergency.
#[test]
fn qa_a4_the_unicode_property_refuses_the_third_batch_entirely() {
    let (state, _root) = test_state();
    let w = workflow(&state, KEY.0, KEY.1, KEY.2);
    let run = start_against(&state, &w.id, None);
    // Created UP FRONT: every invisible reason the denylist lets through is itself a rejection of
    // `run`, so by the time the probe loop finishes the family is one strike down and
    // `start_workflow_run` would refuse three more. (My first draft created them afterwards and the
    // fixture refused itself — caught by the assertion, fixed rather than worked around.)
    let victims: Vec<String> = (0..3).map(|_| start_against(&state, &w.id, None).id).collect();

    // Range representatives, as `(label, code point)`.
    let third_batch: &[(&str, u32)] = &[
        ("combining-grapheme-joiner", 0x034F),
        ("mongolian-fvs1", 0x180B),
        ("mongolian-fvs2", 0x180C),
        ("mongolian-fvs3", 0x180D),
        ("mongolian-fvs4", 0x180F),
        ("unassigned-2065", 0x2065),
        ("unassigned-fff0", 0xFFF0),
        ("unassigned-fff8", 0xFFF8),
        ("kaithi-number-sign", 0x110BD),
        ("kaithi-number-sign-above", 0x110CD),
        ("egyptian-vertical-joiner", 0x13430),
        ("egyptian-end-segment", 0x13438),
        ("musical-begin-beam", 0x1D173),
        ("musical-end-phrase", 0x1D17A),
        ("language-tag", 0xE0001),
        ("tag-space", 0xE0020),
        ("cancel-tag", 0xE007F),
        ("variation-selector-17", 0xE0100),
        ("variation-selector-256", 0xE01EF),
    ];

    let mut holes = Vec::new();
    for (label, code_point) in third_batch {
        let character = char::from_u32(*code_point).expect("a valid scalar value");
        let reason = character.to_string();
        if reject_workflow_run(&state, &run.id, &reason).is_ok() {
            holes.push(*label);
        }
    }
    // FIXED, and by the table rather than by a fourth round of growing a list. The denylist is
    // gone: the rule is `Default_Ignorable_Code_Point` (Unicode 16.0, encoded in-crate with the
    // version named beside it) plus the three groups the property deliberately omits — the braille
    // blank, the interlinear annotation controls, and the prepended concatenation marks. The last
    // of those is this corpus's contribution: the two Kaithi number signs passed a table that
    // implemented the property faithfully, because Unicode excludes them on the grounds that they
    // affect the rendering of a FOLLOWING sequence — which a reason consisting only of them does
    // not have.
    assert!(
        holes.is_empty(),
        "every one of these renders as nothing and must be refused as a reason: {holes:?}"
    );

    // The same hole on a resume reason, which is the one a halted search shows a human.
    for (strike, victim) in victims.iter().enumerate() {
        reject_workflow_run(&state, victim, &format!("strike {strike}")).expect("reject");
    }
    assert!(search(&state, KEY.0, KEY.1, KEY.2).halted);
    let invisible_resume = char::from_u32(0xE0100).unwrap().to_string();
    let resumed = resume_workflow_search(
        &state,
        &repo_id(KEY.0),
        KEY.1,
        KEY.2,
        &invisible_resume,
        true,
    );
    // FIXED. This was the measured CONSEQUENCE rather than the defect — a halt lifted on a reason
    // that renders as nothing — and it is the half that made the invisible-character work worth
    // doing at all.
    let error = resumed.expect_err("a halt must not be liftable on an invisible reason");
    assert!(error.contains("must contain visible text"), "got: {error}");
    assert!(
        search(&state, KEY.0, KEY.1, KEY.2).halted,
        "and the halt stands"
    );
    // A VISIBLE reason still lifts it, so the guard discriminates rather than refusing everything.
    resume_workflow_search(
        &state,
        &repo_id(KEY.0),
        KEY.1,
        KEY.2,
        "the gate was misconfigured for all three",
        true,
    )
    .expect("a visible reason lifts the halt");
    assert!(!search(&state, KEY.0, KEY.1, KEY.2).halted);

    // Non-vacuous twice over: the nine from round 3 are now CAUGHT, and visible text still passes.
    for fixed in [0x2800_u32, 0x3164, 0xFFA0, 0x115F, 0x1160, 0x17B4, 0x17B5, 0xFE00, 0xFE0F] {
        let reason = char::from_u32(fixed).unwrap().to_string();
        assert!(
            reject_workflow_run(&state, &run.id, &reason).is_err(),
            "round 3's batch must stay closed: U+{fixed:04X}"
        );
    }
    reject_workflow_run(&state, &run.id, "a real reason").expect("visible text still passes");
}

// =================================================================================================
// PRIORITY 3 — my claim against Dev A's fact. Dev A is right.
// =================================================================================================

/// **I was wrong and Dev A is right.** My round-3 report said renaming a remote orphans a champion
/// "inherent to keying on the remote". It is not inherent to that at all: the identity is built
/// from the remote's URL, so a rename that leaves the URL alone changes nothing.
///
/// What my round-3 test actually demonstrated was the consequence of the `origin`-only LOOKUP —
/// `upstream` → `origin` moves the identity because the pre-rename state fell back to the path, not
/// because of the rename. Dev A's correction is exactly right, and the report's framing was not.
///
/// Measured here across all four cases so the distinction is pinned rather than argued, including
/// the one thing that IS inherent: changing the remote's URL moves the identity, which is what
/// happens when a repo is moved, mirrored or its org is renamed.
#[test]
fn qa_a4_renaming_a_remote_does_not_move_the_identity_but_changing_its_url_does() {
    const URL: &str = "git@github.com:owner/repo.git";
    const EXPECTED: &str = "git:github.com/owner/repo";

    // (a) origin renamed away and a new origin added at the SAME url: unchanged. This is the case
    //     my report got wrong.
    let a = tmp_dir("qa-r4-rename-a");
    init_repo(&a);
    git(&a, &["remote", "add", "origin", URL]);
    let before = repo_identity(&a.to_string_lossy()).expect("before");
    assert_eq!(before, EXPECTED);
    git(&a, &["remote", "rename", "origin", "legacy"]);
    git(&a, &["remote", "add", "origin", URL]);
    assert_eq!(
        repo_identity(&a.to_string_lossy()).expect("after"),
        before,
        "a rename that leaves the URL in place must not move the identity"
    );

    // (b) `git remote rename origin upstream` with nothing added back. My round-3 report predicted
    //     this falls back to the path — which was true THEN, and is the measurement my wrong claim
    //     rested on. It is no longer: `git_remote_url` now prefers `origin` and otherwise takes the
    //     first remote alphabetically, so the round-3 finding about `origin`-only lookups is closed
    //     and the identity does not move at all. Dev A is right, and more completely than they
    //     claimed: a rename is invisible to the key in BOTH directions.
    let b = tmp_dir("qa-r4-rename-b");
    init_repo(&b);
    git(&b, &["remote", "add", "origin", URL]);
    assert_eq!(repo_identity(&b.to_string_lossy()).expect("b before"), EXPECTED);
    git(&b, &["remote", "rename", "origin", "upstream"]);
    assert_eq!(
        repo_identity(&b.to_string_lossy()).expect("b after"),
        EXPECTED,
        "the lookup finds `upstream`, so the identity is untouched by the rename"
    );
    git(&b, &["remote", "rename", "upstream", "origin"]);
    assert_eq!(
        repo_identity(&b.to_string_lossy()).expect("b restored"),
        EXPECTED,
        "and back again — nothing about the key is derived from the remote's NAME"
    );

    // (b2) The round-3 finding itself, confirmed closed: a repo whose ONLY remote is `upstream`
    //      gets a real git identity, and two worktrees of it therefore share a champion.
    let b2root = tmp_dir("qa-r4-upstream-only");
    let main = b2root.join("main");
    init_repo(&main);
    git(&main, &["remote", "add", "upstream", URL]);
    assert_eq!(
        repo_identity(&main.to_string_lossy()).expect("upstream only"),
        EXPECTED,
        "a repo with no `origin` is no longer keyed on its path"
    );
    let linked = b2root.join("linked");
    git(&main, &["worktree", "add", "-q", linked.to_str().unwrap(), "-b", "side"]);
    assert_eq!(
        repo_identity(&linked.to_string_lossy()).expect("worktree"),
        repo_identity(&main.to_string_lossy()).expect("main"),
        "two worktreees of an origin-less repo now share one champion"
    );

    // (c) the URL changed: the identity moves. INHERENT to keying on the URL, and the real version
    //     of the concern my report mis-stated. A repo moved to a mirror, or an org renamed, gets a
    //     new identity and its champion is orphaned.
    let c = tmp_dir("qa-r4-rename-c");
    init_repo(&c);
    git(&c, &["remote", "add", "origin", URL]);
    assert_eq!(repo_identity(&c.to_string_lossy()).expect("c before"), EXPECTED);
    git(&c, &["remote", "set-url", "origin", "git@git.internal:owner/repo.git"]);
    assert_eq!(
        repo_identity(&c.to_string_lossy()).expect("c after"),
        "git:git.internal/owner/repo",
        "changing the URL moves the identity — inherent, and the one real orphaning case"
    );

    // (d) a cosmetic re-spelling of the SAME url does not move it, which is what the normalisation
    //     is for and is the reason (c) is narrow rather than constant.
    let d = tmp_dir("qa-r4-rename-d");
    init_repo(&d);
    git(&d, &["remote", "add", "origin", URL]);
    for respelling in [
        "https://github.com/owner/repo.git",
        "https://GitHub.com/Owner/Repo",
        "ssh://git@github.com/owner/repo",
    ] {
        git(&d, &["remote", "set-url", "origin", respelling]);
        assert_eq!(
            repo_identity(&d.to_string_lossy()).expect("respelled"),
            EXPECTED,
            "{respelling} is the same repo"
        );
    }
}

// =================================================================================================
// PRIORITY 4 — `sanitise_collected`'s encoding
// =================================================================================================

/// The advisory count never costs the score, on every path that can discard it — and a legitimate
/// `0` is still distinguishable from a discarded count.
///
/// `0` is the design's "this gate discriminates nothing" signal and must survive; `None` is "no
/// count the host could trust". The two must not collapse into each other, in either direction.
#[tokio::test]
async fn qa_a4_a_discarded_count_never_costs_the_score_on_any_path() {
    let (state, _root) = test_state();
    let workspace = tmp_dir("qa-r4-collected");
    let w = workflow(&state, KEY.0, KEY.1, KEY.2);
    let plan = seed_plan(&state, &workspace, Some(r#"["true"]"#));

    let bad_counts = [Some(-1_i64), Some(MAX_GATE_COLLECTED + 1), Some(i64::MAX), Some(i64::MIN)];

    // (1) the direct recorder.
    for bad in bad_counts {
        let run = start_against(&state, &w.id, None);
        let scored = record_workflow_run_gate(&state, &run.id, 0, bad)
            .expect("an unusable count must never refuse the result");
        assert_eq!(scored.gate_exit, Some(0), "the score survives {bad:?}");
        assert_eq!(scored.gate_collected, None, "and the count is dropped");
        // Persisted that way, not just returned.
        assert_eq!(get_workflow_run(&state, &run.id).unwrap().gate_collected, None);
    }

    // (2) the explicit re-score recorder.
    let run = start_against(&state, &w.id, None);
    record_workflow_run_gate(&state, &run.id, 0, Some(12)).expect("first");
    let rescored = rescore_workflow_run_gate(&state, &run.id, 5, Some(i64::MAX))
        .expect("a re-score with an unusable count keeps the new exit code");
    assert_eq!(rescored.gate_exit, Some(5), "the re-scored exit code lands");
    assert_eq!(
        rescored.gate_collected, None,
        "and the unusable count replaces the old good one with None, not with 12"
    );

    // (3) and (4) the two EXECUTOR entry points. The gate itself produces the count, so a bad one
    //     cannot be injected — what matters is that a real run still records a real score through
    //     the same `sanitise_collected` path.
    let exec_run = start_against(&state, &w.id, Some(&plan));
    let scored = score_workflow_run(&state, &exec_run.id, None)
        .await
        .expect("executor");
    assert_eq!(scored.gate_exit, Some(0));
    let re = rescore_workflow_run(&state, &exec_run.id, None)
        .await
        .expect("executor re-score");
    assert_eq!(re.gate_exit, Some(0));

    // A legitimate ZERO is NOT confused with a discarded count, in either direction.
    let zero_run = start_against(&state, &w.id, None);
    let zero = record_workflow_run_gate(&state, &zero_run.id, 0, Some(0)).expect("zero");
    assert_eq!(
        zero.gate_collected,
        Some(0),
        "0 collected is the design's 'this gate discriminates nothing' signal and must survive"
    );
    let json = serde_json::to_value(&zero).expect("serialise");
    assert_eq!(json["gateCollected"].as_i64(), Some(0), "and on the wire: {json}");
    let none_run = start_against(&state, &w.id, None);
    let none = record_workflow_run_gate(&state, &none_run.id, 0, None).expect("none");
    assert!(
        serde_json::to_value(&none).expect("serialise")["gateCollected"].is_null(),
        "and an absent count stays null, never 0"
    );
    // The boundary itself is trusted, so the discard is not off by one.
    let bound_run = start_against(&state, &w.id, None);
    assert_eq!(
        record_workflow_run_gate(&state, &bound_run.id, 0, Some(MAX_GATE_COLLECTED))
            .expect("boundary")
            .gate_collected,
        Some(MAX_GATE_COLLECTED)
    );
}

/// **The one cost of the chosen encoding, stated rather than hidden.** `None` now carries three
/// distinct meanings in the ROW — "the gate printed nothing a parser recognised", "a negative count
/// was discarded", and "an over-bound count was discarded" — and nothing persisted distinguishes
/// them. The `warn!` carries `run_id`, the offending value and the max, so the *log* can identify
/// the run; the row cannot.
///
/// I think this is the right trade and I am not asking for it to change: the alternative the
/// coordinator asked for (clamping) stores a precise number nobody measured, which is the
/// quiet-wrong-value failure one field over, and the field is advisory with nothing branching on
/// it. But `gate_collected`'s stated purpose is *"to let a human notice a suite that collects
/// nothing"*, and a human now sees `null` where a discarded 10^18 was — so the one thing this
/// encoding cannot do is the thing the column was added for, in that one case. Pinned so the
/// ambiguity is a known property rather than a surprise.
#[test]
fn qa_a4_a_discarded_count_is_indistinguishable_from_an_unparseable_one_in_the_row() {
    let (state, _root) = test_state();
    let w = workflow(&state, KEY.0, KEY.1, KEY.2);

    let unparseable = start_against(&state, &w.id, None);
    let discarded_negative = start_against(&state, &w.id, None);
    let discarded_huge = start_against(&state, &w.id, None);

    let a = record_workflow_run_gate(&state, &unparseable.id, 0, None).expect("a");
    let b = record_workflow_run_gate(&state, &discarded_negative.id, 0, Some(-5)).expect("b");
    let c = record_workflow_run_gate(&state, &discarded_huge.id, 0, Some(i64::MAX)).expect("c");

    assert_eq!(
        (a.gate_collected, b.gate_collected, c.gate_collected),
        (None, None, None),
        "DEFECT PIN: three different causes, one indistinguishable stored value. The warning \
         carries the detail; the row does not. Accepted as the right trade — recorded so it is not \
         rediscovered as a bug."
    );
    // All three kept their score, which is the property that actually matters.
    for run in [&a, &b, &c] {
        assert_eq!(run.gate_exit, Some(0));
    }
}

// =================================================================================================
// PRIORITY 1 — the discard, completed. Their inversion claims more than it checks.
// =================================================================================================

/// The halt fix discards the result **completely**: no exit code, and no `ended_at` either.
///
/// Dev A's inversion of my TOCTOU test asserts `gate_exit` is `None` under the message *"no partial
/// score, no ended_at"* — but it never checks `ended_at`. The claim happens to be true, because the
/// refusal returns before the single UPDATE and the transaction rolls back, so both columns move
/// together or not at all. Checking it is what makes that an assertion rather than a belief: a
/// future edit that wrote `ended_at` first, to record the attempt, would keep their test green.
#[tokio::test]
async fn qa_a4_a_discarded_score_leaves_no_ended_at_either() {
    let (state, _root) = test_state();
    let workspace = tmp_dir("qa-r4-discard");
    let w = workflow(&state, KEY.0, KEY.1, KEY.2);
    let plan = seed_plan(&state, &workspace, Some(r#"["sleep 1"]"#));
    let run = start_against(&state, &w.id, Some(&plan));
    assert_eq!(run.ended_at, None, "a fresh run has not ended");

    let victims: Vec<String> = (0..3).map(|_| start_against(&state, &w.id, None).id).collect();
    let scoring = score_workflow_run(&state, &run.id, None);
    let rejecting = async {
        tokio::time::sleep(std::time::Duration::from_millis(250)).await;
        for (index, victim) in victims.iter().enumerate() {
            reject_workflow_run(&state, victim, &format!("mid-gate {index}")).expect("reject");
        }
    };
    let (scored, ()) = tokio::join!(scoring, rejecting);
    assert!(scored.is_err(), "the score must be discarded");

    let after = get_workflow_run(&state, &run.id).expect("run");
    assert_eq!(after.gate_exit, None, "no exit code");
    assert_eq!(after.gate_collected, None, "no collected count");
    assert_eq!(
        after.ended_at, None,
        "and no ended_at: the whole UPDATE is rolled back, so the run is indistinguishable from \
         one that never started a gate"
    );
    assert_eq!(
        after.started_at, run.started_at,
        "nothing else moved either"
    );
    // No decision row was written for the discarded attempt.
    let promotes: i64 = state
        .db
        .with_conn(|conn| {
            conn.query_row(
                "SELECT COUNT(*) FROM workflow_decisions WHERE kind <> 'reject'",
                [],
                |row| row.get(0),
            )
            .map_err(|e| e.to_string())
        })
        .expect("count");
    assert_eq!(promotes, 0, "and the decision log records only the three rejections");
}

// =================================================================================================
// PRIORITY 6 — the lease contract, re-confirmed against feat/arena-b @ 341b7f4
// =================================================================================================

/// **The contract, re-confirmed at Dev B's current tip, and the mismatch has got WORSE in one
/// respect: Dev B now prescribes the losing order in their module header.**
///
/// Re-read at `feat/arena-b` `341b7f4` (575 lines added to `leases.rs` since `eadd1fd`):
///
/// * `LeasePool::busy()`, `fleet_busy_json()` and `SeatLease::seats_held_json()` are **byte-
///   identical** to what they were. Still per-family TOTALS, still no self-exclusion, still no
///   `busy_excluding` variant anywhere in the module, still idle families present at `0`.
/// * **Zero production call sites** of either snapshot function on that branch — ten in its tests,
///   none outside them. So the wiring, and therefore the ordering decision, lands entirely on this
///   side, in `measure_run_budget`.
/// * Dev B's module header now states the intended usage explicitly, and it is acquire-then-
///   snapshot: *"It will `acquire` `fan_out` seats on the workflow's family, snapshot
///   `fleet_busy_json` and `SeatLease::seats_held_json` into the `workflow_runs` row's `fleet_busy`
///   / `seats_held` columns at start."* `start_workflow_run_with_budget`'s own doc on this branch
///   says the same thing. **Both branches independently prescribe the order that makes the feature
///   inert.**
///
/// The fix is not "move the snapshot", because the two columns cannot be taken at the same moment:
/// `seats_held_json` is a method on the LEASE and so only exists after `acquire`, while
/// `fleet_busy` must be read BEFORE it. Snapshot `fleet_busy` first, acquire second, take
/// `seats_held` from the returned lease.
///
/// Driven here on the exact literals Dev B's own tests assert, so this is measurement rather than
/// reading.
#[test]
fn qa_a4_the_lease_contract_still_needs_snapshot_before_acquire() {
    // Verbatim from feat/arena-b @ 341b7f4, `the_snapshot_columns_are_json_a_run_can_store`.
    const BEFORE_ACQUIRE: &str =
        r#"{"deepseek-v4.1-flash":0,"glm-5.3-flash":0,"qwen3.8-flash-next":0}"#;
    const AFTER_ACQUIRE: &str =
        r#"{"deepseek-v4.1-flash":0,"glm-5.3-flash":2,"qwen3.8-flash-next":0}"#;
    const SEATS_HELD: &str = r#"{"glm-5.3-flash":2}"#;

    // Snapshot-before-acquire IS the contract: all-zero normalises to empty, which is "quiet".
    assert!(
        parse_seat_map(BEFORE_ACQUIRE).expect("parses").is_empty(),
        "an all-zero snapshot must normalise to the empty map"
    );
    let quiet = run_with(SEATS_HELD, BEFORE_ACQUIRE);
    assert!(
        run_is_scoreable(&quiet).is_none(),
        "snapshot-before-acquire is SCOREABLE — the contract holds in that order"
    );

    // Snapshot-after-acquire — the order BOTH module docs prescribe — is not.
    let self_included = run_with(SEATS_HELD, AFTER_ACQUIRE);
    let block = run_is_scoreable(&self_included)
        .expect("the run's own seats read as foreign contention");
    assert!(
        block.contains("2 foreign seats were held"),
        "and the refusal blames the run for its own lease: {block}"
    );

    // Two runs of differing width — the normal case, since fan_out is what the arena varies — are
    // not rescued by the identity escape either, so nothing can be compared at all.
    let wider = r#"{"deepseek-v4.1-flash":0,"glm-5.3-flash":4,"qwen3.8-flash-next":0}"#;
    assert!(
        !contention_was_identical(AFTER_ACQUIRE, wider),
        "a 2-seat and a 4-seat run are not comparable under the wrong order"
    );
    // Identical widths survive by accident, which is exactly why a shallow smoke test would pass.
    assert!(contention_was_identical(AFTER_ACQUIRE, AFTER_ACQUIRE));

    // And `seats_held` is unaffected by the ordering, which is why only one of the two columns has
    // to move: the lease describes itself correctly whenever it is asked.
    assert_eq!(
        parse_seat_map(SEATS_HELD).expect("parses").get("glm-5.3-flash"),
        Some(&2)
    );
}

fn run_with(seats_held: &str, fleet_busy: &str) -> WorkflowRun {
    WorkflowRun {
        id: "r".to_string(),
        workflow_id: "w".to_string(),
        plan_id: None,
        seed: 0,
        gate_exit: Some(0),
        gate_collected: None,
        seats_held: seats_held.to_string(),
        fleet_busy: fleet_busy.to_string(),
        started_at: "now".to_string(),
        ended_at: None,
        scoreable: true,
        not_scoreable_reason: None,
    }
}

// =================================================================================================
// 20af2aa — the contract test that now exists, and the citation sweep turned into a guard
// =================================================================================================

/// **DEFECT (documented, not fixed). A convention mismatch is NOT distinguishable from real
/// contention by the refusal alone — and the host holds both halves of the comparison.**
///
/// `the_fleet_busy_contract_is_foreign_contention_only` pins the right four cases and has real
/// teeth (I reverted `scoreability` to always-quiet and `parse_seat_map` to keep zero entries; each
/// turns it red at its own assertion). But its case 4 asserts, in a comment, that an inclusive-
/// totals mismatch and genuine foreign load *"look the same here on purpose, because the host
/// cannot tell them apart and must refuse either way"*.
///
/// The second half of that is right — refuse either way. The first half is not. The host has
/// `seats_held` and `fleet_busy` in the SAME ROW, and `fleet_busy == seats_held` exactly is the
/// signature an inclusive-totals producer emits on an idle fleet. It cannot be conclusive on one
/// run (a genuinely contended run could coincidentally see foreign load equal to its own seats),
/// but across a fleet it is: if EVERY run carries that signature, the arena is inert for one
/// reason and not the other.
///
/// This matters because of who reads it. The person debugging an arena that crowns nothing has the
/// `not_scoreable_reason` string and little else, and both causes hand them the identical sentence
/// with only the numbers differing. One clause — "this equals this run's own seatsHeld, which is
/// what an inclusive-totals lease pool emits; see the fleet_busy contract" — is the difference
/// between a five-minute diagnosis and an afternoon.
#[test]
fn qa_a4_a_convention_mismatch_reads_differently_from_real_contention() {
    let (state, _root) = test_state();
    let w = workflow(&state, KEY.0, KEY.1, KEY.2);
    let seats = r#"{"glm":3}"#;

    let reason_for = |fleet: &str| -> String {
        let run = start_workflow_run_with_budget(
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
        run.not_scoreable_reason.expect("an unscoreable run says why")
    };

    // FIXED. The two columns are in the same ROW, so the host CAN tell the two causes apart, and
    // the refusal now says which it is looking at.
    //
    // NOTE ON THIS TEST, from Dev A: as written, its two cases were the SAME INPUT. `mismatch` was
    // `reason_for(seats)` and `real` was `reason_for(r#"{"glm":3}"#)` — and `seats` IS
    // `{"glm":3}`, so the equality it asserted was trivially true and could not have failed for
    // any implementation. The genuinely different case is a foreign load that is NOT the run's own
    // budget, which the test already computed below as `other_family`; it is now the comparison.
    //
    // The two causes have completely different fixes — one is "swap two lines at integration", the
    // other is "wait for a quiet fleet" — which is why one shared sentence was expensive.
    let mismatch = reason_for(seats);
    let other_family = reason_for(r#"{"qwen":3}"#);

    // Both still say the fleet was not quiet, with the magnitude: that part was never the problem.
    for reason in [&mismatch, &other_family] {
        assert!(reason.contains("the fleet was not quiet when it started"));
        assert!(reason.contains("3 foreign seats were held"));
    }
    // And they are no longer the same sentence.
    assert_ne!(
        mismatch, other_family,
        "the inclusive-totals signature and real foreign load must not read identically"
    );
    assert!(
        mismatch.contains("seatsHeld"),
        "the refusal must name the run's own budget — the one fact that separates the two causes, \
         sitting in the same row: {mismatch}"
    );
    assert!(
        mismatch.contains("inclusive-totals"),
        "and must name the convention it looks like: {mismatch}"
    );
    assert!(
        mismatch.contains("not conclusive"),
        "while saying it is not conclusive on one run: {mismatch}"
    );
    assert!(
        !other_family.contains("inclusive-totals"),
        "and real contention must NOT be reported as a convention mismatch: {other_family}"
    );
}

/// **The citation sweep as a GUARD rather than a one-off — and it finds two Dev A's own sweep
/// missed.**
///
/// This round's lesson was a doc comment that cited a test by name for three rounds with nothing
/// behind it. Dev A then swept mechanically for the same shape and reported finding two stale
/// citations of `contention_is_comparable`, renamed when the escape rule was dropped. Three
/// survive in the tree. One is legitimate — `workflows_qa_attacks.rs` names it to say explicitly
/// that it no longer exists — and two are stale, including a bracketed intra-doc link in
/// `measure_run_budget`'s own documentation.
///
/// A sweep run once is what produced the original problem, so it is a test now. The allowlist is
/// the honest part: each entry names something real that this crate does not define, and adding to
/// it is a deliberate act rather than an accident.
#[test]
fn qa_a4_no_doc_comment_cites_a_name_this_crate_does_not_define() {
    // Sources that carry the arena's documentation.
    let sources: [(&str, &str); 5] = [
        ("services/workflows.rs", include_str!("workflows.rs")),
        (
            "services/workflows_qa_attacks.rs",
            include_str!("workflows_qa_attacks.rs"),
        ),
        (
            "services/workflows_qa_attacks_r2.rs",
            include_str!("workflows_qa_attacks_r2.rs"),
        ),
        (
            "services/workflows_qa_attacks_r3.rs",
            include_str!("workflows_qa_attacks_r3.rs"),
        ),
        (
            "services/workflows_qa_attacks_r4.rs",
            include_str!("workflows_qa_attacks_r4.rs"),
        ),
    ];
    let defined: String = sources.iter().map(|(_, text)| *text).collect();

    // Names legitimately cited that this crate does not define. Each is a real thing somewhere
    // else, and the one that matters is `busy_excluding`: it is named precisely BECAUSE it does not
    // exist on `feat/arena-b`, which is the whole point of the sentence it appears in.
    const ALLOWED: &[&str] = &[
        "busy_excluding",          // deliberately absent on feat/arena-b — the contract's premise
        "fleet_busy_json",         // feat/arena-b: LeasePool
        "seats_held_json",         // feat/arena-b: SeatLease
        "spawn_blocking",          // tokio
        "unchecked_transaction",   // rusqlite
        "run_plan_test_gate",      // services::agent_plans
        "effective_gate",          // services::agent_plans
        "effective_test_commands", // services::agent_plans
        "workflow_decisions",      // a SQL table
        "workflow_runs",           // a SQL table
        "workflow_prefs",          // a SQL table
        "agent_plan_phases",       // a SQL table
        "agent_plans",             // a SQL table
        "workflows_one_champion",  // a SQL index
        "test_commands",           // a SQL column
        "lease_pool",              // AppState field, feat/arena-b
    ];

    // Only BRACKETED intra-doc citations: `[`name`]` is a claim that the name resolves, which is
    // what rustdoc checks and what the missing test's citation looked like. Backticked prose is
    // deliberately out of scope — it is allowed to name columns, tables and other branches.
    let mut dangling: Vec<String> = Vec::new();
    for (file, text) in sources {
        for (index, line) in text.lines().enumerate() {
            let trimmed = line.trim_start();
            if !(trimmed.starts_with("///") || trimmed.starts_with("//!")) {
                continue;
            }
            let mut rest = trimmed;
            while let Some(open) = rest.find("[`") {
                let after = &rest[open + 2..];
                let Some(close) = after.find("`]") else { break };
                let cited = &after[..close];
                rest = &after[close + 2..];
                let name = cited.rsplit("::").next().unwrap_or(cited);
                let name = name.trim_end_matches("()");
                // Only snake_case function-shaped names; types and modules resolve differently.
                if name.is_empty()
                    || !name.contains('_')
                    || !name.chars().all(|c| c.is_ascii_lowercase() || c == '_' || c.is_ascii_digit())
                    || ALLOWED.contains(&name)
                {
                    continue;
                }
                if !defined.contains(&format!("fn {}", name)) {
                    dangling.push(format!("{file}:{} cites [`{name}`]", index + 1));
                }
            }
        }
    }

    // FIXED: the list is EMPTY. The known one — `measure_run_budget`'s bracketed link to the
    // renamed comparability helper — is gone, and so is a second, stale plain-prose copy of the
    // same name that this sweep's `[`…`]`-only matching could not see.
    //
    // Two gates now cover this shape from different directions: this sweep, and
    // `RUSTDOCFLAGS="-D rustdoc::broken_intra_doc_links" cargo doc`, which catches the bracketed
    // form for the whole crate including files no sweep reads. Running it for the first time found
    // exactly one more, in `agent_plans.rs`, where the link target is `#[cfg(test)]` and therefore
    // genuinely unresolvable from non-test docs.
    assert!(
        dangling.is_empty(),
        "a doc comment cites a name nothing defines — a citation that reads as a guarantee with \
         nothing behind it: {dangling:?}"
    );

    // Non-vacuous: the sweep really does resolve names that exist, and really would catch a fake.
    //
    // The needles are ASSEMBLED with `format!` rather than written as literals, and that is not
    // fussiness. This file is one of the five the sweep reads, so any literal here of the form
    // "fn" + "_" + a stale name makes that name count as DEFINED and the sweep goes quiet. A first
    // draft wrote the needle inline and reported nothing; a second draft explained the mistake in
    // a comment that spelled the needle out again and reported nothing for the same reason. Both
    // were caught only because this assertion expects a specific non-empty list. Prose below says
    // what the stale name is without ever placing it next to the definition keyword.
    assert!(
        defined.contains(&format!("fn {}", "scoreability")),
        "the corpus is readable"
    );
    assert!(
        !defined.contains(&format!("fn {}", "contention_is_comparable")),
        "the name in the dangling list really is undefined"
    );
}
