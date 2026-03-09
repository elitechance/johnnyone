use crate::services::settings as settings_service;
use crate::state::app_state::AppState;
use tauri::State;

#[tauri::command]
pub async fn get_setting(key: String, state: State<'_, AppState>) -> Result<String, String> {
    settings_service::get_setting(state.inner(), key)
}

#[tauri::command]
pub async fn set_setting(
    key: String,
    value: String,
    state: State<'_, AppState>,
) -> Result<(), String> {
    settings_service::set_setting(state.inner(), key, value)
}
