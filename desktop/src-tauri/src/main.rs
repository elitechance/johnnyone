// Prevents additional console window on Windows in release builds
#![cfg_attr(not(debug_assertions), windows_subsystem = "windows")]

use johnnyone_desktop_lib::commands;
use johnnyone_desktop_lib::db::Database;
use johnnyone_desktop_lib::paths::default_db_path;
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

    // Initialize SQLite database
    let db_path = default_db_path();
    let db = Database::open(&db_path).expect("Failed to initialize database");

    let app_state = AppState::new(db);

    tauri::Builder::default()
        .plugin(tauri_plugin_shell::init())
        .plugin(tauri_plugin_dialog::init())
        .manage(app_state)
        .invoke_handler(tauri::generate_handler![
            // Agent session (worker connection)
            commands::agent_session::start_agent,
            commands::agent_session::stop_agent,
            commands::agent_session::get_connection_status,
            // Tool status
            commands::tool_status::get_tool_executions,
            commands::tool_status::get_pending_approvals,
            commands::tool_status::approve_tool,
            commands::tool_status::deny_tool,
            // Sessions
            commands::sessions::create_session,
            commands::sessions::list_sessions,
            commands::sessions::get_session,
            commands::sessions::update_session_title,
            commands::sessions::update_session_working_directory,
            commands::sessions::update_session_provider,
            commands::sessions::archive_session,
            commands::sessions::delete_session,
            // Chat
            commands::chat::send_chat_message,
            commands::chat::stop_generation,
            commands::chat::list_messages,
            // Terminal
            commands::terminal::attach_terminal_session,
            commands::terminal::send_terminal_input,
            commands::terminal::resize_terminal,
            // Clipboard
            commands::clipboard::read_clipboard_image_data_url,
            // Providers
            commands::providers::list_provider_configs,
            commands::providers::upsert_provider_config,
            commands::providers::delete_provider_config,
            commands::providers::detect_cli_tools,
            // Settings
            commands::settings::get_setting,
            commands::settings::set_setting,
        ])
        .run(tauri::generate_context!())
        .expect("error while running JohnnyOne");
}
