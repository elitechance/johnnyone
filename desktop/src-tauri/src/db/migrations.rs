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
const MIGRATION_018: &str = include_str!("../../migrations/018_add_briefing_session.sql");
const MIGRATION_019: &str = include_str!("../../migrations/019_add_validation_config.sql");
const MIGRATION_020: &str = include_str!("../../migrations/020_complete_approved_dev_runs.sql");
// 021 landed on `feat/local-small-executor`; 022/023 on `main`. Both were applied to live DBs
// under these exact numbers (verified against `_migrations` at merge time), so the union below is
// what the running databases already reflect. Versions are a shared, hand-assigned namespace across
// branches — the runner only checks whether the *number* was applied, so never reuse one.
const MIGRATION_021: &str = include_str!("../../migrations/021_add_executor_config.sql");
const MIGRATION_022: &str = include_str!("../../migrations/022_add_dev_stage_providers.sql");
const MIGRATION_023: &str = include_str!("../../migrations/023_add_test_commands.sql");
const MIGRATION_024: &str = include_str!("../../migrations/024_add_session_reports.sql");
const MIGRATION_025: &str = include_str!("../../migrations/025_add_phase_test_commands.sql");
const MIGRATION_026: &str = include_str!("../../migrations/026_add_workflow_arena.sql");
const MIGRATION_027: &str = include_str!("../../migrations/027_add_session_watchdog.sql");
const MIGRATION_028: &str = include_str!("../../migrations/028_add_watchdog_held_count.sql");
const MIGRATION_029: &str = include_str!("../../migrations/029_add_watchdog_triggers.sql");
const MIGRATION_030: &str = include_str!("../../migrations/030_add_plan_findings.sql");
const MIGRATION_031: &str = include_str!("../../migrations/031_add_brief_versions.sql");
const MIGRATION_032: &str = include_str!("../../migrations/032_findings_dispute_red_first.sql");

/// Map an execution `status` to the initiative `health` axis. The SQL health seeding in
/// `BACKFILL_017` and this fn must agree (pinned by `health_from_status_maps_all_axes`).
pub fn health_from_status(status: &str) -> &'static str {
    match status {
        "blocked" => "blocked",
        "needs_attention" | "phase_needs_changes" | "phase_replan_running" => "needs-attention",
        // Terminal, healthy end states: a fully-approved/complete run is DONE, not
        // "in-progress". Without this, the HEALTH axis is stuck on in-progress forever
        // after approval (the console reads it straight from this derivation).
        "approved" | "done" | "complete" | "completed" | "passed" => "complete",
        _ => "in-progress",
    }
}

/// Run all pending migrations. Uses a simple version table to track applied migrations.
pub fn run_migrations(conn: &Connection) -> Result<(), String> {
    run_migrations_through(conn, i64::MAX)
}

