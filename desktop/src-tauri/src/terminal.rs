use crate::db::Database;
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

/// The one SET clause that detaches a terminal row, shared by every path that writes it.
///
/// Three callers now agree on it: [`kill_terminal_session`], the capture loop's failure break, and
/// the batched [`demote_terminal_rows`]. Hoisted rather than left as three literals because they
/// had already drifted — the capture-failure write cleared `terminal_status` but NOT
/// `tmux_pane_id`, so a row sat `'detached'` while still naming a dead pane until the next host
/// restart reconciled it. A detached row must never carry a pane id.
const DETACH_TERMINAL_SET: &str =
    "terminal_status = 'detached', tmux_pane_id = NULL, updated_at = datetime('now')";

const INBOX_DIR: &str = ".johnnyone/inbox";
/// Multiline or long prompts are written to a file and a one-line handoff is
/// typed into the TUI — avoids Codex `\n` literals and Grok paste blobs.
const FILE_HANDOFF_MIN_LEN: usize = 200;

const CAPTURE_INTERVAL_ACTIVE_MS: u64 = 500;
const CAPTURE_INTERVAL_IDLE_MS: u64 = 10_000;
const CAPTURE_ACTIVITY_WINDOW_MS: u128 = 3_000;
const CURSOR_ONLY_MIN_INTERVAL_MS: u128 = 500;
/// Minimum interval between `terminal_screen` relay events (all publish paths).
/// This throttle bounds Cloudflare Durable Object traffic — it is a cost control,
/// not a rendering detail. Keep the mechanism; only the value is tuned. At 500ms a
/// continuously-changing pane publishes at most 2 events/sec per subscribed session
/// (4x the previous 2s ceiling). The idle cadence above is what keeps a quiet
/// session cheap, so lower that one only deliberately.
const MIN_TERMINAL_SCREEN_PUBLISH_MS: u128 = 500;
const DEFAULT_HISTORY_CAPTURE_LINES: u16 = 200;
const MAX_HISTORY_CAPTURE_LINES: u16 = 2000;

/// Clamp a client-requested history-capture depth (from `captureTerminal(sessionId, historyLines)`) to
/// the supported window. `None` ⇒ the default depth; a present value is bounded to
/// `[1, MAX_HISTORY_CAPTURE_LINES]` so a zero, a huge, or a `> u16` request can neither no-op nor
/// overflow the capture. Pure/deterministic — this is the seam the C2b console scrollback depth flows
/// through, and the regression guard for "the 1500 request silently captured only 200".
pub fn clamp_history_capture_lines(requested: Option<u64>) -> u16 {
    match requested {
        None => DEFAULT_HISTORY_CAPTURE_LINES,
        Some(n) => n.clamp(1, MAX_HISTORY_CAPTURE_LINES as u64) as u16,
    }
}
/// Live relay captures include recent tmux scrollback so mobile viewports still
/// receive Grok/TUI responses that scrolled above the visible pane.
const LIVE_CAPTURE_SCROLLBACK_LINES: u16 = 120;

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
    /// For shell sessions: commands to run in the pane on first spawn.
    setup_commands: Option<String>,
    /// When true, this session attaches to the EXTERNAL tmux session named in
    /// `tmux_session_name` instead of spawning a `johnnyone_<id>` pane.
    attached_tmux: bool,
    /// The stored tmux session name (the external name when `attached_tmux`).
    tmux_session_name: Option<String>,
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
    // Explicit refresh is a direct user/UI request — bypass the publish throttle
    // so the snapshot shows immediately instead of being delayed up to ~2s.
    publish_snapshot(state, &snapshot, None, true).await;
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
    // Explicit history fetch — bypass the throttle so it is not delayed.
    publish_snapshot(state, &snapshot, None, true).await;
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
        .insert(session_id.clone(), Instant::now());
    // Wake/restart the capture loop so the echoed input and its output are
    // captured at the active cadence right away, instead of waiting out the
    // current (possibly idle, up to ~10s) sleep before the next tick.
    if has_terminal_visual_subscribers(state, &session_id).await {
        start_capture_loop(state, None, terminal).await;
    }
    Ok(())
}

/// Send named keys (Escape, Down, C-c …) to a session's pane.
///
/// Separate from `send_terminal_input` because these are KEYS, not text: they go to `send-keys`
/// WITHOUT `-l`, so tmux resolves each name against the pane's current mode. That is the whole point
/// — an arrow is `ESC [ A` in normal mode and `ESC O A` in application cursor mode, and the agent
/// CLIs run in the latter, so literal bytes would be wrong half the time.
///
/// The whole sequence goes in one `send-keys` call so a menu answer like Down Down Enter cannot be
/// interleaved with anything else.
pub async fn send_terminal_keys(
    state: &AppState,
    session_id: String,
    keys: &[String],
) -> Result<(), String> {
    if keys.is_empty() {
        return Ok(());
    }
    let terminal = ensure_terminal_session_for_input(state, &session_id).await?;

    run_tmux(send_keys_named_args(&terminal.pane_id, keys)).await?;

    state
        .terminal_last_input_at
        .lock()
        .await
        .insert(session_id.clone(), Instant::now());
    // Same reason as text input: wake the capture loop so the effect of the key shows up at the
    // active cadence instead of after an idle sleep.
    if has_terminal_visual_subscribers(state, &session_id).await {
        start_capture_loop(state, None, terminal).await;
    }
    Ok(())
}

pub async fn resize_terminal(
    state: &AppState,
    session_id: String,
    cols: u16,
    rows: u16,
) -> Result<(), String> {
    let terminal = ensure_terminal_session(state, &session_id, cols, rows).await?;
    // Never resize an EXTERNAL tmux pane — a tmux session has one shared size,
    // so resizing here would squash the user's own attached client to the web
    // viewport. Same rule `ensure_terminal_session` already applies; this path
    // (the xterm resize observer) was missing it.
    if !load_session_config(state, &session_id)?.attached_tmux {
        resize_pane(&terminal.pane_id, cols, rows).await?;
    }
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
            tmux_session_target(&tmux_session_name),
        ])
        .await
        .map(|_| ())?;
    }

    detach_terminal_row(&state.db, session_id)?;

    Ok(())
}

/// One row whose `terminal_status` claims `'attached'`, as startup reconciliation reads it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AttachedTerminalRow {
    pub session_id: String,
    pub attached_tmux: bool,
    pub tmux_session_name: Option<String>,
}

