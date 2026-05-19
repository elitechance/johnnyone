use johnnyone_desktop_lib::db::Database;
use johnnyone_desktop_lib::host;
use johnnyone_desktop_lib::paths::default_db_path;
use johnnyone_desktop_lib::agent::{AgentConfig, AgentService};
use johnnyone_desktop_lib::state::app_state::AppState;
use tracing_subscriber::EnvFilter;

#[tokio::main]
async fn main() {
    tracing_subscriber::fmt()
        .with_env_filter(
            EnvFilter::try_from_default_env()
                .unwrap_or_else(|_| EnvFilter::new("johnnyone_desktop_lib=debug,info")),
        )
        .init();

    let bind_addr =
        std::env::var("JOHNNYONE_HOST_ADDR").unwrap_or_else(|_| "0.0.0.0:7788".to_string());

    let db_path = default_db_path();
    let db = Database::open(&db_path).expect("Failed to initialize database");
    let state = AppState::new(db);

    if let Ok(worker_url) = std::env::var("JOHNNYONE_WORKER_URL") {
        let user_id = std::env::var("JOHNNYONE_USER_ID")
            .unwrap_or_else(|_| "00000000-0000-0000-0000-000000000002".to_string());
        let tenant_id = std::env::var("JOHNNYONE_TENANT_ID")
            .unwrap_or_else(|_| "00000000-0000-0000-0000-000000000001".to_string());
        let agent_state = state.clone();
        {
            let mut status = state.connection_status.lock().await;
            status.session_id = Some("host-agent".to_string());
        }

        tokio::spawn(async move {
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
                        tracing::error!(%error, "Backend relay connection failed; retrying");
                        tokio::time::sleep(std::time::Duration::from_secs(2)).await;
                    }
                }
            }
        });
    }

    let app = host::router(state);
    let listener = tokio::net::TcpListener::bind(&bind_addr)
        .await
        .unwrap_or_else(|e| panic!("Failed to bind {}: {}", bind_addr, e));

    tracing::info!(addr = %bind_addr, "Starting JohnnyOne on-prem host");

    axum::serve(listener, app)
        .await
        .expect("host server failed");
}
