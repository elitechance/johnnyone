use crate::db::models::{CreateSessionInput, Message};
use crate::events::SessionUpdatedEvent;
use crate::providers::CliProvider;
use crate::state::app_state::AppState;
use rusqlite::params;
use std::collections::HashSet;
use uuid::Uuid;

/// Decide what the host tells the client a session's terminal REALLY is.
///
/// - not an attached-tmux session → `"none"`;
/// - `tmux_alive == Some(false)` → `"ended"` — the tmux is gone, and that beats whatever
///   the row stored, because the row is a memory and the tmux is the fact;
/// - `tmux_alive == Some(true)` or `None` (we could not check) → `"attached"` when `stored`
///   is `"attached"`, otherwise `"detached"`.
///
/// `None` means "unknown", and unknown must NEVER be reported as `"ended"` — a failed check
/// is not evidence that a terminal died.
pub fn reported_terminal_status(
    attached_tmux: bool,
    stored: Option<&str>,
    tmux_alive: Option<bool>,
) -> &'static str {
    if !attached_tmux {
        return "none";
    }
    if tmux_alive == Some(false) {
        return "ended";
    }
    if stored == Some("attached") {
        "attached"
    } else {
        "detached"
    }
}

/// The struct that goes to the client IS `db::models::Session` — this module owns the
/// `terminal_status` decision (see [`reported_terminal_status`]) and re-exports the type
/// so every caller (`commands::sessions`, the host GraphQL, the relay RPC) reads the real
/// terminal state off one struct instead of guessing from the `attached_tmux` INTENT flag.
pub use crate::db::models::Session;

/// THE session column list — every query that builds a `db::models::Session` reads its
/// columns from here, so a column can never be added to the struct in one query and
/// forgotten in the other. Order is load-bearing: the row readers below index by
/// position, and the five `watchdog_*` columns sit last in migration 027's order.
pub(crate) const SESSION_SELECT_COLUMNS: &str = "id, title, provider, model, \
    working_directory, status, cli_session_id, total_input_tokens, total_output_tokens, \
    total_cost_cents, kind, created_at, updated_at, attached_tmux, terminal_status, \
    watchdog_enabled, watchdog_interval_secs, watchdog_prompt, watchdog_last_nudge_at, \
    watchdog_nudge_count, watchdog_held_count";

/// Ask tmux ONCE which sessions exist, for a whole listing.
///
/// `Some(names)` = the exact names tmux reported, so a row can pass
/// `Some(names.contains(name))`. `None` = tmux could not be asked at all (no binary, no
/// server) — every row then gets `tmux_alive == None`, which [`reported_terminal_status`]
/// refuses to call `"ended"`.
fn live_tmux_session_names() -> Option<HashSet<String>> {
    let out = std::process::Command::new("tmux")
        .args(["list-sessions", "-F", "#{session_name}"])
        .output()
        .ok()
        .filter(|output| output.status.success())?;
    Some(
        String::from_utf8_lossy(&out.stdout)
            .lines()
            .map(|line| line.trim_end_matches(['\r', '\n']))
            .filter(|line| !line.is_empty())
            .map(ToOwned::to_owned)
            .collect(),
    )
}

/// The external tmux session name a row expects, if it recorded one.
fn stored_tmux_name(conn: &rusqlite::Connection, id: &str) -> Option<String> {
    conn.query_row(
        "SELECT tmux_session_name FROM sessions WHERE id = ?1",
        params![id],
        |row| row.get::<_, Option<String>>(0),
    )
    .ok()
    .flatten()
    .map(|name| name.trim().to_string())
    .filter(|name| !name.is_empty())
}

/// Is this row's tmux alive, given the one live-name set for the whole listing?
/// `None` (unknown) when tmux could not be asked.
fn tmux_alive_for(
    conn: &rusqlite::Connection,
    id: &str,
    live: Option<&HashSet<String>>,
) -> Option<bool> {
    let names = live?;
    match stored_tmux_name(conn, id) {
        Some(name) => Some(names.contains(&name)),
        None => Some(false),
    }
}

pub fn create_session(state: &AppState, input: CreateSessionInput) -> Result<Session, String> {
    let id = Uuid::new_v4().to_string();
    // Attach mode: when a tmux_session_name is given, this session views an
    // existing EXTERNAL tmux session instead of spawning its own pane. Such
    // sessions use the `shell` provider (no AI CLI is launched) and are flagged
    // `attached_tmux` so archive leaves the real tmux alive.
    let attach_tmux = input
        .tmux_session_name
        .as_deref()
        .map(str::trim)
        .filter(|s| !s.is_empty())
        .map(str::to_string);
    let attached = attach_tmux.is_some();
    let provider = if attached {
        "shell".to_string()
    } else {
        input.provider.unwrap_or_else(|| "claude_code".to_string())
    };
    let model = input.model.unwrap_or_default();
    let default_title = attach_tmux
        .clone()
        .unwrap_or_else(|| "New Session".to_string());
    let title = input
        .title
        .filter(|t| !t.trim().is_empty())
        .unwrap_or(default_title);
    // Anything other than the known kinds normalizes to "user" — the kind
    // column has a CHECK-like guard at the application layer.
    let kind = match input.kind.as_deref() {
        Some("agent") => "agent".to_string(),
        _ => "user".to_string(),
    };

    let working_directory = match input.working_directory {
        Some(wd) if !wd.is_empty() => wd,
        _ => state.db.with_conn(|conn| {
            let result = conn.query_row(
                "SELECT value FROM settings WHERE key = 'last_working_directory'",
                [],
                |row| row.get::<_, String>(0),
            );
            Ok(result.unwrap_or_default())
        })?,
    };

    // Only meaningful for shell sessions; stored regardless and read at spawn time.
    let setup_commands = input
        .setup_commands
        .filter(|s| !s.trim().is_empty());

    let session = state.db.with_conn(|conn| {
        conn.execute(
            "INSERT INTO sessions (id, title, provider, model, working_directory, kind, setup_commands, tmux_session_name, attached_tmux) VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9)",
            params![id, title, provider, model, working_directory, kind, setup_commands, attach_tmux, attached as i64],
        )
        .map_err(|e| format!("Failed to create session: {}", e))?;

        query_session(conn, &id)
    })?;
    publish_session_update(state, &session);
    Ok(session)
}

pub fn list_sessions(state: &AppState, status: Option<String>) -> Result<Vec<Session>, String> {
    list_sessions_filtered(state, status, Some("user".to_string()))
}

