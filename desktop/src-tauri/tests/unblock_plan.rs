//! A plan must be able to leave `blocked`.
//!
//! THE DEFECT THIS PINS, measured twice.
//!
//! On 2026-10-23 the kart planner reported `READY_FOR_T2_PLAN_REVIEW` at 03:21
//! and the plan then sat at `blocked` for **2 hours 19 minutes** with nothing
//! running, until a watchdog fired on a timeout rather than on progress. On
//! 2026-10-24 it happened again: `agent_blocked` at 02:00:39, and the plan is
//! still there.
//!
//! Of the twenty plan mutations, **none clears it**.
//! `updateAgentPlanBlocked(id, reason)` only sets it, and `reason` is non-null.
//! `clear_plan_blocked_conn` exists but needs the status to restore, which only
//! the caller that blocked the plan holds — in memory. The host restarted at
//! 00:06 on 2026-10-24, so even that is gone, and the plan is stranded in a
//! state no API can leave. `retryAgentReviewer` is the workaround I have been
//! using: it ignores plan status entirely and calls the gate directly.
//!
//! `start_planning_run_inner` returns early and silently on `blocked`
//! (`agent_plans.rs:1598`), so a blocked plan cannot be restarted either. The
//! two facts together mean every honest `BLOCKED` from a reviewer parks the run
//! until a human runs a workaround — which is why this moved onto the critical
//! path rather than staying a tidy-up item.
//!
//! WHAT THE RESTORE TARGET IS. The status the plan held before the block is
//! already on the record: the `agent_blocked` event carries
//! `{"statusBefore":"planning_planner_running"}`, which is how I read the live
//! plan's history after the restart. The events table survives a restart; the
//! caller's local variable does not. So the recorded transition is the source
//! of truth, and `u4` pins what happens when there isn't one.
//!
//! WHY `u6` IS THE TEST THAT MATTERS. The lesson of the last four defects on
//! this branch is that proving a layer says nothing about the layer past it. A
//! mutation that flips the status column and leaves the plan unrunnable has
//! fixed nothing a human would notice — `start_planning_run_inner` would still
//! return early, just on a different status. So `u6` does not assert on the
//! column. It calls the real start path afterwards and requires the plan to
//! actually move.

use johnnyone_desktop_lib::db::Database;
use johnnyone_desktop_lib::services::agent_plans;
use johnnyone_desktop_lib::state::app_state::AppState;

/// A plan seeded straight into `blocked`, with the `agent_blocked` event that a
/// real block leaves behind. `status_before` of `None` seeds the event WITHOUT
/// a recorded transition — the `block_plan` path, which emits
/// `agent_plan_blocked {"reason":…}` and no status at all.
fn seed_blocked(tag: &str, status_before: Option<&str>) -> (AppState, String) {
    let root = std::env::temp_dir().join(format!("unblock-{tag}-{}", std::process::id()));
    std::fs::create_dir_all(&root).expect("temp root");
    let db = Database::open(&root.join("test.db")).expect("open db");
    let state = AppState::new(db);
    let plan_id = format!("unblock-{tag}");
    let workspace = root.join("ws");
    std::fs::create_dir_all(workspace.join("plan/phases/00-x")).expect("plan dirs");
    std::fs::write(workspace.join("plan/overview.md"), "# Plan\n").expect("overview");

    let pid = plan_id.clone();
    let ws = workspace.to_string_lossy().to_string();
    let pp = workspace.join("plan").to_string_lossy().to_string();
    let before = status_before.map(|s| s.to_string());
    state
        .db
        .with_conn(move |conn| {
            conn.execute(
                "INSERT INTO agent_plans (id, run_type, title, workspace_path, plan_path, status,
                                          worker_session_id, reviewer_session_id, worker_provider,
                                          reviewer_provider, current_phase_index, initiative_id,
                                          initiative_status, health, error)
                 VALUES (?1, 'planning', 'the stranded plan', ?2, ?3, 'blocked',
                         NULL, NULL, 'claude', 'claude', 0, ?1, 'planning', 'needs-attention',
                         'the reviewer returned BLOCKED')",
                rusqlite::params![pid, ws, pp],
            )
            .map_err(|e| e.to_string())?;
            let (etype, payload) = match before.as_deref() {
                Some(s) => (
                    "agent_blocked",
                    format!(r#"{{"statusBefore":"{s}"}}"#),
                ),
                None => (
                    "agent_plan_blocked",
                    r#"{"reason":"blocked by user"}"#.to_string(),
                ),
            };
            conn.execute(
                "INSERT INTO agent_plan_events (id, plan_id, phase_id, type, payload_json, created_at)
                 VALUES (?1, ?2, NULL, ?3, ?4, datetime('now'))",
                rusqlite::params![format!("{pid}-ev"), pid, etype, payload],
            )
            .map_err(|e| e.to_string())
        })
        .expect("seed a blocked plan");

    (state, plan_id)
}

