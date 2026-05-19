use rusqlite::Connection;

const MIGRATION_001: &str = include_str!("../../migrations/001_initial.sql");
const MIGRATION_002: &str = include_str!("../../migrations/002_add_cli_session_id.sql");
const MIGRATION_003: &str = include_str!("../../migrations/003_add_worker_settings.sql");
const MIGRATION_004: &str = include_str!("../../migrations/004_add_tmux_terminal.sql");

/// Run all pending migrations. Uses a simple version table to track applied migrations.
pub fn run_migrations(conn: &Connection) -> Result<(), String> {
    // Create the migration tracking table
    conn.execute_batch(
        "CREATE TABLE IF NOT EXISTS _migrations (
            version INTEGER PRIMARY KEY,
            name TEXT NOT NULL,
            applied_at TEXT NOT NULL DEFAULT (datetime('now'))
        );",
    )
    .map_err(|e| format!("Failed to create migrations table: {}", e))?;

    let applied: Vec<i64> = {
        let mut stmt = conn
            .prepare("SELECT version FROM _migrations ORDER BY version")
            .map_err(|e| format!("Failed to query migrations: {}", e))?;
        let result = stmt
            .query_map([], |row| row.get(0))
            .map_err(|e| format!("Failed to read migrations: {}", e))?
            .filter_map(|r| r.ok())
            .collect();
        result
    };

    let migrations: Vec<(i64, &str, &str)> = vec![
        (1, "001_initial", MIGRATION_001),
        (2, "002_add_cli_session_id", MIGRATION_002),
        (3, "003_add_worker_settings", MIGRATION_003),
        (4, "004_add_tmux_terminal", MIGRATION_004),
    ];

    for (version, name, sql) in migrations {
        if !applied.contains(&version) {
            tracing::info!(version = version, name = name, "Applying migration");
            conn.execute_batch(sql)
                .map_err(|e| format!("Migration {} failed: {}", name, e))?;
            conn.execute(
                "INSERT INTO _migrations (version, name) VALUES (?1, ?2)",
                rusqlite::params![version, name],
            )
            .map_err(|e| format!("Failed to record migration {}: {}", name, e))?;
        }
    }

    tracing::info!("All migrations applied");
    Ok(())
}
