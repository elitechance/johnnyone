//! Orphan sweep: reap `johnnyone_<uuid>` tmux sessions that no running job owns.
//!
//! Three layers, top to bottom:
//! 1. **Pure decision** ([`select_orphan_sessions`]): a tmux inventory + DB facts in, verdicts out.
//!    No I/O, no clock, no engine state — every rule is pinned by a fixture test.
//! 2. **Thin tmux adapter** ([`TmuxInventory`] / [`LiveTmux`]) over `terminal::list_tmux_inventory`
//!    and the guarded `terminal::kill_owned_orphan_pane`.
//! 3. **Executor** ([`run_orphan_sweep_with`]): load facts, decide, re-validate each verdict at act
//!    time, then retire row-backed sessions through `agent_plans::retire_agent_session_with` (which
//!    ARCHIVES the row, so `ensure_terminal_session` can never resurrect it) or kill a rowless pane.
//!    [`spawn_orphan_sweep`] runs it at engine start and every 10 minutes.
//!
//! Safety guards (master plan §9): only exact `johnnyone_<canonical uuid>` names are ever
//! considered; a name stored by an `attached_tmux = 1` row, a row flagged `attached_tmux`, and a
//! tmux session with a human client attached are always kept; an unknown plan status counts as
//! running.

use std::collections::{HashMap, HashSet};

use crate::terminal::owned_session_id_from_tmux_name;

// ── 1. Pure decision ────────────────────────────────────────────────────────────────────────

/// A rowless owned pane younger than this is kept: it may belong to a session another engine or a
/// test process is creating right now.
pub const ORPHAN_GRACE_SECS: i64 = 600;

/// One tmux session as `list-sessions` reports it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct LiveTmuxSession {
    pub name: String,
    pub created_unix: i64,
    pub attached_clients: u32,
}

/// The `sessions` row behind an owned tmux name.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SessionFacts {
    pub session_id: String,
    pub archived: bool,
    pub attached_tmux: bool,
    pub created_unix: i64,
}

/// One plan that references a session: its worker/reviewer column, or an ephemeral
/// `agent_plan_review_sessions` row (`ephemeral = true`).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PlanLink {
    pub plan_id: String,
    pub plan_status: String,
    /// `planner` / `worker` / `reviewer` for a column link; the registered role for an ephemeral one.
    pub role: String,
    pub ephemeral: bool,
    /// Whether this link's job has ended (master §4), computed by the executor ([`link_job_ended`]).
    pub job_ended: bool,
}

/// Everything the decision reads from the DB.
#[derive(Debug, Clone, Default)]
pub struct SweepFacts {
    /// By session id.
    pub sessions: HashMap<String, SessionFacts>,
    /// By session id: worker/reviewer columns + review_sessions rows.
    pub links: HashMap<String, Vec<PlanLink>>,
    /// `tmux_session_name` of every `attached_tmux = 1` row.
    pub attached_tmux_names: HashSet<String>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum OrphanReason {
    /// Owned name, no session row at all, older than the grace window.
    NoSessionRow,
    /// The row is archived but its pane is still alive.
    SessionArchived,
    /// Every plan that references the session is terminal and its job there has ended.
    PlanTerminal,
    /// An ephemeral lens/docs session of a running plan, created before this engine booted —
    /// its in-process waiter died with the previous engine, so nothing will read its report.
    StaleEphemeral,
}

impl OrphanReason {
    pub fn as_str(&self) -> &'static str {
        match self {
            OrphanReason::NoSessionRow => "no_session_row",
            OrphanReason::SessionArchived => "session_archived",
            OrphanReason::PlanTerminal => "plan_terminal",
            OrphanReason::StaleEphemeral => "stale_ephemeral",
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct OrphanVerdict {
    pub tmux_name: String,
    pub session_id: String,
    pub reason: OrphanReason,
    /// `(plan_id, role)` of the link that justified the verdict, when there is one.
    pub owner: Option<(String, String)>,
}

/// `approved | done | stopped | closed`. Anything else — including a status this code has never
/// seen — is running and its sessions are protected (D-05: a missed kill costs memory, a wrong kill
/// costs a run).
pub fn is_terminal_plan_status(s: &str) -> bool {
    matches!(s, "approved" | "done" | "stopped" | "closed")
}

/// Decide which owned tmux sessions to retire, and why. Arms, in order (phase-01 flowchart):
///
/// 1. the name is not exactly `johnnyone_<uuid>` → keep (foreign: `j1`, `kord`, …);
/// 2. the name is stored by an `attached_tmux = 1` row → keep (the user's tmux);
/// 3. a tmux client is attached → keep (a human is looking at it);
/// 4. no session row → select `NoSessionRow` once older than [`ORPHAN_GRACE_SECS`], else keep;
/// 5. the row itself is flagged `attached_tmux` → keep;
/// 6. the row is archived → select `SessionArchived`;
/// 7. active, with no plan links → keep (a standalone `/shells` session);
/// 8. any link is still LIVE (its plan is running, or its job has not ended — e.g. a paused
///    single-phase worker, or the docs-commit agent of a plan that just completed) → select
///    `StaleEphemeral` only if every live link is an ephemeral review/docs link and the row
///    predates this engine's boot (its in-process waiter died with the previous engine); else keep;
/// 9. no live link (every plan terminal, every job ended) → select `PlanTerminal`.
///
/// Output is sorted by tmux name.
pub fn select_orphan_sessions(
    live: &[LiveTmuxSession],
    facts: &SweepFacts,
    now_unix: i64,
    engine_started_unix: i64,
) -> Vec<OrphanVerdict> {
    let mut verdicts = Vec::new();
    for pane in live {
        let Some(session_id) = owned_session_id_from_tmux_name(&pane.name) else {
            continue;
        };
        if facts.attached_tmux_names.contains(&pane.name) || pane.attached_clients > 0 {
            continue;
        }
        let verdict = |reason: OrphanReason, owner: Option<&PlanLink>| OrphanVerdict {
            tmux_name: pane.name.clone(),
            session_id: session_id.clone(),
            reason,
            owner: owner.map(|l| (l.plan_id.clone(), l.role.clone())),
        };
        let links: &[PlanLink] = facts.links.get(&session_id).map(Vec::as_slice).unwrap_or(&[]);
        let Some(row) = facts.sessions.get(&session_id) else {
            if now_unix - pane.created_unix > ORPHAN_GRACE_SECS {
                verdicts.push(verdict(OrphanReason::NoSessionRow, None));
            }
            continue;
        };
        if row.attached_tmux {
            continue;
        }
        if row.archived {
            verdicts.push(verdict(OrphanReason::SessionArchived, links.first()));
            continue;
        }
        if links.is_empty() {
            continue;
        }
        let live_links: Vec<&PlanLink> = links
            .iter()
            .filter(|l| !is_terminal_plan_status(&l.plan_status) || !l.job_ended)
            .collect();
        if !live_links.is_empty() {
            let all_ephemeral = live_links.iter().all(|l| l.ephemeral);
            if all_ephemeral && row.created_unix < engine_started_unix {
                verdicts.push(verdict(OrphanReason::StaleEphemeral, live_links.first().copied()));
            }
            continue;
        }
        verdicts.push(verdict(OrphanReason::PlanTerminal, links.first()));
    }
    verdicts.sort_by(|a, b| a.tmux_name.cmp(&b.tmux_name));
    verdicts
}

/// Master §4 `job_ended` table for one link:
/// - plan `stopped`/`closed` → ended; plan running (non-terminal) → not ended;
/// - plan `approved`/`done`: a reviewer column → ended; the planner (worker column of a `planning`
///   run) → ended; an EPHEMERAL link → NOT ended (it is still registered, so its job — e.g. the
///   docs-commit agent that runs after `agent_plan_completed` already set `approved` — is in
///   flight; once done it is disposed and unregistered. A pre-boot one is reaped by the
///   stale-ephemeral rule instead); the dev worker → ended only when `all_phases_passed`, which the
///   executor computes as "every phase row passed (≥1 row) AND no ephemeral agent is still
///   registered for the plan", so the worker outlives docs-commit (brief: retire AFTER docs-commit).
pub fn link_job_ended(
    run_type: &str,
    plan_status: &str,
    role: &str,
    ephemeral: bool,
    all_phases_passed: bool,
) -> bool {
    match plan_status {
        "stopped" | "closed" => true,
        "approved" | "done" => {
            if ephemeral {
                false
            } else if role == "reviewer" || run_type == "planning" {
                true
            } else {
                all_phases_passed
            }
        }
        _ => false,
    }
}

// ── 2. Thin tmux adapter ────────────────────────────────────────────────────────────────────

/// Boxed future returned by [`TmuxInventory`] (the same idiom as `agent_plans::AgentPanes`).
pub type TmuxFuture<'a, T> =
    std::pin::Pin<Box<dyn std::future::Future<Output = Result<T, String>> + Send + 'a>>;

/// The two tmux operations the sweep needs. Tests use a fake; production uses [`LiveTmux`].
pub trait TmuxInventory: Send + Sync {
    fn list(&self) -> TmuxFuture<'_, Vec<LiveTmuxSession>>;
    /// Kill an owned pane that has no session row. Implementations must refuse foreign names.
    fn kill_orphan<'a>(&'a self, name: &'a str) -> TmuxFuture<'a, bool>;
}

/// The real tmux server.
pub struct LiveTmux;

impl TmuxInventory for LiveTmux {
    fn list(&self) -> TmuxFuture<'_, Vec<LiveTmuxSession>> {
        Box::pin(async {
            Ok(crate::terminal::list_tmux_inventory()
                .await?
                .into_iter()
                .map(|(name, created_unix, attached_clients)| LiveTmuxSession {
                    name,
                    created_unix,
                    attached_clients,
                })
                .collect())
        })
    }

    fn kill_orphan<'a>(&'a self, name: &'a str) -> TmuxFuture<'a, bool> {
        Box::pin(crate::terminal::kill_owned_orphan_pane(name))
    }
}

// ── 3. Executor ─────────────────────────────────────────────────────────────────────────────

use crate::services::agent_plans::{retire_agent_session_with, AgentPanes, LivePanes};
use crate::state::app_state::AppState;
use rusqlite::{params_from_iter, OptionalExtension};

/// What one sweep did. The executor also logs it as a single `"orphan sweep"` line.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct SweepReport {
    pub listed: usize,
    pub owned: usize,
    pub selected: usize,
    pub revalidated_kept: usize,
    pub retired: usize,
    pub killed_rowless: usize,
    pub errors: Vec<String>,
}

/// Test seam: `before_act` runs right before a verdict is re-validated (to simulate a plan that
/// changes between "decide" and "act"). Production passes `SweepHooks::default()`.
#[derive(Default)]
pub(crate) struct SweepHooks<'a> {
    pub before_act: Option<&'a (dyn Fn(&OrphanVerdict) + Send + Sync)>,
}