/// u1 — a blocked plan comes back to the status it had before the block.
///
/// The recorded transition is the source of truth because it survives a restart.
#[tokio::test]
async fn u1_unblock_restores_the_recorded_previous_status() {
    let (state, id) = seed_blocked("restore", Some("planning_planner_running"));

    let run = agent_plans::unblock_plan(&state, id.clone())
        .await
        .expect("unblocking a blocked plan must succeed");

    assert_eq!(
        run.plan.status, "planning_planner_running",
        "the plan did not come back to the status its own agent_blocked event \
         recorded. That event is the only copy that survives a host restart, \
         and the host restarted at 00:06 on 2026-10-24 while a plan was blocked."
    );
    assert!(
        run.plan.error.is_none(),
        "the block reason is still on the plan after unblocking, so the console \
         will go on showing why it WAS blocked: {:?}",
        run.plan.error
    );
    assert_ne!(
        run.plan.health, "needs-attention",
        "the health badge still says needs-attention on a plan that is no \
         longer blocked"
    );
}

/// u2 — unblocking records the transition, both ends of it.
///
/// A status that changes with no event leaves the console's Events column
/// showing a block that was never lifted, which is exactly the gap that made
/// the 2h19m stall unreadable.
#[tokio::test]
async fn u2_unblock_appends_an_event_naming_both_statuses() {
    let (state, id) = seed_blocked("event", Some("planning_review_running"));

    agent_plans::unblock_plan(&state, id.clone())
        .await
        .expect("unblock");

    let after = agent_plans::get_plan(&state, &id).expect("reload");
    let ev = after
        .events
        .iter()
        .find(|e| e.event_type == "agent_unblocked")
        .unwrap_or_else(|| {
            panic!(
                "no agent_unblocked event. Events seen: {:?}",
                after.events.iter().map(|e| &e.event_type).collect::<Vec<_>>()
            )
        });
    assert!(
        ev.payload_json.contains("blocked"),
        "the unblock event does not name the status it left: {}",
        ev.payload_json
    );
    assert!(
        ev.payload_json.contains("planning_review_running"),
        "the unblock event does not name the status it restored, so the \
         timeline cannot say WHERE the run resumed: {}",
        ev.payload_json
    );
}

/// u3 — a plan that is not blocked is REFUSED, and the refusal says what it is.
///
/// Silence here is the dangerous answer: an unblock that quietly succeeds on an
/// `approved` plan would reopen finished work, and one that quietly succeeds on
/// a `stopped` plan would restart a run a human deliberately killed.
#[tokio::test]
async fn u3_unblock_refuses_a_plan_that_is_not_blocked() {
    let (state, id) = seed_blocked("notblocked", Some("planning_planner_running"));
    agent_plans::unblock_plan(&state, id.clone())
        .await
        .expect("the first unblock succeeds");

    let err = agent_plans::unblock_plan(&state, id.clone())
        .await
        .expect_err("a second unblock must be refused, not silently applied");

    assert!(
        err.to_lowercase().contains("planning_planner_running")
            || err.to_lowercase().contains("not blocked"),
        "the refusal does not say what the plan's status actually is, so the \
         caller cannot tell a refusal from a failure: {err}"
    );

    let after = agent_plans::get_plan(&state, &id).expect("reload");
    assert_eq!(
        after.plan.status, "planning_planner_running",
        "the refused second call changed the status anyway"
    );
}

