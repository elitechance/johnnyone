use crate::events::TerminalScreenEvent;
use crate::providers::CliProvider;
use crate::state::app_state::AppState;
use rusqlite::params;
use serde::Serialize;
use std::collections::HashMap;
use std::path::Path;
use std::process::Stdio;
use std::sync::Arc;
use tauri::Emitter;
use tokio::io::AsyncWriteExt;
use tokio::process::Command;
use std::time::Instant;
use tokio::time::{sleep, Duration};

const INBOX_DIR: &str = ".johnnyone/inbox";
/// Multiline or long prompts are written to a file and a one-line handoff is
/// typed into the TUI — avoids Codex `\n` literals and Grok paste blobs.
const FILE_HANDOFF_MIN_LEN: usize = 200;

const CAPTURE_INTERVAL_ACTIVE_MS: u64 = 2_000;
const CAPTURE_INTERVAL_IDLE_MS: u64 = 10_000;
const CAPTURE_ACTIVITY_WINDOW_MS: u128 = 3_000;
const CURSOR_ONLY_MIN_INTERVAL_MS: u128 = 2_000;
/// Minimum interval between `terminal_screen` relay events (all publish paths).
const MIN_TERMINAL_SCREEN_PUBLISH_MS: u128 = 2_000;
const DEFAULT_HISTORY_CAPTURE_LINES: u16 = 200;
const MAX_HISTORY_CAPTURE_LINES: u16 = 2000;

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct TerminalSnapshot {
    pub session_id: String,
    pub tmux_session_name: String,
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
    tmux_session_name: String,
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
    publish_snapshot(state, &snapshot, Some(&app_handle), true).await;
    start_capture_loop(state, Some(app_handle), terminal).await;
    Ok(snapshot)
}

pub async fn attach_terminal_headless(
    state: &AppState,
    session_id: String,
    cols: u16,
    rows: u16,
) -> Result<TerminalSnapshot, String> {
    subscribe_terminal_visual(state, session_id, cols, rows).await
}

pub async fn subscribe_terminal_visual(
    state: &AppState,
    session_id: String,
    cols: u16,
    rows: u16,
) -> Result<TerminalSnapshot, String> {
    {
        let mut subscribers = state.terminal_visual_subscribers.lock().await;
        let count = subscribers.entry(session_id.clone()).or_insert(0);
        *count += 1;
    }

    let terminal = ensure_terminal_session(state, &session_id, cols, rows).await?;
    let snapshot = capture_snapshot(state, &terminal).await?;
    publish_snapshot(state, &snapshot, None, true).await;
    start_capture_loop(state, None, terminal).await;
    Ok(snapshot)
}

pub async fn refresh_terminal_visual(
    state: &AppState,
    session_id: String,
    cols: u16,
    rows: u16,
) -> Result<TerminalSnapshot, String> {
    let terminal = ensure_terminal_session(state, &session_id, cols, rows).await?;
    let snapshot = capture_snapshot(state, &terminal).await?;
    publish_snapshot(state, &snapshot, None, false).await;
    if has_terminal_visual_subscribers(state, &session_id).await {
        start_capture_loop(state, None, terminal).await;
    }
    Ok(snapshot)
}

pub async fn refresh_terminal_visual_with_history(
    state: &AppState,
    session_id: String,
    cols: u16,
    rows: u16,
    history_rows: u16,
) -> Result<TerminalSnapshot, String> {
    let terminal = ensure_terminal_session(state, &session_id, cols, rows).await?;
    let snapshot = capture_snapshot_with_history_rows(state, &terminal, history_rows).await?;
    publish_snapshot(state, &snapshot, None, false).await;
    Ok(snapshot)
}

pub async fn unsubscribe_terminal_visual(state: &AppState, session_id: &str) -> Result<(), String> {
    let should_stop = {
        let mut subscribers = state.terminal_visual_subscribers.lock().await;
        match subscribers.get_mut(session_id) {
            Some(count) if *count > 1 => {
                *count -= 1;
                false
            }
            Some(_) => {
                subscribers.remove(session_id);
                true
            }
            None => false,
        }
    };

    if should_stop {
        stop_terminal_capture(state, session_id).await;
    }

    Ok(())
}

