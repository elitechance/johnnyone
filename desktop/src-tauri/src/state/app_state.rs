use crate::tools::tool_schema::ToolExecutionRecord;
use chrono::{DateTime, Utc};
use tokio::sync::{broadcast, Mutex};
use uuid::Uuid;

/// Connection status information for the agent WebSocket.
#[derive(Debug, Clone)]
pub struct ConnectionStatus {
    /// Whether the WebSocket is currently connected.
    pub connected: bool,
    /// The active session ID (if any).
    pub session_id: Option<String>,
    /// Timestamp of the last successful heartbeat.
    pub last_heartbeat: Option<DateTime<Utc>>,
}

impl Default for ConnectionStatus {
    fn default() -> Self {
        Self {
            connected: false,
            session_id: None,
            last_heartbeat: None,
        }
    }
}

/// Central application state, shared across Tauri commands and background tasks.
///
/// Uses `tokio::sync::Mutex` for async-safe access from Tauri command handlers
/// and the agent service background task.
#[derive(Debug, Clone)]
pub struct AppState {
    /// Current WebSocket connection status.
    pub connection_status: std::sync::Arc<Mutex<ConnectionStatus>>,
    /// Unique node ID for this desktop agent instance.
    pub node_id: String,
    /// Human-readable node name.
    pub node_name: String,
    /// History of tool executions.
    pub tool_executions: std::sync::Arc<Mutex<Vec<ToolExecutionRecord>>>,
    /// Broadcast channel sender for shutdown signals.
    pub shutdown_tx: broadcast::Sender<()>,
}

impl AppState {
    /// Create a new AppState with default values.
    pub fn new() -> Self {
        let node_id = Uuid::new_v4().to_string();
        let node_name = format!("desktop-{}", &node_id[..8]);
        let (shutdown_tx, _) = broadcast::channel(16);

        tracing::info!(
            node_id = %node_id,
            node_name = %node_name,
            "Initializing app state"
        );

        Self {
            connection_status: std::sync::Arc::new(Mutex::new(ConnectionStatus::default())),
            node_id,
            node_name,
            tool_executions: std::sync::Arc::new(Mutex::new(Vec::new())),
            shutdown_tx,
        }
    }

    /// Create a new AppState with a specific node ID and name.
    pub fn with_node_info(node_id: String, node_name: String) -> Self {
        let (shutdown_tx, _) = broadcast::channel(16);

        Self {
            connection_status: std::sync::Arc::new(Mutex::new(ConnectionStatus::default())),
            node_id,
            node_name,
            tool_executions: std::sync::Arc::new(Mutex::new(Vec::new())),
            shutdown_tx,
        }
    }

    /// Clear old tool execution records, keeping only the most recent `max_records`.
    pub async fn prune_executions(&self, max_records: usize) {
        let mut executions = self.tool_executions.lock().await;
        if executions.len() > max_records {
            let drain_count = executions.len() - max_records;
            executions.drain(..drain_count);
            tracing::debug!(
                pruned = drain_count,
                remaining = executions.len(),
                "Pruned old execution records"
            );
        }
    }
}

impl Default for AppState {
    fn default() -> Self {
        Self::new()
    }
}