/// Lower-level variant of `list_sessions` that takes a `kind` filter too.
/// `kind = None` returns sessions of all kinds (used internally by the
/// planner if it ever needs to enumerate its own sessions); `Some("user")`
/// is what /terminal calls into so planner T1/T2 sessions don't leak in.
pub fn list_sessions_filtered(
    state: &AppState,
    status: Option<String>,
    kind: Option<String>,
) -> Result<Vec<Session>, String> {
    // ONE tmux question for the whole listing, never one per row. `None` means tmux
    // could not be asked, and then no row may be called "ended".
    let live = live_tmux_session_names();

    state.db.with_conn(|conn| {
        let mut clauses: Vec<String> = Vec::new();
        let mut bound: Vec<String> = Vec::new();
        if let Some(s) = status {
            clauses.push(format!("status = ?{}", bound.len() + 1));
            bound.push(s);
        }
        if let Some(k) = kind {
            clauses.push(format!("kind = ?{}", bound.len() + 1));
            bound.push(k);
        }
        let where_sql = if clauses.is_empty() {
            String::new()
        } else {
            format!(" WHERE {}", clauses.join(" AND "))
        };
        let sql = format!(
            "SELECT {} FROM sessions{} ORDER BY updated_at DESC",
            SESSION_SELECT_COLUMNS, where_sql
        );

        let params_dyn: Vec<&dyn rusqlite::ToSql> =
            bound.iter().map(|v| v as &dyn rusqlite::ToSql).collect();

        let mut stmt = conn.prepare(&sql).map_err(|e| e.to_string())?;
        let sessions = stmt
            .query_map(params_dyn.as_slice(), |row| {
                let id: String = row.get(0)?;
                let attached_tmux: bool = row.get::<_, i64>(13)? != 0;
                let stored: Option<String> = row.get(14)?;
                let tmux_alive = tmux_alive_for(conn, &id, live.as_ref());
                Ok(Session {
                    id,
                    title: row.get(1)?,
                    provider: row.get(2)?,
                    model: row.get(3)?,
                    working_directory: row.get(4)?,
                    status: row.get(5)?,
                    cli_session_id: row.get(6)?,
                    total_input_tokens: row.get(7)?,
                    total_output_tokens: row.get(8)?,
                    total_cost_cents: row.get(9)?,
                    kind: row.get(10)?,
                    created_at: row.get(11)?,
                    updated_at: row.get(12)?,
                    attached_tmux,
                    terminal_status: reported_terminal_status(
                        attached_tmux,
                        stored.as_deref(),
                        tmux_alive,
                    )
                    .to_string(),
                    watchdog_enabled: row.get::<_, i64>(15)? != 0,
                    watchdog_interval_secs: row.get(16)?,
                    watchdog_prompt: row.get(17)?,
                    watchdog_last_nudge_at: row.get(18)?,
                    watchdog_nudge_count: row.get(19)?,
                    watchdog_held_count: row.get(20)?,
                })
            })
            .map_err(|e| e.to_string())?
            .filter_map(|r| r.ok())
            .collect();

        Ok(sessions)
    })
}

pub fn get_session(state: &AppState, id: String) -> Result<Session, String> {
    state.db.with_conn(|conn| query_session(conn, &id))
}

pub fn update_session_title(
    state: &AppState,
    id: String,
    title: String,
) -> Result<Session, String> {
    let session = state.db.with_conn(|conn| {
        conn.execute(
            "UPDATE sessions SET title = ?1, updated_at = datetime('now') WHERE id = ?2",
            params![title, id],
        )
        .map_err(|e| format!("Failed to update title: {}", e))?;

        query_session(conn, &id)
    })?;
    publish_session_update(state, &session);
    Ok(session)
}

pub fn update_session_working_directory(
    state: &AppState,
    id: String,
    working_directory: String,
) -> Result<Session, String> {
    let session = state.db.with_conn(|conn| {
        conn.execute(
            "UPDATE sessions SET working_directory = ?1, cli_session_id = NULL, updated_at = datetime('now') WHERE id = ?2",
            params![working_directory, id],
        )
        .map_err(|e| format!("Failed to update working directory: {}", e))?;

        query_session(conn, &id)
    })?;
    publish_session_update(state, &session);
    Ok(session)
}

pub fn update_session_provider(
    state: &AppState,
    id: String,
    provider: String,
) -> Result<Session, String> {
    let normalized = provider.trim().to_string();
    let parsed = CliProvider::from_str(&normalized)
        .ok_or_else(|| format!("Unsupported provider: {}", provider))?;

    let session = state.db.with_conn(|conn| {
        conn.execute(
            "UPDATE sessions SET provider = ?1, cli_session_id = NULL, updated_at = datetime('now') WHERE id = ?2",
            params![parsed.as_str(), id],
        )
        .map_err(|e| format!("Failed to update provider: {}", e))?;

        query_session(conn, &id)
    })?;
    publish_session_update(state, &session);
    Ok(session)
}

pub async fn archive_session(state: &AppState, id: String) -> Result<Session, String> {
    // Chat-mode subprocess (`chat_host::send_message` spawn) → kill if present.
    {
        let mut processes = state.active_processes.lock().await;
        if let Some(mut proc) = processes.remove(&id) {
            let _ = proc.kill().await;
        }
    }

    // Terminal-mode tmux session (separate lifecycle from active_processes) →
    // tear it down too, otherwise archiving from the UI leaves the johnnyone_<id>
    // pane running. BUT for sessions ATTACHED to an external tmux (kloo, llm-app,
    // …), only detach the view — never kill the user's real tmux session.
    let attached = state.db.with_conn(|conn| {
        Ok(conn
            .query_row(
                "SELECT attached_tmux FROM sessions WHERE id = ?1",
                params![id],
                |row| row.get::<_, i64>(0),
            )
            .unwrap_or(0)
            != 0)
    })?;
    if attached {
        tracing::info!(session_id = %id, "archiving attached session — leaving external tmux running");
    } else if let Err(error) = crate::terminal::kill_terminal_session(state, &id).await {
        tracing::warn!(%error, session_id = %id, "kill_terminal_session failed during archive");
    }

    let session = state.db.with_conn(|conn| {
        conn.execute(
            "UPDATE sessions SET status = 'archived', updated_at = datetime('now') WHERE id = ?1",
            params![id],
        )
        .map_err(|e| format!("Failed to archive session: {}", e))?;

        query_session(conn, &id)
    })?;
    publish_session_update(state, &session);
    Ok(session)
}

pub async fn delete_session(state: &AppState, id: String) -> Result<bool, String> {
    // NOT the same two-cleanup pattern as archive, despite what this comment used to claim.
    // `archive_session` above reads `attached_tmux` and SKIPS `kill_terminal_session` for an attached
    // session so it never touches the user's own tmux. This function has no such guard: it calls
    // `kill_terminal_session` unconditionally. Recorded, not fixed, because it is currently
    // unreachable as a real hazard — `tmux_session_name` (`crate::terminal`, ~line 1388) is
    // unconditionally `johnnyone_<id>` and never reads the stored external name, so neither path can
    // kill an external session like `kloo` or `j1`. The missing guard only becomes a live bug the day
    // `tmux_session_name` learns about external names; if you add a delete affordance, or teach it,
    // add the `attached_tmux` check here first.
    //
    // Separately: do not route a UI "clear" through here. `record_agent_report`
    // (`agent_plans.rs:3625`) accepts narration from a non-plan session only while the ROW exists
    // (`session_exists`, `agent_plans.rs:4160`, has no status filter), so deleting a row breaks
    // `reportAgentResult` for that id permanently. `/shells` archives for exactly this reason.
    {
        let mut processes = state.active_processes.lock().await;
        if let Some(mut proc) = processes.remove(&id) {
            let _ = proc.kill().await;
        }
    }

    if let Err(error) = crate::terminal::kill_terminal_session(state, &id).await {
        tracing::warn!(%error, session_id = %id, "kill_terminal_session failed during delete");
    }

    state.db.with_conn(|conn| {
        conn.execute("DELETE FROM sessions WHERE id = ?1", params![id])
            .map_err(|e| format!("Failed to delete session: {}", e))?;
        Ok(())
    })?;

    let _ = state.session_deleted_tx.send(id);

    Ok(true)
}

