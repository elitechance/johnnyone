use johnnyone_desktop_lib::db::Database;
use johnnyone_desktop_lib::host;
use johnnyone_desktop_lib::paths::default_db_path;
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

    let app = host::router(state);
    let listener = tokio::net::TcpListener::bind(&bind_addr)
        .await
        .unwrap_or_else(|e| panic!("Failed to bind {}: {}", bind_addr, e));

    tracing::info!(addr = %bind_addr, "Starting JohnnyOne on-prem host");

    axum::serve(listener, app)
        .await
        .expect("host server failed");
}
