use crate::db::models::{CreateSessionInput, Session};
use crate::services::sessions as session_service;
use crate::state::app_state::AppState;
use tauri::State;

#[tauri::command]
pub async fn create_session(
    input: CreateSessionInput,
    state: State<'_, AppState>,
) -> Result<Session, String> {
    session_service::create_session(state.inner(), input)
}

#[tauri::command]
pub async fn list_sessions(
    status: Option<String>,
    state: State<'_, AppState>,
) -> Result<Vec<Session>, String> {
    session_service::list_sessions(state.inner(), status)
}

#[tauri::command]
pub async fn get_session(id: String, state: State<'_, AppState>) -> Result<Session, String> {
    session_service::get_session(state.inner(), id)
}

#[tauri::command]
pub async fn update_session_title(
    id: String,
    title: String,
    state: State<'_, AppState>,
) -> Result<Session, String> {
    session_service::update_session_title(state.inner(), id, title)
}

#[tauri::command]
pub async fn update_session_working_directory(
    id: String,
    working_directory: String,
    state: State<'_, AppState>,
) -> Result<Session, String> {
    session_service::update_session_working_directory(state.inner(), id, working_directory)
}

#[tauri::command]
pub async fn update_session_provider(
    id: String,
    provider: String,
    state: State<'_, AppState>,
) -> Result<Session, String> {
    session_service::update_session_provider(state.inner(), id, provider)
}

#[tauri::command]
pub async fn archive_session(id: String, state: State<'_, AppState>) -> Result<Session, String> {
    session_service::archive_session(state.inner(), id).await
}

#[tauri::command]
pub async fn delete_session(id: String, state: State<'_, AppState>) -> Result<bool, String> {
    session_service::delete_session(state.inner(), id).await
}
