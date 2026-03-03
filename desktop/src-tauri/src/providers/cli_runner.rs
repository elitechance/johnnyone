use super::{CliSpawnConfig, StreamChunk};
use std::process::Stdio;
use tokio::io::{AsyncBufReadExt, AsyncWriteExt, BufReader};
use tokio::process::{Child, Command};
use tokio::sync::mpsc;

/// Handle to a running CLI subprocess.
pub struct CliProcess {
    child: Child,
    pub session_id: String,
}

impl CliProcess {
    /// Kill the subprocess.
    pub async fn kill(&mut self) -> Result<(), String> {
        self.child
            .kill()
            .await
            .map_err(|e| format!("Failed to kill process: {}", e))
    }

    /// Check if the subprocess is still running.
    pub fn is_running(&mut self) -> bool {
        self.child.try_wait().ok().flatten().is_none()
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
    let working_dir = if config.working_directory.is_empty() {
        dirs::home_dir()
            .unwrap_or_else(|| std::path::PathBuf::from("/"))
            .to_string_lossy()
            .to_string()
    } else {
        config.working_directory.clone()
    };

    tracing::info!(
        cmd = %config.command,
        args = ?config.args,
        cwd = %working_dir,
        "Spawning CLI subprocess"
    );

    let mut cmd = Command::new(&config.command);
    cmd.args(&config.args)
        .current_dir(&working_dir)
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped());

    for (key, val) in &config.env_vars {
        cmd.env(key, val);
    }

    let mut child = cmd
        .spawn()
        .map_err(|e| format!("Failed to spawn '{}': {}", config.command, e))?;

    let stdout = child
        .stdout
        .take()
        .ok_or("Failed to capture stdout")?;
    let stderr = child
        .stderr
        .take()
        .ok_or("Failed to capture stderr")?;

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

    let process = CliProcess {
        child,
        session_id,
    };

    Ok((process, rx))
}

/// Send input to a running CLI subprocess's stdin.
pub async fn send_stdin(process: &mut CliProcess, input: &str) -> Result<(), String> {
    if let Some(stdin) = process.child.stdin.as_mut() {
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
