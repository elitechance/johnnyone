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
    watchdog_prompt, watchdog_last_nudge_at, watchdog_nudge_count, title, watchdog_on_blocked, \
    watchdog_on_stale_report FROM sessions \
    WHERE watchdog_enabled = 1 AND watchdog_prompt != '' AND status != 'archived'";

/// Which trigger (if any) should fire a nudge for a session on this tick.
///
/// The three real reasons plus "nothing yet", as ONE decision so the precedence between the
/// timer and the two "Also nudge when" checkboxes lives in exactly one place and is testable
/// without an `AppState` harness or a tmux pane.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum WatchdogTrigger {
    /// The interval has elapsed since the last nudge / most recent report.
    Timer,
    /// The "blocked" checkbox is on and the pane is sitting on a question — fires at once.
    Blocked,
    /// The "stale report" checkbox is on and nothing has been reported for twice the interval.
    StaleReport,
    /// No trigger applies; skip this session this tick.
    NotDue,
}

/// Decide whether a nudge is due for one session, and WHY.
///
/// Precedence, in order:
/// 1. `on_blocked && pane_is_asking` → [`WatchdogTrigger::Blocked`], IGNORING the interval
///    entirely. A pane waiting on an answer is stuck no matter how recently it last spoke, and
///    waiting out the interval to notice is the whole complaint the checkbox answers.
/// 2. `on_stale_report` and `latest_report_at` older than **2 × interval** →
///    [`WatchdogTrigger::StaleReport`]. Twice, deliberately: at one interval the timer trigger
///    usually fires anyway, so the checkbox only means something beyond that.
/// 3. [`sessions::watchdog_is_due`] → [`WatchdogTrigger::Timer`].
/// 4. otherwise → [`WatchdogTrigger::NotDue`].
///
/// `last` is the later of `watchdog_last_nudge_at` and `latest_report_at`, as the sweep computes
/// it. A `None` means "never nudged and nothing reported", which `watchdog_is_due` reads as due.
/// A missing `latest_report_at` is never "stale" — a session that has never reported is covered by
/// rule 3, not by a stale-report rule that would fire on every tick.
pub fn watchdog_trigger_decision(
    now_utc: &chrono::DateTime<chrono::Utc>,
    last: Option<&str>,
    interval_secs: u64,
    latest_report_at: Option<&str>,
    pane_is_asking: bool,
    on_blocked: bool,
    on_stale_report: bool,
) -> WatchdogTrigger {
    if on_blocked && pane_is_asking {
        return WatchdogTrigger::Blocked;
    }

    if on_stale_report {
        if let Some(report_at) = latest_report_at {
            if let Some(age_secs) = sessions::host_timestamp_age_secs(now_utc, report_at) {
                if age_secs >= interval_secs.saturating_mul(2) {
                    return WatchdogTrigger::StaleReport;
                }
            }
        }
    }

    if sessions::watchdog_is_due(now_utc, last, interval_secs) {
        return WatchdogTrigger::Timer;
    }

    WatchdogTrigger::NotDue
}

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

#[cfg(test)]
mod watchdog_trigger_tests {
    use super::WatchdogTrigger::{Blocked, NotDue, StaleReport, Timer};
    use super::*;

    /// A fixed clock, so the assertions do not race two `Utc::now()` reads. Deriving the "past"
    /// timestamps from a second `now` leaves the age a hair under the whole second intended, which
    /// is precisely the boundary the 2× stale test is about.
    fn now() -> chrono::DateTime<chrono::Utc> {
        chrono::DateTime::parse_from_rfc3339("2024-01-15T10:00:00Z")
            .unwrap()
            .with_timezone(&chrono::Utc)
    }

    /// `now - secs`, as the SQLite `datetime('now')` string the host actually stores.
    fn ago_secs(secs: i64) -> String {
        (now() - chrono::Duration::seconds(secs))
            .format("%Y-%m-%d %H:%M:%S")
            .to_string()
    }