const ID_CHUNK: usize = 500;

fn placeholders(n: usize) -> String {
    vec!["?"; n].join(",")
}

/// Load the [`SweepFacts`] for these session ids (chunked at 500 like `demote_terminal_rows`).
fn load_sweep_facts(state: &AppState, ids: &[String]) -> Result<SweepFacts, String> {
    state.db.with_conn(|conn| {
        let mut facts = SweepFacts::default();
        let mut stmt = conn
            .prepare(
                "SELECT tmux_session_name FROM sessions \
                 WHERE attached_tmux = 1 AND tmux_session_name IS NOT NULL",
            )
            .map_err(|e| e.to_string())?;
        facts.attached_tmux_names = stmt
            .query_map([], |r| r.get::<_, String>(0))
            .map_err(|e| e.to_string())?
            .filter_map(|r| r.ok())
            .collect();

        for chunk in ids.chunks(ID_CHUNK) {
            let ph = placeholders(chunk.len());
            let sql = format!(
                "SELECT id, status, attached_tmux, CAST(strftime('%s', created_at) AS INTEGER) \
                 FROM sessions WHERE id IN ({ph})"
            );
            let mut stmt = conn.prepare(&sql).map_err(|e| e.to_string())?;
            let rows = stmt
                .query_map(params_from_iter(chunk.iter()), |r| {
                    Ok(SessionFacts {
                        session_id: r.get(0)?,
                        archived: r.get::<_, String>(1)? != "active",
                        attached_tmux: r.get::<_, i64>(2)? != 0,
                        created_unix: r.get::<_, Option<i64>>(3)?.unwrap_or(0),
                    })
                })
                .map_err(|e| e.to_string())?;
            for row in rows {
                let row = row.map_err(|e| e.to_string())?;
                facts.sessions.insert(row.session_id.clone(), row);
            }

            // Column links (worker / reviewer).
            let sql = format!(
                "SELECT id, status, run_type, worker_session_id, reviewer_session_id FROM agent_plans \
                 WHERE worker_session_id IN ({ph}) OR reviewer_session_id IN ({ph})"
            );
            let mut stmt = conn.prepare(&sql).map_err(|e| e.to_string())?;
            let plans = stmt
                .query_map(params_from_iter(chunk.iter().chain(chunk.iter())), |r| {
                    Ok((
                        r.get::<_, String>(0)?,
                        r.get::<_, String>(1)?,
                        r.get::<_, String>(2)?,
                        r.get::<_, Option<String>>(3)?,
                        r.get::<_, Option<String>>(4)?,
                    ))
                })
                .map_err(|e| e.to_string())?
                .filter_map(|r| r.ok())
                .collect::<Vec<_>>();
            let wanted: HashSet<&String> = chunk.iter().collect();
            for (plan_id, status, run_type, worker, reviewer) in plans {
                if let Some(sid) = worker.filter(|s| wanted.contains(s)) {
                    let role = if run_type == "planning" { "planner" } else { "worker" };
                    let all_passed = if role == "worker" && matches!(status.as_str(), "approved" | "done") {
                        all_phases_passed(conn, &plan_id)?
                    } else {
                        false
                    };
                    let job_ended = link_job_ended(&run_type, &status, role, false, all_passed);
                    facts.links.entry(sid).or_default().push(PlanLink {
                        plan_id: plan_id.clone(),
                        plan_status: status.clone(),
                        role: role.to_string(),
                        ephemeral: false,
                        job_ended,
                    });
                }
                if let Some(sid) = reviewer.filter(|s| wanted.contains(s)) {
                    let job_ended = link_job_ended(&run_type, &status, "reviewer", false, false);
                    facts.links.entry(sid).or_default().push(PlanLink {
                        plan_id: plan_id.clone(),
                        plan_status: status.clone(),
                        role: "reviewer".to_string(),
                        ephemeral: false,
                        job_ended,
                    });
                }
            }

            // Ephemeral links (lens reviewers, docs agent).
            let sql = format!(
                "SELECT r.session_id, r.plan_id, r.role, p.status, p.run_type \
                 FROM agent_plan_review_sessions r JOIN agent_plans p ON p.id = r.plan_id \
                 WHERE r.session_id IN ({ph})"
            );
            let mut stmt = conn.prepare(&sql).map_err(|e| e.to_string())?;
            let rows = stmt
                .query_map(params_from_iter(chunk.iter()), |r| {
                    Ok((
                        r.get::<_, String>(0)?,
                        r.get::<_, String>(1)?,
                        r.get::<_, String>(2)?,
                        r.get::<_, String>(3)?,
                        r.get::<_, String>(4)?,
                    ))
                })
                .map_err(|e| e.to_string())?
                .filter_map(|r| r.ok())
                .collect::<Vec<_>>();
            for (sid, plan_id, role, status, run_type) in rows {
                let job_ended = link_job_ended(&run_type, &status, &role, true, false);
                facts.links.entry(sid).or_default().push(PlanLink {
                    plan_id,
                    plan_status: status,
                    role,
                    ephemeral: true,
                    job_ended,
                });
            }
        }
        Ok(facts)
    })
}

