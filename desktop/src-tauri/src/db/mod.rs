pub mod migrations;
pub mod models;

use rusqlite::Connection;
use std::path::PathBuf;
use std::sync::{Arc, Mutex};

/// Thread-safe SQLite database wrapper.
/// Uses `std::sync::Mutex` because rusqlite `Connection` is not `Send`.
#[derive(Debug, Clone)]
pub struct Database {
    conn: Arc<Mutex<Connection>>,
}

impl Database {
    /// Open (or create) the database at the given path and run migrations.
    pub fn open(path: &PathBuf) -> Result<Self, String> {
        // Ensure parent directory exists
        if let Some(parent) = path.parent() {
            std::fs::create_dir_all(parent)
                .map_err(|e| format!("Failed to create database directory: {}", e))?;
        }

        let conn = Connection::open(path)
            .map_err(|e| format!("Failed to open database at {:?}: {}", path, e))?;

        // Enable WAL mode for better concurrent read performance
        conn.execute_batch("PRAGMA journal_mode=WAL; PRAGMA foreign_keys=ON;")
            .map_err(|e| format!("Failed to set PRAGMA: {}", e))?;

        // Run migrations
        migrations::run_migrations(&conn)?;

        tracing::info!(path = ?path, "Database initialized");

        Ok(Self {
            conn: Arc::new(Mutex::new(conn)),
        })
    }

    /// Execute a closure with a reference to the database connection.
    /// The closure runs while holding the mutex lock.
    pub fn with_conn<F, T>(&self, f: F) -> Result<T, String>
    where
        F: FnOnce(&Connection) -> Result<T, String>,
    {
        let conn = self
            .conn
            .lock()
            .map_err(|e| format!("Database lock poisoned: {}", e))?;
        f(&conn)
    }
}
