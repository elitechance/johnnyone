use crate::agent::AgentService;
use crate::state::app_state::AppState;
use serde::Serialize;
use tauri::State;

#[derive(Debug, Serialize)]
pub struct ConnectionStatusResponse {
    pub connected: bool,
    pub session_id: Option<String>,
    pub last_heartbeat: Option<String>,
}

/// Start the agent WebSocket connection for the given session.
#[tauri::command]
pub async fn start_agent(
    session_id: String,
    state: State<'_, AppState>,
) -> Result<(), String> {
    tracing::info!(session_id = %session_id, "Starting agent connection");

    // Validate session ID
    if session_id.is_empty() {
        return Err("Session ID cannot be empty".to_string());
    }

    // Check if already connected
    {
        let status = state.connection_status.lock().await;
        if status.connected {
            return Err("Agent is already connected. Disconnect first.".to_string());
        }
    }

    // Update state with new session
    {
        let mut status = state.connection_status.lock().await;
        status.session_id = Some(session_id.clone());
    }

    // Build the WebSocket URL from the session ID
    let ws_url = format!(
        "wss://johnnyone.app/api/agent/session/{}/ws",
        session_id
    );

    // Start the agent service in a background task
    let agent_state = state.inner().clone();
    tokio::spawn(async move {
        match AgentService::start(ws_url, agent_state).await {
            Ok(()) => tracing::info!("Agent service stopped cleanly"),
            Err(e) => tracing::error!(error = %e, "Agent service error"),
        }
    });

    Ok(())
}

/// Stop the agent WebSocket connection.
#[tauri::command]
pub async fn stop_agent(state: State<'_, AppState>) -> Result<(), String> {
    tracing::info!("Stopping agent connection");

    // Signal the agent to stop
    state
        .shutdown_tx
        .send(())
        .map_err(|_| "Failed to send shutdown signal".to_string())?;

    // Update connection status
    {
        let mut status = state.connection_status.lock().await;
        status.connected = false;
        status.session_id = None;
        status.last_heartbeat = None;
    }

    tracing::info!("Agent connection stopped");
    Ok(())
}

/// Get the current connection status.
#[tauri::command]
pub async fn get_connection_status(
    state: State<'_, AppState>,
) -> Result<ConnectionStatusResponse, String> {
    let status = state.connection_status.lock().await;
    Ok(ConnectionStatusResponse {
        connected: status.connected,
        session_id: status.session_id.clone(),
        last_heartbeat: status.last_heartbeat.as_ref().map(|t| t.to_rfc3339()),
    })
}
