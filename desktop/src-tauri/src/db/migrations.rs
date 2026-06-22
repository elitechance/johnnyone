use rusqlite::Connection;

const MIGRATION_001: &str = include_str!("../../migrations/001_initial.sql");
const MIGRATION_002: &str = include_str!("../../migrations/002_add_cli_session_id.sql");
const MIGRATION_003: &str = include_str!("../../migrations/003_add_worker_settings.sql");
const MIGRATION_004: &str = include_str!("../../migrations/004_add_tmux_terminal.sql");
const MIGRATION_005: &str = include_str!("../../migrations/005_add_agent_plans.sql");
const MIGRATION_006: &str = include_str!("../../migrations/006_workspace_modes.sql");
const MIGRATION_007: &str = include_str!("../../migrations/007_add_amend_brief.sql");
const MIGRATION_008: &str = include_str!("../../migrations/008_session_kind.sql");
const MIGRATION_009: &str = include_str!("../../migrations/009_agent_plan_phase_run_mode.sql");
const MIGRATION_010: &str = include_str!("../../migrations/010_host_planner_settings.sql");
const MIGRATION_011: &str = include_str!("../../migrations/011_worker_url_without_dev.sql");
const MIGRATION_012: &str = include_str!("../../migrations/012_worker_url_prod_naming.sql");
const MIGRATION_013: &str = include_str!("../../migrations/013_agent_plan_review_sessions.sql");
const MIGRATION_014: &str = include_str!("../../migrations/014_add_session_setup_commands.sql");
const MIGRATION_015: &str = include_str!("../../migrations/015_add_reviewer_setup_commands.sql");
const MIGRATION_016: &str = include_str!("../../migrations/016_add_attached_tmux.sql");

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
        (5, "005_add_agent_plans", MIGRATION_005),
        (6, "006_workspace_modes", MIGRATION_006),
        (7, "007_add_amend_brief", MIGRATION_007),
        (8, "008_session_kind", MIGRATION_008),
        (9, "009_agent_plan_phase_run_mode", MIGRATION_009),
        (10, "010_host_planner_settings", MIGRATION_010),
        (11, "011_worker_url_without_dev", MIGRATION_011),
        (12, "012_worker_url_prod_naming", MIGRATION_012),
        (13, "013_agent_plan_review_sessions", MIGRATION_013),
        (14, "014_add_session_setup_commands", MIGRATION_014),
        (15, "015_add_reviewer_setup_commands", MIGRATION_015),
        (16, "016_add_attached_tmux", MIGRATION_016),
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
