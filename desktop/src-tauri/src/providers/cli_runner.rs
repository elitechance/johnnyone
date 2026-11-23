use super::{CliSpawnConfig, StreamChunk};
use std::process::Stdio;
use tokio::io::{AsyncBufReadExt, AsyncWriteExt, BufReader};
use tokio::process::{Child, Command};
use tokio::sync::mpsc;

/// Boxed future returned by [`ExternalTurn::stop`].
pub type StopFuture<'a> = std::pin::Pin<Box<dyn std::future::Future<Output = Result<(), String>> + Send + 'a>>;

/// A turn that runs somewhere other than a direct child of the engine — the plan agent's turn runs
/// inside its tmux pane (`services::plan_agent_tmux`). It is registered in `active_processes` like
/// a child so Stop and the plan agent's retire can end it the same way.
pub trait ExternalTurn: Send {
    /// End the turn (best effort).
    fn stop(&mut self) -> StopFuture<'_>;
    /// Whether the turn is still in flight.
    fn is_running(&self) -> bool;
}

enum Handle {
    Child(Child),
    External(Box<dyn ExternalTurn>),
}

/// Handle to a running CLI subprocess (or an [`ExternalTurn`]).
pub struct CliProcess {
    handle: Handle,
    pub session_id: String,
    /// Unique per handle, so a chat path only ever removes/kills ITS OWN turn from
    /// `active_processes` (see `chat_host::take_own_process`).
    turn_id: u64,
}

static NEXT_TURN_ID: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(1);

fn next_turn_id() -> u64 {
    NEXT_TURN_ID.fetch_add(1, std::sync::atomic::Ordering::Relaxed)
}

impl CliProcess {
    /// Wrap a turn that is not a child process.
    pub fn external(turn: Box<dyn ExternalTurn>, session_id: String) -> Self {
        Self { handle: Handle::External(turn), session_id, turn_id: next_turn_id() }
    }

    /// This handle's unique turn id.
    pub fn turn_id(&self) -> u64 {
        self.turn_id
    }

    /// Kill the subprocess.
    pub async fn kill(&mut self) -> Result<(), String> {
        match &mut self.handle {
            Handle::Child(child) => child
                .kill()
                .await
                .map_err(|e| format!("Failed to kill process: {}", e)),
            Handle::External(turn) => turn.stop().await,
        }
    }

    /// Check if the subprocess is still running.
    pub fn is_running(&mut self) -> bool {
        match &mut self.handle {
            Handle::Child(child) => child.try_wait().ok().flatten().is_none(),
            Handle::External(turn) => turn.is_running(),
        }
    }

    /// Block until the child exits and return its status (needed for check 1 / 127).
    pub async fn wait(&mut self) -> Result<std::process::ExitStatus, String> {
        match &mut self.handle {
            Handle::Child(child) => child
                .wait()
                .await
                .map_err(|e| format!("Failed to wait for process: {}", e)),
            Handle::External(_) => Err("not a child process".to_string()),
        }
    }

    /// OS pid, if the child is still attached.
    pub fn id(&self) -> Option<u32> {
        match &self.handle {
            Handle::Child(child) => child.id(),
            Handle::External(_) => None,
        }
    }

    #[cfg(test)]
    pub fn from_child(child: Child, session_id: String) -> Self {
        Self { handle: Handle::Child(child), session_id, turn_id: next_turn_id() }
    }
}

/// Spawn a CLI subprocess and stream its output line by line.
///
/// Returns a `CliProcess` handle and an `mpsc::Receiver<StreamChunk>` for consuming output.
/// The `parse_line` function converts raw output lines into `StreamChunk`s.
pub async fn spawn_cli<F>(
    config: CliSpawnConfig,
    session_id: String,
    parse_line: F,
) -> Result<(CliProcess, mpsc::Receiver<StreamChunk>), String>
where
    F: Fn(&str) -> Option<StreamChunk> + Send + 'static,
{
    spawn_cli_inner(config, session_id, parse_line, false).await
}

/// Same as [`spawn_cli`] but the child is the leader of a new process group
/// (unix) so Stop can `kill -TERM -<pid>` and reap verify/tool grandchildren.
pub async fn spawn_cli_process_group<F>(
    config: CliSpawnConfig,
    session_id: String,
    parse_line: F,
) -> Result<(CliProcess, mpsc::Receiver<StreamChunk>), String>
where
    F: Fn(&str) -> Option<StreamChunk> + Send + 'static,
{
    spawn_cli_inner(config, session_id, parse_line, true).await
}

