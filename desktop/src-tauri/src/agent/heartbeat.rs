use super::message_types::{AgentEnvelope, AgentMessage, Heartbeat, SystemInfoSnapshot};
use super::ws_client::WsWrite;
use futures_util::SinkExt;
use std::sync::Arc;
use sysinfo::System;
use tokio::sync::{broadcast, Mutex};
use tokio_tungstenite::tungstenite::Message;

/// Default heartbeat interval in seconds.
const HEARTBEAT_INTERVAL_SECS: u64 = 30;

/// Interval (in heartbeat cycles) at which to include system info.
const SYSTEM_INFO_EVERY_N: u64 = 4;

/// Run the periodic heartbeat sender.
///
/// Sends a heartbeat message at regular intervals to keep the WebSocket
/// connection alive and report node status to the Cloudflare Worker.
/// Every N heartbeats, includes a system info snapshot.
pub async fn run_heartbeat(
    ws_write: Arc<Mutex<WsWrite>>,
    shutdown_rx: &mut broadcast::Receiver<()>,
) {
    let mut interval = tokio::time::interval(tokio::time::Duration::from_secs(HEARTBEAT_INTERVAL_SECS));
    let mut cycle: u64 = 0;
    let mut sys = System::new();

    tracing::info!(
        interval_secs = HEARTBEAT_INTERVAL_SECS,
        "Heartbeat sender started"
    );

    loop {
        tokio::select! {
            _ = interval.tick() => {
                cycle += 1;

                // Gather system info every N cycles
                let system_info = if cycle % SYSTEM_INFO_EVERY_N == 0 {
                    sys.refresh_cpu_usage();
                    sys.refresh_memory();

                    Some(SystemInfoSnapshot {
                        cpu_usage_percent: sys.global_cpu_usage(),
                        memory_used_mb: sys.used_memory() / (1024 * 1024),
                        memory_total_mb: sys.total_memory() / (1024 * 1024),
                        uptime_seconds: System::uptime(),
                    })
                } else {
                    None
                };

                let heartbeat = AgentEnvelope {
                    message: AgentMessage::Heartbeat(Heartbeat {
                        timestamp: chrono::Utc::now().to_rfc3339(),
                        node_id: None, // Will be populated from app state in future
                        system_info,
                    }),
                };

                let json = match serde_json::to_string(&heartbeat) {
                    Ok(j) => j,
                    Err(e) => {
                        tracing::error!(error = %e, "Failed to serialize heartbeat");
                        continue;
                    }
                };

                let mut writer = ws_write.lock().await;
                match writer.send(Message::Text(json.into())).await {
                    Ok(()) => {
                        tracing::trace!(cycle = cycle, "Heartbeat sent");
                    }
                    Err(e) => {
                        tracing::error!(error = %e, "Failed to send heartbeat");
                        // Connection is likely broken, exit the heartbeat loop
                        // The main message loop will detect this and handle reconnection
                        return;
                    }
                }
            }
            _ = shutdown_rx.recv() => {
                tracing::info!("Heartbeat sender shutting down");
                return;
            }
        }
    }
}