    /// The checkbox that promises "fires at once, ignores the interval" must keep that promise: the
    /// interval has NOT elapsed here (nudged 10s ago on a 300s timer), yet a pane sitting on a
    /// question is exactly the case worth interrupting.
    #[test]
    fn watchdog_trigger_fires_at_once_when_blocked_on_a_question() {
        let last = ago_secs(10);
        let report = ago_secs(10);
        assert_eq!(
            watchdog_trigger_decision(
                &now(),
                Some(&last),
                300,
                Some(&report),
                true, // the pane is asking
                true, // on_blocked
                false,
            ),
            Blocked,
            "a blocked pane must fire immediately, not after the remaining 290s"
        );

        // And with the pane NOT asking, the same row is simply not due yet.
        assert_eq!(
            watchdog_trigger_decision(&now(), Some(&last), 300, Some(&report), false, true, false),
            NotDue,
            "the blocked trigger must not fire on a pane that is not asking"
        );
    }

    /// The stale-report trigger means "twice the interval", so the boundary is 2× and not 1× — at
    /// 1× the timer trigger would have fired anyway and the checkbox would mean nothing.
    #[test]
    fn watchdog_trigger_fires_when_reports_are_stale_for_twice_the_interval() {
        // Interval 300 ⇒ the threshold is 600s of silence.
        let last = ago_secs(60);
        let just_under = ago_secs(599);
        assert_eq!(
            watchdog_trigger_decision(
                &now(),
                Some(&last),
                300,
                Some(&just_under),
                false,
                false,
                true, // on_stale_report
            ),
            NotDue,
            "599s of silence is not yet twice the 300s interval"
        );

        let just_over = ago_secs(601);
        assert_eq!(
            watchdog_trigger_decision(&now(), Some(&last), 300, Some(&just_over), false, false, true),
            StaleReport,
            "601s of silence is past twice the interval"
        );

        // Exactly on the boundary counts as stale (>=, matching `watchdog_is_due`).
        let exactly = ago_secs(600);
        assert_eq!(
            watchdog_trigger_decision(&now(), Some(&last), 300, Some(&exactly), false, false, true),
            StaleReport
        );

        // A session that has NEVER reported is not "stale" — the timer owns that case.
        assert_eq!(
            watchdog_trigger_decision(&now(), Some(&last), 300, None, false, false, true),
            NotDue,
            "no report at all must not read as a stale report"
        );
    }

    /// Both checkboxes off ⇒ neither special trigger may fire, no matter how blocked the pane looks
    /// or how stale the reports are. The timer is still free to fire — that is not a checkbox.
    #[test]
    fn watchdog_trigger_respects_the_checkboxes_being_off() {
        // Blocked pane, ancient reports, both flags false, and the timer NOT elapsed.
        let last = ago_secs(10);
        let stale = ago_secs(3600);
        assert_eq!(
            watchdog_trigger_decision(&now(), Some(&last), 300, Some(&stale), true, false, false),
            NotDue,
            "a blocked pane must not fire with the checkbox off"
        );
        assert_eq!(
            watchdog_trigger_decision(&now(), Some(&last), 300, Some(&stale), false, false, false),
            NotDue,
            "a stale report must not fire with the checkbox off"
        );

        // Same inputs, timer elapsed ⇒ Timer, and never Blocked/StaleReport.
        let old = ago_secs(301);
        assert_eq!(
            watchdog_trigger_decision(&now(), Some(&old), 300, Some(&stale), true, false, false),
            Timer,
            "with both checkboxes off the only reason left is the timer"
        );

        // Precedence: blocked outranks a stale report, which outranks the timer.
        assert_eq!(
            watchdog_trigger_decision(&now(), Some(&old), 300, Some(&stale), true, true, true),
            Blocked
        );
        assert_eq!(
            watchdog_trigger_decision(&now(), Some(&old), 300, Some(&stale), false, false, true),
            StaleReport
        );
        assert_eq!(
            watchdog_trigger_decision(&now(), Some(&old), 300, Some(&old), false, false, false),
            Timer
        );
    }

