pub mod heartbeat;
pub mod message_types;
pub mod reconnect;
pub mod registration;
pub mod ws_client;

use crate::providers::{
    claude_code, cli_runner, cline, codex, grok, ollama_cli, ChunkType, CliProvider, StreamChunk,
};
use crate::services::{
    chat_host, providers as provider_service, sessions as session_service,
    settings as settings_service,
};
use crate::state::app_state::AppState;
use crate::tools::ToolDispatcher;
use futures_util::{SinkExt, StreamExt};
use message_types::{
    AgentEnvelope, AgentMessage, AgentPlanRunUpdated, RelayChatComplete, RelayChatDelta,
    RelayChatRequest, RpcMessageView, RpcRequest, RpcResponse, RpcSessionView, SessionDeleted,
    SessionUpdated, TerminalCommandAck, TerminalScreen,
};
use std::sync::Arc;
use tokio::sync::Mutex;
use tracing;

/// Configuration for connecting the agent to the worker.
pub struct AgentConfig {
    pub worker_url: String,
    pub user_id: String,
    pub tenant_id: String,
    pub access_token: String,
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

        // Base URL parts (stable across reconnects)
        let ws_scheme = if config.worker_url.starts_with("https") {
            "wss"
        } else {
            "ws"
        };
        let host = config
            .worker_url
            .trim_start_matches("https://")
            .trim_start_matches("http://");
        let base_ws_url = format!(
            "{}://{}/api/relay/ws?nodeId={}&clientType=desktop&userId={}&tenantId={}",
            ws_scheme, host, node_id, config.user_id, config.tenant_id
        );

