use crate::services::relay;
use crate::state::app_state::AppState;
use serde::Serialize;
use tauri::State;

#[derive(Debug, Serialize)]
pub struct ConnectionStatusResponse {
    pub connected: bool,
    pub session_id: Option<String>,
    pub last_heartbeat: Option<String>,
}

/// Start the agent WebSocket connection using persisted host settings.
#[tauri::command]
pub async fn start_agent(state: State<'_, AppState>) -> Result<(), String> {
    tracing::info!("Starting agent connection from host settings");
    relay::ensure_connected(state.inner().clone()).await
}

/// Stop the agent WebSocket connection.
#[tauri::command]
pub async fn stop_agent(state: State<'_, AppState>) -> Result<(), String> {
    tracing::info!("Stopping agent connection");

    if state.shutdown_tx.send(()).is_err() {
        tracing::debug!("No active agent listeners for shutdown signal");
    }

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