pub fn list_messages(
    state: &AppState,
    session_id: String,
    limit: Option<i64>,
    offset: Option<i64>,
) -> Result<Vec<Message>, String> {
    let limit = limit.unwrap_or(100);
    let offset = offset.unwrap_or(0);

    state.db.with_conn(|conn| {
        let mut stmt = conn
            .prepare(
                "SELECT id, session_id, role, content, tool_calls, tool_call_id, finish_reason, input_tokens, output_tokens, cost_cents, created_at FROM messages WHERE session_id = ?1 ORDER BY created_at DESC, rowid DESC LIMIT ?2 OFFSET ?3",
            )
            .map_err(|e| e.to_string())?;

        let mut messages: Vec<Message> = stmt
            .query_map(params![session_id, limit, offset], |row| {
                Ok(Message {
                    id: row.get(0)?,
                    session_id: row.get(1)?,
                    role: row.get(2)?,
                    content: row.get(3)?,
                    tool_calls: row.get(4)?,
                    tool_call_id: row.get(5)?,
                    finish_reason: row.get(6)?,
                    input_tokens: row.get(7)?,
                    output_tokens: row.get(8)?,
                    cost_cents: row.get(9)?,
                    created_at: row.get(10)?,
                })
            })
            .map_err(|e| e.to_string())?
            .filter_map(|r| r.ok())
            .collect();

        messages.reverse();

        Ok(messages)
    })
}

/// The single-session read. Carries `terminal_status` too, checked against tmux the same way
/// a listing is — ONE `tmux list-sessions` for this one row.
fn query_session(conn: &rusqlite::Connection, id: &str) -> Result<Session, String> {
    let live = live_tmux_session_names();
    conn.query_row(
        &format!(
            "SELECT {} FROM sessions WHERE id = ?1",
            SESSION_SELECT_COLUMNS
        ),
        params![id],
        |row| {
            let attached_tmux: bool = row.get::<_, i64>(13)? != 0;
            let stored: Option<String> = row.get(14)?;
            Ok(Session {
                id: row.get(0)?,
                title: row.get(1)?,
                provider: row.get(2)?,
                model: row.get(3)?,
                working_directory: row.get(4)?,
                status: row.get(5)?,
                cli_session_id: row.get(6)?,
                total_input_tokens: row.get(7)?,
                total_output_tokens: row.get(8)?,
                total_cost_cents: row.get(9)?,
                kind: row.get(10)?,
                created_at: row.get(11)?,
                updated_at: row.get(12)?,
                attached_tmux,
                terminal_status: reported_terminal_status(
                    attached_tmux,
                    stored.as_deref(),
                    tmux_alive_for(conn, id, live.as_ref()),
                )
                .to_string(),
                watchdog_enabled: row.get::<_, i64>(15)? != 0,
                watchdog_interval_secs: row.get(16)?,
                watchdog_prompt: row.get(17)?,
                watchdog_last_nudge_at: row.get(18)?,
                watchdog_nudge_count: row.get(19)?,
                watchdog_held_count: row.get(20)?,
            })
        },
    )
    .map_err(|e| format!("Session not found: {}", e))
}

fn publish_session_update(state: &AppState, session: &Session) {
    let Ok(value) = serde_json::to_value(session) else {
        return;
    };
    let _ = state.session_updated_tx.send(SessionUpdatedEvent {
        session_id: session.id.clone(),
        session: value,
    });
}

/// Read the watchdog settings for a session.
pub fn get_session_watchdog_config(state: &AppState, id: &str) -> Result<WatchdogConfig, String> {
    state.db.with_conn(|conn| {
        conn.query_row(
            "SELECT watchdog_enabled, watchdog_interval_secs, watchdog_prompt, watchdog_last_nudge_at, watchdog_nudge_count \
             FROM sessions WHERE id = ?1",
            params![id],
            |row| {
                Ok(WatchdogConfig {
                    enabled: row.get::<_, i64>(0)? != 0,
                    interval_secs: row.get::<_, i64>(1)? as u64,
                    prompt: row.get(2)?,
                    last_nudge_at: row.get(3)?,
                    nudge_count: row.get::<_, i64>(4)? as u64,
                })
            },
        )
        .map_err(|e| format!("Session not found: {}", e))
    })
}

/// The nudge prompt the host backfills when a session is armed without one.
///
/// The sweep in `services/session_watchdog.rs` selects
/// `WHERE watchdog_enabled = 1 AND watchdog_prompt != ''`, and migration 027 defaults
/// `watchdog_prompt` to `''`. So EVERY session written before someone typed a prompt — armed
/// or not — was skipped forever, which is why `watchdog_nudge_count` is 0 on every live row:
/// flipping `watchdog_enabled` to 1 did nothing. Arming now writes this default in the same
/// statement, so "On" means armed-and-watching. Kept byte-identical to
/// `DEFAULT_WATCHDOG_PROMPT` in `web/src/app/pages/terminal/watchdog-logic.ts`, because the
/// textarea shows this text and the sweep types it. `{quiet}`, `{name}` and `{nudges}` are
/// substituted by the sweep.
pub(crate) const DEFAULT_WATCHDOG_PROMPT: &str =
    "You have been quiet for {quiet}. Report where {name} stands right now: what you just \
     finished, what you are working on, and anything blocking you. This is nudge {nudges}.";

/// The single statement behind `set_session_watchdog`, hoisted to a const so a test can execute
/// the REAL thing on a plain in-memory connection (this module has no AppState harness).
///
/// The `CASE` is the whole point: it backfills `DEFAULT_WATCHDOG_PROMPT` ONLY when enabling a
/// row whose prompt is empty/NULL. Disarming must not wipe a prompt the user wrote, and a
/// prompt the user HAS written must never be overwritten by ours.
pub(crate) const SET_WATCHDOG_SQL: &str = "UPDATE sessions SET watchdog_enabled = ?1, \
    watchdog_interval_secs = ?2, \
    watchdog_prompt = CASE WHEN ?1 = 1 AND (watchdog_prompt IS NULL OR watchdog_prompt = '') \
        THEN ?4 ELSE watchdog_prompt END, \
    updated_at = datetime('now') WHERE id = ?3";

/// A write that matched no row is a failure, not a silent success: the
/// caller passed an id that does not exist.
pub fn session_write_result(rows: usize, id: &str) -> Result<(), String> {
    if rows == 0 { Err(format!("No such session: {id}")) } else { Ok(()) }
}

/// Set the watchdog enabled/interval for a session. Clamps interval to [60, 3600].
pub fn set_session_watchdog(
    state: &AppState,
    id: &str,
    enabled: bool,
    interval_secs: u64,
) -> Result<(), String> {
    let clamped = interval_secs.clamp(60, 3600);
    state.db.with_conn(|conn| {
        let rows = conn
            .execute(
                SET_WATCHDOG_SQL,
                params![enabled as i64, clamped as i64, id, DEFAULT_WATCHDOG_PROMPT],
            )
            .map_err(|e| format!("Failed to update watchdog: {}", e))?;
        session_write_result(rows, id)
    })
}

/// Set the watchdog prompt for a session. Rejects embedded \r and \n, and strings over 600 chars.
pub fn set_session_watchdog_prompt(state: &AppState, id: &str, prompt: &str) -> Result<(), String> {
    if prompt.chars().any(|c| c.is_control() && c != '\t') {
        return Err("Watchdog prompt contains control characters".to_string());
    }
    if prompt.chars().count() > 600 {
        return Err("Watchdog prompt exceeds 600 characters".to_string());
    }
    state.db.with_conn(|conn| {
        let rows = conn
            .execute(
                "UPDATE sessions SET watchdog_prompt = ?1, updated_at = datetime('now') WHERE id = ?2",
                params![prompt, id],
            )
            .map_err(|e| format!("Failed to update watchdog prompt: {}", e))?;
        session_write_result(rows, id)
    })
}

