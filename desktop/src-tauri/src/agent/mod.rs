pub mod heartbeat;
pub mod message_types;
pub mod reconnect;
pub mod registration;
pub mod ws_client;

use crate::providers::{
    claude_code, cli_runner, cline, codex, ollama_cli, ChunkType, CliProvider, StreamChunk,
};
use crate::state::app_state::AppState;
use crate::tools::ToolDispatcher;
use futures_util::{SinkExt, StreamExt};
use message_types::{
    AgentEnvelope, AgentMessage, RelayChatComplete, RelayChatDelta, RelayChatRequest,
    RpcMessageView, RpcRequest, RpcResponse, RpcSessionView, SessionDeleted,
};
use std::sync::Arc;
use tokio::sync::Mutex;
use tracing;

/// Configuration for connecting the agent to the worker.
pub struct AgentConfig {
    pub worker_url: String,
    pub user_id: String,
    pub tenant_id: String,
}

/// AgentService manages the lifecycle of the WebSocket connection to the
/// Cloudflare Worker ChatRelayDO. It handles connecting, message routing,
/// heartbeats, and reconnection.
pub struct AgentService;

impl AgentService {
    /// Start the agent service. This will:
    /// 1. Register this desktop node with the worker via GraphQL
    /// 2. Connect to the WebSocket endpoint
    /// 3. Start the heartbeat sender
    /// 4. Listen for incoming messages and route them
    /// 5. Handle reconnection on disconnect
    pub async fn start(config: AgentConfig, state: AppState) -> Result<(), String> {
        let state = Arc::new(state);
        let reconnector = reconnect::ReconnectPolicy::new();
        let tool_dispatcher = Arc::new(Mutex::new(ToolDispatcher::new()));

        // Register this desktop node with the worker
        let hostname = gethostname::gethostname().to_string_lossy().to_string();

        tracing::info!(
            worker_url = %config.worker_url,
            user_id = %config.user_id,
            hostname = %hostname,
            "Registering desktop node"
        );

        let registered = registration::register_node(
            &config.worker_url,
            &config.user_id,
            &config.tenant_id,
            &hostname,
        )
        .await?;

        let node_id = registered.id;
        tracing::info!(
            node_id = %node_id,
            status = %registered.status,
            "Desktop node registered"
        );

        // Build WebSocket URL
        let ws_scheme = if config.worker_url.starts_with("https") {
            "wss"
        } else {
            "ws"
        };
        let host = config
            .worker_url
            .trim_start_matches("https://")
            .trim_start_matches("http://");
        let ws_url = format!(
            "{}://{}/api/relay/ws?nodeId={}&clientType=desktop&userId={}&tenantId={}",
            ws_scheme, host, node_id, config.user_id, config.tenant_id
        );

        loop {
            tracing::info!(url = %ws_url, "Connecting to agent session");

            match ws_client::connect(&ws_url).await {
                Ok((ws_write, ws_read)) => {
                    // Update connection status
                    {
                        let mut status = state.connection_status.lock().await;
                        status.connected = true;
                    }

                    tracing::info!("WebSocket connected");
                    reconnector.reset();

                    let ws_write = Arc::new(Mutex::new(ws_write));

                    // Start heartbeat sender
                    let heartbeat_handle = {
                        let ws_write = Arc::clone(&ws_write);
                        let state = Arc::clone(&state);
                        let mut shutdown_rx = state.shutdown_tx.subscribe();
                        let hb_node_id = node_id.clone();
                        tokio::spawn(async move {
                            heartbeat::run_heartbeat(ws_write, &mut shutdown_rx, hb_node_id).await;
                        })
                    };

                    // Process incoming messages
                    let process_result = Self::process_messages(
                        ws_read,
                        Arc::clone(&ws_write),
                        Arc::clone(&state),
                        Arc::clone(&tool_dispatcher),
                    )
                    .await;

                    // Clean up heartbeat
                    heartbeat_handle.abort();

                    // Update connection status
                    {
                        let mut status = state.connection_status.lock().await;
                        status.connected = false;
                    }

                    match process_result {
                        Ok(()) => {
                            tracing::info!("Agent session ended normally");
                            return Ok(());
                        }
                        Err(e) => {
                            tracing::warn!(error = %e, "WebSocket disconnected");
                        }
                    }
                }
                Err(e) => {
                    tracing::error!(error = %e, "Failed to connect");
                }
            }

            // Check if we should shut down
            {
                let status = state.connection_status.lock().await;
                if status.session_id.is_none() {
                    tracing::info!("Session cleared, stopping reconnection");
                    return Ok(());
                }
            }

            // Wait before reconnecting
            let delay = reconnector.next_delay();
            tracing::info!(delay_ms = delay.as_millis(), "Reconnecting after delay");
            tokio::time::sleep(delay).await;
        }
    }