/// The one log line written before every CLI spawn. Its args are safe to log by construction: a
/// plan-agent turn carries only the PATH of its MCP config, never the token (phase 02 task 07,
/// pinned by `logged_launch_args_contain_no_token`).
pub(crate) fn log_spawn(config: &CliSpawnConfig, working_dir: &str) {
    tracing::info!(
        cmd = %config.command,
        args = ?config.args,
        cwd = %working_dir,
        "Spawning CLI subprocess"
    );
}

async fn spawn_cli_inner<F>(
    mut config: CliSpawnConfig,
    session_id: String,
    parse_line: F,
    process_group: bool,
) -> Result<(CliProcess, mpsc::Receiver<StreamChunk>), String>
where
    F: Fn(&str) -> Option<StreamChunk> + Send + 'static,
{
    let working_dir = if config.working_directory.is_empty() {
        dirs::home_dir()
            .unwrap_or_else(|| std::path::PathBuf::from("/"))
            .to_string_lossy()
            .to_string()
    } else {
        config.working_directory.clone()
    };

    log_spawn(&config, &working_dir);

    let mut cmd = Command::new(&config.command);
    cmd.args(&config.args)
        .current_dir(&working_dir)
        .stdout(Stdio::piped())
        .stderr(Stdio::piped());
    // A body to hand the child on stdin (see `CliSpawnConfig::stdin` — a single
    // argv string is capped at MAX_ARG_STRLEN, so a big request body has
    // nowhere else to go). Otherwise stdin is /dev/null exactly as before.
    if config.stdin.is_some() {
        cmd.stdin(Stdio::piped());
    } else {
        cmd.stdin(Stdio::null());
    }
    if process_group {
        #[cfg(unix)]
        cmd.process_group(0);
    }

    for (key, val) in &config.env_vars {
        if val.is_empty() {
            cmd.env_remove(key);
        } else {
            cmd.env(key, val);
        }
    }

    let mut child = cmd
        .spawn()
        .map_err(|e| format!("Failed to spawn '{}': {}", config.command, e))?;

    let stdout = child.stdout.take().ok_or("Failed to capture stdout")?;
    let stderr = child.stderr.take().ok_or("Failed to capture stderr")?;

    // Write the body and CLOSE the handle. Dropping it is what sends EOF; a
    // child reading `--data @-` waits for that EOF, so keeping the handle alive
    // (or leaving the write to a caller that never comes) hangs the lens
    // forever — strictly worse than the E2BIG this replaces.
    if let Some(body) = config.stdin.take() {
        let mut stdin = child.stdin.take().ok_or("Failed to capture stdin")?;
        stdin
            .write_all(body.as_bytes())
            .await
            .map_err(|e| format!("Failed to write stdin for '{}': {}", config.command, e))?;
        stdin
            .flush()
            .await
            .map_err(|e| format!("Failed to flush stdin for '{}': {}", config.command, e))?;
        drop(stdin);
    }

    let (tx, rx) = mpsc::channel::<StreamChunk>(256);

    // Spawn stdout reader
    let stdout_tx = tx.clone();
    tokio::spawn(async move {
        let reader = BufReader::new(stdout);
        let mut lines = reader.lines();
        while let Ok(Some(line)) = lines.next_line().await {
            if let Some(chunk) = parse_line(&line) {
                if stdout_tx.send(chunk).await.is_err() {
                    break;
                }
            }
        }
    });

    // Spawn stderr reader
    let stderr_tx = tx;
    tokio::spawn(async move {
        let reader = BufReader::new(stderr);
        let mut lines = reader.lines();
        while let Ok(Some(line)) = lines.next_line().await {
            let chunk = StreamChunk {
                chunk_type: super::ChunkType::Stderr,
                content: line,
                tool_name: None,
                tool_input: None,
                tool_output: None,
                cost_usd: None,
                input_tokens: None,
                output_tokens: None,
                is_final: false,
                session_id: None,
            };
            if stderr_tx.send(chunk).await.is_err() {
                break;
            }
        }
    });

    let process = CliProcess { handle: Handle::Child(child), session_id, turn_id: next_turn_id() };

    Ok((process, rx))
}

/// Send input to a running CLI subprocess's stdin.
pub async fn send_stdin(process: &mut CliProcess, input: &str) -> Result<(), String> {
    let Handle::Child(child) = &mut process.handle else {
        return Err("Process stdin not available".to_string());
    };
    if let Some(stdin) = child.stdin.as_mut() {
        stdin
            .write_all(input.as_bytes())
            .await
            .map_err(|e| format!("Failed to write to stdin: {}", e))?;
        stdin
            .write_all(b"\n")
            .await
            .map_err(|e| format!("Failed to write newline: {}", e))?;
        stdin
            .flush()
            .await
            .map_err(|e| format!("Failed to flush stdin: {}", e))?;
        Ok(())
    } else {
        Err("Process stdin not available".to_string())
    }
}
