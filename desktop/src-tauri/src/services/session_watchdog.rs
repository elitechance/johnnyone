use crate::services::sessions;
use crate::state::app_state::AppState;
use std::sync::Arc;
use std::time::Duration;

/// ONE ticking loop for ALL sessions. Spawned from main.rs beside `question_watch`.
///
/// Every 20s, for every session with `watchdog_enabled = 1` and a non-empty prompt,
/// checks whether it is due for a nudge. If so, and the pane is idle, delivers the
/// configured prompt through `deliver_chat_message` with `role = "watchdog"`.
///
/// The sweep's candidate SELECT, hoisted to a const so a test can run the REAL query against a
/// row armed by the REAL `sessions::SET_WATCHDOG_SQL`.
///
/// The bug this guards: the write that arms a session and the query that selects nudge candidates
/// live in different files, and nothing checked that the first satisfies the second — so arming a
/// session left `watchdog_prompt = ''` (migration 027's default) and `AND watchdog_prompt != ''`
/// skipped the row forever. `watchdog_nudge_count` was 0 on every live row while every unit test
/// stayed green. Kept byte-identical to what `tick` used before the hoist; change no behaviour.
pub(crate) const WATCHDOG_CANDIDATE_SQL: &str = "SELECT id, watchdog_interval_secs, \
    watchdog_prompt, watchdog_last_nudge_at, watchdog_nudge_count, title FROM sessions \
    WHERE watchdog_enabled = 1 AND watchdog_prompt != '' AND status != 'archived'";

/// Must run on the Tauri async runtime (`tauri::async_runtime::spawn`) so its tokio
/// timers share the executor the rest of the host uses.
pub fn spawn_session_watchdog(state: AppState) {
    // `tauri::async_runtime::spawn`, NOT `tokio::spawn`: this is called from Tauri's `setup`,
    // which runs on the main thread with no tokio reactor entered, so a bare `tokio::spawn`
    // panics there with "there is no reactor running" and takes the host down on boot.
    tauri::async_runtime::spawn(async move {
        let state = Arc::new(state);
        tracing::info!("session watchdog loop starting (interval 20s)");
        let mut ticker = tokio::time::interval(Duration::from_secs(20));

        loop {
            ticker.tick().await;
            if let Err(error) = tick(&state).await {
                tracing::warn!(%error, "session watchdog tick failed");
            }
        }
    });
}

/// Substitute `{quiet}`, `{name}` and `{nudges}` into a watchdog prompt.
///
/// The ONE place these placeholders are expanded. Both callers — the 20s sweep in `tick` and the
/// "Nudge now" button via `nudge_session_now` — go through here, so the text means the same thing
/// whichever of the two types it. `{nudges}` is the number of the nudge about to be delivered,
/// i.e. `nudge_count + 1`, since the counter is bumped only after a successful delivery. A prompt
/// with no placeholders comes back untouched.
pub fn render_watchdog_nudge(
    prompt: &str,
    interval_secs: u64,
    title: &str,
    nudge_count: i64,
) -> String {
    prompt
        .replace("{quiet}", &sessions::humanise_duration(interval_secs))
        .replace("{name}", title)
        .replace("{nudges}", &(nudge_count + 1).to_string())
}

/// Why "Nudge now" would refuse to fire, if it would. Pure so the precondition is testable without
/// an `AppState` harness.
///
/// The prompt is the only thing a nudge cannot be improvised from: a session migration 027 left at
/// `watchdog_prompt = ''` has nothing to type, and saying so beats silently sending an empty
/// message into the pane.
pub fn nudge_now_rejection(prompt: &str) -> Option<String> {
    if prompt.trim().is_empty() {
        Some(
            "Session has no watchdog prompt to send. Arm the watchdog or type a prompt first."
                .to_string(),
        )
    } else {
        None
    }
}

/// Deliver one watchdog nudge for `id` RIGHT NOW, on the user's demand.
///
/// Deliberately skips the two guards the sweep applies — the due check and the two-capture idle
/// probe. The user pressed the button, so it fires even if the interval has not elapsed and even
/// if the pane is mid-turn. Everything else is shared with the sweep: the same renderer, the same
/// `deliver_chat_message` seam with `role = "watchdog"`, and the same counter bump.
pub async fn nudge_session_now(state: &Arc<AppState>, id: &str) -> Result<(), String> {
    let (prompt, interval_secs, title, nudge_count) = state.db.with_conn(|conn| {
        conn.query_row(
            "SELECT watchdog_prompt, watchdog_interval_secs, title, watchdog_nudge_count \
             FROM sessions WHERE id = ?1",
            rusqlite::params![id],
            |row| {
                Ok((
                    row.get::<_, String>(0)?,
                    row.get::<_, i64>(1)? as u64,
                    row.get::<_, String>(2)?,
                    row.get::<_, i64>(3)?,
                ))
            },
        )
        .map_err(|e| format!("Session not found: {}", e))
    })?;

    if let Some(reason) = nudge_now_rejection(&prompt) {
        return Err(reason);
    }

    let rendered = render_watchdog_nudge(&prompt, interval_secs, &title, nudge_count);

    crate::agent::AgentService::deliver_chat_message(state, id, &rendered, "watchdog").await?;

    sessions::bump_watchdog_nudge(state, id)?;

    tracing::info!(
        session_id = %id,
        nudge = nudge_count + 1,
        interval_secs,
        "delivered manual watchdog nudge"
    );
    Ok(())
}