async fn has_terminal_visual_subscribers(state: &AppState, session_id: &str) -> bool {
    state
        .terminal_visual_subscribers
        .lock()
        .await
        .get(session_id)
        .copied()
        .unwrap_or(0)
        > 0
}

pub async fn send_terminal_input(
    state: &AppState,
    session_id: String,
    input: String,
) -> Result<(), String> {
    let config = load_session_config(state, &session_id)?;
    let terminal = ensure_terminal_session_for_input(state, &session_id).await?;
    send_raw_input(
        &terminal.pane_id,
        &input,
        config.provider,
        &config.working_directory,
    )
    .await?;
    state
        .terminal_last_input_at
        .lock()
        .await
        .insert(session_id, Instant::now());
    Ok(())
}

pub async fn resize_terminal(
    state: &AppState,
    session_id: String,
    cols: u16,
    rows: u16,
) -> Result<(), String> {
    let terminal = ensure_terminal_session(state, &session_id, cols, rows).await?;
    resize_pane(&terminal.pane_id, cols, rows).await?;
    if has_terminal_visual_subscribers(state, &session_id).await {
        start_capture_loop(state, None, terminal).await;
    }
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
    state
        .terminal_visual_subscribers
        .lock()
        .await
        .remove(session_id);

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

pub async fn capture_terminal_session(
    state: &AppState,
    session_id: &str,
) -> Result<TerminalSnapshot, String> {
    let terminal = ensure_terminal_session_for_input(state, session_id).await?;
    capture_snapshot(state, &terminal).await
}

pub async fn capture_terminal_session_with_history(
    state: &AppState,
    session_id: &str,
) -> Result<TerminalSnapshot, String> {
    let terminal = ensure_terminal_session_for_input(state, session_id).await?;
    capture_snapshot_with_history(state, &terminal).await
}

async fn publish_snapshot(
    state: &AppState,
    snapshot: &TerminalSnapshot,
    app_handle: Option<&tauri::AppHandle>,
    bypass_throttle: bool,
) {
    let event = TerminalScreenEvent {
        session_id: snapshot.session_id.clone(),
        tmux_session_name: snapshot.tmux_session_name.clone(),
        pane_id: snapshot.pane_id.clone(),
        cursor: snapshot.cursor,
        content: snapshot.content.clone(),
        cursor_x: snapshot.cursor_x,
        cursor_y: snapshot.cursor_y,
        history_lines: snapshot.history_lines,
        rows: snapshot.rows,
        cols: snapshot.cols,
        status: snapshot.status.clone(),
    };

    publish_terminal_screen(state, event, app_handle.cloned(), bypass_throttle).await;
}

async fn publish_terminal_screen(
    state: &AppState,
    event: TerminalScreenEvent,
    app_handle: Option<tauri::AppHandle>,
    bypass_throttle: bool,
) {
    publish_terminal_screen_throttled(
        state.terminal_last_screen_publish_at.clone(),
        state.terminal_pending_screen.clone(),
        state.terminal_screen_flush_scheduled.clone(),
        state.terminal_screen_tx.clone(),
        event,
        app_handle,
        bypass_throttle,
    )
    .await;
}

async fn publish_terminal_screen_throttled(
    last_publish: Arc<tokio::sync::Mutex<HashMap<String, Instant>>>,
    pending: Arc<tokio::sync::Mutex<HashMap<String, TerminalScreenEvent>>>,
    flush_scheduled: Arc<tokio::sync::Mutex<HashMap<String, bool>>>,
    screen_tx: tokio::sync::broadcast::Sender<TerminalScreenEvent>,
    event: TerminalScreenEvent,
    app_handle: Option<tauri::AppHandle>,
    bypass_throttle: bool,
) {
    let session_id = event.session_id.clone();
    let now = Instant::now();

    if !bypass_throttle {
        let mut last = last_publish.lock().await;
        if let Some(previous) = last.get(&session_id) {
            let elapsed = now.duration_since(*previous).as_millis();
            if elapsed < MIN_TERMINAL_SCREEN_PUBLISH_MS {
                pending.lock().await.insert(session_id.clone(), event);
                let delay = MIN_TERMINAL_SCREEN_PUBLISH_MS - elapsed;
                drop(last);
                schedule_terminal_screen_flush(
                    last_publish,
                    pending,
                    flush_scheduled,
                    screen_tx,
                    session_id,
                    delay,
                )
                .await;
                return;
            }
        }
        last.insert(session_id.clone(), now);
    } else {
        last_publish
            .lock()
            .await
            .insert(session_id.clone(), now);
        pending.lock().await.remove(&session_id);
    }

    let _ = screen_tx.send(event.clone());
    if let Some(handle) = app_handle {
        let _ = handle.emit("terminal:screen", event);
    }
}

async fn schedule_terminal_screen_flush(
    last_publish: Arc<tokio::sync::Mutex<HashMap<String, Instant>>>,
    pending: Arc<tokio::sync::Mutex<HashMap<String, TerminalScreenEvent>>>,
    flush_scheduled: Arc<tokio::sync::Mutex<HashMap<String, bool>>>,
    screen_tx: tokio::sync::broadcast::Sender<TerminalScreenEvent>,
    session_id: String,
    delay_ms: u128,
) {
    {
        let mut scheduled = flush_scheduled.lock().await;
        if scheduled.get(&session_id).copied().unwrap_or(false) {
            return;
        }
        scheduled.insert(session_id.clone(), true);
    }

    tokio::spawn(async move {
        tokio::time::sleep(Duration::from_millis(delay_ms.max(1) as u64)).await;

        let event = pending.lock().await.remove(&session_id);
        flush_scheduled.lock().await.remove(&session_id);

        if let Some(event) = event {
            last_publish
                .lock()
                .await
                .insert(session_id, Instant::now());
            let _ = screen_tx.send(event);
        }
    });
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
    let visual_subscribers = state.terminal_visual_subscribers.clone();
    let terminal_last_input_at = state.terminal_last_input_at.clone();
    let last_screen_publish = state.terminal_last_screen_publish_at.clone();
    let pending_screen = state.terminal_pending_screen.clone();
    let flush_scheduled = state.terminal_screen_flush_scheduled.clone();
    let screen_tx = state.terminal_screen_tx.clone();
    let session_id = terminal.session_id.clone();
    let handle = tokio::spawn(async move {
        let mut last_content_key = String::new();
        let mut last_published_cursor_key = String::new();
        let mut last_publish_at = Instant::now() - Duration::from_secs(10);
        let mut cursor: i64 = 0;

        loop {
            let subscriber_count = visual_subscribers
                .lock()
                .await
                .get(&terminal.session_id)
                .copied()
                .unwrap_or(0);
            if subscriber_count == 0 {
                tracing::debug!(
                    session_id = %terminal.session_id,
                    "stopping terminal capture loop without visual subscribers"
                );
                break;
            }

            match capture_terminal(&terminal.pane_id).await {
                Ok(capture) => {
                    let content_key = capture.content.clone();
                    let cursor_key = format!("{}:{}", capture.cursor_x, capture.cursor_y);
                    let content_changed = content_key != last_content_key;
                    let cursor_changed = cursor_key != last_published_cursor_key;
                    let now = Instant::now();
                    let cursor_only_due = now.duration_since(last_publish_at).as_millis()
                        >= CURSOR_ONLY_MIN_INTERVAL_MS;
                    let should_publish = content_changed
                        || (cursor_changed && cursor_only_due);

                    if should_publish {
                        if content_changed {
                            last_content_key = content_key;
                        }
                        last_published_cursor_key = cursor_key;
                        last_publish_at = now;
                        cursor += 1;

                        let event = TerminalScreenEvent {
                            session_id: terminal.session_id.clone(),
                            tmux_session_name: terminal.tmux_session_name.clone(),
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

                        publish_terminal_screen_throttled(
                            last_screen_publish.clone(),
                            pending_screen.clone(),
                            flush_scheduled.clone(),
                            screen_tx.clone(),
                            event,
                            app_handle.clone(),
                            false,
                        )
                        .await;
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

            let interval_ms = {
                let last_input = terminal_last_input_at
                    .lock()
                    .await
                    .get(&terminal.session_id)
                    .copied();
                match last_input {
                    Some(at) if at.elapsed().as_millis() < CAPTURE_ACTIVITY_WINDOW_MS => {
                        CAPTURE_INTERVAL_ACTIVE_MS
                    }
                    _ => CAPTURE_INTERVAL_IDLE_MS,
                }
            };
            sleep(Duration::from_millis(interval_ms)).await;
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
            "UPDATE sessions SET tmux_session_name = ?1, tmux_pane_id = ?2, terminal_status = 'attached', status = 'active', updated_at = datetime('now') WHERE id = ?3",
            params![&tmux_session_name, &pane_id, session_id],
        )
        .map_err(|e| e.to_string())
    })?;

    Ok(TerminalSession {
        session_id: config.session_id,
        tmux_session_name,
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
            "UPDATE sessions SET tmux_session_name = ?1, tmux_pane_id = ?2, terminal_status = 'attached', status = 'active', updated_at = datetime('now') WHERE id = ?3",
            params![&tmux_session_name, &pane_id, session_id],
        )
        .map_err(|e| e.to_string())
    })?;

    Ok(TerminalSession {
        session_id: config.session_id,
        tmux_session_name,
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
    // For Shell, prefer the user's $SHELL over the static default ("bash"),
    // so people on zsh/fish get their actual login shell.
    let command = if matches!(config.provider, CliProvider::Shell) {
        config
            .cli_path
            .clone()
            .filter(|p| !p.trim().is_empty())
            .or_else(|| std::env::var("SHELL").ok())
            .unwrap_or_else(|| config.provider.default_command().to_string())
    } else {
        config
            .cli_path
            .clone()
            .unwrap_or_else(|| config.provider.default_command().to_string())
    };

    match config.provider {
        CliProvider::ClaudeCode => (
            command,
            vec![
                "--dangerously-skip-permissions".to_string(),
                "--permission-mode".to_string(),
                "bypassPermissions".to_string(),
            ],
        ),
        CliProvider::Codex => (
            command,
            vec!["--dangerously-bypass-approvals-and-sandbox".to_string()],
        ),
        CliProvider::Ollama if !config.model.trim().is_empty() => {
            (command, vec!["run".to_string(), config.model.clone()])
        }
        // grok TUI — auto-approve tool executions so the pane isn't blocked on
        // approval prompts, matching the bypass behavior of the other TUIs.
        // Inline mode keeps output in tmux scrollback so relay capture + small
        // xterm viewports can still show responses on narrow screens.
        CliProvider::Grok => {
            let mut args = vec![
                "--always-approve".to_string(),
                "--no-alt-screen".to_string(),
            ];
            if !config.model.trim().is_empty() {
                args.push("-m".to_string());
                args.push(config.model.clone());
            }
            (command, args)
        }
        // Plain shell — no args. tmux will run it as the pane's command and
        // the user types whatever they want.
        CliProvider::Shell => (command, Vec::new()),
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
    snapshot_from_capture(state, terminal, capture).await
}

async fn capture_snapshot_with_history(
    state: &AppState,
    terminal: &TerminalSession,
) -> Result<TerminalSnapshot, String> {
    capture_snapshot_with_history_rows(state, terminal, DEFAULT_HISTORY_CAPTURE_LINES).await
}

async fn capture_snapshot_with_history_rows(
    state: &AppState,
    terminal: &TerminalSession,
    history_rows: u16,
) -> Result<TerminalSnapshot, String> {
    let capture = capture_terminal_with_history(&terminal.pane_id, history_rows).await?;
    snapshot_from_capture(state, terminal, capture).await
}

async fn snapshot_from_capture(
    state: &AppState,
    terminal: &TerminalSession,
    capture: TerminalCapture,
) -> Result<TerminalSnapshot, String> {
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
        tmux_session_name: terminal.tmux_session_name.clone(),
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

async fn capture_terminal_with_history(
    pane_id: &str,
    history_rows: u16,
) -> Result<TerminalCapture, String> {
    let start = format!(
        "-{}",
        history_rows.clamp(1, MAX_HISTORY_CAPTURE_LINES),
    );
    let content = run_tmux(vec![
        "capture-pane".to_string(),
        "-p".to_string(),
        "-e".to_string(),
        "-N".to_string(),
        "-J".to_string(),
        "-S".to_string(),
        start,
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

fn normalize_terminal_input(input: &str) -> String {
    if input.contains('\n') || input.contains('\r') {
        return input.to_string();
    }
    if input.contains("\\n") || input.contains("\\t") {
        return input
            .replace("\\r\\n", "\n")
            .replace("\\n", "\n")
            .replace("\\t", "\t");
    }
    input.to_string()
}

fn should_use_file_handoff(text: &str, provider: CliProvider) -> bool {
    if matches!(provider, CliProvider::Shell) {
        return false;
    }
    text.contains('\n') || text.len() >= FILE_HANDOFF_MIN_LEN
}

fn write_inbox_prompt(working_dir: &str, text: &str) -> Result<String, String> {
    let workspace = Path::new(working_dir);
    let inbox = workspace.join(INBOX_DIR);
    std::fs::create_dir_all(&inbox)
        .map_err(|e| format!("Failed to create {}: {}", inbox.display(), e))?;
    let file_name = format!("{}.md", uuid::Uuid::new_v4());
    let path = inbox.join(&file_name);
    std::fs::write(&path, text).map_err(|e| format!("Failed to write {}: {}", path.display(), e))?;
    Ok(path
        .strip_prefix(workspace)
        .unwrap_or(&path)
        .to_string_lossy()
        .to_string())
}

async fn send_file_handoff_input(
    pane_id: &str,
    working_dir: &str,
    input: &str,
    provider: CliProvider,
) -> Result<(), String> {
    let submit = input.ends_with('\r') || input.ends_with('\n');
    let text = input.trim_end_matches(['\r', '\n']);
    let rel_path = write_inbox_prompt(working_dir, text)?;
    let msg = format!(
        "Read and follow the instructions in {rel_path}. Use your file-read tool on that path before acting."
    );
    send_single_line_input(pane_id, &format!("{msg}\r"), provider, submit).await
}

async fn send_single_line_input(
    pane_id: &str,
    input: &str,
    provider: CliProvider,
    submit: bool,
) -> Result<(), String> {
    for segment in input.split_inclusive('\r') {
        let text = segment.trim_end_matches('\r');
        if !text.is_empty() {
            run_tmux(vec![
                "send-keys".to_string(),
                "-t".to_string(),
                pane_id.to_string(),
                "-l".to_string(),
                "--".to_string(),
                text.to_string(),
            ])
            .await?;
        }
        if segment.ends_with('\r') && submit {
            if provider == CliProvider::Codex {
                send_submit_key(pane_id).await?;
                sleep(Duration::from_millis(200)).await;
                send_submit_key(pane_id).await?;
            } else {
                send_submit_key(pane_id).await?;
            }
        }
    }
    Ok(())
}

async fn send_raw_input(
    pane_id: &str,
    input: &str,
    provider: CliProvider,
    working_directory: &str,
) -> Result<(), String> {
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

    let input = normalize_terminal_input(input);
    let submit = input.ends_with('\r') || input.ends_with('\n');
    let body = input.trim_end_matches(['\r', '\n']);

    if should_use_file_handoff(body, provider) {
        return send_file_handoff_input(pane_id, working_directory, &input, provider).await;
    }

    // Shell-only fallback: other providers use inbox file handoff above.
    if input.contains('\n') || input.len() > 512 {
        let text = input.trim_end_matches(['\r', '\n']);
        if !text.is_empty() {
            paste_buffer(pane_id, text).await?;
        }
        if submit {
            sleep(Duration::from_millis(250)).await;
            run_tmux(vec![
                "send-keys".to_string(),
                "-t".to_string(),
                pane_id.to_string(),
                "C-m".to_string(),
            ])
            .await?;
        }
        return Ok(());
    }

    send_single_line_input(pane_id, &input, provider, submit).await
}

async fn send_submit_key(pane_id: &str) -> Result<(), String> {
    run_tmux(vec![
        "send-keys".to_string(),
        "-t".to_string(),
        pane_id.to_string(),
        "C-m".to_string(),
    ])
    .await
    .map(|_| ())
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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn normalize_unescapes_json_newlines() {
        let input = "line one\\n\\nline two";
        assert_eq!(normalize_terminal_input(input), "line one\n\nline two");
    }

    #[test]
    fn file_handoff_for_multiline_cli_providers() {
        assert!(should_use_file_handoff("a\nb", CliProvider::Grok));
        assert!(!should_use_file_handoff("a\nb", CliProvider::Shell));
    }

    #[test]
    fn file_handoff_for_long_single_line() {
        let long = "x".repeat(FILE_HANDOFF_MIN_LEN);
        assert!(should_use_file_handoff(&long, CliProvider::Codex));
    }
}
