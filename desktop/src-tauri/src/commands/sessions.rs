use crate::db::models::{CreateSessionInput, Session};
use crate::state::app_state::AppState;
use rusqlite::params;
use tauri::State;
use uuid::Uuid;

/// Create a new chat session.
#[tauri::command]
pub async fn create_session(
    input: CreateSessionInput,
    state: State<'_, AppState>,
) -> Result<Session, String> {
    let id = Uuid::new_v4().to_string();
    let provider = input.provider.unwrap_or_else(|| "claude_code".to_string());
    let model = input.model.unwrap_or_default();
    let title = input.title.unwrap_or_else(|| "New Session".to_string());

    // Use last working directory from settings if not specified
    let working_directory = match input.working_directory {
        Some(wd) if !wd.is_empty() => wd,
        _ => state
            .db
            .with_conn(|conn| {
                let mut stmt = conn
                    .prepare("SELECT value FROM settings WHERE key = 'last_working_directory'")
                    .map_err(|e| e.to_string())?;
                stmt.query_row([], |row| row.get::<_, String>(0))
                    .unwrap_or_default()
                    .pipe_ref(|v| Ok(v.clone()))
            })
            .unwrap_or_default(),
    };

    state.db.with_conn(|conn| {
        conn.execute(
            "INSERT INTO sessions (id, title, provider, model, working_directory) VALUES (?1, ?2, ?3, ?4, ?5)",
            params![id, title, provider, model, working_directory],
        )
        .map_err(|e| format!("Failed to create session: {}", e))?;

        query_session(conn, &id)
    })
}

/// List all sessions, optionally filtered by status.
#[tauri::command]
pub async fn list_sessions(
    status: Option<String>,
    state: State<'_, AppState>,
) -> Result<Vec<Session>, String> {
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

/// Get a single session by ID.
#[tauri::command]
pub async fn get_session(
    id: String,
    state: State<'_, AppState>,
) -> Result<Session, String> {
    state.db.with_conn(|conn| query_session(conn, &id))
}

/// Update a session's title.
#[tauri::command]
pub async fn update_session_title(
    id: String,
    title: String,
    state: State<'_, AppState>,
) -> Result<Session, String> {
    state.db.with_conn(|conn| {
        conn.execute(
            "UPDATE sessions SET title = ?1, updated_at = datetime('now') WHERE id = ?2",
            params![title, id],
        )
        .map_err(|e| format!("Failed to update title: {}", e))?;

        query_session(conn, &id)
    })
}

/// Update a session's working directory.
/// Also clears cli_session_id since Claude CLI sessions are tied to the project directory.
#[tauri::command]
pub async fn update_session_working_directory(
    id: String,
    working_directory: String,
    state: State<'_, AppState>,
) -> Result<Session, String> {
    state.db.with_conn(|conn| {
        conn.execute(
            "UPDATE sessions SET working_directory = ?1, cli_session_id = NULL, updated_at = datetime('now') WHERE id = ?2",
            params![working_directory, id],
        )
        .map_err(|e| format!("Failed to update working directory: {}", e))?;

        query_session(conn, &id)
    })
}

/// Archive a session.
#[tauri::command]
pub async fn archive_session(
    id: String,
    state: State<'_, AppState>,
) -> Result<Session, String> {
    // Kill any running process for this session
    {
        let mut processes = state.active_processes.lock().await;
        if let Some(mut proc) = processes.remove(&id) {
            let _ = proc.kill().await;
        }
    }

    state.db.with_conn(|conn| {
        conn.execute(
            "UPDATE sessions SET status = 'archived', updated_at = datetime('now') WHERE id = ?1",
            params![id],
        )
        .map_err(|e| format!("Failed to archive session: {}", e))?;

        query_session(conn, &id)
    })
}

/// Helper to query a single session by ID.
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

/// Helper trait to pipe references.
trait PipeRef {
    fn pipe_ref<T, F: FnOnce(&Self) -> T>(&self, f: F) -> T;
}

impl<S> PipeRef for S {
    fn pipe_ref<T, F: FnOnce(&Self) -> T>(&self, f: F) -> T {
        f(self)
    }
}
