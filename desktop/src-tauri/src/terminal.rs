use crate::events::TerminalScreenEvent;
use crate::providers::CliProvider;
use crate::state::app_state::AppState;
use rusqlite::params;
use serde::Serialize;
use std::process::Stdio;
use tauri::Emitter;
use tokio::io::AsyncWriteExt;
use tokio::process::Command;
use tokio::time::{sleep, Duration};

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct TerminalSnapshot {
    pub session_id: String,
    pub pane_id: String,
    pub cursor: i64,
    pub content: String,
    pub cursor_x: u16,
    pub cursor_y: u16,
    pub history_lines: u16,
    pub rows: u16,
    pub cols: u16,
    pub status: String,
}

#[derive(Debug, Clone)]
struct TerminalSession {
    session_id: String,
    pane_id: String,
    rows: u16,
    cols: u16,
}

#[derive(Debug)]
struct TerminalCapture {
    content: String,
    cursor_x: u16,
    cursor_y: u16,
    history_lines: u16,
}

#[derive(Debug)]
struct SessionConfig {
    session_id: String,
    provider: CliProvider,
    model: String,
    working_directory: String,
    cli_path: Option<String>,
}

pub async fn attach_terminal(
    state: &AppState,
    app_handle: tauri::AppHandle,
    session_id: String,
    cols: u16,
    rows: u16,
) -> Result<TerminalSnapshot, String> {
    let terminal = ensure_terminal_session(state, &session_id, cols, rows).await?;
    let snapshot = capture_snapshot(state, &terminal).await?;
    start_capture_loop(state, Some(app_handle), terminal).await;
    Ok(snapshot)
}

pub async fn attach_terminal_headless(
    state: &AppState,
    session_id: String,
    cols: u16,
    rows: u16,
) -> Result<TerminalSnapshot, String> {
    let terminal = ensure_terminal_session(state, &session_id, cols, rows).await?;
    let snapshot = capture_snapshot(state, &terminal).await?;
    start_capture_loop(state, None, terminal).await;
    Ok(snapshot)
}

pub async fn send_terminal_input(
    state: &AppState,
    session_id: String,
    input: String,
) -> Result<(), String> {
    let terminal = ensure_terminal_session_for_input(state, &session_id).await?;
    send_raw_input(&terminal.pane_id, &input).await
}

pub async fn resize_terminal(
    state: &AppState,
    session_id: String,
    cols: u16,
    rows: u16,
) -> Result<(), String> {
    let terminal = ensure_terminal_session(state, &session_id, cols, rows).await?;
    resize_pane(&terminal.pane_id, cols, rows).await?;
    start_capture_loop(state, None, terminal).await;
    Ok(())
}

pub async fn stop_terminal_capture(state: &AppState, session_id: &str) {
    let mut tasks = state.terminal_capture_tasks.lock().await;
    if let Some(task) = tasks.remove(session_id) {
        task.abort();
    }
}

pub async fn kill_terminal_session(state: &AppState, session_id: &str) -> Result<(), String> {
    stop_terminal_capture(state, session_id).await;

    let tmux_session_name = tmux_session_name(session_id);
    if tmux_has_session(&tmux_session_name).await {
        run_tmux(vec![
            "kill-session".to_string(),
            "-t".to_string(),
            tmux_session_name,
        ])
        .await
        .map(|_| ())?;
    }

    state.db.with_conn(|conn| {
        conn.execute(
            "UPDATE sessions SET terminal_status = 'detached', tmux_pane_id = NULL, updated_at = datetime('now') WHERE id = ?1",
            params![session_id],
        )
        .map_err(|e| e.to_string())
    })?;

    Ok(())
}

