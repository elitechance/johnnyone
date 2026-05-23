//! Host-process supervisor.
//!
//! The Tauri installer shell spawns `johnnyone-host` as a managed child process
//! and keeps it alive across crashes / restarts. The control-panel UI (host-app)
//! reaches the host over `127.0.0.1:7788/graphql`; the supervisor only owns the
//! process lifecycle, not the host's business logic.
//!
//! Design:
//!   - One `HostSupervisor` per Tauri app instance, stored in Tauri state.
//!   - `start()` spawns the host bin with the appropriate env, monitors stdout +
//!     stderr (logs to tracing), and restarts on exit with exponential backoff
//!     up to `MAX_BACKOFF`.
//!   - `stop()` kills the child cleanly on app shutdown.
//!   - `status()` returns current state (Running { pid } | Restarting | Stopped).
//!
//! Cross-platform: uses `tokio::process::Command` so it works on Mac/Win/Linux.
//! The host binary path is resolved relative to the Tauri executable's directory
//! at runtime (Phase 4 packaging will bundle the host bin alongside the Tauri
//! shell so the resolution Just Works after install).

use std::path::PathBuf;
use std::sync::Arc;
use std::time::Duration;
use tokio::process::Command;
use tokio::sync::Mutex;
use tracing::{error, info, warn};

const INITIAL_BACKOFF_MS: u64 = 500;
const MAX_BACKOFF_MS: u64 = 30_000;
const MAX_CONSECUTIVE_FAILURES: u32 = 5;

#[derive(Debug, Clone, serde::Serialize)]
#[serde(rename_all = "camelCase", tag = "state")]
pub enum HostStatus {
    Running { pid: u32 },
    Restarting { attempt: u32, next_in_ms: u64 },
    Stopped { reason: String },
}

pub struct HostSupervisor {
    inner: Arc<Mutex<SupervisorInner>>,
}

struct SupervisorInner {
    status: HostStatus,
    handle: Option<tokio::task::JoinHandle<()>>,
}

impl HostSupervisor {
    pub fn new() -> Self {
        Self {
            inner: Arc::new(Mutex::new(SupervisorInner {
                status: HostStatus::Stopped {
                    reason: "Not started".into(),
                },
                handle: None,
            })),
        }
    }

    /// Spawn the host bin and keep it alive in a background task.
    pub async fn start(&self, host_bin: PathBuf, env: Vec<(String, String)>) {
        let inner = self.inner.clone();
        let handle = tokio::spawn(async move {
            run_with_restart(inner, host_bin, env).await;
        });

        let mut guard = self.inner.lock().await;
        guard.handle = Some(handle);
    }

    /// Kill the supervised child and stop the supervisor loop.
    pub async fn stop(&self) {
        let mut guard = self.inner.lock().await;
        if let Some(handle) = guard.handle.take() {
            handle.abort();
        }
        guard.status = HostStatus::Stopped {
            reason: "Supervisor stopped by app".into(),
        };
    }

    pub async fn status(&self) -> HostStatus {
        self.inner.lock().await.status.clone()
    }
}

impl Default for HostSupervisor {
    fn default() -> Self {
        Self::new()
    }
}

async fn run_with_restart(
    inner: Arc<Mutex<SupervisorInner>>,
    host_bin: PathBuf,
    env: Vec<(String, String)>,
) {
    let mut backoff_ms = INITIAL_BACKOFF_MS;
    let mut consecutive_failures: u32 = 0;

    loop {
        info!(?host_bin, "Spawning johnnyone-host");
        let mut cmd = Command::new(&host_bin);
        for (k, v) in &env {
            cmd.env(k, v);
        }
        cmd.kill_on_drop(true);

        let child_result = cmd.spawn();
        let mut child = match child_result {
            Ok(c) => c,
            Err(e) => {
                error!(error = %e, "Failed to spawn johnnyone-host");
                consecutive_failures += 1;
                if consecutive_failures >= MAX_CONSECUTIVE_FAILURES {
                    let mut g = inner.lock().await;
                    g.status = HostStatus::Stopped {
                        reason: format!("Too many failures: {e}"),
                    };
                    return;
                }
                {
                    let mut g = inner.lock().await;
                    g.status = HostStatus::Restarting {
                        attempt: consecutive_failures,
                        next_in_ms: backoff_ms,
                    };
                }
                tokio::time::sleep(Duration::from_millis(backoff_ms)).await;
                backoff_ms = (backoff_ms * 2).min(MAX_BACKOFF_MS);
                continue;
            }
        };

        if let Some(pid) = child.id() {
            let mut g = inner.lock().await;
            g.status = HostStatus::Running { pid };
            // Reset backoff once the child reaches Running.
            backoff_ms = INITIAL_BACKOFF_MS;
            consecutive_failures = 0;
        }

        let exit = child.wait().await;
        match exit {
            Ok(status) if status.success() => {
                info!(?status, "johnnyone-host exited cleanly; not restarting");
                let mut g = inner.lock().await;
                g.status = HostStatus::Stopped {
                    reason: "Host exited cleanly".into(),
                };
                return;
            }
            Ok(status) => {
                warn!(?status, "johnnyone-host exited with non-zero status; restarting");
            }
            Err(e) => {
                warn!(error = %e, "Error waiting for johnnyone-host; restarting");
            }
        }

        consecutive_failures += 1;
        if consecutive_failures >= MAX_CONSECUTIVE_FAILURES {
            let mut g = inner.lock().await;
            g.status = HostStatus::Stopped {
                reason: format!(
                    "johnnyone-host failed {MAX_CONSECUTIVE_FAILURES} times consecutively"
                ),
            };
            return;
        }

        {
            let mut g = inner.lock().await;
            g.status = HostStatus::Restarting {
                attempt: consecutive_failures,
                next_in_ms: backoff_ms,
            };
        }
        tokio::time::sleep(Duration::from_millis(backoff_ms)).await;
        backoff_ms = (backoff_ms * 2).min(MAX_BACKOFF_MS);
    }
}

/// Locate the `johnnyone-host` bin next to the running Tauri executable, or
/// fall back to the dev path (`target/debug/johnnyone-host`).
pub fn resolve_host_bin() -> Option<PathBuf> {
    if let Ok(exe) = std::env::current_exe() {
        if let Some(parent) = exe.parent() {
            let candidate = parent.join(if cfg!(windows) {
                "johnnyone-host.exe"
            } else {
                "johnnyone-host"
            });
            if candidate.exists() {
                return Some(candidate);
            }
        }
    }
    // Dev fallback — sibling target dir.
    let dev = PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("target/debug/johnnyone-host");
    if dev.exists() {
        return Some(dev);
    }
    None
}
