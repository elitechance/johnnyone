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

/// Spawn the relay connection loop on the current async runtime, storing its
/// handle so it can be torn down on a settings change (see `reconnect`).
pub async fn spawn_connection(state: AppState, config: RelayConfig) {
    // Tear down any prior relay loop before starting a new one.
    if let Some(prev) = state.relay_task.lock().await.take() {
        prev.abort();
    }

    state
        .set_worker_relay_config(WorkerRelayConfig {
            worker_url: config.worker_url.clone(),
            user_id: config.user_id.clone(),
            tenant_id: config.tenant_id.clone(),
            access_token: config.access_token.clone(),
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
        access_token: config.access_token,
    };

    let task_state = state.clone();
    let handle = tokio::spawn(async move {
        loop {
            match AgentService::start(
                AgentConfig {
                    worker_url: agent_config.worker_url.clone(),
                    user_id: agent_config.user_id.clone(),
                    tenant_id: agent_config.tenant_id.clone(),
                    access_token: agent_config.access_token.clone(),
                },
                task_state.clone(),
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

    *state.relay_task.lock().await = Some(handle);
}

/// Tear down the current relay loop and reconnect using freshly-resolved
/// settings (worker URL, credential, identity). Called after a
/// connection-relevant setting changes so a Save takes effect immediately
/// instead of only on the next app restart (reconnect-on-save).
pub async fn reconnect(state: AppState) {
    if let Some(prev) = state.relay_task.lock().await.take() {
        prev.abort();
    }
    {
        let mut status = state.connection_status.lock().await;
        status.session_id = None;
        status.connected = false;
    }
    tracing::info!("Relay settings changed — reconnecting");
    spawn_if_configured(state).await;
}

/// True for settings that the relay connection depends on — changing any of
/// these must tear down and reconnect the relay (reconnect-on-save).
pub fn is_connection_key(key: &str) -> bool {
    matches!(
        key,
        settings::KEY_WORKER_URL
            | settings::KEY_TENANT_ID
            | settings::KEY_USER_ID
            | settings::KEY_ACCESS_TOKEN
            | settings::KEY_REFRESH_TOKEN
    )
}

/// Persist a setting and, if it's connection-relevant, reconnect the relay so
/// the change takes effect immediately. Shared by every set-setting entry point
/// (Tauri command, host GraphQL, relay-RPC) so reconnect-on-save is uniform.
pub async fn apply_setting(state: &AppState, key: String, value: String) -> Result<(), String> {
    settings::set_setting(state, key.clone(), value)?;
    if is_connection_key(&key) {
        reconnect(state.clone()).await;
    }
    Ok(())
}

/// Refresh a short-lived JWT relay credential using the stored refresh token.
///
/// Returns `Ok(true)` when a new token was persisted, `Ok(false)` when there is
/// nothing to refresh — a durable `jk_` API key (never expires) or no refresh
/// token stored. The relay loop re-resolves the credential on its next
/// reconnect, so a successful refresh is picked up automatically.
pub async fn refresh_access_token(state: &AppState) -> Result<bool, String> {
    let token = settings::get_setting_or(state, settings::KEY_ACCESS_TOKEN, "");
    if token.starts_with("jk_") {
        return Ok(false); // durable API key — never expires, nothing to refresh
    }
    let refresh = settings::get_setting_or(state, settings::KEY_REFRESH_TOKEN, "");
    if refresh.trim().is_empty() {
        return Ok(false); // key-only / no refresh token available
    }

    let worker_url = settings::resolve_worker_url(state);
    let graphql_url = format!("{}/graphql", worker_url.trim_end_matches('/'));
    let tenant = settings::get_setting_or(state, settings::KEY_TENANT_ID, settings::DEFAULT_TENANT_ID);

    let body = serde_json::json!({
        "query": "mutation($i: RefreshTokenInput!) { refreshToken(input: $i) { accessToken refreshToken } }",
        "variables": { "i": { "refreshToken": refresh } }
    });

    let resp = reqwest::Client::new()
        .post(&graphql_url)
        .header("content-type", "application/json")
        .header("x-tenant-id", tenant)
        .json(&body)
        .send()
        .await
        .map_err(|e| format!("refresh request failed: {e}"))?;

    let json: serde_json::Value = resp
        .json()
        .await
        .map_err(|e| format!("refresh parse failed: {e}"))?;

    if let Some(errors) = json.get("errors") {
        return Err(format!("refresh rejected: {errors}"));
    }
    let payload = json
        .get("data")
        .and_then(|d| d.get("refreshToken"))
        .ok_or_else(|| "refresh: no refreshToken in response".to_string())?;
    let new_access = payload
        .get("accessToken")
        .and_then(|v| v.as_str())
        .ok_or_else(|| "refresh: missing accessToken".to_string())?;
    let new_refresh = payload
        .get("refreshToken")
        .and_then(|v| v.as_str())
        .unwrap_or(refresh.as_str());

    settings::set_setting(state, settings::KEY_ACCESS_TOKEN.to_string(), new_access.to_string())?;
    settings::set_setting(state, settings::KEY_REFRESH_TOKEN.to_string(), new_refresh.to_string())?;
    {
        let mut guard = state.worker_relay_config.lock().await;
        if let Some(cfg) = guard.as_mut() {
            cfg.access_token = new_access.to_string();
        }
    }
    tracing::info!("Relay access token refreshed");
    Ok(true)
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