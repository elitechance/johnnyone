use crate::db::models::Message;
use crate::events::{ChatCompleteEvent, ChatDeltaEvent};
use crate::providers::{
    claude_code, cline, codex, grok, lokalai, ollama_cli, ChunkType, CliProvider,
    StreamChunk,
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
    send_chat_message_blocking_with(
        state,
        std::sync::Arc::new(crate::services::plan_agent_tmux::LiveTmux),
        session_id,
        content,
        TURN_IDLE_TIMEOUT,
    )
    .await
}

/// [`send_chat_message_blocking`] with an explicit tmux and idle timeout (tests pass a fake tmux
/// and a short timeout; nothing else differs).
pub(crate) async fn send_chat_message_blocking_with(
    state: &AppState,
    tmux: std::sync::Arc<dyn crate::services::plan_agent_tmux::TmuxRunner>,
    session_id: String,
    content: String,
    stream_idle_timeout: Duration,
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

    // Plans tab 02/07: a plan's agent session runs claude_code with the j1 MCP config and
    // least-privilege flags; any other provider, a plan that left briefing, or a turn already in
    // flight (the slot is CLAIMED here, F-2-19) refuses the turn before anything is stored.
    let plan_turn = claim_plan_agent_turn(state, &session_id, &provider_str)?;

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
    if matches!(provider, CliProvider::Kloo) {
        return Err("kloo is a oneshot executor, not a chat provider".to_string());
    }
    let cli_path_ref = cli_path.as_deref();
    let cli_sid_ref = cli_session_id.as_deref();
    let config = match provider {
        CliProvider::ClaudeCode => claude_code::build_config_with(
            &content,
            &working_dir,
            &model,
            cli_path_ref,
            cli_sid_ref,
            plan_turn.as_ref().map(|t| &t.extras),
        ),
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
        CliProvider::Kloo => unreachable!("kloo provider already rejected"),
        CliProvider::Lokalai => {
            let (endpoint, api_key) = lokalai::resolve_endpoint_and_key(state).unwrap_or_default();
            lokalai::build_config(&content, &working_dir, &model, &endpoint, &api_key)
        }
    };

    let parse_fn: fn(&str) -> Option<StreamChunk> = match provider {
        CliProvider::ClaudeCode => claude_code::parse_line,
        CliProvider::Codex => codex::parse_line,
        CliProvider::Grok => grok::parse_line,
        CliProvider::Cline => cline::parse_line,
        CliProvider::Ollama => ollama_cli::parse_line,
        CliProvider::Shell => unreachable!("shell provider already rejected"),
        CliProvider::Kloo => unreachable!("kloo provider already rejected"),
        CliProvider::Lokalai => lokalai::parse_line,
    };

    // A plan-agent turn runs visibly in its `johnnyone_<sid>` pane; its data still comes only from
    // the turn's stream-json (`plan_agent_tmux`). Every other turn is the headless `spawn_cli`.
    let (process, mut rx) = crate::services::plan_agent_tmux::spawn_chat_turn_with(
        state,
        tmux,
        config,
        session_id.clone(),
        parse_fn,
        plan_turn.map(|t| t.slot),
    )
    .await?;

    let turn_id = process.turn_id();
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
    let mut tool_chunks: Vec<StreamChunk> = Vec::new();

    loop {
        let chunk = match recv_turn_chunk(&mut rx, stream_idle_timeout).await {
            TurnRecv::Chunk(chunk) => chunk,
            TurnRecv::Closed => {
                if !saw_final_chunk {
                    finish_reason = "stream_closed".to_string();
                }
                break;
            }
            TurnRecv::TimedOut => {
                finish_reason = "timeout".to_string();
                should_kill_process = true;
                fallback_error = Some(TURN_TIMEOUT_MESSAGE.to_string());

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

        if matches!(chunk.chunk_type, ChunkType::ToolUse | ChunkType::ToolResult) {
            tool_chunks.push(chunk.clone());
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

    // Only THIS turn's handle: a later turn may already be registered under the same session.
    end_own_turn(&state.active_processes, &session_id, turn_id, should_kill_process).await;

    let (content, reason) =
        final_assistant_content(&full_content, fallback_error.as_deref(), &stderr_lines, &finish_reason);
    full_content = content;
    finish_reason = reason;

    if !full_content.trim().is_empty() && !saw_final_chunk {
        let _ = state.chat_delta_tx.send(ChatDeltaEvent {
            session_id: session_id.clone(),
            message_id: assistant_msg_id.clone(),
            delta: full_content.clone(),
            chunk_type: "text".to_string(),
            is_final: false,
        });
    }

    let tool_calls = fold_tool_calls(&tool_chunks);
    let assistant_message = state.db.with_conn(|conn| {
        conn.execute(
            "UPDATE messages SET content = ?1, input_tokens = ?2, output_tokens = ?3, cost_cents = ?4, finish_reason = ?5, tool_calls = ?7 WHERE id = ?6",
            params![full_content, total_input_tokens, total_output_tokens, total_cost_cents, finish_reason, assistant_msg_id, tool_calls],
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

/// Plan-agent extras for one chat turn. `Ok(None)` for every non-plan session; `Err` refuses the
/// turn: a plan session on another provider, or a plan that left briefing. PRIVATE on purpose
/// (G-1-13): it does not claim the turn slot, so every chat path goes through
/// [`claim_plan_agent_turn`], the only entry point.
fn plan_agent_turn_extras(
    state: &AppState,
    session_id: &str,
    provider: &str,
) -> Result<Option<claude_code::LaunchExtras>, String> {
    match CliProvider::from_str(provider) {
        Some(CliProvider::ClaudeCode) => {
            // One turn at a time is enforced by the CLAIM in [`claim_plan_agent_turn`], which every
            // chat path makes before it stores anything.
            crate::services::plan_agent::plan_agent_extras(state, session_id)
        }
        _ if crate::services::plan_agent::is_plan_agent_session(state, session_id) => {
            Err(crate::services::plan_agent::PLAN_AGENT_PROVIDER_ONLY.to_string())
        }
        _ => Ok(None),
    }
}

/// Remove `session_id`'s entry from `active_processes` only when it is the handle with `turn_id`
/// (the caller's own turn). A stale turn's cleanup can never remove or kill a later turn's handle.
pub async fn take_own_process(
    processes: &tokio::sync::Mutex<std::collections::HashMap<String, crate::providers::cli_runner::CliProcess>>,
    session_id: &str,
    turn_id: u64,
) -> Option<crate::providers::cli_runner::CliProcess> {
    let mut map = processes.lock().await;
    if map.get(session_id).map(|p| p.turn_id()) == Some(turn_id) {
        map.remove(session_id)
    } else {
        None
    }
}

/// How long a chat turn may go without a chunk before it is killed (all 3 chat paths).
pub const TURN_IDLE_TIMEOUT: Duration = Duration::from_secs(120);

/// The stored/shown text of an idle-timed-out turn.
pub const TURN_TIMEOUT_MESSAGE: &str = "Response timed out waiting for model output. Please retry.";

/// One wait on a turn's chunk stream.
#[derive(Debug)]
pub enum TurnRecv {
    Chunk(StreamChunk),
    /// The stream ended (turn over, or stopped).
    Closed,
    /// No chunk within the idle timeout.
    TimedOut,
}

/// Wait up to `idle` for the turn's next chunk.
pub async fn recv_turn_chunk(rx: &mut tokio::sync::mpsc::Receiver<StreamChunk>, idle: Duration) -> TurnRecv {
    match timeout(idle, rx.recv()).await {
        Ok(Some(chunk)) => TurnRecv::Chunk(chunk),
        Ok(None) => TurnRecv::Closed,
        Err(_) => TurnRecv::TimedOut,
    }
}

/// The turn is over: drop THIS turn's registration and, when `kill` (an idle timeout), kill its
/// process — for a plan-agent pane turn that is `PaneTurn::stop` (`respawn-pane -k` on the agent's
/// own pane), which also ends its tail and so releases the turn slot. `true` when it killed.
pub async fn end_own_turn(
    processes: &tokio::sync::Mutex<std::collections::HashMap<String, crate::providers::cli_runner::CliProcess>>,
    session_id: &str,
    turn_id: u64,
    kill: bool,
) -> bool {
    let Some(mut proc) = take_own_process(processes, session_id, turn_id).await else {
        return false;
    };
    if kill && proc.is_running() {
        if let Err(error) = proc.kill().await {
            tracing::warn!(%session_id, %error, "chat turn: kill after idle timeout failed");
        }
        return true;
    }
    false
}

/// The stored assistant text + finish reason of a finished turn (pure; all 3 chat paths):
/// - streamed text AND an error (a crash/timeout after partial output, F-1-10) → the text, a blank
///   line, then the error line — the crash is never dropped;
/// - streamed text only → the text;
/// - no text → the error, else the recent stderr, else a generic "ended without text" line.
///
/// A `"stop"` reason becomes `error` / `stderr` / `empty` when the fallback explains the end.
pub fn final_assistant_content(
    full_content: &str,
    error: Option<&str>,
    stderr_lines: &[String],
    finish_reason: &str,
) -> (String, String) {
    let upgrade = |to: &str| if finish_reason == "stop" { to.to_string() } else { finish_reason.to_string() };
    if !full_content.trim().is_empty() {
        return match error {
            Some(e) if !e.trim().is_empty() && !full_content.contains(e.trim()) => {
                (format!("{}\n\n{}", full_content.trim_end(), e.trim()), upgrade("error"))
            }
            _ => (full_content.to_string(), finish_reason.to_string()),
        };
    }
    if let Some(e) = error {
        (e.to_string(), upgrade("error"))
    } else if !stderr_lines.is_empty() {
        (
            format!("Generation ended without assistant text. Recent stderr:\n{}", stderr_lines.join("\n")),
            upgrade("stderr"),
        )
    } else {
        ("Generation ended without assistant text. Please retry.".to_string(), upgrade("empty"))
    }
}

/// A claimed plan-agent turn: its launch extras and the held one-turn-at-a-time slot.
pub struct PlanAgentTurn {
    pub extras: claude_code::LaunchExtras,
    pub slot: crate::services::plan_agent_tmux::TurnSlot,
}

/// The refusal for a chat turn on a session that has ended (archived, or a worker/reviewer whose
/// plan job has ended): the chat paths never start a CLI for it (G-1-12, the same resurrection
/// class as `terminal::AGENT_JOB_ENDED`).
pub const CHAT_SESSION_ENDED: &str =
    "This agent's job has ended; it does not take new messages. Transcript only.";

/// The refusal for a plain archived session that is not a finished agent (G-2-21).
pub const CHAT_SESSION_ARCHIVED: &str = "This session is archived; it does not take new messages.";

/// The refusal for a closed plan's old plan-agent session reached through the non-plan branch
/// (its retire failed or never ran, G-2-20).
pub const CHAT_PLAN_AGENT_RETIRED: &str = "This plan's agent is retired; it does not take new messages.";

/// The ONLY entry point of the 3 chat paths (host, relay, Tauri command), and their FIRST step,
/// before any message is stored:
/// - a plan's agent → its extras plus the CLAIMED turn slot, so a second send is refused with
///   `PLAN_AGENT_BUSY` before it stores anything (F-2-19). The slot travels into the spawn and is
///   released when the turn's tail ends (or on drop, if the turn never starts);
/// - any other session → `Ok(None)`, unless it has ENDED: an archived row, or a worker/reviewer
///   whose plan job has ended (`agent_reaper::session_job_ended`) → `Err(CHAT_SESSION_ENDED)`, so
///   no headless CLI is spawned for it. A missing row (a relay "Mobile Session" seen for the first
///   time) is not ended.
pub fn claim_plan_agent_turn(
    state: &AppState,
    session_id: &str,
    provider: &str,
) -> Result<Option<PlanAgentTurn>, String> {
    let Some(extras) = plan_agent_turn_extras(state, session_id, provider)? else {
        refuse_ended_session(state, session_id)?;
        return Ok(None);
    };
    let slot = crate::services::plan_agent_tmux::claim_turn(state, session_id)?;
    Ok(Some(PlanAgentTurn { extras, slot }))
}

fn refuse_ended_session(state: &AppState, session_id: &str) -> Result<(), String> {
    let status: Option<String> = state.db.with_conn(|conn| {
        use rusqlite::OptionalExtension;
        conn.query_row("SELECT status FROM sessions WHERE id = ?1", params![session_id], |r| r.get(0))
            .optional()
            .map_err(|e| e.to_string())
    })?;
    let Some(status) = status else { return Ok(()) };
    if crate::services::agent_reaper::session_job_ended(state, session_id)? {
        return Err(CHAT_SESSION_ENDED.to_string());
    }
    // Reached only when no LIVE plan owns the session as its agent: a session that is still some
    // (closed) plan's briefing session is a retired plan agent (fails closed on a lookup error).
    if crate::services::plan_agent::is_any_plan_agent_session(state, session_id) {
        return Err(CHAT_PLAN_AGENT_RETIRED.to_string());
    }
    if status == "archived" {
        return Err(CHAT_SESSION_ARCHIVED.to_string());
    }
    Ok(())
}

/// Max chars of a tool result kept in `messages.tool_calls`.
pub const TOOL_RESULT_MAX_CHARS: usize = 400;

/// Fold a turn's tool chunks into the `messages.tool_calls` JSON the web's `message-bubble` renders:
/// `[{id:"t<n>", name, input, result?}]`. Each `ToolUse` opens a call; each `ToolResult` answers the
/// oldest still-open call (results arrive in call order), clipped to [`TOOL_RESULT_MAX_CHARS`].
/// `None` when the turn used no tools. Pure.
pub fn fold_tool_calls(chunks: &[StreamChunk]) -> Option<String> {
    let mut calls: Vec<serde_json::Value> = Vec::new();
    let mut open: std::collections::VecDeque<usize> = std::collections::VecDeque::new();
    for c in chunks {
        match c.chunk_type {
            ChunkType::ToolUse => {
                calls.push(serde_json::json!({
                    "id": format!("t{}", calls.len() + 1),
                    "name": c.tool_name.clone().unwrap_or_else(|| "unknown".to_string()),
                    "input": c.tool_input.clone().unwrap_or_else(|| serde_json::json!({})),
                }));
                open.push_back(calls.len() - 1);
            }
            ChunkType::ToolResult => {
                if let Some(i) = open.pop_front() {
                    let text = c.tool_output.clone().unwrap_or_else(|| c.content.clone());
                    let clipped: String = text.chars().take(TOOL_RESULT_MAX_CHARS).collect();
                    calls[i]["result"] = serde_json::Value::String(clipped);
                }
            }
            _ => {}
        }
    }
    if calls.is_empty() {
        None
    } else {
        serde_json::to_string(&calls).ok()
    }
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

#[cfg(test)]
mod tool_call_tests {
    use super::*;

    fn chunk(kind: ChunkType, name: Option<&str>, input: Option<serde_json::Value>, output: Option<&str>) -> StreamChunk {
        StreamChunk {
            chunk_type: kind,
            content: String::new(),
            tool_name: name.map(str::to_string),
            tool_input: input,
            tool_output: output.map(str::to_string),
            cost_usd: None,
            input_tokens: None,
            output_tokens: None,
            is_final: false,
            session_id: None,
        }
    }

    fn text(t: &str) -> StreamChunk {
        StreamChunk { content: t.to_string(), ..chunk(ChunkType::Text, None, None, None) }
    }

    #[test]
    fn fold_tool_calls_pairs_results() {
        let chunks = vec![
            text("Let me update the brief."),
            chunk(ChunkType::ToolUse, Some("mcp__j1__brief_update"), Some(serde_json::json!({"field": "why", "add": "x"})), None),
            chunk(ChunkType::ToolResult, None, None, Some("{\"version\":3}")),
            chunk(ChunkType::ToolUse, Some("mcp__j1__brief_get"), None, None),
            chunk(ChunkType::ToolUse, Some("mcp__j1__plan_check"), Some(serde_json::json!({})), None),
            chunk(ChunkType::ToolResult, None, None, Some("brief")),
            chunk(ChunkType::ToolResult, None, None, Some("check")),
            text("Done."),
        ];
        let v: serde_json::Value = serde_json::from_str(&fold_tool_calls(&chunks).unwrap()).unwrap();
        let calls = v.as_array().unwrap();
        assert_eq!(calls.len(), 3);
        assert_eq!(calls[0]["id"], "t1");
        assert_eq!(calls[0]["name"], "mcp__j1__brief_update");
        assert_eq!(calls[0]["input"]["field"], "why");
        assert_eq!(calls[0]["result"], "{\"version\":3}");
        assert_eq!((calls[1]["id"].as_str(), calls[1]["result"].as_str()), (Some("t2"), Some("brief")));
        assert_eq!(calls[1]["input"], serde_json::json!({}), "missing input → an empty object");
        assert_eq!((calls[2]["id"].as_str(), calls[2]["result"].as_str()), (Some("t3"), Some("check")));
        // A call whose result never came keeps no `result` key.
        let v: serde_json::Value = serde_json::from_str(
            &fold_tool_calls(&[chunk(ChunkType::ToolUse, Some("x"), None, None)]).unwrap(),
        )
        .unwrap();
        assert!(v[0].get("result").is_none());
    }

    #[test]
    fn fold_tool_calls_none_without_tools() {
        assert_eq!(fold_tool_calls(&[]), None);
        assert_eq!(fold_tool_calls(&[text("hello"), text(" world")]), None);
        // A stray result with no call is ignored.
        assert_eq!(fold_tool_calls(&[chunk(ChunkType::ToolResult, None, None, Some("r"))]), None);
    }

    #[test]
    fn fold_tool_calls_truncates_result() {
        let long = "é".repeat(TOOL_RESULT_MAX_CHARS + 50);
        let chunks = vec![
            chunk(ChunkType::ToolUse, Some("mcp__j1__repo_search"), Some(serde_json::json!({"query": "q"})), None),
            chunk(ChunkType::ToolResult, None, None, Some(&long)),
        ];
        let v: serde_json::Value = serde_json::from_str(&fold_tool_calls(&chunks).unwrap()).unwrap();
        let r = v[0]["result"].as_str().unwrap();
        assert_eq!(r.chars().count(), TOOL_RESULT_MAX_CHARS);
        assert!(long.starts_with(r));
    }
}