/// The dev worker's job has ended: every phase row of the plan has passed (and there is at least
/// one — zero rows = not ended), AND no ephemeral agent (the docs-commit agent) is still registered
/// for the plan, so the completion pipeline has finished.
fn all_phases_passed(conn: &rusqlite::Connection, plan_id: &str) -> Result<bool, String> {
    conn.query_row(
        "SELECT NOT EXISTS (SELECT 1 FROM agent_plan_phases WHERE plan_id = ?1 AND status != 'passed') \
         AND EXISTS (SELECT 1 FROM agent_plan_phases WHERE plan_id = ?1) \
         AND NOT EXISTS (SELECT 1 FROM agent_plan_review_sessions WHERE plan_id = ?1)",
        [plan_id],
        |r| r.get::<_, bool>(0),
    )
    .optional()
    .map(|v| v.unwrap_or(false))
    .map_err(|e| e.to_string())
}

/// One sweep: list → decide → re-validate → retire (row-backed) or kill (rowless) → report.
/// Every row-backed kill goes through `retire_agent_session_with`, which archives the row and
/// writes `agent_session_retired` when a plan owns it. One failure never aborts the rest.
pub(crate) async fn run_orphan_sweep_with(
    state: &AppState,
    tmux: &dyn TmuxInventory,
    panes: &dyn AgentPanes,
    now_unix: i64,
    engine_started_unix: i64,
    hooks: &SweepHooks<'_>,
) -> SweepReport {
    let mut report = SweepReport::default();
    let live = match tmux.list().await {
        Ok(live) => live,
        Err(error) => {
            tracing::warn!(%error, "orphan sweep: tmux could not be asked; nothing reaped");
            return report;
        }
    };
    report.listed = live.len();
    let owned: Vec<LiveTmuxSession> = live
        .into_iter()
        .filter(|s| owned_session_id_from_tmux_name(&s.name).is_some())
        .collect();
    report.owned = owned.len();
    if owned.is_empty() {
        log_summary(&report);
        return report;
    }
    let ids: Vec<String> = owned
        .iter()
        .filter_map(|s| owned_session_id_from_tmux_name(&s.name))
        .collect();
    let facts = match load_sweep_facts(state, &ids) {
        Ok(facts) => facts,
        Err(error) => {
            tracing::warn!(%error, "orphan sweep: could not load facts; nothing reaped");
            report.errors.push(error);
            log_summary(&report);
            return report;
        }
    };
    let verdicts = select_orphan_sessions(&owned, &facts, now_unix, engine_started_unix);
    report.selected = verdicts.len();

    for verdict in verdicts {
        if let Some(hook) = hooks.before_act {
            hook(&verdict);
        }
        // Re-validate at act time (master §9 "Sweep TOCTOU", D-12): re-load this one session's
        // facts and re-decide. A plan that started running since the snapshot keeps its session.
        // Re-list tmux too: a human may have attached a client since the first listing.
        let pane: Vec<LiveTmuxSession> = match tmux.list().await {
            Ok(now_live) => now_live.into_iter().filter(|s| s.name == verdict.tmux_name).collect(),
            Err(error) => {
                report.errors.push(format!("{}: {}", verdict.tmux_name, error));
                continue;
            }
        };
        let fresh = match load_sweep_facts(state, std::slice::from_ref(&verdict.session_id)) {
            Ok(fresh) => fresh,
            Err(error) => {
                report.errors.push(format!("{}: {}", verdict.session_id, error));
                continue;
            }
        };
        let still = select_orphan_sessions(&pane, &fresh, now_unix, engine_started_unix);
        let confirmed = still
            .iter()
            .any(|v| v.session_id == verdict.session_id && v.reason == verdict.reason);
        if !confirmed {
            tracing::info!(session_id = %verdict.session_id, "orphan sweep: revalidated: kept");
            report.revalidated_kept += 1;
            continue;
        }

        let reason = format!("orphan_sweep:{}", verdict.reason.as_str());
        if fresh.sessions.contains_key(&verdict.session_id) {
            match retire_agent_session_with(state, panes, &verdict.session_id, &reason).await {
                Ok(true) => report.retired += 1,
                Ok(false) => {}
                Err(error) => report.errors.push(format!("{}: {}", verdict.session_id, error)),
            }
        } else {
            match tmux.kill_orphan(&verdict.tmux_name).await {
                Ok(_) => {
                    report.killed_rowless += 1;
                    tracing::info!(
                        session_id = %verdict.session_id,
                        tmux_name = %verdict.tmux_name,
                        reason = %reason,
                        "agent_session_retired"
                    );
                }
                Err(error) => report.errors.push(format!("{}: {}", verdict.tmux_name, error)),
            }
        }
    }
    log_summary(&report);
    report
}

fn log_summary(report: &SweepReport) {
    tracing::info!(
        listed = report.listed,
        owned = report.owned,
        selected = report.selected,
        revalidated_kept = report.revalidated_kept,
        retired = report.retired,
        killed_rowless = report.killed_rowless,
        errors = report.errors.len(),
        "orphan sweep"
    );
}