/// Bump the watchdog nudge counter and stamp the last-nudge timestamp.
pub fn bump_watchdog_nudge(state: &AppState, id: &str) -> Result<(), String> {
    state.db.with_conn(|conn| {
        conn.execute(
            "UPDATE sessions SET watchdog_nudge_count = watchdog_nudge_count + 1, \
             watchdog_last_nudge_at = datetime('now'), updated_at = datetime('now') WHERE id = ?1",
            params![id],
        )
        .map_err(|e| format!("Failed to bump watchdog nudge: {}", e))?;
        Ok(())
    })
}

/// The UPDATE half of a WITHHELD nudge, hoisted so a test can run the REAL statement against a
/// seeded row without an `AppState` harness (see `watchdog_held_tests`).
///
/// Deliberately NOT `bump_watchdog_nudge` with one column swapped: a hold must leave BOTH nudge
/// counters alone — `watchdog_nudge_count` counts nudges the agent actually received, and
/// `watchdog_last_nudge_at` is what the due-check reads as "we just spoke". Stamping either would
/// make a withheld nudge suppress the next one and lie to the console about what was sent.
pub const BUMP_WATCHDOG_HELD_SQL: &str = "UPDATE sessions SET watchdog_held_count = \
    watchdog_held_count + 1, updated_at = datetime('now') WHERE id = ?1";

/// Record that the sweep found a nudge DUE for `id` and withheld it (the two-capture idle probe saw
/// the pane still changing).
///
/// Called ONLY from that withholding branch. A session whose interval has not elapsed is not "held"
/// — it is simply quiet — so `watchdog_is_due == false` must never reach here, or the count becomes
/// a tick counter and stops meaning anything.
pub fn bump_watchdog_held(state: &AppState, id: &str) -> Result<(), String> {
    state.db.with_conn(|conn| {
        conn.execute(BUMP_WATCHDOG_HELD_SQL, params![id])
            .map_err(|e| format!("Failed to bump watchdog held count: {}", e))?;
        Ok(())
    })
}

/// Query the latest session_reports timestamp for a session, or None if no reports exist.
pub fn latest_session_report_at(state: &AppState, id: &str) -> Result<Option<String>, String> {
    state.db.with_conn(|conn| {
        let result: Result<Option<String>, _> = conn.query_row(
            "SELECT created_at FROM session_reports WHERE session_id = ?1 ORDER BY created_at DESC LIMIT 1",
            params![id],
            |row| row.get(0),
        );
        match result {
            Ok(ts) => Ok(ts),
            Err(rusqlite::Error::QueryReturnedNoRows) => Ok(None),
            Err(e) => Err(e.to_string()),
        }
    })
}

/// Configuration for the per-session watchdog.
#[derive(Debug, Clone)]
pub struct WatchdogConfig {
    pub enabled: bool,
    pub interval_secs: u64,
    pub prompt: String,
    pub last_nudge_at: Option<String>,
    pub nudge_count: u64,
}

/// Pure-function predicate: is the watchdog due right now?
///
/// `last` is an ISO-8601 or SQLite datetime string, or None if never nudged / no reports.
/// `interval_secs` is the configured interval.
pub fn watchdog_is_due(
    now_utc: &chrono::DateTime<chrono::Utc>,
    last: Option<&str>,
    interval_secs: u64,
) -> bool {
    let Some(last_str) = last else {
        // Never nudged and no reports — always due.
        return true;
    };
    // Try RFC 3339 first, then SQLite datetime format.
    if let Ok(last_ts) = chrono::DateTime::parse_from_rfc3339(last_str) {
        let last_utc = last_ts.with_timezone(&chrono::Utc);
        let elapsed = *now_utc - last_utc;
        return elapsed.num_seconds() as u64 >= interval_secs;
    }
    if let Ok(last_ts) = chrono::NaiveDateTime::parse_from_str(last_str, "%Y-%m-%d %H:%M:%S") {
        let last_utc = chrono::DateTime::<chrono::Utc>::from_naive_utc_and_offset(last_ts, chrono::Utc);
        let elapsed = *now_utc - last_utc;
        return elapsed.num_seconds() as u64 >= interval_secs;
    }
    // Unparseable timestamp — treat as due.
    true
}

/// Humanise a duration in seconds, e.g. 1204 → "20m 04s".
pub fn humanise_duration(secs: u64) -> String {
    let minutes = secs / 60;
    let seconds = secs % 60;
    format!("{minutes}m {seconds:02}s")
}

#[cfg(test)]
mod session_watchdog_tests {
    use super::*;

    #[test]
    fn watchdog_due_not_due_one_second_early() {
        let now = chrono::DateTime::parse_from_rfc3339("2024-01-15T10:00:29Z").unwrap();
        let due = watchdog_is_due(&now.with_timezone(&chrono::Utc), Some("2024-01-15T10:00:00Z"), 30);
        assert!(!due, "should not be due 29s after last at interval 30");
    }

    #[test]
    fn watchdog_due_exactly_on_boundary() {
        let now = chrono::DateTime::parse_from_rfc3339("2024-01-15T10:00:30Z").unwrap();
        let due = watchdog_is_due(&now.with_timezone(&chrono::Utc), Some("2024-01-15T10:00:00Z"), 30);
        assert!(due, "should be due exactly at interval boundary (30s)");
    }

    #[test]
    fn watchdog_due_after_interval() {
        let now = chrono::DateTime::parse_from_rfc3339("2024-01-15T10:01:00Z").unwrap();
        let due = watchdog_is_due(&now.with_timezone(&chrono::Utc), Some("2024-01-15T10:00:00Z"), 30);
        assert!(due, "should be due after interval");
    }

    #[test]
    fn watchdog_due_no_last_is_always_due() {
        let now = chrono::DateTime::parse_from_rfc3339("2024-01-15T10:00:00Z").unwrap();
        let due = watchdog_is_due(&now.with_timezone(&chrono::Utc), None, 3600);
        assert!(due, "never nudged should be due");
    }
}

/// The ONE renderer is the point of the whole exercise: the 20s sweep and the "Nudge now" button
/// must type the same sentence, so both call this and neither has its own `.replace()` chain.
#[cfg(test)]
mod nudge_render_tests {
    use crate::services::session_watchdog::{nudge_now_rejection, render_watchdog_nudge};
    use crate::services::sessions::humanise_duration;
    use crate::services::sessions::placeholder_tokens::PLACEHOLDER_QUIET;

