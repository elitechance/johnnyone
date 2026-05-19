use crate::db::Database;
use crate::events::{ChatCompleteEvent, ChatDeltaEvent, TerminalScreenEvent};
use crate::providers::cli_runner::CliProcess;
use crate::tools::tool_schema::ToolExecutionRecord;
use chrono::{DateTime, Utc};
use std::collections::HashMap;
use std::fmt;
use std::sync::Arc;
use tokio::sync::{broadcast, Mutex};
use tokio::task::JoinHandle;
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
#[derive(Clone)]
pub struct AppState {
    /// Local SQLite database.
    pub db: Database,
    /// Current WebSocket connection status.
    pub connection_status: Arc<Mutex<ConnectionStatus>>,
    /// Unique node ID for this desktop agent instance.
    pub node_id: String,
    /// Human-readable node name.
    pub node_name: String,
    /// History of tool executions.
    pub tool_executions: Arc<Mutex<Vec<ToolExecutionRecord>>>,
    /// Currently running CLI subprocesses, keyed by session_id.
    pub active_processes: Arc<Mutex<HashMap<String, CliProcess>>>,
    /// Broadcast channel sender for shutdown signals.
    pub shutdown_tx: broadcast::Sender<()>,
    /// Broadcast channel for session deletion events (sends session ID).
    pub session_deleted_tx: broadcast::Sender<String>,
    /// Broadcast channel for host chat deltas.
    pub chat_delta_tx: broadcast::Sender<ChatDeltaEvent>,
    /// Broadcast channel for host chat completion notifications.
    pub chat_complete_tx: broadcast::Sender<ChatCompleteEvent>,
    /// Broadcast channel for terminal screen updates.
    pub terminal_screen_tx: broadcast::Sender<TerminalScreenEvent>,
    /// Active tmux capture loops, keyed by session_id.
    pub terminal_capture_tasks: Arc<Mutex<HashMap<String, JoinHandle<()>>>>,
}

impl AppState {
    /// Create a new AppState with a database.
    pub fn new(db: Database) -> Self {
        let node_id = Uuid::new_v4().to_string();
        let node_name = format!("desktop-{}", &node_id[..8]);
        let (shutdown_tx, _) = broadcast::channel(16);
        let (session_deleted_tx, _) = broadcast::channel(16);
        let (chat_delta_tx, _) = broadcast::channel(256);
        let (chat_complete_tx, _) = broadcast::channel(64);
        let (terminal_screen_tx, _) = broadcast::channel(256);

        tracing::info!(
            node_id = %node_id,
            node_name = %node_name,
            "Initializing app state"
        );

        Self {
            db,
            connection_status: Arc::new(Mutex::new(ConnectionStatus::default())),
            node_id,
            node_name,
            tool_executions: Arc::new(Mutex::new(Vec::new())),
            active_processes: Arc::new(Mutex::new(HashMap::new())),
            shutdown_tx,
            session_deleted_tx,
            chat_delta_tx,
            chat_complete_tx,
            terminal_screen_tx,
            terminal_capture_tasks: Arc::new(Mutex::new(HashMap::new())),
        }
    }

    /// Create a new AppState with a specific node ID and name.
    pub fn with_node_info(db: Database, node_id: String, node_name: String) -> Self {
        let (shutdown_tx, _) = broadcast::channel(16);
        let (session_deleted_tx, _) = broadcast::channel(16);
        let (chat_delta_tx, _) = broadcast::channel(256);
        let (chat_complete_tx, _) = broadcast::channel(64);
        let (terminal_screen_tx, _) = broadcast::channel(256);

        Self {
            db,
            connection_status: Arc::new(Mutex::new(ConnectionStatus::default())),
            node_id,
            node_name,
            tool_executions: Arc::new(Mutex::new(Vec::new())),
            active_processes: Arc::new(Mutex::new(HashMap::new())),
            shutdown_tx,
            session_deleted_tx,
            chat_delta_tx,
            chat_complete_tx,
            terminal_screen_tx,
            terminal_capture_tasks: Arc::new(Mutex::new(HashMap::new())),
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

impl fmt::Debug for AppState {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("AppState")
            .field("node_id", &self.node_id)
            .field("node_name", &self.node_name)
            .finish_non_exhaustive()
    }
}