    /// Process incoming WebSocket messages, dispatching tool calls and relay requests.
    async fn process_messages(
        mut ws_read: ws_client::WsRead,
        ws_write: Arc<Mutex<ws_client::WsWrite>>,
        state: Arc<AppState>,
        tool_dispatcher: Arc<Mutex<ToolDispatcher>>,
    ) -> Result<(), String> {
        let mut shutdown_rx = state.shutdown_tx.subscribe();
        let mut session_deleted_rx = state.session_deleted_tx.subscribe();

        loop {
            tokio::select! {
                msg = ws_read.next() => {
                    match msg {
                        Some(Ok(tokio_tungstenite::tungstenite::Message::Text(text))) => {
                            Self::handle_text_message(
                                &text,
                                &ws_write,
                                &state,
                                &tool_dispatcher,
                            ).await?;
                        }
                        Some(Ok(tokio_tungstenite::tungstenite::Message::Ping(data))) => {
                            let mut writer = ws_write.lock().await;
                            let _ = writer.send(tokio_tungstenite::tungstenite::Message::Pong(data)).await;
                        }
                        Some(Ok(tokio_tungstenite::tungstenite::Message::Close(_))) => {
                            tracing::info!("Received close frame");
                            return Ok(());
                        }
                        Some(Err(e)) => {
                            return Err(format!("WebSocket error: {}", e));
                        }
                        None => {
                            return Err("WebSocket stream ended".to_string());
                        }
                        _ => {
                            // Ignore binary and other frame types
                        }
                    }
                }
                Ok(session_id) = session_deleted_rx.recv() => {
                    // Broadcast session_deleted to mobile clients via relay
                    let envelope = AgentEnvelope {
                        message: AgentMessage::SessionDeleted(SessionDeleted {
                            session_id: session_id.clone(),
                        }),
                    };
                    if let Ok(json) = serde_json::to_string(&envelope) {
                        let mut writer = ws_write.lock().await;
                        let _ = writer.send(
                            tokio_tungstenite::tungstenite::Message::Text(json.into())
                        ).await;
                        tracing::info!(session_id = %session_id, "Broadcast session_deleted to relay");
                    }
                }
                _ = shutdown_rx.recv() => {
                    tracing::info!("Shutdown signal received");
                    let mut writer = ws_write.lock().await;
                    let _ = writer.send(tokio_tungstenite::tungstenite::Message::Close(None)).await;
                    return Ok(());
                }
            }
        }
    }