async fn start_capture_loop(
    state: &AppState,
    app_handle: Option<tauri::AppHandle>,
    terminal: TerminalSession,
) {
    {
        let mut tasks = state.terminal_capture_tasks.lock().await;
        if let Some(existing) = tasks.remove(&terminal.session_id) {
            existing.abort();
        }
    }

    let db = state.db.clone();
    let terminal_screen_tx = state.terminal_screen_tx.clone();
    let session_id = terminal.session_id.clone();
    let handle = tokio::spawn(async move {
        let mut last_capture_key = String::new();
        let mut cursor: i64 = 0;

        loop {
            match capture_terminal(&terminal.pane_id).await {
                Ok(capture) => {
                    let capture_key = format!(
                        "{}\n{}:{}:{}",
                        capture.content, capture.cursor_x, capture.cursor_y, capture.history_lines
                    );
                    if capture_key != last_capture_key {
                        cursor += 1;
                        last_capture_key = capture_key;

                        let event = TerminalScreenEvent {
                            session_id: terminal.session_id.clone(),
                            pane_id: terminal.pane_id.clone(),
                            cursor,
                            content: capture.content,
                            cursor_x: capture.cursor_x,
                            cursor_y: capture.cursor_y,
                            history_lines: capture.history_lines,
                            rows: terminal.rows,
                            cols: terminal.cols,
                            status: "attached".to_string(),
                        };

                        let _ = db.with_conn(|conn| {
                            conn.execute(
                                "UPDATE sessions SET tmux_screen_cursor = ?1, terminal_status = 'attached', updated_at = datetime('now') WHERE id = ?2",
                                params![cursor, &terminal.session_id],
                            )
                            .map_err(|e| e.to_string())
                        });

                        let _ = terminal_screen_tx.send(event.clone());
                        if let Some(handle) = &app_handle {
                            let _ = handle.emit("terminal:screen", event);
                        }
                    }
                }
                Err(error) => {
                    tracing::warn!(
                        session_id = %terminal.session_id,
                        pane_id = %terminal.pane_id,
                        error = %error,
                        "terminal capture failed"
                    );
                    let _ = db.with_conn(|conn| {
                        conn.execute(
                            "UPDATE sessions SET terminal_status = 'detached', updated_at = datetime('now') WHERE id = ?1",
                            params![&terminal.session_id],
                        )
                        .map_err(|e| e.to_string())
                    });
                    break;
                }
            }

            sleep(Duration::from_millis(350)).await;
        }
    });

    let mut tasks = state.terminal_capture_tasks.lock().await;
    tasks.insert(session_id, handle);
}

async fn ensure_terminal_session(
    state: &AppState,
    session_id: &str,
    cols: u16,
    rows: u16,
) -> Result<TerminalSession, String> {
    let config = load_session_config(state, session_id)?;
    let tmux_session_name = tmux_session_name(session_id);

    if !tmux_has_session(&tmux_session_name).await {
        if let Err(error) = create_tmux_session(&tmux_session_name, &config, cols, rows).await {
            if !tmux_has_session(&tmux_session_name).await {
                return Err(error);
            }
        }
    }

    let pane_id = list_first_pane(&tmux_session_name).await?;
    resize_pane(&pane_id, cols, rows).await?;

    state.db.with_conn(|conn| {
        conn.execute(
            "UPDATE sessions SET tmux_session_name = ?1, tmux_pane_id = ?2, terminal_status = 'attached', updated_at = datetime('now') WHERE id = ?3",
            params![&tmux_session_name, &pane_id, session_id],
        )
        .map_err(|e| e.to_string())
    })?;

    Ok(TerminalSession {
        session_id: config.session_id,
        pane_id,
        rows,
        cols,
    })
}

async fn ensure_terminal_session_for_input(
    state: &AppState,
    session_id: &str,
) -> Result<TerminalSession, String> {
    let config = load_session_config(state, session_id)?;
    let tmux_session_name = tmux_session_name(session_id);

    if !tmux_has_session(&tmux_session_name).await {
        if let Err(error) = create_tmux_session(&tmux_session_name, &config, 100, 30).await {
            if !tmux_has_session(&tmux_session_name).await {
                return Err(error);
            }
        }
    }

    let pane_id = list_first_pane(&tmux_session_name).await?;
    let (cols, rows) = pane_size(&pane_id).await.unwrap_or((100, 30));

    state.db.with_conn(|conn| {
        conn.execute(
            "UPDATE sessions SET tmux_session_name = ?1, tmux_pane_id = ?2, terminal_status = 'attached', updated_at = datetime('now') WHERE id = ?3",
            params![&tmux_session_name, &pane_id, session_id],
        )
        .map_err(|e| e.to_string())
    })?;

    Ok(TerminalSession {
        session_id: config.session_id,
        pane_id,
        rows,
        cols,
    })
}

fn load_session_config(state: &AppState, session_id: &str) -> Result<SessionConfig, String> {
    let (provider_str, model, working_directory) = state.db.with_conn(|conn| {
        conn.query_row(
            "SELECT provider, model, working_directory FROM sessions WHERE id = ?1",
            params![session_id],
            |row| {
                Ok((
                    row.get::<_, String>(0)?,
                    row.get::<_, String>(1)?,
                    row.get::<_, String>(2)?,
                ))
            },
        )
        .map_err(|e| format!("Session not found: {}", e))
    })?;

    let provider = CliProvider::from_str(&provider_str)
        .ok_or_else(|| format!("Unknown provider: {}", provider_str))?;

    let cli_path: Option<String> = state.db.with_conn(|conn| {
        let result = conn.query_row(
            "SELECT cli_path FROM provider_configs WHERE provider = ?1 AND is_available = 1",
            params![provider.as_str()],
            |row| row.get::<_, String>(0),
        );
        Ok(result.ok().filter(|path| !path.trim().is_empty()))
    })?;

    Ok(SessionConfig {
        session_id: session_id.to_string(),
        provider,
        model,
        working_directory,
        cli_path,
    })
}

