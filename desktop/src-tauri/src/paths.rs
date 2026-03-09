use std::path::PathBuf;

/// Default SQLite path shared by the desktop shell and the standalone host.
pub fn default_db_path() -> PathBuf {
    if let Ok(configured_path) = std::env::var("JOHNNYONE_DB_PATH") {
        let trimmed = configured_path.trim();
        if !trimmed.is_empty() {
            return PathBuf::from(trimmed);
        }
    }

    let data_dir = dirs::data_local_dir()
        .unwrap_or_else(|| {
            dirs::home_dir()
                .unwrap_or_else(|| PathBuf::from("/tmp"))
                .join(".local/share")
        })
        .join("johnnyone");

    data_dir.join("johnnyone.db")
}