    /// Handle a text message from the WebSocket.
    async fn handle_text_message(
        text: &str,
        ws_write: &Arc<Mutex<ws_client::WsWrite>>,
        state: &Arc<AppState>,
        tool_dispatcher: &Arc<Mutex<ToolDispatcher>>,
    ) -> Result<(), String> {
        let envelope: AgentEnvelope = serde_json::from_str(text).map_err(|e| {
            tracing::warn!(error = %e, raw = %text, "Failed to parse message");
            format!("Parse error: {}", e)
        })?;

        match envelope.message {
            AgentMessage::ToolCall(tool_call) => {
                tracing::info!(
                    tool = %tool_call.tool_name,
                    call_id = %tool_call.call_id,
                    "Received tool call"
                );

                // Record the execution
                let record = crate::tools::tool_schema::ToolExecutionRecord::new_pending(
                    tool_call.call_id.clone(),
                    tool_call.tool_name.clone(),
                    tool_call.input.clone(),
                );
                {
                    let mut executions = state.tool_executions.lock().await;
                    executions.push(record);
                }

                // Execute the tool
                let dispatcher = tool_dispatcher.lock().await;
                let result = dispatcher.dispatch(&tool_call).await;

                // Update the execution record
                {
                    let mut executions = state.tool_executions.lock().await;
                    if let Some(rec) = executions
                        .iter_mut()
                        .find(|r| r.call_id == tool_call.call_id)
                    {
                        rec.complete(result.clone());
                    }
                }

                // Send the result back
                let response = AgentEnvelope {
                    message: AgentMessage::ToolResult(result),
                };
                let response_text = serde_json::to_string(&response)
                    .map_err(|e| format!("Serialize error: {}", e))?;

                let mut writer = ws_write.lock().await;
                writer
                    .send(tokio_tungstenite::tungstenite::Message::Text(
                        response_text.into(),
                    ))
                    .await
                    .map_err(|e| format!("Send error: {}", e))?;
            }
            AgentMessage::RelayChatRequest(relay_req) => {
                tracing::info!(
                    relay_id = %relay_req.relay_id,
                    session_id = %relay_req.session_id,
                    "Received relay chat request from mobile"
                );

                // Handle relay in a background task
                let ws = Arc::clone(ws_write);
                let st = Arc::clone(state);
                tokio::spawn(async move {
                    if let Err(e) = Self::handle_relay_chat(relay_req, ws, st).await {
                        tracing::error!(error = %e, "Relay chat error");
                    }
                });
            }
            AgentMessage::RpcRequest(rpc_req) => {
                tracing::info!(
                    request_id = %rpc_req.request_id,
                    method = %rpc_req.method,
                    "Received RPC request"
                );

                let ws = Arc::clone(ws_write);
                let st = Arc::clone(state);
                tokio::spawn(async move {
                    let response = Self::handle_rpc(&rpc_req, &st).await;
                    let envelope = AgentEnvelope {
                        message: AgentMessage::RpcResponse(response),
                    };
                    if let Ok(msg) = serde_json::to_string(&envelope) {
                        let mut writer = ws.lock().await;
                        let _ = writer
                            .send(tokio_tungstenite::tungstenite::Message::Text(msg.into()))
                            .await;
                    }
                });
            }
            AgentMessage::Heartbeat(hb) => {
                tracing::debug!(ts = %hb.timestamp, "Received heartbeat ack");
                let mut status = state.connection_status.lock().await;
                status.last_heartbeat = Some(chrono::Utc::now());
            }
            AgentMessage::ToolResult(_) => {
                tracing::debug!("Received tool result echo");
            }
            AgentMessage::RpcResponse(_) => {
                tracing::debug!("Received RPC response echo");
            }
            AgentMessage::Error(err) => {
                tracing::error!(code = %err.code, message = %err.message, "Server error");
            }
            _ => {
                // Ignore other message types (they're outbound-only)
            }
        }

        Ok(())
    }

    /// Handle an RPC request by dispatching to the appropriate query method.
    async fn handle_rpc(req: &RpcRequest, state: &Arc<AppState>) -> RpcResponse {
        let result = match req.method.as_str() {
            "list_sessions" => Self::rpc_list_sessions(&req.params, state),
            "get_session" => Self::rpc_get_session(&req.params, state),
            "list_messages" => Self::rpc_list_messages(&req.params, state),
            "delete_session" => Self::rpc_delete_session(&req.params, state).await,
            _ => Err(format!("Unknown RPC method: {}", req.method)),
        };

        match result {
            Ok(data) => RpcResponse {
                request_id: req.request_id.clone(),
                success: true,
                data: Some(data),
                error: None,
            },
            Err(e) => RpcResponse {
                request_id: req.request_id.clone(),
                success: false,
                data: None,
                error: Some(e),
            },
        }
    }

    fn rpc_list_sessions(
        params: &serde_json::Value,
        state: &Arc<AppState>,
    ) -> Result<serde_json::Value, String> {
        let status = params.get("status").and_then(|v| v.as_str());

        state.db.with_conn(|conn| {
            let (sql, param_value);
            let query_params: Vec<&dyn rusqlite::ToSql>;

            if let Some(s) = status {
                sql = "SELECT id, title, provider, model, working_directory, status, total_input_tokens, total_output_tokens, total_cost_cents, created_at, updated_at FROM sessions WHERE status = ?1 ORDER BY updated_at DESC";
                param_value = s.to_string();
                query_params = vec![&param_value as &dyn rusqlite::ToSql];
            } else {
                sql = "SELECT id, title, provider, model, working_directory, status, total_input_tokens, total_output_tokens, total_cost_cents, created_at, updated_at FROM sessions ORDER BY updated_at DESC";
                query_params = vec![];
            }

            let mut stmt = conn.prepare(sql).map_err(|e| e.to_string())?;
            let sessions: Vec<RpcSessionView> = stmt
                .query_map(query_params.as_slice(), |row| {
                    Ok(RpcSessionView {
                        id: row.get(0)?,
                        title: row.get(1)?,
                        provider: row.get(2)?,
                        model: row.get(3)?,
                        working_directory: row.get(4)?,
                        status: row.get(5)?,
                        total_input_tokens: row.get(6)?,
                        total_output_tokens: row.get(7)?,
                        total_cost_cents: row.get(8)?,
                        created_at: row.get(9)?,
                        updated_at: row.get(10)?,
                    })
                })
                .map_err(|e| e.to_string())?
                .filter_map(|r| r.ok())
                .collect();

            serde_json::to_value(&sessions).map_err(|e| e.to_string())
        })
    }

