use crate::db::Database;
use crate::events::{
    AgentPlanRunEvent, ChatCompleteEvent, ChatDeltaEvent, SessionUpdatedEvent, TerminalScreenEvent,
};
use crate::providers::cli_runner::CliProcess;
use crate::tools::tool_schema::ToolExecutionRecord;
use chrono::{DateTime, Utc};
use std::collections::HashMap;
use std::fmt;
use std::sync::Arc;
use std::time::Instant;
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

#[derive(Debug, Clone, Default)]
pub struct WorkerRelayConfig {
    pub worker_url: String,
    pub user_id: String,
    pub tenant_id: String,
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
    /// Broadcast channel for session create/update/archive events.
    pub session_updated_tx: broadcast::Sender<SessionUpdatedEvent>,
    /// Broadcast channel for host chat deltas.
    pub chat_delta_tx: broadcast::Sender<ChatDeltaEvent>,
    /// Broadcast channel for host chat completion notifications.
    pub chat_complete_tx: broadcast::Sender<ChatCompleteEvent>,
    /// Broadcast channel for terminal screen updates.
    pub terminal_screen_tx: broadcast::Sender<TerminalScreenEvent>,
    /// Broadcast channel for planner run state updates.
    pub agent_plan_run_tx: broadcast::Sender<AgentPlanRunEvent>,
    /// Active tmux capture loops, keyed by session_id.
    pub terminal_capture_tasks: Arc<Mutex<HashMap<String, JoinHandle<()>>>>,
    /// Visible UI subscribers for tmux screen mirroring, keyed by session_id.
    pub terminal_visual_subscribers: Arc<Mutex<HashMap<String, usize>>>,
    /// Last keyboard/input activity per terminal session for adaptive capture pacing.
    pub terminal_last_input_at: Arc<Mutex<HashMap<String, Instant>>>,
    /// Last relay/UI publish time per session — enforces min interval for DO traffic.
    pub terminal_last_screen_publish_at: Arc<Mutex<HashMap<String, Instant>>>,
    /// Latest screen event coalesced while publish throttle is active.
    pub terminal_pending_screen: Arc<Mutex<HashMap<String, TerminalScreenEvent>>>,
    /// Sessions that already have a deferred publish flush scheduled.
    pub terminal_screen_flush_scheduled: Arc<Mutex<HashMap<String, bool>>>,
    /// Worker connection context used by host-side relay follow-up calls.
    pub worker_relay_config: Arc<Mutex<Option<WorkerRelayConfig>>>,
}

impl AppState {
    /// Create a new AppState with a database.
    pub fn new(db: Database) -> Self {
        let node_id = Uuid::new_v4().to_string();
        let node_name = format!("desktop-{}", &node_id[..8]);
        let (shutdown_tx, _) = broadcast::channel(16);
        let (session_deleted_tx, _) = broadcast::channel(16);
        let (session_updated_tx, _) = broadcast::channel(256);
        let (chat_delta_tx, _) = broadcast::channel(256);
        let (chat_complete_tx, _) = broadcast::channel(64);
        let (terminal_screen_tx, _) = broadcast::channel(256);
        let (agent_plan_run_tx, _) = broadcast::channel(256);

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
            session_updated_tx,
            chat_delta_tx,
            chat_complete_tx,
            terminal_screen_tx,
            agent_plan_run_tx,
            terminal_capture_tasks: Arc::new(Mutex::new(HashMap::new())),
            terminal_visual_subscribers: Arc::new(Mutex::new(HashMap::new())),
            terminal_last_input_at: Arc::new(Mutex::new(HashMap::new())),
            terminal_last_screen_publish_at: Arc::new(Mutex::new(HashMap::new())),
            terminal_pending_screen: Arc::new(Mutex::new(HashMap::new())),
            terminal_screen_flush_scheduled: Arc::new(Mutex::new(HashMap::new())),
            worker_relay_config: Arc::new(Mutex::new(None)),
        }
    }

    /// Create a new AppState with a specific node ID and name.
    pub fn with_node_info(db: Database, node_id: String, node_name: String) -> Self {
        let (shutdown_tx, _) = broadcast::channel(16);
        let (session_deleted_tx, _) = broadcast::channel(16);
        let (session_updated_tx, _) = broadcast::channel(256);
        let (chat_delta_tx, _) = broadcast::channel(256);
        let (chat_complete_tx, _) = broadcast::channel(64);
        let (terminal_screen_tx, _) = broadcast::channel(256);
        let (agent_plan_run_tx, _) = broadcast::channel(256);

        Self {
            db,
            connection_status: Arc::new(Mutex::new(ConnectionStatus::default())),
            node_id,
            node_name,
            tool_executions: Arc::new(Mutex::new(Vec::new())),
            active_processes: Arc::new(Mutex::new(HashMap::new())),
            shutdown_tx,
            session_deleted_tx,
            session_updated_tx,
            chat_delta_tx,
            chat_complete_tx,
            terminal_screen_tx,
            agent_plan_run_tx,
            terminal_capture_tasks: Arc::new(Mutex::new(HashMap::new())),
            terminal_visual_subscribers: Arc::new(Mutex::new(HashMap::new())),
            terminal_last_input_at: Arc::new(Mutex::new(HashMap::new())),
            terminal_last_screen_publish_at: Arc::new(Mutex::new(HashMap::new())),
            terminal_pending_screen: Arc::new(Mutex::new(HashMap::new())),
            terminal_screen_flush_scheduled: Arc::new(Mutex::new(HashMap::new())),
            worker_relay_config: Arc::new(Mutex::new(None)),
        }
    }

    pub async fn set_worker_relay_config(&self, config: WorkerRelayConfig) {
        let mut worker_config = self.worker_relay_config.lock().await;
        *worker_config = Some(config);
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
