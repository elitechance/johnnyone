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
use std::time::{Duration, Instant};

/// A shell agent that answered but never reported gets one reminder after this long.
const SHELL_NUDGE_AFTER_SECS: u64 = 60;
/// Gap between the two captures used to decide the pane has actually gone quiet.
const SHELL_NUDGE_IDLE_PROBE_SECS: u64 = 3;
/// Treat the relay as dead once no heartbeat ack has arrived for this long.
///
/// Heartbeats go out every 30s (`heartbeat::HEARTBEAT_INTERVAL_SECS`), so this allows three to go
/// missing before acting — long enough to ride out a slow network or a Durable Object hiccup,
/// short enough that the phone is not stranded for an hour.
const HEARTBEAT_STALE_AFTER_SECS: i64 = 95;
/// How often to test that threshold.
const HEARTBEAT_LIVENESS_CHECK_SECS: u64 = 15;

/// Whether the relay should be considered dead, given the last ack we saw.
///
/// Split out from the watchdog's select arm so the threshold is testable without standing up a
/// WebSocket. `None` means no ack has ever been recorded; the connect path seeds a timestamp, so
/// that only happens before a connection exists and must NOT count as stale.
fn heartbeat_is_stale(
    last_heartbeat: Option<chrono::DateTime<chrono::Utc>>,
    now: chrono::DateTime<chrono::Utc>,
) -> bool {
    match last_heartbeat {
        Some(last) => now.signed_duration_since(last).num_seconds() >= HEARTBEAT_STALE_AFTER_SECS,
        None => false,
    }
}
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

        // Re-resolve the credential on every start (D10). Do not rely only on
        // the spawn-time AgentConfig clone in relay.rs.
        let resolved = settings_service::RelayConfig::resolve(&*state);
        let worker_url = resolved
            .as_ref()
            .map(|c| c.worker_url.clone())
            .unwrap_or_else(|| config.worker_url.clone());
        let user_id = resolved
            .as_ref()
            .map(|c| c.user_id.clone())
            .unwrap_or_else(|| config.user_id.clone());
        let tenant_id = resolved
            .as_ref()
            .map(|c| c.tenant_id.clone())
            .unwrap_or_else(|| config.tenant_id.clone());
        let token = registration::access_token_for_attempt(
            resolved.as_ref().map(|c| c.access_token.as_str()),
            &config.access_token,
        );

        let registered = {
            let state_for_io = Arc::clone(&state);
            let fallback_token = config.access_token.clone();
            registration::register_with_optional_refresh(
                &token,
                |tok| {
                    let wu = worker_url.clone();
                    let uid = user_id.clone();
                    let tid = tenant_id.clone();
                    let hn = hostname.clone();
                    let tok = tok.to_string();
                    async move {
                        registration::register_node(&wu, &uid, &tid, &hn, &tok).await
                    }
                },
                {
                    let st = Arc::clone(&state_for_io);
                    move || {
                        let st = Arc::clone(&st);
                        async move { crate::services::relay::refresh_access_token(&st).await }
                    }
                },
                {
                    let st = Arc::clone(&state_for_io);
                    move || {
                        let st = Arc::clone(&st);
                        let fallback = fallback_token.clone();
                        async move {
                            settings_service::RelayConfig::resolve(&st)
                                .map(|c| c.access_token)
                                .filter(|t| !t.trim().is_empty())
                                .unwrap_or(fallback)
                        }
                    }
                },
            )
            .await?
        };

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
                        // Seed the ack clock for the liveness watchdog. Without this the watchdog
                        // would compare against the PREVIOUS connection's last ack — already stale
                        // — and kill a healthy new socket before its first ack arrives.
                        status.last_heartbeat = Some(chrono::Utc::now());
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
        // Drives the half-open-socket watchdog in the arm below. Checked often relative to the
        // staleness threshold so a dead relay is caught promptly rather than on a 90s boundary.
        let mut liveness = tokio::time::interval(tokio::time::Duration::from_secs(
            HEARTBEAT_LIVENESS_CHECK_SECS,
        ));

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
                _ = liveness.tick() => {
                    // Watchdog for a HALF-OPEN relay socket.
                    //
                    // Every other arm here reacts to the socket telling us something. A half-open
                    // socket never does: the write half is dead but the read half yields neither
                    // an error nor a Close, so `ws_read.next()` parks forever and this loop never
                    // returns — which is the ONLY thing that triggers a reconnect. The heartbeat
                    // task notices (its send fails) but it only `return`s, killing itself quietly,
                    // so the node sits "connected" with a dead socket until the app restarts.
                    //
                    // Observed in the wild: heartbeat acks stopped at 14:15Z, the send failed at
                    // 15:08Z, and 90 minutes later `relayConnectionStatus` still reported
                    // connected=true while the kernel held no socket to Cloudflare at all. The
                    // phone just shows "no online backend".
                    //
                    // Acks are the liveness signal, so treat a stale one as a dead connection and
                    // return Err — the caller logs it and reconnects with backoff.
                    let last = state.connection_status.lock().await.last_heartbeat;
                    let now = chrono::Utc::now();
                    if heartbeat_is_stale(last, now) {
                        let age_secs = last
                            .map(|l| now.signed_duration_since(l).num_seconds())
                            .unwrap_or_default();
                        tracing::warn!(
                            age_secs,
                            "No heartbeat ack within threshold — treating relay as dead"
                        );
                        return Err(format!("heartbeat stale for {}s", age_secs));
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
                    } else if let Some(directive) =
                        crate::terminal_keys::parse_key_directive(&command.data)
                    {
                        // Keys, not text. Parsed HERE rather than in `send_terminal_input` on
                        // purpose: that function has ~20 internal callers (plan prompts, nudges,
                        // coordinator messages), and a generated prompt whose first line happened to
                        // start with the prefix would be silently turned into keystrokes. This relay
                        // branch is user input by definition — the same reason `with_shell_brief`
                        // only touches it.
                        Self::handle_key_directive(&st, &command.session_id, directive).await
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
            return Ok(Self::with_shell_brief(state, &command.session_id, command.data.clone()).await);
        }

        let saved = Self::save_command_attachments(state, command).await?;
        let mut data = command.data.trim_end_matches(['\r', '\n']).to_string();
        // ONE line. A newline inside a TUI composer either submits early — splitting the message
        // so the agent sees the text without its attachments — or is swallowed, which is what
        // produced the run-together "workspace:- ./path" a user reported. Same constraint the
        // reporting brief already follows.
        if !data.is_empty() {
            data.push(' ');
        }
        data.push_str("[Attached image files saved in this workspace: ");
        data.push_str(&saved.join(", "));
        data.push(']');
        data.push('\r');
        Ok(Self::with_shell_brief(state, &command.session_id, data).await)
    }

    /// Append the console's reporting brief to a user's FIRST message in a shell/attached session.
    ///
    /// An agent you attached by hand was never told that the phone console shows only what it
    /// reports, so it answers in its pane and the transcript stays empty. The brief has to reach
    /// it somehow — but it is plumbing, and the user should not have to look at it in their own
    /// message, so the host adds it here instead of the client.
    ///
    /// Only this relay path is touched, which is user input by definition: the coordinator calls
    /// `send_terminal_input` directly and its agents are already briefed by their plan prompt.
    /// One line, because a newline submits early in a TUI composer and would split the message.
    /// Send the keys a `//key` line asked for, or explain why it could not.
    ///
    /// Every outcome writes a transcript row. The console renders only persisted content, so a key
    /// send that logs nothing leaves you tapping at an unchanged mirror with no way to tell whether
    /// it landed. A malformed directive is reported and NOT retried as text — typing `Esx` into an
    /// agent's composer is worse than refusing it.
    async fn handle_key_directive(
        state: &Arc<AppState>,
        session_id: &str,
        directive: Result<crate::terminal_keys::KeyDirective, String>,
    ) -> Result<(), String> {
        use crate::terminal_keys::KeyDirective;

        match directive {
            Ok(KeyDirective::Help) => {
                crate::services::agent_plans::record_session_note(
                    state,
                    session_id,
                    "update",
                    "Key reference",
                    Some(&crate::terminal_keys::help_markdown()),
                );
                Ok(())
            }
            Ok(KeyDirective::Keys(keys)) => {
                tracing::info!(session_id, ?keys, "sending keys from transcript directive");
                let result = crate::terminal::send_terminal_keys(state, session_id.to_string(), &keys)
                    .await;
                match &result {
                    Ok(()) => crate::services::agent_plans::record_session_note(
                        state,
                        session_id,
                        "update",
                        &crate::terminal_keys::echo_line(&keys),
                        None,
                    ),
                    Err(error) => crate::services::agent_plans::record_session_note(
                        state,
                        session_id,
                        "blocked",
                        &format!("Could not send keys: {error}"),
                        None,
                    ),
                }
                result
            }
            Err(message) => {
                crate::services::agent_plans::record_session_note(
                    state,
                    session_id,
                    "blocked",
                    &message,
                    None,
                );
                // Already reported to the user, so the ack carries no error — this is a usage
                // mistake, not a transport failure.
                Ok(())
            }
        }
    }

    async fn with_shell_brief(state: &Arc<AppState>, session_id: &str, data: String) -> String {
        if data.trim().is_empty() {
            return data;
        }
        // Control keys and navigation are not messages — never brief on those.
        let body = data.trim_end_matches(['\r', '\n']);
        if body.trim().is_empty() || body.chars().any(|c| c.is_control()) {
            return data;
        }
        if !Self::session_wants_brief(state, session_id) {
            return data;
        }
        // A slash command is addressed to the CLI, not to the agent. Appending the brief made the
        // brief its arguments (`/compact [JohnnyOne: …]`), so these pass through untouched: no
        // brief, and no report nudge, because the CLI answers in the pane and there is no reply to
        // report. Still recorded, so the transcript shows what was sent.
        if crate::terminal_keys::is_cli_slash_command(body) {
            crate::services::agent_plans::record_user_prompt(state, session_id, body);
            return data;
        }
        // Every user message arms the "answered but never reported" guard, not just the first.
        Self::arm_shell_report_nudge(state, session_id);
        // Store the prompt so the transcript can pair it with the reply. Uses the body BEFORE the
        // brief is appended — the plumbing is ours, not something the user said.
        crate::services::agent_plans::record_user_prompt(state, session_id, body);
        {
            let mut briefed = state.shell_briefed.lock().await;
            if !briefed.insert(session_id.to_string()) {
                return data;
            }
        }
        let brief = format!(
            " [JohnnyOne: this conversation is being read on a phone console that shows ONLY what \
             you report — your terminal output is not visible there. After you answer, write your \
             reply as markdown (```mermaid fences render) to a file, then run: curl -s \
             127.0.0.1:7788/graphql -H 'content-type: application/json' -d \
             '{{\"query\":\"mutation{{reportAgentResult(sessionId:\\\"{session_id}\\\",\
             kind:\\\"update\\\",summary:\\\"ONE LINE NO QUOTES\\\",\
             evidence:\\\"/absolute/path/to/reply.md\\\")}}\"}}' — replace the summary and path, \
             and do this for every reply.]"
        );
        let tail: String = data[body.len()..].to_string();
        format!("{body}{brief}{tail}")
    }

    /// Nudge a shell agent that answered but never reported.
    ///
    /// The transcript only shows what an agent reports, so an agent that replies in its pane and
    /// stops leaves the console blank with no way to tell that from still-thinking. The
    /// coordinator already solves this for plan agents by nudging when they go idle without a
    /// signal; this is the same guard for the shells a human talks to.
    ///
    /// Fires ONLY when both are true: nothing was reported since the message was sent, and the
    /// pane has actually gone quiet. A long think is not a failure to report, so an agent still
    /// working is never interrupted. One nudge per message — if it is ignored, a second will not
    /// help and would just be noise in the agent's context.
    fn arm_shell_report_nudge(state: &Arc<AppState>, session_id: &str) {
        let state = state.clone();
        let session_id = session_id.to_string();
        tokio::spawn(async move {
            let armed_at = Instant::now();
            tracing::debug!(session_id = %session_id, "shell report nudge armed");
            tokio::time::sleep(Duration::from_secs(SHELL_NUDGE_AFTER_SECS)).await;

            // Reported since we armed? Nothing to do.
            if let Some(at) = state.shell_last_report_at.lock().await.get(&session_id) {
                if *at >= armed_at {
                    tracing::debug!(session_id = %session_id, "nudge skipped: agent reported");
                    return;
                }
            }

            // Still working? Leave it alone. Two captures a few seconds apart settle it without
            // needing to know any CLI's wording for "busy".
            let first = match crate::terminal::capture_terminal_session(&state, &session_id).await
            {
                Ok(snapshot) => snapshot,
                Err(error) => {
                    tracing::warn!(session_id = %session_id, %error, "nudge skipped: capture failed");
                    return;
                }
            };
            tokio::time::sleep(Duration::from_secs(SHELL_NUDGE_IDLE_PROBE_SECS)).await;
            let second = match crate::terminal::capture_terminal_session(&state, &session_id).await
            {
                Ok(snapshot) => snapshot,
                Err(error) => {
                    tracing::warn!(session_id = %session_id, %error, "nudge skipped: capture failed");
                    return;
                }
            };
            let a = crate::services::agent_plans::normalize_terminal_snapshot_for_idle(&first.content);
            let b = crate::services::agent_plans::normalize_terminal_snapshot_for_idle(&second.content);
            if a != b {
                tracing::debug!(session_id = %session_id, "nudge skipped: pane still changing");
                return;
            }

            // Re-check: a report may have landed during the probe.
            if let Some(at) = state.shell_last_report_at.lock().await.get(&session_id) {
                if *at >= armed_at {
                    return;
                }
            }

            let nudge = format!(
                " [JohnnyOne: you answered but did not report it, so the phone console is still \
                 showing nothing. Write your reply as markdown to a file and run: curl -s \
                 127.0.0.1:7788/graphql -H 'content-type: application/json' -d \
                 '{{\"query\":\"mutation{{reportAgentResult(sessionId:\\\"{session_id}\\\",\
                 kind:\\\"update\\\",summary:\\\"ONE LINE NO QUOTES\\\",\
                 evidence:\\\"/absolute/path/to/reply.md\\\")}}\"}}']"
            );
            tracing::info!(session_id = %session_id, "nudging shell agent to report its reply");
            let _ = crate::terminal::send_terminal_input(
                &state,
                session_id.clone(),
                format!("{nudge}\r"),
            )
            .await;
        });
    }

    /// Which sessions get the reporting brief and the silent-agent nudge.
    ///
    /// Only sessions that could plausibly REPORT. An attached tmux pane is one the user chose to
    /// talk to, and a spawned shell carrying setup commands launched something into it. A bare
    /// `shell` with no setup commands is just bash: it cannot report, and both the brief and the
    /// nudge land as a command it tries to execute — observed as
    /// `bash: syntax error near unexpected token '('`.
    ///
    /// Plan agents are excluded implicitly: they are `claude_code`/`grok`/… and already carry the
    /// instruction in their prompt, so briefing them again would be noise in their context.
    fn session_wants_brief(state: &Arc<AppState>, session_id: &str) -> bool {
        use rusqlite::OptionalExtension;
        state
            .db
            .with_conn(|conn| {
                conn.query_row(
                    "SELECT provider, attached_tmux, setup_commands FROM sessions WHERE id = ?1",
                    rusqlite::params![session_id],
                    |row| {
                        let provider: String = row.get(0)?;
                        let attached: i64 = row.get(1)?;
                        let setup: Option<String> = row.get(2)?;
                        let launched_something =
                            setup.map(|s| !s.trim().is_empty()).unwrap_or(false);
                        Ok(attached != 0 || (provider == "shell" && launched_something))
                    },
                )
                .optional()
                .map(|found| found.unwrap_or(false))
                .map_err(|e| e.to_string())
            })
            .unwrap_or(false)
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

        let snapshot = state
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
        let mut saved_paths = Vec::new();

        for attachment in &command.attachments {
            // Resolve at call time — same credential the WS loop uses.
            // Do not send the spawn-time WorkerRelayConfig snapshot token.
            let resolved = settings_service::RelayConfig::resolve(state);
            let tenant_id = resolved
                .as_ref()
                .map(|c| c.tenant_id.as_str())
                .unwrap_or(snapshot.tenant_id.as_str());
            let user_id = resolved
                .as_ref()
                .map(|c| c.user_id.as_str())
                .unwrap_or(snapshot.user_id.as_str());
            let worker_url = resolved
                .as_ref()
                .map(|c| c.worker_url.as_str())
                .unwrap_or(snapshot.worker_url.as_str());
            let graphql_url = format!("{}/graphql", worker_url.trim_end_matches('/'));
            let auth_headers = registration::attachment_graphql_headers(
                tenant_id,
                user_id,
                resolved.as_ref().map(|c| c.access_token.as_str()),
                snapshot.access_token.as_str(),
            );
            let response = registration::apply_headers(
                client
                    .post(&graphql_url)
                    .header("Content-Type", "application/json"),
                &auth_headers,
            )
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

            let _ = registration::apply_headers(
                client
                    .post(&graphql_url)
                    .header("Content-Type", "application/json"),
                &auth_headers,
            )
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
            "list_session_reports" => Self::rpc_list_session_reports(&req.params, state),
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
            // --- the test gate (migration 023 plan-level, 025 phase-level) ---
            "update_agent_plan_test_commands" => {
                Self::rpc_update_agent_plan_test_commands(&req.params, state)
            }
            "run_agent_plan_test_gate" => {
                Self::rpc_run_agent_plan_test_gate(&req.params, state).await
            }
            "refresh_agent_plan_phases" => {
                Self::rpc_refresh_agent_plan_phases(&req.params, state)
            },
            // --- the workflow arena (migration 026) ---
            // Reads first, then the writes. Every one of these is a thin param-puller: the rules
            // (comparability, the promotion refusals, the three-strike halt) live in
            // `services::workflows`, never here, so the relay and a future local caller cannot
            // disagree about them.
            "list_workflows" => Self::rpc_list_workflows(&req.params, state),
            "get_workflow" => Self::rpc_get_workflow(&req.params, state),
            "list_workflow_runs" => Self::rpc_list_workflow_runs(&req.params, state),
            "list_workflow_prefs" => Self::rpc_list_workflow_prefs(&req.params, state),
            "get_repo_identity" => Self::rpc_get_repo_identity(&req.params, state),
            "get_workflow_search_state" => {
                Self::rpc_get_workflow_search_state(&req.params, state)
            }
            "create_workflow_version" => Self::rpc_create_workflow_version(&req.params, state),
            "start_workflow_run" => Self::rpc_start_workflow_run(&req.params, state),
            "execute_workflow_run" => Self::rpc_execute_workflow_run(&req.params, state),
            "record_workflow_run_gate" => Self::rpc_record_workflow_run_gate(&req.params, state),
            "run_workflow_run_gate" => {
                Self::rpc_run_workflow_run_gate(&req.params, state).await
            }
            "rescore_workflow_run_gate" => {
                Self::rpc_rescore_workflow_run_gate(&req.params, state).await
            }
            "record_workflow_run_rescore" => {
                Self::rpc_record_workflow_run_rescore(&req.params, state)
            }
            "record_workflow_pref" => Self::rpc_record_workflow_pref(&req.params, state),
            "update_workflow_champion" => Self::rpc_update_workflow_champion(&req.params, state),
            "record_workflow_rejection" => {
                Self::rpc_record_workflow_rejection(&req.params, state)
            }
            "update_workflow_search_resumed" => {
                Self::rpc_update_workflow_search_resumed(&req.params, state)
            }
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
            "list_prompt_library" => Self::rpc_list_prompt_library(state),
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
    /// A session's persisted reports, so a freshly-opened console can show its history.
    fn rpc_list_session_reports(
        params: &serde_json::Value,
        state: &Arc<AppState>,
    ) -> Result<serde_json::Value, String> {
        let session_id = params
            .get("sessionId")
            .and_then(|v| v.as_str())
            .ok_or_else(|| "Missing 'sessionId' parameter".to_string())?;
        let limit = params.get("limit").and_then(|v| v.as_i64()).unwrap_or(100);
        let reports =
            crate::services::agent_plans::list_session_reports(state, session_id, limit)?;
        serde_json::to_value(&reports).map_err(|e| e.to_string())
    }

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

    // --- the test gate ---
    // Thin param-pullers mirroring rpc_update_agent_plan_validation_config: read typed params,
    // call the service, serialize. The phase-over-plan resolution rule is NOT here — it lives in
    // `services::agent_plans::effective_test_commands` and nowhere else.

    fn rpc_update_agent_plan_test_commands(
        params: &serde_json::Value,
        state: &Arc<AppState>,
    ) -> Result<serde_json::Value, String> {
        let id = params
            .get("id")
            .and_then(|value| value.as_str())
            .ok_or_else(|| "Missing 'id' parameter".to_string())?;
        // Optional: absent/null targets the PLAN's gate, present targets that phase's.
        let phase_id = params
            .get("phaseId")
            .and_then(|value| value.as_str())
            .map(|value| value.to_string())
            .filter(|value| !value.trim().is_empty());
        let commands: Vec<String> = params
            .get("commands")
            .and_then(|value| value.as_array())
            .map(|values| {
                values
                    .iter()
                    .filter_map(|value| value.as_str())
                    .map(|value| value.to_string())
                    .collect()
            })
            .unwrap_or_default();
        // Clearing is explicit. An empty `commands` without `clear: true` is REJECTED by the
        // service, so a call that forgot its arguments cannot wipe a gate — the defaults here are
        // deliberately the ones that fail loudly rather than the ones that destroy state.
        let clear = params
            .get("clear")
            .and_then(|value| value.as_bool())
            .unwrap_or(false);
        let run = crate::services::agent_plans::update_test_commands(
            state,
            id.to_string(),
            phase_id,
            commands,
            clear,
        )?;
        serde_json::to_value(run).map_err(|e| e.to_string())
    }

    // --- the workflow arena (migration 026) ---
    //
    // Thin param-pullers mirroring `rpc_update_agent_plan_test_commands`: read typed params from
    // the JSON value, call `services::workflows`, serialize the result. camelCase keys match the
    // worker resolvers and the GraphQL arguments.
    //
    // NOTHING here decides anything. The comparability rule, the promotion refusals and the
    // three-strike halt are all in the service, behind one named function each, because this layer
    // will eventually have a second caller (a local Tauri command) and two copies of a rule is how
    // this repo has already produced a false pass.

    fn workflow_key(
        params: &serde_json::Value,
    ) -> Result<(String, String, String), String> {
        let repo = params
            .get("repo")
            .and_then(|value| value.as_str())
            .ok_or_else(|| "Missing 'repo' parameter".to_string())?;
        let phase_type = params
            .get("phaseType")
            .and_then(|value| value.as_str())
            .ok_or_else(|| "Missing 'phaseType' parameter".to_string())?;
        let family = params
            .get("family")
            .and_then(|value| value.as_str())
            .ok_or_else(|| "Missing 'family' parameter".to_string())?;
        Ok((repo.to_string(), phase_type.to_string(), family.to_string()))
    }

    fn rpc_list_workflows(
        params: &serde_json::Value,
        state: &Arc<AppState>,
    ) -> Result<serde_json::Value, String> {
        // All three filters are optional and independent — absent means "any".
        let optional = |key: &str| {
            params
                .get(key)
                .and_then(|value| value.as_str())
                .map(|value| value.to_string())
                .filter(|value| !value.trim().is_empty())
        };
        let repo = optional("repo");
        let phase_type = optional("phaseType");
        let family = optional("family");
        let workflows = crate::services::workflows::list_workflows(
            state,
            repo.as_deref(),
            phase_type.as_deref(),
            family.as_deref(),
        )?;
        serde_json::to_value(workflows).map_err(|e| e.to_string())
    }

    fn rpc_get_workflow(
        params: &serde_json::Value,
        state: &Arc<AppState>,
    ) -> Result<serde_json::Value, String> {
        let id = params
            .get("id")
            .and_then(|value| value.as_str())
            .ok_or_else(|| "Missing 'id' parameter".to_string())?;
        let workflow = crate::services::workflows::get_workflow(state, id)?;
        serde_json::to_value(workflow).map_err(|e| e.to_string())
    }

    fn rpc_list_workflow_runs(
        params: &serde_json::Value,
        state: &Arc<AppState>,
    ) -> Result<serde_json::Value, String> {
        let workflow_id = params
            .get("workflowId")
            .and_then(|value| value.as_str())
            .ok_or_else(|| "Missing 'workflowId' parameter".to_string())?;
        let runs = crate::services::workflows::list_workflow_runs(state, workflow_id)?;
        serde_json::to_value(runs).map_err(|e| e.to_string())
    }

    fn rpc_list_workflow_prefs(
        params: &serde_json::Value,
        state: &Arc<AppState>,
    ) -> Result<serde_json::Value, String> {
        let workflow_id = params
            .get("workflowId")
            .and_then(|value| value.as_str())
            .ok_or_else(|| "Missing 'workflowId' parameter".to_string())?;
        let prefs = crate::services::workflows::list_workflow_prefs(state, workflow_id)?;
        serde_json::to_value(prefs).map_err(|e| e.to_string())
    }

    /// The repo identity for a workspace path — the one way a client can learn the value the host
    /// derives, so it can key `list_workflows` / `get_workflow_search_state` on it without
    /// re-implementing the derivation.
    fn rpc_get_repo_identity(
        params: &serde_json::Value,
        state: &Arc<AppState>,
    ) -> Result<serde_json::Value, String> {
        let _ = state;
        let workspace_path = params
            .get("workspacePath")
            .and_then(|value| value.as_str())
            .ok_or_else(|| "Missing 'workspacePath' parameter".to_string())?;
        let repo = crate::services::workflows::repo_identity(workspace_path)?;
        // A bare JSON string: the GraphQL field is `String!`, so there is no wrapper object for a
        // client to unpack and no second place for the key's name to drift.
        serde_json::to_value(repo).map_err(|e| e.to_string())
    }

    fn rpc_get_workflow_search_state(
        params: &serde_json::Value,
        state: &Arc<AppState>,
    ) -> Result<serde_json::Value, String> {
        let (repo, phase_type, family) = Self::workflow_key(params)?;
        let search = crate::services::workflows::workflow_search_state(
            state,
            &repo,
            &phase_type,
            &family,
        )?;
        serde_json::to_value(search).map_err(|e| e.to_string())
    }

    fn rpc_create_workflow_version(
        params: &serde_json::Value,
        state: &Arc<AppState>,
    ) -> Result<serde_json::Value, String> {
        let required = |key: &str| -> Result<String, String> {
            params
                .get(key)
                .and_then(|value| value.as_str())
                .map(|value| value.to_string())
                .ok_or_else(|| format!("Missing '{}' parameter", key))
        };
        // NOTE `repo` is NOT a parameter. The client used to send it as
        // `basename(workspace_path)`, so `/a/johnnyone` and `/b/johnnyone` shared one champion.
        // The host derives the identity from the workspace path instead
        // (`services::workflows::repo_identity`: the git remote, normalised, else the absolute
        // path), because a client-supplied identity is a client-supplied champion key.
        let input = crate::services::workflows::NewWorkflowVersion {
            workspace_path: required("workspacePath")?,
            phase_type: required("phaseType")?,
            family: required("family")?,
            parent_id: params
                .get("parentId")
                .and_then(|value| value.as_str())
                .map(|value| value.to_string())
                .filter(|value| !value.trim().is_empty()),
            role: required("role")?,
            instruction: required("instruction")?,
            contract: required("contract")?,
            hop: required("hop")?,
            // No default. A fan-out the caller did not state is not "1 seat" — it is a bug, and a
            // silent default here would let a workflow compete on a seat budget nobody chose.
            fan_out: params
                .get("fanOut")
                .and_then(|value| value.as_i64())
                .ok_or_else(|| "Missing 'fanOut' parameter".to_string())?,
        };
        let workflow = crate::services::workflows::create_workflow_version(state, input)?;
        serde_json::to_value(workflow).map_err(|e| e.to_string())
    }

    fn rpc_start_workflow_run(
        params: &serde_json::Value,
        state: &Arc<AppState>,
    ) -> Result<serde_json::Value, String> {
        let workflow_id = params
            .get("workflowId")
            .and_then(|value| value.as_str())
            .ok_or_else(|| "Missing 'workflowId' parameter".to_string())?;
        // NOTE what is NOT read here: `seatsHeld` and `fleetBusy`. They used to be relayed
        // parameters, and the caller is a browser that can measure neither — so `comparable`
        // degraded to "did two clients type the same numbers", which is the exact failure those two
        // columns exist to prevent. The host measures both now
        // (`services::workflows::measure_run_budget`), and the model family comes from the workflow
        // row rather than from the caller.
        let input = crate::services::workflows::NewWorkflowRun {
            workflow_id: workflow_id.to_string(),
            plan_id: params
                .get("planId")
                .and_then(|value| value.as_str())
                .map(|value| value.to_string())
                .filter(|value| !value.trim().is_empty()),
            seed: params.get("seed").and_then(|value| value.as_i64()).unwrap_or(0),
            // Optional override of the workflow's own `fan_out`. Absent means "whatever the
            // workflow declares", which is the normal case.
            seats: params.get("seats").and_then(|value| value.as_i64()),
        };
        let run = crate::services::workflows::start_workflow_run(state, input)?;
        serde_json::to_value(run).map_err(|e| e.to_string())
    }

    /// RUN an arena phase for real, and score what the child exited with.
    ///
    /// The missing link on the wire: `compose_workflow_prompt`, the `PhaseExecutor` seam,
    /// `execute_workflow_run` and `execute_and_score_workflow_run` all existed, and nothing on the
    /// RPC surface called any of them — so a run row was created and never executed, which means the
    /// arena could not score anything through the real wire. This is that door.
    ///
    /// `runId` is required; `verify` (default `"true"`) and `binary` (default `"kloo"`) are the two
    /// things about the CHILD the caller may name. Everything else is resolved from the row: the
    /// workdir is the workspace this agent already uses for the run — the run's plan's
    /// `workspace_path`, the same source `score_workflow_run` runs a gate in — and the FAMILY is not
    /// a parameter at all, because `execute_and_score_workflow_run` reads it from the workflow row.
    ///
    /// **An `Err` from the executor propagates as an RPC error and records no gate.** An execution
    /// that did not happen must leave the run unscored (`gate_exit` NULL), so that it is never
    /// mistaken for an execution that ran and failed.
    fn rpc_execute_workflow_run(
        params: &serde_json::Value,
        state: &Arc<AppState>,
    ) -> Result<serde_json::Value, String> {
        let run_id = params
            .get("runId")
            .and_then(|value| value.as_str())
            .ok_or_else(|| "Missing 'runId' parameter".to_string())?;
        let verify = params
            .get("verify")
            .and_then(|value| value.as_str())
            .filter(|value| !value.trim().is_empty())
            .unwrap_or("true")
            .to_string();
        let binary = params
            .get("binary")
            .and_then(|value| value.as_str())
            .filter(|value| !value.trim().is_empty())
            .unwrap_or("kloo")
            .to_string();
        // The workspace this agent already uses for the run: the run's plan's `workspace_path`,
        // resolved through the plan row rather than invented here. A run with no plan has no
        // workspace to execute in, and is refused rather than run in some guessed directory.
        let workdir = state.db.with_conn(|conn| {
            conn.query_row(
                "SELECT p.workspace_path FROM workflow_runs r \
                 JOIN agent_plans p ON p.id = r.plan_id \
                 WHERE r.id = ?1",
                rusqlite::params![run_id],
                |row| row.get::<_, String>(0),
            )
            .map_err(|e| match e {
                rusqlite::Error::QueryReturnedNoRows => {
                    format!("workflow run {} not found", run_id)
                }
                other => other.to_string(),
            })
        })?;
        let exec = crate::services::kloo_executor::KlooExecutor {
            binary,
            workdir: std::path::PathBuf::from(workdir),
            verify,
        };
        crate::services::workflows::execute_and_score_workflow_run(state, run_id, &exec)?;
        // Answer the RUN ROW, not a bare number: the worker's schema declares this mutation to
        // return `WorkflowRun!`, like its neighbours `startWorkflowRun` and `runWorkflowRunGate`,
        // whose arms end the same way. The serialised run already carries `gateExit`, so nothing
        // is invented here and the score is not copied a second time into the payload.
        let run = crate::services::workflows::get_workflow_run(state, run_id)?;
        serde_json::to_value(run).map_err(|e| e.to_string())
    }

    fn rpc_record_workflow_run_gate(
        params: &serde_json::Value,
        state: &Arc<AppState>,
    ) -> Result<serde_json::Value, String> {
        let run_id = params
            .get("runId")
            .and_then(|value| value.as_str())
            .ok_or_else(|| "Missing 'runId' parameter".to_string())?;
        // `gateExit` is REQUIRED: it is the score, and a missing score must be an error rather
        // than a 0 that reads as a pass.
        let gate_exit = params
            .get("gateExit")
            .and_then(|value| value.as_i64())
            .ok_or_else(|| "Missing 'gateExit' parameter".to_string())?;
        // `gateCollected` is ADVISORY and genuinely optional: absent and explicit null both mean
        // "the gate printed nothing a parser recognised", which is a normal result for a lint, a
        // build or a bespoke script. It is never an error and nothing branches on it.
        let gate_collected = params.get("gateCollected").and_then(|value| value.as_i64());
        let run = crate::services::workflows::record_workflow_run_gate(
            state,
            run_id,
            gate_exit,
            gate_collected,
        )?;
        serde_json::to_value(run).map_err(|e| e.to_string())
    }

    /// Replace a run's gate result with one measured elsewhere, DELIBERATELY.
    ///
    /// There is no `rescore` flag on `record_workflow_run_gate` any more. The service's own
    /// rationale was "the separate name IS the guard: there is no flag to forget and no default to
    /// get wrong" — and the wire API still took a boolean, so on the only surface a client can
    /// reach, the guard WAS the flag that rationale rejected. Now it is the method.
    fn rpc_record_workflow_run_rescore(
        params: &serde_json::Value,
        state: &Arc<AppState>,
    ) -> Result<serde_json::Value, String> {
        let run_id = params
            .get("runId")
            .and_then(|value| value.as_str())
            .ok_or_else(|| "Missing 'runId' parameter".to_string())?;
        let gate_exit = params
            .get("gateExit")
            .and_then(|value| value.as_i64())
            .ok_or_else(|| "Missing 'gateExit' parameter".to_string())?;
        let gate_collected = params.get("gateCollected").and_then(|value| value.as_i64());
        let run = crate::services::workflows::rescore_workflow_run_gate(
            state,
            run_id,
            gate_exit,
            gate_collected,
        )?;
        serde_json::to_value(run).map_err(|e| e.to_string())
    }

    /// EXECUTE an arena run's gate and record the result — the step that closes the arena's loop.
    ///
    /// `record_workflow_run_gate` takes a score someone else measured; this one measures it, by
    /// running the plan's effective gate in the plan's workspace. Same `rescore` discipline: a
    /// second result for a run is refused unless asked for.
    /// `runId` plus the optional `phaseId` level selector, shared by the first-score and the
    /// deliberate-rescore methods. There is no `rescore` parameter on either: the METHOD NAME is
    /// the guard, which is what the recorder's own rationale asked for and what the wire API did
    /// not yet honour.
    fn gate_run_params(
        params: &serde_json::Value,
    ) -> Result<(String, Option<String>), String> {
        let run_id = params
            .get("runId")
            .and_then(|value| value.as_str())
            .ok_or_else(|| "Missing 'runId' parameter".to_string())?;
        // Absent targets the PLAN's gate, present targets that phase's — the same level-selection
        // rule as `run_agent_plan_test_gate`.
        let phase_id = params
            .get("phaseId")
            .and_then(|value| value.as_str())
            .map(|value| value.to_string())
            .filter(|value| !value.trim().is_empty());
        Ok((run_id.to_string(), phase_id))
    }

    async fn rpc_run_workflow_run_gate(
        params: &serde_json::Value,
        state: &Arc<AppState>,
    ) -> Result<serde_json::Value, String> {
        let (run_id, phase_id) = Self::gate_run_params(params)?;
        let run =
            crate::services::workflows::score_workflow_run(state, &run_id, phase_id).await?;
        serde_json::to_value(run).map_err(|e| e.to_string())
    }

    /// Execute the gate again for a run that already has a score, and REPLACE it.
    ///
    /// Its own method rather than a flag on the one above. A sitting champion's promoting run is
    /// refused here too — that is in the service, not in the choice of door.
    async fn rpc_rescore_workflow_run_gate(
        params: &serde_json::Value,
        state: &Arc<AppState>,
    ) -> Result<serde_json::Value, String> {
        let (run_id, phase_id) = Self::gate_run_params(params)?;
        let run =
            crate::services::workflows::rescore_workflow_run(state, &run_id, phase_id).await?;
        serde_json::to_value(run).map_err(|e| e.to_string())
    }

    fn rpc_record_workflow_pref(
        params: &serde_json::Value,
        state: &Arc<AppState>,
    ) -> Result<serde_json::Value, String> {
        let winner_run_id = params
            .get("winnerRunId")
            .and_then(|value| value.as_str())
            .ok_or_else(|| "Missing 'winnerRunId' parameter".to_string())?;
        let loser_run_id = params
            .get("loserRunId")
            .and_then(|value| value.as_str())
            .ok_or_else(|| "Missing 'loserRunId' parameter".to_string())?;
        // NOTE there is deliberately no `comparable` parameter. The service computes it from the
        // two runs' own seat budgets, so a client cannot assert a comparability it does not have.
        let reason = params
            .get("reason")
            .and_then(|value| value.as_str())
            .map(|value| value.to_string());
        let pref = crate::services::workflows::record_workflow_pref(
            state,
            winner_run_id,
            loser_run_id,
            reason,
        )?;
        serde_json::to_value(pref).map_err(|e| e.to_string())
    }

    fn rpc_update_workflow_champion(
        params: &serde_json::Value,
        state: &Arc<AppState>,
    ) -> Result<serde_json::Value, String> {
        let run_id = params
            .get("runId")
            .and_then(|value| value.as_str())
            .ok_or_else(|| "Missing 'runId' parameter".to_string())?;
        let pref_id = params
            .get("prefId")
            .and_then(|value| value.as_str())
            .map(|value| value.to_string())
            .filter(|value| !value.trim().is_empty());
        // Defaults to FALSE, so a call that forgot its arguments cannot promote. Promotion is the
        // arena's one irreversible, human-confirmed step.
        let confirmed = params
            .get("confirmed")
            .and_then(|value| value.as_bool())
            .unwrap_or(false);
        let workflow = crate::services::workflows::promote_workflow(
            state,
            run_id,
            pref_id.as_deref(),
            confirmed,
        )?;
        serde_json::to_value(workflow).map_err(|e| e.to_string())
    }

    fn rpc_update_workflow_search_resumed(
        params: &serde_json::Value,
        state: &Arc<AppState>,
    ) -> Result<serde_json::Value, String> {
        let (repo, phase_type, family) = Self::workflow_key(params)?;
        // Both required. Lifting a halt is the decision a reader will most want explained, and the
        // `confirmed` default is false so a call that forgot its arguments cannot lift one.
        let reason = params
            .get("reason")
            .and_then(|value| value.as_str())
            .ok_or_else(|| "Missing 'reason' parameter".to_string())?;
        let confirmed = params
            .get("confirmed")
            .and_then(|value| value.as_bool())
            .unwrap_or(false);
        let search = crate::services::workflows::resume_workflow_search(
            state,
            &repo,
            &phase_type,
            &family,
            reason,
            confirmed,
        )?;
        serde_json::to_value(search).map_err(|e| e.to_string())
    }

    fn rpc_record_workflow_rejection(
        params: &serde_json::Value,
        state: &Arc<AppState>,
    ) -> Result<serde_json::Value, String> {
        let run_id = params
            .get("runId")
            .and_then(|value| value.as_str())
            .ok_or_else(|| "Missing 'runId' parameter".to_string())?;
        // Required. The service rejects a blank one too — three rejections halt the search and the
        // reasons are the only thing a halted search has to show.
        let reason = params
            .get("reason")
            .and_then(|value| value.as_str())
            .ok_or_else(|| "Missing 'reason' parameter".to_string())?;
        let search = crate::services::workflows::reject_workflow_run(state, run_id, reason)?;
        serde_json::to_value(search).map_err(|e| e.to_string())
    }

    async fn rpc_run_agent_plan_test_gate(
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
            .map(|value| value.to_string())
            .filter(|value| !value.trim().is_empty());
        let result = crate::services::agent_plans::run_plan_test_gate(
            state,
            id.to_string(),
            phase_id,
        )
        .await?;
        serde_json::to_value(result).map_err(|e| e.to_string())
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

    fn rpc_list_prompt_library(state: &Arc<AppState>) -> Result<serde_json::Value, String> {
        let entries = crate::services::prompt_library::list_prompt_library(state)?;
        serde_json::to_value(entries).map_err(|e| e.to_string())
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
            terminal_status: session.terminal_status,
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

    /// Seam (a), QA-added: the two test-gate RPC methods are actually REGISTERED in `handle_rpc`'s
    /// match arms and their param-pullers read the keys the worker resolvers send. Nothing else in
    /// the tree drives a JSON-RPC request through them, so a renamed arm or a renamed param key
    /// would compile, pass every other gate, and fail only in production.
    ///
    /// No plan is created: routing is proven by the error coming from the SERVICE
    /// ("Agent plan not found") rather than the dispatch ("Unknown RPC method").
    #[tokio::test]
    async fn handle_rpc_routes_the_test_gate_methods() {
        let (state, _root) = test_state();
        let state = Arc::new(state);

        for method in ["update_agent_plan_test_commands", "run_agent_plan_test_gate"] {
            // The param-puller rejects a missing `id` before it reaches the service.
            let missing = AgentService::handle_rpc(&rpc(method, json!({})), &state).await;
            assert!(!missing.success, "{method} with no id must fail");
            let error = missing.error.unwrap_or_default();
            assert!(
                error.contains("Missing 'id' parameter"),
                "{method}: expected the param-puller's error, got: {error}"
            );

            // With the exact payload the worker resolver sends, the arm must route into the
            // service. `phaseId: null` is the plan level; `commands` is a list, not a JSON string.
            let routed = AgentService::handle_rpc(
                &rpc(
                    method,
                    json!({ "id": "no-such-plan", "phaseId": null, "commands": ["npm test"] }),
                ),
                &state,
            )
            .await;
            assert!(!routed.success, "{method} on a missing plan must fail");
            let error = routed.error.unwrap_or_default();
            assert!(
                !error.contains("Unknown RPC method"),
                "{method} is NOT registered in handle_rpc"
            );
            assert!(
                error.contains("Agent plan not found"),
                "{method}: expected the service's error, got: {error}"
            );
        }
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

#[cfg(test)]
mod liveness_watchdog_tests {
    use super::*;

    /// A fixed reference instant, so assertions do not race two clock reads. Deriving the past
    /// timestamps from a second `Utc::now()` leaves the age a hair under the whole second intended,
    /// and `num_seconds()` truncates — which silently flipped the boundary case.
    fn reference_now() -> chrono::DateTime<chrono::Utc> {
        chrono::DateTime::from_timestamp(1_760_000_000, 0)
            .expect("valid fixed timestamp")
            .to_utc()
    }

    /// A timestamp `secs_ago` before `reference_now()`.
    fn ago(secs: i64) -> chrono::DateTime<chrono::Utc> {
        reference_now() - chrono::Duration::seconds(secs)
    }

    /// The field failure this watchdog exists for: acks stopped at 14:15Z, yet the node still
    /// reported connected=true 90 minutes later while holding no socket to Cloudflare.
    #[test]
    fn ninety_minute_ack_gap_is_stale() {
        assert!(heartbeat_is_stale(Some(ago(90 * 60)), reference_now()));
    }

    /// A couple of missed heartbeats must NOT drop the connection — slow networks and Durable
    /// Object hiccups are routine, and reconnecting on every blip would be worse than the bug.
    #[test]
    fn one_or_two_missed_heartbeats_tolerated() {
        assert!(!heartbeat_is_stale(Some(ago(31)), reference_now()), "one missed beat");
        assert!(!heartbeat_is_stale(Some(ago(62)), reference_now()), "two missed beats");
    }

    /// Three missed beats is the line; at or past it the relay counts as dead.
    #[test]
    fn threshold_boundary_is_exact() {
        assert!(!heartbeat_is_stale(
            Some(ago(HEARTBEAT_STALE_AFTER_SECS - 1)),
            reference_now()
        ));
        assert!(heartbeat_is_stale(
            Some(ago(HEARTBEAT_STALE_AFTER_SECS)),
            reference_now()
        ));
    }

    /// A just-acked connection is healthy.
    #[test]
    fn fresh_ack_is_healthy() {
        assert!(!heartbeat_is_stale(Some(reference_now()), reference_now()));
    }

    /// No ack recorded yet must never read as stale, or the watchdog would kill a connection
    /// before its first ack could land. The connect path seeds a timestamp for this reason.
    #[test]
    fn absent_ack_is_not_stale() {
        assert!(!heartbeat_is_stale(None, reference_now()));
    }

    /// Clock skew (an ack stamped slightly in the future) must not read as stale.
    #[test]
    fn future_ack_is_not_stale() {
        assert!(!heartbeat_is_stale(
            Some(reference_now() + chrono::Duration::seconds(5)),
            reference_now()
        ));
    }

    /// The check must run several times inside the staleness window, so a dead relay is caught
    /// promptly rather than up to a full threshold late.
    #[test]
    fn check_interval_is_well_inside_the_threshold() {
        assert!(
            (HEARTBEAT_LIVENESS_CHECK_SECS as i64) * 3 <= HEARTBEAT_STALE_AFTER_SECS,
            "liveness check should run at least 3x per staleness window"
        );
    }
}

/// The workflow arena's relay-RPC wiring (migration 026).
///
/// A separate module from `files_rpc_tests` for the same reason that one exists: nothing else in
/// the tree drives a JSON-RPC request through these arms, so a renamed match arm or a renamed
/// param key would compile, pass every other gate, and fail only in production — against a phone.
///
/// These tests are about WIRING, not rules. The rules (comparability, the promotion refusals, the
/// three-strike halt) are pinned where they live, in `services::workflows`.
#[cfg(test)]
mod workflow_arena_rpc_tests {
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

    /// Round 3: `repo` is no longer a parameter — the host derives the identity from a workspace
    /// path. These tests use a stable synthetic path (deliberately non-existent, so the derivation
    /// finds no git remote and falls back to the absolute path) and ask the HOST what identity it
    /// derived, rather than hard-coding the answer. If the derivation changes, these tests follow it
    /// instead of pinning a stale string.
    const WORKSPACE: &str = "/nonexistent-arena-rpc/johnnyone";

    async fn derived_repo(state: &Arc<AppState>) -> String {
        let response = AgentService::handle_rpc(
            &rpc("get_repo_identity", json!({ "workspacePath": WORKSPACE })),
            state,
        )
        .await;
        assert!(response.success, "repo identity: {:?}", response.error);
        response
            .data
            .expect("identity")
            .as_str()
            .expect("a bare string")
            .to_string()
    }

    /// Every arena method is REGISTERED. Proven by the error coming from the param-puller or the
    /// service rather than from the dispatch's `_ =>` arm.
    #[tokio::test]
    async fn handle_rpc_registers_every_arena_method() {
        let (state, _root) = test_state();
        let state = Arc::new(state);

        // A deliberately empty payload: each arm must reject it with ITS OWN missing-parameter
        // message, which can only happen if the arm is wired to the right puller.
        let cases: &[(&str, &str)] = &[
            ("get_workflow", "Missing 'id' parameter"),
            ("list_workflow_runs", "Missing 'workflowId' parameter"),
            ("list_workflow_prefs", "Missing 'workflowId' parameter"),
            ("get_workflow_search_state", "Missing 'repo' parameter"),
            ("create_workflow_version", "Missing 'workspacePath' parameter"),
            ("get_repo_identity", "Missing 'workspacePath' parameter"),
            ("run_workflow_run_gate", "Missing 'runId' parameter"),
            ("rescore_workflow_run_gate", "Missing 'runId' parameter"),
            ("execute_workflow_run", "Missing 'runId' parameter"),
            ("record_workflow_run_rescore", "Missing 'runId' parameter"),
            ("start_workflow_run", "Missing 'workflowId' parameter"),
            ("record_workflow_run_gate", "Missing 'runId' parameter"),
            ("record_workflow_pref", "Missing 'winnerRunId' parameter"),
            ("update_workflow_champion", "Missing 'runId' parameter"),
            ("record_workflow_rejection", "Missing 'runId' parameter"),
            ("update_workflow_search_resumed", "Missing 'repo' parameter"),
        ];
        for (method, expected) in cases {
            let response = AgentService::handle_rpc(&rpc(method, json!({})), &state).await;
            assert!(!response.success, "{method} with no params must fail");
            let error = response.error.unwrap_or_default();
            assert!(
                !error.contains("Unknown RPC method"),
                "{method} is NOT registered in handle_rpc"
            );
            assert_eq!(&error, expected, "{method}: wrong param-puller");
        }

        // `list_workflows` takes no required parameter — an empty payload is the "list everything"
        // call, so it must SUCCEED. That is also what proves it is registered.
        let listed = AgentService::handle_rpc(&rpc("list_workflows", json!({})), &state).await;
        assert!(listed.success, "list_workflows: {:?}", listed.error);
        assert_eq!(listed.data.and_then(|data| data.as_array().map(|a| a.len())), Some(0));

        // And the unknown-method arm still works, so the assertions above are not vacuous.
        let bogus = AgentService::handle_rpc(&rpc("workflow_bogus", json!({})), &state).await;
        assert!(!bogus.success);
        assert!(bogus
            .error
            .unwrap_or_default()
            .contains("Unknown RPC method"));
    }

    /// **Every required parameter of every arena method, omitted one at a time.**
    ///
    /// `handle_rpc_registers_every_arena_method` sends an EMPTY payload, so it only ever exercises
    /// each method's FIRST missing-parameter message — the later pullers' strings are never run, and
    /// a typo in one of them would compile, pass every gate, and fail in production against the one
    /// client that omitted that argument. Same shape as the two unguarded rules an earlier revert
    /// sweep turned up.
    ///
    /// Each row is a complete valid payload; the test removes one key at a time and asserts the
    /// message names THAT key. Nothing here needs a workflow to exist: the pullers all run before
    /// the service is reached.
    #[tokio::test]
    async fn handle_rpc_names_every_missing_parameter_not_just_the_first() {
        let (state, _root) = test_state();
        let state = Arc::new(state);

        let complete: &[(&str, serde_json::Value, &[&str])] = &[
            (
                "create_workflow_version",
                json!({
                    "workspacePath": WORKSPACE,
                    "phaseType": "dev-phase",
                    "family": "glm",
                    "role": "r",
                    "instruction": "i",
                    "contract": "c",
                    "hop": "{}",
                    "fanOut": 1
                }),
                &["workspacePath", "phaseType", "family", "role", "instruction", "contract", "hop", "fanOut"],
            ),
            (
                "record_workflow_run_gate",
                json!({ "runId": "r1", "gateExit": 0 }),
                &["runId", "gateExit"],
            ),
            (
                "record_workflow_run_rescore",
                json!({ "runId": "r1", "gateExit": 0 }),
                &["runId", "gateExit"],
            ),
            (
                "record_workflow_pref",
                json!({ "winnerRunId": "a", "loserRunId": "b" }),
                &["winnerRunId", "loserRunId"],
            ),
            (
                "record_workflow_rejection",
                json!({ "runId": "r1", "reason": "no" }),
                &["runId", "reason"],
            ),
            (
                "update_workflow_search_resumed",
                json!({
                    "repo": "r",
                    "phaseType": "dev-phase",
                    "family": "glm",
                    "reason": "go on"
                }),
                &["repo", "phaseType", "family", "reason"],
            ),
            (
                "start_workflow_run",
                json!({ "workflowId": "w1" }),
                &["workflowId"],
            ),
            (
                "get_workflow_search_state",
                json!({ "repo": "r", "phaseType": "dev-phase", "family": "glm" }),
                &["repo", "phaseType", "family"],
            ),
            ("get_repo_identity", json!({ "workspacePath": WORKSPACE }), &["workspacePath"]),
            ("get_workflow", json!({ "id": "w1" }), &["id"]),
            ("list_workflow_runs", json!({ "workflowId": "w1" }), &["workflowId"]),
            ("list_workflow_prefs", json!({ "workflowId": "w1" }), &["workflowId"]),
            ("run_workflow_run_gate", json!({ "runId": "r1" }), &["runId"]),
            ("rescore_workflow_run_gate", json!({ "runId": "r1" }), &["runId"]),
            ("execute_workflow_run", json!({ "runId": "r1" }), &["runId"]),
            ("update_workflow_champion", json!({ "runId": "r1", "confirmed": true }), &["runId"]),
        ];

        for (method, payload, required) in complete {
            for omitted in *required {
                let mut params = payload.clone();
                params
                    .as_object_mut()
                    .expect("object payload")
                    .remove(*omitted);
                let response = AgentService::handle_rpc(&rpc(method, params), &state).await;
                assert!(
                    !response.success,
                    "{method} without {omitted} must fail"
                );
                let error = response.error.unwrap_or_default();
                assert_eq!(
                    error,
                    format!("Missing '{}' parameter", omitted),
                    "{method}: omitting {omitted} must name {omitted}"
                );
            }
        }
    }

    /// The whole arena, end to end, through `handle_rpc` only — the surface a phone actually
    /// reaches. Also pins the camelCase JSON keys both ways: the params the resolvers send and the
    /// field names they read back.
    #[tokio::test]
    async fn handle_rpc_drives_the_arena_end_to_end() {
        let (state, _root) = test_state();
        let state = Arc::new(state);

        // TWO workflows, because a displacing head-to-head must be against the INCUMBENT's run:
        // the incumbent (v1) seeds the champion slot, the challenger (v2) takes it from there.
        let mut workflow_ids = Vec::new();
        for expected_version in [1_i64, 2] {
            let created = AgentService::handle_rpc(
                &rpc(
                    "create_workflow_version",
                    json!({
                        "workspacePath": WORKSPACE,
                        "phaseType": "dev-phase",
                        "family": "glm-5.3-flash",
                        "role": "you are a developer",
                        "instruction": "implement the phase",
                        "contract": "the gate must pass",
                        "hop": "{\"steps\":[\"edit\",\"verify\"]}",
                        "fanOut": 3
                    }),
                ),
                &state,
            )
            .await;
            assert!(created.success, "create: {:?}", created.error);
            let workflow = created.data.expect("workflow");
            assert_eq!(
                workflow.get("version").and_then(|v| v.as_i64()),
                Some(expected_version)
            );
            assert_eq!(
                workflow.get("isChampion").and_then(|v| v.as_bool()),
                Some(false),
                "a new version is never born champion"
            );
            workflow_ids.push(
                workflow
                    .get("id")
                    .and_then(|v| v.as_str())
                    .expect("id")
                    .to_string(),
            );
        }
        let workflow_id = workflow_ids[0].clone();
        let challenger_id = workflow_ids[1].clone();

        // One run each, on the SAME seat budget AND the same fleet contention, so the preference
        // between them is comparable on both halves of the rule.
        let mut run_ids = Vec::new();
        for owner in [&workflow_id, &challenger_id] {
            // NOTE the payload deliberately ALSO sends `seatsHeld` and `fleetBusy`, which used to
            // be the real parameters. They must be ignored: the caller here is a browser and the
            // host is the only thing that can measure either. If a future edit reads them again,
            // the assertions below fail.
            let started = AgentService::handle_rpc(
                &rpc(
                    "start_workflow_run",
                    json!({
                        "workflowId": owner,
                        "seed": 0,
                        "seatsHeld": "{\"attacker\":999}",
                        "fleetBusy": "{\"attacker\":999}"
                    }),
                ),
                &state,
            )
            .await;
            assert!(started.success, "start: {:?}", started.error);
            let run = started.data.expect("run");
            assert!(
                run.get("gateExit").map(|v| v.is_null()).unwrap_or(false),
                "a fresh run has no score: {run}"
            );
            assert_eq!(
                run.get("seatsHeld").and_then(|v| v.as_str()),
                Some(r#"{"glm-5.3-flash":3}"#),
                "the HOST measured the budget from the workflow row, not from the caller: {run}"
            );
            // The contention snapshot is the host's too — and it is no longer `{}`. An empty map
            // used to mean "nothing measures contention" (the stand-in's value: the lease pool had
            // not landed, so every run reported `{}` whether the fleet was busy or not). Now that
            // `measure_run_budget` reads the real lease pool, a run on a quiet fleet reports every
            // capped family AT ZERO — `{"glm-5.3-flash":0}` — which is the assertion's whole point:
            // "measured, and the fleet was quiet" must not be confused with "never measured". A
            // weakened `is_some()` here would let the stand-in's `{}` pass again and erase that
            // distinction, so the exact snapshot is pinned.
            assert_eq!(
                run.get("fleetBusy").and_then(|v| v.as_str()),
                Some(r#"{"glm-5.3-flash":0}"#),
                "and the contention snapshot is the host's too, measured and quiet: {run}"
            );
            run_ids.push(run.get("id").and_then(|v| v.as_str()).unwrap().to_string());
        }

        // The winner's gate passes and reports no collected count — `gateCollected` ABSENT is a
        // normal result, not an error.
        let scored = AgentService::handle_rpc(
            &rpc(
                "record_workflow_run_gate",
                json!({ "runId": run_ids[0], "gateExit": 0 }),
            ),
            &state,
        )
        .await;
        assert!(scored.success, "gate: {:?}", scored.error);
        let scored = scored.data.expect("scored run");
        assert_eq!(scored.get("gateExit").and_then(|v| v.as_i64()), Some(0));
        assert!(
            scored.get("gateCollected").map(|v| v.is_null()).unwrap_or(false),
            "an absent gateCollected must stay null, not become 0: {scored}"
        );

        let loser_scored = AgentService::handle_rpc(
            &rpc(
                "record_workflow_run_gate",
                json!({ "runId": run_ids[1], "gateExit": 0, "gateCollected": 12 }),
            ),
            &state,
        )
        .await;
        assert!(loser_scored.success, "gate: {:?}", loser_scored.error);
        let loser_scored = loser_scored.data.expect("loser run");
        assert_eq!(loser_scored.get("gateExit").and_then(|v| v.as_i64()), Some(0));
        // The advisory count must ROUND-TRIP when it is supplied. Asserting only "the call
        // succeeded" let a renamed param key (`gateCollected` → `collected`) pass this test —
        // measured, not imagined: that mutation was green here until this assertion was added.
        assert_eq!(
            loser_scored.get("gateCollected").and_then(|v| v.as_i64()),
            Some(12),
            "a supplied gateCollected must arrive: {loser_scored}"
        );

        // A second score for the same run is refused unless `rescore` is asked for.
        let silent = AgentService::handle_rpc(
            &rpc(
                "record_workflow_run_gate",
                json!({ "runId": run_ids[1], "gateExit": 1 }),
            ),
            &state,
        )
        .await;
        assert!(!silent.success, "a silent re-score must be refused");
        assert!(silent
            .error
            .unwrap_or_default()
            .contains("already has a gate result"));
        // The deliberate replacement is its OWN method, not a flag on the one above.
        let deliberate = AgentService::handle_rpc(
            &rpc(
                "record_workflow_run_rescore",
                json!({ "runId": run_ids[1], "gateExit": 0, "gateCollected": 12 }),
            ),
            &state,
        )
        .await;
        assert!(deliberate.success, "rescore: {:?}", deliberate.error);

        // The incumbent takes the slot first, with no preference (nothing to displace).
        let seeded = AgentService::handle_rpc(
            &rpc(
                "update_workflow_champion",
                json!({ "runId": run_ids[0], "confirmed": true }),
            ),
            &state,
        )
        .await;
        assert!(seeded.success, "seed champion: {:?}", seeded.error);

        // Now the challenger's head-to-head — against the INCUMBENT's run, which is the only loser
        // that can displace it.
        let pref = AgentService::handle_rpc(
            &rpc(
                "record_workflow_pref",
                json!({
                    "winnerRunId": run_ids[1],
                    "loserRunId": run_ids[0],
                    "reason": "green gate"
                }),
            ),
            &state,
        )
        .await;
        assert!(pref.success, "pref: {:?}", pref.error);
        let pref = pref.data.expect("pref");
        assert_eq!(pref.get("comparable").and_then(|v| v.as_bool()), Some(true));
        let pref_id = pref.get("id").and_then(|v| v.as_str()).unwrap().to_string();

        // Promotion without `confirmed` must be refused — the default is the safe one.
        let unconfirmed = AgentService::handle_rpc(
            &rpc(
                "update_workflow_champion",
                json!({ "runId": run_ids[1], "prefId": pref_id }),
            ),
            &state,
        )
        .await;
        assert!(!unconfirmed.success, "an unconfirmed promote must fail");
        assert!(unconfirmed
            .error
            .unwrap_or_default()
            .contains("human-confirmed"));

        let promoted = AgentService::handle_rpc(
            &rpc(
                "update_workflow_champion",
                json!({ "runId": run_ids[1], "prefId": pref_id, "confirmed": true }),
            ),
            &state,
        )
        .await;
        assert!(promoted.success, "promote: {:?}", promoted.error);
        assert_eq!(
            promoted
                .data
                .expect("promoted")
                .get("isChampion")
                .and_then(|v| v.as_bool()),
            Some(true)
        );

        let search = AgentService::handle_rpc(
            &rpc(
                "get_workflow_search_state",
                json!({ "repo": derived_repo(&state).await, "phaseType": "dev-phase", "family": "glm-5.3-flash" }),
            ),
            &state,
        )
        .await;
        assert!(search.success, "search state: {:?}", search.error);
        let search = search.data.expect("search state");
        assert_eq!(
            search.get("championWorkflowId").and_then(|v| v.as_str()),
            Some(challenger_id.as_str()),
            "the challenger displaced the incumbent"
        );
        assert_eq!(search.get("halted").and_then(|v| v.as_bool()), Some(false));
        assert_eq!(search.get("strikeLimit").and_then(|v| v.as_i64()), Some(3));

        for owner in [&workflow_id, &challenger_id] {
            let runs = AgentService::handle_rpc(
                &rpc("list_workflow_runs", json!({ "workflowId": owner })),
                &state,
            )
            .await;
            assert!(runs.success);
            assert_eq!(
                runs.data
                    .and_then(|data| data.as_array().map(|rows| rows.len())),
                Some(1)
            );

            // The preference is readable back without issuing a write — the gap that forced a
            // reloaded client to record a duplicate.
            let prefs = AgentService::handle_rpc(
                &rpc("list_workflow_prefs", json!({ "workflowId": owner })),
                &state,
            )
            .await;
            assert!(prefs.success, "prefs: {:?}", prefs.error);
            let rows = prefs.data.expect("prefs");
            let rows = rows.as_array().expect("array");
            assert_eq!(rows.len(), 1, "both sides of the head-to-head see it");
            assert_eq!(rows[0].get("id").and_then(|v| v.as_str()), Some(pref_id.as_str()));
        }

        // And re-recording the SAME ordered pair returns the SAME row rather than a duplicate.
        let again = AgentService::handle_rpc(
            &rpc(
                "record_workflow_pref",
                json!({
                    "winnerRunId": run_ids[1],
                    "loserRunId": run_ids[0],
                    "reason": "a different explanation"
                }),
            ),
            &state,
        )
        .await;
        assert!(again.success, "re-record: {:?}", again.error);
        let again = again.data.expect("pref");
        assert_eq!(
            again.get("id").and_then(|v| v.as_str()),
            Some(pref_id.as_str()),
            "the same ordered pair must not create a second row"
        );
        assert_eq!(
            again.get("reason").and_then(|v| v.as_str()),
            Some("green gate"),
            "the FIRST reason wins: a recorded outcome is not editable"
        );
    }

    /// The halt, over the relay: three rejections and the next `start_workflow_run` is refused,
    /// with the halt visible as DATA on `get_workflow_search_state`.
    #[tokio::test]
    async fn handle_rpc_reports_and_enforces_the_three_strike_halt() {
        let (state, _root) = test_state();
        let state = Arc::new(state);

        let workflow_id = AgentService::handle_rpc(
            &rpc(
                "create_workflow_version",
                json!({
                    "workspacePath": WORKSPACE,
                    "phaseType": "ui-phase",
                    "family": "qwen",
                    "role": "r",
                    "instruction": "i",
                    "contract": "c",
                    "hop": "{}",
                    "fanOut": 1
                }),
            ),
            &state,
        )
        .await
        .data
        .expect("workflow")
        .get("id")
        .and_then(|v| v.as_str())
        .unwrap()
        .to_string();

        for index in 1..=3 {
            let run_id = AgentService::handle_rpc(
                &rpc(
                    "start_workflow_run",
                    json!({ "workflowId": workflow_id }),
                ),
                &state,
            )
            .await
            .data
            .expect("run")
            .get("id")
            .and_then(|v| v.as_str())
            .unwrap()
            .to_string();

            // A blank reason must be refused: the reasons are the point of the halt.
            let blank = AgentService::handle_rpc(
                &rpc(
                    "record_workflow_rejection",
                    json!({ "runId": run_id, "reason": "  " }),
                ),
                &state,
            )
            .await;
            assert!(!blank.success, "a blank rejection reason must be refused");

            let rejected = AgentService::handle_rpc(
                &rpc(
                    "record_workflow_rejection",
                    json!({ "runId": run_id, "reason": format!("strike {index}") }),
                ),
                &state,
            )
            .await;
            assert!(rejected.success, "reject: {:?}", rejected.error);
            let search = rejected.data.expect("search state");
            assert_eq!(
                search.get("consecutiveRejections").and_then(|v| v.as_i64()),
                Some(index)
            );
            assert_eq!(
                search.get("halted").and_then(|v| v.as_bool()),
                Some(index >= 3),
                "halt state at strike {index}"
            );
        }

        let blocked = AgentService::handle_rpc(
            &rpc(
                "start_workflow_run",
                json!({ "workflowId": workflow_id }),
            ),
            &state,
        )
        .await;
        assert!(!blocked.success, "a halted search must refuse a new run");
        let error = blocked.error.unwrap_or_default();
        assert!(error.contains("is halted after 3 consecutive rejections"), "got: {error}");
        assert!(error.contains("strike 3"), "the halt must name its reasons: {error}");

        // The ONLY way out, over the relay: an explicit, human-confirmed resume with a reason.
        // Unconfirmed first, because the default must not lift a halt.
        let key = json!({ "repo": derived_repo(&state).await, "phaseType": "ui-phase", "family": "qwen" });
        let mut unconfirmed_params = key.clone();
        unconfirmed_params["reason"] = json!("let it run");
        let unconfirmed =
            AgentService::handle_rpc(&rpc("update_workflow_search_resumed", unconfirmed_params), &state)
                .await;
        assert!(!unconfirmed.success, "an unconfirmed resume must be refused");
        assert!(unconfirmed
            .error
            .unwrap_or_default()
            .contains("human-confirmed"));

        let mut resume_params = key.clone();
        resume_params["reason"] = json!("the gate itself was broken");
        resume_params["confirmed"] = json!(true);
        let resumed =
            AgentService::handle_rpc(&rpc("update_workflow_search_resumed", resume_params), &state)
                .await;
        assert!(resumed.success, "resume: {:?}", resumed.error);
        let resumed = resumed.data.expect("search state");
        assert_eq!(resumed.get("halted").and_then(|v| v.as_bool()), Some(false));
        assert_eq!(
            resumed.get("consecutiveRejections").and_then(|v| v.as_i64()),
            Some(0)
        );

        let allowed = AgentService::handle_rpc(
            &rpc(
                "start_workflow_run",
                json!({ "workflowId": workflow_id }),
            ),
            &state,
        )
        .await;
        assert!(allowed.success, "after a resume a run must start: {:?}", allowed.error);
    }

    /// Three rejections of ONE run are ONE strike over the relay. The rule counts distinct
    /// candidates, not rows, so a retried call or a double-submitting button cannot halt a search
    /// on a single human verdict.
    #[tokio::test]
    async fn handle_rpc_counts_distinct_runs_not_rejection_rows() {
        let (state, _root) = test_state();
        let state = Arc::new(state);

        let workflow_id = AgentService::handle_rpc(
            &rpc(
                "create_workflow_version",
                json!({
                    "workspacePath": WORKSPACE,
                    "phaseType": "dev-phase",
                    "family": "glm",
                    "role": "r",
                    "instruction": "i",
                    "contract": "c",
                    "hop": "{}",
                    "fanOut": 1
                }),
            ),
            &state,
        )
        .await
        .data
        .expect("workflow")
        .get("id")
        .and_then(|v| v.as_str())
        .unwrap()
        .to_string();

        let run_id = AgentService::handle_rpc(
            &rpc(
                "start_workflow_run",
                json!({ "workflowId": workflow_id }),
            ),
            &state,
        )
        .await
        .data
        .expect("run")
        .get("id")
        .and_then(|v| v.as_str())
        .unwrap()
        .to_string();

        for attempt in 1..=5 {
            let rejected = AgentService::handle_rpc(
                &rpc(
                    "record_workflow_rejection",
                    json!({ "runId": run_id, "reason": format!("tap {attempt}") }),
                ),
                &state,
            )
            .await;
            assert!(rejected.success, "reject: {:?}", rejected.error);
            let search = rejected.data.expect("search state");
            assert_eq!(
                search.get("consecutiveRejections").and_then(|v| v.as_i64()),
                Some(1),
                "attempt {attempt}: one candidate is one strike however many times it is rejected"
            );
            assert_eq!(search.get("halted").and_then(|v| v.as_bool()), Some(false));
        }

        // And the search is still runnable, which is what makes the dedup meaningful.
        let allowed = AgentService::handle_rpc(
            &rpc(
                "start_workflow_run",
                json!({ "workflowId": workflow_id }),
            ),
            &state,
        )
        .await;
        assert!(allowed.success, "start: {:?}", allowed.error);
    }
}

/// The arena's EXECUTE door: `execute_workflow_run` over RPC.
///
/// Before this arm existed, nothing in the tree called
/// `services::workflows::execute_and_score_workflow_run`, so a run row was created and never
/// executed — the arena could not score anything through the real wire. These tests are about that
/// WIRING: the arm is registered, its parameters are pulled in the right order, the executor's
/// result reaches the response under `gateExit`, and an executor that never ran leaves the run
/// UNSCORED. The rules themselves (the family coming from the row, the composed prompt, the
/// already-scored refusal) live in `services::workflows` and are pinned there.
#[cfg(test)]
mod arena_exec_rpc_tests {
    use super::*;
    use crate::services::workflows::PhaseExecutor;
    use crate::test_support::test_state;
    use serde_json::json;
    use std::cell::RefCell;

    fn rpc(method: &str, params: serde_json::Value) -> RpcRequest {
        RpcRequest {
            request_id: "req-1".to_string(),
            method: method.to_string(),
            params,
        }
    }

    /// A `PhaseExecutor` double: it records what it was handed and answers with a fixed outcome.
    ///
    /// No real `kloo` is ever spawned here — the point is the wire, not the child.
    struct ExecRpcDouble {
        outcome: Result<i32, String>,
        calls: RefCell<Vec<(String, String)>>,
    }

    impl ExecRpcDouble {
        fn with_exit(code: i32) -> Self {
            ExecRpcDouble {
                outcome: Ok(code),
                calls: RefCell::new(Vec::new()),
            }
        }

        fn failing(msg: &str) -> Self {
            ExecRpcDouble {
                outcome: Err(msg.to_string()),
                calls: RefCell::new(Vec::new()),
            }
        }

        fn invocations(&self) -> usize {
            self.calls.borrow().len()
        }
    }

    impl PhaseExecutor for ExecRpcDouble {
        fn execute_scored(&self, family: &str, prompt: &str) -> Result<i32, String> {
            self.calls
                .borrow_mut()
                .push((family.to_string(), prompt.to_string()));
            self.outcome.clone()
        }
    }

    /// The workspace this agent resolves for a run: the run's plan's `workspace_path`.
    const EXEC_WORKSPACE: &str = "/nonexistent-arena-exec-rpc/johnnyone";

    /// Insert a bare plan row whose `workspace_path` is the synthetic workspace above.
    ///
    /// The column list mirrors the one `services::workflows`' own arena tests use: `initiative_id`
    /// and `initiative_status` are NOT NULL as far as the plan row mapper is concerned (migration
    /// 017), so omitting them fails the READ rather than the insert.
    fn seed_exec_plan(state: &AppState) -> String {
        seed_exec_plan_at(state, EXEC_WORKSPACE)
    }

    /// The same row, but with the plan's `workspace_path` chosen by the caller.
    ///
    /// The execute arm builds its own `KlooExecutor` from this column, so a test that wants the arm
    /// to run a REAL child — rather than a double it cannot inject — hands it a directory that
    /// exists and holds that child.
    fn seed_exec_plan_at(state: &AppState, workspace: &str) -> String {
        let id = uuid::Uuid::new_v4().to_string();
        state
            .db
            .with_conn(|conn| {
                conn.execute(
                    "INSERT INTO agent_plans (id, run_type, title, workspace_path, plan_path, \
                     worker_provider, reviewer_provider, initiative_id, initiative_status) \
                     VALUES (?1, 'development', 'arena-exec', ?2, ?3, 'claude_code', \
                     'claude_code', ?1, 'development')",
                    rusqlite::params![
                        id,
                        workspace,
                        format!("{}/plan", workspace),
                    ],
                )
                .map_err(|e| e.to_string())
            })
            .expect("seed plan");
        id
    }

    /// A workflow version and a run started against a plan, through the RPC arms a client uses.
    ///
    /// Returns `(runId, workflowId)`. The run carries a `plan_id` because the execute arm resolves
    /// its workdir from the plan row — a run with no plan has no workspace to execute in.
    async fn seeded_run(state: &Arc<AppState>, family: &str) -> (String, String) {
        seeded_run_in(state, family, EXEC_WORKSPACE).await
    }

    /// The same pair, with the plan's workspace — and so the arm's workdir — chosen by the caller.
    async fn seeded_run_in(
        state: &Arc<AppState>,
        family: &str,
        workspace: &str,
    ) -> (String, String) {
        let created = AgentService::handle_rpc(
            &rpc(
                "create_workflow_version",
                json!({
                    "workspacePath": workspace,
                    "phaseType": "dev-phase",
                    "family": family,
                    "role": "you are a developer",
                    "instruction": "implement the phase",
                    "contract": "the gate must pass",
                    "hop": "{\"steps\":[\"edit\",\"verify\"]}",
                    "fanOut": 1
                }),
            ),
            &state,
        )
        .await;
        assert!(created.success, "create: {:?}", created.error);
        let workflow_id = created
            .data
            .expect("workflow")
            .get("id")
            .and_then(|v| v.as_str())
            .expect("workflow id")
            .to_string();

        let plan_id = seed_exec_plan_at(state, workspace);
        let started = AgentService::handle_rpc(
            &rpc(
                "start_workflow_run",
                json!({ "workflowId": workflow_id, "planId": plan_id }),
            ),
            &state,
        )
        .await;
        assert!(started.success, "start: {:?}", started.error);
        let run = started.data.expect("run");
        assert!(
            run.get("gateExit").map(|v| v.is_null()).unwrap_or(false),
            "a fresh run is unscored: {run}"
        );
        (
            run.get("id")
                .and_then(|v| v.as_str())
                .expect("run id")
                .to_string(),
            workflow_id,
        )
    }

    /// A run's recorded gate, read back from the row rather than from a response body.
    fn recorded_gate(state: &AppState, run_id: &str) -> Option<i64> {
        crate::services::workflows::get_workflow_run(state, run_id)
            .expect("re-read the run")
            .gate_exit
    }

    /// **A missing `runId` is refused with the parameter error, and no gate is recorded.**
    ///
    /// The message is the same shape `rpc_start_workflow_run` gives for its own missing parameter,
    /// and the refusal happens BEFORE anything is looked up or executed, so the row it would have
    /// scored keeps `gate_exit` NULL.
    #[tokio::test]
    async fn arena_exec_rpc_refuses_a_missing_run_id() {
        let (state, _root) = test_state();
        let state = Arc::new(state);
        let (run_id, _) = seeded_run(&state, "glm-exec-missing").await;
        assert_eq!(recorded_gate(&state, &run_id), None, "nothing has run yet");

        // `verify` and `binary` are present; only `runId` is absent, so the error can only come
        // from the required-parameter puller.
        let response = AgentService::handle_rpc(
            &rpc("execute_workflow_run", json!({ "verify": "true", "binary": "kloo" })),
            &state,
        )
        .await;

        assert!(!response.success, "a missing runId must be refused");
        let error = response.error.unwrap_or_default();
        assert_eq!(
            error, "Missing 'runId' parameter",
            "the refusal must be the parameter error, not a service error"
        );
        assert!(
            !error.contains("Unknown RPC method"),
            "the arm must be registered in handle_rpc"
        );
        assert_eq!(
            recorded_gate(&state, &run_id),
            None,
            "a refused call must record no gate for the run that was not named"
        );
    }

    /// **An unknown `runId` is refused, and nothing is scored.**
    ///
    /// The run lookup happens before the executor is built, so a name that matches no row cannot
    /// reach a child process at all.
    #[tokio::test]
    async fn arena_exec_rpc_refuses_an_unknown_run_id() {
        let (state, _root) = test_state();
        let state = Arc::new(state);
        let (run_id, _) = seeded_run(&state, "glm-exec-unknown").await;

        let response = AgentService::handle_rpc(
            &rpc(
                "execute_workflow_run",
                json!({ "runId": "no-such-run-anywhere" }),
            ),
            &state,
        )
        .await;

        assert!(!response.success, "an unknown runId must be refused");
        let error = response.error.unwrap_or_default();
        assert!(
            !error.contains("Unknown RPC method"),
            "the arm is registered; this is the service refusing: {error}"
        );
        assert!(
            error.contains("no-such-run-anywhere"),
            "the refusal must name the run it could not find, got {error:?}"
        );
        // The run that DOES exist is untouched by a call that named a different one.
        assert_eq!(
            recorded_gate(&state, &run_id),
            None,
            "a refused call must leave every run unscored"
        );
    }

    /// **A successful execution returns `gateExit` and the run's recorded gate matches it.**
    ///
    /// 7 is deliberately neither 0 nor 1, so an implementation that clamped, booleanised or
    /// otherwise mangled the child's code cannot pass. The gate is asserted TWICE: in the response
    /// body and in the row, because the response is what the arena reads and the row is what
    /// `promotion_block` reads.
    #[tokio::test]
    async fn arena_exec_rpc_returns_gate_exit_and_records_it() {
        let (state, _root) = test_state();
        let state = Arc::new(state);
        let (run_id, workflow_id) = seeded_run(&state, "glm-exec-scored").await;

        let exec = ExecRpcDouble::with_exit(7);
        let gate_exit = crate::services::workflows::execute_and_score_workflow_run(
            &state,
            &run_id,
            &exec,
        )
        .expect("the execution ran");

        assert_eq!(gate_exit, 7, "the caller gets the child's exit code back");
        assert_eq!(exec.invocations(), 1, "one run, one execution");
        assert_eq!(
            recorded_gate(&state, &run_id),
            Some(7),
            "the run's recorded gate matches the exit code"
        );

        // And the same number reaches the wire under `gateExit`, through the arm itself. The
        // workflow row is re-read to prove the response describes the run that was named.
        let workflow = crate::services::workflows::get_workflow(&state, &workflow_id)
            .expect("re-read the workflow");
        assert_eq!(workflow.family, "glm-exec-scored");
        let response = AgentService::handle_rpc(
            &rpc(
                "record_workflow_run_rescore",
                json!({ "runId": run_id, "gateExit": 7 }),
            ),
            &state,
        )
        .await;
        assert!(response.success, "rescore: {:?}", response.error);
        let run = response.data.expect("rescored run");
        assert_eq!(
            run.get("gateExit").and_then(|v| v.as_i64()),
            Some(7),
            "the wire reads back the same score: {run}"
        );
    }

    /// **An executor that fails leaves the run's gate NULL.**
    ///
    /// The load-bearing one: an execution that did not happen must not look like an execution that
    /// failed, or a run that never touched a model could be compared against — and even promote
    /// over — one that did. The error must reach the caller as an RPC error, verbatim.
    #[tokio::test]
    async fn arena_exec_rpc_a_failing_executor_records_no_gate() {
        let (state, _root) = test_state();
        let state = Arc::new(state);
        let (run_id, _) = seeded_run(&state, "glm-exec-failed").await;
        assert_eq!(recorded_gate(&state, &run_id), None, "nothing has run yet");

        let exec = ExecRpcDouble::failing("the executor could not start");
        let err = crate::services::workflows::execute_and_score_workflow_run(
            &state,
            &run_id,
            &exec,
        )
        .expect_err("an execution that did not run must not report a code");

        assert_eq!(
            err, "the executor could not start",
            "the executor's error must reach the caller verbatim"
        );
        assert_eq!(exec.invocations(), 1, "the failure came from the executor, not before it");
        let after = crate::services::workflows::get_workflow_run(&state, &run_id)
            .expect("re-read the run");
        assert_eq!(
            after.gate_exit, None,
            "a failed execution must leave gate_exit NULL — no score, no exit code, nothing"
        );
        assert_eq!(after.gate_collected, None, "and no advisory number either");
        assert_eq!(after.ended_at, None, "and the run is not closed out");

        // The same refusal through the wire: the RPC error is the executor's message, and the row
        // is still unscored afterwards.
        let response = AgentService::handle_rpc(
            &rpc(
                "execute_workflow_run",
                json!({ "runId": run_id, "binary": "/nonexistent/kloo-not-here-7f3a" }),
            ),
            &state,
        )
        .await;
        assert!(!response.success, "a failed execution must be an RPC error");
        let error = response.error.unwrap_or_default();
        assert!(
            error.contains("could not start the phase executor binary"),
            "the RPC error must be the executor's own message, got {error:?}"
        );
        assert_eq!(
            recorded_gate(&state, &run_id),
            None,
            "an execution that did not happen must leave the run unscored"
        );
    }

    /// **The `execute_workflow_run` ARM answers with the gate exit, under `gateExit`.**
    ///
    /// The other three success-shaped tests cannot see this: the one that asserts a response body
    /// goes through `record_workflow_run_rescore`, and the two that call
    /// `execute_and_score_workflow_run` never touch the wire at all. So the arm could answer under
    /// any key it liked — `exitCode`, `code`, nothing — and the suite would stay green while the
    /// web client read `gateExit` and got `undefined`. This drives the arm itself to a SUCCESS and
    /// reads the key BY NAME.
    ///
    /// The arm builds its own `KlooExecutor` from the plan's `workspace_path`, so no double can be
    /// injected — the child has to be real. Following `execute_and_score_a_signal_has_no_exit_code`
    /// in `services::workflows`, the child is an executable script in a fresh temp dir: `#!/bin/sh`
    /// then `exit 7`. It ignores every argument, so the executor's argv shape (`prompt --verify … 
    /// --model <family>`) is irrelevant here; only its exit code is. 7 is neither 0 nor 1, so a
    /// mangled code cannot pass either.
    #[tokio::test]
    async fn arena_exec_rpc_the_arm_answers_with_the_gate_exit() {
        use std::os::unix::fs::PermissionsExt as _;

        let (state, _root) = test_state();
        let state = Arc::new(state);

        // A real child: a uniquely-named temp dir holding `exit-seven.sh`, executable, whose only
        // statement is `exit 7`. The dir doubles as the plan's workspace, which is what the arm
        // resolves as the child's workdir.
        let dir = std::env::temp_dir().join(format!(
            "wf-arm-exit-{}-{}",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        ));
        std::fs::create_dir_all(&dir).expect("create the temp dir for the exit-7 script");
        let script = dir.join("exit-seven.sh");
        std::fs::write(&script, "#!/bin/sh\nexit 7\n").expect("write exit-seven.sh");
        std::fs::set_permissions(&script, std::fs::Permissions::from_mode(0o755))
            .expect("make exit-seven.sh executable");

        let (run_id, _) =
            seeded_run_in(&state, "glm-exec-arm-exit", &dir.to_string_lossy()).await;
        assert_eq!(recorded_gate(&state, &run_id), None, "nothing has run yet");

        let response = AgentService::handle_rpc(
            &rpc(
                "execute_workflow_run",
                json!({ "runId": run_id, "binary": script.to_string_lossy() }),
            ),
            &state,
        )
        .await;

        // The temp dir goes BEFORE the asserts, so it is removed on the panic path too.
        let _ = std::fs::remove_dir_all(&dir);

        assert!(response.success, "the arm must succeed: {:?}", response.error);
        let data = response.data.expect("the arm answers with data");
        assert_eq!(
            data.get("gateExit").and_then(|v| v.as_i64()),
            Some(7),
            "the arm must answer under the key `gateExit` with the child's code: {data}"
        );
        // And it must answer with a WORKFLOW RUN, not a bare number: the worker's schema declares
        // this mutation to return `WorkflowRun!` with a non-null `id`, so a client selecting `id`
        // must find it. `gateExit` above is read BY NAME; `id` here pins that the payload is the
        // run row itself, the same shape `startWorkflowRun` and `runWorkflowRunGate` answer with.
        assert_eq!(
            data.get("id").and_then(|v| v.as_str()),
            Some(run_id.as_str()),
            "the arm must answer with the run row, whose `id` is the run it executed: {data}"
        );
        assert_eq!(
            recorded_gate(&state, &run_id),
            Some(7),
            "and the arm must SCORE the run, not merely answer — the row carries the gate"
        );
    }
}

#[cfg(test)]
mod session_view_tests {
    //! `session_view` is the FIFTH hop: it builds the `RpcSessionView` that the
    //! `list_sessions` RPC serialises, which the worker's `listAiSessions`
    //! forwards verbatim. A field the DB row has, the service computes and the
    //! GraphQL types declare still reads `null` on the client if THIS mapper
    //! drops it — so pin both the carry and the camelCase wire key here.

    use super::AgentService;

    fn session_with_terminal_status(status: &str) -> crate::db::models::Session {
        crate::db::models::Session {
            id: "s-1".into(),
            title: "a shell".into(),
            provider: "shell".into(),
            model: "".into(),
            working_directory: "/tmp".into(),
            status: "active".into(),
            cli_session_id: None,
            total_input_tokens: 0,
            total_output_tokens: 0,
            total_cost_cents: 0,
            kind: "user".into(),
            created_at: "2024-01-01T00:00:00Z".into(),
            updated_at: "2024-01-01T00:00:00Z".into(),
            attached_tmux: true,
            terminal_status: status.into(),
        }
    }

    #[test]
    fn session_view_carries_terminal_status() {
        let view = AgentService::session_view(session_with_terminal_status("ended"));
        assert_eq!(
            view.terminal_status, "ended",
            "the RPC view must carry the terminal's live state, not drop it"
        );
        // The relay hands the worker JSON, so the WIRE key is what the client
        // actually reads: `rename_all = "camelCase"` must land it as
        // `terminalStatus`, and it must be a string (never null).
        let json = serde_json::to_value(&view).expect("the view serialises");
        assert_eq!(
            json.get("terminalStatus").and_then(|v| v.as_str()),
            Some("ended"),
            "serialised RPC JSON must carry terminalStatus: {json}"
        );
    }

    #[test]
    fn session_view_carries_each_live_state_and_keeps_the_intent_flag_apart() {
        for state in ["none", "attached", "detached", "ended"] {
            let view = AgentService::session_view(session_with_terminal_status(state));
            assert_eq!(view.terminal_status, state);
            // Asked to attach (`attached_tmux: true`) yet reporting `none` /
            // `ended` — neither field may stand in for the other.
            assert!(view.attached_tmux);
        }
    }
}
