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

/// Give up on a heartbeat write after this long, including the wait for the shared writer lock.
///
/// Comfortably longer than any healthy send, well under the main loop's staleness threshold, so a
/// wedged write surfaces as a reconnect rather than silence.
const HEARTBEAT_SEND_TIMEOUT_SECS: u64 = 10;

/// Run the periodic heartbeat sender.
///
/// Sends a heartbeat message at regular intervals to keep the WebSocket
/// connection alive and report node status to the Cloudflare Worker.
/// Every N heartbeats, includes a system info snapshot.
pub async fn run_heartbeat(
    ws_write: Arc<Mutex<WsWrite>>,
    shutdown_rx: &mut broadcast::Receiver<()>,
    node_id: String,
) {
    let mut interval =
        tokio::time::interval(tokio::time::Duration::from_secs(HEARTBEAT_INTERVAL_SECS));
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
                        node_id: Some(node_id.clone()),
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

                // Bound BOTH the lock wait and the send.
                //
                // On a half-open socket a send can block indefinitely, and it holds `ws_write`
                // while it does — which starves every other writer, this loop included. Seen in
                // the wild as a 53-minute gap in heartbeats (14:15Z → 15:08Z) with no error
                // logged: the loop was not idle, it was parked on the mutex behind a hung send.
                // That silence is what made the dead relay invisible.
                //
                // Timing out instead lets the loop exit, which stops the acks, which the main
                // loop's liveness watchdog sees — so a wedged socket now ends in a reconnect
                // rather than a permanently connected-looking node.
                let send = async {
                    let mut writer = ws_write.lock().await;
                    writer.send(Message::Text(json.into())).await
                };
                match tokio::time::timeout(
                    tokio::time::Duration::from_secs(HEARTBEAT_SEND_TIMEOUT_SECS),
                    send,
                )
                .await
                {
                    Ok(Ok(())) => {
                        tracing::trace!(cycle = cycle, "Heartbeat sent");
                    }
                    Ok(Err(e)) => {
                        tracing::error!(error = %e, "Failed to send heartbeat");
                        // Connection is broken. Exiting stops the acks, so the main loop's
                        // liveness watchdog tears the connection down and reconnects.
                        return;
                    }
                    Err(_) => {
                        tracing::error!(
                            timeout_secs = HEARTBEAT_SEND_TIMEOUT_SECS,
                            "Heartbeat send timed out — relay write side is wedged"
                        );
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