/// One live sweep against the real tmux and pane killer.
pub async fn run_orphan_sweep(state: &AppState, engine_started_unix: i64) -> SweepReport {
    let now_unix = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_secs() as i64)
        .unwrap_or(0);
    run_orphan_sweep_with(
        state,
        &LiveTmux,
        &LivePanes,
        now_unix,
        engine_started_unix,
        &SweepHooks::default(),
    )
    .await
}

// ── 4. Scheduling ───────────────────────────────────────────────────────────────────────────

/// The brief's "every ~10 min".
pub const ORPHAN_SWEEP_INTERVAL_SECS: u64 = 600;
/// Let `resume_active_plan_loops` re-attach running workers before the boot pass (log hygiene,
/// not correctness — the boot instant is captured before this delay).
pub const ORPHAN_SWEEP_FIRST_DELAY_SECS: u64 = 15;

/// Start the sweep: one boot pass (the interval's immediate first tick, after a short delay), then
/// every [`ORPHAN_SWEEP_INTERVAL_SECS`]. `engine_started_unix` must be captured at the top of
/// `setup`, before any coordinator loop resumes, so a lens spawned after boot is never mistaken
/// for a pre-boot leftover.
pub fn spawn_orphan_sweep(state: AppState, engine_started_unix: i64) {
    // `tauri::async_runtime::spawn`, NOT a bare tokio spawn: this is called from Tauri's `setup`,
    // which has no tokio reactor entered (see `session_watchdog::spawn_session_watchdog`).
    tauri::async_runtime::spawn(async move {
        tokio::time::sleep(std::time::Duration::from_secs(ORPHAN_SWEEP_FIRST_DELAY_SECS)).await;
        tracing::info!(interval_secs = ORPHAN_SWEEP_INTERVAL_SECS, "orphan sweep loop starting");
        let mut ticker =
            tokio::time::interval(std::time::Duration::from_secs(ORPHAN_SWEEP_INTERVAL_SECS));
        loop {
            ticker.tick().await;
            let _ = run_orphan_sweep(&state, engine_started_unix).await;
        }
    });
}

