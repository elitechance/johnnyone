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
            "SELECT id, title, provider, model, working_directory, status, cli_session_id, total_input_tokens, total_output_tokens, total_cost_cents, kind, created_at, updated_at, attached_tmux, terminal_status FROM sessions{} ORDER BY updated_at DESC",
            where_sql
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
        "SELECT id, title, provider, model, working_directory, status, cli_session_id, total_input_tokens, total_output_tokens, total_cost_cents, kind, created_at, updated_at, attached_tmux, terminal_status FROM sessions WHERE id = ?1",
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
