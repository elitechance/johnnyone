use crate::db::models::{CreateSessionInput, Message, Session};
use crate::events::SessionUpdatedEvent;
use crate::providers::CliProvider;
use crate::state::app_state::AppState;
use rusqlite::params;
use uuid::Uuid;

pub fn create_session(state: &AppState, input: CreateSessionInput) -> Result<Session, String> {
    let id = Uuid::new_v4().to_string();
    let provider = input.provider.unwrap_or_else(|| "claude_code".to_string());
    let model = input.model.unwrap_or_default();
    let title = input.title.unwrap_or_else(|| "New Session".to_string());

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

    let session = state.db.with_conn(|conn| {
        conn.execute(
            "INSERT INTO sessions (id, title, provider, model, working_directory) VALUES (?1, ?2, ?3, ?4, ?5)",
            params![id, title, provider, model, working_directory],
        )
        .map_err(|e| format!("Failed to create session: {}", e))?;

        query_session(conn, &id)
    })?;
    publish_session_update(state, &session);
    Ok(session)
}

pub fn list_sessions(state: &AppState, status: Option<String>) -> Result<Vec<Session>, String> {
    state.db.with_conn(|conn| {
        let (sql, param_value);
        let params: Vec<&dyn rusqlite::ToSql>;

        if let Some(ref s) = status {
            sql = "SELECT id, title, provider, model, working_directory, status, cli_session_id, total_input_tokens, total_output_tokens, total_cost_cents, created_at, updated_at FROM sessions WHERE status = ?1 ORDER BY updated_at DESC";
            param_value = s.clone();
            params = vec![&param_value as &dyn rusqlite::ToSql];
        } else {
            sql = "SELECT id, title, provider, model, working_directory, status, cli_session_id, total_input_tokens, total_output_tokens, total_cost_cents, created_at, updated_at FROM sessions ORDER BY updated_at DESC";
            params = vec![];
        }

        let mut stmt = conn.prepare(sql).map_err(|e| e.to_string())?;
        let sessions = stmt
            .query_map(params.as_slice(), |row| {
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
                    created_at: row.get(10)?,
                    updated_at: row.get(11)?,
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
    {
        let mut processes = state.active_processes.lock().await;
        if let Some(mut proc) = processes.remove(&id) {
            let _ = proc.kill().await;
        }
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
    {
        let mut processes = state.active_processes.lock().await;
        if let Some(mut proc) = processes.remove(&id) {
            let _ = proc.kill().await;
        }
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
        "SELECT id, title, provider, model, working_directory, status, cli_session_id, total_input_tokens, total_output_tokens, total_cost_cents, created_at, updated_at FROM sessions WHERE id = ?1",
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
                created_at: row.get(10)?,
                updated_at: row.get(11)?,
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
