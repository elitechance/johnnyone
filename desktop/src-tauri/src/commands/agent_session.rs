use crate::agent::{AgentConfig, AgentService};
use crate::state::app_state::AppState;
use serde::Serialize;
use tauri::State;

#[derive(Debug, Serialize)]
pub struct ConnectionStatusResponse {
    pub connected: bool,
    pub session_id: Option<String>,
    pub last_heartbeat: Option<String>,
}

/// Helper to read a setting from the DB, returning a default if not found.
fn get_setting(state: &AppState, key: &str, default: &str) -> String {
    state
        .db
        .with_conn(|conn| {
            let result = conn.query_row(
                "SELECT value FROM settings WHERE key = ?1",
                rusqlite::params![key],
                |row| row.get::<_, String>(0),
            );
            Ok(result.unwrap_or_else(|_| default.to_string()))
        })
        .unwrap_or_else(|_| default.to_string())
}

/// Start the agent WebSocket connection.
/// Reads worker_url, user_id, and tenant_id from settings DB.
#[tauri::command]
pub async fn start_agent(
    state: State<'_, AppState>,
) -> Result<(), String> {
    tracing::info!("Starting agent connection");

    // Check if already started (connected or currently retrying)
    {
        let status = state.connection_status.lock().await;
        if status.session_id.is_some() {
            tracing::info!("Agent connection already active; skipping duplicate start");
            return Ok(());
        }
    }

    // Read config from settings DB
    let worker_url = get_setting(&state, "worker_url", "http://localhost:7714");
    let user_id = get_setting(&state, "user_id", "");
    let tenant_id = get_setting(&state, "tenant_id", "johnnyone-default");

    if user_id.is_empty() {
        return Err("user_id not configured. Set it in settings first.".to_string());
    }

    // Mark session as active
    {
        let mut status = state.connection_status.lock().await;
        status.session_id = Some("agent".to_string());
    }

    let config = AgentConfig {
        worker_url,
        user_id,
        tenant_id,
    };

    // Start the agent service in a background task
    let agent_state = state.inner().clone();
    tokio::spawn(async move {
        match AgentService::start(config, agent_state).await {
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
    if state.shutdown_tx.send(()).is_err() {
        tracing::debug!("No active agent listeners for shutdown signal");
    }

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
