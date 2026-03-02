// Prevents additional console window on Windows in release builds
#![cfg_attr(not(debug_assertions), windows_subsystem = "windows")]

use johnnyone_desktop_lib::commands;
use johnnyone_desktop_lib::state::app_state::AppState;
use tracing_subscriber::EnvFilter;

fn main() {
    // Initialize tracing/logging
    tracing_subscriber::fmt()
        .with_env_filter(
            EnvFilter::try_from_default_env()
                .unwrap_or_else(|_| EnvFilter::new("johnnyone_desktop_lib=debug,info")),
        )
        .init();

    tracing::info!("Starting JohnnyOne Desktop Agent");

    let app_state = AppState::new();

    tauri::Builder::default()
        .plugin(tauri_plugin_shell::init())
        .manage(app_state)
        .invoke_handler(tauri::generate_handler![
            commands::agent_session::start_agent,
            commands::agent_session::stop_agent,
            commands::agent_session::get_connection_status,
            commands::tool_status::get_tool_executions,
            commands::tool_status::get_pending_approvals,
            commands::tool_status::approve_tool,
            commands::tool_status::deny_tool,
        ])
        .run(tauri::generate_context!())
        .expect("error while running JohnnyOne");
}