/// u4 — with no recorded transition, the fallback is NAMED, not guessed silently.
///
/// `block_plan` emits `agent_plan_blocked {"reason":…}` and records no status at
/// all, so this is a real case and not a hypothetical. A fallback that fires
/// silently is the defect behind "a default that fires on unparseable input":
/// the caller cannot tell a restored status from an invented one.
#[tokio::test]
async fn u4_without_a_recorded_status_the_fallback_is_named_in_the_event() {
    let (state, id) = seed_blocked("fallback", None);

    let run = agent_plans::unblock_plan(&state, id.clone())
        .await
        .expect("a plan blocked without a recorded transition must still escape");

    assert_ne!(
        run.plan.status, "blocked",
        "the plan is still blocked, so a plan blocked through block_plan can \
         never leave — and that is the mutation the console actually calls"
    );

    let after = agent_plans::get_plan(&state, &id).expect("reload");
    let ev = after
        .events
        .iter()
        .find(|e| e.event_type == "agent_unblocked")
        .expect("agent_unblocked event");
    assert!(
        ev.payload_json.contains("default") || ev.payload_json.contains("fallback"),
        "the status was chosen by fallback and the event does not say so. A \
         restored status and an invented one must not look identical in the \
         timeline: {}",
        ev.payload_json
    );
}

/// u5 — a recorded status that is itself a dead end is not restored.
///
/// `statusBefore` is whatever the plan held, and that can be `stopped`,
/// `closed`, `approved` or even `blocked` again. Restoring one of those puts
/// the plan straight back into a state it cannot leave, which is the bug with
/// extra steps.
#[tokio::test]
async fn u5_a_terminal_recorded_status_is_not_restored() {
    for dead in ["blocked", "stopped", "closed", "approved"] {
        let (state, id) = seed_blocked(&format!("dead-{dead}"), Some(dead));
        let run = agent_plans::unblock_plan(&state, id.clone())
            .await
            .unwrap_or_else(|e| panic!("unblock with statusBefore={dead:?} failed: {e}"));
        assert!(
            !matches!(
                run.plan.status.as_str(),
                "blocked" | "stopped" | "closed" | "approved"
            ),
            "statusBefore was {dead:?} and the plan was restored straight into \
             {:?}, a status it cannot leave either",
            run.plan.status
        );
    }
}

/// u6 — THE TEST THAT MATTERS: the plan must actually RUN afterwards.
///
/// Not an assertion about the status column. `start_planning_run_inner` returns
/// early and silently on `blocked` (`agent_plans.rs:1598`), and a mutation that
/// moves the plan to some other status it also refuses has fixed nothing a
/// human would notice. The last four defects on this branch were all of exactly
/// that shape: the module proved, the layer past it never consulted.
///
/// So: unblock, then call the real start path, and require the plan to move.
#[tokio::test]
async fn u6_an_unblocked_plan_is_actually_runnable() {
    let (state, id) = seed_blocked("runnable", Some("planning_planner_running"));

    agent_plans::unblock_plan(&state, id.clone())
        .await
        .expect("unblock");

    let before = agent_plans::get_plan(&state, &id).expect("reload").plan.status;
    // The start path may fail for reasons that have nothing to do with being
    // blocked (no provider CLI on this box, no tmux). What it must NOT do is
    // return the plan untouched, which is the silent early return.
    let _ = agent_plans::start_planning_run(state.clone(), id.clone()).await;
    let after = agent_plans::get_plan(&state, &id).expect("reload");

    assert_ne!(
        after.plan.status, "blocked",
        "starting the plan put it straight back to blocked"
    );
    assert!(
        after.plan.status != before
            || !after
                .events
                .iter()
                .filter(|e| e.event_type != "agent_unblocked")
                .collect::<Vec<_>>()
                .is_empty(),
        "the start path neither moved the status nor recorded a single event, \
         which is the silent early return at agent_plans.rs:1598 happening on a \
         different status. Status stayed {before:?}."
    );
}