async fn create_tmux_session(
    tmux_session_name: &str,
    config: &SessionConfig,
    cols: u16,
    rows: u16,
) -> Result<(), String> {
    let working_dir = if config.working_directory.trim().is_empty() {
        dirs::home_dir()
            .unwrap_or_else(|| std::path::PathBuf::from("/"))
            .to_string_lossy()
            .to_string()
    } else {
        config.working_directory.clone()
    };

    let (command, args) = provider_command(config);
    let mut tmux_args = vec![
        "new-session".to_string(),
        "-d".to_string(),
        "-s".to_string(),
        tmux_session_name.to_string(),
        "-x".to_string(),
        cols.to_string(),
        "-y".to_string(),
        rows.to_string(),
        "-c".to_string(),
        working_dir,
        command,
    ];
    tmux_args.extend(args);

    run_tmux(tmux_args).await.map(|_| ())
}

fn provider_command(config: &SessionConfig) -> (String, Vec<String>) {
    let command = config
        .cli_path
        .clone()
        .unwrap_or_else(|| config.provider.default_command().to_string());

    match config.provider {
        CliProvider::Ollama if !config.model.trim().is_empty() => {
            (command, vec!["run".to_string(), config.model.clone()])
        }
        _ => (command, Vec::new()),
    }
}

async fn tmux_has_session(name: &str) -> bool {
    Command::new("tmux")
        .args(["has-session", "-t", name])
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .status()
        .await
        .map(|status| status.success())
        .unwrap_or(false)
}

async fn list_first_pane(tmux_session_name: &str) -> Result<String, String> {
    let output = run_tmux(vec![
        "list-panes".to_string(),
        "-t".to_string(),
        tmux_session_name.to_string(),
        "-F".to_string(),
        "#{pane_id}".to_string(),
    ])
    .await?;

    output
        .lines()
        .map(str::trim)
        .find(|line| !line.is_empty())
        .map(ToOwned::to_owned)
        .ok_or_else(|| "tmux session has no panes".to_string())
}

async fn resize_pane(pane_id: &str, cols: u16, rows: u16) -> Result<(), String> {
    run_tmux(vec![
        "resize-pane".to_string(),
        "-t".to_string(),
        pane_id.to_string(),
        "-x".to_string(),
        cols.max(20).to_string(),
        "-y".to_string(),
        rows.max(5).to_string(),
    ])
    .await
    .map(|_| ())
}

async fn pane_size(pane_id: &str) -> Result<(u16, u16), String> {
    let output = run_tmux(vec![
        "display-message".to_string(),
        "-p".to_string(),
        "-t".to_string(),
        pane_id.to_string(),
        "#{pane_width} #{pane_height}".to_string(),
    ])
    .await?;

    let mut parts = output.split_whitespace();
    let cols = parts
        .next()
        .ok_or_else(|| "tmux did not return pane width".to_string())?
        .parse::<u16>()
        .map_err(|e| format!("Invalid pane width: {}", e))?;
    let rows = parts
        .next()
        .ok_or_else(|| "tmux did not return pane height".to_string())?
        .parse::<u16>()
        .map_err(|e| format!("Invalid pane height: {}", e))?;
    Ok((cols, rows))
}

async fn capture_snapshot(
    state: &AppState,
    terminal: &TerminalSession,
) -> Result<TerminalSnapshot, String> {
    let capture = capture_terminal(&terminal.pane_id).await?;
    let cursor = state.db.with_conn(|conn| {
        conn.query_row(
            "SELECT tmux_screen_cursor FROM sessions WHERE id = ?1",
            params![&terminal.session_id],
            |row| row.get::<_, i64>(0),
        )
        .map_err(|e| e.to_string())
    })?;

    Ok(TerminalSnapshot {
        session_id: terminal.session_id.clone(),
        pane_id: terminal.pane_id.clone(),
        cursor,
        content: capture.content,
        cursor_x: capture.cursor_x,
        cursor_y: capture.cursor_y,
        history_lines: capture.history_lines,
        rows: terminal.rows,
        cols: terminal.cols,
        status: "attached".to_string(),
    })
}

