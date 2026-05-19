use crate::state::app_state::AppState;
use crate::terminal::{self, TerminalSnapshot};
use serde::Deserialize;
use tauri::State;

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct AttachTerminalInput {
    pub session_id: String,
    pub cols: Option<u16>,
    pub rows: Option<u16>,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct TerminalInput {
    pub session_id: String,
    pub data: String,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ResizeTerminalInput {
    pub session_id: String,
    pub cols: u16,
    pub rows: u16,
}

#[tauri::command]
pub async fn attach_terminal_session(
    input: AttachTerminalInput,
    app_handle: tauri::AppHandle,
    state: State<'_, AppState>,
) -> Result<TerminalSnapshot, String> {
    terminal::attach_terminal(
        state.inner(),
        app_handle,
        input.session_id,
        input.cols.unwrap_or(100),
        input.rows.unwrap_or(30),
    )
    .await
}

#[tauri::command]
pub async fn send_terminal_input(
    input: TerminalInput,
    state: State<'_, AppState>,
) -> Result<(), String> {
    terminal::send_terminal_input(state.inner(), input.session_id, input.data).await
}

#[tauri::command]
pub async fn resize_terminal(
    input: ResizeTerminalInput,
    state: State<'_, AppState>,
) -> Result<(), String> {
    terminal::resize_terminal(state.inner(), input.session_id, input.cols, input.rows).await
}
