//! Shared, test-only harness for the overhaul P2 file-ops phase.
//!
//! `host_files.rs` (service + serde) and `agent/mod.rs` (relay-RPC wiring) both need an in-process
//! `AppState` backed by a throwaway temp dir. This is the single reusable copy of the Phase-1
//! `test_state`/`tmp_dir` idiom so the two modules don't each fork their own. `#[cfg(test)]`-gated:
//! it is never compiled into the shipped binary.

use crate::db::Database;
use crate::state::app_state::AppState;
use std::path::PathBuf;
use std::sync::atomic::{AtomicU64, Ordering};

static SEQ: AtomicU64 = AtomicU64::new(0);

/// Unique, freshly-created temp dir. `Date::now`/random are unavailable in this environment, so the
/// suffix is made unique with the pid + a static counter (mirrors the Phase-1 harness).
pub fn tmp_dir(tag: &str) -> PathBuf {
    let d = std::env::temp_dir().join(format!(
        "j1-p2-{}-{}-{}",
        tag,
        std::process::id(),
        SEQ.fetch_add(1, Ordering::Relaxed)
    ));
    std::fs::create_dir_all(&d).unwrap();
    d
}

/// Full in-process harness: `Database::open` runs migrations; `AppState::new` wires the broadcast
/// channels with no live process, relay, or Tauri handle. Returns the state plus its temp root so
/// callers can clean up / point `files_root` at it.
pub fn test_state() -> (AppState, PathBuf) {
    let root = tmp_dir("state");
    let db = Database::open(&root.join("test.db")).expect("open db");
    (AppState::new(db), root)
}