// ── Tests ───────────────────────────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;

    const NOW: i64 = 1_800_000_000;
    const BOOT: i64 = NOW - 300;

    fn uid() -> String {
        uuid::Uuid::new_v4().to_string()
    }

    fn pane(id: &str, age: i64) -> LiveTmuxSession {
        LiveTmuxSession { name: format!("johnnyone_{id}"), created_unix: NOW - age, attached_clients: 0 }
    }

    /// Plan agent in tmux (D1, 2026-11-23): a live plan agent's `johnnyone_<sid>` pane is kept
    /// (active row, no worker/reviewer link); once the agent is retired (archived) or its row is
    /// gone, the pane is swept like any other owned orphan.
    #[tokio::test]
    async fn plan_agent_pane_kept_while_live_swept_once_retired() {
        use crate::services::plan_mcp::fixture::{plan, use_store};
        let (state, _r) = crate::test_support::test_state();
        let store = use_store(&state);
        let p = plan(&state, &store, "A").await;
        let live = vec![LiveTmuxSession { name: format!("johnnyone_{}", p.session), created_unix: NOW - 3600, attached_clients: 0 }];
        let facts = load_sweep_facts(&state, &[p.session.clone()]).unwrap();
        assert!(select_orphan_sessions(&live, &facts, NOW, BOOT).is_empty(), "a live plan agent's pane is kept");
        crate::services::sessions::mark_session_archived(&state, &p.session).unwrap();
        let facts = load_sweep_facts(&state, &[p.session.clone()]).unwrap();
        let v = select_orphan_sessions(&live, &facts, NOW, BOOT);
        assert_eq!(v.len(), 1);
        assert_eq!(v[0].reason, OrphanReason::SessionArchived);
        assert_eq!(v[0].session_id, p.session);
    }

    fn row(id: &str, archived: bool) -> SessionFacts {
        SessionFacts { session_id: id.into(), archived, attached_tmux: false, created_unix: NOW - 3600 }
    }

    fn link(plan: &str, status: &str, role: &str, ephemeral: bool, job_ended: bool) -> PlanLink {
        PlanLink {
            plan_id: plan.into(),
            plan_status: status.into(),
            role: role.into(),
            ephemeral,
            job_ended,
        }
    }

    fn facts_with(id: &str, archived: bool, links: Vec<PlanLink>) -> SweepFacts {
        let mut f = SweepFacts::default();
        f.sessions.insert(id.into(), row(id, archived));
        if !links.is_empty() {
            f.links.insert(id.into(), links);
        }
        f
    }

    // The five brief cases first.

    #[test]
    fn foreign_name_is_never_selected() {
        let id = uuid::Uuid::new_v4();
        let names = [
            "j1".to_string(),
            "kord".to_string(),
            "guard".to_string(),
            "johnnyone_".to_string(),
            "johnnyone_abc".to_string(),
            format!("johnnyone_{id}-x"),
            format!("JOHNNYONE_{id}"),
        ];
        let live: Vec<_> = names
            .iter()
            .map(|n| LiveTmuxSession { name: n.clone(), created_unix: NOW - 86_400, attached_clients: 0 })
            .collect();
        assert!(select_orphan_sessions(&live, &SweepFacts::default(), NOW, BOOT).is_empty());
    }

    #[test]
    fn attached_tmux_session_is_never_selected() {
        // (a) a row flagged attached_tmux, named like ours, on a finished plan.
        let a = uid();
        let mut f = facts_with(&a, false, vec![link("P", "done", "worker", false, true)]);
        f.sessions.get_mut(&a).unwrap().attached_tmux = true;
        assert!(select_orphan_sessions(&[pane(&a, 9999)], &f, NOW, BOOT).is_empty());
        // (b) the live name is stored by an attached_tmux=1 row.
        let b = uid();
        let mut f = facts_with(&b, true, vec![]);
        f.attached_tmux_names.insert(format!("johnnyone_{b}"));
        assert!(select_orphan_sessions(&[pane(&b, 9999)], &f, NOW, BOOT).is_empty());
        // (c) a human client is attached, no row.
        let c = uid();
        let mut p = pane(&c, 9999);
        p.attached_clients = 1;
        assert!(select_orphan_sessions(&[p], &SweepFacts::default(), NOW, BOOT).is_empty());
    }

    #[test]
    fn active_worker_of_running_plan_is_never_selected() {
        for status in [
            "draft",
            "planning_planner_running",
            "planning_review_running",
            "phase_worker_running",
            "phase_review_running",
            "phase_replan_running",
            "needs_attention",
            "blocked",
            "some_future_status",
        ] {
            let id = uid();
            let f = facts_with(&id, false, vec![link("P", status, "worker", false, false)]);
            assert!(select_orphan_sessions(&[pane(&id, 9999)], &f, NOW, BOOT).is_empty(), "{status}");
        }
    }

    #[test]
    fn finished_planner_and_worker_of_terminal_plan_are_selected() {
        for status in ["approved", "done", "stopped", "closed"] {
            for role in ["planner", "worker"] {
                let id = uid();
                let f = facts_with(&id, false, vec![link("P", status, role, false, true)]);
                let v = select_orphan_sessions(&[pane(&id, 9999)], &f, NOW, BOOT);
                assert_eq!(v.len(), 1, "{status}/{role}");
                assert_eq!(v[0].reason, OrphanReason::PlanTerminal);
                assert_eq!(v[0].owner, Some(("P".to_string(), role.to_string())));
                assert_eq!(v[0].session_id, id);
            }
        }
    }

    #[test]
    fn unknown_owned_session_with_no_row_is_selected() {
        let id = uid();
        let v = select_orphan_sessions(&[pane(&id, 601)], &SweepFacts::default(), NOW, BOOT);
        assert_eq!(v.len(), 1);
        assert_eq!(v[0].reason, OrphanReason::NoSessionRow);
        assert_eq!(v[0].owner, None);
    }

    #[test]
    fn unknown_owned_session_inside_grace_is_kept() {
        let id = uid();
        assert!(select_orphan_sessions(&[pane(&id, 599)], &SweepFacts::default(), NOW, BOOT).is_empty());
    }

    #[test]
    fn archived_row_with_live_pane_is_selected() {
        let id = uid();
        let f = facts_with(&id, true, vec![]);
        let v = select_orphan_sessions(&[pane(&id, 10)], &f, NOW, BOOT);
        assert_eq!(v.len(), 1);
        assert_eq!(v[0].reason, OrphanReason::SessionArchived);
    }

    #[test]
    fn active_standalone_session_is_kept() {
        let id = uid();
        let f = facts_with(&id, false, vec![]);
        assert!(select_orphan_sessions(&[pane(&id, 99_999)], &f, NOW, BOOT).is_empty());
    }

    #[test]
    fn pre_boot_ephemeral_lens_of_running_plan_is_selected() {
        let id = uid();
        let f = facts_with(&id, false, vec![link("P", "phase_review_running", "qa", true, false)]);
        // row created an hour ago, engine booted 5 minutes ago.
        let v = select_orphan_sessions(&[pane(&id, 3600)], &f, NOW, BOOT);
        assert_eq!(v.len(), 1);
        assert_eq!(v[0].reason, OrphanReason::StaleEphemeral);
        assert_eq!(v[0].owner, Some(("P".to_string(), "qa".to_string())));
    }

    #[test]
    fn post_boot_ephemeral_lens_is_kept() {
        let id = uid();
        let mut f = facts_with(&id, false, vec![link("P", "phase_review_running", "qa", true, false)]);
        f.sessions.get_mut(&id).unwrap().created_unix = BOOT + 10;
        assert!(select_orphan_sessions(&[pane(&id, 60)], &f, NOW, BOOT).is_empty());
    }

    #[test]
    fn session_linked_to_running_and_terminal_plan_is_kept() {
        let id = uid();
        let f = facts_with(
            &id,
            false,
            vec![
                link("A", "approved", "planner", false, true),
                link("B", "phase_worker_running", "worker", false, false),
            ],
        );
        assert!(select_orphan_sessions(&[pane(&id, 9999)], &f, NOW, BOOT).is_empty());
    }

    #[test]
    fn paused_single_phase_worker_of_approved_plan_is_kept() {
        let id = uid();
        let f = facts_with(&id, false, vec![link("P", "approved", "worker", false, false)]);
        assert!(select_orphan_sessions(&[pane(&id, 9999)], &f, NOW, BOOT).is_empty());
    }

    #[test]
    fn worker_of_fully_completed_dev_plan_is_selected() {
        let id = uid();
        let f = facts_with(&id, false, vec![link("P", "approved", "worker", false, true)]);
        let v = select_orphan_sessions(&[pane(&id, 9999)], &f, NOW, BOOT);
        assert_eq!(v.len(), 1);
        assert_eq!(v[0].reason, OrphanReason::PlanTerminal);
    }

    #[test]
    fn replaced_old_worker_of_terminal_plan_is_selected() {
        // Archived on swap, no longer in any column → no links at all.
        let id = uid();
        let f = facts_with(&id, true, vec![]);
        let v = select_orphan_sessions(&[pane(&id, 9999)], &f, NOW, BOOT);
        assert_eq!(v.len(), 1);
        assert_eq!(v[0].reason, OrphanReason::SessionArchived);
        assert_eq!(v[0].owner, None);
    }

    #[test]
    fn docs_agent_of_just_completed_plan_is_kept() {
        let id = uid();
        let mut f = facts_with(&id, false, vec![link("P", "approved", "docs", true, false)]);
        f.sessions.get_mut(&id).unwrap().created_unix = BOOT + 10;
        assert!(select_orphan_sessions(&[pane(&id, 60)], &f, NOW, BOOT).is_empty());
    }

    #[test]
    fn pre_boot_docs_agent_of_completed_plan_is_stale() {
        let id = uid();
        let f = facts_with(&id, false, vec![link("P", "approved", "docs", true, false)]);
        let v = select_orphan_sessions(&[pane(&id, 3600)], &f, NOW, BOOT);
        assert_eq!(v.len(), 1);
        assert_eq!(v[0].reason, OrphanReason::StaleEphemeral);
        assert_eq!(v[0].owner, Some(("P".to_string(), "docs".to_string())));
    }

    #[test]
    fn output_is_sorted_by_tmux_name() {
        let mut ids: Vec<String> = (0..6).map(|_| uid()).collect();
        let live: Vec<_> = ids.iter().map(|id| pane(id, 9999)).collect();
        let v = select_orphan_sessions(&live, &SweepFacts::default(), NOW, BOOT);
        ids.sort();
        let got: Vec<String> = v.into_iter().map(|v| v.session_id).collect();
        assert_eq!(got, ids);
    }

    #[test]
    fn link_job_ended_table() {
        // (run_type, status, role, ephemeral, all_phases_passed) -> ended
        let rows = [
            ("development", "stopped", "worker", false, false, true),
            ("planning", "closed", "planner", false, false, true),
            ("development", "phase_worker_running", "worker", false, true, false),
            ("planning", "planning_review_running", "planner", false, false, false),
            ("development", "phase_review_running", "qa", true, false, false),
            ("development", "some_future_status", "worker", false, true, false),
            ("planning", "approved", "planner", false, false, true),
            ("development", "approved", "worker", false, true, true),
            ("development", "approved", "worker", false, false, false),
            ("development", "done", "worker", false, true, true),
            ("development", "done", "worker", false, false, false),
            ("development", "approved", "reviewer", false, false, true),
            ("development", "approved", "docs", true, false, false),
            ("development", "done", "qa", true, true, false),
            ("development", "stopped", "docs", true, false, true),
        ];
        for (rt, st, role, eph, all, want) in rows {
            assert_eq!(link_job_ended(rt, st, role, eph, all), want, "{rt}/{st}/{role}/{eph}/{all}");
        }
    }

    #[test]
    fn sweep_interval_is_ten_minutes() {
        assert_eq!(ORPHAN_SWEEP_INTERVAL_SECS, 600);
    }

    #[test]
    fn main_rs_captures_boot_time_before_resume_and_spawns_sweep_once() {
        let src = include_str!("../main.rs");
        assert_eq!(src.matches("spawn_orphan_sweep(").count(), 1, "exactly one spawn");
        let setup = src.find(".setup(").expect(".setup( present");
        let boot = src.find("let engine_started_unix").expect("boot instant captured");
        let resume = src.find("resume_active_plan_loops").expect("resume present");
        let spawn = src.find("spawn_orphan_sweep(").unwrap();
        assert!(setup < boot, "boot instant captured inside setup");
        assert!(boot < resume, "boot instant captured before resume_active_plan_loops");
        assert!(setup < spawn, "spawn is inside setup");
    }
}

