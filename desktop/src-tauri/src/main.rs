// Prevents additional console window on Windows in release builds
#![cfg_attr(not(debug_assertions), windows_subsystem = "windows")]

use johnnyone_desktop_lib::agent::{AgentConfig, AgentService};
use johnnyone_desktop_lib::commands;
use johnnyone_desktop_lib::db::Database;
use johnnyone_desktop_lib::host;
use johnnyone_desktop_lib::paths::default_db_path;
use johnnyone_desktop_lib::services::agent_plans;
use johnnyone_desktop_lib::state::app_state::{AppState, WorkerRelayConfig};
use tracing_subscriber::EnvFilter;

fn main() {
    // Initialize tracing/logging
    tracing_subscriber::fmt()
        .with_env_filter(
            EnvFilter::try_from_default_env()
                .unwrap_or_else(|_| EnvFilter::new("johnnyone_desktop_lib=debug,info")),
        )
        .init();

    tracing::info!("Starting JohnnyOne Desktop");

    // Initialize SQLite database
    let db_path = default_db_path();
    let db = Database::open(&db_path).expect("Failed to initialize database");
    let app_state = AppState::new(db);

    tauri::Builder::default()
        .plugin(tauri_plugin_shell::init())
        .plugin(tauri_plugin_dialog::init())
        .manage(app_state.clone())
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
        .setup(move |_app| {
            // Background work that the old standalone `johnnyone-host` binary
            // used to do — now folded into the desktop app so users only run
            // one binary.
            //
            // Two things start here (both on Tauri's tokio runtime):
            //   1) Resume any in-flight planner coordinator loops from SQLite.
            //   2) If `JOHNNYONE_WORKER_URL` is set, register this machine
            //      with the deployed worker and keep a WebSocket open for
            //      relay-RPC (so the hosted web client can talk to this host).
            //   3) Local axum GraphQL listener on 127.0.0.1:7788 — what the
            //      embedded host-app Angular UI fetches against. (Will move
            //      to Tauri `invoke()` later; this is the minimal port-over.)
            let state_for_resume = app_state.clone();
            tauri::async_runtime::spawn(async move {
                match agent_plans::resume_active_plan_loops(state_for_resume).await {
                    Ok(count) if count > 0 => {
                        tracing::info!(count, "Resumed active planner coordinator loops")
                    }
                    Ok(_) => {}
                    Err(error) => {
                        tracing::warn!(%error, "Failed to resume active planner coordinator loops")
                    }
                }
            });

            if let Ok(worker_url) = std::env::var("JOHNNYONE_WORKER_URL") {
                let user_id = std::env::var("JOHNNYONE_USER_ID").unwrap_or_else(|_| {
                    "00000000-0000-0000-0000-000000000002".to_string()
                });
                let tenant_id = std::env::var("JOHNNYONE_TENANT_ID").unwrap_or_else(|_| {
                    "00000000-0000-0000-0000-000000000001".to_string()
                });

                let agent_state = app_state.clone();
                tauri::async_runtime::spawn(async move {
                    agent_state
                        .set_worker_relay_config(WorkerRelayConfig {
                            worker_url: worker_url.clone(),
                            user_id: user_id.clone(),
                            tenant_id: tenant_id.clone(),
                        })
                        .await;
                    {
                        let mut status = agent_state.connection_status.lock().await;
                        status.session_id = Some("host-agent".to_string());
                    }

                    let config = AgentConfig {
                        worker_url,
                        user_id,
                        tenant_id,
                    };
                    loop {
                        match AgentService::start(
                            AgentConfig {
                                worker_url: config.worker_url.clone(),
                                user_id: config.user_id.clone(),
                                tenant_id: config.tenant_id.clone(),
                            },
                            agent_state.clone(),
                        )
                        .await
                        {
                            Ok(()) => break,
                            Err(error) => {
                                tracing::error!(
                                    %error,
                                    "Backend relay connection failed; retrying in 2s"
                                );
                                tokio::time::sleep(std::time::Duration::from_secs(2)).await;
                            }
                        }
                    }
                });
            } else {
                tracing::info!(
                    "JOHNNYONE_WORKER_URL not set — running fully offline (no relay)"
                );
            }

            let bind_addr = std::env::var("JOHNNYONE_HOST_ADDR")
                .unwrap_or_else(|_| "127.0.0.1:7788".to_string());
            let axum_state = app_state.clone();
            tauri::async_runtime::spawn(async move {
                let app = host::router(axum_state);
                let listener = match tokio::net::TcpListener::bind(&bind_addr).await {
                    Ok(l) => l,
                    Err(e) => {
                        tracing::error!(addr = %bind_addr, %e, "Failed to bind embedded host listener");
                        return;
                    }
                };
                tracing::info!(addr = %bind_addr, "Embedded host GraphQL listener up");
                if let Err(e) = axum::serve(listener, app).await {
                    tracing::error!(%e, "Embedded host listener exited");
                }
            });

            Ok(())
        })
        .run(tauri::generate_context!())
        .expect("error while running JohnnyOne");
}