    fn rpc_get_session(
        params: &serde_json::Value,
        state: &Arc<AppState>,
    ) -> Result<serde_json::Value, String> {
        let id = params
            .get("id")
            .and_then(|v| v.as_str())
            .ok_or_else(|| "Missing 'id' parameter".to_string())?;

        state.db.with_conn(|conn| {
            let session = conn
                .query_row(
                    "SELECT id, title, provider, model, working_directory, status, total_input_tokens, total_output_tokens, total_cost_cents, created_at, updated_at FROM sessions WHERE id = ?1",
                    rusqlite::params![id],
                    |row| {
                        Ok(RpcSessionView {
                            id: row.get(0)?,
                            title: row.get(1)?,
                            provider: row.get(2)?,
                            model: row.get(3)?,
                            working_directory: row.get(4)?,
                            status: row.get(5)?,
                            total_input_tokens: row.get(6)?,
                            total_output_tokens: row.get(7)?,
                            total_cost_cents: row.get(8)?,
                            created_at: row.get(9)?,
                            updated_at: row.get(10)?,
                        })
                    },
                )
                .map_err(|e| format!("Session not found: {}", e))?;

            serde_json::to_value(&session).map_err(|e| e.to_string())
        })
    }

    fn rpc_list_messages(
        params: &serde_json::Value,
        state: &Arc<AppState>,
    ) -> Result<serde_json::Value, String> {
        let session_id = params
            .get("sessionId")
            .and_then(|v| v.as_str())
            .ok_or_else(|| "Missing 'sessionId' parameter".to_string())?;
        let limit = params.get("limit").and_then(|v| v.as_i64()).unwrap_or(100);
        let offset = params.get("offset").and_then(|v| v.as_i64()).unwrap_or(0);

        state.db.with_conn(|conn| {
            let mut stmt = conn
                .prepare(
                    "SELECT id, session_id, role, content, tool_calls, finish_reason, input_tokens, output_tokens, cost_cents, created_at FROM messages WHERE session_id = ?1 ORDER BY created_at ASC LIMIT ?2 OFFSET ?3",
                )
                .map_err(|e| e.to_string())?;

            let messages: Vec<RpcMessageView> = stmt
                .query_map(rusqlite::params![session_id, limit, offset], |row| {
                    Ok(RpcMessageView {
                        id: row.get(0)?,
                        session_id: row.get(1)?,
                        role: row.get(2)?,
                        content: row.get(3)?,
                        tool_calls: row.get(4)?,
                        finish_reason: row.get(5)?,
                        input_tokens: row.get(6)?,
                        output_tokens: row.get(7)?,
                        cost_cents: row.get(8)?,
                        created_at: row.get(9)?,
                    })
                })
                .map_err(|e| e.to_string())?
                .filter_map(|r| r.ok())
                .collect();

            serde_json::to_value(&messages).map_err(|e| e.to_string())
        })
    }

    async fn rpc_delete_session(
        params: &serde_json::Value,
        state: &Arc<AppState>,
    ) -> Result<serde_json::Value, String> {
        let id = params
            .get("id")
            .and_then(|v| v.as_str())
            .ok_or_else(|| "Missing 'id' parameter".to_string())?;

        // Kill any active CLI process for this session
        {
            let mut processes = state.active_processes.lock().await;
            if let Some(mut proc) = processes.remove(id) {
                let _ = proc.kill().await;
                tracing::info!(session_id = %id, "Killed active process before deleting session");
            }
        }

        state.db.with_conn(|conn| {
            conn.execute("DELETE FROM sessions WHERE id = ?1", rusqlite::params![id])
                .map_err(|e| e.to_string())?;
            serde_json::to_value(serde_json::json!({ "deleted": true })).map_err(|e| e.to_string())
        })
    }