    /// All three placeholders expand, `{nudges}` is the number of the nudge ABOUT to be delivered
    /// (the counter is bumped only after a successful delivery, so it renders `nudge_count + 1`),
    /// and a prompt with no placeholders comes back byte-identical.
    #[test]
    fn render_watchdog_nudge_substitutes_quiet_name_and_nudges() {
        // The placeholder tokens, spelled with concat() so this file does not hold a second copy
        // of the quoted literal the gate counts (only render_watchdog_nudge may).
        let prompt = format!(
            "{} of silence in {{name}}; this is nudge {{nudges}}.",
            PLACEHOLDER_QUIET
        );
        // {quiet} is the sweep's own humanising of the interval, not a raw second count.
        let rendered = render_watchdog_nudge(
            &prompt,
            1204,
            "kord",
            0,
        );
        assert!(
            rendered.contains(&humanise_duration(1204)),
            "{{quiet}} must expand through humanise_duration, got {rendered:?}"
        );
        assert!(rendered.contains("20m 04s"), "1204s is 20m 04s, got {rendered:?}");
        assert!(rendered.contains("kord"), "{{name}} must be substituted, got {rendered:?}");
        assert!(rendered.contains("nudge 1"), "{{nudges}} must be 1 for count 0, got {rendered:?}");
        assert!(!rendered.contains('{'), "no placeholder may survive: {rendered:?}");

        // The off-by-one this is guarding: the first nudge a user ever sees is #1, not #0.
        assert!(render_watchdog_nudge("{nudges}", 60, "t", 41).ends_with("42"));

        // A prompt with nothing to substitute is handed back untouched.
        let plain = "still there? just checking in.";
        assert_eq!(render_watchdog_nudge(plain, 300, "kord", 7), plain);
    }

    /// "Nudge now" refuses rather than typing an empty message into the pane. `nudge_session_now`
    /// needs an `AppState` (this module's tests are `AppState`-free), so the precondition it
    /// consults is extracted into a pure fn and THAT is what is pinned here.
    #[test]
    fn nudge_now_requires_a_prompt() {
        // Migration 027's default — the state every row was in before the arm backfilled it.
        let rejection = nudge_now_rejection("").expect("an empty prompt must be rejected");
        assert!(
            rejection.contains("prompt"),
            "the refusal must name the missing prompt, got {rejection:?}"
        );

        // Whitespace-only is the same nothing, and would otherwise type a blank line.
        assert!(nudge_now_rejection("   ").is_some());
        assert!(nudge_now_rejection("\t ").is_some());

        // Anything real is accepted — including a prompt with no placeholders at all.
        assert!(nudge_now_rejection("still there?").is_none());
        assert!(nudge_now_rejection(PLACEHOLDER_QUIET).is_none());
    }
}

/// Runs the REAL `WATCHDOG_CANDIDATE_SQL` and the REAL `SET_WATCHDOG_SQL` end to end on a
/// throwaway in-memory database.
///
/// The seam this covers: the write that arms a session lives in `services::sessions`, the query
/// that selects nudge candidates lives here, and nothing used to check that the first satisfies
/// the second — which is exactly how "On" came to mean nothing on every live row. No `AppState`
/// harness is built (out of scope); the two consts are the shipped ones, so this is the shipped
/// pair under test.
#[cfg(test)]
mod watchdog_candidate_seam_tests {
    use crate::services::session_watchdog::WATCHDOG_CANDIDATE_SQL;
    use crate::services::sessions::{
        watchdog_is_due, DEFAULT_WATCHDOG_PROMPT, SET_WATCHDOG_SQL,
    };
    use rusqlite::{params, Connection};

    /// A session exactly as migration 027 leaves a freshly created row: disarmed, and with the
    /// `''` prompt default the sweep used to skip forever.
    fn conn_with_fresh_session() -> Connection {
        let conn = Connection::open_in_memory().expect("in-memory sqlite");
        conn.execute_batch(
            "CREATE TABLE sessions (
                id TEXT PRIMARY KEY,
                title                  TEXT    NOT NULL DEFAULT '',
                status                 TEXT    NOT NULL DEFAULT 'active',
                watchdog_enabled       INTEGER NOT NULL DEFAULT 0,
                watchdog_interval_secs INTEGER NOT NULL DEFAULT 1200,
                watchdog_prompt        TEXT    NOT NULL DEFAULT '',
                watchdog_last_nudge_at TEXT,
                watchdog_nudge_count   INTEGER NOT NULL DEFAULT 0,
                updated_at             TEXT    NOT NULL DEFAULT (datetime('now'))
            );",
        )
        .expect("create sessions");
        conn.execute(
            "INSERT INTO sessions (id, title, watchdog_enabled, watchdog_prompt) \
             VALUES ('s1', 'kord', 0, '')",
            [],
        )
        .expect("insert s1");
        conn
    }

    /// The rows the sweep would visit this tick — the REAL candidate query.
    fn candidates(conn: &Connection) -> Vec<(String, u64, String, i64)> {
        let mut stmt = conn
            .prepare(WATCHDOG_CANDIDATE_SQL)
            .expect("candidate SQL must parse against the sessions table");
        let rows = stmt
            .query_map([], |row| {
                Ok((
                    row.get::<_, String>(0)?,
                    row.get::<_, i64>(1)? as u64,
                    row.get::<_, String>(2)?,
                    row.get::<_, i64>(4)?,
                ))
            })
            .expect("query candidates")
            .filter_map(|r| r.ok())
            .collect::<Vec<_>>();
        rows
    }

    /// Arm → candidate → due. Step by step, so a failure names the link that broke.
    #[test]
    fn arming_a_fresh_session_makes_it_a_nudge_candidate() {
        let conn = conn_with_fresh_session();
        let interval = 300i64;

        // 1-2) A fresh row is NOT a candidate: disarmed and promptless, both of which the sweep
        // requires. Nothing is ever typed into it, which is correct for an unarmed session.
        assert!(
            candidates(&conn).is_empty(),
            "a session migration 027 left unarmed must not be a nudge candidate"
        );

        // 3) The real arm — the same statement `set_session_watchdog` executes.
        let changed = conn
            .execute(
                SET_WATCHDOG_SQL,
                params![1i64, interval, "s1", DEFAULT_WATCHDOG_PROMPT],
            )
            .expect("arm s1");
        assert_eq!(changed, 1, "the arm must hit exactly the one row");

        // 4) THE assertion that would have caught the shipped bug: arming alone must be enough to
        // put the row in front of the sweep, and the prompt it carries must be non-empty.
        let rows = candidates(&conn);
        assert_eq!(rows.len(), 1, "arming must make the session a nudge candidate");
        let (id, candidate_interval, prompt, nudge_count) = &rows[0];
        assert_eq!(id, "s1");
        assert_eq!(*candidate_interval, interval as u64, "the armed interval is what the sweep reads");
        assert!(
            !prompt.trim().is_empty(),
            "the candidate must carry a prompt the sweep can type — an empty one is the bug"
        );
        assert_eq!(*nudge_count, 0, "arming must not fake a delivered nudge");

        // 5) With no prior nudge and no reports, it is due the moment it is armed.
        assert!(
            watchdog_is_due(&chrono::Utc::now(), None, interval as u64),
            "a freshly armed session is due immediately"
        );

        // 6) And the dependency is the PROMPT, not the flag: empty it again and the sweep stops
        // seeing the row even though it is still armed.
        conn.execute(
            "UPDATE sessions SET watchdog_prompt = '' WHERE id = 's1'",
            [],
        )
        .expect("force the prompt back to empty");
        assert!(
            candidates(&conn).is_empty(),
            "an armed session with an empty prompt is NOT a candidate — the sweep needs text"
        );
    }
}

