//! The WITHHELD-nudge counter (migration 028).
//!
//! Pure, like `sessions::watchdog_backfill_tests`: the crate has no `AppState`/DB harness, so the
//! hold is proven at the level that actually ships — `sessions::bump_watchdog_held` executes the
//! very `BUMP_WATCHDOG_HELD_SQL` const under test, on a connection holding just the columns it
//! touches. Registered from `services/mod.rs` as `#[cfg(test)] mod watchdog_held_tests;`.

use rusqlite::{params, Connection};

/// A connection holding one session `id='s1'` with the watchdog columns of migrations 027 + 028,
/// seeded as a row that is ARMED and has been nudged exactly 3 times, so a hold can never be
/// confused with a nudge.
fn conn_with_armed_session() -> Connection {
    let conn = Connection::open_in_memory().expect("in-memory sqlite");
    conn.execute_batch(
        "CREATE TABLE sessions (
            id TEXT PRIMARY KEY,
            watchdog_enabled       INTEGER NOT NULL DEFAULT 0,
            watchdog_interval_secs INTEGER NOT NULL DEFAULT 1200,
            watchdog_prompt        TEXT    NOT NULL DEFAULT '',
            watchdog_last_nudge_at TEXT,
            watchdog_nudge_count   INTEGER NOT NULL DEFAULT 0,
            watchdog_held_count    INTEGER NOT NULL DEFAULT 0,
            updated_at             TEXT    NOT NULL DEFAULT (datetime('now'))
        );",
    )
    .expect("create sessions");
    conn.execute(
        "INSERT INTO sessions (id, watchdog_enabled, watchdog_prompt, \
         watchdog_last_nudge_at, watchdog_nudge_count) \
         VALUES ('s1', 1, 'Report where you stand.', '2024-01-15 10:00:00', 3)",
        [],
    )
    .expect("insert s1");
    conn
}

/// The REAL statement `sessions::bump_watchdog_held` executes. Going through the const rather than
/// retyping the SQL here is the point: if the shipped UPDATE becomes a no-op, these tests go red.
fn hold(conn: &Connection) {
    conn.execute(crate::services::sessions::BUMP_WATCHDOG_HELD_SQL, params!["s1"])
        .expect("the held-count UPDATE must hit the row");
}

fn held_of(conn: &Connection) -> i64 {
    conn.query_row(
        "SELECT watchdog_held_count FROM sessions WHERE id = 's1'",
        [],
        |row| row.get::<_, i64>(0),
    )
    .expect("read back watchdog_held_count")
}

/// Migration 028's default must be 0, not NULL — `SESSION_SELECT_COLUMNS` reads the column as a
/// plain `i64`, so a NULL would fail the row and silently drop the session from the listing.
#[test]
fn a_fresh_row_starts_at_zero_holds() {
    let conn = conn_with_armed_session();
    assert_eq!(held_of(&conn), 0, "028 defaults to 0, not NULL");
}

/// The gap this closes: the sweep decided a nudge WAS due, the two-capture idle probe saw the pane
/// still changing, and the nudge was withheld — silently. Two sweeps that each hold one nudge must
/// leave the row saying `2`, so the console can print "held 2×" instead of the old silence that
/// made "held back" indistinguishable from "quiet, nothing to say".
#[test]
fn held_count_bumps_only_when_a_due_nudge_is_withheld() {
    let conn = conn_with_armed_session();
    assert_eq!(held_of(&conn), 0, "starts at 0");

    hold(&conn);
    assert_eq!(held_of(&conn), 1, "one withheld nudge reads as 1");

    hold(&conn);
    assert_eq!(held_of(&conn), 2, "a second withheld nudge reads as 2");

    // And it is per-row: holding s1 must not move any other session's count.
    conn.execute(
        "INSERT INTO sessions (id, watchdog_prompt) VALUES ('s2', 'other')",
        [],
    )
    .expect("insert s2");
    hold(&conn);
    assert_eq!(held_of(&conn), 3, "s1 keeps accumulating");
    assert_eq!(
        conn.query_row(
            "SELECT watchdog_held_count FROM sessions WHERE id = 's2'",
            [],
            |row| row.get::<_, i64>(0)
        )
        .expect("read s2"),
        0,
        "the UPDATE is keyed by id — a hold on s1 cannot hold another session"
    );
}

/// THE assertion that stops someone "simplifying" a hold into a nudge later.
///
/// A held nudge was never delivered, so it must not appear in `watchdog_nudge_count` (what the
/// console prints as "N sent", and what `{nudges}` in the prompt expands to), and it must not stamp
/// `watchdog_last_nudge_at` — that timestamp is what `watchdog_is_due` reads as "we just spoke", so
/// stamping it would make every hold suppress the next nudge and the watchdog would quietly stop
/// firing on a busy pane while the UI still read "armed".
#[test]
fn held_count_does_not_touch_the_nudge_counters() {
    let conn = conn_with_armed_session();

    hold(&conn);
    hold(&conn);

    let (nudge_count, last_nudge_at, held): (i64, Option<String>, i64) = conn
        .query_row(
            "SELECT watchdog_nudge_count, watchdog_last_nudge_at, watchdog_held_count \
             FROM sessions WHERE id = 's1'",
            [],
            |row| Ok((row.get(0)?, row.get(1)?, row.get(2)?)),
        )
        .expect("read the watchdog counters");

    assert_eq!(held, 2, "both holds landed");
    assert_eq!(
        nudge_count, 3,
        "watchdog_nudge_count counts nudges the agent actually received — a hold is not one"
    );
    assert_eq!(
        last_nudge_at.as_deref(),
        Some("2024-01-15 10:00:00"),
        "watchdog_last_nudge_at is the due-check's clock; stamping it on a hold would suppress \
         the next nudge"
    );

    // A never-nudged row must stay never-nudged after a hold: NULL, not a timestamp.
    conn.execute(
        "UPDATE sessions SET watchdog_last_nudge_at = NULL, watchdog_nudge_count = 0 \
         WHERE id = 's1'",
        [],
    )
    .expect("clear the nudge stamp");
    hold(&conn);
    assert_eq!(
        conn.query_row(
            "SELECT watchdog_last_nudge_at FROM sessions WHERE id = 's1'",
            [],
            |row| row.get::<_, Option<String>>(0)
        )
        .expect("read back the stamp"),
        None,
        "a hold on a never-nudged row must leave watchdog_last_nudge_at NULL"
    );
}