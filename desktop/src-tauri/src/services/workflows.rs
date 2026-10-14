//! The workflow arena's data API (migration 026).
//!
//! A *workflow* is the searched artifact: the four prompt parts — role, instruction, contract, hop
//! — a phase-type is executed with, versioned, with `parent_id` lineage. A *run* is one execution
//! of one workflow, scored by one number: the test gate's exit code. A *preference* records that
//! one run beat another. A *champion* is the one workflow per `(repo, phase_type, family)` that
//! J1 actually uses, and it materialises as an ordinary plan file, so nothing in the execution
//! path changes.
//!
//! # Every rule lives in exactly one named function
//!
//! This module follows the precedent set by [`crate::services::agent_plans::effective_gate`]: a
//! rule that more than one caller needs is a `pub fn` with a name, not a clause inlined at each
//! call site. The repo has already been bitten by three independent writers of one SQL clause
//! drifting apart. The rules here, and the only places they live:
//!
//! | rule | the one function |
//! |---|---|
//! | what a seat budget is | [`parse_seat_map`] |
//! | when two runs are comparable | [`budgets_are_comparable`] |
//! | whether a run may become champion | [`promotion_block`] |
//! | when the search has halted | [`workflow_search_state`] + [`REJECTION_STRIKE_LIMIT`] |
//! | the next version number in a family | [`next_version_in_family`] |
//!
//! # The score
//!
//! `gate_exit` is the score and the only score. `gate_collected` is **advisory and nullable** — a
//! gate that prints nothing a parser recognises is a legitimate gate with a well-known exit code,
//! so `None` is a normal result, never an error. [`promotion_block`] reads `gate_exit` alone, and
//! that is deliberate: the moment anything branches on `gate_collected`, a gate that collects
//! nothing stops being reportable and starts being load-bearing.

use crate::state::app_state::AppState;
use chrono::Utc;
use rusqlite::{params, Connection, OptionalExtension};
use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;
use uuid::Uuid;

/// How many consecutive rejections in one `(repo, phase_type, family)` halt the search.
///
/// "Consecutive" means trailing in [`workflow_decisions`](../../../migrations/026_add_workflow_arena.sql)
/// `seq` order: a promotion in the same family resets the streak, which is also the only way out of
/// a halt. Three is the design's number; it is a constant so that the enforcement point
/// ([`start_workflow_run`]) and the reporting point ([`workflow_search_state`]) can never disagree
/// about it.
pub const REJECTION_STRIKE_LIMIT: i64 = 3;

