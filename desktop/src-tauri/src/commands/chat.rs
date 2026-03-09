use crate::db::models::Message;
use crate::providers::{
    cli_runner, claude_code, codex, cline, ollama_cli, ChunkType, CliProvider, StreamChunk,
};
use crate::state::app_state::AppState;
use rusqlite::params;
use serde::Serialize;
use tauri::{Emitter, State};
use tokio::time::{timeout, Duration};
use uuid::Uuid;

/// Delta event emitted to the frontend during streaming.
#[derive(Debug, Clone, Serialize)]
pub struct ChatDelta {
    pub session_id: String,
    pub message_id: String,
    pub chunk: StreamChunk,
}

/// Send a chat message: saves user message, spawns CLI subprocess, streams response.
#[tauri::command]
pub async fn send_chat_message(
    session_id: String,
    content: String,
    app_handle: tauri::AppHandle,
    state: State<'_, AppState>,
) -> Result<Message, String> {
    // 1. Look up session to get provider + working_directory + cli_session_id
    let (provider_str, model, working_dir, cli_session_id) = state.db.with_conn(|conn| {
        conn.query_row(
            "SELECT provider, model, working_directory, cli_session_id FROM sessions WHERE id = ?1",
            params![session_id],
            |row| Ok((
                row.get::<_, String>(0)?,
                row.get::<_, String>(1)?,
                row.get::<_, String>(2)?,
                row.get::<_, Option<String>>(3)?,
            )),
        )
        .map_err(|e| format!("Session not found: {}", e))
    })?;

    // 2. Save user message to SQLite
    let user_msg_id = Uuid::new_v4().to_string();
    let user_message = state.db.with_conn(|conn| {
        conn.execute(
            "INSERT INTO messages (id, session_id, role, content) VALUES (?1, ?2, 'user', ?3)",
            params![user_msg_id, session_id, content],
        )
        .map_err(|e| format!("Failed to save user message: {}", e))?;

        conn.execute(
            "UPDATE sessions SET updated_at = datetime('now') WHERE id = ?1",
            params![session_id],
        )
        .map_err(|e| format!("Failed to update session: {}", e))?;

        query_message(conn, &user_msg_id)
    })?;

    // 3. Look up provider config for CLI path
    let cli_path: Option<String> = state.db.with_conn(|conn| {
        let result = conn.query_row(
            "SELECT cli_path FROM provider_configs WHERE provider = ?1",
            params![provider_str],
            |row| row.get::<_, String>(0),
        );
        Ok(result.ok().filter(|p| !p.is_empty()))
    })?;

    // 4. Build CLI config based on provider
    let provider = CliProvider::from_str(&provider_str)
        .ok_or_else(|| format!("Unknown provider: {}", provider_str))?;

    let cli_path_ref = cli_path.as_deref();
    let cli_sid_ref = cli_session_id.as_deref();
    let config = match provider {
        CliProvider::ClaudeCode => claude_code::build_config(&content, &working_dir, &model, cli_path_ref, cli_sid_ref),
        CliProvider::Codex => codex::build_config(&content, &working_dir, &model, cli_path_ref, cli_sid_ref),
        CliProvider::Cline => cline::build_config(&content, &working_dir, &model, cli_path_ref),
        CliProvider::Ollama => ollama_cli::build_config(&content, &working_dir, &model, cli_path_ref),
    };

    // 5. Select the parser for this provider
    let parse_fn: fn(&str) -> Option<StreamChunk> = match provider {
        CliProvider::ClaudeCode => claude_code::parse_line,
        CliProvider::Codex => codex::parse_line,
        CliProvider::Cline => cline::parse_line,
        CliProvider::Ollama => ollama_cli::parse_line,
    };

    // 6. Spawn CLI subprocess
    let (process, mut rx) =
        cli_runner::spawn_cli(config, session_id.clone(), parse_fn).await?;

    // Store the process handle so it can be killed
    {
        let mut processes = state.active_processes.lock().await;
        processes.insert(session_id.clone(), process);
    }

    // 7. Create placeholder assistant message
    let assistant_msg_id = Uuid::new_v4().to_string();
    state.db.with_conn(|conn| {
        conn.execute(
            "INSERT INTO messages (id, session_id, role, content) VALUES (?1, ?2, 'assistant', '')",
            params![assistant_msg_id, session_id],
        )
        .map_err(|e| format!("Failed to create assistant message: {}", e))
    })?;

    // 8. Stream chunks to frontend via Tauri events
    let db = state.db.clone();
    let active_processes = state.active_processes.clone();
    let sid = session_id.clone();
    let amid = assistant_msg_id.clone();

    tokio::spawn(async move {
        let stream_idle_timeout = Duration::from_secs(120);
        let mut full_content = String::new();
        let mut total_input_tokens: i64 = 0;
        let mut total_output_tokens: i64 = 0;
        let mut total_cost_cents: i64 = 0;
        let mut captured_cli_session_id: Option<String> = None;
        let mut fallback_error: Option<String> = None;
        let mut stderr_lines: Vec<String> = Vec::new();
        let mut finish_reason = "stop".to_string();
        let mut saw_final_chunk = false;
        let mut should_kill_process = false;

        loop {
            let chunk = match timeout(stream_idle_timeout, rx.recv()).await {
                Ok(Some(chunk)) => chunk,
                Ok(None) => {
                    if !saw_final_chunk {
                        finish_reason = "stream_closed".to_string();
                    }
                    break;
                }
                Err(_) => {
                    finish_reason = "timeout".to_string();
                    should_kill_process = true;

                    let message = "Response timed out waiting for model output. Please retry.".to_string();
                    fallback_error = Some(message.clone());

                    let _ = db.with_conn(|conn| {
                        conn.execute(
                            "UPDATE sessions SET cli_session_id = NULL WHERE id = ?1",
                            params![&sid],
                        )
                        .map_err(|e| format!("Failed to clear cli_session_id: {}", e))
                    });

                    let timeout_delta = ChatDelta {
                        session_id: sid.clone(),
                        message_id: amid.clone(),
                        chunk: StreamChunk {
                            chunk_type: ChunkType::Error,
                            content: message,
                            tool_name: None,
                            tool_input: None,
                            tool_output: None,
                            cost_usd: None,
                            input_tokens: None,
                            output_tokens: None,
                            is_final: true,
                            session_id: None,
                        },
                    };
                    let _ = app_handle.emit("chat:delta", &timeout_delta);
                    break;
                }
            };

            // Accumulate text content
            if chunk.chunk_type == ChunkType::Text {
                full_content.push_str(&chunk.content);
            }

            if chunk.chunk_type == ChunkType::Stderr {
                let line = chunk.content.trim();
                if !line.is_empty() {
                    stderr_lines.push(line.to_string());
                    if stderr_lines.len() > 6 {
                        stderr_lines.remove(0);
                    }
                }
            }

            // Capture cli_session_id from System (init) or Result events
            if (chunk.chunk_type == ChunkType::System || chunk.chunk_type == ChunkType::Result)
                && chunk.session_id.is_some()
            {
                captured_cli_session_id = chunk.session_id.clone();
            }

            // On error, clear cli_session_id so next attempt starts fresh
            if chunk.chunk_type == ChunkType::Error {
                fallback_error = Some(chunk.content.clone());
                finish_reason = "error".to_string();

                let _ = db.with_conn(|conn| {
                    conn.execute(
                        "UPDATE sessions SET cli_session_id = NULL WHERE id = ?1",
                        params![&sid],
                    )
                    .map_err(|e| format!("Failed to clear cli_session_id: {}", e))
                });
            }

            // Capture usage from result chunks
            if chunk.chunk_type == ChunkType::Result {
                if let Some(tokens) = chunk.input_tokens {
                    total_input_tokens = tokens;
                }
                if let Some(tokens) = chunk.output_tokens {
                    total_output_tokens = tokens;
                }
                if let Some(cost) = chunk.cost_usd {
                    total_cost_cents = (cost * 100.0) as i64;
                }
            }

            let is_final = chunk.is_final;

            // Emit delta event to frontend
            let delta = ChatDelta {
                session_id: sid.clone(),
                message_id: amid.clone(),
                chunk,
            };
            let _ = app_handle.emit("chat:delta", &delta);

            if is_final {
                saw_final_chunk = true;
                break;
            }
        }

        {
            let mut processes = active_processes.lock().await;
            if let Some(mut proc) = processes.remove(&sid) {
                if should_kill_process && proc.is_running() {
                    let _ = proc.kill().await;
                }
            }
        }

        let mut used_fallback_content = false;
        if full_content.trim().is_empty() {
            used_fallback_content = true;

            if let Some(error) = fallback_error {
                full_content = error;
                if finish_reason == "stop" {
                    finish_reason = "error".to_string();
                }
            } else if !stderr_lines.is_empty() {
                full_content = format!(
                    "Generation ended without assistant text. Recent stderr:\n{}",
                    stderr_lines.join("\n")
                );
                if finish_reason == "stop" {
                    finish_reason = "stderr".to_string();
                }
            } else {
                full_content = "Generation ended without assistant text. Please retry.".to_string();
                if finish_reason == "stop" {
                    finish_reason = "empty".to_string();
                }
            }
        }

        if used_fallback_content {
            let fallback_delta = ChatDelta {
                session_id: sid.clone(),
                message_id: amid.clone(),
                chunk: StreamChunk {
                    chunk_type: ChunkType::Text,
                    content: full_content.clone(),
                    tool_name: None,
                    tool_input: None,
                    tool_output: None,
                    cost_usd: None,
                    input_tokens: None,
                    output_tokens: None,
                    is_final: false,
                    session_id: None,
                },
            };
            let _ = app_handle.emit("chat:delta", &fallback_delta);
        }

        // 9. Update assistant message in SQLite with final content
        let _ = db.with_conn(|conn| {
            conn.execute(
                "UPDATE messages SET content = ?1, input_tokens = ?2, output_tokens = ?3, cost_cents = ?4, finish_reason = ?5 WHERE id = ?6",
                params![full_content, total_input_tokens, total_output_tokens, total_cost_cents, finish_reason, amid],
            )
            .map_err(|e| format!("Failed to update assistant message: {}", e))?;

            // Store cli_session_id from the CLI tool (for --resume on subsequent messages)
            if saw_final_chunk {
                if let Some(ref cli_sid) = captured_cli_session_id {
                    conn.execute(
                        "UPDATE sessions SET cli_session_id = ?1, updated_at = datetime('now') WHERE id = ?2",
                        params![cli_sid, &sid],
                    )
                    .map_err(|e| format!("Failed to store cli_session_id: {}", e))?;
                }
            } else {
                conn.execute(
                    "UPDATE sessions SET cli_session_id = NULL, updated_at = datetime('now') WHERE id = ?1",
                    params![&sid],
                )
                .map_err(|e| format!("Failed to clear cli_session_id after incomplete stream: {}", e))?;
            }

            // Update session token totals
            conn.execute(
                "UPDATE sessions SET total_input_tokens = total_input_tokens + ?1, total_output_tokens = total_output_tokens + ?2, total_cost_cents = total_cost_cents + ?3, updated_at = datetime('now') WHERE id = ?4",
                params![total_input_tokens, total_output_tokens, total_cost_cents, sid],
            )
            .map_err(|e| format!("Failed to update session totals: {}", e))?;

            // Log usage
            conn.execute(
                "INSERT INTO usage_log (session_id, message_id, provider, model, input_tokens, output_tokens, cost_cents) VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7)",
                params![sid, amid, provider.as_str(), "", total_input_tokens, total_output_tokens, total_cost_cents],
            )
            .map_err(|e| format!("Failed to log usage: {}", e))
        });

        // Emit completion event
        let _ = app_handle.emit("chat:complete", &serde_json::json!({
            "session_id": sid,
            "message_id": amid,
        }));
    });

    Ok(user_message)
}

/// Stop the currently streaming response for a session.
#[tauri::command]
pub async fn stop_generation(
    session_id: String,
    state: State<'_, AppState>,
) -> Result<(), String> {
    let mut processes = state.active_processes.lock().await;
    if let Some(mut proc) = processes.remove(&session_id) {
        proc.kill().await?;
        tracing::info!(session_id = %session_id, "Stopped generation");
        Ok(())
    } else {
        Err("No active generation for this session".to_string())
    }
}

/// List messages for a session.
#[tauri::command]
pub async fn list_messages(
    session_id: String,
    limit: Option<i64>,
    offset: Option<i64>,
    state: State<'_, AppState>,
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

/// Helper to query a single message by ID.
fn query_message(conn: &rusqlite::Connection, id: &str) -> Result<Message, String> {
    conn.query_row(
        "SELECT id, session_id, role, content, tool_calls, tool_call_id, finish_reason, input_tokens, output_tokens, cost_cents, created_at FROM messages WHERE id = ?1",
        params![id],
        |row| {
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
        },
    )
    .map_err(|e| format!("Message not found: {}", e))
}