async fn tick(state: &Arc<AppState>) -> Result<(), String> {
    let now = chrono::Utc::now();

    // Grab all session IDs that have watchdog enabled and a non-empty prompt.
    let candidates = state.db.with_conn(|conn| {
        let mut stmt = conn
            .prepare(WATCHDOG_CANDIDATE_SQL)
            .map_err(|e| e.to_string())?;
        let rows = stmt
            .query_map([], |row| {
                Ok((
                    row.get::<_, String>(0)?,   // id
                    row.get::<_, i64>(1)? as u64, // interval_secs
                    row.get::<_, String>(2)?,   // prompt
                    row.get::<_, Option<String>>(3)?, // last_nudge_at
                    row.get::<_, i64>(4)? as u64, // nudge_count
                    row.get::<_, String>(5)?,   // title
                ))
            })
            .map_err(|e| e.to_string())?
            .filter_map(|r| r.ok())
            .collect::<Vec<_>>();
        Ok(rows)
    })?;

    for (id, interval_secs, prompt, last_nudge_at, nudge_count, title) in &candidates {
        // Eligibility: same rule as `session_wants_brief`.
        let wants_brief = state.db.with_conn(|conn| {
            conn.query_row(
                "SELECT provider, attached_tmux, setup_commands FROM sessions WHERE id = ?1",
                rusqlite::params![id],
                |row| {
                    let provider: String = row.get(0)?;
                    let attached: i64 = row.get(1)?;
                    let setup: Option<String> = row.get(2)?;
                    let launched_something =
                        setup.map(|s| !s.trim().is_empty()).unwrap_or(false);
                    Ok(attached != 0 || (provider == "shell" && launched_something))
                },
            )
            .map_err(|e| e.to_string())
        }).unwrap_or(false);

        if !wants_brief {
            continue;
        }

        // Determine the most recent timestamp to compare against.
        let latest_report = sessions::latest_session_report_at(state, id)
            .unwrap_or(None);

        // The "last" timestamp is the later of watchdog_last_nudge_at and the most recent report.
        let last_ts = last_nudge_at
            .as_ref()
            .map(|s| s.as_str())
            .or(latest_report.as_deref());

        let interval = *interval_secs;

        // Skip if not yet due.
        if !sessions::watchdog_is_due(&now, last_ts, interval) {
            continue;
        }

        // Skip if the pane is still changing — reuse the two-capture idle probe.
        let first = match crate::terminal::capture_terminal_session(state, id).await {
            Ok(snapshot) => snapshot,
            Err(_) => continue,
        };
        tokio::time::sleep(Duration::from_secs(3)).await;
        let second = match crate::terminal::capture_terminal_session(state, id).await {
            Ok(snapshot) => snapshot,
            Err(_) => continue,
        };
        let a = crate::services::agent_plans::normalize_terminal_snapshot_for_idle(&first.content);
        let b = crate::services::agent_plans::normalize_terminal_snapshot_for_idle(&second.content);
        if a != b {
            // Pane is still changing — the nudge WAS due and is being withheld. Record it: without
            // this, "held back" is indistinguishable from "quiet, nothing to say", which is exactly
            // the silence that hid the original watchdog bug. NOT bumped when `watchdog_is_due`
            // returned false above — a timer that has not elapsed is not a hold.
            if let Err(error) = sessions::bump_watchdog_held(state, id) {
                tracing::warn!(session_id = %id, %error, "watchdog held bump failed");
            }
            continue;
        }

        // Substitute the placeholders — the same renderer the "Nudge now" button uses.
        let nudges_val = *nudge_count + 1;
        let rendered = render_watchdog_nudge(prompt, interval, title, *nudge_count as i64);

        // Deliver through the unified seam.
        if let Err(error) =
            crate::agent::AgentService::deliver_chat_message(state, id, &rendered, "watchdog").await
        {
            tracing::warn!(session_id = %id, %error, "watchdog deliver failed");
            continue;
        }

        // Bump counters.
        if let Err(error) = sessions::bump_watchdog_nudge(state, id) {
            tracing::warn!(session_id = %id, %error, "watchdog bump failed");
        }

        tracing::info!(
            session_id = %id,
            nudge = nudges_val,
            interval_secs = interval,
            "delivered watchdog nudge"
        );
    }

    Ok(())
}