/// Which rows' `terminal_status = 'attached'` is a lie, given the tmux session names that exist.
///
/// Pure, because the failure mode worth pinning is mass-demoting the user's live shells. The
/// question asked of every row is the same one `ensure_terminal_session` asks — "does a tmux
/// session with EXACTLY this name exist right now?" — so an owned `johnnyone_<id>` row and a row
/// attached to the user's external tmux need no separate rules: an owned session's tmux is always
/// gone after a host restart and so always answers no, while the user's `kloo` may legitimately
/// still be running and so answers yes. One rule, two outcomes.
///
/// `live_names = None` means tmux could not be ASKED (no binary, no server) — the `Err` arm of
/// [`tmux_session_names`], deliberately distinct from `Ok(names)` not containing a name. Then
/// NOTHING is demoted: wiping every row because the tmux server happened to be down would be
/// strictly worse than the stale bookkeeping this exists to clear.
fn terminal_rows_to_demote(
    rows: &[AttachedTerminalRow],
    live_names: Option<&[String]>,
) -> Vec<String> {
    let Some(live) = live_names else {
        return Vec::new();
    };
    rows.iter()
        .filter(|row| {
            match expected_tmux_session_name(row) {
                // An attached row with no usable name can never be captured (`resolve_tmux_target`
                // errors on it), so its 'attached' is unconditionally stale.
                None => true,
                // Exact string equality, never tmux's target parser: `has-session -t kloo`
                // prefix-matches and would rescue a row whose session had already exited.
                Some(name) => !live.iter().any(|candidate| *candidate == name),
            }
        })
        .map(|row| row.session_id.clone())
        .collect()
}

/// The tmux session name a row's terminal lives under — the same derivation
/// [`resolve_tmux_target`] performs, over the raw row instead of a loaded `SessionConfig`.
fn expected_tmux_session_name(row: &AttachedTerminalRow) -> Option<String> {
    if row.attached_tmux {
        row.tmux_session_name
            .as_deref()
            .map(str::trim)
            .filter(|name| !name.is_empty())
            .map(ToOwned::to_owned)
    } else {
        Some(tmux_session_name(&row.session_id))
    }
}

/// Detach ONE terminal row: the end state every detach path must produce.
///
/// The seam both live-process detach paths share, so neither can drift from the other or from the
/// startup reconciler. Takes `&Database` rather than `&AppState` because the capture loop holds
/// only a cloned `Database`.
fn detach_terminal_row(db: &Database, session_id: &str) -> Result<(), String> {
    db.with_conn(|conn| {
        conn.execute(
            &format!("UPDATE sessions SET {DETACH_TERMINAL_SET} WHERE id = ?1"),
            params![session_id],
        )
        .map_err(|e| e.to_string())
    })
    .map(|_| ())
}

/// Write the demotion: `terminal_status = 'detached'`, `tmux_pane_id = NULL`, nothing else.
///
/// One batched statement per chunk, not one round trip per row. Chunked under SQLite's default
/// 999-parameter ceiling so a long-lived database cannot overflow the statement. Separate from
/// [`reconcile_terminal_status_on_startup`] so the SQL can be exercised against a real database
/// without a tmux server deciding the outcome.
fn demote_terminal_rows(state: &AppState, session_ids: &[String]) -> Result<(), String> {
    if session_ids.is_empty() {
        return Ok(());
    }
    state.db.with_conn(|conn| {
        for chunk in session_ids.chunks(500) {
            let placeholders = vec!["?"; chunk.len()].join(",");
            conn.execute(
                &format!(
                    "UPDATE sessions SET {DETACH_TERMINAL_SET} WHERE id IN ({placeholders})"
                ),
                rusqlite::params_from_iter(chunk.iter()),
            )
            .map_err(|e| e.to_string())?;
        }
        Ok(())
    })
}

/// Reconcile `terminal_status` against reality once, at host startup.
///
/// Nothing in the running system ever demoted a row that was not re-subscribed: the only writes of
/// `'detached'` are [`kill_terminal_session`] and the capture loop's failure break, both driven by a
/// live process. A host restart kills every `johnnyone_*` pane without running either, so rows
/// accumulate claiming `'attached'` forever and `terminal_status` stops meaning anything. This is
/// the missing reconcile-on-startup pass.
///
/// Touches `terminal_status` and `tmux_pane_id` only — the same end state `kill_terminal_session`
/// writes. Never `status`; nothing is archived or deleted.
///
/// Returns the number of rows demoted.
pub async fn reconcile_terminal_status_on_startup(state: &AppState) -> Result<usize, String> {
    let rows: Vec<AttachedTerminalRow> = state.db.with_conn(|conn| {
        let mut stmt = conn
            .prepare(
                "SELECT id, attached_tmux, tmux_session_name FROM sessions WHERE terminal_status = 'attached'",
            )
            .map_err(|e| e.to_string())?;
        let rows = stmt
            .query_map([], |row| {
                Ok(AttachedTerminalRow {
                    session_id: row.get::<_, String>(0)?,
                    attached_tmux: row.get::<_, i64>(1)? != 0,
                    tmux_session_name: row.get::<_, Option<String>>(2)?,
                })
            })
            .map_err(|e| e.to_string())?
            .collect::<Result<Vec<_>, _>>()
            .map_err(|e| e.to_string())?;
        Ok(rows)
    })?;

    // A clean boot never shells out to tmux and never logs.
    if rows.is_empty() {
        return Ok(0);
    }

    let live = match tmux_session_names().await {
        Ok(names) => names,
        Err(error) => {
            // Not a clean boot and not silent: say why nothing was reconciled.
            tracing::warn!(
                attached_rows = rows.len(),
                %error,
                "tmux could not be asked which sessions exist; left terminal_status untouched"
            );
            return Ok(0);
        }
    };

    let stale = terminal_rows_to_demote(&rows, Some(&live));
    if stale.is_empty() {
        return Ok(0);
    }

    demote_terminal_rows(state, &stale)?;

    tracing::info!(
        demoted = stale.len(),
        attached_rows = rows.len(),
        live_tmux_sessions = live.len(),
        "Reconciled stale terminal_status='attached' rows to detached at startup"
    );
    Ok(stale.len())
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

/// Capture a session's pane WITH an explicit scrollback depth (C2b). The console primary pane requests
/// `CONSOLE_CAPTURE_LINES` (1500) so the home-anchored repaint carries real history the user can scroll
/// within; coordinator callers keep the default-depth `capture_terminal_session_with_history`. `history_rows`
/// should already be clamped via `clamp_history_capture_lines`; the downstream tmux `-<rows>` is clamped
/// again defensively.
pub async fn capture_terminal_session_with_history_rows(
    state: &AppState,
    session_id: &str,
    history_rows: u16,
) -> Result<TerminalSnapshot, String> {
    let terminal = ensure_terminal_session_for_input(state, session_id).await?;
    capture_snapshot_with_history_rows(state, &terminal, history_rows).await
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
        // Track when the pane content last changed so we can poll fast while the
        // agent is actively streaming output — even when that output came from a
        // chat message rather than direct terminal input.
        let mut last_content_change_at = Instant::now() - Duration::from_secs(60);
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

                    if content_changed {
                        last_content_change_at = now;
                    }
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
                    // Same end state as `kill_terminal_session`: a detached row must not keep a
                    // pane id pointing at the pane that just died.
                    let _ = detach_terminal_row(&db, &terminal.session_id);
                    break;
                }
            }

            let interval_ms = {
                let last_input = terminal_last_input_at
                    .lock()
                    .await
                    .get(&terminal.session_id)
                    .copied();
                let recent_input = last_input
                    .map(|at| at.elapsed().as_millis() < CAPTURE_ACTIVITY_WINDOW_MS)
                    .unwrap_or(false);
                // Poll fast while the agent is actively streaming output (the pane
                // keeps changing), not only right after direct terminal input —
                // chat-driven output otherwise refreshed at the 10s idle cadence.
                let recent_change =
                    last_content_change_at.elapsed().as_millis() < CAPTURE_ACTIVITY_WINDOW_MS;
                if recent_input || recent_change {
                    CAPTURE_INTERVAL_ACTIVE_MS
                } else {
                    CAPTURE_INTERVAL_IDLE_MS
                }
            };
            sleep(Duration::from_millis(interval_ms)).await;
        }
    });

    let mut tasks = state.terminal_capture_tasks.lock().await;
    tasks.insert(session_id, handle);
}

