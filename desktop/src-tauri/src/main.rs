// Prevents additional console window on Windows in release builds
#![cfg_attr(not(debug_assertions), windows_subsystem = "windows")]

use johnnyone_desktop_lib::commands;
use johnnyone_desktop_lib::db::Database;
use johnnyone_desktop_lib::host;
use johnnyone_desktop_lib::paths::default_db_path;
use johnnyone_desktop_lib::services::agent_plans;
use johnnyone_desktop_lib::services::agent_reaper;
use johnnyone_desktop_lib::services::brief_store;
use johnnyone_desktop_lib::services::provider_preflight;
use johnnyone_desktop_lib::services::question_watch;
use johnnyone_desktop_lib::services::relay;
use johnnyone_desktop_lib::services::session_watchdog;
use johnnyone_desktop_lib::state::app_state::AppState;
use johnnyone_desktop_lib::terminal;
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
            // The engine's boot instant, captured FIRST — before any coordinator loop resumes — so
            // the orphan sweep's stale-ephemeral rule never mistakes a lens spawned after boot for
            // one left over from the previous engine.
            let engine_started_unix = std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .map(|d| d.as_secs() as i64)
                .unwrap_or(0);

            // Background work that the old standalone `johnnyone-host` binary
            // used to do — now folded into the desktop app so users only run
            // one binary.
            //
            // Two things start here (both on Tauri's tokio runtime):
            //   1) Resume any in-flight planner coordinator loops from SQLite.
            //   2) Reconcile `terminal_status` against the tmux sessions that
            //      actually exist, THEN (if relay settings are configured in
            //      SQLite, with optional JOHNNYONE_* env overrides) register
            //      this machine with the worker and keep a WebSocket open for
            //      relay-RPC.
            //   3) The orphan sweep (`agent_reaper`): at boot and every 10 min, retire
            //      `johnnyone_<uuid>` panes no running job owns (never foreign names).
            //   4) Local axum GraphQL listener on 127.0.0.1:7788 — what the
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

            let agent_state = app_state.clone();
            tauri::async_runtime::spawn(async move {
                // Reconcile BEFORE the relay registers, awaited in the same task so the ordering is
                // structural rather than a race between two spawns: a client that connects the
                // instant we register must not be served pre-reconciliation `terminal_status`.
                //
                // A host restart kills every `johnnyone_*` pane without running either of the two
                // paths that write 'detached' (kill_terminal_session, the capture-loop failure
                // break), so without this pass rows claim 'attached' forever.
                match terminal::reconcile_terminal_status_on_startup(&agent_state).await {
                    Ok(_) => {}
                    Err(error) => tracing::warn!(
                        %error,
                        "Failed to reconcile terminal_status at startup"
                    ),
                }
                relay::spawn_if_configured(agent_state).await;
            });

            // Poll the host's own agent panes for a question the agent is stuck
            // at, and tell the console about it once per ask. Like the two above,
            // this is a long-lived task on Tauri's tokio runtime, and it takes an
            // `AppState` BY VALUE exactly as `relay::spawn_if_configured` does -
            // the struct is internally `Arc`'d, so the clone is cheap and the task
            // shares the one state.
            let question_state = app_state.clone();
            question_watch::spawn_question_watch(question_state);

            // Session watchdog loop — a single 20s ticker for all sessions, checks per-session
            // config and delivers nudges through the unified chat seam. Like `question_watch`,
            // this is a long-lived task on Tauri's tokio runtime.
            let watchdog_state = app_state.clone();
            session_watchdog::spawn_session_watchdog(watchdog_state);

            // Orphan sweep — a boot pass, then every 10 minutes: retire `johnnyone_<uuid>` panes
            // whose row is archived/gone, whose plan finished, or that an engine restart left
            // behind. Every row-backed kill archives the row and writes `agent_session_retired`.
            // Like the watchdog, a long-lived task on Tauri's tokio runtime.
            agent_reaper::spawn_orphan_sweep(app_state.clone(), engine_started_unix);

            // Plans tab (00/08): every 60 s, start each queued briefing whose conflicting
            // initiatives have finished (`brief_store::release_queued_briefs` → `accept_brief`).
            // Isolated from the planning/development loop by design.
            brief_store::spawn_planning_queue_release(app_state.clone());

            // One read of the provider's model list, compared with the seat caps: a cap for a
            // family the provider will not serve is a pool that can be leased and never used.
            // It now gets the LIVE pool rather than a default caps table, so the comparison is
            // made against the caps this host actually enforces (`seat_caps_from_db`) and the
            // answer is told to the pool, not just to the log — `acquire` then refuses a family
            // the provider will not serve. A hiccup at boot leaves the pool untouched.
            provider_preflight::spawn_provider_preflight(app_state.lease_pool.clone());

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