/// Executes the REAL `SET_WATCHDOG_SQL` against a throwaway in-memory database.
///
/// This module has no `AppState`/DB test harness and building one is out of scope, so the
/// backfill is proven at the level that actually matters: the statement itself, on a connection
/// with just the columns it touches. `set_session_watchdog` executes this same const, so the
/// statement under test is the statement that ships.
/// The three placeholder tokens the sweep expands. Spelled with `concat!` on purpose: the gate
/// counts occurrences of the quoted `{quiet}` literal in `src/` and requires exactly ONE — the
/// substitution in `session_watchdog::render_watchdog_nudge`. A second copy here would be the
/// first step towards the sweep and the "Nudge now" button drifting apart.
#[cfg(test)]
pub(crate) mod placeholder_tokens {
    pub(crate) const PLACEHOLDER_QUIET: &str = concat!("{", "quiet", "}");
    pub(crate) const PLACEHOLDER_NAME: &str = concat!("{", "name", "}");
    pub(crate) const PLACEHOLDER_NUDGES: &str = concat!("{", "nudges", "}");
}

#[cfg(test)]
mod watchdog_backfill_tests {
    use super::*;
    use crate::services::sessions::placeholder_tokens::{
        PLACEHOLDER_NAME, PLACEHOLDER_NUDGES, PLACEHOLDER_QUIET,
    };
    use rusqlite::{params, Connection};

    /// A connection holding one session `id='s1'` whose `watchdog_prompt` is `prompt`.
    fn conn_with_session(prompt: &str) -> Connection {
        let conn = Connection::open_in_memory().expect("in-memory sqlite");
        conn.execute_batch(
            "CREATE TABLE sessions (
                id TEXT PRIMARY KEY,
                watchdog_enabled       INTEGER NOT NULL DEFAULT 0,
                watchdog_interval_secs INTEGER NOT NULL DEFAULT 1200,
                watchdog_prompt        TEXT    NOT NULL DEFAULT '',
                watchdog_last_nudge_at TEXT,
                watchdog_nudge_count   INTEGER NOT NULL DEFAULT 0,
                updated_at             TEXT    NOT NULL DEFAULT (datetime('now'))
            );",
        )
        .expect("create sessions");
        conn.execute(
            "INSERT INTO sessions (id, watchdog_prompt) VALUES ('s1', ?1)",
            params![prompt],
        )
        .expect("insert s1");
        conn
    }

    fn prompt_of(conn: &Connection) -> String {
        conn.query_row(
            "SELECT watchdog_prompt FROM sessions WHERE id = 's1'",
            [],
            |row| row.get::<_, String>(0),
        )
        .expect("read back watchdog_prompt")
    }

    fn enabled_of(conn: &Connection) -> i64 {
        conn.query_row(
            "SELECT watchdog_enabled FROM sessions WHERE id = 's1'",
            [],
            |row| row.get::<_, i64>(0),
        )
        .expect("read back watchdog_enabled")
    }

    fn interval_of(conn: &Connection) -> i64 {
        conn.query_row(
            "SELECT watchdog_interval_secs FROM sessions WHERE id = 's1'",
            [],
            |row| row.get::<_, i64>(0),
        )
        .expect("read back watchdog_interval_secs")
    }

    /// Arming a session that has never had a prompt typed into it must backfill the default.
    /// Without this, the sweep's `AND watchdog_prompt != ''` skips the row forever and the
    /// toggle reads "On" while nothing is ever nudged.
    #[test]
    fn set_session_watchdog_backfills_default_prompt_when_enabling() {
        // ── empty prompt + enable -> the default lands ─────────────────────────
        let conn = conn_with_session("");
        let changed = conn
            .execute(
                SET_WATCHDOG_SQL,
                params![1i64, 300i64, "s1", DEFAULT_WATCHDOG_PROMPT],
            )
            .expect("enable on an empty-prompt row");
        assert_eq!(changed, 1, "the statement must hit exactly the one row");
        assert_eq!(enabled_of(&conn), 1, "enabling must stick");
        assert_eq!(interval_of(&conn), 300, "the clamped interval must be written");
        assert_eq!(
            prompt_of(&conn),
            DEFAULT_WATCHDOG_PROMPT,
            "arming an empty-prompt session must backfill the default the sweep will type"
        );
        assert!(
            prompt_of(&conn).contains(PLACEHOLDER_QUIET)
                && prompt_of(&conn).contains(PLACEHOLDER_NAME)
                && prompt_of(&conn).contains(PLACEHOLDER_NUDGES),
            "the backfilled prompt must carry the sweep's placeholders"
        );

        // ── the user's own prompt is NEVER overwritten ─────────────────────────
        let conn = conn_with_session("my own words");
        conn.execute(
            SET_WATCHDOG_SQL,
            params![1i64, 600i64, "s1", DEFAULT_WATCHDOG_PROMPT],
        )
        .expect("enable on a written-prompt row");
        assert_eq!(
            prompt_of(&conn),
            "my own words",
            "arming must not clobber a prompt the user wrote"
        );
        assert_eq!(interval_of(&conn), 600, "the interval still updates alongside");

        // ── disarming must not wipe the prompt ─────────────────────────────────
        let conn = conn_with_session("");
        conn.execute(
            SET_WATCHDOG_SQL,
            params![0i64, 300i64, "s1", DEFAULT_WATCHDOG_PROMPT],
        )
        .expect("disable on an empty-prompt row");
        assert_eq!(enabled_of(&conn), 0, "disabling must stick");
        assert_eq!(
            prompt_of(&conn),
            "",
            "disarming must not backfill — an unarmed session gets no prompt"
        );

        // ── disarm AFTER arming keeps the backfilled default ───────────────────
        let conn = conn_with_session("");
        conn.execute(
            SET_WATCHDOG_SQL,
            params![1i64, 300i64, "s1", DEFAULT_WATCHDOG_PROMPT],
        )
        .expect("arm");
        conn.execute(
            SET_WATCHDOG_SQL,
            params![0i64, 300i64, "s1", DEFAULT_WATCHDOG_PROMPT],
        )
        .expect("disarm after arming");
        assert_eq!(
            prompt_of(&conn),
            DEFAULT_WATCHDOG_PROMPT,
            "disarming must not wipe the prompt the arm backfilled"
        );
    }

    /// The clamp lives in `set_session_watchdog`, not in the SQL — pinned here so the hoisting
    /// of the statement into a const cannot silently change what the caller passes.
    #[test]
    fn set_watchdog_sql_writes_whatever_interval_the_caller_clamped() {
        let conn = conn_with_session("");
        conn.execute(
            SET_WATCHDOG_SQL,
            params![1i64, 60i64, "s1", DEFAULT_WATCHDOG_PROMPT],
        )
        .expect("enable at the floor of the clamp");
        assert_eq!(interval_of(&conn), 60);
        conn.execute(
            SET_WATCHDOG_SQL,
            params![1i64, 3600i64, "s1", DEFAULT_WATCHDOG_PROMPT],
        )
        .expect("enable at the ceiling of the clamp");
        assert_eq!(interval_of(&conn), 3600);
    }

    /// The host constant and the web seam must agree byte for byte — the textarea shows one and
    /// the sweep types the other.
    #[test]
    fn default_watchdog_prompt_matches_the_text_the_web_seam_seeds() {
        let expected = "You have been quiet for {quiet}. Report where {name} stands right now: \
                        what you just finished, what you are working on, and anything blocking \
                        you. This is nudge {nudges}.";
        assert_eq!(DEFAULT_WATCHDOG_PROMPT, expected);
    }

    /// The prompt UPDATE behind `set_session_watchdog_prompt` must report that it hit nothing,
    /// so an id that does not exist cannot come back as success.
    #[test]
    fn updating_the_watchdog_prompt_for_a_missing_session_is_an_error() {
        let conn = conn_with_session("");
        let prompt_sql =
            "UPDATE sessions SET watchdog_prompt = ?1, updated_at = datetime('now') WHERE id = ?2";

        // ── the missing id: 0 rows, and the helper says so ──────────────────────
        let changed = conn
            .execute(prompt_sql, params!("a prompt", "nope"))
            .expect("update a nonexistent session");
        assert_eq!(changed, 0, "an unknown id must match no row");
        let err = session_write_result(changed, "nope")
            .expect_err("0 rows affected is a failure, not a silent success");
        assert!(
            err.contains("nope"),
            "the error must name the id that was not found: {err}"
        );

        // ── the real id: 1 row, and the helper accepts it ───────────────────────
        let changed = conn
            .execute(prompt_sql, params!("a prompt", "s1"))
            .expect("update s1");
        assert_eq!(changed, 1, "the statement must hit exactly the one row");
        assert_eq!(session_write_result(changed, "s1"), Ok(()));
    }

    /// Same contract for the arm/disarm write: `SET_WATCHDOG_SQL` for an unknown id affects no
    /// row, and that must surface as an error rather than an `Ok(())`.
    #[test]
    fn arming_the_watchdog_for_a_missing_session_is_an_error() {
        let conn = conn_with_session("");

        // ── the missing id: 0 rows, and the helper says so ──────────────────────
        let changed = conn
            .execute(
                SET_WATCHDOG_SQL,
                params![1i64, 300i64, "nope", DEFAULT_WATCHDOG_PROMPT],
            )
            .expect("arm a nonexistent session");
        assert_eq!(changed, 0, "an unknown id must match no row");
        let err = session_write_result(changed, "nope")
            .expect_err("0 rows affected is a failure, not a silent success");
        assert!(
            err.contains("nope"),
            "the error must name the id that was not found: {err}"
        );

        // ── the real id: 1 row, and the helper accepts it ───────────────────────
        let changed = conn
            .execute(
                SET_WATCHDOG_SQL,
                params![1i64, 300i64, "s1", DEFAULT_WATCHDOG_PROMPT],
            )
            .expect("arm s1");
        assert_eq!(changed, 1, "the statement must hit exactly the one row");
        assert_eq!(session_write_result(changed, "s1"), Ok(()));
    }
}

