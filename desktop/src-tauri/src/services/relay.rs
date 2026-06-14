use crate::agent::{AgentConfig, AgentService};
use crate::services::settings::{self, RelayConfig};
use crate::state::app_state::{AppState, WorkerRelayConfig};
use std::time::Duration;

/// Start (or no-op if already active) the outbound worker relay WebSocket.
pub async fn ensure_connected(state: AppState) -> Result<(), String> {
    {
        let status = state.connection_status.lock().await;
        if status.session_id.is_some() {
            tracing::info!("Relay connection already active; skipping duplicate start");
            return Ok(());
        }
    }

    let config = settings::RelayConfig::resolve(&state)
        .ok_or_else(|| {
            "Relay is not configured. Set worker URL and sign in via the host app.".to_string()
        })?;

    spawn_connection(state, config).await;
    Ok(())
}

/// Spawn the relay connection loop on the current async runtime.
pub async fn spawn_connection(state: AppState, config: RelayConfig) {
    state
        .set_worker_relay_config(WorkerRelayConfig {
            worker_url: config.worker_url.clone(),
            user_id: config.user_id.clone(),
            tenant_id: config.tenant_id.clone(),
        })
        .await;

    {
        let mut status = state.connection_status.lock().await;
        status.session_id = Some("host-agent".to_string());
    }

    let agent_config = AgentConfig {
        worker_url: config.worker_url,
        user_id: config.user_id,
        tenant_id: config.tenant_id,
    };

    tokio::spawn(async move {
        loop {
            match AgentService::start(
                AgentConfig {
                    worker_url: agent_config.worker_url.clone(),
                    user_id: agent_config.user_id.clone(),
                    tenant_id: agent_config.tenant_id.clone(),
                },
                state.clone(),
            )
            .await
            {
                Ok(()) => break,
                Err(error) => {
                    tracing::error!(
                        %error,
                        "Backend relay connection failed; retrying in 2s"
                    );
                    tokio::time::sleep(Duration::from_secs(2)).await;
                }
            }
        }
    });
}

/// Boot-time hook: connect when settings (or env overrides) are complete.
pub async fn spawn_if_configured(state: AppState) {
    let Some(config) = settings::RelayConfig::resolve(&state) else {
        tracing::info!(
            "Relay not configured — sign in via the host app or set worker_url + user_id in settings"
        );
        return;
    };

    tracing::info!(
        worker_url = %config.worker_url,
        user_id = %config.user_id,
        tenant_id = %config.tenant_id,
        "Starting relay from persisted host settings"
    );
    spawn_connection(state, config).await;
}