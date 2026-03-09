use crate::db::models::{ProviderConfig, UpsertProviderConfigInput};
use crate::services::providers::{self as provider_service, DetectedTool};
use crate::state::app_state::AppState;
use tauri::State;

#[tauri::command]
pub async fn list_provider_configs(
    state: State<'_, AppState>,
) -> Result<Vec<ProviderConfig>, String> {
    provider_service::list_provider_configs(state.inner())
}

#[tauri::command]
pub async fn upsert_provider_config(
    input: UpsertProviderConfigInput,
    state: State<'_, AppState>,
) -> Result<ProviderConfig, String> {
    provider_service::upsert_provider_config(state.inner(), input)
}

#[tauri::command]
pub async fn delete_provider_config(
    provider: String,
    state: State<'_, AppState>,
) -> Result<(), String> {
    provider_service::delete_provider_config(state.inner(), provider)
}

#[tauri::command]
pub async fn detect_cli_tools(state: State<'_, AppState>) -> Result<Vec<DetectedTool>, String> {
    provider_service::detect_cli_tools(state.inner()).await
}