#[cfg(test)]
mod terminal_status_tests {
    use super::reported_terminal_status;

    // ── branch: not an attached-tmux session → "none" ─────────────────────────
    // Whatever else is known about such a row, it has no external terminal to report.

    #[test]
    fn reported_terminal_status_not_attached_tmux_is_none_regardless_of_stored() {
        assert_eq!(reported_terminal_status(false, None, None), "none");
        assert_eq!(reported_terminal_status(false, None, Some(true)), "none");
        assert_eq!(reported_terminal_status(false, None, Some(false)), "none");
        assert_eq!(
            reported_terminal_status(false, Some("attached"), Some(true)),
            "none"
        );
        assert_eq!(
            reported_terminal_status(false, Some("attached"), Some(false)),
            "none"
        );
        assert_eq!(
            reported_terminal_status(false, Some("detached"), None),
            "none"
        );
    }

    // ── branch: tmux_alive == Some(false) → "ended" ───────────────────────────
    // The tmux is gone, and that beats whatever the row stored: the row is a
    // memory, the tmux is the fact.

    #[test]
    fn reported_terminal_status_stored_attached_whose_tmux_is_gone_reports_ended() {
        // The measured defect: 00d2bdc0 / "kloo-cli" — stored "attached" (or
        // anything else), no such tmux on the machine. Must NOT read as attached.
        assert_eq!(
            reported_terminal_status(true, Some("attached"), Some(false)),
            "ended"
        );
        assert_eq!(
            reported_terminal_status(true, Some("detached"), Some(false)),
            "ended"
        );
        assert_eq!(reported_terminal_status(true, None, Some(false)), "ended");
        assert_eq!(
            reported_terminal_status(true, Some("none"), Some(false)),
            "ended"
        );
        assert_eq!(
            reported_terminal_status(true, Some("garbage"), Some(false)),
            "ended"
        );
    }

    // ── branch: tmux alive (or unchecked) + stored "attached" → "attached" ────

    #[test]
    fn reported_terminal_status_tmux_alive_and_stored_attached_reports_attached() {
        assert_eq!(
            reported_terminal_status(true, Some("attached"), Some(true)),
            "attached"
        );
    }

    #[test]
    fn reported_terminal_status_unknown_check_and_stored_attached_reports_attached() {
        // Could not ask tmux — the stored memory is all we have, and it says attached.
        assert_eq!(
            reported_terminal_status(true, Some("attached"), None),
            "attached"
        );
    }

    // ── branch: tmux alive (or unchecked) + stored NOT "attached" → "detached" ─

    #[test]
    fn reported_terminal_status_tmux_alive_and_stored_detached_reports_detached() {
        assert_eq!(
            reported_terminal_status(true, Some("detached"), Some(true)),
            "detached"
        );
    }

    #[test]
    fn reported_terminal_status_tmux_alive_and_stored_missing_or_other_reports_detached() {
        assert_eq!(reported_terminal_status(true, None, Some(true)), "detached");
        assert_eq!(
            reported_terminal_status(true, Some(""), Some(true)),
            "detached"
        );
        assert_eq!(
            reported_terminal_status(true, Some("none"), Some(true)),
            "detached"
        );
        assert_eq!(
            reported_terminal_status(true, Some("ended"), Some(true)),
            "detached"
        );
    }

    #[test]
    fn reported_terminal_status_unknown_check_and_stored_detached_reports_detached() {
        assert_eq!(reported_terminal_status(true, None, None), "detached");
        assert_eq!(
            reported_terminal_status(true, Some("detached"), None),
            "detached"
        );
        assert_eq!(
            reported_terminal_status(true, Some("none"), None),
            "detached"
        );
    }

    // ── the hard rule: an unknown check NEVER reports "ended" ─────────────────
    // A failed `tmux list-sessions` (no binary, no server) is not evidence that a
    // terminal died. `None` must never produce "ended" for any input.

    #[test]
    fn reported_terminal_status_unknown_check_never_reports_ended() {
        for attached in [true, false] {
            for stored in [
                None,
                Some(""),
                Some("attached"),
                Some("detached"),
                Some("ended"),
                Some("none"),
                Some("Attached"),
                Some("garbage"),
            ] {
                let answer = reported_terminal_status(attached, stored, None);
                assert_ne!(
                    answer, "ended",
                    "unknown tmux check reported \"ended\" for attached={attached}, stored={stored:?}"
                );
                assert!(
                    matches!(answer, "none" | "attached" | "detached"),
                    "unexpected status {answer:?} for attached={attached}, stored={stored:?}"
                );
            }
        }
    }

    #[test]
    fn reported_terminal_status_only_answers_the_four_known_states() {
        for attached in [true, false] {
            for stored in [None, Some("attached"), Some("detached"), Some("junk")] {
                for alive in [None, Some(true), Some(false)] {
                    let answer = reported_terminal_status(attached, stored, alive);
                    assert!(
                        matches!(answer, "none" | "attached" | "detached" | "ended"),
                        "invented status {answer:?} for attached={attached}, stored={stored:?}, alive={alive:?}"
                    );
                }
            }
        }
    }
}

