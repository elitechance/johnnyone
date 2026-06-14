use crate::db::models::Message;
use crate::events::{ChatCompleteEvent, ChatDeltaEvent};
use crate::providers::{
    claude_code, cli_runner, cline, codex, grok, ollama_cli, ChunkType, CliProvider, StreamChunk,
};
use crate::simulator;
use crate::state::app_state::AppState;
use rusqlite::params;
use tokio::time::{sleep, timeout, Duration};
use uuid::Uuid;

#[derive(Debug, Clone, serde::Serialize)]
pub struct ChatRunResult {
    pub user_message: Message,
    pub assistant_message: Message,
}

pub async fn send_chat_message_blocking(
    state: &AppState,
    session_id: String,
    content: String,
) -> Result<ChatRunResult, String> {
    let (provider_str, model, working_dir, cli_session_id) =
        state.db.with_conn(|conn| {
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

    if simulator::host_simulator_enabled() {
        return send_simulated_chat_message(
            state,
            session_id,
            provider_str,
            model,
            content,
            user_message,
        )
        .await;
    }

    let cli_path: Option<String> = state.db.with_conn(|conn| {
        let result = conn.query_row(
            "SELECT cli_path FROM provider_configs WHERE provider = ?1",
            params![provider_str],
            |row| row.get::<_, String>(0),
        );
        Ok(result.ok().filter(|p| !p.is_empty()))
    })?;

    let provider = CliProvider::from_str(&provider_str)
        .ok_or_else(|| format!("Unknown provider: {}", provider_str))?;

    // Shell sessions are terminal-mode only — they don't accept chat messages.
    // The user interacts with the tmux pane directly. Reject explicitly so the
    // caller (web client / API) sees a clear error rather than a silent no-op.
    if matches!(provider, CliProvider::Shell) {
        return Err(
            "Shell sessions don't support chat-mode messages — type into the terminal pane instead.".to_string(),
        );
    }
    let cli_path_ref = cli_path.as_deref();
    let cli_sid_ref = cli_session_id.as_deref();
    let config = match provider {
        CliProvider::ClaudeCode => {
            claude_code::build_config(&content, &working_dir, &model, cli_path_ref, cli_sid_ref)
        }
        CliProvider::Codex => {
            codex::build_config(&content, &working_dir, &model, cli_path_ref, cli_sid_ref)
        }
        CliProvider::Grok => {
            grok::build_config(&content, &working_dir, &model, cli_path_ref, cli_sid_ref)
        }
        CliProvider::Cline => cline::build_config(&content, &working_dir, &model, cli_path_ref),
        CliProvider::Ollama => {
            ollama_cli::build_config(&content, &working_dir, &model, cli_path_ref)
        }
        // Shell is filtered out above; unreachable here but the compiler needs
        // the arm so the match stays exhaustive.
        CliProvider::Shell => unreachable!("shell provider already rejected"),
    };

    let parse_fn: fn(&str) -> Option<StreamChunk> = match provider {
        CliProvider::ClaudeCode => claude_code::parse_line,
        CliProvider::Codex => codex::parse_line,
        CliProvider::Grok => grok::parse_line,
        CliProvider::Cline => cline::parse_line,
        CliProvider::Ollama => ollama_cli::parse_line,
        CliProvider::Shell => unreachable!("shell provider already rejected"),
    };

    let (process, mut rx) = cli_runner::spawn_cli(config, session_id.clone(), parse_fn).await?;

    {
        let mut processes = state.active_processes.lock().await;
        processes.insert(session_id.clone(), process);
    }

    let assistant_msg_id = Uuid::new_v4().to_string();
    state.db.with_conn(|conn| {
        conn.execute(
            "INSERT INTO messages (id, session_id, role, content) VALUES (?1, ?2, 'assistant', '')",
            params![assistant_msg_id, session_id],
        )
        .map_err(|e| format!("Failed to create assistant message: {}", e))
    })?;

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
                let message =
                    "Response timed out waiting for model output. Please retry.".to_string();
                fallback_error = Some(message.clone());

                let _ = state.db.with_conn(|conn| {
                    conn.execute(
                        "UPDATE sessions SET cli_session_id = NULL WHERE id = ?1",
                        params![&session_id],
                    )
                    .map_err(|e| format!("Failed to clear cli_session_id: {}", e))
                });
                break;
            }
        };

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

        if (chunk.chunk_type == ChunkType::System || chunk.chunk_type == ChunkType::Result)
            && chunk.session_id.is_some()
        {
            captured_cli_session_id = chunk.session_id.clone();
        }

        if chunk.chunk_type == ChunkType::Error {
            fallback_error = Some(chunk.content.clone());
            finish_reason = "error".to_string();

            let _ = state.db.with_conn(|conn| {
                conn.execute(
                    "UPDATE sessions SET cli_session_id = NULL WHERE id = ?1",
                    params![&session_id],
                )
                .map_err(|e| format!("Failed to clear cli_session_id: {}", e))
            });
        }

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

        let _ = state.chat_delta_tx.send(ChatDeltaEvent {
            session_id: session_id.clone(),
            message_id: assistant_msg_id.clone(),
            delta: chunk.content.clone(),
            chunk_type: format!("{:?}", chunk.chunk_type).to_lowercase(),
            is_final: chunk.is_final,
        });

        if chunk.is_final {
            saw_final_chunk = true;
            break;
        }
    }

    {
        let mut processes = state.active_processes.lock().await;
        if let Some(mut proc) = processes.remove(&session_id) {
            if should_kill_process && proc.is_running() {
                let _ = proc.kill().await;
            }
        }
    }

    if full_content.trim().is_empty() {
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

    if !full_content.trim().is_empty() && !saw_final_chunk {
        let _ = state.chat_delta_tx.send(ChatDeltaEvent {
            session_id: session_id.clone(),
            message_id: assistant_msg_id.clone(),
            delta: full_content.clone(),
            chunk_type: "text".to_string(),
            is_final: false,
        });
    }

    let assistant_message = state.db.with_conn(|conn| {
        conn.execute(
            "UPDATE messages SET content = ?1, input_tokens = ?2, output_tokens = ?3, cost_cents = ?4, finish_reason = ?5 WHERE id = ?6",
            params![full_content, total_input_tokens, total_output_tokens, total_cost_cents, finish_reason, assistant_msg_id],
        )
        .map_err(|e| format!("Failed to update assistant message: {}", e))?;

        if saw_final_chunk {
            if let Some(ref cli_sid) = captured_cli_session_id {
                conn.execute(
                    "UPDATE sessions SET cli_session_id = ?1, updated_at = datetime('now') WHERE id = ?2",
                    params![cli_sid, &session_id],
                )
                .map_err(|e| format!("Failed to store cli_session_id: {}", e))?;
            }
        } else {
            conn.execute(
                "UPDATE sessions SET cli_session_id = NULL, updated_at = datetime('now') WHERE id = ?1",
                params![&session_id],
            )
            .map_err(|e| format!("Failed to clear cli_session_id after incomplete stream: {}", e))?;
        }

        conn.execute(
            "UPDATE sessions SET total_input_tokens = total_input_tokens + ?1, total_output_tokens = total_output_tokens + ?2, total_cost_cents = total_cost_cents + ?3, updated_at = datetime('now') WHERE id = ?4",
            params![total_input_tokens, total_output_tokens, total_cost_cents, &session_id],
        )
        .map_err(|e| format!("Failed to update session totals: {}", e))?;

        conn.execute(
            "INSERT INTO usage_log (session_id, message_id, provider, model, input_tokens, output_tokens, cost_cents) VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7)",
            params![&session_id, &assistant_msg_id, provider.as_str(), "", total_input_tokens, total_output_tokens, total_cost_cents],
        )
        .map_err(|e| format!("Failed to log usage: {}", e))?;

        query_message(conn, &assistant_msg_id)
    })?;

    let _ = state.chat_complete_tx.send(ChatCompleteEvent {
        session_id,
        message_id: assistant_message.id.clone(),
    });

    Ok(ChatRunResult {
        user_message,
        assistant_message,
    })
}

