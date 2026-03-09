use crate::state::app_state::AppState;
use rusqlite::params;

pub fn get_setting(state: &AppState, key: String) -> Result<String, String> {
    state.db.with_conn(|conn| {
        conn.query_row(
            "SELECT value FROM settings WHERE key = ?1",
            params![key],
            |row| row.get::<_, String>(0),
        )
        .map_err(|e| format!("Setting not found: {}", e))
    })
}

pub fn set_setting(state: &AppState, key: String, value: String) -> Result<(), String> {
    state.db.with_conn(|conn| {
        conn.execute(
            "INSERT INTO settings (key, value) VALUES (?1, ?2)
             ON CONFLICT(key) DO UPDATE SET value = ?2",
            params![key, value],
        )
        .map_err(|e| format!("Failed to set setting: {}", e))?;
        Ok(())
    })
}