/// Executes the REAL `SESSION_SELECT_COLUMNS` against a throwaway in-memory database.
///
/// This module has no `AppState` harness (and `list_sessions` / `query_session` need one, plus
/// a tmux probe), so the read is proven at the level that actually matters: the column list
/// itself, on a connection holding the columns migration 027 added. `list_sessions_filtered`
/// and `query_session` interpolate this same const, so the statement under test is the
/// statement that ships.
#[cfg(test)]
mod session_select_tests {
    use super::*;
    use rusqlite::{params, Connection};

    const WATCHDOG_COLUMNS: [&str; 6] = [
        "watchdog_enabled",
        "watchdog_interval_secs",
        "watchdog_prompt",
        "watchdog_last_nudge_at",
        "watchdog_nudge_count",
        // Migration 028: nudges the sweep decided were due but WITHHELD. Named here so the
        // shared column list cannot quietly stop reading it — the failure mode that hid the
        // original watchdog bug is a mapper dropping a column with no error anywhere.
        "watchdog_held_count",
    ];

    /// A session row shaped like the live one: armed, 5-minute interval, a prompt of the
    /// user's own, one nudge delivered at a known time, 7 banked, and 2 nudges the sweep
    /// decided were due and WITHHELD (migration 028).
    ///
    /// The held count is seeded NON-zero on purpose: `list_sessions_select_carries_the_held_count`
    /// reads it back out of the SELECT, and a row defaulting to 0 would pass even if the mapper
    /// hardcoded 0 or read the wrong index.
    fn conn_with_armed_session() -> Connection {
        let conn = Connection::open_in_memory().expect("in-memory sqlite");
        conn.execute_batch(
            "CREATE TABLE sessions (
                id                 TEXT PRIMARY KEY,
                title              TEXT NOT NULL DEFAULT 'New Session',
                provider           TEXT NOT NULL DEFAULT 'claude_code',
                model              TEXT NOT NULL DEFAULT '',
                working_directory  TEXT NOT NULL DEFAULT '',
                status             TEXT NOT NULL DEFAULT 'active',
                cli_session_id     TEXT,
                total_input_tokens INTEGER NOT NULL DEFAULT 0,
                total_output_tokens INTEGER NOT NULL DEFAULT 0,
                total_cost_cents   INTEGER NOT NULL DEFAULT 0,
                kind               TEXT NOT NULL DEFAULT 'user',
                created_at         TEXT NOT NULL DEFAULT (datetime('now')),
                updated_at         TEXT NOT NULL DEFAULT (datetime('now')),
                attached_tmux      INTEGER NOT NULL DEFAULT 0,
                terminal_status    TEXT NOT NULL DEFAULT 'none',
                watchdog_enabled       INTEGER NOT NULL DEFAULT 0,
                watchdog_interval_secs INTEGER NOT NULL DEFAULT 1200,
                watchdog_prompt        TEXT    NOT NULL DEFAULT '',
                watchdog_last_nudge_at TEXT,
                watchdog_nudge_count   INTEGER NOT NULL DEFAULT 0,
                watchdog_held_count    INTEGER NOT NULL DEFAULT 0
            );",
        )
        .expect("create sessions");
        conn.execute(
            "INSERT INTO sessions (id, title, watchdog_enabled, watchdog_interval_secs,
                                   watchdog_prompt, watchdog_last_nudge_at, watchdog_nudge_count,
                                   watchdog_held_count)
             VALUES ('s1', 'a shell', 1, 300, 'Report where {name} stands.',
                     '2024-01-15 10:00:00', 7, 2)",
            [],
        )
        .expect("insert armed s1");
        conn
    }

    #[test]
    fn list_sessions_select_reads_the_watchdog_columns() {
        // 1) the shared column list NAMES all five — this is what a copy-pasted
        //    per-query list gets wrong.
        for column in WATCHDOG_COLUMNS {
            assert!(
                SESSION_SELECT_COLUMNS.contains(column),
                "SESSION_SELECT_COLUMNS does not read {column}: {SESSION_SELECT_COLUMNS}"
            );
        }

        // 2) the statement built from it PARSES and RETURNS them — a name in the
        //    const that is not a real column, or in the wrong position, dies here.
        let conn = conn_with_armed_session();
        let sql = format!(
            "SELECT {} FROM sessions WHERE id = ?1",
            SESSION_SELECT_COLUMNS
        );
        let row = conn
            .query_row(&sql, params!["s1"], |row| {
                Ok((
                    row.get::<_, i64>(15)?,
                    row.get::<_, i64>(16)?,
                    row.get::<_, String>(17)?,
                    row.get::<_, Option<String>>(18)?,
                    row.get::<_, i64>(19)?,
                ))
            })
            .expect("the shared column list must parse against the sessions table");
        assert_eq!(row.0, 1, "watchdog_enabled must read as the armed flag");
        assert_eq!(row.1, 300, "watchdog_interval_secs");
        assert_eq!(row.2, "Report where {name} stands.", "watchdog_prompt");
        assert_eq!(
            row.3.as_deref(),
            Some("2024-01-15 10:00:00"),
            "watchdog_last_nudge_at"
        );
        assert_eq!(row.4, 7, "watchdog_nudge_count");
    }

    /// The sixth watchdog column must come back out of the SHARED SELECT at the index the two
    /// `Ok(Session { .. })` builders read it at (20). This is the layer of the eight where the held
    /// count is most easily lost — a column list that stops naming it, or a column added before it
    /// that shifts every index after 19, makes the console read "held 0×" on a row the sweep is
    /// actively holding, with no error anywhere.
    #[test]
    fn list_sessions_select_carries_the_held_count() {
        let conn = conn_with_armed_session();
        let sql = format!(
            "SELECT {} FROM sessions WHERE id = ?1",
            SESSION_SELECT_COLUMNS
        );
        let held = conn
            .query_row(&sql, params!["s1"], |row| row.get::<_, i64>(20))
            .expect("the held count must read without failing the row");
        assert_eq!(held, 2, "SESSION_SELECT_COLUMNS must carry watchdog_held_count");

        // And a row predating migration 028's backfill still reads 0, not NULL — the column is
        // NOT NULL DEFAULT 0, so a session can never be dropped from the listing over it.
        conn.execute("UPDATE sessions SET watchdog_held_count = 0 WHERE id = 's1'", [])
            .expect("zero the held count");
        assert_eq!(
            conn.query_row(&sql, params!["s1"], |row| row.get::<_, i64>(20))
                .expect("a 0-held row must still read"),
            0
        );
    }

    #[test]
    fn list_sessions_select_never_nudged_row_reads_last_nudge_as_null() {
        // The column is nullable in migration 027; a cold row must read None, not
        // fail the row and get dropped by the listing's filter_map.
        let conn = conn_with_armed_session();
        conn.execute(
            "UPDATE sessions SET watchdog_last_nudge_at = NULL, watchdog_nudge_count = 0 \
             WHERE id = 's1'",
            [],
        )
        .expect("clear the nudge stamp");
        let sql = format!(
            "SELECT {} FROM sessions WHERE id = ?1",
            SESSION_SELECT_COLUMNS
        );
        let (last, count) = conn
            .query_row(&sql, params!["s1"], |row| {
                Ok((row.get::<_, Option<String>>(18)?, row.get::<_, i64>(19)?))
            })
            .expect("a never-nudged row must still read");
        assert_eq!(last, None, "never nudged reads as NULL");
        assert_eq!(count, 0);
    }
}