#[cfg(test)]
mod executor_tests {
    use super::*;
    use crate::db::Database;
    use crate::services::agent_plans::session_lifecycle_tests::FakePanes;
    use crate::services::agent_plans::register_review_session;
    use rusqlite::params;
    use std::sync::Mutex;

    const NOW: i64 = 1_900_000_000; // far after any created_at the test DB writes
    const BOOT: i64 = NOW - 60;

    struct FakeTmux {
        sessions: Vec<LiveTmuxSession>,
        killed: Mutex<Vec<String>>,
        list_err: bool,
        fail_kill: Option<String>,
        /// After the first listing, report every session as having a client attached (a human
        /// attached between "decide" and "act").
        attach_after_first_list: bool,
        lists: Mutex<u32>,
    }

    impl FakeTmux {
        fn new(sessions: Vec<LiveTmuxSession>) -> Self {
            Self {
                sessions,
                killed: Mutex::new(Vec::new()),
                list_err: false,
                fail_kill: None,
                attach_after_first_list: false,
                lists: Mutex::new(0),
            }
        }
        fn killed(&self) -> Vec<String> {
            self.killed.lock().unwrap().clone()
        }
    }

    impl TmuxInventory for FakeTmux {
        fn list(&self) -> TmuxFuture<'_, Vec<LiveTmuxSession>> {
            let n = {
                let mut lists = self.lists.lock().unwrap();
                *lists += 1;
                *lists
            };
            let mut sessions = self.sessions.clone();
            if self.attach_after_first_list && n > 1 {
                for s in &mut sessions {
                    s.attached_clients = 1;
                }
            }
            let out = if self.list_err { Err("no server".to_string()) } else { Ok(sessions) };
            Box::pin(async move { out })
        }
        fn kill_orphan<'a>(&'a self, name: &'a str) -> TmuxFuture<'a, bool> {
            self.killed.lock().unwrap().push(name.to_string());
            let fail = self.fail_kill.as_deref() == Some(name);
            Box::pin(async move { if fail { Err("kill failed".to_string()) } else { Ok(true) } })
        }
    }

    fn state() -> (AppState, std::path::PathBuf) {
        let root = crate::test_support::tmp_dir("reaper");
        let db = Database::open(&root.join("test.db")).expect("open db");
        (AppState::new(db), root)
    }

    fn live(id: &str) -> LiveTmuxSession {
        LiveTmuxSession { name: format!("johnnyone_{id}"), created_unix: NOW - 3600, attached_clients: 0 }
    }

    fn session(state: &AppState) -> String {
        crate::services::sessions::create_session(
            state,
            crate::db::models::CreateSessionInput {
                provider: Some("shell".into()),
                model: None,
                working_directory: None,
                title: Some("t".into()),
                kind: Some("agent".into()),
                setup_commands: None,
                tmux_session_name: None,
            },
        )
        .unwrap()
        .id
    }

    fn plan(state: &AppState, run_type: &str, status: &str, worker: Option<&str>) -> String {
        let id = uuid::Uuid::new_v4().to_string();
        state
            .db
            .with_conn(|conn| {
                conn.execute(
                    "INSERT INTO agent_plans (id, run_type, title, workspace_path, plan_path, status, worker_session_id, \
                     worker_provider, reviewer_provider, initiative_id, initiative_status) \
                     VALUES (?1, ?2, 't', '/w', '/p', ?3, ?4, 'claude_code', 'claude_code', ?1, 'development')",
                    params![id, run_type, status, worker],
                )
                .map_err(|e| e.to_string())
            })
            .unwrap();
        id
    }

    fn phase(state: &AppState, plan_id: &str, idx: i64, status: &str) {
        state
            .db
            .with_conn(|conn| {
                conn.execute(
                    "INSERT INTO agent_plan_phases (id, plan_id, phase_id, phase_title, phase_index, status) \
                     VALUES (?1, ?2, ?3, 'x', ?4, ?5)",
                    params![uuid::Uuid::new_v4().to_string(), plan_id, format!("0{idx}-p"), idx, status],
                )
                .map_err(|e| e.to_string())
            })
            .unwrap();
    }

    fn exec(state: &AppState, sql: &str, p: &[&dyn rusqlite::ToSql]) {
        state.db.with_conn(|conn| conn.execute(sql, p).map_err(|e| e.to_string())).unwrap();
    }

    fn status(state: &AppState, id: &str) -> Option<String> {
        state
            .db
            .with_conn(|conn| {
                conn.query_row("SELECT status FROM sessions WHERE id = ?1", params![id], |r| r.get(0))
                    .optional()
                    .map_err(|e| e.to_string())
            })
            .unwrap()
    }

    fn retired(state: &AppState, plan_id: &str) -> Vec<serde_json::Value> {
        state
            .db
            .with_conn(|conn| {
                let mut stmt = conn
                    .prepare(
                        "SELECT payload_json FROM agent_plan_events \
                         WHERE plan_id = ?1 AND type = 'agent_session_retired' ORDER BY rowid",
                    )
                    .map_err(|e| e.to_string())?;
                let rows = stmt
                    .query_map(params![plan_id], |r| r.get::<_, String>(0))
                    .map_err(|e| e.to_string())?
                    .filter_map(|r| r.ok())
                    .map(|p| serde_json::from_str(&p).unwrap())
                    .collect::<Vec<_>>();
                Ok(rows)
            })
            .unwrap()
    }

    fn snapshot(state: &AppState) -> (Vec<(String, String)>, i64) {
        state
            .db
            .with_conn(|conn| {
                let mut stmt = conn
                    .prepare("SELECT id, status FROM sessions ORDER BY id")
                    .map_err(|e| e.to_string())?;
                let rows = stmt
                    .query_map([], |r| Ok((r.get(0)?, r.get(1)?)))
                    .map_err(|e| e.to_string())?
                    .filter_map(|r| r.ok())
                    .collect::<Vec<_>>();
                let events: i64 = conn
                    .query_row("SELECT COUNT(*) FROM agent_plan_events", [], |r| r.get(0))
                    .map_err(|e| e.to_string())?;
                Ok((rows, events))
            })
            .unwrap()
    }

    async fn sweep(state: &AppState, tmux: &FakeTmux, panes: &FakePanes) -> SweepReport {
        run_orphan_sweep_with(state, tmux, panes, NOW, BOOT, &SweepHooks::default()).await
    }

    #[tokio::test]
    async fn sweep_retires_terminal_plan_worker_and_records_event() {
        let (state, root) = state();
        let w = session(&state);
        let p = plan(&state, "development", "approved", Some(&w));
        phase(&state, &p, 0, "passed");
        let tmux = FakeTmux::new(vec![live(&w)]);
        let panes = FakePanes::new(true);
        let r = sweep(&state, &tmux, &panes).await;
        assert_eq!(r.retired, 1);
        assert_eq!(status(&state, &w).as_deref(), Some("archived"));
        assert_eq!(panes.calls(), vec![w.clone()]);
        let ev = retired(&state, &p);
        assert_eq!(ev.len(), 1);
        assert_eq!(ev[0]["reason"], "orphan_sweep:plan_terminal");
        assert_eq!(ev[0]["role"], "worker");
        let _ = std::fs::remove_dir_all(&root);
    }

    #[tokio::test]
    async fn sweep_retires_finished_planner_of_approved_planning_run() {
        let (state, root) = state();
        let w = session(&state);
        let p = plan(&state, "planning", "approved", Some(&w));
        let tmux = FakeTmux::new(vec![live(&w)]);
        let r = sweep(&state, &tmux, &FakePanes::new(true)).await;
        assert_eq!(r.retired, 1);
        assert_eq!(status(&state, &w).as_deref(), Some("archived"));
        assert_eq!(retired(&state, &p)[0]["role"], "planner");
        let _ = std::fs::remove_dir_all(&root);
    }

    #[tokio::test]
    async fn sweep_kills_rowless_owned_pane_after_grace() {
        let (state, root) = state();
        let id = uuid::Uuid::new_v4().to_string();
        let tmux = FakeTmux::new(vec![live(&id)]);
        let panes = FakePanes::new(true);
        let before = snapshot(&state);
        let r = sweep(&state, &tmux, &panes).await;
        assert_eq!(r.killed_rowless, 1);
        assert_eq!(tmux.killed(), vec![format!("johnnyone_{id}")]);
        assert!(panes.calls().is_empty());
        assert_eq!(snapshot(&state), before, "no DB event for a rowless kill");
        let _ = std::fs::remove_dir_all(&root);
    }

    #[tokio::test]
    async fn sweep_never_touches_foreign_attached_or_running() {
        let (state, root) = state();
        // attached_tmux row on a finished plan
        let att = session(&state);
        exec(&state, "UPDATE sessions SET attached_tmux = 1, tmux_session_name = ?1 WHERE id = ?2",
             &[&format!("johnnyone_{att}"), &att]);
        plan(&state, "development", "done", Some(&att));
        // running worker
        let run = session(&state);
        plan(&state, "development", "phase_worker_running", Some(&run));
        // client-attached pane with no row
        let watched = uuid::Uuid::new_v4().to_string();
        let mut watched_pane = live(&watched);
        watched_pane.attached_clients = 1;
        let foreign = ["j1", "kord", "guard"].map(|n| LiveTmuxSession {
            name: n.into(),
            created_unix: NOW - 99_999,
            attached_clients: 0,
        });
        let mut all = foreign.to_vec();
        all.extend([live(&att), live(&run), watched_pane]);
        let tmux = FakeTmux::new(all);
        let panes = FakePanes::new(true);
        let before = snapshot(&state);
        let r = sweep(&state, &tmux, &panes).await;
        assert_eq!(r.selected, 0);
        assert!(tmux.killed().is_empty());
        assert!(panes.calls().is_empty());
        assert_eq!(snapshot(&state), before, "nothing archived, no events");
        let _ = std::fs::remove_dir_all(&root);
    }

    #[tokio::test]
    async fn sweep_reaps_pre_boot_lens_of_running_plan() {
        let (state, root) = state();
        let w = session(&state);
        let p = plan(&state, "development", "phase_review_running", Some(&w));
        let lens = session(&state);
        register_review_session(&state, &p, &lens, "qa").unwrap();
        // The test DB stamps created_at with the real clock, far before BOOT.
        let tmux = FakeTmux::new(vec![live(&w), live(&lens)]);
        let r = sweep(&state, &tmux, &FakePanes::new(true)).await;
        assert_eq!(r.retired, 1);
        assert_eq!(status(&state, &lens).as_deref(), Some("archived"));
        assert_eq!(status(&state, &w).as_deref(), Some("active"), "running worker kept");
        assert!(crate::services::agent_plans::plan_role_for_session(&state, &lens).is_err(), "unregistered");
        let ev = retired(&state, &p);
        assert_eq!(ev.len(), 1);
        assert_eq!(ev[0]["reason"], "orphan_sweep:stale_ephemeral");
        assert_eq!(ev[0]["role"], "qa");
        let _ = std::fs::remove_dir_all(&root);
    }

    #[tokio::test]
    async fn sweep_with_tmux_unavailable_changes_nothing() {
        let (state, root) = state();
        let w = session(&state);
        plan(&state, "development", "done", Some(&w));
        let mut tmux = FakeTmux::new(vec![live(&w)]);
        tmux.list_err = true;
        let panes = FakePanes::new(true);
        let before = snapshot(&state);
        let r = sweep(&state, &tmux, &panes).await;
        assert_eq!(r, SweepReport::default());
        assert_eq!(snapshot(&state), before);
        assert!(panes.calls().is_empty());
        let _ = std::fs::remove_dir_all(&root);
    }

    #[tokio::test]
    async fn sweep_is_idempotent() {
        let (state, root) = state();
        let w = session(&state);
        let p = plan(&state, "planning", "approved", Some(&w));
        let tmux = FakeTmux::new(vec![live(&w)]);
        assert_eq!(sweep(&state, &tmux, &FakePanes::new(true)).await.retired, 1);
        // The pane is gone now; the row is archived.
        let r2 = sweep(&state, &tmux, &FakePanes::new(false)).await;
        assert_eq!(r2.retired, 0);
        assert_eq!(retired(&state, &p).len(), 1, "no new event");
        let _ = std::fs::remove_dir_all(&root);
    }

    #[tokio::test]
    async fn sweep_continues_after_one_failure() {
        let (state, root) = state();
        let a = uuid::Uuid::new_v4().to_string();
        let b = uuid::Uuid::new_v4().to_string();
        let w = session(&state);
        plan(&state, "planning", "approved", Some(&w));
        let mut tmux = FakeTmux::new(vec![live(&a), live(&b), live(&w)]);
        tmux.fail_kill = Some(format!("johnnyone_{}", std::cmp::min(&a, &b)));
        let r = sweep(&state, &tmux, &FakePanes::new(true)).await;
        assert_eq!(r.selected, 3);
        assert_eq!(r.errors.len(), 1);
        assert_eq!(r.killed_rowless, 1);
        assert_eq!(r.retired, 1);
        assert_eq!(tmux.killed().len(), 2, "both rowless kills attempted");
        let _ = std::fs::remove_dir_all(&root);
    }

    #[tokio::test]
    async fn sweep_revalidates_before_acting() {
        let (state, root) = state();
        let w = session(&state);
        let p = plan(&state, "development", "approved", Some(&w));
        phase(&state, &p, 0, "passed");
        let tmux = FakeTmux::new(vec![live(&w)]);
        let panes = FakePanes::new(true);
        let hook_state = state.clone();
        let hook_plan = p.clone();
        let hook = move |_v: &OrphanVerdict| {
            // Run-from-phase reuses the still-active worker between decide and act.
            exec(&hook_state, "UPDATE agent_plans SET status = 'phase_worker_running' WHERE id = ?1", &[&hook_plan]);
        };
        let hooks = SweepHooks { before_act: Some(&hook) };
        let r = run_orphan_sweep_with(&state, &tmux, &panes, NOW, BOOT, &hooks).await;
        assert_eq!(r.selected, 1);
        assert_eq!(r.revalidated_kept, 1);
        assert_eq!(r.retired, 0);
        assert!(panes.calls().is_empty());
        assert_eq!(status(&state, &w).as_deref(), Some("active"));
        assert!(retired(&state, &p).is_empty());
        let _ = std::fs::remove_dir_all(&root);
    }

    /// Validator finding (phase 01): `agent_plan_completed` sets the plan `approved` BEFORE the
    /// docs-commit agent runs. A tick in that window must keep both the live docs agent and the dev
    /// worker (whose retirement belongs AFTER docs-commit).
    #[tokio::test]
    async fn sweep_keeps_live_docs_agent_and_worker_of_just_completed_plan() {
        let (state, root) = state();
        let w = session(&state);
        let p = plan(&state, "development", "approved", Some(&w));
        phase(&state, &p, 0, "passed");
        let docs = session(&state);
        register_review_session(&state, &p, &docs, "docs").unwrap();
        let tmux = FakeTmux::new(vec![live(&w), live(&docs)]);
        let panes = FakePanes::new(true);
        // Engine booted before both rows were created: the docs waiter is alive in this process.
        let r = run_orphan_sweep_with(&state, &tmux, &panes, NOW, 0, &SweepHooks::default()).await;
        assert_eq!(r.selected, 0);
        assert!(panes.calls().is_empty());
        assert_eq!(status(&state, &w).as_deref(), Some("active"));
        assert_eq!(status(&state, &docs).as_deref(), Some("active"));
        // After an engine restart (rows predate boot) the docs agent is stale and reaped;
        // the worker waits until it is unregistered.
        let r = sweep(&state, &tmux, &panes).await;
        assert_eq!(r.retired, 1);
        assert_eq!(status(&state, &docs).as_deref(), Some("archived"));
        assert_eq!(status(&state, &w).as_deref(), Some("active"));
        assert_eq!(retired(&state, &p)[0]["reason"], "orphan_sweep:stale_ephemeral");
        // Next tick (the docs pane is gone now): docs unregistered, completion settled → worker retired.
        let tmux = FakeTmux::new(vec![live(&w)]);
        let r = sweep(&state, &tmux, &panes).await;
        assert_eq!(r.retired, 1);
        assert_eq!(status(&state, &w).as_deref(), Some("archived"));
        let _ = std::fs::remove_dir_all(&root);
    }

    /// Validator finding (phase 01): re-validation must re-read tmux too, so a human who attached
    /// between the listing and the act is respected.
    #[tokio::test]
    async fn sweep_revalidation_rechecks_attached_clients() {
        let (state, root) = state();
        let w = session(&state);
        plan(&state, "planning", "approved", Some(&w));
        let rowless = uuid::Uuid::new_v4().to_string();
        let mut tmux = FakeTmux::new(vec![live(&w), live(&rowless)]);
        tmux.attach_after_first_list = true;
        let panes = FakePanes::new(true);
        let r = sweep(&state, &tmux, &panes).await;
        assert_eq!(r.selected, 2);
        assert_eq!(r.revalidated_kept, 2);
        assert!(tmux.killed().is_empty());
        assert!(panes.calls().is_empty());
        assert_eq!(status(&state, &w).as_deref(), Some("active"));
        let _ = std::fs::remove_dir_all(&root);
    }

    #[tokio::test]
    async fn sweep_keeps_paused_single_phase_worker() {
        let (state, root) = state();
        let w = session(&state);
        let p = plan(&state, "development", "approved", Some(&w));
        phase(&state, &p, 0, "passed");
        phase(&state, &p, 1, "planned");
        let tmux = FakeTmux::new(vec![live(&w)]);
        let panes = FakePanes::new(true);
        let r = sweep(&state, &tmux, &panes).await;
        assert_eq!(r.selected, 0);
        assert!(panes.calls().is_empty());
        assert_eq!(status(&state, &w).as_deref(), Some("active"));
        let _ = std::fs::remove_dir_all(&root);
    }

    #[tokio::test]
    async fn sweep_retires_worker_of_fully_passed_dev_plan() {
        let (state, root) = state();
        let w = session(&state);
        let p = plan(&state, "development", "approved", Some(&w));
        phase(&state, &p, 0, "passed");
        phase(&state, &p, 1, "passed");
        let tmux = FakeTmux::new(vec![live(&w)]);
        let r = sweep(&state, &tmux, &FakePanes::new(true)).await;
        assert_eq!(r.retired, 1);
        assert_eq!(retired(&state, &p)[0]["reason"], "orphan_sweep:plan_terminal");
        let _ = std::fs::remove_dir_all(&root);
    }

    #[tokio::test]
    async fn sweep_retires_replaced_old_worker_of_terminal_plan() {
        let (state, root) = state();
        let old = session(&state);
        crate::services::sessions::mark_session_archived(&state, &old).unwrap();
        let new = session(&state);
        let p = plan(&state, "development", "approved", Some(&new));
        let tmux = FakeTmux::new(vec![live(&old)]);
        let panes = FakePanes::new(true);
        let r = sweep(&state, &tmux, &panes).await;
        assert_eq!(r.retired, 1);
        assert_eq!(panes.calls(), vec![old.clone()]);
        assert_eq!(status(&state, &old).as_deref(), Some("archived"));
        assert!(retired(&state, &p).is_empty(), "no plan owns it any more: tracing only");
        let _ = std::fs::remove_dir_all(&root);
    }

    #[test]
    fn link_job_ended_table() {
        // Executor-side pin of the master §4 table, including "zero phase rows → not ended".
        let (state, root) = state();
        let p = plan(&state, "development", "approved", None);
        let none = state.db.with_conn(|c| all_phases_passed(c, &p)).unwrap();
        assert!(!none, "zero phase rows is not ended");
        assert!(!link_job_ended("development", "approved", "worker", false, none));
        phase(&state, &p, 0, "passed");
        let one = state.db.with_conn(|c| all_phases_passed(c, &p)).unwrap();
        assert!(link_job_ended("development", "approved", "worker", false, one));
        phase(&state, &p, 1, "reviewer_running");
        let mixed = state.db.with_conn(|c| all_phases_passed(c, &p)).unwrap();
        assert!(!link_job_ended("development", "approved", "worker", false, mixed));
        assert!(link_job_ended("development", "stopped", "worker", false, false));
        assert!(link_job_ended("planning", "done", "planner", false, false));
        assert!(!link_job_ended("development", "done", "qa", true, false), "registered ephemeral still in flight");
        assert!(!link_job_ended("development", "phase_review_running", "qa", true, false));
        let _ = std::fs::remove_dir_all(&root);
    }
}