async fn send_simulated_chat_message(
    state: &AppState,
    session_id: String,
    provider: String,
    model: String,
    content: String,
    user_message: Message,
) -> Result<ChatRunResult, String> {
    let assistant_msg_id = Uuid::new_v4().to_string();
    state.db.with_conn(|conn| {
        conn.execute(
            "INSERT INTO messages (id, session_id, role, content) VALUES (?1, ?2, 'assistant', '')",
            params![assistant_msg_id, session_id],
        )
        .map_err(|e| format!("Failed to create assistant message: {}", e))
    })?;

    let full_content = simulator::simulated_chat_response(&provider, &model, &content);
    let deltas = full_content
        .chars()
        .collect::<Vec<_>>()
        .chunks(12)
        .map(|chunk| chunk.iter().collect::<String>())
        .collect::<Vec<_>>();
    let total_input_tokens = content.split_whitespace().count() as i64;
    let total_output_tokens = full_content.split_whitespace().count() as i64;

    for (index, delta) in deltas.iter().enumerate() {
        sleep(Duration::from_millis(180)).await;
        let _ = state.chat_delta_tx.send(ChatDeltaEvent {
            session_id: session_id.clone(),
            message_id: assistant_msg_id.clone(),
            delta: delta.clone(),
            chunk_type: "text".to_string(),
            is_final: index + 1 == deltas.len(),
        });
    }

    let assistant_message = state.db.with_conn(|conn| {
        conn.execute(
            "UPDATE messages SET content = ?1, input_tokens = ?2, output_tokens = ?3, cost_cents = 0, finish_reason = 'stop' WHERE id = ?4",
            params![full_content, total_input_tokens, total_output_tokens, assistant_msg_id],
        )
        .map_err(|e| format!("Failed to update simulated assistant message: {}", e))?;

        conn.execute(
            "UPDATE sessions SET total_input_tokens = total_input_tokens + ?1, total_output_tokens = total_output_tokens + ?2, updated_at = datetime('now') WHERE id = ?3",
            params![total_input_tokens, total_output_tokens, &session_id],
        )
        .map_err(|e| format!("Failed to update simulated session totals: {}", e))?;

        conn.execute(
            "INSERT INTO usage_log (session_id, message_id, provider, model, input_tokens, output_tokens, cost_cents) VALUES (?1, ?2, ?3, ?4, ?5, ?6, 0)",
            params![&session_id, &assistant_msg_id, &provider, &model, total_input_tokens, total_output_tokens],
        )
        .map_err(|e| format!("Failed to log simulated usage: {}", e))?;

        query_message(conn, &assistant_msg_id)
    })?;

    let _ = state.chat_complete_tx.send(ChatCompleteEvent {
        session_id,
        message_id: assistant_message.id.clone(),
    });

    Ok(ChatRunResult {
        user_message,
        assistant_message,
    })
}

pub async fn stop_generation(state: &AppState, session_id: String) -> Result<(), String> {
    let mut processes = state.active_processes.lock().await;
    if let Some(mut proc) = processes.remove(&session_id) {
        proc.kill().await?;
        tracing::info!(session_id = %session_id, "Stopped generation");
        Ok(())
    } else {
        Err("No active generation for this session".to_string())
    }
}

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