    /// Handle a relayed chat request from mobile.
    /// Spawns a CLI subprocess and streams deltas back via WebSocket.
    async fn handle_relay_chat(
        req: RelayChatRequest,
        ws_write: Arc<Mutex<ws_client::WsWrite>>,
        state: Arc<AppState>,
    ) -> Result<(), String> {
        // Prefer the existing local session config to avoid resetting provider/model/cwd
        // when relay callers omit metadata (or send generic defaults).
        let existing_session: Option<(String, String, String, Option<String>)> =
            state.db.with_conn(|conn| {
                match conn.query_row(
                    "SELECT provider, model, working_directory, cli_session_id FROM sessions WHERE id = ?1",
                    rusqlite::params![req.session_id],
                    |row| {
                        Ok((
                            row.get::<_, String>(0)?,
                            row.get::<_, String>(1)?,
                            row.get::<_, String>(2)?,
                            row.get::<_, Option<String>>(3)?,
                        ))
                    },
                ) {
                    Ok(row) => Ok(Some(row)),
                    Err(rusqlite::Error::QueryReturnedNoRows) => Ok(None),
                    Err(e) => Err(e.to_string()),
                }
            })?;

        let req_provider = req
            .provider
            .as_deref()
            .map(str::trim)
            .filter(|s| !s.is_empty());
        let req_model = req
            .model
            .as_deref()
            .map(str::trim)
            .filter(|s| !s.is_empty());
        let req_working_dir = req
            .working_directory
            .as_deref()
            .map(str::trim)
            .filter(|s| !s.is_empty());

        let provider_str = existing_session
            .as_ref()
            .map(|(provider, _, _, _)| provider.clone())
            .or_else(|| req_provider.map(str::to_string))
            .unwrap_or_else(|| "claude_code".to_string());

        let model = req_model
            .map(str::to_string)
            .or_else(|| {
                existing_session
                    .as_ref()
                    .map(|(_, model, _, _)| model.clone())
            })
            .unwrap_or_default();

        let working_dir = req_working_dir
            .map(str::to_string)
            .or_else(|| {
                existing_session
                    .as_ref()
                    .map(|(_, _, working_dir, _)| working_dir.clone())
            })
            .unwrap_or_default();

        let persisted_cli_session_id = existing_session
            .as_ref()
            .and_then(|(_, _, _, cli_session_id)| cli_session_id.clone());

        let provider = CliProvider::from_str(&provider_str)
            .ok_or_else(|| format!("Unknown provider: {}", provider_str))?;

        // Look up CLI path from config
        let provider_key = provider.as_str();
        let cli_path: Option<String> = state.db.with_conn(|conn| {
            let result = conn.query_row(
                "SELECT cli_path FROM provider_configs WHERE provider = ?1",
                rusqlite::params![provider_key],
                |row| row.get::<_, String>(0),
            );
            Ok(result.ok().filter(|p| !p.is_empty()))
        })?;

        let cli_path_ref = cli_path.as_deref();
        let cli_sid_ref = if provider == CliProvider::ClaudeCode || provider == CliProvider::Codex {
            persisted_cli_session_id.as_deref()
        } else {
            None
        };
        let config = match provider {
            CliProvider::ClaudeCode => claude_code::build_config(
                &req.content,
                &working_dir,
                &model,
                cli_path_ref,
                cli_sid_ref,
            ),
            CliProvider::Codex => {
                codex::build_config(&req.content, &working_dir, &model, cli_path_ref, cli_sid_ref)
            }
            CliProvider::Cline => {
                cline::build_config(&req.content, &working_dir, &model, cli_path_ref)
            }
            CliProvider::Ollama => {
                ollama_cli::build_config(&req.content, &working_dir, &model, cli_path_ref)
            }
        };

        let parse_fn: fn(&str) -> Option<StreamChunk> = match provider {
            CliProvider::ClaudeCode => claude_code::parse_line,
            CliProvider::Codex => codex::parse_line,
            CliProvider::Cline => cline::parse_line,
            CliProvider::Ollama => ollama_cli::parse_line,
        };

        let (_process, mut rx) =
            cli_runner::spawn_cli(config, req.session_id.clone(), parse_fn).await?;

        // Stream chunks back through WebSocket as relay deltas
        let mut full_content = String::new();
        let mut total_input_tokens: i64 = 0;
        let mut total_output_tokens: i64 = 0;
        let mut total_cost_cents: i64 = 0;
        let mut captured_cli_session_id: Option<String> = None;
        let mut clear_cli_session_id = false;

        while let Some(chunk) = rx.recv().await {
            if chunk.chunk_type == ChunkType::Text {
                full_content.push_str(&chunk.content);
            }

            if (chunk.chunk_type == ChunkType::System || chunk.chunk_type == ChunkType::Result)
                && chunk.session_id.is_some()
            {
                captured_cli_session_id = chunk.session_id.clone();
            }

            if chunk.chunk_type == ChunkType::Error {
                clear_cli_session_id = true;
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

            let delta = RelayChatDelta {
                relay_id: req.relay_id.clone(),
                session_id: req.session_id.clone(),
                delta: chunk.content.clone(),
                chunk_type: format!("{:?}", chunk.chunk_type).to_lowercase(),
                is_final: chunk.is_final,
            };

            let envelope = AgentEnvelope {
                message: AgentMessage::RelayChatDelta(delta),
            };

            let msg =
                serde_json::to_string(&envelope).map_err(|e| format!("Serialize error: {}", e))?;

            let mut writer = ws_write.lock().await;
            let _ = writer
                .send(tokio_tungstenite::tungstenite::Message::Text(msg.into()))
                .await;

            if chunk.is_final {
                break;
            }
        }

        // Send completion message
        let complete = RelayChatComplete {
            relay_id: req.relay_id.clone(),
            session_id: req.session_id.clone(),
            message_id: uuid::Uuid::new_v4().to_string(),
        };

        let envelope = AgentEnvelope {
            message: AgentMessage::RelayChatComplete(complete),
        };

        let msg =
            serde_json::to_string(&envelope).map_err(|e| format!("Serialize error: {}", e))?;

        let mut writer = ws_write.lock().await;
        let _ = writer
            .send(tokio_tungstenite::tungstenite::Message::Text(msg.into()))
            .await;

        // Also save to local SQLite
        let _ = state.db.with_conn(|conn| {
            let session_id = &req.session_id;
            let user_msg_id = uuid::Uuid::new_v4().to_string();
            let assistant_msg_id = uuid::Uuid::new_v4().to_string();

            // Ensure session exists locally
            conn.execute(
                "INSERT OR IGNORE INTO sessions (id, title, provider, model, working_directory) VALUES (?1, 'Mobile Session', ?2, ?3, ?4)",
                rusqlite::params![session_id, provider_key, model, working_dir],
            ).map_err(|e| e.to_string())?;

            // Keep local session config aligned with the resolved relay config.
            conn.execute(
                "UPDATE sessions SET provider = ?1, model = ?2, working_directory = ?3, updated_at = datetime('now') WHERE id = ?4",
                rusqlite::params![provider_key, model, working_dir, session_id],
            ).map_err(|e| e.to_string())?;

            conn.execute(
                "INSERT INTO messages (id, session_id, role, content) VALUES (?1, ?2, 'user', ?3)",
                rusqlite::params![user_msg_id, session_id, req.content],
            ).map_err(|e| e.to_string())?;

            conn.execute(
                "INSERT INTO messages (id, session_id, role, content, finish_reason, input_tokens, output_tokens, cost_cents) VALUES (?1, ?2, 'assistant', ?3, 'stop', ?4, ?5, ?6)",
                rusqlite::params![assistant_msg_id, session_id, full_content, total_input_tokens, total_output_tokens, total_cost_cents],
            ).map_err(|e| e.to_string())?;

            if provider == CliProvider::ClaudeCode || provider == CliProvider::Codex {
                if clear_cli_session_id {
                    conn.execute(
                        "UPDATE sessions SET cli_session_id = NULL, updated_at = datetime('now') WHERE id = ?1",
                        rusqlite::params![session_id],
                    )
                    .map_err(|e| e.to_string())?;
                } else if let Some(ref cli_sid) = captured_cli_session_id {
                    conn.execute(
                        "UPDATE sessions SET cli_session_id = ?1, updated_at = datetime('now') WHERE id = ?2",
                        rusqlite::params![cli_sid, session_id],
                    )
                    .map_err(|e| e.to_string())?;
                }
            }

            conn.execute(
                "UPDATE sessions SET total_input_tokens = total_input_tokens + ?1, total_output_tokens = total_output_tokens + ?2, total_cost_cents = total_cost_cents + ?3, updated_at = datetime('now') WHERE id = ?4",
                rusqlite::params![total_input_tokens, total_output_tokens, total_cost_cents, session_id],
            )
            .map_err(|e| e.to_string())?;

            conn.execute(
                "INSERT INTO usage_log (session_id, message_id, provider, model, input_tokens, output_tokens, cost_cents) VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7)",
                rusqlite::params![session_id, assistant_msg_id, provider_key, model, total_input_tokens, total_output_tokens, total_cost_cents],
            )
            .map_err(|e| e.to_string())
        });

        Ok(())
    }
}