/// Apply pending migrations whose version is `<= max_version`. Production always passes
/// `i64::MAX` (via [`run_migrations`]); tests pass a lower bound to build a DB exactly as an
/// older release left it, then upgrade it (e.g. `migration_031_applies_on_030_db`).
fn run_migrations_through(conn: &Connection, max_version: i64) -> Result<(), String> {
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
        (18, "018_add_briefing_session", MIGRATION_018),
        (19, "019_add_validation_config", MIGRATION_019),
        (20, "020_complete_approved_dev_runs", MIGRATION_020),
        (21, "021_add_executor_config", MIGRATION_021),
        (22, "022_add_dev_stage_providers", MIGRATION_022),
        (23, "023_add_test_commands", MIGRATION_023),
        (24, "024_add_session_reports", MIGRATION_024),
        (25, "025_add_phase_test_commands", MIGRATION_025),
        (26, "026_add_workflow_arena", MIGRATION_026),
        (27, "027_add_session_watchdog", MIGRATION_027),
        (28, "028_add_watchdog_held_count", MIGRATION_028),
        (29, "029_add_watchdog_triggers", MIGRATION_029),
        (30, "030_add_plan_findings", MIGRATION_030),
        (31, "031_add_brief_versions", MIGRATION_031),
        (32, "032_findings_dispute_red_first", MIGRATION_032),
    ];

    // Guard the shared version namespace. Versions are hand-assigned integers, and branches that
    // develop in parallel can each claim the same number (021 was claimed twice — see the NOTE above).
    // The applied-set check below keys on the NUMBER alone, so a DB that already ran someone else's
    // migration N silently skips ours, leaving the schema short of columns the code then assumes.
    // Detect the mismatch instead of limping on: same number, different name = stop.
    let applied_names: std::collections::HashMap<i64, String> = {
        let mut stmt = conn
            .prepare("SELECT version, name FROM _migrations")
            .map_err(|e| e.to_string())?;
        let rows = stmt
            .query_map([], |row| Ok((row.get::<_, i64>(0)?, row.get::<_, String>(1)?)))
            .map_err(|e| e.to_string())?;
        rows.filter_map(|r| r.ok()).collect()
    };
    for (version, name, _) in &migrations {
        if let Some(existing) = applied_names.get(version) {
            if existing != name {
                return Err(format!(
                    "Migration version {} is already applied as '{}', but this build expects '{}'.                      Two branches claimed the same version number. Renumber the newer migration                      rather than reusing the slot — reusing it would silently skip the schema change.",
                    version, existing, name
                ));
            }
        }
    }

    for (version, name, sql) in migrations {
        if version > max_version {
            continue;
        }
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
    fn migration_030_creates_plan_findings() {
        let conn = Connection::open_in_memory().unwrap();
        conn.execute_batch("PRAGMA foreign_keys=ON;").unwrap();
        run_migrations_through(&conn, 30).expect("first run_migrations");
        run_migrations_through(&conn, 30).expect("second run_migrations (idempotent)");

        assert_eq!(
            column_names(&conn, "plan_findings"),
            vec![
                "plan_id", "id", "phase_id", "lens", "severity", "state", "round_opened",
                "round_updated", "claim", "expected", "saw", "where_", "fence", "evidence",
                "waive_reason", "reopen_count", "last_saw", "created_at", "updated_at",
            ]
        );
        let index_count: i64 = conn
            .query_row(
                "SELECT COUNT(*) FROM sqlite_master WHERE type='index' AND name='idx_plan_findings_phase'",
                [],
                |row| row.get(0),
            )
            .unwrap();
        assert_eq!(index_count, 1, "idx_plan_findings_phase missing");

        insert_legacy_row(&conn, "p1", "development", "phase_review_running");
        let insert = |id: &str, severity: &str, state: &str| {
            conn.execute(
                "INSERT INTO plan_findings
                    (plan_id, id, phase_id, lens, severity, state, round_opened, round_updated, claim)
                 VALUES ('p1', ?1, '00-x', 'qa', ?2, ?3, 1, 1, 'c')",
                params![id, severity, state],
            )
        };
        insert("F-00-x-1", "blocking", "open").expect("valid row inserts");
        assert!(insert("F-00-x-2", "blocking", "bogus").is_err(), "state CHECK must reject 'bogus'");
        assert!(insert("F-00-x-3", "nit", "open").is_err(), "severity CHECK must reject 'nit'");
        assert!(insert("F-00-x-1", "followup", "open").is_err(), "duplicate (plan_id,id) must fail");

        let (reopen_count, expected, where_): (i64, String, String) = conn
            .query_row(
                "SELECT reopen_count, expected, where_ FROM plan_findings WHERE plan_id='p1' AND id='F-00-x-1'",
                [],
                |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?)),
            )
            .unwrap();
        assert_eq!((reopen_count, expected.as_str(), where_.as_str()), (0, "", ""));

        conn.execute("DELETE FROM agent_plans WHERE id='p1'", []).unwrap();
        let left: i64 = conn
            .query_row("SELECT COUNT(*) FROM plan_findings", [], |r| r.get(0))
            .unwrap();
        assert_eq!(left, 0, "ON DELETE CASCADE must remove the plan's findings");
    }

    #[test]
    fn migrations_are_idempotent_and_add_initiative_columns() {
        let conn = Connection::open_in_memory().unwrap();
        run_migrations(&conn).expect("first run_migrations");
        // Re-running must be a no-op, not a "duplicate column" error.
        run_migrations(&conn).expect("second run_migrations (idempotent)");

        let cols = column_names(&conn, "agent_plans");
        for expected in [
            "initiative_id",
            "initiative_status",
            "health",
            "briefing_session_id",
        ] {
            assert!(
                cols.iter().any(|c| c == expected),
                "agent_plans missing column {} (have: {:?})",
                expected,
                cols
            );
        }
    }

    #[test]
    fn migration_019_adds_validation_config_column_idempotently() {
        // Overhaul P7 phase 01: the nullable JSON validation_config column lands on agent_plans
        // and re-running migrations is a no-op (not a "duplicate column" error).
        let conn = Connection::open_in_memory().unwrap();
        run_migrations(&conn).expect("first run_migrations");
        run_migrations(&conn).expect("second run_migrations (idempotent)");

        let cols = column_names(&conn, "agent_plans");
        assert!(
            cols.iter().any(|c| c == "validation_config"),
            "agent_plans missing column validation_config (have: {:?})",
            cols
        );
    }

    #[test]
    fn migration_025_adds_phase_test_commands_column_idempotently() {
        // The gate is per-PHASE as well as per-plan: a phase's own test_commands override the
        // plan's. Same encoding as agent_plans.test_commands (JSON array of shell strings run
        // from workspace_path); NULL or empty = no gate, so existing phases are unaffected.
        let conn = Connection::open_in_memory().unwrap();
        run_migrations(&conn).expect("first run_migrations");
        run_migrations(&conn).expect("second run_migrations (idempotent)");

        let cols = column_names(&conn, "agent_plan_phases");
        assert!(
            cols.iter().any(|c| c == "test_commands"),
            "agent_plan_phases missing column test_commands (have: {:?})",
            cols
        );
        // The exit code is the only score, by design — no score column anywhere.
        assert!(
            !cols.iter().any(|c| c == "score"),
            "agent_plan_phases must not grow a score column: {:?}",
            cols
        );
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
        assert_eq!(health_from_status("phase_replan_running"), "needs-attention");
        assert_eq!(health_from_status("draft"), "in-progress");
        assert_eq!(health_from_status("approved"), "complete");
        assert_eq!(health_from_status("done"), "complete");
    }

    // ── Migration 031 (Plans tab, phase 00 task 01): brief_versions + mcp_tokens + 2 columns ──

    fn migrated_031() -> Connection {
        let conn = Connection::open_in_memory().unwrap();
        conn.execute_batch("PRAGMA foreign_keys=ON;").unwrap();
        run_migrations(&conn).expect("first run_migrations");
        run_migrations(&conn).expect("second run_migrations (idempotent)");
        conn
    }

    #[test]
    fn migration_031_creates_tables_and_columns() {
        let conn = migrated_031();
        assert_eq!(
            column_names(&conn, "brief_versions"),
            vec![
                "initiative_id", "version", "author", "fields_json", "outline_json",
                "changed_json", "change_note", "created_at",
            ]
        );
        assert_eq!(
            column_names(&conn, "mcp_tokens"),
            vec!["token_hash", "session_id", "initiative_id", "role", "created_at", "revoked_at"]
        );
        let cols = column_names(&conn, "agent_plans");
        for expected in ["accepted_brief_version", "planning_queued_after"] {
            assert!(cols.iter().any(|c| c == expected), "agent_plans missing {expected}: {cols:?}");
        }
        let index_count: i64 = conn
            .query_row(
                "SELECT COUNT(*) FROM sqlite_master WHERE type='index' AND name='idx_mcp_tokens_active'",
                [],
                |row| row.get(0),
            )
            .unwrap();
        assert_eq!(index_count, 1, "idx_mcp_tokens_active missing");
    }

    #[test]
    fn migration_031_rejects_bad_author_and_role() {
        let conn = migrated_031();
        insert_legacy_row(&conn, "p1", "planning", "draft");
        let ok = conn.execute(
            "INSERT INTO brief_versions (initiative_id, version, author, fields_json, outline_json) VALUES ('p1', 1, 'you', '{}', '{}')",
            [],
        );
        assert!(ok.is_ok(), "author='you' must be accepted: {ok:?}");
        let bad_author = conn.execute(
            "INSERT INTO brief_versions (initiative_id, version, author, fields_json, outline_json) VALUES ('p1', 2, 'bot', '{}', '{}')",
            [],
        );
        assert!(bad_author.is_err(), "author='bot' must violate the CHECK");
        let bad_role = conn.execute(
            "INSERT INTO mcp_tokens (token_hash, session_id, initiative_id, role) VALUES ('h1', 's1', 'p1', 'admin')",
            [],
        );
        assert!(bad_role.is_err(), "role='admin' must violate the CHECK");
    }

    #[test]
    fn mcp_tokens_one_live_token_per_initiative() {
        let conn = migrated_031();
        insert_legacy_row(&conn, "p1", "planning", "draft");
        let insert = |hash: &str, revoked: Option<&str>| {
            conn.execute(
                "INSERT INTO mcp_tokens (token_hash, session_id, initiative_id, role, revoked_at) VALUES (?1, 's1', 'p1', 'plan_agent', ?2)",
                params![hash, revoked],
            )
        };
        insert("h1", None).expect("first live token");
        assert!(insert("h2", None).is_err(), "a second live token must violate the partial unique index");
        insert("h3", Some("2026-11-23 00:00:00")).expect("a revoked token is not constrained");
        conn.execute("UPDATE mcp_tokens SET revoked_at = datetime('now') WHERE token_hash = 'h1'", [])
            .unwrap();
        insert("h4", None).expect("a new live token after revoking the old one");
    }

    #[test]
    fn brief_versions_cascade_on_plan_delete() {
        let conn = migrated_031();
        insert_legacy_row(&conn, "p1", "planning", "draft");
        for v in 1..=3 {
            conn.execute(
                "INSERT INTO brief_versions (initiative_id, version, author, fields_json, outline_json) VALUES ('p1', ?1, 'agent', '{}', '{}')",
                params![v],
            )
            .unwrap();
        }
        conn.execute(
            "INSERT INTO mcp_tokens (token_hash, session_id, initiative_id, role) VALUES ('h1', 's1', 'p1', 'plan_agent')",
            [],
        )
        .unwrap();
        conn.execute("DELETE FROM agent_plans WHERE id='p1'", []).unwrap();
        let versions: i64 = conn
            .query_row("SELECT COUNT(*) FROM brief_versions", [], |r| r.get(0))
            .unwrap();
        assert_eq!(versions, 0, "ON DELETE CASCADE must remove the plan's brief versions");
        let tokens: i64 = conn
            .query_row("SELECT COUNT(*) FROM mcp_tokens", [], |r| r.get(0))
            .unwrap();
        assert_eq!(tokens, 0, "ON DELETE CASCADE must remove the plan's MCP tokens");
    }

    #[test]
    fn migration_031_applies_on_030_db() {
        let conn = Connection::open_in_memory().unwrap();
        conn.execute_batch("PRAGMA foreign_keys=ON;").unwrap();
        run_migrations_through(&conn, 30).expect("migrate to 030");
        assert!(
            !column_names(&conn, "agent_plans").iter().any(|c| c == "accepted_brief_version"),
            "a 030 DB must not have the 031 columns yet"
        );
        insert_legacy_row(&conn, "keep", "planning", "draft");
        run_migrations_through(&conn, 31).expect("upgrade 030 → 031");
        let max: i64 = conn
            .query_row("SELECT MAX(version) FROM _migrations", [], |r| r.get(0))
            .unwrap();
        assert_eq!(max, 31);
        let (title, accepted, queued): (String, Option<i64>, Option<String>) = conn
            .query_row(
                "SELECT title, accepted_brief_version, planning_queued_after FROM agent_plans WHERE id='keep'",
                [],
                |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?)),
            )
            .unwrap();
        assert_eq!(title, "t", "the pre-existing row survives the upgrade");
        assert_eq!(accepted, None);
        assert_eq!(queued, None);
    }

    #[test]
    fn migration_032_rebuilds_plan_findings_keeping_rows() {
        let conn = Connection::open_in_memory().unwrap();
        conn.execute_batch("PRAGMA foreign_keys=ON;").unwrap();
        run_migrations_through(&conn, 31).expect("migrate to 031");
        insert_legacy_row(&conn, "p1", "development", "phase_review_running");
        conn.execute(
            "INSERT INTO plan_findings
                (plan_id, id, phase_id, lens, severity, state, round_opened, round_updated, claim, evidence, reopen_count, last_saw)
             VALUES ('p1', 'F-00-x-1', '00-x', 'qa', 'blocking', 'claimed', 1, 2, 'c', 'ev', 1, 'saw1')",
            [],
        )
        .unwrap();
        assert!(
            conn.execute(
                "INSERT INTO plan_findings (plan_id, id, phase_id, lens, severity, state, round_opened, round_updated, claim)
                 VALUES ('p1', 'F-00-x-2', '00-x', 'qa', 'blocking', 'disputed', 1, 1, 'c')",
                [],
            )
            .is_err(),
            "a 031 DB must refuse 'disputed'"
        );
        run_migrations(&conn).expect("upgrade 031 → 032");
        run_migrations(&conn).expect("idempotent");
        let cols = column_names(&conn, "plan_findings");
        for c in ["claim_test", "broke_what", "red_output", "red_first", "dispute_reason"] {
            assert!(cols.iter().any(|x| x == c), "missing {c}: {cols:?}");
        }
        let (state, evidence, reopen, last, red): (String, String, i64, String, Option<i64>) = conn
            .query_row(
                "SELECT state, evidence, reopen_count, last_saw, red_first FROM plan_findings WHERE id='F-00-x-1'",
                [],
                |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?, r.get(3)?, r.get(4)?)),
            )
            .unwrap();
        assert_eq!((state.as_str(), evidence.as_str(), reopen, last.as_str(), red), ("claimed", "ev", 1, "saw1", None));
        let insert = |id: &str, state: &str| {
            conn.execute(
                "INSERT INTO plan_findings (plan_id, id, phase_id, lens, severity, state, round_opened, round_updated, claim)
                 VALUES ('p1', ?1, '00-x', 'qa', 'blocking', ?2, 1, 1, 'c')",
                params![id, state],
            )
        };
        insert("F-00-x-2", "disputed").expect("disputed is a legal state now");
        insert("F-00-x-3", "withdrawn").expect("withdrawn is a legal state now");
        assert!(insert("F-00-x-4", "bogus").is_err(), "the CHECK still rejects unknown states");
        assert!(
            conn.execute("UPDATE plan_findings SET red_first = 2 WHERE id='F-00-x-1'", []).is_err(),
            "red_first is 0/1/NULL"
        );
        let index_count: i64 = conn
            .query_row(
                "SELECT COUNT(*) FROM sqlite_master WHERE type='index' AND name='idx_plan_findings_phase'",
                [],
                |row| row.get(0),
            )
            .unwrap();
        assert_eq!(index_count, 1);
        conn.execute("DELETE FROM agent_plans WHERE id='p1'", []).unwrap();
        let left: i64 = conn.query_row("SELECT COUNT(*) FROM plan_findings", [], |r| r.get(0)).unwrap();
        assert_eq!(left, 0, "the rebuilt table keeps ON DELETE CASCADE");
    }
}
