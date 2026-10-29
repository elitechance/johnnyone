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
/// Must run on the Tauri async runtime (`tauri::async_runtime::spawn`) so its tokio
/// timers share the executor the rest of the host uses.
pub async fn spawn_session_watchdog(state: AppState) {
    let state = Arc::new(state);
    tracing::info!("session watchdog loop starting (interval 20s)");
    let mut ticker = tokio::time::interval(Duration::from_secs(20));

    loop {
        ticker.tick().await;
        if let Err(error) = tick(&state).await {
            tracing::warn!(%error, "session watchdog tick failed");
        }
    }
}

async fn tick(state: &Arc<AppState>) -> Result<(), String> {
    let now = chrono::Utc::now();

    // Grab all session IDs that have watchdog enabled and a non-empty prompt.
    let candidates = state.db.with_conn(|conn| {
        let mut stmt = conn
            .prepare(
                "SELECT id, watchdog_interval_secs, watchdog_prompt, watchdog_last_nudge_at, \
                 watchdog_nudge_count, title FROM sessions \
                 WHERE watchdog_enabled = 1 AND watchdog_prompt != '' \
                 AND status != 'archived'",
            )
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
            // Pane is still changing — skip.
            continue;
        }

        // Substitute {quiet}, {name}, {nudges} in the prompt.
        let quiet_str = sessions::humanise_duration(interval);
        let nudges_val = *nudge_count + 1;
        let rendered = prompt
            .replace("{quiet}", &quiet_str)
            .replace("{name}", title)
            .replace("{nudges}", &nudges_val.to_string());

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