async fn capture_terminal(pane_id: &str) -> Result<TerminalCapture, String> {
    let content = run_tmux(vec![
        "capture-pane".to_string(),
        "-p".to_string(),
        "-e".to_string(),
        "-N".to_string(),
        "-S".to_string(),
        "-2000".to_string(),
        "-t".to_string(),
        pane_id.to_string(),
    ])
    .await?;
    let (cursor_x, cursor_y, history_size) = pane_cursor_meta(pane_id).await?;
    Ok(TerminalCapture {
        content,
        cursor_x,
        cursor_y,
        history_lines: history_size.min(2000),
    })
}

async fn pane_cursor_meta(pane_id: &str) -> Result<(u16, u16, u16), String> {
    let output = run_tmux(vec![
        "display-message".to_string(),
        "-p".to_string(),
        "-t".to_string(),
        pane_id.to_string(),
        "#{cursor_x} #{cursor_y} #{history_size}".to_string(),
    ])
    .await?;

    let mut parts = output.split_whitespace();
    let cursor_x = parts
        .next()
        .ok_or_else(|| "tmux did not return cursor_x".to_string())?
        .parse::<u16>()
        .map_err(|e| format!("Invalid cursor_x: {}", e))?;
    let cursor_y = parts
        .next()
        .ok_or_else(|| "tmux did not return cursor_y".to_string())?
        .parse::<u16>()
        .map_err(|e| format!("Invalid cursor_y: {}", e))?;
    let history_size = parts
        .next()
        .ok_or_else(|| "tmux did not return history_size".to_string())?
        .parse::<u16>()
        .map_err(|e| format!("Invalid history_size: {}", e))?;
    Ok((cursor_x, cursor_y, history_size))
}

async fn send_raw_input(pane_id: &str, input: &str) -> Result<(), String> {
    if input == "\u{3}" {
        return run_tmux(vec![
            "send-keys".to_string(),
            "-t".to_string(),
            pane_id.to_string(),
            "C-c".to_string(),
        ])
        .await
        .map(|_| ());
    }

    if input.contains('\n') || input.len() > 512 {
        return paste_buffer(pane_id, input).await;
    }

    for segment in input.split_inclusive('\r') {
        let text = segment.trim_end_matches('\r');
        if !text.is_empty() {
            run_tmux(vec![
                "send-keys".to_string(),
                "-t".to_string(),
                pane_id.to_string(),
                "-l".to_string(),
                text.to_string(),
            ])
            .await?;
        }
        if segment.ends_with('\r') {
            run_tmux(vec![
                "send-keys".to_string(),
                "-t".to_string(),
                pane_id.to_string(),
                "Enter".to_string(),
            ])
            .await?;
        }
    }

    Ok(())
}

async fn paste_buffer(pane_id: &str, input: &str) -> Result<(), String> {
    let buffer_name = format!("johnnyone-{}", uuid::Uuid::new_v4());
    let mut child = Command::new("tmux")
        .args(["load-buffer", "-b", &buffer_name, "-"])
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .map_err(|e| format!("Failed to run tmux load-buffer: {}", e))?;

    if let Some(stdin) = child.stdin.as_mut() {
        stdin
            .write_all(input.as_bytes())
            .await
            .map_err(|e| format!("Failed to write tmux buffer: {}", e))?;
    }

    let output = child
        .wait_with_output()
        .await
        .map_err(|e| format!("Failed to wait for tmux load-buffer: {}", e))?;

    if !output.status.success() {
        return Err(String::from_utf8_lossy(&output.stderr).trim().to_string());
    }

    run_tmux(vec![
        "paste-buffer".to_string(),
        "-b".to_string(),
        buffer_name,
        "-t".to_string(),
        pane_id.to_string(),
    ])
    .await
    .map(|_| ())
}

async fn run_tmux(args: Vec<String>) -> Result<String, String> {
    let output = Command::new("tmux")
        .args(&args)
        .output()
        .await
        .map_err(|e| format!("Failed to run tmux {:?}: {}", args, e))?;

    if !output.status.success() {
        let stderr = String::from_utf8_lossy(&output.stderr).trim().to_string();
        return Err(if stderr.is_empty() {
            format!("tmux {:?} failed with status {}", args, output.status)
        } else {
            stderr
        });
    }

    Ok(String::from_utf8_lossy(&output.stdout).to_string())
}

fn tmux_session_name(session_id: &str) -> String {
    let safe = session_id
        .chars()
        .map(|ch| {
            if ch.is_ascii_alphanumeric() || ch == '-' || ch == '_' {
                ch
            } else {
                '_'
            }
        })
        .collect::<String>();
    format!("johnnyone_{}", safe)
}
