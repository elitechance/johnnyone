pub mod heartbeat;
pub mod message_types;
pub mod reconnect;
pub mod ws_client;

use crate::state::app_state::AppState;
use crate::tools::ToolDispatcher;
use futures_util::{SinkExt, StreamExt};
use message_types::{AgentEnvelope, AgentMessage};
use std::sync::Arc;
use tokio::sync::Mutex;
use tracing;

/// AgentService manages the lifecycle of the WebSocket connection to the
/// Cloudflare Worker AgentSessionDO. It handles connecting, message routing,
/// heartbeats, and reconnection.
pub struct AgentService;

impl AgentService {
    /// Start the agent service. This will:
    /// 1. Connect to the WebSocket endpoint
    /// 2. Start the heartbeat sender
    /// 3. Listen for incoming messages and route them to tool executors
    /// 4. Handle reconnection on disconnect
    pub async fn start(ws_url: String, state: AppState) -> Result<(), String> {
        let state = Arc::new(state);
        let reconnector = reconnect::ReconnectPolicy::new();
        let tool_dispatcher = Arc::new(Mutex::new(ToolDispatcher::new()));

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
                        tokio::spawn(async move {
                            heartbeat::run_heartbeat(ws_write, &mut shutdown_rx).await;
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

    /// Process incoming WebSocket messages, dispatching tool calls and sending results.
    async fn process_messages(
        mut ws_read: ws_client::WsRead,
        ws_write: Arc<Mutex<ws_client::WsWrite>>,
        state: Arc<AppState>,
        tool_dispatcher: Arc<Mutex<ToolDispatcher>>,
    ) -> Result<(), String> {
        let mut shutdown_rx = state.shutdown_tx.subscribe();

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
                    if let Some(rec) = executions.iter_mut().find(|r| r.call_id == tool_call.call_id) {
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
                    .send(tokio_tungstenite::tungstenite::Message::Text(response_text.into()))
                    .await
                    .map_err(|e| format!("Send error: {}", e))?;
            }
            AgentMessage::Heartbeat(hb) => {
                tracing::debug!(ts = %hb.timestamp, "Received heartbeat ack");
                let mut status = state.connection_status.lock().await;
                status.last_heartbeat = Some(chrono::Utc::now());
            }
            AgentMessage::ToolResult(_) => {
                // We sent this, shouldn't receive it back normally
                tracing::debug!("Received tool result echo");
            }
            AgentMessage::Error(err) => {
                tracing::error!(code = %err.code, message = %err.message, "Server error");
            }
        }

        Ok(())
    }
}