// ---------------------------------------------------------------------------------------------
// Row types. camelCase JSON so the host RPC payload matches the worker GraphQL field names.
// ---------------------------------------------------------------------------------------------

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Workflow {
    pub id: String,
    pub repo: String,
    pub phase_type: String,
    pub family: String,
    pub version: i64,
    pub parent_id: Option<String>,
    pub role: String,
    pub instruction: String,
    pub contract: String,
    /// JSON: steps, parallel groups, returns. Validated as JSON on write by [`parse_hop`]; served
    /// back as the stored string, because the host does not own the Hop's shape.
    pub hop: String,
    pub fan_out: i64,
    pub is_champion: bool,
    pub created_at: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct WorkflowRun {
    pub id: String,
    pub workflow_id: String,
    pub plan_id: Option<String>,
    pub seed: i64,
    /// The score. `None` means the gate never ran — which [`promotion_block`] refuses, because a
    /// run that was never scored has not earned anything.
    pub gate_exit: Option<i64>,
    /// **Advisory and nullable.** See the module docs. Never branch on this.
    pub gate_collected: Option<i64>,
    /// JSON object, model → seats. The seat budget this run was given.
    pub seats_held: String,
    /// JSON object, fleet contention at start. Stored for a human to read when two runs disagree;
    /// not part of the v1 comparability rule, which is [`budgets_are_comparable`] over
    /// `seats_held`.
    pub fleet_busy: String,
    pub started_at: String,
    pub ended_at: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct WorkflowPref {
    pub id: String,
    pub winner_run_id: String,
    pub loser_run_id: String,
    /// False when the two runs' seat budgets differed. A non-comparable preference is still
    /// recorded — it happened — but it cannot promote. See [`promotion_block`].
    pub comparable: bool,
    pub reason: Option<String>,
    pub created_at: String,
}

/// The search's state for one `(repo, phase_type, family)`, as data rather than a log line.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct WorkflowSearchState {
    pub repo: String,
    pub phase_type: String,
    pub family: String,
    /// Rejections since the last promotion in this family, counted from the end of the decision
    /// log. Capped only by how many rejections are actually there.
    pub consecutive_rejections: i64,
    /// `consecutive_rejections >= REJECTION_STRIKE_LIMIT`. While true, [`start_workflow_run`]
    /// refuses to start another run in this family.
    pub halted: bool,
    pub strike_limit: i64,
    /// The reasons of the trailing rejections, newest first — the one thing a halted search needs
    /// to show a human.
    pub recent_rejections: Vec<String>,
    /// The current champion's workflow id, if this family has one.
    pub champion_workflow_id: Option<String>,
}

/// Input for [`create_workflow_version`].
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct NewWorkflowVersion {
    pub repo: String,
    pub phase_type: String,
    pub family: String,
    /// Lineage. When set, it must name an existing workflow in the SAME
    /// `(repo, phase_type, family)` — a parent from another family is not lineage, it is a
    /// category error, and allowing it is how a Hop tuned for one context window ends up
    /// attributed to another.
    pub parent_id: Option<String>,
    pub role: String,
    pub instruction: String,
    pub contract: String,
    pub hop: String,
    pub fan_out: i64,
}

/// Input for [`start_workflow_run`].
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct NewWorkflowRun {
    pub workflow_id: String,
    pub plan_id: Option<String>,
    pub seed: i64,
    pub seats_held: String,
    pub fleet_busy: String,
}

// ---------------------------------------------------------------------------------------------
// The rules. One function each.
// ---------------------------------------------------------------------------------------------

/// **The seat-budget reader.** The single parser of the `seats_held` / `fleet_busy` encoding: a
/// JSON object of model name → seat count.
///
/// Entries whose count is zero are dropped, so `{"glm":3}` and `{"glm":3,"qwen":0}` are the same
/// budget. Without that, "I held no Qwen seats" and "I did not mention Qwen" would compare
/// unequal and every preference between them would be silently downgraded to non-comparable.
///
/// A negative count, a non-integer count, a non-object payload and malformed JSON are all errors
/// rather than empty maps: an unreadable budget must not look like "no seats", which would make
/// two broken runs compare equal and promotable.
pub fn parse_seat_map(raw: &str) -> Result<BTreeMap<String, i64>, String> {
    let value: serde_json::Value = serde_json::from_str(raw.trim())
        .map_err(|e| format!("seat budget is not JSON: {}", e))?;
    let object = value
        .as_object()
        .ok_or_else(|| "seat budget must be a JSON object of model → seats".to_string())?;
    let mut out = BTreeMap::new();
    for (model, count) in object {
        let count = count
            .as_i64()
            .ok_or_else(|| format!("seat count for '{}' is not an integer", model))?;
        if count < 0 {
            return Err(format!("seat count for '{}' is negative", model));
        }
        if count > 0 {
            out.insert(model.clone(), count);
        }
    }
    Ok(out)
}

/// **The comparability rule.** Two runs are comparable when their seat budgets are identical.
///
/// This is the whole of `workflow_prefs.comparable`, and it is a function rather than an inline
/// comparison because two callers need it: [`record_workflow_pref`], which writes the flag, and
/// the tests that pin it. The refusal that makes it bite lives in [`promotion_block`].
///
/// An UNREADABLE budget on either side is not comparable. That is the conservative direction: the
/// cost of a false "not comparable" is one preference that cannot promote, and the cost of a false
/// "comparable" is promoting a champion because the other box happened to be quiet — which is the
/// exact failure `seats_held` was added to prevent.
pub fn budgets_are_comparable(left_seats_held: &str, right_seats_held: &str) -> bool {
    match (parse_seat_map(left_seats_held), parse_seat_map(right_seats_held)) {
        (Ok(left), Ok(right)) => left == right,
        _ => false,
    }
}

/// **The hop reader.** `hop` is JSON (steps, parallel groups, returns) and is validated as such on
/// write, then stored and served verbatim.
///
/// The host deliberately does not model the Hop's shape — that belongs to whatever executes it —
/// but it does refuse a payload that is not JSON at all. `test_commands` had to tolerate legacy
/// malformed rows and so has a three-state [`crate::services::agent_plans::DeclaredGate`]; this
/// column is new in migration 026, there are no legacy rows, and so the cheaper guarantee is
/// available: nothing unparseable is ever stored.
pub fn parse_hop(raw: &str) -> Result<serde_json::Value, String> {
    let trimmed = raw.trim();
    if trimmed.is_empty() {
        return Err("hop is required: a workflow with no hop has nothing to execute".to_string());
    }
    serde_json::from_str(trimmed).map_err(|e| format!("hop is not JSON: {}", e))
}

/// **The promotion rule.** `None` means this run may be promoted; `Some(reason)` is the refusal,
/// in words a human can act on.
///
/// Every refusal the design asks for, in one place, because promotion is the one irreversible step
/// in the arena:
///
/// * `gate_exit` is `NULL` — the gate never ran, so nothing was proven.
/// * `gate_exit` is non-zero — the gate ran and failed. The exit code is the only score, and a
///   failing score cannot win.
/// * the preference is not `comparable` — the two runs held different seat budgets, so the
///   comparison measured the fleet, not the workflow.
/// * the preference does not name this run as its winner — a caller passing someone else's
///   preference is a bug, not a promotion.
/// * there is no preference at all AND the family already has a champion — displacing an incumbent
///   without a comparable head-to-head is exactly what `comparable` exists to prevent. Seeding the
///   FIRST champion of a family is allowed with no preference, because there is nothing to compare
///   against; its gate still has to be green.
///
/// `gate_collected` is not consulted. See the module docs.
pub fn promotion_block(
    run: &WorkflowRun,
    pref: Option<&WorkflowPref>,
    incumbent_champion_workflow_id: Option<&str>,
) -> Option<String> {
    match run.gate_exit {
        None => {
            return Some(format!(
                "run {} has no gate result: the gate never ran, so nothing is proven",
                run.id
            ));
        }
        Some(0) => {}
        Some(exit) => {
            return Some(format!(
                "run {} failed its gate (exit {}): the exit code is the only score",
                run.id, exit
            ));
        }
    }
    match pref {
        Some(pref) => {
            if pref.winner_run_id != run.id {
                return Some(format!(
                    "preference {} names run {} as the winner, not run {}",
                    pref.id, pref.winner_run_id, run.id
                ));
            }
            if !pref.comparable {
                return Some(format!(
                    "preference {} is not comparable: the two runs held different seat budgets, \
                     so the comparison measured the fleet rather than the workflow",
                    pref.id
                ));
            }
            None
        }
        None => match incumbent_champion_workflow_id {
            None => None,
            Some(incumbent) => Some(format!(
                "workflow {} is already the champion of this (repo, phase_type, family); \
                 displacing it needs a comparable preference naming run {} as the winner",
                incumbent, run.id
            )),
        },
    }
}

// ---------------------------------------------------------------------------------------------
// Row mappers. One per table, used by every read, so no two queries shape a row differently.
// ---------------------------------------------------------------------------------------------

const WORKFLOW_COLUMNS: &str = "id, repo, phase_type, family, version, parent_id, role, \
                                instruction, contract, hop, fan_out, is_champion, created_at";

fn map_workflow(row: &rusqlite::Row<'_>) -> rusqlite::Result<Workflow> {
    Ok(Workflow {
        id: row.get(0)?,
        repo: row.get(1)?,
        phase_type: row.get(2)?,
        family: row.get(3)?,
        version: row.get(4)?,
        parent_id: row.get(5)?,
        role: row.get(6)?,
        instruction: row.get(7)?,
        contract: row.get(8)?,
        hop: row.get(9)?,
        fan_out: row.get(10)?,
        is_champion: row.get::<_, i64>(11)? != 0,
        created_at: row.get(12)?,
    })
}

const RUN_COLUMNS: &str = "id, workflow_id, plan_id, seed, gate_exit, gate_collected, \
                           seats_held, fleet_busy, started_at, ended_at";

fn map_run(row: &rusqlite::Row<'_>) -> rusqlite::Result<WorkflowRun> {
    Ok(WorkflowRun {
        id: row.get(0)?,
        workflow_id: row.get(1)?,
        plan_id: row.get(2)?,
        seed: row.get(3)?,
        gate_exit: row.get(4)?,
        gate_collected: row.get(5)?,
        seats_held: row.get(6)?,
        fleet_busy: row.get(7)?,
        started_at: row.get(8)?,
        ended_at: row.get(9)?,
    })
}

const PREF_COLUMNS: &str = "id, winner_run_id, loser_run_id, comparable, reason, created_at";

fn map_pref(row: &rusqlite::Row<'_>) -> rusqlite::Result<WorkflowPref> {
    Ok(WorkflowPref {
        id: row.get(0)?,
        winner_run_id: row.get(1)?,
        loser_run_id: row.get(2)?,
        comparable: row.get::<_, i64>(3)? != 0,
        reason: row.get(4)?,
        created_at: row.get(5)?,
    })
}

// ---------------------------------------------------------------------------------------------
// Connection-level helpers. Everything that touches SQL goes through one of these, so the
// transactional operations and the plain reads share the same statements.
// ---------------------------------------------------------------------------------------------

fn read_workflow(conn: &Connection, id: &str) -> Result<Workflow, String> {
    conn.query_row(
        &format!("SELECT {} FROM workflows WHERE id = ?1", WORKFLOW_COLUMNS),
        params![id],
        map_workflow,
    )
    .optional()
    .map_err(|e| e.to_string())?
    .ok_or_else(|| format!("workflow {} not found", id))
}

fn read_run(conn: &Connection, id: &str) -> Result<WorkflowRun, String> {
    conn.query_row(
        &format!("SELECT {} FROM workflow_runs WHERE id = ?1", RUN_COLUMNS),
        params![id],
        map_run,
    )
    .optional()
    .map_err(|e| e.to_string())?
    .ok_or_else(|| format!("workflow run {} not found", id))
}

fn read_pref(conn: &Connection, id: &str) -> Result<WorkflowPref, String> {
    conn.query_row(
        &format!("SELECT {} FROM workflow_prefs WHERE id = ?1", PREF_COLUMNS),
        params![id],
        map_pref,
    )
    .optional()
    .map_err(|e| e.to_string())?
    .ok_or_else(|| format!("workflow preference {} not found", id))
}

fn champion_workflow_id(
    conn: &Connection,
    repo: &str,
    phase_type: &str,
    family: &str,
) -> Result<Option<String>, String> {
    conn.query_row(
        "SELECT id FROM workflows \
         WHERE repo = ?1 AND phase_type = ?2 AND family = ?3 AND is_champion = 1",
        params![repo, phase_type, family],
        |row| row.get::<_, String>(0),
    )
    .optional()
    .map_err(|e| e.to_string())
}

/// **The version rule.** The next `version` for a `(repo, phase_type, family)`: one past the
/// highest already there, starting at 1.
///
/// Monotonic WITHIN a family and independent across families, which is what makes `version` mean
/// "the Nth attempt at this phase-type on this model" rather than a global counter. Callers run it
/// inside the same transaction as the INSERT — read-then-write outside one would hand two
/// concurrent creates the same number, and the schema has no unique index on
/// `(repo, phase_type, family, version)` to catch it. (It deliberately does not: the design's only
/// uniqueness claim is one champion per family.)
pub fn next_version_in_family(
    conn: &Connection,
    repo: &str,
    phase_type: &str,
    family: &str,
) -> Result<i64, String> {
    conn.query_row(
        "SELECT COALESCE(MAX(version), 0) + 1 FROM workflows \
         WHERE repo = ?1 AND phase_type = ?2 AND family = ?3",
        params![repo, phase_type, family],
        |row| row.get::<_, i64>(0),
    )
    .map_err(|e| e.to_string())
}

fn require_non_blank(label: &str, value: &str) -> Result<String, String> {
    let trimmed = value.trim();
    if trimmed.is_empty() {
        return Err(format!("{} is required", label));
    }
    Ok(trimmed.to_string())
}

// ---------------------------------------------------------------------------------------------
// Writes
// ---------------------------------------------------------------------------------------------

/// Create a new workflow version from the four prompt parts.
///
/// The new row is never a champion: becoming one is [`promote_workflow`]'s job and needs a green
/// gate and a human. `version` is assigned by [`next_version_in_family`] inside the transaction.
pub fn create_workflow_version(
    state: &AppState,
    input: NewWorkflowVersion,
) -> Result<Workflow, String> {
    let repo = require_non_blank("repo", &input.repo)?;
    let phase_type = require_non_blank("phaseType", &input.phase_type)?;
    let family = require_non_blank("family", &input.family)?;
    let role = require_non_blank("role", &input.role)?;
    let instruction = require_non_blank("instruction", &input.instruction)?;
    let contract = require_non_blank("contract", &input.contract)?;
    // Validated, then stored as the caller's own text: the host does not own the Hop's shape.
    parse_hop(&input.hop)?;
    let hop = input.hop.trim().to_string();
    if input.fan_out < 1 {
        return Err(format!(
            "fanOut must be at least 1 seat (got {}): a workflow that holds no seat cannot run",
            input.fan_out
        ));
    }

    let id = Uuid::new_v4().to_string();
    let created_at = Utc::now().to_rfc3339();

    state.db.with_conn(|conn| {
        let tx = conn.unchecked_transaction().map_err(|e| e.to_string())?;
        if let Some(parent_id) = input.parent_id.as_deref() {
            let parent = read_workflow(&tx, parent_id)?;
            if parent.repo != repo || parent.phase_type != phase_type || parent.family != family {
                return Err(format!(
                    "parent {} is ({}, {}, {}) but this version is ({}, {}, {}): lineage does not \
                     cross families, because a hop tuned for one model does not transfer to another",
                    parent_id,
                    parent.repo,
                    parent.phase_type,
                    parent.family,
                    repo,
                    phase_type,
                    family
                ));
            }
        }
        let version = next_version_in_family(&tx, &repo, &phase_type, &family)?;
        tx.execute(
            "INSERT INTO workflows (id, repo, phase_type, family, version, parent_id, role, \
             instruction, contract, hop, fan_out, is_champion, created_at) \
             VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11, 0, ?12)",
            params![
                id,
                repo,
                phase_type,
                family,
                version,
                input.parent_id,
                role,
                instruction,
                contract,
                hop,
                input.fan_out,
                created_at
            ],
        )
        .map_err(|e| e.to_string())?;
        let created = read_workflow(&tx, &id)?;
        tx.commit().map_err(|e| e.to_string())?;
        Ok(created)
    })
}

/// Start a run of a workflow: the seat budget it holds, the fleet contention it started under, and
/// its replay seed.
///
/// **Refuses while the family's search is halted.** That is what "three consecutive rejections halt
/// the search" means operationally — not a log line. The way out is a promotion in that family,
/// which resets the streak.
pub fn start_workflow_run(state: &AppState, input: NewWorkflowRun) -> Result<WorkflowRun, String> {
    // Both budgets are validated BEFORE the insert: an unreadable `seats_held` would make every
    // preference involving this run non-comparable, which is a silent dead end rather than an error.
    parse_seat_map(&input.seats_held)
        .map_err(|e| format!("seatsHeld: {}", e))?;
    parse_seat_map(&input.fleet_busy).map_err(|e| format!("fleetBusy: {}", e))?;
    if input.seed < 0 {
        return Err(format!("seed must not be negative (got {})", input.seed));
    }

    let id = Uuid::new_v4().to_string();
    let started_at = Utc::now().to_rfc3339();
    let seats_held = input.seats_held.trim().to_string();
    let fleet_busy = input.fleet_busy.trim().to_string();

    state.db.with_conn(|conn| {
        let tx = conn.unchecked_transaction().map_err(|e| e.to_string())?;
        let workflow = read_workflow(&tx, &input.workflow_id)?;
        let search = search_state_on(&tx, &workflow.repo, &workflow.phase_type, &workflow.family)?;
        if search.halted {
            return Err(format!(
                "the search for ({}, {}, {}) is halted after {} consecutive rejections \
                 (limit {}): record a promotion in this family to resume. Recent reasons: {}",
                workflow.repo,
                workflow.phase_type,
                workflow.family,
                search.consecutive_rejections,
                search.strike_limit,
                search.recent_rejections.join(" | ")
            ));
        }
        tx.execute(
            "INSERT INTO workflow_runs (id, workflow_id, plan_id, seed, gate_exit, \
             gate_collected, seats_held, fleet_busy, started_at, ended_at) \
             VALUES (?1, ?2, ?3, ?4, NULL, NULL, ?5, ?6, ?7, NULL)",
            params![
                id,
                input.workflow_id,
                input.plan_id,
                input.seed,
                seats_held,
                fleet_busy,
                started_at
            ],
        )
        .map_err(|e| e.to_string())?;
        let created = read_run(&tx, &id)?;
        tx.commit().map_err(|e| e.to_string())?;
        Ok(created)
    })
}

/// Record a run's gate result and end the run.
///
/// `gate_exit` is authoritative. `gate_collected` is **advisory and may be `None`** — a gate that
/// printed nothing a parser recognises still has a perfectly good exit code, and passing `None`
/// here is a normal outcome, not a failure. Nothing in this module branches on it.
pub fn record_workflow_run_gate(
    state: &AppState,
    run_id: &str,
    gate_exit: i64,
    gate_collected: Option<i64>,
) -> Result<WorkflowRun, String> {
    let ended_at = Utc::now().to_rfc3339();
    state.db.with_conn(|conn| {
        let tx = conn.unchecked_transaction().map_err(|e| e.to_string())?;
        // Read first so a missing run is "not found" rather than a silent zero-row UPDATE.
        read_run(&tx, run_id)?;
        tx.execute(
            "UPDATE workflow_runs SET gate_exit = ?2, gate_collected = ?3, ended_at = ?4 \
             WHERE id = ?1",
            params![run_id, gate_exit, gate_collected, ended_at],
        )
        .map_err(|e| e.to_string())?;
        let updated = read_run(&tx, run_id)?;
        tx.commit().map_err(|e| e.to_string())?;
        Ok(updated)
    })
}

/// Record that one run beat another.
///
/// `comparable` is NOT a caller-supplied field: it is computed here by [`budgets_are_comparable`]
/// from the two runs' own `seats_held`, so a client cannot assert comparability it does not have.
/// A non-comparable preference is still written — it happened, and the design wants it as data —
/// it simply cannot promote.
pub fn record_workflow_pref(
    state: &AppState,
    winner_run_id: &str,
    loser_run_id: &str,
    reason: Option<String>,
) -> Result<WorkflowPref, String> {
    if winner_run_id == loser_run_id {
        return Err(
            "a run cannot be preferred over itself: `workflow_prefs` is the project-level \
             measurement, and a self-preference corrupts any ranking computed from it"
                .to_string(),
        );
    }
    let id = Uuid::new_v4().to_string();
    let created_at = Utc::now().to_rfc3339();
    let reason = reason
        .map(|text| text.trim().to_string())
        .filter(|text| !text.is_empty());

    state.db.with_conn(|conn| {
        let tx = conn.unchecked_transaction().map_err(|e| e.to_string())?;
        let winner = read_run(&tx, winner_run_id)?;
        let loser = read_run(&tx, loser_run_id)?;
        let comparable = budgets_are_comparable(&winner.seats_held, &loser.seats_held);
        tx.execute(
            "INSERT INTO workflow_prefs (id, winner_run_id, loser_run_id, comparable, reason, \
             created_at) VALUES (?1, ?2, ?3, ?4, ?5, ?6)",
            params![
                id,
                winner_run_id,
                loser_run_id,
                if comparable { 1_i64 } else { 0_i64 },
                reason,
                created_at
            ],
        )
        .map_err(|e| e.to_string())?;
        let created = read_pref(&tx, &id)?;
        tx.commit().map_err(|e| e.to_string())?;
        Ok(created)
    })
}

/// Promote a run's workflow to champion of its `(repo, phase_type, family)`.
///
/// Human-confirmed: `confirmed` must be `true`. This is the one irreversible step in the arena, so
/// it is not something a client reaches by forgetting an argument.
///
/// The demote and the promote are ONE transaction, in that order. Two statements outside a
/// transaction would leave a window with two champions — which the partial unique index
/// `workflows_one_champion` rejects outright, so the failure mode is a hard error rather than
/// corruption; doing it in the wrong order inside the transaction fails the same way. Both are
/// pinned by tests.
///
/// Every refusal is [`promotion_block`]'s, not this function's: the rule has one home.
pub fn promote_workflow(
    state: &AppState,
    run_id: &str,
    pref_id: Option<&str>,
    confirmed: bool,
) -> Result<Workflow, String> {
    if !confirmed {
        return Err(
            "promotion is the arena's one human-confirmed step and must be explicitly confirmed"
                .to_string(),
        );
    }
    let decision_id = Uuid::new_v4().to_string();
    let created_at = Utc::now().to_rfc3339();

    state.db.with_conn(|conn| {
        let tx = conn.unchecked_transaction().map_err(|e| e.to_string())?;
        let run = read_run(&tx, run_id)?;
        let workflow = read_workflow(&tx, &run.workflow_id)?;
        let pref = match pref_id {
            Some(pref_id) => Some(read_pref(&tx, pref_id)?),
            None => None,
        };
        let incumbent =
            champion_workflow_id(&tx, &workflow.repo, &workflow.phase_type, &workflow.family)?;
        // An already-champion workflow is a no-op rather than a self-displacement error, but it
        // still has to pass the gate check, so the block is consulted first either way.
        if let Some(reason) =
            promotion_block(&run, pref.as_ref(), incumbent.as_deref().filter(|id| *id != workflow.id))
        {
            return Err(format!("refusing to promote: {}", reason));
        }
        // Demote THEN promote, inside this transaction. The other order trips
        // `workflows_one_champion`.
        tx.execute(
            "UPDATE workflows SET is_champion = 0 \
             WHERE repo = ?1 AND phase_type = ?2 AND family = ?3 AND is_champion = 1",
            params![workflow.repo, workflow.phase_type, workflow.family],
        )
        .map_err(|e| e.to_string())?;
        tx.execute(
            "UPDATE workflows SET is_champion = 1 WHERE id = ?1",
            params![workflow.id],
        )
        .map_err(|e| e.to_string())?;
        tx.execute(
            "INSERT INTO workflow_decisions (id, run_id, kind, reason, created_at) \
             VALUES (?1, ?2, 'promote', NULL, ?3)",
            params![decision_id, run_id, created_at],
        )
        .map_err(|e| e.to_string())?;
        let promoted = read_workflow(&tx, &workflow.id)?;
        tx.commit().map_err(|e| e.to_string())?;
        Ok(promoted)
    })
}

/// Reject a run, with a reason, and report the family's resulting search state.
///
/// The reason is mandatory — a refusal with nothing recorded teaches the search nothing, and the
/// one thing a halted search owes a human is the three reasons it halted on. The return value is
/// the halt state as DATA: `halted` is true once [`REJECTION_STRIKE_LIMIT`] rejections have run
/// consecutively in this family, and from then on [`start_workflow_run`] refuses.
pub fn reject_workflow_run(
    state: &AppState,
    run_id: &str,
    reason: &str,
) -> Result<WorkflowSearchState, String> {
    let reason = require_non_blank("reason", reason).map_err(|_| {
        "a rejection must record a reason: three of them halt the search, and the reasons are \
         what a halted search has to show"
            .to_string()
    })?;
    let id = Uuid::new_v4().to_string();
    let created_at = Utc::now().to_rfc3339();

    state.db.with_conn(|conn| {
        let tx = conn.unchecked_transaction().map_err(|e| e.to_string())?;
        let run = read_run(&tx, run_id)?;
        let workflow = read_workflow(&tx, &run.workflow_id)?;
        tx.execute(
            "INSERT INTO workflow_decisions (id, run_id, kind, reason, created_at) \
             VALUES (?1, ?2, 'reject', ?3, ?4)",
            params![id, run_id, reason, created_at],
        )
        .map_err(|e| e.to_string())?;
        let state_after =
            search_state_on(&tx, &workflow.repo, &workflow.phase_type, &workflow.family)?;
        tx.commit().map_err(|e| e.to_string())?;
        Ok(state_after)
    })
}

// ---------------------------------------------------------------------------------------------
// Reads
// ---------------------------------------------------------------------------------------------

/// List workflows, newest version first, optionally narrowed by any part of the champion key.
pub fn list_workflows(
    state: &AppState,
    repo: Option<&str>,
    phase_type: Option<&str>,
    family: Option<&str>,
) -> Result<Vec<Workflow>, String> {
    state.db.with_conn(|conn| {
        let mut stmt = conn
            .prepare(&format!(
                "SELECT {} FROM workflows \
                 WHERE (?1 IS NULL OR repo = ?1) \
                   AND (?2 IS NULL OR phase_type = ?2) \
                   AND (?3 IS NULL OR family = ?3) \
                 ORDER BY repo, phase_type, family, version DESC",
                WORKFLOW_COLUMNS
            ))
            .map_err(|e| e.to_string())?;
        let rows = stmt
            .query_map(params![repo, phase_type, family], map_workflow)
            .map_err(|e| e.to_string())?;
        let mut out = Vec::new();
        for row in rows {
            out.push(row.map_err(|e| e.to_string())?);
        }
        Ok(out)
    })
}

pub fn get_workflow(state: &AppState, id: &str) -> Result<Workflow, String> {
    state.db.with_conn(|conn| read_workflow(conn, id))
}

/// A workflow's runs, newest first.
pub fn list_workflow_runs(state: &AppState, workflow_id: &str) -> Result<Vec<WorkflowRun>, String> {
    state.db.with_conn(|conn| {
        // A missing workflow is an error, not an empty list: "no runs" and "no such workflow" are
        // different answers and a caller showing an empty arena cannot tell them apart.
        read_workflow(conn, workflow_id)?;
        let mut stmt = conn
            .prepare(&format!(
                "SELECT {} FROM workflow_runs WHERE workflow_id = ?1 \
                 ORDER BY started_at DESC, rowid DESC",
                RUN_COLUMNS
            ))
            .map_err(|e| e.to_string())?;
        let rows = stmt
            .query_map(params![workflow_id], map_run)
            .map_err(|e| e.to_string())?;
        let mut out = Vec::new();
        for row in rows {
            out.push(row.map_err(|e| e.to_string())?);
        }
        Ok(out)
    })
}

/// **The halt rule.** The search state for one `(repo, phase_type, family)`.
///
/// "Three CONSECUTIVE rejections" is an ordering question, so it is answered by walking the
/// decision log backwards in `seq` order and counting rejections until a promotion stops the
/// count. A promotion in the family therefore resets the streak, and is the only thing that does —
/// which is also the documented way out of a halt.
///
/// `seq` rather than `created_at` because timestamps tie: two decisions in the same second are
/// indistinguishable by time, and the whole question is which came last.
pub fn workflow_search_state(
    state: &AppState,
    repo: &str,
    phase_type: &str,
    family: &str,
) -> Result<WorkflowSearchState, String> {
    state
        .db
        .with_conn(|conn| search_state_on(conn, repo, phase_type, family))
}

/// [`workflow_search_state`] against an arbitrary connection, so the enforcement points can
/// consult it inside their own transaction instead of re-deriving the rule.
fn search_state_on(
    conn: &Connection,
    repo: &str,
    phase_type: &str,
    family: &str,
) -> Result<WorkflowSearchState, String> {
    let mut stmt = conn
        .prepare(
            "SELECT d.kind, d.reason FROM workflow_decisions d \
             JOIN workflow_runs r ON r.id = d.run_id \
             JOIN workflows w ON w.id = r.workflow_id \
             WHERE w.repo = ?1 AND w.phase_type = ?2 AND w.family = ?3 \
             ORDER BY d.seq DESC",
        )
        .map_err(|e| e.to_string())?;
    let rows = stmt
        .query_map(params![repo, phase_type, family], |row| {
            Ok((row.get::<_, String>(0)?, row.get::<_, Option<String>>(1)?))
        })
        .map_err(|e| e.to_string())?;

    let mut consecutive_rejections = 0_i64;
    let mut recent_rejections = Vec::new();
    for row in rows {
        let (kind, reason) = row.map_err(|e| e.to_string())?;
        if kind != "reject" {
            break;
        }
        consecutive_rejections += 1;
        if let Some(reason) = reason {
            recent_rejections.push(reason);
        }
    }

    Ok(WorkflowSearchState {
        repo: repo.to_string(),
        phase_type: phase_type.to_string(),
        family: family.to_string(),
        consecutive_rejections,
        halted: consecutive_rejections >= REJECTION_STRIKE_LIMIT,
        strike_limit: REJECTION_STRIKE_LIMIT,
        recent_rejections,
        champion_workflow_id: champion_workflow_id(conn, repo, phase_type, family)?,
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::test_support::test_state;

    fn new_version(repo: &str, phase_type: &str, family: &str) -> NewWorkflowVersion {
        NewWorkflowVersion {
            repo: repo.to_string(),
            phase_type: phase_type.to_string(),
            family: family.to_string(),
            parent_id: None,
            role: "you are a developer".to_string(),
            instruction: "implement the phase".to_string(),
            contract: "the gate must pass".to_string(),
            hop: r#"{"steps":["plan","edit","verify"]}"#.to_string(),
            fan_out: 3,
        }
    }

    fn seed_workflow(state: &AppState, repo: &str, phase_type: &str, family: &str) -> Workflow {
        create_workflow_version(state, new_version(repo, phase_type, family)).expect("create")
    }

    fn start(state: &AppState, workflow_id: &str, seats: &str) -> WorkflowRun {
        start_workflow_run(
            state,
            NewWorkflowRun {
                workflow_id: workflow_id.to_string(),
                plan_id: None,
                seed: 0,
                seats_held: seats.to_string(),
                fleet_busy: "{}".to_string(),
            },
        )
        .expect("start run")
    }

    /// A run that has been scored. `exit` 0 is a pass.
    fn scored_run(state: &AppState, workflow_id: &str, seats: &str, exit: i64) -> WorkflowRun {
        let run = start(state, workflow_id, seats);
        record_workflow_run_gate(state, &run.id, exit, Some(42)).expect("record gate")
    }

    fn champion_ids(state: &AppState, repo: &str, phase_type: &str, family: &str) -> Vec<String> {
        state
            .db
            .with_conn(|conn| {
                let mut stmt = conn
                    .prepare(
                        "SELECT id FROM workflows WHERE repo = ?1 AND phase_type = ?2 \
                         AND family = ?3 AND is_champion = 1 ORDER BY id",
                    )
                    .map_err(|e| e.to_string())?;
                let rows = stmt
                    .query_map(params![repo, phase_type, family], |row| {
                        row.get::<_, String>(0)
                    })
                    .map_err(|e| e.to_string())?;
                let mut out = Vec::new();
                for row in rows {
                    out.push(row.map_err(|e| e.to_string())?);
                }
                Ok(out)
            })
            .expect("champion ids")
    }

    // -------------------------------------------------------------------------------------
    // The schema's own guarantee
    // -------------------------------------------------------------------------------------

    /// Two champions in one `(repo, phase_type, family)` must be IMPOSSIBLE, not merely avoided by
    /// the service. This goes straight at the table with raw SQL, bypassing `promote_workflow`
    /// entirely, because the partial index is the last line of defence against a future second
    /// writer of the promote clause.
    #[test]
    fn champion_index_makes_two_champions_impossible() {
        let (state, _root) = test_state();
        let a = seed_workflow(&state, "repo", "dev-phase", "glm");
        let b = seed_workflow(&state, "repo", "dev-phase", "glm");

        state
            .db
            .with_conn(|conn| {
                conn.execute(
                    "UPDATE workflows SET is_champion = 1 WHERE id = ?1",
                    params![a.id],
                )
                .map_err(|e| e.to_string())?;
                // The second one must be rejected by `workflows_one_champion`.
                let second = conn.execute(
                    "UPDATE workflows SET is_champion = 1 WHERE id = ?1",
                    params![b.id],
                );
                let err = second.expect_err("a second champion must violate the unique index");
                let text = err.to_string();
                assert!(
                    text.contains("UNIQUE constraint failed")
                        || text.contains("constraint failed"),
                    "expected a uniqueness violation, got: {}",
                    text
                );
                Ok(())
            })
            .expect("raw sql");

        assert_eq!(champion_ids(&state, "repo", "dev-phase", "glm"), vec![a.id]);
    }

    /// A different family is a different champion slot — the partial index must NOT be global.
    #[test]
    fn champions_are_per_family_not_global() {
        let (state, _root) = test_state();
        let glm = seed_workflow(&state, "repo", "dev-phase", "glm");
        let qwen = seed_workflow(&state, "repo", "dev-phase", "qwen");
        state
            .db
            .with_conn(|conn| {
                conn.execute(
                    "UPDATE workflows SET is_champion = 1 WHERE id IN (?1, ?2)",
                    params![glm.id, qwen.id],
                )
                .map_err(|e| e.to_string())
            })
            .expect("two families, two champions");
        assert_eq!(champion_ids(&state, "repo", "dev-phase", "glm").len(), 1);
        assert_eq!(champion_ids(&state, "repo", "dev-phase", "qwen").len(), 1);
    }

    /// The ORDER inside `promote_workflow`'s transaction is load-bearing. Promote-then-demote
    /// trips the index, which is why the shipped function demotes first. Pinned so nobody
    /// "simplifies" the order back.
    #[test]
    fn promoting_before_demoting_trips_the_index() {
        let (state, _root) = test_state();
        let incumbent = seed_workflow(&state, "repo", "dev-phase", "glm");
        let challenger = seed_workflow(&state, "repo", "dev-phase", "glm");
        state
            .db
            .with_conn(|conn| {
                conn.execute(
                    "UPDATE workflows SET is_champion = 1 WHERE id = ?1",
                    params![incumbent.id],
                )
                .map_err(|e| e.to_string())
            })
            .expect("seed incumbent");

        state
            .db
            .with_conn(|conn| {
                let tx = conn.unchecked_transaction().map_err(|e| e.to_string())?;
                let wrong_order = tx.execute(
                    "UPDATE workflows SET is_champion = 1 WHERE id = ?1",
                    params![challenger.id],
                );
                assert!(
                    wrong_order.is_err(),
                    "promoting before demoting must be rejected"
                );
                Ok(())
            })
            .expect("tx");
    }

    // -------------------------------------------------------------------------------------
    // Versions and lineage
    // -------------------------------------------------------------------------------------

    #[test]
    fn version_is_monotonic_within_a_family_and_independent_across_families() {
        let (state, _root) = test_state();
        let one = seed_workflow(&state, "repo", "dev-phase", "glm");
        let two = seed_workflow(&state, "repo", "dev-phase", "glm");
        let other_family = seed_workflow(&state, "repo", "dev-phase", "qwen");
        let other_phase = seed_workflow(&state, "repo", "ui-phase", "glm");
        let other_repo = seed_workflow(&state, "other", "dev-phase", "glm");

        assert_eq!(one.version, 1);
        assert_eq!(two.version, 2);
        assert_eq!(other_family.version, 1);
        assert_eq!(other_phase.version, 1);
        assert_eq!(other_repo.version, 1);
        assert!(!one.is_champion, "a new version is never born champion");
    }

    #[test]
    fn lineage_records_the_parent_and_cannot_cross_families() {
        let (state, _root) = test_state();
        let parent = seed_workflow(&state, "repo", "dev-phase", "glm");

        let mut child = new_version("repo", "dev-phase", "glm");
        child.parent_id = Some(parent.id.clone());
        let child = create_workflow_version(&state, child).expect("child");
        assert_eq!(child.parent_id.as_deref(), Some(parent.id.as_str()));
        assert_eq!(child.version, 2);

        let mut crossed = new_version("repo", "dev-phase", "qwen");
        crossed.parent_id = Some(parent.id.clone());
        let err = create_workflow_version(&state, crossed).expect_err("cross-family lineage");
        assert!(err.contains("lineage does not"), "got: {}", err);
    }

    #[test]
    fn a_workflow_needs_a_parseable_hop_and_at_least_one_seat() {
        let (state, _root) = test_state();
        let mut bad_hop = new_version("repo", "dev-phase", "glm");
        bad_hop.hop = "not json".to_string();
        assert!(create_workflow_version(&state, bad_hop)
            .expect_err("malformed hop")
            .contains("hop is not JSON"));

        let mut no_seats = new_version("repo", "dev-phase", "glm");
        no_seats.fan_out = 0;
        assert!(create_workflow_version(&state, no_seats)
            .expect_err("zero fan-out")
            .contains("fanOut must be at least 1"));
    }

    // -------------------------------------------------------------------------------------
    // The gate result
    // -------------------------------------------------------------------------------------

    /// `gate_collected` is ADVISORY and NULLABLE. A gate that prints nothing a parser recognises
    /// is a legitimate gate, so `None` must round-trip as `None` — not 0, not an error — and must
    /// not change anything that depends on the gate.
    #[test]
    fn gate_collected_is_nullable_and_none_is_not_an_error() {
        let (state, _root) = test_state();
        let workflow = seed_workflow(&state, "repo", "dev-phase", "glm");
        let run = start(&state, &workflow.id, r#"{"glm":3}"#);
        assert_eq!(run.gate_exit, None, "a fresh run has no score");
        assert_eq!(run.gate_collected, None);
        assert_eq!(run.ended_at, None);

        let scored = record_workflow_run_gate(&state, &run.id, 0, None).expect("unparseable count");
        assert_eq!(scored.gate_exit, Some(0));
        assert_eq!(
            scored.gate_collected, None,
            "None must stay None: coercing it to 0 would invent a 'collected nothing' report"
        );
        assert!(scored.ended_at.is_some());

        // And it must not block promotion — `gate_exit` alone is the score.
        assert_eq!(promotion_block(&scored, None, None), None);

        // A real count round-trips too, including the meaningful 0.
        let zero = record_workflow_run_gate(&state, &run.id, 0, Some(0)).expect("zero collected");
        assert_eq!(zero.gate_collected, Some(0));
    }

    #[test]
    fn recording_a_gate_result_for_an_unknown_run_is_not_found() {
        let (state, _root) = test_state();
        let err = record_workflow_run_gate(&state, "nope", 0, None).expect_err("unknown run");
        assert!(err.contains("not found"), "got: {}", err);
    }

    // -------------------------------------------------------------------------------------
    // Comparability
    // -------------------------------------------------------------------------------------

    #[test]
    fn parse_seat_map_drops_zero_seats_and_refuses_garbage() {
        assert_eq!(
            parse_seat_map(r#"{"glm":3,"qwen":0}"#).unwrap(),
            parse_seat_map(r#"{"glm":3}"#).unwrap(),
            "an explicit zero and an absent model are the same budget"
        );
        assert!(parse_seat_map("[]").is_err(), "an array is not a budget");
        assert!(parse_seat_map("nope").is_err());
        assert!(parse_seat_map(r#"{"glm":-1}"#).is_err());
        assert!(parse_seat_map(r#"{"glm":"three"}"#).is_err());
    }

    #[test]
    fn budgets_are_comparable_is_key_order_insensitive_and_fails_closed() {
        assert!(budgets_are_comparable(
            r#"{"glm":3,"deepseek":1}"#,
            r#"{"deepseek":1,"glm":3}"#
        ));
        assert!(!budgets_are_comparable(r#"{"glm":3}"#, r#"{"glm":4}"#));
        assert!(
            !budgets_are_comparable(r#"{"glm":3}"#, "garbage"),
            "an unreadable budget must fail closed, never compare equal"
        );
    }

    /// `comparable` is computed from the runs, never supplied by the caller.
    #[test]
    fn record_pref_marks_mismatched_seat_budgets_not_comparable() {
        let (state, _root) = test_state();
        let workflow = seed_workflow(&state, "repo", "dev-phase", "glm");
        let a = scored_run(&state, &workflow.id, r#"{"glm":3}"#, 0);
        let b = scored_run(&state, &workflow.id, r#"{"glm":3}"#, 1);
        let c = scored_run(&state, &workflow.id, r#"{"glm":8}"#, 1);

        let matched = record_workflow_pref(&state, &a.id, &b.id, Some("a was cleaner".into()))
            .expect("matched budgets");
        assert!(matched.comparable);
        assert_eq!(matched.reason.as_deref(), Some("a was cleaner"));

        let mismatched =
            record_workflow_pref(&state, &a.id, &c.id, None).expect("mismatched budgets");
        assert!(
            !mismatched.comparable,
            "different seat budgets must record comparable = 0"
        );
        // It is still recorded: a non-comparable preference is data, it just cannot promote.
        assert_eq!(
            state
                .db
                .with_conn(|conn| conn
                    .query_row("SELECT COUNT(*) FROM workflow_prefs", [], |row| row
                        .get::<_, i64>(0))
                    .map_err(|e| e.to_string()))
                .unwrap(),
            2
        );
    }

    #[test]
    fn a_run_cannot_be_preferred_over_itself() {
        let (state, _root) = test_state();
        let workflow = seed_workflow(&state, "repo", "dev-phase", "glm");
        let run = scored_run(&state, &workflow.id, r#"{"glm":1}"#, 0);
        let err = record_workflow_pref(&state, &run.id, &run.id, None).expect_err("self pref");
        assert!(err.contains("cannot be preferred over itself"), "got: {}", err);
    }

    // -------------------------------------------------------------------------------------
    // Promotion
    // -------------------------------------------------------------------------------------

    #[test]
    fn promote_requires_explicit_human_confirmation() {
        let (state, _root) = test_state();
        let workflow = seed_workflow(&state, "repo", "dev-phase", "glm");
        let run = scored_run(&state, &workflow.id, r#"{"glm":1}"#, 0);
        let err = promote_workflow(&state, &run.id, None, false).expect_err("unconfirmed");
        assert!(err.contains("human-confirmed"), "got: {}", err);
        assert!(champion_ids(&state, "repo", "dev-phase", "glm").is_empty());
    }

    #[test]
    fn promote_seeds_the_first_champion_with_no_preference() {
        let (state, _root) = test_state();
        let workflow = seed_workflow(&state, "repo", "dev-phase", "glm");
        let run = scored_run(&state, &workflow.id, r#"{"glm":1}"#, 0);
        let promoted = promote_workflow(&state, &run.id, None, true).expect("seed champion");
        assert!(promoted.is_champion);
        assert_eq!(
            champion_ids(&state, "repo", "dev-phase", "glm"),
            vec![workflow.id]
        );
    }

    #[test]
    fn promote_refuses_a_run_whose_gate_never_ran() {
        let (state, _root) = test_state();
        let workflow = seed_workflow(&state, "repo", "dev-phase", "glm");
        let run = start(&state, &workflow.id, r#"{"glm":1}"#);
        assert_eq!(run.gate_exit, None);
        let err = promote_workflow(&state, &run.id, None, true).expect_err("NULL gate_exit");
        assert!(err.contains("no gate result"), "got: {}", err);
        assert!(champion_ids(&state, "repo", "dev-phase", "glm").is_empty());
    }

    #[test]
    fn promote_refuses_a_run_whose_gate_failed() {
        let (state, _root) = test_state();
        let workflow = seed_workflow(&state, "repo", "dev-phase", "glm");
        let run = scored_run(&state, &workflow.id, r#"{"glm":1}"#, 1);
        let err = promote_workflow(&state, &run.id, None, true).expect_err("non-zero gate_exit");
        assert!(err.contains("failed its gate (exit 1)"), "got: {}", err);
        assert!(champion_ids(&state, "repo", "dev-phase", "glm").is_empty());
    }

    #[test]
    fn promote_refuses_a_non_comparable_preference() {
        let (state, _root) = test_state();
        let workflow = seed_workflow(&state, "repo", "dev-phase", "glm");
        let challenger = scored_run(&state, &workflow.id, r#"{"glm":3}"#, 0);
        let incumbent_run = scored_run(&state, &workflow.id, r#"{"glm":8}"#, 0);
        let pref = record_workflow_pref(&state, &challenger.id, &incumbent_run.id, None)
            .expect("pref across budgets");
        assert!(!pref.comparable);

        let err = promote_workflow(&state, &challenger.id, Some(&pref.id), true)
            .expect_err("non-comparable pref");
        assert!(err.contains("not comparable"), "got: {}", err);
        assert!(champion_ids(&state, "repo", "dev-phase", "glm").is_empty());
    }

    #[test]
    fn promote_refuses_a_preference_that_names_a_different_winner() {
        let (state, _root) = test_state();
        let workflow = seed_workflow(&state, "repo", "dev-phase", "glm");
        let winner = scored_run(&state, &workflow.id, r#"{"glm":3}"#, 0);
        let loser = scored_run(&state, &workflow.id, r#"{"glm":3}"#, 0);
        let pref = record_workflow_pref(&state, &winner.id, &loser.id, None).expect("pref");
        assert!(pref.comparable);

        let err =
            promote_workflow(&state, &loser.id, Some(&pref.id), true).expect_err("wrong winner");
        assert!(err.contains("as the winner, not run"), "got: {}", err);
    }

    #[test]
    fn promote_refuses_to_displace_an_incumbent_without_a_comparable_preference() {
        let (state, _root) = test_state();
        let old = seed_workflow(&state, "repo", "dev-phase", "glm");
        let old_run = scored_run(&state, &old.id, r#"{"glm":3}"#, 0);
        promote_workflow(&state, &old_run.id, None, true).expect("seed champion");

        let new = seed_workflow(&state, "repo", "dev-phase", "glm");
        let new_run = scored_run(&state, &new.id, r#"{"glm":3}"#, 0);
        let err = promote_workflow(&state, &new_run.id, None, true).expect_err("no pref");
        assert!(err.contains("already the champion"), "got: {}", err);
        assert_eq!(champion_ids(&state, "repo", "dev-phase", "glm"), vec![old.id]);
    }

    /// The demote and the promote are one transaction: afterwards there is EXACTLY one champion,
    /// and it is the challenger.
    #[test]
    fn promote_demotes_the_incumbent_in_one_transaction() {
        let (state, _root) = test_state();
        let old = seed_workflow(&state, "repo", "dev-phase", "glm");
        let old_run = scored_run(&state, &old.id, r#"{"glm":3}"#, 0);
        promote_workflow(&state, &old_run.id, None, true).expect("seed champion");

        let new = seed_workflow(&state, "repo", "dev-phase", "glm");
        let new_run = scored_run(&state, &new.id, r#"{"glm":3}"#, 0);
        let pref = record_workflow_pref(&state, &new_run.id, &old_run.id, Some("faster".into()))
            .expect("pref");
        assert!(pref.comparable);

        let promoted =
            promote_workflow(&state, &new_run.id, Some(&pref.id), true).expect("promote");
        assert!(promoted.is_champion);
        assert_eq!(
            champion_ids(&state, "repo", "dev-phase", "glm"),
            vec![new.id],
            "exactly one champion, and it is the challenger"
        );
        assert!(!get_workflow(&state, &old.id).unwrap().is_champion);
    }

    /// A refused promotion must leave the incumbent exactly as it was — the refusal happens before
    /// any UPDATE, and the whole thing is one transaction either way.
    #[test]
    fn a_refused_promotion_leaves_the_incumbent_in_place() {
        let (state, _root) = test_state();
        let old = seed_workflow(&state, "repo", "dev-phase", "glm");
        let old_run = scored_run(&state, &old.id, r#"{"glm":3}"#, 0);
        promote_workflow(&state, &old_run.id, None, true).expect("seed champion");

        let new = seed_workflow(&state, "repo", "dev-phase", "glm");
        let failing = scored_run(&state, &new.id, r#"{"glm":3}"#, 2);
        let pref = record_workflow_pref(&state, &failing.id, &old_run.id, None).expect("pref");
        assert!(pref.comparable);

        promote_workflow(&state, &failing.id, Some(&pref.id), true).expect_err("red gate");
        assert_eq!(champion_ids(&state, "repo", "dev-phase", "glm"), vec![old.id]);
    }

    /// Re-promoting the current champion is a no-op, not a self-displacement error.
    #[test]
    fn re_promoting_the_current_champion_is_idempotent() {
        let (state, _root) = test_state();
        let workflow = seed_workflow(&state, "repo", "dev-phase", "glm");
        let run = scored_run(&state, &workflow.id, r#"{"glm":3}"#, 0);
        promote_workflow(&state, &run.id, None, true).expect("first");
        promote_workflow(&state, &run.id, None, true).expect("again");
        assert_eq!(
            champion_ids(&state, "repo", "dev-phase", "glm"),
            vec![workflow.id]
        );
    }

    // -------------------------------------------------------------------------------------
    // Rejection and the halt
    // -------------------------------------------------------------------------------------

    #[test]
    fn a_rejection_must_record_a_reason() {
        let (state, _root) = test_state();
        let workflow = seed_workflow(&state, "repo", "dev-phase", "glm");
        let run = scored_run(&state, &workflow.id, r#"{"glm":3}"#, 0);
        let err = reject_workflow_run(&state, &run.id, "   ").expect_err("blank reason");
        assert!(err.contains("must record a reason"), "got: {}", err);
        assert_eq!(
            workflow_search_state(&state, "repo", "dev-phase", "glm")
                .unwrap()
                .consecutive_rejections,
            0
        );
    }

    /// Three consecutive rejections halt the search, the halt is DATA, and the halt is ENFORCED:
    /// no further run can start in that family.
    #[test]
    fn three_consecutive_rejections_halt_the_search_and_block_new_runs() {
        let (state, _root) = test_state();
        let workflow = seed_workflow(&state, "repo", "dev-phase", "glm");

        for (index, expected) in [(1_i64, false), (2, false), (3, true)] {
            let run = scored_run(&state, &workflow.id, r#"{"glm":3}"#, 1);
            let after = reject_workflow_run(&state, &run.id, &format!("reason {}", index))
                .expect("reject");
            assert_eq!(after.consecutive_rejections, index);
            assert_eq!(
                after.halted, expected,
                "halt at {} (limit {})",
                index, REJECTION_STRIKE_LIMIT
            );
            assert_eq!(after.strike_limit, REJECTION_STRIKE_LIMIT);
        }

        let halted = workflow_search_state(&state, "repo", "dev-phase", "glm").unwrap();
        assert!(halted.halted);
        assert_eq!(
            halted.recent_rejections,
            vec!["reason 3", "reason 2", "reason 1"],
            "newest first, so a halted search can show why"
        );

        let err = start_workflow_run(
            &state,
            NewWorkflowRun {
                workflow_id: workflow.id.clone(),
                plan_id: None,
                seed: 0,
                seats_held: r#"{"glm":3}"#.to_string(),
                fleet_busy: "{}".to_string(),
            },
        )
        .expect_err("halted search must refuse a new run");
        assert!(err.contains("is halted after 3 consecutive rejections"), "got: {}", err);
    }

    /// The halt is per `(repo, phase_type, family)`. Rejections in one family must not stop
    /// another — otherwise one bad model family freezes the whole arena.
    #[test]
    fn the_halt_is_scoped_to_one_repo_phase_type_family() {
        let (state, _root) = test_state();
        let glm = seed_workflow(&state, "repo", "dev-phase", "glm");
        let qwen = seed_workflow(&state, "repo", "dev-phase", "qwen");
        for index in 1..=3 {
            let run = scored_run(&state, &glm.id, r#"{"glm":3}"#, 1);
            reject_workflow_run(&state, &run.id, &format!("glm {}", index)).expect("reject");
        }
        assert!(workflow_search_state(&state, "repo", "dev-phase", "glm")
            .unwrap()
            .halted);
        let qwen_state = workflow_search_state(&state, "repo", "dev-phase", "qwen").unwrap();
        assert_eq!(qwen_state.consecutive_rejections, 0);
        assert!(!qwen_state.halted);
        // And a run in the untouched family still starts.
        start(&state, &qwen.id, r#"{"qwen":3}"#);
    }

    /// "CONSECUTIVE" is the whole point: a promotion in the family resets the streak, which is also
    /// the documented way out of a halt.
    #[test]
    fn a_promotion_resets_the_rejection_streak() {
        let (state, _root) = test_state();
        let workflow = seed_workflow(&state, "repo", "dev-phase", "glm");
        for index in 1..=2 {
            let run = scored_run(&state, &workflow.id, r#"{"glm":3}"#, 1);
            reject_workflow_run(&state, &run.id, &format!("no {}", index)).expect("reject");
        }
        assert_eq!(
            workflow_search_state(&state, "repo", "dev-phase", "glm")
                .unwrap()
                .consecutive_rejections,
            2
        );

        let good = scored_run(&state, &workflow.id, r#"{"glm":3}"#, 0);
        promote_workflow(&state, &good.id, None, true).expect("promote");

        let after = workflow_search_state(&state, "repo", "dev-phase", "glm").unwrap();
        assert_eq!(after.consecutive_rejections, 0, "a promotion resets the streak");
        assert!(!after.halted);
        assert_eq!(after.champion_workflow_id.as_deref(), Some(workflow.id.as_str()));

        // Two more rejections do not re-halt: the count restarts after the promotion.
        for index in 1..=2 {
            let run = scored_run(&state, &workflow.id, r#"{"glm":3}"#, 1);
            reject_workflow_run(&state, &run.id, &format!("again {}", index)).expect("reject");
        }
        assert!(!workflow_search_state(&state, "repo", "dev-phase", "glm")
            .unwrap()
            .halted);
    }

    /// A halted search resumes after a promotion — the only escape, and it has to actually work.
    #[test]
    fn a_promotion_un_halts_a_halted_search() {
        let (state, _root) = test_state();
        let workflow = seed_workflow(&state, "repo", "dev-phase", "glm");
        let mut last_green = None;
        for index in 1..=3 {
            let green = scored_run(&state, &workflow.id, r#"{"glm":3}"#, 0);
            last_green = Some(green.clone());
            reject_workflow_run(&state, &green.id, &format!("no {}", index)).expect("reject");
        }
        assert!(workflow_search_state(&state, "repo", "dev-phase", "glm")
            .unwrap()
            .halted);

        promote_workflow(&state, &last_green.unwrap().id, None, true).expect("promote to resume");
        assert!(!workflow_search_state(&state, "repo", "dev-phase", "glm")
            .unwrap()
            .halted);
        start(&state, &workflow.id, r#"{"glm":3}"#);
    }

    // -------------------------------------------------------------------------------------
    // Reads
    // -------------------------------------------------------------------------------------

    #[test]
    fn list_workflows_filters_on_any_part_of_the_champion_key() {
        let (state, _root) = test_state();
        seed_workflow(&state, "repo", "dev-phase", "glm");
        seed_workflow(&state, "repo", "dev-phase", "glm");
        seed_workflow(&state, "repo", "ui-phase", "glm");
        seed_workflow(&state, "other", "dev-phase", "qwen");

        assert_eq!(list_workflows(&state, None, None, None).unwrap().len(), 4);
        assert_eq!(
            list_workflows(&state, Some("repo"), None, None).unwrap().len(),
            3
        );
        assert_eq!(
            list_workflows(&state, Some("repo"), Some("dev-phase"), Some("glm"))
                .unwrap()
                .len(),
            2
        );
        let newest_first: Vec<i64> = list_workflows(&state, Some("repo"), Some("dev-phase"), Some("glm"))
            .unwrap()
            .into_iter()
            .map(|workflow| workflow.version)
            .collect();
        assert_eq!(newest_first, vec![2, 1]);
    }

    #[test]
    fn list_workflow_runs_distinguishes_no_runs_from_no_such_workflow() {
        let (state, _root) = test_state();
        let workflow = seed_workflow(&state, "repo", "dev-phase", "glm");
        assert!(list_workflow_runs(&state, &workflow.id).unwrap().is_empty());
        start(&state, &workflow.id, r#"{"glm":3}"#);
        assert_eq!(list_workflow_runs(&state, &workflow.id).unwrap().len(), 1);
        assert!(list_workflow_runs(&state, "nope")
            .expect_err("unknown workflow")
            .contains("not found"));
    }

    #[test]
    fn starting_a_run_refuses_an_unreadable_seat_budget() {
        let (state, _root) = test_state();
        let workflow = seed_workflow(&state, "repo", "dev-phase", "glm");
        let bad = start_workflow_run(
            &state,
            NewWorkflowRun {
                workflow_id: workflow.id.clone(),
                plan_id: None,
                seed: 0,
                seats_held: "nope".to_string(),
                fleet_busy: "{}".to_string(),
            },
        )
        .expect_err("unreadable seatsHeld");
        assert!(bad.contains("seatsHeld"), "got: {}", bad);

        let bad_fleet = start_workflow_run(
            &state,
            NewWorkflowRun {
                workflow_id: workflow.id.clone(),
                plan_id: None,
                seed: 0,
                seats_held: "{}".to_string(),
                fleet_busy: "[]".to_string(),
            },
        )
        .expect_err("unreadable fleetBusy");
        assert!(bad_fleet.contains("fleetBusy"), "got: {}", bad_fleet);
    }
}