/// True when the session row is `archived`. A subscribe/refresh/input that races
/// an archive must NOT re-create the tmux or flip the row back to `active` — that
/// resurrection is why "closed" terminals reappeared after a refresh.
fn session_is_archived(state: &AppState, session_id: &str) -> bool {
    state
        .db
        .with_conn(|conn| {
            conn.query_row(
                "SELECT status FROM sessions WHERE id = ?1",
                params![session_id],
                |row| row.get::<_, String>(0),
            )
            .map_err(|e| e.to_string())
        })
        .map(|status| status == "archived")
        .unwrap_or(false)
}

async fn ensure_terminal_session(
    state: &AppState,
    session_id: &str,
    cols: u16,
    rows: u16,
) -> Result<TerminalSession, String> {
    if session_is_archived(state, session_id) {
        return Err(format!(
            "session {session_id} is archived; refusing to resurrect it"
        ));
    }
    let config = load_session_config(state, session_id)?;
    let (tmux_session_name, attached) = resolve_tmux_target(&config)?;

    if !tmux_has_session(&tmux_session_name).await {
        if attached {
            // Attached view of an external tmux that has since gone away — don't
            // spawn a replacement under its name; report it as not running.
            return Err(format!(
                "external tmux session '{tmux_session_name}' is not running"
            ));
        }
        if let Err(error) = create_tmux_session(&tmux_session_name, &config, cols, rows).await {
            if !tmux_has_session(&tmux_session_name).await {
                return Err(error);
            }
        }
    }

    let pane_id = list_first_pane(&tmux_session_name).await?;
    // Don't resize an external tmux pane — that would fight the user's own
    // terminal (tmux sessions have a single shared size). Capture it as-is.
    if !attached {
        resize_pane(&pane_id, cols, rows).await?;
    }

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
    if session_is_archived(state, session_id) {
        return Err(format!(
            "session {session_id} is archived; refusing to resurrect it"
        ));
    }
    let config = load_session_config(state, session_id)?;
    let (tmux_session_name, attached) = resolve_tmux_target(&config)?;

    if !tmux_has_session(&tmux_session_name).await {
        if attached {
            return Err(format!(
                "external tmux session '{tmux_session_name}' is not running"
            ));
        }
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
    let (provider_str, model, working_directory, setup_commands, tmux_session_name, attached_tmux) =
        state.db.with_conn(|conn| {
            conn.query_row(
                "SELECT provider, model, working_directory, setup_commands, tmux_session_name, attached_tmux FROM sessions WHERE id = ?1",
                params![session_id],
                |row| {
                    Ok((
                        row.get::<_, String>(0)?,
                        row.get::<_, String>(1)?,
                        row.get::<_, String>(2)?,
                        row.get::<_, Option<String>>(3)?,
                        row.get::<_, Option<String>>(4)?,
                        row.get::<_, i64>(5)? != 0,
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
        setup_commands: setup_commands.filter(|s| !s.trim().is_empty()),
        attached_tmux,
        tmux_session_name,
    })
}

/// Resolve the tmux session name + whether it's external (attached) for a
/// session: an attached session uses its stored external name and must NOT be
/// (re)created; an owned session uses `johnnyone_<id>`.
fn resolve_tmux_target(config: &SessionConfig) -> Result<(String, bool), String> {
    if config.attached_tmux {
        let name = config
            .tmux_session_name
            .clone()
            .filter(|s| !s.trim().is_empty())
            .ok_or_else(|| "attached session has no tmux_session_name".to_string())?;
        Ok((name, true))
    } else {
        Ok((tmux_session_name(&config.session_id), false))
    }
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

    run_tmux(tmux_args).await?;

    // Shell sessions with setup commands: type them into the freshly-spawned
    // pane (e.g. cd into the app + launch an agent CLI), then pause so an agent
    // launched by the setup is ready before the caller sends any prompt/input.
    if matches!(config.provider, CliProvider::Shell) {
        if let Some(setup) = config.setup_commands.as_deref() {
            // Address the PANE, never the session name: `send-keys -t <name>` takes a target-pane
            // and shadows on a same-named window in the current session exactly as `list-panes`
            // does, so setup commands could be typed into somebody else's live CLI.
            let pane_id = list_first_pane(tmux_session_name).await?;
            for line in setup.lines() {
                let trimmed = line.trim_end();
                if !trimmed.is_empty() {
                    run_tmux(send_keys_literal_args(&pane_id, trimmed)).await?;
                }
                run_tmux(send_keys_named_args(&pane_id, &["Enter".to_string()])).await?;
            }
            // Boot delay for any agent CLI launched by the setup commands.
            tokio::time::sleep(std::time::Duration::from_secs(4)).await;
        }
    }

    Ok(())
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
        // Catch-all (Ollama with empty model, Cline, Kloo): no extra pane args.
        // kloo oneshot never uses this command (phase 03 spawn_kloo_task goes
        // through cli_runner with a full argv). Empty args would start the
        // interactive TUI if someone attached a kloo session — acceptable only
        // because the task loop does not call provider_command.
        _ => (command, Vec::new()),
    }
}

/// Anchored tmux target for a command whose target is a *target-session* (`kill-session`).
///
/// For a target-session tmux falls back to fnmatch and then a PREFIX match when nothing matches
/// exactly, so an unanchored `-t kloo` resolves to `kloo-cli` whenever `kloo` is absent. The `=`
/// prefix disables that fallback. Measured on tmux 3.6: `has-session -t zzuniq` → rc 0 (it matched
/// `zzuniq-cli`), `has-session -t =zzuniq` → rc 1.
///
/// This is only enough for commands that take a target-SESSION. It is NOT enough for `list-panes`
/// or `send-keys`, which take a target-window/pane — see [`first_pane_for_session`]. Pane-id
/// targets (`%18`) are already globally unique and must not be wrapped.
fn tmux_session_target(session_name: &str) -> String {
    format!("={session_name}")
}

/// Session names tmux currently has, as exact strings.
///
/// `Err` means tmux itself could not be asked (binary missing, no server running at all). That is
/// deliberately distinct from `Ok(names)` simply not containing a name: the old `has-session`
/// shell-out collapsed both into `false`, so "the tmux server is gone" and "that one session
/// exited" were indistinguishable to callers.
async fn tmux_session_names() -> Result<Vec<String>, String> {
    let out = run_tmux(vec![
        "list-sessions".to_string(),
        "-F".to_string(),
        "#{session_name}".to_string(),
    ])
    .await?;
    Ok(parse_session_names(&out))
}

/// Exact session names out of `list-sessions -F '#{session_name}'`.
///
/// Pure, so the matching rule is testable without a tmux server. Only the line terminator is
/// stripped — a session name may contain spaces, and trimming them would make two distinct
/// sessions compare equal.
fn parse_session_names(output: &str) -> Vec<String> {
    output
        .lines()
        .map(|line| line.trim_end_matches(['\r', '\n']))
        .filter(|line| !line.is_empty())
        .map(ToOwned::to_owned)
        .collect()
}

/// Does a tmux session with EXACTLY this name exist?
///
/// Decided by string equality in Rust, not by tmux's target parser. `tmux has-session -t <name>`
/// prefix-matches, so the old form answered "yes" for `kloo` while only `kloo-cli` existed, and the
/// "external tmux session is not running" guard waved through a session that had already exited.
async fn tmux_has_session(name: &str) -> bool {
    tmux_session_names()
        .await
        .map(|names| names.iter().any(|candidate| candidate == name))
        .unwrap_or(false)
}

/// Pick a session's pane out of `list-panes -a -F '#{pane_id}\t#{window_active}\t#{session_name}'`
/// by EXACT session name.
///
/// Why a whole-server listing plus a compare in Rust, instead of `list-panes -t <name>`:
/// `list-panes` takes a target-WINDOW, and for a colonless target tmux first looks for a window of
/// that name inside the server's *current* session, consulting the session table only if that
/// fails. tmux auto-renames a window after the command running in it, so the session `kloo-cli`
/// (running the `kloo` TUI) owns a window named `kloo` — and `list-panes -t kloo` then returned
/// kloo-cli's pane `%18`. Measured on tmux 3.6 in the host's own environment (no `$TMUX`, no tty,
/// where tmux treats the most recently active session as "current"):
///
/// ```text
/// current session: kloo-cli
/// list-panes -t kloo      -> kloo-cli/kloo/%18   # WRONG
/// list-panes -t =kloo     -> kloo-cli/kloo/%18   # `=` does NOT anchor a window target
/// list-panes -s -t =kloo  -> kloo-cli/kloo/%18   # nor does -s
/// list-panes -t =kloo:    -> kloo/claude/%17     # only the trailing `:` pins the session
/// ```
///
/// Matching here takes tmux's `cmd_find_target` out of the decision entirely, so neither hazard
/// (prefix fallback, window-name shadowing) can reach the stored `tmux_pane_id` — which feeds
/// `send-keys` as well as `capture-pane`, so a misresolution typed input into the wrong CLI.
///
/// `pane_id` comes first and `session_name` last so `splitn(3, '\t')` keeps a name containing a tab
/// intact rather than truncating it into some other session's name.
fn first_pane_for_session(output: &str, session_name: &str) -> Option<String> {
    let mut fallback: Option<String> = None;
    for line in output.lines() {
        let mut parts = line.splitn(3, '\t');
        let pane_id = parts.next().unwrap_or("").trim();
        let window_active = parts.next().unwrap_or("").trim();
        let name = parts.next().unwrap_or("").trim_end_matches(['\r', '\n']);
        if pane_id.is_empty() || name != session_name {
            continue;
        }
        // Preserve the old semantics: `list-panes -t <session>` reported the session's CURRENT
        // window. Fall back to the session's first pane if tmux reported no active window.
        if window_active == "1" {
            return Some(pane_id.to_string());
        }
        if fallback.is_none() {
            fallback = Some(pane_id.to_string());
        }
    }
    fallback
}

/// First pane of the named session — exact match, with no tmux target matching involved.
async fn list_first_pane(tmux_session_name: &str) -> Result<String, String> {
    let output = run_tmux(vec![
        "list-panes".to_string(),
        "-a".to_string(),
        "-F".to_string(),
        "#{pane_id}\t#{window_active}\t#{session_name}".to_string(),
    ])
    .await?;

    first_pane_for_session(&output, tmux_session_name)
        .ok_or_else(|| format!("no tmux pane found for session '{tmux_session_name}'"))
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
    let (cursor_x, cursor_y, history_size) = pane_cursor_meta(pane_id).await?;
    let content = if history_size > 0 {
        let scrollback = history_size
            .min(LIVE_CAPTURE_SCROLLBACK_LINES)
            .max(1);
        run_tmux(vec![
            "capture-pane".to_string(),
            "-p".to_string(),
            "-e".to_string(),
            "-N".to_string(),
            "-J".to_string(),
            "-S".to_string(),
            format!("-{}", scrollback),
            "-t".to_string(),
            pane_id.to_string(),
        ])
        .await?
    } else {
        run_tmux(vec![
            "capture-pane".to_string(),
            "-p".to_string(),
            "-e".to_string(),
            "-N".to_string(),
            "-t".to_string(),
            pane_id.to_string(),
        ])
        .await?
    };
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
            run_tmux(send_keys_literal_args(pane_id, text)).await?;
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
        return run_tmux(send_keys_named_args(pane_id, &["C-c".to_string()]))
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
            run_tmux(send_keys_named_args(pane_id, &["C-m".to_string()])).await?;
        }
        return Ok(());
    }

    send_single_line_input(pane_id, &input, provider, submit).await
}

async fn send_submit_key(pane_id: &str) -> Result<(), String> {
    run_tmux(send_keys_named_args(pane_id, &["C-m".to_string()]))
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

/// argv for a `send-keys` that types LITERAL text (`-l`) into a pane.
///
/// The payload is user data — a setup command, a typed prompt line — so it can begin with `-`, and
/// tmux parses its own options before positionals. Without an option terminator a line like
/// `-la` is eaten as flags and never reaches the pane; a line that is exactly `--` would be
/// swallowed as the terminator itself. One builder so every literal-text call site shares the rule,
/// and so the rule is testable without a tmux server.
fn send_keys_literal_args(pane_id: &str, text: &str) -> Vec<String> {
    vec![
        "send-keys".to_string(),
        "-t".to_string(),
        pane_id.to_string(),
        "-l".to_string(),
        // The payload is user data and may begin with `-`, so stop option parsing here. Verified on
        // tmux 3.6: `send-keys -t %83 -l -- --`, `-- -la` and `-- -l` all reach the pane as text.
        "--".to_string(),
        text.to_string(),
    ]
}

/// argv for a `send-keys` that sends KEY NAMES (Enter, C-c, Down …) to a pane.
///
/// Key names can also look like options (a literal `-`), so the same terminator applies.
fn send_keys_named_args(pane_id: &str, keys: &[String]) -> Vec<String> {
    let mut args = vec![
        "send-keys".to_string(),
        "-t".to_string(),
        pane_id.to_string(),
        "--".to_string(),
    ];
    args.extend(keys.iter().cloned());
    args
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

/// An external tmux session a JohnnyOne terminal can attach to.
#[derive(serde::Serialize)]
pub struct ExternalTmuxSession {
    pub name: String,
    pub attached: bool,
    pub windows: u32,
}

/// List tmux sessions on the default socket that JohnnyOne can attach to,
/// EXCLUDING the `johnnyone_<id>` panes it already manages. A missing tmux
/// server (no sessions) yields an empty list, not an error.
pub async fn list_external_tmux_sessions() -> Result<Vec<ExternalTmuxSession>, String> {
    let out = match run_tmux(vec![
        "list-sessions".to_string(),
        "-F".to_string(),
        "#{session_name}\t#{session_attached}\t#{session_windows}".to_string(),
    ])
    .await
    {
        Ok(s) => s,
        Err(_) => return Ok(vec![]),
    };

    let mut sessions = Vec::new();
    for line in out.lines() {
        let mut parts = line.splitn(3, '\t');
        let name = parts.next().unwrap_or("").trim().to_string();
        if name.is_empty() || name.starts_with("johnnyone_") {
            continue;
        }
        let attached = parts.next().map(|s| s.trim() != "0").unwrap_or(false);
        let windows = parts
            .next()
            .and_then(|s| s.trim().parse::<u32>().ok())
            .unwrap_or(1);
        sessions.push(ExternalTmuxSession {
            name,
            attached,
            windows,
        });
    }
    Ok(sessions)
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
    fn clamp_history_capture_lines_honors_request_within_window() {
        // C2b regression guard: a client asking for 1500 (the console pane's CONSOLE_CAPTURE_LINES)
        // must get 1500 — NOT the old 200 default. This is the bug T2 caught: the RPC discarded the
        // value and always captured DEFAULT_HISTORY_CAPTURE_LINES.
        assert_eq!(clamp_history_capture_lines(Some(1500)), 1500);
        assert!(clamp_history_capture_lines(Some(1500)) > DEFAULT_HISTORY_CAPTURE_LINES);
    }

    #[test]
    fn clamp_history_capture_lines_defaults_and_bounds() {
        assert_eq!(clamp_history_capture_lines(None), DEFAULT_HISTORY_CAPTURE_LINES); // 200
        assert_eq!(clamp_history_capture_lines(Some(0)), 1); // never zero rows
        assert_eq!(clamp_history_capture_lines(Some(5000)), MAX_HISTORY_CAPTURE_LINES); // 2000 ceiling
        // A value larger than u16::MAX must clamp, not wrap/overflow the `as u16` cast.
        assert_eq!(clamp_history_capture_lines(Some(10_000_000_000)), MAX_HISTORY_CAPTURE_LINES);
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

    /// A `Shell` session spawns the plain shell command with NO agent args — proving Shell uses the
    /// same spawn path as agents but without provider flags (overhaul P2, decision D7). Pure: no tmux
    /// pane is created. `cli_path` pins the command so the result is deterministic regardless of the
    /// test host's `$SHELL`.
    #[test]
    fn shell_provider_command_is_shell() {
        let config = SessionConfig {
            session_id: "t".to_string(),
            provider: CliProvider::Shell,
            model: String::new(),
            working_directory: "/tmp".to_string(),
            cli_path: Some("bash".to_string()),
            setup_commands: None,
            attached_tmux: false,
            tmux_session_name: None,
        };
        let (command, args) = provider_command(&config);
        assert_eq!(command, "bash");
        assert!(args.is_empty(), "shell must launch with no agent args");
    }

    /// The real `kloo` / `kloo-cli` table, verbatim from
    /// `tmux list-panes -a -F '#{pane_id}\t#{window_active}\t#{session_name}'` on the host while the
    /// bug was live. `kloo-cli`'s window is NAMED `kloo` (tmux renames a window after the command
    /// running in it), and that window is what tmux's target matcher resolved `-t kloo` onto.
    const LIVE_PANES: &str = "\
%19\t1\tj1
%5\t1\tjohnnyone_768d2c33-d85a-4013-9cce-722948c87a69
%17\t1\tkloo
%18\t1\tkloo-cli
%0\t1\tkord";

    #[test]
    fn first_pane_for_session_matches_exactly_not_by_prefix() {
        // The whole bug in one assertion: `kloo` must resolve to %17, never kloo-cli's %18.
        assert_eq!(
            first_pane_for_session(LIVE_PANES, "kloo").as_deref(),
            Some("%17")
        );
        assert_eq!(
            first_pane_for_session(LIVE_PANES, "kloo-cli").as_deref(),
            Some("%18")
        );
        // A name that is only a prefix of a real session must not resolve at all.
        assert_eq!(first_pane_for_session(LIVE_PANES, "kl"), None);
        assert_eq!(first_pane_for_session(LIVE_PANES, "klo"), None);
        assert_eq!(first_pane_for_session(LIVE_PANES, "j"), None);
        assert_eq!(first_pane_for_session(LIVE_PANES, ""), None);
    }

    #[test]
    fn first_pane_for_session_prefers_the_active_window() {
        // Preserves the old `list-panes -t <session>` semantics (the session's CURRENT window),
        // and still answers when tmux reports no active window for the session.
        let out = "%1\t0\tsess\n%2\t1\tsess\n%3\t0\tsess";
        assert_eq!(first_pane_for_session(out, "sess").as_deref(), Some("%2"));
        let inactive = "%7\t0\tsess\n%8\t0\tsess";
        assert_eq!(
            first_pane_for_session(inactive, "sess").as_deref(),
            Some("%7")
        );
    }

    #[test]
    fn session_names_compare_exactly() {
        let names = parse_session_names("kloo\nkloo-cli\nj1\n\n");
        assert_eq!(names, vec!["kloo", "kloo-cli", "j1"]);
        assert!(names.iter().any(|n| n == "kloo"));
        // The prefix hole the shelled `has-session -t kloo` had: `kloo` must not count as present
        // merely because `kloo-cli` is.
        let only_sibling = parse_session_names("kloo-cli\n");
        assert!(!only_sibling.iter().any(|n| n == "kloo"));
        // A name containing a space survives intact — trimming it would merge distinct sessions.
        assert_eq!(parse_session_names("my sess\n"), vec!["my sess"]);
    }

    // ---------------------------------------------------------------------------------------
    // send-keys option terminator
    // ---------------------------------------------------------------------------------------

    /// Where in an argv the payload starts: everything after the first bare `--`.
    ///
    /// This mirrors how tmux's own getopt parses the vector, so the assertions below are about
    /// tmux's reading of the argv rather than about our formatting of it.
    fn payload_after_terminator(args: &[String]) -> Option<&[String]> {
        args.iter().position(|a| a == "--").map(|i| &args[i + 1..])
    }

    #[test]
    fn send_keys_literal_payload_cannot_be_read_as_options() {
        // The bug: `create_tmux_session` typed setup lines with `send-keys … -l <line>` and no
        // terminator, so a setup command starting with `-` was parsed as tmux flags and never
        // reached the pane.
        for line in [
            "-la",
            "-l",
            "--",
            "--version",
            "-",
            "cd /tmp && ls -la",
            "-N 3",
        ] {
            let args = send_keys_literal_args("%7", line);
            let payload = payload_after_terminator(&args)
                .unwrap_or_else(|| panic!("no `--` terminator in argv for {line:?}: {args:?}"));
            assert_eq!(
                payload,
                [line.to_string()],
                "payload for {line:?} must be exactly the line, as text"
            );
            // `-l` must still be a FLAG (before the terminator), not part of the payload.
            let terminator = args.iter().position(|a| a == "--").unwrap();
            assert!(
                args[..terminator].iter().any(|a| a == "-l"),
                "the literal flag must precede the terminator: {args:?}"
            );
            // The target must also be a flag, not swallowed into the text.
            assert_eq!(args[..terminator], ["send-keys", "-t", "%7", "-l"]);
        }
    }

    #[test]
    fn send_keys_literal_line_that_is_exactly_the_terminator_still_goes_through_as_text() {
        // A line that is itself `--` is the nastiest case: the FIRST `--` is ours (the terminator)
        // and the second is the payload. Asserting on the first position proves the line survives
        // as text rather than being consumed as the terminator.
        let args = send_keys_literal_args("%7", "--");
        assert_eq!(
            args,
            [
                "send-keys".to_string(),
                "-t".to_string(),
                "%7".to_string(),
                "-l".to_string(),
                "--".to_string(),
                "--".to_string(),
            ]
        );
        assert_eq!(payload_after_terminator(&args).unwrap(), ["--".to_string()]);
    }

    #[test]
    fn send_keys_named_keys_cannot_be_read_as_options() {
        let keys = vec!["-".to_string(), "Enter".to_string()];
        let args = send_keys_named_args("%7", &keys);
        assert_eq!(payload_after_terminator(&args).unwrap(), keys.as_slice());
        let terminator = args.iter().position(|a| a == "--").unwrap();
        assert_eq!(args[..terminator], ["send-keys", "-t", "%7"]);
        // No `-l`: these are key NAMES, resolved by tmux against the pane's mode.
        assert!(!args.iter().any(|a| a == "-l"));
    }

    // ---------------------------------------------------------------------------------------
    // startup reconciliation of terminal_status
    // ---------------------------------------------------------------------------------------

    fn owned_row(session_id: &str) -> AttachedTerminalRow {
        AttachedTerminalRow {
            session_id: session_id.to_string(),
            attached_tmux: false,
            tmux_session_name: None,
        }
    }

    fn attached_row(session_id: &str, name: Option<&str>) -> AttachedTerminalRow {
        AttachedTerminalRow {
            session_id: session_id.to_string(),
            attached_tmux: true,
            tmux_session_name: name.map(ToOwned::to_owned),
        }
    }

    /// THE assertion that matters: when tmux cannot be ASKED, reconcile NOTHING.
    ///
    /// `tmux_session_names()` returns `Err` for "no binary / no server", which is indistinguishable
    /// from "every session is gone" if you only look at the name list. Treating it as the latter
    /// would mass-demote every row on the user's machine — strictly worse than the stale-row bug
    /// this reconciliation exists to fix.
    #[test]
    fn tmux_cannot_be_asked_demotes_nothing() {
        let rows = vec![
            owned_row("a"),
            owned_row("b"),
            attached_row("c", Some("kloo")),
            attached_row("d", None),
        ];
        assert!(
            terminal_rows_to_demote(&rows, None).is_empty(),
            "an unaskable tmux must never demote a row"
        );
    }

    #[test]
    fn demotes_only_rows_whose_tmux_session_is_gone() {
        // Modelled on the live host: `johnnyone_768d…` and `kloo` exist, nothing else does.
        let live = vec![
            "j1".to_string(),
            "johnnyone_768d".to_string(),
            "kloo".to_string(),
            "kloo-cli".to_string(),
            "kord".to_string(),
        ];
        let rows = vec![
            owned_row("768d"),           // johnnyone_768d — alive, keep
            owned_row("dead-1"),         // johnnyone_dead-1 — gone, demote
            attached_row("x", Some("kloo")),     // user's own shell, alive, keep
            attached_row("y", Some("kord")),     // alive, keep
            attached_row("z", Some("gone-sess")), // gone, demote
        ];
        assert_eq!(
            terminal_rows_to_demote(&rows, Some(&live)),
            vec!["dead-1".to_string(), "z".to_string()]
        );
    }

    #[test]
    fn attached_row_is_not_rescued_by_a_prefix_match() {
        // Same hole the exact-match resolution closed: `kloo` must not count as present merely
        // because `kloo-cli` is. A shelled `has-session -t kloo` would have said yes.
        let live = vec!["kloo-cli".to_string()];
        assert_eq!(
            terminal_rows_to_demote(&[attached_row("x", Some("kloo"))], Some(&live)),
            vec!["x".to_string()]
        );
        // And the converse: an exact match keeps the row.
        assert!(terminal_rows_to_demote(&[attached_row("x", Some("kloo-cli"))], Some(&live)).is_empty());
    }

    #[test]
    fn attached_row_without_a_name_is_demoted() {
        // `resolve_tmux_target` errors on this row, so it can never be captured — its
        // `terminal_status='attached'` is unconditionally a lie. But only when tmux COULD be asked.
        let live: Vec<String> = Vec::new();
        for name in [None, Some(""), Some("   ")] {
            assert_eq!(
                terminal_rows_to_demote(&[attached_row("x", name)], Some(&live)),
                vec!["x".to_string()],
                "attached row with name {name:?} must be demoted"
            );
        }
    }

    #[test]
    fn an_empty_live_list_still_demotes() {
        // `Ok(vec![])` is "the server answered: no sessions" — distinct from `None`/Err above.
        let live: Vec<String> = Vec::new();
        assert_eq!(
            terminal_rows_to_demote(&[owned_row("a")], Some(&live)),
            vec!["a".to_string()]
        );
    }

    #[test]
    fn no_rows_means_no_demotions() {
        assert!(terminal_rows_to_demote(&[], Some(&["kloo".to_string()])).is_empty());
    }

    #[test]
    fn owned_session_name_matches_the_spawn_rule() {
        // The reconciler must derive the SAME name `create_tmux_session` spawns under, including the
        // non-alphanumeric sanitisation, or it would demote live owned panes.
        let live = vec![tmux_session_name("a b/c")];
        assert!(
            terminal_rows_to_demote(&[owned_row("a b/c")], Some(&live)).is_empty(),
            "sanitised owned name must resolve against the live list"
        );
        assert_eq!(live[0], "johnnyone_a_b_c");
    }

    /// The batched write, against a real migrated database: exactly the named rows are demoted,
    /// `tmux_pane_id` is cleared, and `status` is untouched. The pure test above decides WHICH ids;
    /// this one pins what the SQL does with them — including that one statement covers many rows.
    #[test]
    fn demote_terminal_rows_is_one_batched_pass_and_leaves_status_alone() {
        let (state, _root) = crate::test_support::test_state();
        state
            .db
            .with_conn(|conn| {
                for i in 0..120 {
                    conn.execute(
                        "INSERT INTO sessions (id, terminal_status, tmux_pane_id, status, working_directory) VALUES (?1, 'attached', ?2, 'active', '/tmp')",
                        params![format!("s{i}"), format!("%{i}")],
                    )
                    .map_err(|e| e.to_string())?;
                }
                // A row that must NOT be touched.
                conn.execute(
                    "INSERT INTO sessions (id, terminal_status, tmux_pane_id, status, working_directory) VALUES ('keep', 'attached', '%999', 'active', '/tmp')",
                    [],
                )
                .map_err(|e| e.to_string())?;
                Ok(())
            })
            .unwrap();

        let ids: Vec<String> = (0..120).map(|i| format!("s{i}")).collect();
        demote_terminal_rows(&state, &ids).unwrap();

        state
            .db
            .with_conn(|conn| {
                let demoted: i64 = conn
                    .query_row(
                        "SELECT COUNT(*) FROM sessions WHERE terminal_status = 'detached' AND tmux_pane_id IS NULL",
                        [],
                        |r| r.get(0),
                    )
                    .map_err(|e| e.to_string())?;
                assert_eq!(demoted, 120, "every named row demoted, pane id cleared");

                let (status, term, pane): (String, String, Option<String>) = conn
                    .query_row(
                        "SELECT status, terminal_status, tmux_pane_id FROM sessions WHERE id = 'keep'",
                        [],
                        |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?)),
                    )
                    .map_err(|e| e.to_string())?;
                assert_eq!(term, "attached", "an unnamed row must survive untouched");
                assert_eq!(pane.as_deref(), Some("%999"));
                assert_eq!(status, "active");

                // `status` is never written by the reconciler.
                let actives: i64 = conn
                    .query_row(
                        "SELECT COUNT(*) FROM sessions WHERE status = 'active'",
                        [],
                        |r| r.get(0),
                    )
                    .map_err(|e| e.to_string())?;
                assert_eq!(actives, 121, "status column untouched");
                Ok(())
            })
            .unwrap();

        // An empty demotion set must be a no-op, not a malformed `IN ()`.
        demote_terminal_rows(&state, &[]).unwrap();
    }

    /// End-to-end wiring against the REAL tmux server: SELECT → decision → batched UPDATE.
    ///
    /// The pure test decides which ids, the DB test pins the SQL; only this one proves the three are
    /// plumbed together and that the live `tmux_session_names()` answer is the one consulted.
    ///
    /// tmux hygiene: creates and kills exactly one session named `zztest-j1-reconcile`, never
    /// touches any other session, and kills it on every exit path. It only ever writes to its own
    /// throwaway database, so the user's rows cannot be affected.
    ///
    /// When tmux cannot be asked (no binary / no server, e.g. CI) this asserts the OTHER half of
    /// the contract instead of skipping silently: nothing is demoted at all.
    #[tokio::test]
    async fn reconcile_demotes_the_gone_session_and_keeps_the_live_one() {
        const LIVE: &str = "zztest-j1-reconcile";
        let tmux_reachable = tmux_session_names().await.is_ok();

        let (state, _root) = crate::test_support::test_state();
        state
            .db
            .with_conn(|conn| {
                // Attached to a tmux session that is about to be real.
                conn.execute(
                    "INSERT INTO sessions (id, terminal_status, tmux_pane_id, attached_tmux, tmux_session_name, status, working_directory) VALUES ('live', 'attached', '%1', 1, ?1, 'active', '/tmp')",
                    params![LIVE],
                )
                .map_err(|e| e.to_string())?;
                // Attached to one that will never exist.
                conn.execute(
                    "INSERT INTO sessions (id, terminal_status, tmux_pane_id, attached_tmux, tmux_session_name, status, working_directory) VALUES ('gone', 'attached', '%2', 1, 'zztest-j1-does-not-exist', 'active', '/tmp')",
                    [],
                )
                .map_err(|e| e.to_string())?;
                // An owned row: `johnnyone_owned-gone` is never spawned, so it is always stale.
                conn.execute(
                    "INSERT INTO sessions (id, terminal_status, tmux_pane_id, attached_tmux, status, working_directory) VALUES ('owned-gone', 'attached', '%3', 0, 'active', '/tmp')",
                    [],
                )
                .map_err(|e| e.to_string())?;
                Ok(())
            })
            .unwrap();

        if tmux_reachable {
            run_tmux(vec![
                "new-session".to_string(),
                "-d".to_string(),
                "-s".to_string(),
                LIVE.to_string(),
                "-c".to_string(),
                "/tmp".to_string(),
                "cat".to_string(),
            ])
            .await
            .expect("create the zztest- session");
        }

        let demoted = reconcile_terminal_status_on_startup(&state).await;

        // Kill our session before asserting, so a failed assertion still leaves tmux clean.
        if tmux_reachable {
            let _ = run_tmux(vec![
                "kill-session".to_string(),
                "-t".to_string(),
                tmux_session_target(LIVE),
            ])
            .await;
        }

        let status_of = |id: &str| -> String {
            state
                .db
                .with_conn(|conn| {
                    conn.query_row(
                        "SELECT terminal_status FROM sessions WHERE id = ?1",
                        params![id],
                        |r| r.get(0),
                    )
                    .map_err(|e| e.to_string())
                })
                .unwrap()
        };

        if tmux_reachable {
            assert_eq!(demoted.unwrap(), 2, "the two gone sessions, and only those");
            assert_eq!(status_of("live"), "attached", "a LIVE tmux must survive");
            assert_eq!(status_of("gone"), "detached");
            assert_eq!(status_of("owned-gone"), "detached");
            let pane: Option<String> = state
                .db
                .with_conn(|conn| {
                    conn.query_row(
                        "SELECT tmux_pane_id FROM sessions WHERE id = 'gone'",
                        [],
                        |r| r.get(0),
                    )
                    .map_err(|e| e.to_string())
                })
                .unwrap();
            assert!(pane.is_none(), "a demoted row's pane id is cleared");
        } else {
            assert_eq!(demoted.unwrap(), 0, "unaskable tmux must demote nothing");
            for id in ["live", "gone", "owned-gone"] {
                assert_eq!(status_of(id), "attached");
            }
        }
    }

    /// A capture-failure detach must leave NO pane id behind.
    ///
    /// The drift this pins: the capture loop's failure break wrote `terminal_status='detached'` but
    /// not `tmux_pane_id = NULL`, so a row sat `detached` while still naming the pane that had just
    /// died — and only the next host restart cleaned it up. Both live detach paths now go through
    /// `detach_terminal_row`, so this exercises the exact write the capture loop performs.
    ///
    /// What it does NOT prove: that the capture loop calls this function. That link is structural —
    /// `detach_terminal_row` is the only detach SQL in the module besides the batched reconciler,
    /// and both share `DETACH_TERMINAL_SET` — not something this test checks.
    #[test]
    fn capture_failure_detach_clears_the_pane_id() {
        let (state, _root) = crate::test_support::test_state();
        state
            .db
            .with_conn(|conn| {
                conn.execute(
                    "INSERT INTO sessions (id, terminal_status, tmux_pane_id, status, working_directory) VALUES ('s', 'attached', '%42', 'active', '/tmp')",
                    [],
                )
                .map_err(|e| e.to_string())
            })
            .unwrap();

        detach_terminal_row(&state.db, "s").unwrap();

        let (term, pane, status): (String, Option<String>, String) = state
            .db
            .with_conn(|conn| {
                conn.query_row(
                    "SELECT terminal_status, tmux_pane_id, status FROM sessions WHERE id = 's'",
                    [],
                    |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?)),
                )
                .map_err(|e| e.to_string())
            })
            .unwrap();

        assert_eq!(term, "detached");
        assert!(
            pane.is_none(),
            "a detached row must not keep a pane id (got {pane:?})"
        );
        assert_eq!(status, "active", "status is never touched by a detach");
    }

    /// The three detach writers agree because they share one SET clause.
    ///
    /// Cheap drift guard: if someone re-specialises one of them, this is what catches the clause
    /// losing `tmux_pane_id = NULL` again.
    #[test]
    fn the_detach_set_clause_clears_the_pane_id() {
        assert!(DETACH_TERMINAL_SET.contains("terminal_status = 'detached'"));
        assert!(
            DETACH_TERMINAL_SET.contains("tmux_pane_id = NULL"),
            "every detach path must clear the pane id: {DETACH_TERMINAL_SET}"
        );
        assert!(DETACH_TERMINAL_SET.contains("updated_at = datetime('now')"));
        // It is a SET clause only — a writer supplies its own WHERE.
        assert!(!DETACH_TERMINAL_SET.to_ascii_uppercase().contains("WHERE"));
    }

    #[test]
    fn kill_session_target_is_anchored() {
        // `kill-session` takes a target-SESSION — the one place `=` alone is the right tool: there
        // is no window to pin, and `=` is what disables tmux's prefix fallback.
        assert_eq!(tmux_session_target("kloo"), "=kloo");
        assert_eq!(
            tmux_session_target(&tmux_session_name("abc-123")),
            "=johnnyone_abc-123"
        );
    }

    // ---------------------------------------------------------------------------------------------
    // tmux target resolution (the `kloo` / `kloo-cli` same-screen bug)
    // ---------------------------------------------------------------------------------------------

    /// True when a tmux server is reachable; the target-resolution tests below drive the REAL tmux
    /// (there is no way to fake `cmd_find_target`), so they skip where tmux is unavailable.
    async fn tmux_available() -> bool {
        Command::new("tmux")
            .arg("-V")
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .status()
            .await
            .map(|s| s.success())
            .unwrap_or(false)
    }

    async fn tmux_quiet(args: &[&str]) {
        let _ = Command::new("tmux")
            .args(args)
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .status()
            .await;
    }

    /// Regression guard for "two shells show the same terminal".
    ///
    /// Shape of the real incident: session `kloo` (running `claude`) and session `kloo-cli` (running
    /// the `kloo` TUI). tmux auto-renames a window after its running command, so `kloo-cli`'s single
    /// window is NAMED `kloo`. `list-panes -t kloo` takes a target-WINDOW, and tmux resolves a
    /// colonless target as a window inside the server's *current* session BEFORE consulting the
    /// session table — so whenever `kloo-cli` was the current session, `list_first_pane("kloo")`
    /// returned kloo-cli's pane and both rows stored the same pane id.
    ///
    /// The host runs tmux with no `$TMUX` and no tty, where "current session" is whichever session
    /// tmux considers most recently active — hence the intermittency. The test reproduces that by
    /// clearing `$TMUX` and creating the sibling LAST so it is the most recently active session.
    #[tokio::test]
    async fn list_first_pane_resolves_the_session_not_a_same_named_window() {
        if !tmux_available().await {
            eprintln!("skipping: no tmux server available");
            return;
        }
        // The host process has no $TMUX (it is not launched from inside tmux). Without this the
        // test would inherit the developer's own pane as the current session and the collision
        // could not arise.
        std::env::remove_var("TMUX");
        std::env::remove_var("TMUX_PANE");

        let target = "zztest-j1pane";
        let sibling = "zztest-j1pane-sib";
        tmux_quiet(&["kill-session", "-t", &format!("={target}")]).await;
        tmux_quiet(&["kill-session", "-t", &format!("={sibling}")]).await;

        // The session we actually want: its window is NOT named `target`.
        tmux_quiet(&[
            "new-session", "-d", "-s", target, "-n", "wanted", "sleep", "600",
        ])
        .await;
        tmux_quiet(&["set-option", "-w", "-t", &format!("={target}:"), "automatic-rename", "off"]).await;
        tmux_quiet(&["rename-window", "-t", &format!("={target}:"), "wanted"]).await;

        // Created LAST so tmux treats it as the current session, and holding a window named exactly
        // like the session above — the `kloo-cli` shape.
        sleep(Duration::from_millis(1200)).await;
        tmux_quiet(&[
            "new-session", "-d", "-s", sibling, "-n", "decoy", "sleep", "600",
        ])
        .await;
        tmux_quiet(&["set-option", "-w", "-t", &format!("={sibling}:"), "automatic-rename", "off"]).await;
        tmux_quiet(&["rename-window", "-t", &format!("={sibling}:"), target]).await;

        let want = run_tmux(vec![
            "list-panes".to_string(),
            "-t".to_string(),
            format!("={target}:"),
            "-F".to_string(),
            "#{pane_id}".to_string(),
        ])
        .await
        .expect("fixture: target session must have a pane");
        let want = want.trim().to_string();
        let decoy = run_tmux(vec![
            "list-panes".to_string(),
            "-t".to_string(),
            format!("={sibling}:"),
            "-F".to_string(),
            "#{pane_id}".to_string(),
        ])
        .await
        .expect("fixture: sibling session must have a pane");
        let decoy = decoy.trim().to_string();

        let current = run_tmux(vec![
            "display-message".to_string(),
            "-p".to_string(),
            "#{session_name}".to_string(),
        ])
        .await
        .unwrap_or_default()
        .trim()
        .to_string();

        let resolved = list_first_pane(target).await;
        let has = tmux_has_session(target).await;

        tmux_quiet(&["kill-session", "-t", &format!("={target}")]).await;
        tmux_quiet(&["kill-session", "-t", &format!("={sibling}")]).await;

        if current != sibling {
            // Someone attached a client mid-test and became the current session; the collision
            // cannot arise in that state, so there is nothing to assert.
            eprintln!("skipping assertions: current session is {current:?}, not the decoy");
            return;
        }
        assert!(has, "exact-match session lookup must see the session");
        assert_ne!(
            resolved.as_deref(),
            Ok(decoy.as_str()),
            "list_first_pane({target}) returned a pane owned by {sibling} — anything typed into \
             the {target} tab would land in the wrong CLI"
        );
        assert_eq!(
            resolved.as_deref(),
            Ok(want.as_str()),
            "list_first_pane({target}) resolved {resolved:?}; the decoy session {sibling} owns \
             {decoy} and must never be returned"
        );
    }

    /// The prefix/fnmatch hazard on a target-SESSION: with `kloo` gone and `kloo-cli` present,
    /// an unanchored `has-session -t kloo` SUCCEEDS, so the "external tmux is not running" guard
    /// waves through a session that does not exist and the caller goes on to capture the sibling.
    #[tokio::test]
    async fn tmux_has_session_rejects_a_prefix_sibling() {
        if !tmux_available().await {
            eprintln!("skipping: no tmux server available");
            return;
        }
        std::env::remove_var("TMUX");
        std::env::remove_var("TMUX_PANE");

        let missing = "zztest-j1prefix";
        let sibling = "zztest-j1prefix-cli";
        tmux_quiet(&["kill-session", "-t", &format!("={missing}")]).await;
        tmux_quiet(&["kill-session", "-t", &format!("={sibling}")]).await;
        tmux_quiet(&["new-session", "-d", "-s", sibling, "sleep", "600"]).await;

        let found = tmux_has_session(missing).await;
        tmux_quiet(&["kill-session", "-t", &format!("={sibling}")]).await;

        assert!(
            !found,
            "has-session must not prefix-match {sibling} when {missing} does not exist"
        );
    }
}
