use crate::db::models::{CreateSessionInput, Message, Session};
use crate::events::SessionUpdatedEvent;
use crate::providers::CliProvider;
use crate::state::app_state::AppState;
use rusqlite::params;
use uuid::Uuid;

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
            "SELECT id, title, provider, model, working_directory, status, cli_session_id, total_input_tokens, total_output_tokens, total_cost_cents, kind, created_at, updated_at, attached_tmux FROM sessions{} ORDER BY updated_at DESC",
            where_sql
        );

        let params_dyn: Vec<&dyn rusqlite::ToSql> =
            bound.iter().map(|v| v as &dyn rusqlite::ToSql).collect();

        let mut stmt = conn.prepare(&sql).map_err(|e| e.to_string())?;
        let sessions = stmt
            .query_map(params_dyn.as_slice(), |row| {
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
                    attached_tmux: row.get::<_, i64>(13)? != 0,
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
    // Same two-cleanup pattern as archive — see archive_session for rationale.
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

fn query_session(conn: &rusqlite::Connection, id: &str) -> Result<Session, String> {
    conn.query_row(
        "SELECT id, title, provider, model, working_directory, status, cli_session_id, total_input_tokens, total_output_tokens, total_cost_cents, kind, created_at, updated_at, attached_tmux FROM sessions WHERE id = ?1",
        params![id],
        |row| {
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
                attached_tmux: row.get::<_, i64>(13)? != 0,
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