        loop {
            // Re-resolve credential inside reconnect loop (Task 03) so rotated jk_ key is picked
            // up without process restart. Use public RelayConfig (which calls resolve_access_token).
            let fresh_token = settings_service::RelayConfig::resolve(&*state)
                .map(|c| c.access_token)
                .unwrap_or_else(|| config.access_token.clone());
            let mut ws_url = base_ws_url.clone();
            if !fresh_token.trim().is_empty() {
                ws_url = format!("{}&token={}", ws_url, fresh_token);
            }

            // Use shared redaction helper (Task 02 fix) so test covers the emitted log path.
            let log_url = ws_client::redact_token_in_url(&ws_url);

            tracing::info!(url = %log_url, "Connecting to agent session");

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
                    // On an auth rejection, try to refresh a short-lived JWT
                    // credential so the next reconnect uses a fresh token. No-op
                    // for durable jk_ keys (which never expire).
                    if e.contains("401") || e.contains("Unauthorized") {
                        match crate::services::relay::refresh_access_token(&*state).await {
                            Ok(true) => tracing::info!("Refreshed relay credential after 401; retrying"),
                            Ok(false) => {}
                            Err(err) => tracing::warn!(error = %err, "Relay token refresh failed"),
                        }
                    }
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
        let mut session_updated_rx = state.session_updated_tx.subscribe();
        let mut terminal_screen_rx = state.terminal_screen_tx.subscribe();
        let mut stream_event_rx = state.stream_event_tx.subscribe();
        let mut agent_plan_run_rx = state.agent_plan_run_tx.subscribe();

        loop {
            tokio::select! {
                msg = ws_read.next() => {
                    match msg {
                        Some(Ok(tokio_tungstenite::tungstenite::Message::Text(text))) => {
                            // A single malformed/unknown message must NOT tear down the relay
                            // connection. Previously `?` propagated a parse error, which the caller
                            // logged as "WebSocket disconnected" — so one unknown frame (e.g. a
                            // `stream_subscribe` before it was a known variant) dropped the whole
                            // socket ("Desktop not connected" flakiness). Log and keep going. #9 fix.
                            if let Err(e) = Self::handle_text_message(
                                &text,
                                &ws_write,
                                &state,
                                &tool_dispatcher,
                            )
                            .await
                            {
                                tracing::warn!(error = %e, "Skipping unhandled relay message; keeping connection alive");
                            }
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
                Ok(event) = session_updated_rx.recv() => {
                    let envelope = AgentEnvelope {
                        message: AgentMessage::SessionUpdated(SessionUpdated {
                            session_id: event.session_id,
                            session: event.session,
                        }),
                    };
                    if let Ok(json) = serde_json::to_string(&envelope) {
                        let mut writer = ws_write.lock().await;
                        let _ = writer.send(
                            tokio_tungstenite::tungstenite::Message::Text(json.into())
                        ).await;
                    }
                }
                Ok(screen) = terminal_screen_rx.recv() => {
                    let envelope = AgentEnvelope {
                        message: AgentMessage::TerminalScreen(TerminalScreen {
                            session_id: screen.session_id,
                            tmux_session_name: screen.tmux_session_name,
                            pane_id: screen.pane_id,
                            cursor: screen.cursor,
                            content: screen.content,
                            cursor_x: screen.cursor_x,
                            cursor_y: screen.cursor_y,
                            history_lines: screen.history_lines,
                            rows: screen.rows,
                            cols: screen.cols,
                            status: screen.status,
                        }),
                    };
                    if let Ok(json) = serde_json::to_string(&envelope) {
                        let mut writer = ws_write.lock().await;
                        let _ = writer.send(
                            tokio_tungstenite::tungstenite::Message::Text(json.into())
                        ).await;
                    }
                }
                Ok(event) = stream_event_rx.recv() => {
                    // Structured stream events ride the same WSS lane as terminal screens (D6).
                    // Mirror the terminal_screen arm exactly; do not log the event payload.
                    let envelope = AgentEnvelope {
                        message: AgentMessage::StreamEvent(event),
                    };
                    if let Ok(json) = serde_json::to_string(&envelope) {
                        let mut writer = ws_write.lock().await;
                        let _ = writer.send(
                            tokio_tungstenite::tungstenite::Message::Text(json.into())
                        ).await;
                    }
                }
                Ok(update) = agent_plan_run_rx.recv() => {
                    tracing::debug!(plan_id = %update.plan_id, deleted = update.deleted, "forwarding agent plan run update");
                    let envelope = AgentEnvelope {
                        message: AgentMessage::AgentPlanRunUpdated(AgentPlanRunUpdated {
                            plan_id: update.plan_id,
                            deleted: update.deleted,
                            run: update.run,
                        }),
                    };
                    if let Ok(json) = serde_json::to_string(&envelope) {
                        let mut writer = ws_write.lock().await;
                        let _ = writer.send(
                            tokio_tungstenite::tungstenite::Message::Text(json.into())
                        ).await;
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
            AgentMessage::TerminalCommand(command) => {
                let ws = Arc::clone(ws_write);
                let st = Arc::clone(state);
                tokio::spawn(async move {
                    tracing::info!(
                        session_id = %command.session_id,
                        control = ?command.control,
                        "terminal_command received from relay"
                    );
                    let result = if command.control.as_deref() == Some("visual_subscribe") {
                        crate::terminal::subscribe_terminal_visual(
                            &st,
                            command.session_id.clone(),
                            120,
                            36,
                        )
                        .await
                        .map(|_| ())
                    } else if command.control.as_deref() == Some("visual_refresh") {
                        crate::terminal::refresh_terminal_visual(
                            &st,
                            command.session_id.clone(),
                            120,
                            36,
                        )
                        .await
                        .map(|_| ())
                    } else if command.control.as_deref() == Some("visual_history") {
                        crate::terminal::refresh_terminal_visual_with_history(
                            &st,
                            command.session_id.clone(),
                            120,
                            36,
                            command.history_rows.unwrap_or(200),
                        )
                        .await
                        .map(|_| ())
                    } else if command.control.as_deref() == Some("visual_unsubscribe") {
                        crate::terminal::unsubscribe_terminal_visual(&st, &command.session_id).await
                    } else if command.data.is_empty() && command.attachments.is_empty() {
                        Ok(())
                    } else {
                        match Self::prepare_terminal_command_data(&st, &command).await {
                            Ok(data) => {
                                crate::terminal::send_terminal_input(
                                    &st,
                                    command.session_id.clone(),
                                    data,
                                )
                                .await
                            }
                            Err(error) => {
                                tracing::error!(%error, session_id = %command.session_id, "Failed to prepare terminal attachments");
                                Err(error)
                            }
                        }
                    };

                    if let Err(error) = &result {
                        tracing::warn!(session_id = %command.session_id, control = ?command.control, %error, "terminal_command failed");
                    }
                    let ack = TerminalCommandAck {
                        request_id: command.request_id,
                        session_id: command.session_id,
                        accepted: result.is_ok(),
                        error: result.err(),
                    };
                    let envelope = AgentEnvelope {
                        message: AgentMessage::TerminalCommandAck(ack),
                    };
                    if let Ok(msg) = serde_json::to_string(&envelope) {
                        let mut writer = ws.lock().await;
                        let _ = writer
                            .send(tokio_tungstenite::tungstenite::Message::Text(msg.into()))
                            .await;
                    }
                });
            }
            AgentMessage::TerminalVisualSubscribe(subscription) => {
                let ws = Arc::clone(ws_write);
                let st = Arc::clone(state);
                tokio::spawn(async move {
                    let result = crate::terminal::subscribe_terminal_visual(
                        &st,
                        subscription.session_id.clone(),
                        120,
                        36,
                    )
                    .await
                    .map(|_| ());

                    let ack = TerminalCommandAck {
                        request_id: subscription.request_id,
                        session_id: subscription.session_id,
                        accepted: result.is_ok(),
                        error: result.err(),
                    };
                    let envelope = AgentEnvelope {
                        message: AgentMessage::TerminalCommandAck(ack),
                    };
                    if let Ok(msg) = serde_json::to_string(&envelope) {
                        let mut writer = ws.lock().await;
                        let _ = writer
                            .send(tokio_tungstenite::tungstenite::Message::Text(msg.into()))
                            .await;
                    }
                });
            }
            AgentMessage::TerminalVisualUnsubscribe(subscription) => {
                let ws = Arc::clone(ws_write);
                let st = Arc::clone(state);
                tokio::spawn(async move {
                    let result =
                        crate::terminal::unsubscribe_terminal_visual(&st, &subscription.session_id)
                            .await;

                    let ack = TerminalCommandAck {
                        request_id: subscription.request_id,
                        session_id: subscription.session_id,
                        accepted: result.is_ok(),
                        error: result.err(),
                    };
                    let envelope = AgentEnvelope {
                        message: AgentMessage::TerminalCommandAck(ack),
                    };
                    if let Ok(msg) = serde_json::to_string(&envelope) {
                        let mut writer = ws.lock().await;
                        let _ = writer
                            .send(tokio_tungstenite::tungstenite::Message::Text(msg.into()))
                            .await;
                    }
                });
            }
            AgentMessage::TerminalResize(resize) => {
                let ws = Arc::clone(ws_write);
                let st = Arc::clone(state);
                tokio::spawn(async move {
                    let result = crate::terminal::resize_terminal(
                        &st,
                        resize.session_id.clone(),
                        resize.cols,
                        resize.rows,
                    )
                    .await;

                    let ack = TerminalCommandAck {
                        request_id: resize.request_id,
                        session_id: resize.session_id,
                        accepted: result.is_ok(),
                        error: result.err(),
                    };
                    let envelope = AgentEnvelope {
                        message: AgentMessage::TerminalCommandAck(ack),
                    };
                    if let Ok(msg) = serde_json::to_string(&envelope) {
                        let mut writer = ws.lock().await;
                        let _ = writer
                            .send(tokio_tungstenite::tungstenite::Message::Text(msg.into()))
                            .await;
                    }
                });
            }
            AgentMessage::TerminalKill(kill) => {
                let ws = Arc::clone(ws_write);
                let st = Arc::clone(state);
                tokio::spawn(async move {
                    let result =
                        crate::terminal::kill_terminal_session(&st, &kill.session_id).await;

                    let ack = TerminalCommandAck {
                        request_id: kill.request_id,
                        session_id: kill.session_id,
                        accepted: result.is_ok(),
                        error: result.err(),
                    };
                    let envelope = AgentEnvelope {
                        message: AgentMessage::TerminalCommandAck(ack),
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

    async fn prepare_terminal_command_data(
        state: &Arc<AppState>,
        command: &message_types::TerminalCommand,
    ) -> Result<String, String> {
        if command.attachments.is_empty() {
            return Ok(command.data.clone());
        }

        let saved = Self::save_command_attachments(state, command).await?;
        let mut data = command.data.trim_end_matches(['\r', '\n']).to_string();
        if !data.is_empty() {
            data.push_str("\n\n");
        }
        data.push_str("Attached image files saved in this workspace:\n");
        for path in saved {
            data.push_str("- ");
            data.push_str(&path);
            data.push('\n');
        }
        data.push('\r');
        Ok(data)
    }

    async fn save_command_attachments(
        state: &Arc<AppState>,
        command: &message_types::TerminalCommand,
    ) -> Result<Vec<String>, String> {
        use base64::{engine::general_purpose, Engine as _};
        use serde::Deserialize;
        use std::path::PathBuf;

        #[derive(Debug, Deserialize)]
        #[serde(rename_all = "camelCase")]
        struct AttachmentContent {
            id: String,
            original_name: String,
            content_type: String,
            size: i64,
            data_base64: String,
        }

        #[derive(Debug, Deserialize)]
        #[serde(rename_all = "camelCase")]
        struct AttachmentData {
            get_chat_attachment: AttachmentContent,
        }

        #[derive(Debug, Deserialize)]
        struct GraphqlResponse<T> {
            data: Option<T>,
            errors: Option<Vec<GraphqlError>>,
        }

        #[derive(Debug, Deserialize)]
        struct GraphqlError {
            message: String,
        }

        let worker_config = state
            .worker_relay_config
            .lock()
            .await
            .clone()
            .ok_or_else(|| "Worker relay config is not available".to_string())?;

        let working_dir = state.db.with_conn(|conn| {
            conn.query_row(
                "SELECT working_directory FROM sessions WHERE id = ?1",
                rusqlite::params![command.session_id],
                |row| row.get::<_, String>(0),
            )
            .map_err(|e| e.to_string())
        })?;
        let workspace_path = if working_dir.trim().is_empty() {
            std::env::current_dir().map_err(|e| e.to_string())?
        } else {
            PathBuf::from(working_dir)
        };
        let attachment_dir = workspace_path.join(".johnnyone").join("attachments");
        tokio::fs::create_dir_all(&attachment_dir)
            .await
            .map_err(|e| format!("Failed to create attachment directory: {}", e))?;

        let client = reqwest::Client::new();
        let graphql_url = format!("{}/graphql", worker_config.worker_url.trim_end_matches('/'));
        let mut saved_paths = Vec::new();

        for attachment in &command.attachments {
            let response = client
                .post(&graphql_url)
                .header("Content-Type", "application/json")
                .header("x-tenant-id", &worker_config.tenant_id)
                .header("x-user-id", &worker_config.user_id)
                .json(&serde_json::json!({
                    "query": "query GetChatAttachment($id: ID!) { getChatAttachment(id: $id) { id originalName contentType size dataBase64 } }",
                    "variables": { "id": attachment.id }
                }))
                .send()
                .await
                .map_err(|e| format!("Attachment download request failed: {}", e))?;

            if !response.status().is_success() {
                let status = response.status();
                let body = response.text().await.unwrap_or_default();
                return Err(format!(
                    "Attachment download failed with status {}: {}",
                    status, body
                ));
            }

            let gql: GraphqlResponse<AttachmentData> = response
                .json()
                .await
                .map_err(|e| format!("Failed to parse attachment response: {}", e))?;
            if let Some(errors) = gql.errors {
                let messages = errors.into_iter().map(|e| e.message).collect::<Vec<_>>();
                return Err(format!(
                    "Attachment download failed: {}",
                    messages.join("; ")
                ));
            }
            let content = gql
                .data
                .ok_or_else(|| "Attachment response had no data".to_string())?
                .get_chat_attachment;
            if !content.content_type.starts_with("image/") {
                return Err(format!(
                    "Attachment {} is not an image: {}",
                    content.id, content.content_type
                ));
            }
            let bytes = general_purpose::STANDARD
                .decode(content.data_base64)
                .map_err(|e| format!("Invalid attachment encoding: {}", e))?;
            if content.size >= 0 && bytes.len() as i64 != content.size {
                return Err(format!("Attachment {} size mismatch", content.id));
            }
            let file_name = Self::local_attachment_file_name(&content.original_name, &content.id);
            let local_path = attachment_dir.join(file_name);
            tokio::fs::write(&local_path, bytes)
                .await
                .map_err(|e| format!("Failed to save attachment: {}", e))?;

            let display_path = Self::display_workspace_path(&workspace_path, &local_path);
            saved_paths.push(display_path.clone());

            let _ = client
                .post(&graphql_url)
                .header("Content-Type", "application/json")
                .header("x-tenant-id", &worker_config.tenant_id)
                .header("x-user-id", &worker_config.user_id)
                .json(&serde_json::json!({
                    "query": "mutation DeleteChatAttachment($input: MarkChatAttachmentDeliveredInput!) { deleteChatAttachment(input: $input) { id status localPath } }",
                    "variables": { "input": { "id": content.id, "localPath": display_path } }
                }))
                .send()
                .await;
        }

        Ok(saved_paths)
    }

    fn local_attachment_file_name(original_name: &str, id: &str) -> String {
        let fallback = "image";
        let safe = original_name
            .trim()
            .rsplit(['/', '\\'])
            .next()
            .unwrap_or(fallback)
            .chars()
            .map(|ch| {
                if ch.is_ascii_alphanumeric() || ch == '.' || ch == '_' || ch == '-' {
                    ch
                } else {
                    '-'
                }
            })
            .collect::<String>();
        let safe = safe.trim_matches('-');
        let path = std::path::Path::new(if safe.is_empty() { fallback } else { safe });
        let stem = path
            .file_stem()
            .and_then(|value| value.to_str())
            .unwrap_or(fallback);
        let ext = path
            .extension()
            .and_then(|value| value.to_str())
            .unwrap_or("png");
        let short_id = id.get(0..8).unwrap_or(id);
        format!("{}-{}.{}", stem, short_id, ext)
    }

    fn display_workspace_path(
        workspace_path: &std::path::Path,
        local_path: &std::path::Path,
    ) -> String {
        local_path
            .strip_prefix(workspace_path)
            .ok()
            .map(|path| format!("./{}", path.to_string_lossy()))
            .unwrap_or_else(|| local_path.to_string_lossy().to_string())
    }

    /// Handle an RPC request by dispatching to the appropriate query method.
    async fn handle_rpc(req: &RpcRequest, state: &Arc<AppState>) -> RpcResponse {
        let result = match req.method.as_str() {
            "list_sessions" => Self::rpc_list_sessions(&req.params, state),
            "list_tmux_sessions" => Self::rpc_list_tmux_sessions().await,
            // Programmatic, deterministic one-shot pane capture (overhaul P2, decision D7).
            // Complements the throttled live terminal_screen push; input still rides the
            // terminal_command envelope — no new shell transport.
            "capture_terminal" => Self::rpc_capture_terminal(&req.params, state).await,
            "create_session" => Self::rpc_create_session(&req.params, state),
            "get_session" => Self::rpc_get_session(&req.params, state),
            "update_session_title" => Self::rpc_update_session_title(&req.params, state),
            "update_session_provider" => Self::rpc_update_session_provider(&req.params, state),
            "update_session_working_directory" => {
                Self::rpc_update_session_working_directory(&req.params, state)
            }
            "archive_session" => Self::rpc_archive_session(&req.params, state).await,
            "list_messages" => Self::rpc_list_messages(&req.params, state),
            "delete_session" => Self::rpc_delete_session(&req.params, state).await,
            "detect_cli_tools" => Self::rpc_detect_cli_tools(state).await,
            "create_agent_plan" => Self::rpc_create_agent_plan(&req.params, state),
            "list_agent_plans" => Self::rpc_list_agent_plans(&req.params, state),
            "get_agent_plan" => Self::rpc_get_agent_plan(&req.params, state),
            "list_initiative_events" => Self::rpc_list_initiative_events(&req.params, state),
            "start_agent_plan" => Self::rpc_start_agent_plan(&req.params, state).await,
            "run_initiative_from_phase" => {
                Self::rpc_run_initiative_from_phase(&req.params, state).await
            }
            "update_agent_plan_title" => Self::rpc_update_agent_plan_title(&req.params, state),
            "update_agent_plan_app_scope" => {
                Self::rpc_update_agent_plan_app_scope(&req.params, state)
            }
            "update_agent_plan_validation_config" => {
                Self::rpc_update_agent_plan_validation_config(&req.params, state)
            }
            "refresh_agent_plan_phases" => {
                Self::rpc_refresh_agent_plan_phases(&req.params, state)
            },
            "amend_agent_plan" => Self::rpc_amend_agent_plan(&req.params, state).await,
            "stop_agent_plan" => Self::rpc_stop_agent_plan(&req.params, state).await,
            "delete_agent_plan" => Self::rpc_delete_agent_plan(&req.params, state).await,
            "block_agent_plan" => Self::rpc_block_agent_plan(&req.params, state).await,
            "manual_pass_agent_phase" => {
                Self::rpc_manual_pass_agent_phase(&req.params, state).await
            }
            "send_agent_feedback_to_worker" => {
                Self::rpc_send_agent_feedback_to_worker(&req.params, state).await
            }
            "rerun_agent_reviewer" => Self::rpc_rerun_agent_reviewer(&req.params, state).await,
            "browse_host_directory" => Self::rpc_browse_host_directory(&req.params),
            "validate_workspace_plan" => Self::rpc_validate_workspace_plan(&req.params),
            "list_workspace_files" => Self::rpc_list_workspace_files(&req.params, state),
            "git_file_view" => Self::rpc_git_file_view(&req.params, state),
            "run_git_action" => Self::rpc_run_git_action(&req.params, state),
            "read_host_file" => Self::rpc_read_host_file(&req.params, state),
            "get_workspace_file_diff" => Self::rpc_get_workspace_file_diff(&req.params, state),
            "git_diff" => Self::rpc_git_diff(&req.params, state),
            // --- file manager (overhaul P2, files_root-rooted) ---
            "files_list_dir" => Self::rpc_files_list_dir(&req.params, state),
            "files_read" => Self::rpc_files_read(&req.params, state),
            "files_write" => Self::rpc_files_write(&req.params, state),
            "files_mkdir" => Self::rpc_files_mkdir(&req.params, state),
            "files_rename" => Self::rpc_files_rename(&req.params, state),
            "files_delete" => Self::rpc_files_delete(&req.params, state),
            "files_upload_chunk" => Self::rpc_files_upload_chunk(&req.params, state),
            "create_briefing_run" => Self::rpc_create_briefing_run(&req.params, state).await,
            "initiative_upload_chunk" => Self::rpc_initiative_upload_chunk(&req.params, state),
            "accept_brief" => Self::rpc_accept_brief(&req.params, state).await,
            "add_initiative_reference_path" => {
                Self::rpc_add_initiative_reference_path(&req.params, state)
            }
            "get_planner_prompt_settings" => Self::rpc_get_planner_prompt_settings(),
            "get_plan_check" => Self::rpc_get_plan_check(&req.params, state),
            "get_task_run" => Self::rpc_get_task_run(&req.params, state),
            "get_kloo_doctor" => Self::rpc_get_kloo_doctor().await,
            "get_kloo_probe" => Self::rpc_get_kloo_probe().await,
            "update_planner_prompt_settings" => {
                Self::rpc_update_planner_prompt_settings(&req.params)
            }
            // Forward-path replacements (Phase 2 of multi-user-saas plan).
            // These mirror the host's GraphQL mutations so the worker can reach them
            // via relay-RPC after host-graphql.ts is deleted.
            "get_setting" => Self::rpc_get_setting(&req.params, state),
            "set_setting" => Self::rpc_set_setting(&req.params, state),
            "list_provider_configs" => Self::rpc_list_provider_configs(state),
            "upsert_provider_config" => Self::rpc_upsert_provider_config(&req.params, state),
            "delete_provider_config" => Self::rpc_delete_provider_config(&req.params, state),
            "stop_ai_generation" => Self::rpc_stop_ai_generation(&req.params, state).await,
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
        let status = params
            .get("status")
            .and_then(|v| v.as_str())
            .map(str::to_string);
        let sessions = session_service::list_sessions(state, status)?
            .into_iter()
            .map(Self::session_view)
            .collect::<Vec<_>>();

        serde_json::to_value(&sessions).map_err(|e| e.to_string())
    }

    /// List external tmux sessions a new terminal can attach to (excludes the
    /// johnnyone_<id> panes JohnnyOne already manages).
    async fn rpc_list_tmux_sessions() -> Result<serde_json::Value, String> {
        let sessions = crate::terminal::list_external_tmux_sessions().await?;
        serde_json::to_value(&sessions).map_err(|e| e.to_string())
    }

    /// Deterministic one-shot capture of a session's current pane (overhaul P2). Wraps the existing
    /// `terminal::capture_terminal_session[_with_history]` — no new capture logic. When
    /// `historyLines` is present, includes recent tmux scrollback. Programmatic input is unchanged
    /// (it rides the `terminal_command` envelope via `send_terminal_input`).
    async fn rpc_capture_terminal(
        params: &serde_json::Value,
        state: &Arc<AppState>,
    ) -> Result<serde_json::Value, String> {
        let session_id = params
            .get("sessionId")
            .and_then(|v| v.as_str())
            .ok_or_else(|| "Missing 'sessionId' parameter".to_string())?;
        // C2b: honor the REQUESTED depth, not just its presence. The old code checked `.is_some()`
        // then captured the hardcoded DEFAULT_HISTORY_CAPTURE_LINES (200), so `captureTerminal(id, 1500)`
        // was a silent no-op. Thread the clamped value through to the depth-aware capture path.
        let requested_history = params.get("historyLines").and_then(|v| v.as_u64());
        let snapshot = if requested_history.is_some() {
            let history_rows = crate::terminal::clamp_history_capture_lines(requested_history);
            crate::terminal::capture_terminal_session_with_history_rows(state, session_id, history_rows)
                .await?
        } else {
            crate::terminal::capture_terminal_session(state, session_id).await?
        };
        serde_json::to_value(snapshot).map_err(|e| e.to_string())
    }

    fn rpc_create_session(
        params: &serde_json::Value,
        state: &Arc<AppState>,
    ) -> Result<serde_json::Value, String> {
        let input_value = params
            .get("input")
            .cloned()
            .unwrap_or_else(|| serde_json::json!({}));
        let input: crate::db::models::CreateSessionInput =
            serde_json::from_value(input_value).map_err(|e| e.to_string())?;
        let session = session_service::create_session(state, input)?;

        serde_json::to_value(Self::session_view(session)).map_err(|e| e.to_string())
    }

    fn rpc_get_session(
        params: &serde_json::Value,
        state: &Arc<AppState>,
    ) -> Result<serde_json::Value, String> {
        let id = params
            .get("id")
            .and_then(|v| v.as_str())
            .ok_or_else(|| "Missing 'id' parameter".to_string())?;

        let session = session_service::get_session(state, id.to_string())?;
        serde_json::to_value(Self::session_view(session)).map_err(|e| e.to_string())
    }

    fn rpc_update_session_title(
        params: &serde_json::Value,
        state: &Arc<AppState>,
    ) -> Result<serde_json::Value, String> {
        let id = params
            .get("id")
            .and_then(|v| v.as_str())
            .ok_or_else(|| "Missing 'id' parameter".to_string())?;
        let title = params
            .get("title")
            .and_then(|v| v.as_str())
            .ok_or_else(|| "Missing 'title' parameter".to_string())?;
        let session =
            session_service::update_session_title(state, id.to_string(), title.to_string())?;

        serde_json::to_value(Self::session_view(session)).map_err(|e| e.to_string())
    }

    fn rpc_update_session_provider(
        params: &serde_json::Value,
        state: &Arc<AppState>,
    ) -> Result<serde_json::Value, String> {
        let id = params
            .get("id")
            .and_then(|v| v.as_str())
            .ok_or_else(|| "Missing 'id' parameter".to_string())?;
        let provider = params
            .get("provider")
            .and_then(|v| v.as_str())
            .ok_or_else(|| "Missing 'provider' parameter".to_string())?;
        let session =
            session_service::update_session_provider(state, id.to_string(), provider.to_string())?;

        serde_json::to_value(Self::session_view(session)).map_err(|e| e.to_string())
    }

    fn rpc_update_session_working_directory(
        params: &serde_json::Value,
        state: &Arc<AppState>,
    ) -> Result<serde_json::Value, String> {
        let id = params
            .get("id")
            .and_then(|v| v.as_str())
            .ok_or_else(|| "Missing 'id' parameter".to_string())?;
        let working_directory = params
            .get("workingDirectory")
            .and_then(|v| v.as_str())
            .ok_or_else(|| "Missing 'workingDirectory' parameter".to_string())?;
        let session = session_service::update_session_working_directory(
            state,
            id.to_string(),
            working_directory.to_string(),
        )?;

        serde_json::to_value(Self::session_view(session)).map_err(|e| e.to_string())
    }

    async fn rpc_archive_session(
        params: &serde_json::Value,
        state: &Arc<AppState>,
    ) -> Result<serde_json::Value, String> {
        let id = params
            .get("id")
            .and_then(|v| v.as_str())
            .ok_or_else(|| "Missing 'id' parameter".to_string())?;
        let session = session_service::archive_session(state, id.to_string()).await?;

        serde_json::to_value(Self::session_view(session)).map_err(|e| e.to_string())
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
                    "SELECT id, session_id, role, content, tool_calls, finish_reason, input_tokens, output_tokens, cost_cents, created_at FROM messages WHERE session_id = ?1 ORDER BY created_at DESC, rowid DESC LIMIT ?2 OFFSET ?3",
                )
                .map_err(|e| e.to_string())?;

            let mut messages: Vec<RpcMessageView> = stmt
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

            messages.reverse();

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

        session_service::delete_session(state, id.to_string()).await?;

        serde_json::to_value(serde_json::json!({ "deleted": true })).map_err(|e| e.to_string())
    }

    async fn rpc_detect_cli_tools(state: &Arc<AppState>) -> Result<serde_json::Value, String> {
        let tools = provider_service::detect_cli_tools(state).await?;
        serde_json::to_value(&tools).map_err(|e| e.to_string())
    }

    fn rpc_create_agent_plan(
        params: &serde_json::Value,
        state: &Arc<AppState>,
    ) -> Result<serde_json::Value, String> {
        let input_value = params
            .get("input")
            .cloned()
            .ok_or_else(|| "Missing 'input' parameter".to_string())?;
        let input: crate::db::models::CreateAgentPlanInput =
            serde_json::from_value(input_value).map_err(|e| e.to_string())?;
        let run = crate::services::agent_plans::create_plan(state, input)?;
        serde_json::to_value(run).map_err(|e| e.to_string())
    }

    fn rpc_list_agent_plans(
        params: &serde_json::Value,
        state: &Arc<AppState>,
    ) -> Result<serde_json::Value, String> {
        let status = params
            .get("status")
            .and_then(|value| value.as_str())
            .map(str::to_string);
        let run_type = params
            .get("runType")
            .and_then(|value| value.as_str())
            .map(str::to_string);
        let only_existing = params
            .get("onlyExisting")
            .and_then(|value| value.as_bool())
            .unwrap_or(false);
        let plans = crate::services::agent_plans::list_plans(
            state,
            status,
            run_type,
            only_existing,
        )?;
        serde_json::to_value(plans).map_err(|e| e.to_string())
    }

    fn rpc_get_agent_plan(
        params: &serde_json::Value,
        state: &Arc<AppState>,
    ) -> Result<serde_json::Value, String> {
        let id = params
            .get("id")
            .and_then(|value| value.as_str())
            .ok_or_else(|| "Missing 'id' parameter".to_string())?;
        let run = crate::services::agent_plans::get_plan(state, id)?;
        serde_json::to_value(run).map_err(|e| e.to_string())
    }

    fn rpc_list_initiative_events(
        params: &serde_json::Value,
        state: &Arc<AppState>,
    ) -> Result<serde_json::Value, String> {
        let initiative_id = params
            .get("initiativeId")
            .and_then(|value| value.as_str())
            .ok_or_else(|| "Missing 'initiativeId' parameter".to_string())?;
        // Latest 2000 (DESC then reverse to oldest-first) so new events stay visible after 500.
        let events = crate::services::agent_plans::list_initiative_events(state, initiative_id, 2000)?;
        serde_json::to_value(events).map_err(|e| e.to_string())
    }

    async fn rpc_start_agent_plan(
        params: &serde_json::Value,
        state: &Arc<AppState>,
    ) -> Result<serde_json::Value, String> {
        let id = params
            .get("id")
            .and_then(|value| value.as_str())
            .ok_or_else(|| "Missing 'id' parameter".to_string())?;
        let phase_id = params
            .get("phaseId")
            .and_then(|value| value.as_str())
            .filter(|value| !value.trim().is_empty())
            .map(str::to_string);
        let phase_run_mode = params
            .get("phaseRunMode")
            .and_then(|value| value.as_str())
            .filter(|value| !value.trim().is_empty())
            .map(str::to_string);
        let run =
            crate::services::agent_plans::start_plan(
                (**state).clone(),
                id.to_string(),
                phase_id,
                phase_run_mode,
            )
            .await?;
        serde_json::to_value(run).map_err(|e| e.to_string())
    }

    /// Unified run/resume dispatch. Extracts `id`/`phaseId`/`phaseRunMode`/`comment`
    /// (empty strings coerced to `None`, same idiom as `rpc_start_agent_plan`) and
    /// delegates to the service fn, which records/injects the optional comment, clears a
    /// paused run, and (re)starts development at the chosen phase.
    async fn rpc_run_initiative_from_phase(
        params: &serde_json::Value,
        state: &Arc<AppState>,
    ) -> Result<serde_json::Value, String> {
        let id = params
            .get("id")
            .and_then(|value| value.as_str())
            .ok_or_else(|| "Missing 'id' parameter".to_string())?;
        let phase_id = params
            .get("phaseId")
            .and_then(|value| value.as_str())
            .filter(|value| !value.trim().is_empty())
            .map(str::to_string);
        let phase_run_mode = params
            .get("phaseRunMode")
            .and_then(|value| value.as_str())
            .filter(|value| !value.trim().is_empty())
            .map(str::to_string);
        // Coerce only a truly-empty string to None — do NOT trim-filter here, or a
        // whitespace-only comment would collapse to None and skip the service's
        // whitespace-only rejection (brief: whitespace-only, when provided, is rejected).
        let comment = params
            .get("comment")
            .and_then(|value| value.as_str())
            .filter(|value| !value.is_empty())
            .map(str::to_string);
        let run = crate::services::agent_plans::run_initiative_from_phase(
            (**state).clone(),
            id.to_string(),
            phase_id,
            phase_run_mode,
            comment,
        )
        .await?;
        serde_json::to_value(run).map_err(|e| e.to_string())
    }

    async fn rpc_stop_agent_plan(
        params: &serde_json::Value,
        state: &Arc<AppState>,
    ) -> Result<serde_json::Value, String> {
        let id = params
            .get("id")
            .and_then(|value| value.as_str())
            .ok_or_else(|| "Missing 'id' parameter".to_string())?;
        let run = crate::services::agent_plans::stop_plan(state, id.to_string()).await?;
        serde_json::to_value(run).map_err(|e| e.to_string())
    }

    fn rpc_update_agent_plan_title(
        params: &serde_json::Value,
        state: &Arc<AppState>,
    ) -> Result<serde_json::Value, String> {
        let id = params
            .get("id")
            .and_then(|value| value.as_str())
            .ok_or_else(|| "Missing 'id' parameter".to_string())?;
        let title = params
            .get("title")
            .and_then(|value| value.as_str())
            .ok_or_else(|| "Missing 'title' parameter".to_string())?;
        let run = crate::services::agent_plans::update_plan_title(state, id.to_string(), title.to_string())?;
        serde_json::to_value(run).map_err(|e| e.to_string())
    }

    fn rpc_update_agent_plan_app_scope(
        params: &serde_json::Value,
        state: &Arc<AppState>,
    ) -> Result<serde_json::Value, String> {
        let id = params
            .get("id")
            .and_then(|value| value.as_str())
            .ok_or_else(|| "Missing 'id' parameter".to_string())?;
        // `appScope` is optional — null/empty clears it.
        let app_scope = params
            .get("appScope")
            .and_then(|value| value.as_str())
            .map(|s| s.to_string())
            .filter(|s| !s.trim().is_empty());
        let run = crate::services::agent_plans::update_plan_app_scope(
            state,
            id.to_string(),
            app_scope,
        )?;
        serde_json::to_value(run).map_err(|e| e.to_string())
    }

    fn rpc_update_agent_plan_validation_config(
        params: &serde_json::Value,
        state: &Arc<AppState>,
    ) -> Result<serde_json::Value, String> {
        let id = params
            .get("id")
            .and_then(|value| value.as_str())
            .ok_or_else(|| "Missing 'id' parameter".to_string())?;
        // `config` is optional — null/empty clears it (→ default resolve). The JSON string is
        // validated (parse + provider) inside the service.
        let config = params
            .get("config")
            .and_then(|value| value.as_str())
            .map(|s| s.to_string())
            .filter(|s| !s.trim().is_empty());
        let run = crate::services::agent_plans::update_plan_validation_config(
            state,
            id.to_string(),
            config,
        )?;
        serde_json::to_value(run).map_err(|e| e.to_string())
    }

    fn rpc_refresh_agent_plan_phases(
        params: &serde_json::Value,
        state: &Arc<AppState>,
    ) -> Result<serde_json::Value, String> {
        let id = params
            .get("id")
            .and_then(|value| value.as_str())
            .ok_or_else(|| "Missing 'id' parameter".to_string())?;
        let run = crate::services::agent_plans::refresh_plan_phases(state, id.to_string())?;
        serde_json::to_value(run).map_err(|e| e.to_string())
    }

    async fn rpc_amend_agent_plan(
        params: &serde_json::Value,
        state: &Arc<AppState>,
    ) -> Result<serde_json::Value, String> {
        let id = params
            .get("id")
            .and_then(|value| value.as_str())
            .ok_or_else(|| "Missing 'id' parameter".to_string())?;
        let brief = params
            .get("brief")
            .and_then(|value| value.as_str())
            .ok_or_else(|| "Missing 'brief' parameter".to_string())?;
        let run = crate::services::agent_plans::amend_plan(
            (**state).clone(),
            id.to_string(),
            brief.to_string(),
        )
        .await?;
        serde_json::to_value(run).map_err(|e| e.to_string())
    }

    async fn rpc_delete_agent_plan(
        params: &serde_json::Value,
        state: &Arc<AppState>,
    ) -> Result<serde_json::Value, String> {
        let id = params
            .get("id")
            .and_then(|value| value.as_str())
            .ok_or_else(|| "Missing 'id' parameter".to_string())?;
        let deleted = crate::services::agent_plans::delete_plan(state, id.to_string()).await?;
        serde_json::to_value(deleted).map_err(|e| e.to_string())
    }

    async fn rpc_block_agent_plan(
        params: &serde_json::Value,
        state: &Arc<AppState>,
    ) -> Result<serde_json::Value, String> {
        let id = params
            .get("id")
            .and_then(|value| value.as_str())
            .ok_or_else(|| "Missing 'id' parameter".to_string())?;
        let reason = params
            .get("reason")
            .and_then(|value| value.as_str())
            .unwrap_or("Blocked by user")
            .to_string();
        let run = crate::services::agent_plans::block_plan(state, id.to_string(), reason).await?;
        serde_json::to_value(run).map_err(|e| e.to_string())
    }

    async fn rpc_manual_pass_agent_phase(
        params: &serde_json::Value,
        state: &Arc<AppState>,
    ) -> Result<serde_json::Value, String> {
        let id = params
            .get("id")
            .and_then(|value| value.as_str())
            .ok_or_else(|| "Missing 'id' parameter".to_string())?;
        let phase_id = params
            .get("phaseId")
            .and_then(|value| value.as_str())
            .ok_or_else(|| "Missing 'phaseId' parameter".to_string())?;
        let run = crate::services::agent_plans::manual_pass_phase(
            (**state).clone(),
            id.to_string(),
            phase_id.to_string(),
        )
        .await?;
        serde_json::to_value(run).map_err(|e| e.to_string())
    }

    async fn rpc_send_agent_feedback_to_worker(
        params: &serde_json::Value,
        state: &Arc<AppState>,
    ) -> Result<serde_json::Value, String> {
        let id = params
            .get("id")
            .and_then(|value| value.as_str())
            .ok_or_else(|| "Missing 'id' parameter".to_string())?;
        let run = crate::services::agent_plans::send_feedback_to_worker(
            (**state).clone(),
            id.to_string(),
        )
        .await?;
        serde_json::to_value(run).map_err(|e| e.to_string())
    }

    async fn rpc_rerun_agent_reviewer(
        params: &serde_json::Value,
        state: &Arc<AppState>,
    ) -> Result<serde_json::Value, String> {
        let id = params
            .get("id")
            .and_then(|value| value.as_str())
            .ok_or_else(|| "Missing 'id' parameter".to_string())?;
        let run =
            crate::services::agent_plans::rerun_reviewer((**state).clone(), id.to_string()).await?;
        serde_json::to_value(run).map_err(|e| e.to_string())
    }

    fn rpc_browse_host_directory(params: &serde_json::Value) -> Result<serde_json::Value, String> {
        let path = params
            .get("path")
            .and_then(|value| value.as_str())
            .ok_or_else(|| "Missing 'path' parameter".to_string())?;
        let entries = crate::services::agent_plans::browse_host_directory(path.to_string())?;
        serde_json::to_value(entries).map_err(|e| e.to_string())
    }

    fn rpc_validate_workspace_plan(
        params: &serde_json::Value,
    ) -> Result<serde_json::Value, String> {
        let workspace_path = params
            .get("workspacePath")
            .and_then(|value| value.as_str())
            .ok_or_else(|| "Missing 'workspacePath' parameter".to_string())?;
        let plan_path = params
            .get("planPath")
            .and_then(|value| value.as_str())
            .ok_or_else(|| "Missing 'planPath' parameter".to_string())?;
        let result = crate::services::agent_plans::validate_workspace_and_plan_path(
            workspace_path.to_string(),
            plan_path.to_string(),
        );
        serde_json::to_value(result).map_err(|e| e.to_string())
    }

    fn rpc_list_workspace_files(
        params: &serde_json::Value,
        state: &Arc<AppState>,
    ) -> Result<serde_json::Value, String> {
        let id = params
            .get("id")
            .and_then(|value| value.as_str())
            .ok_or_else(|| "Missing 'id' parameter".to_string())?;
        let mode = params
            .get("mode")
            .and_then(|value| value.as_str())
            .unwrap_or("changed");
        let path = params
            .get("path")
            .and_then(|value| value.as_str())
            .map(|value| value.to_string());
        let entries = crate::services::agent_plans::list_workspace_files(
            state,
            id.to_string(),
            mode.to_string(),
            path,
        )?;
        serde_json::to_value(entries).map_err(|e| e.to_string())
    }

    fn rpc_git_file_view(
        params: &serde_json::Value,
        state: &Arc<AppState>,
    ) -> Result<serde_json::Value, String> {
        let id = params
            .get("id")
            .and_then(|value| value.as_str())
            .ok_or_else(|| "Missing 'id' parameter".to_string())?;
        let path = params
            .get("path")
            .and_then(|value| value.as_str())
            .map(|value| value.to_string());
        let view = crate::services::agent_plans::git_file_view(state, id.to_string(), path)?;
        serde_json::to_value(view).map_err(|e| e.to_string())
    }

    fn rpc_run_git_action(
        params: &serde_json::Value,
        state: &Arc<AppState>,
    ) -> Result<serde_json::Value, String> {
        let id = params
            .get("id")
            .and_then(|value| value.as_str())
            .ok_or_else(|| "Missing 'id' parameter".to_string())?;
        let action = params
            .get("action")
            .and_then(|value| value.as_str())
            .ok_or_else(|| "Missing 'action' parameter".to_string())?;
        let path = params
            .get("path")
            .and_then(|value| value.as_str())
            .map(|value| value.to_string());
        let message = params
            .get("message")
            .and_then(|value| value.as_str())
            .map(|value| value.to_string());
        let result = crate::services::agent_plans::run_git_action(
            state,
            id.to_string(),
            path,
            action.to_string(),
            message,
        )?;
        serde_json::to_value(result).map_err(|e| e.to_string())
    }

    fn rpc_read_host_file(
        params: &serde_json::Value,
        state: &Arc<AppState>,
    ) -> Result<serde_json::Value, String> {
        let id = params
            .get("id")
            .and_then(|value| value.as_str())
            .ok_or_else(|| "Missing 'id' parameter".to_string())?;
        let path = params
            .get("path")
            .and_then(|value| value.as_str())
            .ok_or_else(|| "Missing 'path' parameter".to_string())?;
        let content =
            crate::services::agent_plans::read_host_file(state, id.to_string(), path.to_string())?;
        serde_json::to_value(content).map_err(|e| e.to_string())
    }

    fn rpc_get_workspace_file_diff(
        params: &serde_json::Value,
        state: &Arc<AppState>,
    ) -> Result<serde_json::Value, String> {
        let id = params
            .get("id")
            .and_then(|value| value.as_str())
            .ok_or_else(|| "Missing 'id' parameter".to_string())?;
        let path = params
            .get("path")
            .and_then(|value| value.as_str())
            .ok_or_else(|| "Missing 'path' parameter".to_string())?;
        let diff = crate::services::agent_plans::get_workspace_file_diff(
            state,
            id.to_string(),
            path.to_string(),
        )?;
        serde_json::to_value(diff).map_err(|e| e.to_string())
    }

    /// Whole-tree working-tree diff, path-keyed off a session's `workingDirectory` (overhaul P7,
    /// D7/D10). Mirrors `rpc_get_workspace_file_diff` but takes only `path` (no plan id) — the repo
    /// is resolved from the path. The `files:read` scope gate lives in the worker resolver (D13).
    fn rpc_git_diff(
        params: &serde_json::Value,
        state: &Arc<AppState>,
    ) -> Result<serde_json::Value, String> {
        let path = params
            .get("path")
            .and_then(|value| value.as_str())
            .ok_or_else(|| "Missing 'path' parameter".to_string())?;
        let view = crate::services::agent_plans::git_diff(state, path.to_string())?;
        serde_json::to_value(view).map_err(|e| e.to_string())
    }

    // --- file manager (overhaul P2, files_root-rooted) ---
    // Thin param-pullers mirroring rpc_read_host_file: read typed params from the JSON value, call
    // the host_files service, and serialize the result. camelCase param keys match the worker
    // resolver (Phase 02) and ui method (Phase 03). No params are logged — they carry file content.

    fn rpc_files_list_dir(
        params: &serde_json::Value,
        state: &Arc<AppState>,
    ) -> Result<serde_json::Value, String> {
        let path = params
            .get("path")
            .and_then(|v| v.as_str())
            .ok_or_else(|| "Missing 'path' parameter".to_string())?;
        let listing = crate::services::host_files::list_dir(state, path)?;
        serde_json::to_value(listing).map_err(|e| e.to_string())
    }

    fn rpc_files_read(
        params: &serde_json::Value,
        state: &Arc<AppState>,
    ) -> Result<serde_json::Value, String> {
        let path = params
            .get("path")
            .and_then(|v| v.as_str())
            .ok_or_else(|| "Missing 'path' parameter".to_string())?;
        let content = crate::services::host_files::read_file(state, path)?;
        serde_json::to_value(content).map_err(|e| e.to_string())
    }

    fn rpc_files_write(
        params: &serde_json::Value,
        state: &Arc<AppState>,
    ) -> Result<serde_json::Value, String> {
        let path = params
            .get("path")
            .and_then(|v| v.as_str())
            .ok_or_else(|| "Missing 'path' parameter".to_string())?;
        let content = params
            .get("content")
            .and_then(|v| v.as_str())
            .ok_or_else(|| "Missing 'content' parameter".to_string())?;
        let encoding = params
            .get("encoding")
            .and_then(|v| v.as_str())
            .unwrap_or("utf8");
        let result = crate::services::host_files::write_file(state, path, content, encoding)?;
        serde_json::to_value(result).map_err(|e| e.to_string())
    }

    fn rpc_files_mkdir(
        params: &serde_json::Value,
        state: &Arc<AppState>,
    ) -> Result<serde_json::Value, String> {
        let path = params
            .get("path")
            .and_then(|v| v.as_str())
            .ok_or_else(|| "Missing 'path' parameter".to_string())?;
        let result = crate::services::host_files::mkdir(state, path)?;
        serde_json::to_value(result).map_err(|e| e.to_string())
    }

    fn rpc_files_rename(
        params: &serde_json::Value,
        state: &Arc<AppState>,
    ) -> Result<serde_json::Value, String> {
        let from = params
            .get("from")
            .and_then(|v| v.as_str())
            .ok_or_else(|| "Missing 'from' parameter".to_string())?;
        let to = params
            .get("to")
            .and_then(|v| v.as_str())
            .ok_or_else(|| "Missing 'to' parameter".to_string())?;
        let result = crate::services::host_files::rename(state, from, to)?;
        serde_json::to_value(result).map_err(|e| e.to_string())
    }

    fn rpc_files_delete(
        params: &serde_json::Value,
        state: &Arc<AppState>,
    ) -> Result<serde_json::Value, String> {
        let path = params
            .get("path")
            .and_then(|v| v.as_str())
            .ok_or_else(|| "Missing 'path' parameter".to_string())?;
        let result = crate::services::host_files::delete(state, path)?;
        serde_json::to_value(result).map_err(|e| e.to_string())
    }

    fn rpc_files_upload_chunk(
        params: &serde_json::Value,
        state: &Arc<AppState>,
    ) -> Result<serde_json::Value, String> {
        let path = params
            .get("path")
            .and_then(|v| v.as_str())
            .ok_or_else(|| "Missing 'path' parameter".to_string())?;
        let chunk_index = params
            .get("chunkIndex")
            .and_then(|v| v.as_u64())
            .ok_or_else(|| "Missing 'chunkIndex' parameter".to_string())?;
        let total_chunks = params
            .get("totalChunks")
            .and_then(|v| v.as_u64())
            .unwrap_or(0);
        let data_base64 = params
            .get("dataBase64")
            .and_then(|v| v.as_str())
            .ok_or_else(|| "Missing 'dataBase64' parameter".to_string())?;
        let done = params.get("done").and_then(|v| v.as_bool()).unwrap_or(false);
        let result = crate::services::host_files::upload_chunk(
            state,
            path,
            chunk_index,
            total_chunks,
            data_base64,
            done,
        )?;
        serde_json::to_value(result).map_err(|e| e.to_string())
    }

    /// Overhaul P4 (D1): create an Initiative in `briefing`. Thin wrapper over
    /// `agent_plans::create_briefing_run` — deserialize `{ input: CreateBriefingInput }`.
    async fn rpc_create_briefing_run(
        params: &serde_json::Value,
        state: &Arc<AppState>,
    ) -> Result<serde_json::Value, String> {
        let input_value = params
            .get("input")
            .cloned()
            .ok_or_else(|| "Missing 'input' parameter".to_string())?;
        let input: crate::db::models::CreateBriefingInput =
            serde_json::from_value(input_value).map_err(|e| e.to_string())?;
        let run = crate::services::agent_plans::create_briefing_run(state, input).await?;
        serde_json::to_value(run).map_err(|e| e.to_string())
    }

    /// Overhaul P4 (D5): upload a briefing attachment into `<id>/attachments/`. Mirrors
    /// `rpc_files_upload_chunk` but routes to the initiative-rooted engine. Never logs `dataBase64`.
    fn rpc_initiative_upload_chunk(
        params: &serde_json::Value,
        state: &Arc<AppState>,
    ) -> Result<serde_json::Value, String> {
        let initiative_id = params
            .get("initiativeId")
            .and_then(|v| v.as_str())
            .ok_or_else(|| "Missing 'initiativeId' parameter".to_string())?;
        let path = params
            .get("path")
            .and_then(|v| v.as_str())
            .ok_or_else(|| "Missing 'path' parameter".to_string())?;
        let chunk_index = params
            .get("chunkIndex")
            .and_then(|v| v.as_u64())
            .ok_or_else(|| "Missing 'chunkIndex' parameter".to_string())?;
        let total_chunks = params
            .get("totalChunks")
            .and_then(|v| v.as_u64())
            .unwrap_or(0);
        let data_base64 = params
            .get("dataBase64")
            .and_then(|v| v.as_str())
            .ok_or_else(|| "Missing 'dataBase64' parameter".to_string())?;
        let done = params.get("done").and_then(|v| v.as_bool()).unwrap_or(false);
        let result = crate::services::host_files::initiative_upload_chunk(
            state,
            initiative_id,
            path,
            chunk_index,
            total_chunks,
            data_base64,
            done,
        )?;
        serde_json::to_value(result).map_err(|e| e.to_string())
    }

    /// Overhaul P4 (D1/D4): accept a briefing brief — flips the same row briefing→planning and starts
    /// the planner. Thin wrapper over `agent_plans::accept_brief`.
    async fn rpc_accept_brief(
        params: &serde_json::Value,
        state: &Arc<AppState>,
    ) -> Result<serde_json::Value, String> {
        let initiative_id = params
            .get("initiativeId")
            .and_then(|v| v.as_str())
            .ok_or_else(|| "Missing 'initiativeId' parameter".to_string())?;
        let final_brief = params
            .get("finalBrief")
            .and_then(|v| v.as_str())
            .map(str::to_string);
        let run =
            crate::services::agent_plans::accept_brief(state, initiative_id, final_brief).await?;
        serde_json::to_value(run).map_err(|e| e.to_string())
    }

    /// Overhaul P4 (D6): record a ▤ Reference host path on a briefing Initiative. Thin wrapper over
    /// `agent_plans::add_initiative_reference_path`.
    fn rpc_add_initiative_reference_path(
        params: &serde_json::Value,
        state: &Arc<AppState>,
    ) -> Result<serde_json::Value, String> {
        let initiative_id = params
            .get("initiativeId")
            .and_then(|v| v.as_str())
            .ok_or_else(|| "Missing 'initiativeId' parameter".to_string())?;
        let path = params
            .get("path")
            .and_then(|v| v.as_str())
            .ok_or_else(|| "Missing 'path' parameter".to_string())?;
        let run =
            crate::services::agent_plans::add_initiative_reference_path(state, initiative_id, path)?;
        serde_json::to_value(run).map_err(|e| e.to_string())
    }

    fn rpc_get_planner_prompt_settings() -> Result<serde_json::Value, String> {
        let settings = crate::services::planner_prompts::load_prompt_settings()?;
        serde_json::to_value(settings).map_err(|e| e.to_string())
    }

    fn rpc_get_plan_check(
        params: &serde_json::Value,
        state: &Arc<AppState>,
    ) -> Result<serde_json::Value, String> {
        let plan_id = params
            .get("planId")
            .and_then(|v| v.as_str())
            .ok_or_else(|| "Missing 'planId' parameter".to_string())?;
        let phase_id = params.get("phaseId").and_then(|v| v.as_str());
        match crate::services::agent_plans::get_plan_check_json(state, plan_id, phase_id)? {
            Some(raw) => Ok(serde_json::Value::String(raw)),
            None => Ok(serde_json::Value::Null),
        }
    }

    fn rpc_get_task_run(
        params: &serde_json::Value,
        state: &Arc<AppState>,
    ) -> Result<serde_json::Value, String> {
        let plan_id = params
            .get("planId")
            .and_then(|v| v.as_str())
            .ok_or_else(|| "Missing 'planId' parameter".to_string())?;
        let phase_id = params
            .get("phaseId")
            .and_then(|v| v.as_str())
            .ok_or_else(|| "Missing 'phaseId' parameter".to_string())?;
        let task_id = params.get("taskId").and_then(|v| v.as_str());
        match crate::services::agent_plans::get_task_run_json(state, plan_id, phase_id, task_id)? {
            Some(raw) => Ok(serde_json::Value::String(raw)),
            None => Ok(serde_json::Value::Null),
        }
    }

    async fn rpc_get_kloo_doctor() -> Result<serde_json::Value, String> {
        tokio::task::spawn_blocking(crate::services::agent_plans::get_kloo_doctor_json)
            .await
            .map(serde_json::Value::String)
            .map_err(|e| format!("kloo doctor join: {e}"))
    }

    async fn rpc_get_kloo_probe() -> Result<serde_json::Value, String> {
        tokio::task::spawn_blocking(crate::services::agent_plans::get_kloo_probe_json)
            .await
            .map(serde_json::Value::String)
            .map_err(|e| format!("kloo probe join: {e}"))
    }

    fn rpc_update_planner_prompt_settings(
        params: &serde_json::Value,
    ) -> Result<serde_json::Value, String> {
        let input = params
            .get("input")
            .cloned()
            .ok_or_else(|| "Missing 'input' parameter".to_string())?;
        let current = crate::services::planner_prompts::load_prompt_settings()?;
        let development: crate::services::planner_prompts::PlannerDevelopmentOverlay =
            serde_json::from_value(
                input
                    .get("development")
                    .cloned()
                    .ok_or_else(|| "Missing development prompts".to_string())?,
            )
            .map_err(|e| e.to_string())?;
        let planning: crate::services::planner_prompts::PlannerPlanningOverlay =
            serde_json::from_value(
                input
                    .get("planning")
                    .cloned()
                    .ok_or_else(|| "Missing planning prompts".to_string())?,
            )
            .map_err(|e| e.to_string())?;
        let small_mode = match input.get("smallMode") {
            Some(v) if !v.is_null() => Some(
                serde_json::from_value(v.clone()).map_err(|e| e.to_string())?,
            ),
            _ => None,
        };
        let merged = crate::services::planner_prompts::overlay_prompt_settings(
            current, development, planning, small_mode,
        );
        let saved = crate::services::planner_prompts::save_prompt_settings(merged)?;
        serde_json::to_value(saved).map_err(|e| e.to_string())
    }

    // ── Phase 2 forward-path replacements ────────────────────────────────────
    // Each method below mirrors a corresponding host GraphQL mutation/query.
    // The worker reaches these via ChatRelayDO `/relay-rpc` once Phase 2 deletes
    // host-graphql.ts. Until then, both the GraphQL handlers in host/mod.rs and
    // these RPC handlers coexist (they delegate to the same service functions).

    fn rpc_get_setting(
        params: &serde_json::Value,
        state: &Arc<AppState>,
    ) -> Result<serde_json::Value, String> {
        let key = params
            .get("key")
            .and_then(|v| v.as_str())
            .ok_or_else(|| "Missing 'key' parameter".to_string())?;
        let value = settings_service::get_setting(state, key.to_string())?;
        serde_json::to_value(value).map_err(|e| e.to_string())
    }

    fn rpc_set_setting(
        params: &serde_json::Value,
        state: &Arc<AppState>,
    ) -> Result<serde_json::Value, String> {
        let key = params
            .get("key")
            .and_then(|v| v.as_str())
            .ok_or_else(|| "Missing 'key' parameter".to_string())?;
        let value = params
            .get("value")
            .and_then(|v| v.as_str())
            .ok_or_else(|| "Missing 'value' parameter".to_string())?;
        settings_service::set_setting(state, key.to_string(), value.to_string())?;
        // Reconnect-on-save for connection-relevant keys. This RPC runs inside
        // the relay loop, so spawn the reconnect (which aborts + respawns that
        // loop) rather than awaiting it inline.
        if crate::services::relay::is_connection_key(key) {
            let st = (**state).clone();
            tokio::spawn(async move {
                crate::services::relay::reconnect(st).await;
            });
        }
        Ok(serde_json::json!(true))
    }

    fn rpc_list_provider_configs(state: &Arc<AppState>) -> Result<serde_json::Value, String> {
        let configs = provider_service::list_provider_configs(state)?;
        serde_json::to_value(configs).map_err(|e| e.to_string())
    }

    fn rpc_upsert_provider_config(
        params: &serde_json::Value,
        state: &Arc<AppState>,
    ) -> Result<serde_json::Value, String> {
        let input_value = params
            .get("input")
            .cloned()
            .ok_or_else(|| "Missing 'input' parameter".to_string())?;
        let input: crate::db::models::UpsertProviderConfigInput =
            serde_json::from_value(input_value).map_err(|e| e.to_string())?;
        let config = provider_service::upsert_provider_config(state, input)?;
        serde_json::to_value(config).map_err(|e| e.to_string())
    }

    fn rpc_delete_provider_config(
        params: &serde_json::Value,
        state: &Arc<AppState>,
    ) -> Result<serde_json::Value, String> {
        let provider = params
            .get("provider")
            .and_then(|v| v.as_str())
            .ok_or_else(|| "Missing 'provider' parameter".to_string())?;
        provider_service::delete_provider_config(state, provider.to_string())?;
        Ok(serde_json::json!(true))
    }

    async fn rpc_stop_ai_generation(
        params: &serde_json::Value,
        state: &Arc<AppState>,
    ) -> Result<serde_json::Value, String> {
        let session_id = params
            .get("sessionId")
            .and_then(|v| v.as_str())
            .or_else(|| params.get("session_id").and_then(|v| v.as_str()))
            .ok_or_else(|| "Missing 'sessionId' parameter".to_string())?;
        chat_host::stop_generation(state, session_id.to_string()).await?;
        Ok(serde_json::json!(true))
    }

    fn session_view(session: crate::db::models::Session) -> RpcSessionView {
        RpcSessionView {
            id: session.id,
            title: session.title,
            provider: session.provider,
            model: session.model,
            working_directory: session.working_directory,
            status: session.status,
            total_input_tokens: session.total_input_tokens,
            total_output_tokens: session.total_output_tokens,
            total_cost_cents: session.total_cost_cents,
            created_at: session.created_at,
            updated_at: session.updated_at,
            attached_tmux: session.attached_tmux,
        }
    }

    /// Map a provider `StreamChunk` to a structured `StreamEvent` (overhaul P2, D6), or `None` for
    /// chunk kinds that aren't part of the transcript lane (System/Result/Stderr — those carry
    /// session/usage metadata, not renderable events). This is the one real emit path exercised this
    /// phase; the full per-provider mapping (code fences, mermaid, tool arg/result shaping) is
    /// Phase 3. Pure — unit-tested in isolation.
    fn stream_event_from_chunk(
        session_id: &str,
        seq: u64,
        chunk: &StreamChunk,
    ) -> Option<crate::events::StreamEvent> {
        use crate::events::StreamEvent;
        let (kind, text, tool_name, data) = match chunk.chunk_type {
            ChunkType::Text => ("text", Some(chunk.content.clone()), None, None),
            ChunkType::ToolUse => (
                "tool_call",
                None,
                chunk.tool_name.clone(),
                chunk.tool_input.clone(),
            ),
            ChunkType::ToolResult => (
                "tool_result",
                chunk.tool_output.clone(),
                chunk.tool_name.clone(),
                None,
            ),
            ChunkType::Error => ("error", Some(chunk.content.clone()), None, None),
            ChunkType::System | ChunkType::Result | ChunkType::Stderr => return None,
        };
        Some(StreamEvent {
            session_id: session_id.to_string(),
            seq,
            kind: kind.to_string(),
            text,
            language: None,
            tool_name,
            data,
            r#final: Some(chunk.is_final),
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
        let cli_sid_ref = if matches!(
            provider,
            CliProvider::ClaudeCode | CliProvider::Codex | CliProvider::Grok
        ) {
            persisted_cli_session_id.as_deref()
        } else {
            None
        };
        // Shell sessions are terminal-mode only; they don't go through the
        // chat-message dispatch. Reject explicitly before the match.
        if matches!(provider, CliProvider::Shell) {
            return Err(
                "Shell sessions don't support chat-mode messages — type into the terminal pane instead.".to_string(),
            );
        }
        if matches!(provider, CliProvider::Kloo) {
            return Err("kloo is a oneshot executor, not a chat provider".to_string());
        }

        let config = match provider {
            CliProvider::ClaudeCode => claude_code::build_config(
                &req.content,
                &working_dir,
                &model,
                cli_path_ref,
                cli_sid_ref,
            ),
            CliProvider::Codex => codex::build_config(
                &req.content,
                &working_dir,
                &model,
                cli_path_ref,
                cli_sid_ref,
            ),
            CliProvider::Grok => grok::build_config(
                &req.content,
                &working_dir,
                &model,
                cli_path_ref,
                cli_sid_ref,
            ),
            CliProvider::Cline => {
                cline::build_config(&req.content, &working_dir, &model, cli_path_ref)
            }
            CliProvider::Ollama => {
                ollama_cli::build_config(&req.content, &working_dir, &model, cli_path_ref)
            }
            CliProvider::Shell => unreachable!("shell provider already rejected"),
            CliProvider::Kloo => unreachable!("kloo provider already rejected"),
        };

        let parse_fn: fn(&str) -> Option<StreamChunk> = match provider {
            CliProvider::ClaudeCode => claude_code::parse_line,
            CliProvider::Codex => codex::parse_line,
            CliProvider::Grok => grok::parse_line,
            CliProvider::Cline => cline::parse_line,
            CliProvider::Ollama => ollama_cli::parse_line,
            CliProvider::Shell => unreachable!("shell provider already rejected"),
            CliProvider::Kloo => unreachable!("kloo provider already rejected"),
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
        // Monotonic ordering key for the structured StreamEvent lane (overhaul P2, D6). Per-turn;
        // full per-session global sequencing is a Phase-3 concern.
        let mut stream_seq: u64 = 0;

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
            drop(writer);

            // Additive: also emit the structured StreamEvent lane (overhaul P2, D6). Purely
            // parallel to the delta push above — the existing chat path is unchanged. Ignore the
            // send error when there are no subscribers (matches the terminal/chat_delta convention).
            if let Some(ev) = Self::stream_event_from_chunk(&req.session_id, stream_seq, &chunk) {
                stream_seq += 1;
                let _ = state.stream_event_tx.send(ev);
            }

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

            if matches!(
                provider,
                CliProvider::ClaudeCode | CliProvider::Codex | CliProvider::Grok
            ) {
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

#[cfg(test)]
mod files_rpc_tests {
    use super::*;
    use crate::services::settings::{set_setting, KEY_FILES_ROOT};
    use crate::test_support::{test_state, tmp_dir};
    use serde_json::json;

    fn rpc(method: &str, params: serde_json::Value) -> RpcRequest {
        RpcRequest {
            request_id: "req-1".to_string(),
            method: method.to_string(),
            params,
        }
    }

    /// Asserts the relay-RPC dispatch actually routes the `files_*` methods into `host_files`
    /// (write → read → list round-trips through `handle_rpc`), and that an unknown `files_bogus`
    /// still falls through to the `_ =>` unknown-method arm.
    #[tokio::test]
    async fn handle_rpc_routes_files_methods() {
        let (state, _root) = test_state();
        let state = Arc::new(state);
        let files_root = tmp_dir("rpc-files");
        set_setting(
            &state,
            KEY_FILES_ROOT.to_string(),
            files_root.to_string_lossy().to_string(),
        )
        .unwrap();

        let write = AgentService::handle_rpc(
            &rpc(
                "files_write",
                json!({ "path": "note.txt", "content": "wired", "encoding": "utf8" }),
            ),
            &state,
        )
        .await;
        assert!(write.success, "files_write should succeed: {:?}", write.error);

        let read = AgentService::handle_rpc(&rpc("files_read", json!({ "path": "note.txt" })), &state)
            .await;
        assert!(read.success);
        let data = read.data.expect("read data");
        assert_eq!(data.get("content").and_then(|v| v.as_str()), Some("wired"));
        assert_eq!(
            data.get("contentType").and_then(|v| v.as_str()),
            Some("text/plain")
        );

        let list =
            AgentService::handle_rpc(&rpc("files_list_dir", json!({ "path": "." })), &state).await;
        assert!(list.success);
        let entries = list
            .data
            .expect("list data")
            .get("entries")
            .and_then(|v| v.as_array())
            .cloned()
            .unwrap_or_default();
        assert!(entries
            .iter()
            .any(|e| e.get("name").and_then(|v| v.as_str()) == Some("note.txt")));

        let bogus = AgentService::handle_rpc(&rpc("files_bogus", json!({})), &state).await;
        assert!(!bogus.success);
        assert!(bogus.error.unwrap_or_default().contains("Unknown RPC method"));
    }
}

#[cfg(test)]
mod terminal_rpc_tests {
    use super::*;
    use crate::test_support::test_state;
    use serde_json::json;

    fn rpc(method: &str, params: serde_json::Value) -> RpcRequest {
        RpcRequest {
            request_id: "req-1".to_string(),
            method: method.to_string(),
            params,
        }
    }

    /// Asserts the `capture_terminal` relay-RPC arm exists and forwards to the real
    /// `terminal::capture_terminal_session` (not a stub): a missing `sessionId` is rejected by the
    /// param-pull, and an unknown session id surfaces the capture path's own "not found" error
    /// (NOT the `_ =>` unknown-method error — that would mean the arm was never wired). The live
    /// tmux drive (send input → capture echoed output) is a deferred operator check (D9): a
    /// `cargo test` cannot reliably spawn a pane in this harness.
    #[tokio::test]
    async fn capture_terminal_wires_to_capture_session() {
        let (state, _root) = test_state();
        let state = Arc::new(state);

        let missing = AgentService::handle_rpc(&rpc("capture_terminal", json!({})), &state).await;
        assert!(!missing.success);
        assert!(missing
            .error
            .unwrap_or_default()
            .contains("Missing 'sessionId'"));

        let unknown = AgentService::handle_rpc(
            &rpc("capture_terminal", json!({ "sessionId": "no-such-session" })),
            &state,
        )
        .await;
        assert!(!unknown.success);
        let err = unknown.error.unwrap_or_default();
        // Reached the capture path (session lookup failed) rather than the unknown-method arm.
        assert!(
            !err.contains("Unknown RPC method"),
            "capture_terminal arm was not wired: {err}"
        );
    }
}

#[cfg(test)]
mod stream_event_tests {
    use super::*;
    use crate::events::StreamEvent;
    use crate::providers::{ChunkType, StreamChunk};
    use crate::test_support::test_state;

    fn chunk(chunk_type: ChunkType, content: &str) -> StreamChunk {
        StreamChunk {
            chunk_type,
            content: content.to_string(),
            tool_name: None,
            tool_input: None,
            tool_output: None,
            cost_usd: None,
            input_tokens: None,
            output_tokens: None,
            is_final: false,
            session_id: None,
        }
    }

    /// AC2 — the wire contract: camelCase field keys and the tagged `type:"stream_event"`
    /// discriminator matching the DO envelope literal and the TS interface.
    #[test]
    fn stream_event_serializes_camelcase_and_tagged() {
        let ev = StreamEvent {
            session_id: "sess-1".to_string(),
            seq: 3,
            kind: "tool_call".to_string(),
            text: None,
            language: None,
            tool_name: Some("bash".to_string()),
            data: Some(serde_json::json!({ "cmd": "ls" })),
            r#final: Some(true),
        };
        let value = serde_json::to_value(&ev).unwrap();
        assert_eq!(value.get("sessionId").and_then(|v| v.as_str()), Some("sess-1"));
        assert_eq!(value.get("toolName").and_then(|v| v.as_str()), Some("bash"));
        assert!(value.get("session_id").is_none());
        assert!(value.get("tool_name").is_none());

        let wrapped = AgentMessage::StreamEvent(ev);
        let tagged = serde_json::to_value(&wrapped).unwrap();
        assert_eq!(tagged.get("type").and_then(|v| v.as_str()), Some("stream_event"));
        assert_eq!(
            tagged.pointer("/data/sessionId").and_then(|v| v.as_str()),
            Some("sess-1")
        );
    }

    /// AC3 (channel wiring) — an event sent on `stream_event_tx` reaches a subscriber intact.
    #[tokio::test]
    async fn stream_event_channel_delivers() {
        let (state, _root) = test_state();
        let mut rx = state.stream_event_tx.subscribe();
        let sent = StreamEvent {
            session_id: "s".to_string(),
            seq: 1,
            kind: "text".to_string(),
            text: Some("hello".to_string()),
            language: None,
            tool_name: None,
            data: None,
            r#final: Some(false),
        };
        state.stream_event_tx.send(sent).unwrap();
        let got = rx.recv().await.unwrap();
        assert_eq!(got.kind, "text");
        assert_eq!(got.text.as_deref(), Some("hello"));
        assert_eq!(got.seq, 1);
    }

    /// AC3 (real emit path) — the provider-chunk → StreamEvent mapping used in `handle_relay_chat`.
    #[test]
    fn stream_event_from_chunk_maps_kinds() {
        let text = AgentService::stream_event_from_chunk("s", 0, &chunk(ChunkType::Text, "hi"));
        assert_eq!(text.as_ref().map(|e| e.kind.as_str()), Some("text"));
        assert_eq!(text.and_then(|e| e.text), Some("hi".to_string()));

        let mut tool = chunk(ChunkType::ToolUse, "");
        tool.tool_name = Some("bash".to_string());
        tool.tool_input = Some(serde_json::json!({ "cmd": "ls" }));
        let call = AgentService::stream_event_from_chunk("s", 1, &tool).unwrap();
        assert_eq!(call.kind, "tool_call");
        assert_eq!(call.tool_name.as_deref(), Some("bash"));
        assert!(call.data.is_some());

        let mut result = chunk(ChunkType::ToolResult, "");
        result.tool_output = Some("done".to_string());
        let res = AgentService::stream_event_from_chunk("s", 2, &result).unwrap();
        assert_eq!(res.kind, "tool_result");
        assert_eq!(res.text.as_deref(), Some("done"));

        let err = AgentService::stream_event_from_chunk("s", 3, &chunk(ChunkType::Error, "boom"));
        assert_eq!(err.map(|e| e.kind), Some("error".to_string()));

        // Metadata chunks are not transcript events.
        assert!(AgentService::stream_event_from_chunk("s", 4, &chunk(ChunkType::System, "")).is_none());
        assert!(AgentService::stream_event_from_chunk("s", 5, &chunk(ChunkType::Result, "")).is_none());
    }
}