    /// The sweep reads the two flags off `WATCHDOG_CANDIDATE_SQL`, so a candidate row must CARRY
    /// them — a SELECT that drops a column makes both checkboxes decoration again, silently.
    #[test]
    fn watchdog_candidate_sql_selects_both_trigger_flags() {
        let conn = rusqlite::Connection::open_in_memory().expect("in-memory db");
        for migration in [
            include_str!("../../migrations/001_initial.sql"),
            include_str!("../../migrations/027_add_session_watchdog.sql"),
            include_str!("../../migrations/028_add_watchdog_held_count.sql"),
            include_str!("../../migrations/029_add_watchdog_triggers.sql"),
        ] {
            conn.execute_batch(migration).expect("migration applies");
        }
        conn.execute(
            "INSERT INTO sessions (id, title, provider, watchdog_enabled, watchdog_prompt, \
             watchdog_on_blocked, watchdog_on_stale_report) \
             VALUES ('s1', 'kord', 'claude_code', 1, 'Report.', 1, 0)",
            [],
        )
        .expect("seed an armed row");

        let (on_blocked, on_stale) = conn
            .query_row(WATCHDOG_CANDIDATE_SQL, [], |row| {
                Ok((row.get::<_, i64>(6)? != 0, row.get::<_, i64>(7)? != 0))
            })
            .expect("the candidate row must read without failing");
        assert!(on_blocked, "WATCHDOG_CANDIDATE_SQL must select watchdog_on_blocked");
        assert!(
            !on_stale,
            "WATCHDOG_CANDIDATE_SQL must select watchdog_on_stale_report at index 7"
        );
    }
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
                    // The two "Also nudge when" checkboxes. Selected here because the sweep is the
                    // only reader, and a candidate row that never carries them means the checkboxes
                    // are decoration — which is exactly what they were before migration 029.
                    row.get::<_, i64>(6)? != 0, // on_blocked
                    row.get::<_, i64>(7)? != 0, // on_stale_report
                ))
            })
            .map_err(|e| e.to_string())?
            .filter_map(|r| r.ok())
            .collect::<Vec<_>>();
        Ok(rows)
    })?;

    for (id, interval_secs, prompt, last_nudge_at, nudge_count, title, on_blocked, on_stale_report) in
        &candidates
    {
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

        // Cheap pre-check on the timer alone, so a session that is simply not due yet does not pay
        // for two pane captures every 20s. The AUTHORITATIVE decision — including the two checkbox
        // triggers, which can fire when the timer has NOT elapsed — is made below, after the
        // captures, because `pane_is_asking` needs the second one.
        if !*on_blocked && !*on_stale_report && !sessions::watchdog_is_due(&now, last_ts, interval) {
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

        // The ONE decision, made on the SECOND capture: the timer, the blocked checkbox (via the
        // real `pane_question` detector — not a second, divergent guess at what "asking" means),
        // and the stale-report checkbox, with their precedence resolved in `session_watchdog`.
        let trigger = watchdog_trigger_decision(
            &now,
            last_ts,
            interval,
            latest_report.as_deref(),
            crate::services::pane_question::pane_is_asking(&second.content).is_some(),
            *on_blocked,
            *on_stale_report,
        );

        if trigger == WatchdogTrigger::NotDue {
            continue;
        }

        // `Timer` and `StaleReport` still respect the idle probe — a nudge typed into a pane that is
        // mid-turn lands in the middle of the agent's own output. `Blocked` deliberately does NOT:
        // a pane sitting on a question is not moving, so the probe would either be pointless or (for
        // a spinner-ish redraw) suppress the one case worth interrupting.
        if trigger != WatchdogTrigger::Blocked && a != b {
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