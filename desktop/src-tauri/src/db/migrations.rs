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
const MIGRATION_017: &str = include_str!("../../migrations/017_add_initiative_axes.sql");
/// Backfill for migration 017. `pub` so the unit test can run the *shipped* SQL verbatim
/// (single source of truth — the test can never drift from what the migration applies).
pub const BACKFILL_017: &str = include_str!("../../migrations/017_backfill_initiative_axes.sql");

/// Map an execution `status` to the initiative `health` axis. The SQL health seeding in
/// `BACKFILL_017` and this fn must agree (pinned by `health_from_status_maps_all_axes`).
pub fn health_from_status(status: &str) -> &'static str {
    match status {
        "blocked" => "blocked",
        "needs_attention" | "phase_needs_changes" => "needs-attention",
        _ => "in-progress",
    }
}

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
        (17, "017_add_initiative_axes", MIGRATION_017),
    ];

    for (version, name, sql) in migrations {
        if !applied.contains(&version) {
            tracing::info!(version = version, name = name, "Applying migration");
            conn.execute_batch(sql)
                .map_err(|e| format!("Migration {} failed: {}", name, e))?;
            // Version 17 ships its backfill as a separately-testable artifact (BACKFILL_017);
            // apply it right after the DDL, before recording the version. The explicit
            // special-case keeps the backfill a shared source of truth with its unit test
            // without changing the migrations-vec element type.
            if version == 17 {
                conn.execute_batch(BACKFILL_017)
                    .map_err(|e| format!("Migration {} backfill failed: {}", name, e))?;
            }
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

#[cfg(test)]
mod tests {
    use super::*;
    use rusqlite::{params, Connection};

    /// Insert a legacy-shaped agent_plans row. `run_type`/`status` are bound; the three new
    /// Initiative columns are forced to their pre-backfill state (`initiative_id`/
    /// `initiative_status` NULL, `health` left at its column DEFAULT) so `BACKFILL_017` has work.
    fn insert_legacy_row(conn: &Connection, id: &str, run_type: &str, status: &str) {
        conn.execute(
            "INSERT INTO agent_plans
                (id, run_type, title, workspace_path, plan_path, status,
                 worker_provider, reviewer_provider, initiative_id, initiative_status)
             VALUES (?1, ?2, 't', '/w', '/p', ?3, 'claude_code', 'claude_code', NULL, NULL)",
            params![id, run_type, status],
        )
        .unwrap();
    }

    /// Insert a legacy row whose `run_type` is NULL. The shipped schema declares
    /// `run_type TEXT NOT NULL DEFAULT 'development'` (migration 006), so a plain INSERT of NULL
    /// is rejected. We transiently relax *only* that constraint via `writable_schema` to
    /// reproduce the pathological pre-006 shape the defensive `IS NULL` sweep exists to catch,
    /// then run the real `BACKFILL_017` against it. This keeps the shipped SQL the single source
    /// of truth while still exercising the branch that a normal INSERT cannot reach.
    fn insert_null_run_type_row(conn: &Connection, id: &str, status: &str) {
        let create_sql: String = conn
            .query_row(
                "SELECT sql FROM sqlite_master WHERE type='table' AND name='agent_plans'",
                [],
                |row| row.get(0),
            )
            .unwrap();
        let relaxed = create_sql.replace("run_type TEXT NOT NULL DEFAULT 'development'", "run_type TEXT");
        assert_ne!(
            relaxed, create_sql,
            "expected the run_type NOT NULL clause to be present so it can be relaxed"
        );
        let schema_version: i64 = conn
            .query_row("PRAGMA schema_version", [], |row| row.get(0))
            .unwrap();
        conn.execute_batch("PRAGMA writable_schema=ON;").unwrap();
        conn.execute(
            "UPDATE sqlite_master SET sql=?1 WHERE type='table' AND name='agent_plans'",
            params![relaxed],
        )
        .unwrap();
        // Bump the schema cookie so the connection reparses the relaxed definition.
        conn.execute_batch(&format!("PRAGMA schema_version={};", schema_version + 1))
            .unwrap();
        conn.execute_batch("PRAGMA writable_schema=OFF;").unwrap();
        conn.execute(
            "INSERT INTO agent_plans
                (id, run_type, title, workspace_path, plan_path, status,
                 worker_provider, reviewer_provider, initiative_id, initiative_status)
             VALUES (?1, NULL, 't', '/w', '/p', ?2, 'claude_code', 'claude_code', NULL, NULL)",
            params![id, status],
        )
        .unwrap();
    }

    fn column_names(conn: &Connection, table: &str) -> Vec<String> {
        let mut stmt = conn
            .prepare(&format!("PRAGMA table_info({})", table))
            .unwrap();
        stmt.query_map([], |row| row.get::<_, String>(1))
            .unwrap()
            .map(|r| r.unwrap())
            .collect()
    }

    #[test]
    fn migrations_are_idempotent_and_add_initiative_columns() {
        let conn = Connection::open_in_memory().unwrap();
        run_migrations(&conn).expect("first run_migrations");
        // Re-running must be a no-op, not a "duplicate column" error.
        run_migrations(&conn).expect("second run_migrations (idempotent)");

        let cols = column_names(&conn, "agent_plans");
        for expected in ["initiative_id", "initiative_status", "health"] {
            assert!(
                cols.iter().any(|c| c == expected),
                "agent_plans missing column {} (have: {:?})",
                expected,
                cols
            );
        }
    }

    #[test]
    fn backfill_017_maps_legacy_rows() {
        let conn = Connection::open_in_memory().unwrap();
        run_migrations(&conn).expect("run_migrations");

        // Legacy-shaped fixtures (new columns forced back to their pre-backfill state).
        insert_legacy_row(&conn, "r1", "planning", "blocked");
        insert_legacy_row(&conn, "r2", "development", "approved");
        insert_null_run_type_row(&conn, "r3", "draft");
        insert_legacy_row(&conn, "r4", "planning", "needs_attention");
        insert_legacy_row(&conn, "r5", "development", "phase_needs_changes");

        // Run the *shipped* backfill verbatim — not a copy.
        conn.execute_batch(BACKFILL_017).unwrap();

        let read = |id: &str| -> (Option<String>, Option<String>, String) {
            conn.query_row(
                "SELECT initiative_id, initiative_status, health FROM agent_plans WHERE id=?1",
                params![id],
                |row| Ok((row.get(0)?, row.get(1)?, row.get(2)?)),
            )
            .unwrap()
        };

        // R1: planning + blocked.
        let (iid, ist, health) = read("r1");
        assert_eq!(iid.as_deref(), Some("r1"));
        assert_eq!(ist.as_deref(), Some("planning"));
        assert_eq!(health, "blocked");

        // R2: development + default health.
        let (_, ist, health) = read("r2");
        assert_eq!(ist.as_deref(), Some("development"));
        assert_eq!(health, "in-progress");

        // R3: NULL run_type caught by the defensive IS NULL sweep.
        let (iid, ist, health) = read("r3");
        assert_eq!(iid.as_deref(), Some("r3"));
        assert_eq!(ist.as_deref(), Some("development"));
        assert_eq!(health, "in-progress");

        // R4: needs_attention → needs-attention (first IN value).
        let (_, ist, health) = read("r4");
        assert_eq!(ist.as_deref(), Some("planning"));
        assert_eq!(health, "needs-attention");

        // R5: phase_needs_changes → needs-attention (second IN value).
        let (_, ist, health) = read("r5");
        assert_eq!(ist.as_deref(), Some("development"));
        assert_eq!(health, "needs-attention");
    }

    #[test]
    fn health_from_status_maps_all_axes() {
        assert_eq!(health_from_status("blocked"), "blocked");
        assert_eq!(health_from_status("needs_attention"), "needs-attention");
        assert_eq!(health_from_status("phase_needs_changes"), "needs-attention");
        assert_eq!(health_from_status("draft"), "in-progress");
        assert_eq!(health_from_status("approved"), "in-progress");
    }
}
