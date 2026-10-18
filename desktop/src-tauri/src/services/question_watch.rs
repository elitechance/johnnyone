//! Question watcher: turn the pure detector in [`pane_question`] into a
//! once-per-question report for the host.
//!
//! Why this exists: an agent pane that asks the human something is blocked at
//! its own prompt and cannot report the ask itself, so the host has to notice
//! it by looking at the captured screen. The screen is re-captured constantly
//! — a waiting pane repaints many times a second — so the raw detector would
//! report the same question over and over. [`QuestionWatch`] is the layer that
//! remembers what has already been reported.
//!
//! It is deliberately pure, like the detector it wraps: no tmux, no
//! `std::process`, no `std::fs`, no network. The caller captures the screen
//! and hands the text in.

use std::collections::HashSet;
use std::time::Duration;

use crate::services::agent_plans::record_agent_report;
use crate::services::pane_question::{pane_is_asking, PaneQuestion};
use crate::state::app_state::AppState;
use crate::terminal::run_tmux;

/// How long the host waits between two question polls.
///
/// Twenty seconds is slow enough that a poll is invisible next to the agent's
/// own turn latency, and fast enough that a blocked agent is not left staring
/// at its own prompt for minutes. It is a `pub const` so the loop, the tests and
/// anyone tuning the cadence all read the same number.
pub const QUESTION_POLL_INTERVAL: Duration = Duration::from_secs(20);

/// The longest ask the console will be shown, in CHARACTERS (not bytes).
///
/// The summary line travels inside a GraphQL mutation that is itself embedded in
/// a shell argument, so it has to stay short enough to survive being quoted,
/// echoed and rendered on a phone screen. 160 characters is the agreed budget:
/// long enough for a real question, short enough that the console shows the
/// whole line without wrapping it into a second row.
const SUMMARY_MAX_CHARS: usize = 160;

/// The one line the console renders for a waiting agent.
///
/// `pane <pane_id> is waiting: <cleaned ask>`, where the cleaned ask is the
/// agent's text squeezed into a single safe line:
/// * every run of whitespace — newlines included, since the ask the detector
///   returns spans the wrapped lines of the screen — collapses to one space,
///   and the ends are trimmed;
/// * `"` and `\` are removed ENTIRELY, not escaped: the mutation this line goes
///   into is JSON inside a shell argument, and a single unescaped one of those
///   would break the payload or let the ask inject text into the mutation;
/// * the result is cut to [`SUMMARY_MAX_CHARS`] CHARACTERS on a character
///   boundary — never mid-UTF-8, which would panic on a multi-byte ask — with a
///   single `…` appended when (and only when) something was cut.
///
/// An ask that is empty, or only whitespace, yields `pane <pane_id> is waiting`
/// with NO trailing colon: a dangling `: ` would render as a broken line.
pub fn question_summary(pane_id: &str, ask: &str) -> String {
    // Collapse whitespace first, then drop the two characters that would break
    // the JSON-in-shell payload the line travels in. Order matters: stripping
    // first cannot join two words, and collapsing first cannot resurrect a
    // stripped character.
    let mut cleaned = String::with_capacity(ask.len());
    let mut prev_was_space = false;
    for ch in ask.chars() {
        if ch == '"' || ch == '\\' {
            // Removed entirely — not replaced by a space, so `say\"hi\"` stays
            // one word rather than becoming two.
            continue;
        }
        if ch.is_whitespace() {
            prev_was_space = true;
            continue;
        }
        if prev_was_space && !cleaned.is_empty() {
            cleaned.push(' ');
        }
        prev_was_space = false;
        cleaned.push(ch);
    }
    // `is_whitespace` above already swallowed any trailing run, so nothing is
    // left to trim at the end; `trim` is here for the leading run, which the
    // `!cleaned.is_empty()` guard dropped.

    if cleaned.is_empty() {
        return format!("pane {pane_id} is waiting");
    }

    // Cut on a character boundary: `char_indices` gives the BYTE offset of each
    // character's start, so truncating at one can never split a multi-byte
    // character (`floor_char_boundary` is unstable). The budget counts
    // CHARACTERS, not bytes, so the index to cut at is the byte offset of the
    // character sitting at character position `SUMMARY_MAX_CHARS` — and
    // reaching that position at all is what proves something was cut.
    let mut cut = None;
    for (count, (index, _ch)) in cleaned.char_indices().enumerate() {
        if count == SUMMARY_MAX_CHARS {
            cut = Some(index);
            break;
        }
    }
    if let Some(index) = cut {
        cleaned.truncate(index);
        cleaned.push('\u{2026}');
    }

    format!("pane {pane_id} is waiting: {cleaned}")
}

/// The `(id, tmux_pane_id)` rows needed to attribute a question, read from
/// `sessions`.
///
/// Only rows with a NON-NULL `tmux_pane_id` are read: a session with no pane is
/// not running in a pane, so it can never be the asking one. `NULL` is filtered
/// in SQL rather than in [`session_for_pane`] so the pure function never has to
/// invent a stand-in for "no pane" — and a stored empty string is handled there
/// anyway.
///
/// A failure is returned as an error string in the same shape the tmux helpers
/// use, so one pass can log it and carry on.
fn session_pane_rows(state: &AppState) -> Result<Vec<(String, String)>, String> {
    state.db.with_conn(|conn| {
        let mut stmt = conn
            .prepare("SELECT id, tmux_pane_id FROM sessions WHERE tmux_pane_id IS NOT NULL")
            .map_err(|e| format!("read session pane ids: {e}"))?;
        let rows = stmt
            .query_map([], |row| {
                let id: String = row.get(0)?;
                let pane_id: String = row.get(1)?;
                Ok((id, pane_id))
            })
            .map_err(|e| format!("read session pane ids: {e}"))?;
        let mut out = Vec::new();
        for row in rows {
            let pair = row.map_err(|e| format!("read session pane ids: {e}"))?;
            out.push(pair);
        }
        Ok(out)
    })
}

/// The session running in a pane, out of the `(session_id, tmux_pane_id)` rows
/// of `sessions`.
///
/// Returns the id of the FIRST row whose pane id equals `pane_id` EXACTLY once
/// both sides are trimmed — a pane id is an opaque token (`%12`), so a prefix or
/// a substring is not a match. `None` when nothing matches.
///
/// A row whose pane id is empty (or only whitespace) never matches ANYTHING,
/// including an empty query: an empty pane id in the database means "no pane",
/// and pairing it with a pane id that failed to parse would attribute a question
/// to a session that has no pane at all.
///
/// Pure — no tmux, no database — because which session a question belongs to is
/// a decision, and a decision is testable.
pub fn session_for_pane(rows: &[(String, String)], pane_id: &str) -> Option<String> {
    let query = pane_id.trim();
    if query.is_empty() {
        return None;
    }
    rows.iter()
        .find(|(_session_id, row_pane_id)| {
            let stored = row_pane_id.trim();
            !stored.is_empty() && stored == query
        })
        // The row is `(session_id, tmux_pane_id)`; hand back the session id.
        .map(|(session_id, _pane_id)| session_id.clone())
}

/// One polling pass: find the host's agent panes, capture each, and report every
/// question that has not been reported yet.
///
/// The shape is exactly the one the pure helpers were cut to allow:
/// [`list_panes_argv`] through [`run_tmux`] -> [`agent_pane_ids`] ->
/// [`capture_argv`] per pane -> [`QuestionWatch::sweep`] -> [`session_for_pane`]
/// per asking pane -> one [`record_agent_report`] per question, with kind
/// `"update"` (narration — what the console's transcript renders) and the
/// [`question_summary`] line as the summary. Every other optional field is
/// `None`: this is a one-line narration, not a verdict, so there are no
/// findings, severity, reason or evidence to carry.
///
/// The session a question is reported under is the ASKING PANE'S OWN, resolved
/// from `sessions.tmux_pane_id` (added by
/// `migrations/004_add_tmux_terminal.sql`) through [`session_for_pane`] — never
/// a session id handed in by the caller, which would attribute an agent's ask to
/// whichever agent happened to start the poll. A pane with no session row is
/// SKIPPED: it is not ours to report, and guessing a session for it would be
/// worse than staying silent about it.
///
/// Returns how many questions were reported. A pane whose capture FAILS is
/// skipped, not fatal — a pane can die between the listing and the capture, and
/// one dead pane must not silence the other panes' asks. A failure of the
/// LISTING itself is returned as an error: without it there is nothing to sweep
/// and the caller has to know the poll did not run.
///
/// Sweep ONLY the panes whose session is known, and return what to report.
///
/// The order matters and is the whole point of this function: the session is
/// resolved BEFORE an ask is handed to [`QuestionWatch::sweep`], so a pane with
/// no session row is never recorded as seen. Such a pane is an agent that just
/// started — its `sessions.tmux_pane_id` has not been written yet — which is
/// exactly when it is most likely to ask something; recording its ask as
/// reported here would lose the question forever, because nothing is ever sent.
/// Leaving it unreported means the pass on which the row finally appears tells
/// the human about it.
///
/// A pane whose session IS known goes through `sweep`, so its ask is recorded
/// as seen and stays reported across later passes: the de-duplication that
/// stops a constantly-repainting pane from reporting the same ask every 20
/// seconds is untouched. A pane that is reported and later has its row removed
/// stays reported — it was already told.
///
/// Whether a pass that found asking panes attributed NONE of them.
///
/// This is the shape of the failure the console can otherwise never see: a
/// fleet of agent panes whose tmux sessions still exist but whose rows were
/// deleted from `sessions` (measured on one host: 101 `johnnyone_` panes, 100
/// of them naming a session row that is gone). Every ask is dropped — correctly,
/// since guessing a session would be worse — but a watch that finds questions
/// and reports NONE of them then looks exactly like a quiet, healthy fleet.
/// That is a dead feature wearing a healthy one, so the decision is made here,
/// as data, and [`sweep_known_panes`] turns it into ONE warning per pass.
///
/// `asking_count` is how many captured panes were asking something;
/// `attributed_count` is how many of those had a session row and were kept.
///
/// * `asking > 0 && attributed == 0` -> `true`: every ask this pass found was
///   dropped because its session row is missing. Nothing was reported and
///   nothing ever will be, and only a warning says so.
/// * `attributed >= 1` -> `false`: the pass WORKED. Dropping some other pane is
///   the normal, expected case — a just-started agent whose `tmux_pane_id` has
///   not been written yet — and warning about it would cry wolf on every pass.
/// * `asking == 0` -> `false`: a quiet fleet. No pane asked anything, so there
///   is nothing this pass could have failed to attribute; the watcher is idle,
///   not broken.
///
/// Pure, and total: no combination of counts panics, including the impossible
/// `attributed > asking`, which is not an all-orphan pass either.
pub fn pass_is_all_orphan(asking_count: usize, attributed_count: usize) -> bool {
    asking_count > 0 && attributed_count == 0
}

/// Returns `(session_id, pane_id, question)` per asking pane with a known
/// session, in the order the screens were given. Pure — no tmux, no database —
/// so the ordering rule above is testable without a tmux server.
///
/// The return value is unchanged by the all-orphan warning below: an
/// unattributable pane is still skipped, still never handed to
/// [`QuestionWatch::sweep`], and still reported by nothing. Only the LOG of
/// that skip changes, once per pass, when the pass attributed nothing at all.
pub fn sweep_known_panes(
    rows: &[(String, String)],
    screens: &[(String, String)],
    watch: &mut QuestionWatch,
) -> Vec<(String, String, PaneQuestion)> {
    // 1. Resolve the session for every captured pane FIRST, and keep only the
    //    panes we can attribute. An unattributable pane is dropped here, so it
    //    never reaches `sweep` and never enters the watcher's memory.
    let mut known: Vec<(String, String, String)> = Vec::new();
    // How many of the captured panes were asking at all, whether or not we can
    // attribute them. Counted here because the drop below loses the pane, and
    // the all-orphan check further down needs to know the pass found asks.
    let mut asking_count = 0usize;
    for (pane_id, screen) in screens {
        if pane_is_asking(screen).is_some() {
            asking_count += 1;
        }
        match session_for_pane(rows, pane_id) {
            Some(session_id) => known.push((session_id, pane_id.clone(), screen.clone())),
            None => {
                tracing::debug!(
                    pane_id = %pane_id,
                    "leaving a question from a pane with no session row unreported"
                );
            }
        }
    }

    // 2. Detect and de-duplicate over just those panes, then attribute each
    //    question to the session resolved in step 1.
    let pairs: Vec<(String, String)> = known
        .iter()
        .map(|(_session_id, pane_id, screen)| (pane_id.clone(), screen.clone()))
        .collect();
    let found = watch.sweep(&pairs);
    let attributed: Vec<(String, String, PaneQuestion)> = found
        .into_iter()
        .filter_map(|(pane_id, question)| {
            let session_id = known
                .iter()
                .find(|(_session_id, known_pane, _screen)| *known_pane == pane_id)
                .map(|(session_id, _pane_id, _screen)| session_id.clone())?;
            Some((session_id, pane_id, question))
        })
        .collect();

    // 3. Say so ONCE per pass when the pass found asks but attributed none of
    //    them. The skip above is correct and unchanged — nothing is returned
    //    for an orphan pane and nothing is reported to the console — but a
    //    watch that finds questions and tells the operator NONE of them looks
    //    exactly like a quiet fleet, so the drop has to be visible somewhere.
    //    A pass that attributed at least one pane is a working pass and stays
    //    silent; so does a pass with no asking panes at all.
    if pass_is_all_orphan(asking_count, attributed.len()) {
        tracing::warn!(
            asking = asking_count,
            attributed = attributed.len(),
            "question watch found asking panes but could attribute none of them: \
             {asking_count} ask(s) were dropped because their session rows are \
             missing from `sessions`"
        );
    }

    attributed
}

/// Needs a live tmux server, so it is deliberately not unit-tested; the argv
/// builders, the pane filter, the sweep, [`session_for_pane`],
/// [`sweep_known_panes`] and [`question_summary`] are the tested parts.
pub async fn sweep_and_report(
    state: &AppState,
    watch: &mut QuestionWatch,
) -> Result<usize, String> {
    // 0. Which session owns which pane. Without this a question would have to
    //    be reported under a session id the caller guessed, which attributes
    //    the ask to whichever agent happened to start the poll.
    let rows = session_pane_rows(state)?;

    // 1. Which panes exist. A failure here is fatal to the pass.
    let listing = run_tmux(list_panes_argv()).await?;

    // 2. Which of them are the host's own agent panes.
    let pane_ids = agent_pane_ids(&listing);

    // 3. Capture each one. A pane can die in the gap between the listing and
    //    its capture, so a failed capture drops just that pane from this pass;
    //    the next pass re-lists and finds it gone.
    let mut screens: Vec<(String, String)> = Vec::new();
    for pane_id in pane_ids {
        match run_tmux(capture_argv(&pane_id)).await {
            Ok(screen) => screens.push((pane_id, screen)),
            Err(err) => {
                tracing::debug!(pane_id = %pane_id, error = %err, "skipping uncapturable pane");
            }
        }
    }

    // 4. Resolve each pane's session BEFORE the ask is recorded as seen, then
    //    detect and de-duplicate over just the panes that can be attributed.
    let found = sweep_known_panes(&rows, &screens, watch);

    // 5. Report each new question to the console, under the session that is
    //    actually running in the pane that asked.
    let mut reported = 0usize;
    for (session_id, pane_id, question) in &found {
        record_agent_report(
            state,
            session_id.clone(),
            "update".to_string(),
            None,
            None,
            Some(question_summary(pane_id, &question.ask)),
            None,
            None,
            None,
        )
        .await?;
        reported += 1;
    }
    Ok(reported)
}

/// Start the host's own question poller.
///
/// The watcher is created ONCE, outside the loop, and lives for the whole task:
/// its memory of what has been reported is what stops a waiting pane — which
/// repaints many times a second — from reporting the same ask on every single
/// pass. Rebuilding it per pass would report the question every 20 seconds.
///
/// An error from a pass is LOGGED and the loop continues. Every failure it can
/// hit is transient by nature: no tmux server yet at startup, a pane that died
/// mid-pass, a GraphQL mutation that timed out, a database row being written
/// while we read it. A poller that exits on the first one of those would stop
/// telling the human about blocked agents for the rest of the session, which is
/// worse than any of the errors it would have died on.
///
/// Needs a live tmux server to do anything, so the loop itself is not
/// unit-tested; [`QUESTION_POLL_INTERVAL`] and [`session_for_pane`] are.
pub fn spawn_question_watch(state: AppState) {
    // `tauri::async_runtime::spawn`, NOT `tokio::spawn`: this is called from Tauri's `setup`,
    // which runs on the main thread with no tokio reactor entered, so a bare `tokio::spawn`
    // panics there with "there is no reactor running" and takes the host down on boot. The two
    // long-lived tasks beside it in `main.rs` are spawned the same way.
    tauri::async_runtime::spawn(async move {
        let mut watch = QuestionWatch::new();
        // The first pass waits for the interval rather than firing at startup:
        // `main.rs` starts this alongside the terminal-status reconciliation and
        // the relay registration, and a `list-panes` competing with all of that
        // buys nothing — no agent has had time to ask anything yet.
        let mut ticker = tokio::time::interval_at(
            tokio::time::Instant::now() + QUESTION_POLL_INTERVAL,
            QUESTION_POLL_INTERVAL,
        );
        loop {
            ticker.tick().await;
            match sweep_and_report(&state, &mut watch).await {
                Ok(0) => {}
                Ok(count) => tracing::info!(count, "Reported waiting agent questions"),
                Err(error) => {
                    tracing::warn!(%error, "Question poll failed; trying again next tick")
                }
            }
        }
    });
}

/// The tmux argv that captures ONE visible screen for one pane.
///
/// `capture-pane -p -N -t <pane_id>` and nothing more:
/// * **no `-e`** — colour mode writes escape sequences into the capture, and
///   they would sit between the caret and the question text, so the detector
///   in [`pane_question`](crate::services::pane_question) would miss the ask.
/// * **no `-S`** — scrollback is not the visible screen; a waiting agent's
///   question is on the screen in front of the user, and older pages would
///   resurrect questions that have already been answered.
/// * **no `-J`** — joined lines would fold the wrapped ask into one line and
///   change what the detector reads.
///
/// Pure on purpose: the caller passes this to its tmux runner (see
/// `run_tmux` in [`crate::terminal`]) so tests can assert the argv without a
/// tmux server.
pub fn capture_argv(pane_id: &str) -> Vec<String> {
    vec![
        "capture-pane".to_string(),
        "-p".to_string(),
        "-N".to_string(),
        "-t".to_string(),
        pane_id.to_string(),
    ]
}

/// The prefix every tmux session this host owns is named with.
///
/// Confirmed in [`crate::terminal`]: `tmux_session_name(session_id)` builds the
/// owned session name as `format!("johnnyone_{}", safe)`, and
/// `list_external_tmux_sessions` skips any name that
/// `starts_with("johnnyone_")` so the user's own sessions are the ones left over
/// for the user. The prefix is the rule here too — a session merely *containing*
/// it later in the name belongs to the user and must never be swept.
pub const AGENT_SESSION_PREFIX: &str = "johnnyone_";

/// The tmux argv that lists EVERY pane on the server with the session it lives
/// in, one pane per line as `<session>\t<pane_id>`.
///
/// `list-panes -a -F '#{session_name}\t#{pane_id}'` and nothing more:
/// * **`-a`** — without it tmux lists only the panes of the *current* session,
///   and a host poller has no current session; it has to see the whole server
///   to find the agent panes it must sweep.
/// * **`-F`** — a format string, so the output is machine-readable instead of
///   the default table (which pads, aligns and can truncate a name).
/// * **`#{session_name}` first** — [`agent_pane_ids`] needs the session to
///   decide whether the pane is the host's, and the session name is the only
///   field allowed to contain odd characters, so it goes first and the pane id
///   is the last field.
///
/// Pure on purpose, exactly like [`capture_argv`]: the caller passes this to its
/// tmux runner (see `run_tmux` in [`crate::terminal`]) and feeds the output to
/// [`agent_pane_ids`], so tests can assert both halves without a tmux server.
pub fn list_panes_argv() -> Vec<String> {
    vec![
        "list-panes".to_string(),
        "-a".to_string(),
        "-F".to_string(),
        "#{session_name}\t#{pane_id}".to_string(),
    ]
}

/// Pick the panes this host owns out of `list_panes_argv()`'s output.
///
/// Returns the pane ids whose SESSION NAME starts with
/// [`AGENT_SESSION_PREFIX`], in the order the lines were given — the caller
/// sweeps in tmux's own order rather than a reordered one.
///
/// The user's own sessions are never returned: the test is `starts_with`, so a
/// session like `my_johnnyone_notes` that merely contains the prefix is the
/// user's and is skipped.
///
/// Tolerant of what tmux and a pipe can hand back: blank lines and a trailing
/// newline are skipped, a line with no tab is skipped (it carries no pane id to
/// act on), and spaces around either field are trimmed — `#{session_name}` is
/// whatever the user called the session and may well be padded. Nothing here
/// panics on any input, since the poller runs this on a timer.
pub fn agent_pane_ids(list_output: &str) -> Vec<String> {
    let mut out = Vec::new();
    for line in list_output.lines() {
        // A session name cannot contain a newline, so the first tab always
        // ends it and the rest is the pane id.
        let (session, pane_id) = match line.split_once('\t') {
            Some(pair) => pair,
            // No tab: not a listing line we can act on.
            None => continue,
        };
        if !session.trim().starts_with(AGENT_SESSION_PREFIX) {
            continue;
        }
        let pane_id = pane_id.trim();
        // An owned session with no pane id on the line is not a pane.
        if pane_id.is_empty() {
            continue;
        }
        out.push(pane_id.to_string());
    }
    out
}

/// What the host has already reported, so one question is reported once.
///
/// The memory is keyed by [`QuestionWatch::key`] — the pane id plus a digest
/// of the ask text — so identity is the *content* of the ask, not the order it
/// was seen in. A pane that goes busy and later asks the identical thing again
/// is the same question and stays reported.
#[derive(Debug, Default)]
pub struct QuestionWatch {
    seen: HashSet<String>,
}

impl QuestionWatch {
    /// An empty watcher: nothing reported yet, so the first ask seen on any
    /// pane is reported.
    pub fn new() -> Self {
        Self::default()
    }

    /// Given a pane's id and its current screen, return the question to report,
    /// or None.
    ///
    /// None when the pane is not asking, and None when this exact question was
    /// already returned for this pane — a pane redraws constantly and must not
    /// report twice. A different ask on the same pane, or the same ask on a
    /// different pane, is a different key and is reported.
    pub fn poll(&mut self, pane_id: &str, screen: &str) -> Option<PaneQuestion> {
        let found = pane_is_asking(screen)?;
        let key = Self::key(pane_id, &found.ask);
        // `insert` returns false when the key was already there, which is
        // exactly the "already reported" case.
        if !self.seen.insert(key) {
            return None;
        }
        Some(found)
    }

    /// Sweep a whole set of captured pane screens at once, returning one entry
    /// per pane that is asking something not yet reported, in the order the
    /// screens were given.
    ///
    /// Each tuple is `(pane_id, screen)`. Every screen goes through
    /// [`QuestionWatch::poll`], so the de-duplication here is exactly the one
    /// the single-pane path uses: a repaint is not re-reported, the same ask on
    /// another pane is a separate question, and a reworded ask is a new one.
    /// A pane that is not asking contributes nothing, and an empty slice
    /// sweeps to an empty vec.
    pub fn sweep(
        &mut self,
        screens: &[(String, String)],
    ) -> Vec<(String, PaneQuestion)> {
        let mut out = Vec::new();
        for (pane_id, screen) in screens {
            if let Some(found) = self.poll(pane_id, screen) {
                out.push((pane_id.clone(), found));
            }
        }
        out
    }

    /// The key a question is remembered by: the pane id plus a digest of the
    /// ask text.
    ///
    /// The digest is FNV-1a 64 over the ask bytes plus its length, so two asks
    /// that differ anywhere — including only in trailing whitespace — get
    /// different keys, while the same ask always gets the same one. The pane id
    /// and the digest are joined by a separator that cannot appear in a pane id
    /// (`%`), so `%`-prefixed ids cannot collide with another pane's key.
    fn key(pane_id: &str, ask: &str) -> String {
        let bytes = ask.as_bytes();
        let mut hash: u64 = 0xcbf29ce484222325;
        for b in bytes {
            hash ^= u64::from(*b);
            hash = hash.wrapping_mul(0x100000001b3);
        }
        format!("{pane_id}%{hash:016x}:{}", bytes.len())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    // --- which session does a pane belong to? ----------------------------
    //
    // A question reported under the WRONG session id is attributed to the wrong
    // agent, so the pane→session lookup is pure data and tested as such. The
    // rows are `(session_id, tmux_pane_id)` — the two columns
    // `sweep_and_report` reads out of `sessions`.

    fn row(session_id: &str, pane_id: &str) -> (String, String) {
        (session_id.to_string(), pane_id.to_string())
    }

    /// The lookup is by the pane's EXACT id, and it hands back the session id —
    /// the first element of the row, not the pane id.
    #[test]
    fn question_session_exact_pane_id_match_returns_that_session_id() {
        let rows = vec![
            row("11111111-1111-1111-1111-111111111111", "%1"),
            row("22222222-2222-2222-2222-222222222222", "%2"),
            row("33333333-3333-3333-3333-333333333333", "%3"),
        ];

        assert_eq!(
            session_for_pane(&rows, "%2"),
            Some("22222222-2222-2222-2222-222222222222".to_string()),
            "the middle pane must resolve to the session running in it"
        );
        // Every row is reachable, in any position.
        assert_eq!(
            session_for_pane(&rows, "%1"),
            Some("11111111-1111-1111-1111-111111111111".to_string())
        );
        assert_eq!(
            session_for_pane(&rows, "%3"),
            Some("33333333-3333-3333-3333-333333333333".to_string())
        );
        // A single row is still resolved.
        let one = vec![row("solo", "%42")];
        assert_eq!(session_for_pane(&one, "%42"), Some("solo".to_string()));
        // An exact match is not a prefix or substring match: a pane id is an
        // opaque token, so `%1` must never resolve through `%12`.
        let prefix = vec![row("wide", "%12")];
        assert_eq!(session_for_pane(&prefix, "%1"), None, "%1 is not %12");
        assert_eq!(session_for_pane(&prefix, "1"), None, "no bare-digit match");
        assert_eq!(session_for_pane(&prefix, "%1x"), None, "no prefix match");
        assert_eq!(session_for_pane(&prefix, "2"), None, "no substring match");
        // And it is case-sensitive: tmux ids are not case-folded.
        assert_eq!(session_for_pane(&prefix, "%X"), None);
    }

    /// A pane with no row is nobody's session — the caller must skip it rather
    /// than report under a guessed id.
    #[test]
    fn question_session_pane_id_matching_no_row_returns_none() {
        let rows = vec![
            row("session-a", "%1"),
            row("session-b", "%2"),
            row("session-c", ""),
        ];

        assert_eq!(session_for_pane(&rows, "%3"), None, "unlisted pane");
        assert_eq!(session_for_pane(&rows, "%99"), None, "farther along");
        assert_eq!(session_for_pane(&rows, ""), None, "empty query");
        assert_eq!(session_for_pane(&rows, "   "), None, "blank query");
        assert_eq!(session_for_pane(&rows, "%"), None, "bare caret");
        assert_eq!(session_for_pane(&rows, "session-a"), None, "not by session id");

        // No rows at all — no session has a pane yet — resolves nothing, and
        // does not panic.
        let empty: Vec<(String, String)> = vec![];
        assert_eq!(session_for_pane(&empty, "%1"), None);
        assert_eq!(session_for_pane(&empty, ""), None);
        for _ in 0..5 {
            assert_eq!(session_for_pane(&empty, "%1"), None);
        }
    }

    /// Both sides are trimmed: a stored pane id with padding and a padded query
    /// still find the same session.
    #[test]
    fn question_session_padded_rows_and_padded_query_still_match() {
        let rows = vec![
            row("session-pad", "  %7  "),
            row("session-tab", "\t%8\n"),
            row("session-plain", "%9"),
        ];

        assert_eq!(
            session_for_pane(&rows, "%7"),
            Some("session-pad".to_string()),
            "a padded stored pane id must still match a clean query"
        );
        assert_eq!(
            session_for_pane(&rows, "  %7"),
            Some("session-pad".to_string()),
            "a padded query must still match"
        );
        assert_eq!(
            session_for_pane(&rows, "\t %7 \n"),
            Some("session-pad".to_string()),
            "any surrounding whitespace is trimmed on both sides"
        );
        // Tabs and a trailing newline in the stored value too.
        assert_eq!(session_for_pane(&rows, "%8"), Some("session-tab".to_string()));
        assert_eq!(
            session_for_pane(&rows, "%8 "),
            Some("session-tab".to_string())
        );
        // A clean row is unaffected by the trimming rule.
        assert_eq!(
            session_for_pane(&rows, " %9 "),
            Some("session-plain".to_string())
        );
        // Trimming must not turn one id into another.
        assert_eq!(session_for_pane(&rows, "%77"), None);
        assert_eq!(session_for_pane(&rows, "7"), None);
    }

    /// An empty pane id in a row means "this session has no pane", so it never
    /// matches — not even an empty query.
    #[test]
    fn question_session_empty_pane_id_never_matches_even_an_empty_query() {
        let rows = vec![
            row("session-empty", ""),
            row("session-blank", "   "),
            row("session-nl", "\n"),
            row("session-real", "%1"),
        ];

        assert_eq!(
            session_for_pane(&rows, ""),
            None,
            "an empty query must not fall back onto a pane-less session"
        );
        assert_eq!(session_for_pane(&rows, "   "), None, "blank is empty");
        assert_eq!(session_for_pane(&rows, "\n"), None, "a newline is empty");
        // A row with an empty pane id matches nothing at all, empty or not.
        for query in ["", " ", "%1", "%2", "session-empty"] {
            let hit = session_for_pane(&rows, query);
            if query.trim() == "%1" {
                assert_eq!(hit, Some("session-real".to_string()), "{query:?}");
            } else {
                assert_eq!(hit, None, "{query:?} must not resolve: {hit:?}");
            }
        }
        // A lone empty row resolves nothing, whatever is asked.
        let only_empty = vec![row("orphan", "")];
        assert_eq!(session_for_pane(&only_empty, ""), None);
        assert_eq!(session_for_pane(&only_empty, "%1"), None);
        // The real row is still found despite the empty ones sitting in front.
        assert_eq!(
            session_for_pane(&rows, " %1 "),
            Some("session-real".to_string())
        );
    }

    /// Two rows claiming one pane: the FIRST wins, deterministically, so the
    /// same ask always lands on the same session.
    #[test]
    fn question_session_first_matching_row_wins_when_two_share_a_pane_id() {
        let rows = vec![
            row("session-first", "%5"),
            row("session-second", "%5"),
            row("session-third", "%5"),
            row("session-other", "%6"),
        ];

        assert_eq!(
            session_for_pane(&rows, "%5"),
            Some("session-first".to_string()),
            "the first row in table order claims the pane"
        );
        // Deterministic across repeated lookups and a padded query.
        for _ in 0..10 {
            assert_eq!(
                session_for_pane(&rows, "%5"),
                Some("session-first".to_string())
            );
        }
        assert_eq!(
            session_for_pane(&rows, " %5 "),
            Some("session-first".to_string())
        );
        // The later duplicate is simply not reachable, and the other pane is
        // untouched by the collision.
        assert_eq!(session_for_pane(&rows, "%6"), Some("session-other".to_string()));
        // An empty-pane-id duplicate cannot win either: the first USABLE row does.
        let with_empty = vec![
            row("orphan", ""),
            row("session-first", "%5"),
            row("session-second", "%5"),
        ];
        assert_eq!(
            session_for_pane(&with_empty, "%5"),
            Some("session-first".to_string()),
            "an empty row is skipped, not chosen as the first match"
        );
    }

    // --- which panes are swept, and when an ask is remembered ------------
    //
    // `sweep_and_report` resolves the pane's session BEFORE the ask is recorded
    // as seen, so a pane with no session row stays unreported and is told when
    // its row appears. That ordering is the whole decision, and
    // `sweep_known_panes` is the pure shape that makes it testable without a
    // tmux server.

    /// The regression test: a pane with no session row must produce NOTHING,
    /// and must not be remembered — the same watcher and the same screen must
    /// report the question once the row appears. Recording the ask as seen
    /// while skipping the report would lose it forever, and the pane in
    /// question is an agent that just started, i.e. exactly the one most likely
    /// to be asking.
    #[test]
    fn question_session_an_unknown_session_is_not_remembered() {
        let mut watch = QuestionWatch::new();
        let screens = vec![("%42".to_string(), waiting_screen())];

        // No session row for %42 yet — the agent just started and its
        // `tmux_pane_id` has not been written. Nothing is reported...
        let rows: Vec<(String, String)> = vec![row("session-other", "%1")];
        let first = sweep_known_panes(&rows, &screens, &mut watch);
        assert!(
            first.is_empty(),
            "a pane with no session row must not be reported: {first:?}"
        );

        // ...and the ask was NOT remembered: many later passes with the SAME
        // watcher and the SAME screen still produce nothing while the row is
        // missing, but the moment the row appears the question is told.
        for _ in 0..5 {
            assert!(
                sweep_known_panes(&rows, &screens, &mut watch).is_empty(),
                "still no row, still nothing to report"
            );
        }

        let rows_with_pane = vec![
            row("session-other", "%1"),
            row("session-new", " %42 "),
        ];
        let after = sweep_known_panes(&rows_with_pane, &screens, &mut watch);
        assert_eq!(
            after.len(),
            1,
            "the ask must survive until the session row appears: {after:?}"
        );
        assert_eq!(
            after[0].0, "session-new",
            "the question is attributed to the pane's own session"
        );
        assert_eq!(after[0].1, "%42", "the entry names the asking pane");
        assert!(
            after[0].2.ask.contains("please\nconfirm which one you want"),
            "the report carries the agent's ask: {:?}",
            after[0].2.ask
        );

        // Now that it HAS been told, it is remembered: the de-duplication is
        // intact, so the repaints after the report stay silent.
        for _ in 0..5 {
            assert!(
                sweep_known_panes(&rows_with_pane, &screens, &mut watch).is_empty(),
                "a reported question must not be reported again"
            );
        }
    }

    // --- a pass that attributes NOTHING must be visible to the operator ----
    //
    // Measured on one host: `tmux list-panes` shows 101 sessions carrying the
    // `johnnyone_` prefix, and for 100 of them the session row they name is gone
    // from `sessions`. `sweep_known_panes` drops each such pane — correct, since
    // guessing a session for it would be worse — but when EVERY ask of a pass is
    // dropped that way the watch finds questions and reports none of them, which
    // looks exactly like a quiet, healthy fleet. The decision to warn is made by
    // the pure [`pass_is_all_orphan`] so it can be tested without a tmux server
    // and without a tracing subscriber (tests cannot read log output).

    /// The dead-feature shape, named: a pass with asking panes that attributed
    /// NONE of them is an all-orphan pass and must be warned about.
    #[test]
    fn question_orphan_a_pass_that_attributes_nothing_is_reported_as_such() {
        // The measured fleet: 100 asking panes, not one of them resolvable.
        assert!(
            pass_is_all_orphan(100, 0),
            "every ask dropped because its session row is missing must be reported"
        );
        // The same shape at the smallest scale — one ask, no session row.
        assert!(pass_is_all_orphan(1, 0), "one orphan ask is still an orphan pass");
        assert!(pass_is_all_orphan(2, 0));
        assert!(pass_is_all_orphan(usize::MAX, 0));

        // A pass that attributed AT LEAST ONE pane is a working pass and must
        // not warn: dropping a just-started agent whose `tmux_pane_id` has not
        // been written yet is the normal case, not a broken watch.
        assert!(
            !pass_is_all_orphan(100, 1),
            "one attributed pane means the watch works: 99 orphans are the \
             normal just-started-agent case, not a dead feature"
        );
        assert!(!pass_is_all_orphan(2, 1));
        assert!(!pass_is_all_orphan(100, 99));
        assert!(!pass_is_all_orphan(1, 1));

        // A pass with NO asking panes at all is simply a quiet fleet — the
        // watcher is idle, not broken — and must not warn either.
        assert!(
            !pass_is_all_orphan(0, 0),
            "no asking panes is a quiet fleet, not an orphan pass"
        );
        assert!(!pass_is_all_orphan(0, 1), "nothing asked, something attributed");
        assert!(!pass_is_all_orphan(0, 50));

        // Total over every count pair, and stable across repeated calls — the
        // helper holds no state, so the same pass always decides the same way.
        for asking in 0..6usize {
            for attributed in 0..6usize {
                let once = pass_is_all_orphan(asking, attributed);
                assert_eq!(once, pass_is_all_orphan(asking, attributed));
                assert_eq!(once, asking > 0 && attributed == 0, "({asking}, {attributed})");
            }
        }

        // And the same decision the helper makes is the decision the pass makes:
        // a watcher, a screen that IS asking, and no session row for that pane
        // — the pass returns nothing (the skip is unchanged) while the counts it
        // would report to the helper are exactly (1, 0), i.e. an orphan pass.
        let mut watch = QuestionWatch::new();
        let rows = vec![row("session-other", "%1")];
        let screens = vec![("%42".to_string(), waiting_screen())];
        let found = sweep_known_panes(&rows, &screens, &mut watch);
        assert!(
            found.is_empty(),
            "an orphan pane must still not be reported: {found:?}"
        );
        assert!(
            pass_is_all_orphan(screens.len(), found.len()),
            "a pass that found an ask and attributed none is an orphan pass"
        );
        // The same screen with its row present is NOT an orphan pass.
        let mut watch = QuestionWatch::new();
        let rows_known = vec![row("session-new", "%42")];
        let found = sweep_known_panes(&rows_known, &screens, &mut watch);
        assert_eq!(found.len(), 1, "the known pane is reported");
        assert!(
            !pass_is_all_orphan(screens.len(), found.len()),
            "a pass that attributed its ask must not be called an orphan pass"
        );
    }

    /// The other side of the rule: a pass with one asking pane whose session IS
    /// known attributed everything it found, so it is not an orphan pass — even
    /// though the fleet around it may be full of orphans.
    #[test]
    fn question_orphan_a_pass_with_one_known_pane_is_not_an_orphan_pass() {
        // The helper: one ask, one attribution.
        assert!(
            !pass_is_all_orphan(1, 1),
            "a pass that attributed its one ask is a working pass"
        );
        // One known ask among many orphans is still a working pass.
        assert!(
            !pass_is_all_orphan(101, 1),
            "the measured fleet minus one orphan: the watch is alive and must \
             not warn"
        );

        // The same through the pass itself: two asking panes, only one of them
        // has a session row. The orphan is dropped and NOT reported, but the
        // pass attributed one pane, so it is not an all-orphan pass.
        let mut watch = QuestionWatch::new();
        let rows = vec![row("session-known", "%1")];
        let screens = vec![
            ("%1".to_string(), waiting_screen()),
            ("%42".to_string(), other_ask_screen()),
        ];
        let found = sweep_known_panes(&rows, &screens, &mut watch);
        assert_eq!(found.len(), 1, "only the pane with a row is reported");
        assert_eq!(found[0].0, "session-known", "under its own session");
        assert_eq!(found[0].1, "%1", "naming the asking pane");
        assert!(
            !pass_is_all_orphan(screens.len(), found.len()),
            "attributing one of two asks is not an orphan pass"
        );
        // The orphan pane is still not remembered: its ask survives until its
        // row appears, exactly as before this warning existed.
        let mut fresh = QuestionWatch::new();
        let orphan_only = vec![("%42".to_string(), other_ask_screen())];
        assert_eq!(
            sweep_known_panes(&rows, &orphan_only, &mut fresh).len(),
            0,
            "the orphan pane reports nothing"
        );
        let rows_with_orphan = vec![
            row("session-known", "%1"),
            row("session-orphan", "%42"),
        ];
        assert_eq!(
            sweep_known_panes(&rows_with_orphan, &orphan_only, &mut fresh).len(),
            1,
            "and its ask is still there once the row appears"
        );
    }

    /// A pane that DOES have a session row is reported once, and a later pass
    /// with the same watcher and the same screen produces nothing — the
    /// de-duplication must not be lost while fixing the unknown-pane case.
    #[test]
    fn question_session_a_known_session_is_remembered() {
        let mut watch = QuestionWatch::new();
        let rows = vec![row("session-a", "%1"), row("session-b", "%2")];
        let screens = vec![("%2".to_string(), waiting_screen())];

        let first = sweep_known_panes(&rows, &screens, &mut watch);
        assert_eq!(
            first.len(),
            1,
            "a pane with a session row must be reported: {first:?}"
        );
        assert_eq!(first[0].0, "session-b", "under its own session");
        assert_eq!(first[0].1, "%2", "naming the asking pane");
        assert!(
            first[0].2.ask.contains("clamp before or after"),
            "the report carries the ask: {:?}",
            first[0].2.ask
        );

        // Later passes: same watcher, same rows, same screen — nothing.
        for _ in 0..10 {
            assert!(
                sweep_known_panes(&rows, &screens, &mut watch).is_empty(),
                "a waiting pane repaints constantly and must report once"
            );
        }
        // A row that vanishes later does not un-report an already-told ask.
        let rows_now_empty: Vec<(String, String)> = vec![];
        assert!(
            sweep_known_panes(&rows_now_empty, &screens, &mut watch).is_empty(),
            "already reported stays reported"
        );
        // And a watcher built per pass would report every pass — which is why
        // the loop keeps one watcher.
        let mut fresh = QuestionWatch::new();
        assert_eq!(sweep_known_panes(&rows, &screens, &mut fresh).len(), 1);
    }

    /// The poll cadence is the one the host and the console agree on: 20 seconds.
    #[test]
    fn question_session_poll_interval_is_twenty_seconds() {
        assert_eq!(
            QUESTION_POLL_INTERVAL,
            Duration::from_secs(20),
            "the host polls for agent questions every 20 seconds"
        );
        assert_eq!(QUESTION_POLL_INTERVAL.as_secs(), 20);
        assert_eq!(QUESTION_POLL_INTERVAL.as_millis(), 20_000);
        // Not a zero or sub-second tick, which would hammer tmux: the loop
        // shells out to `list-panes` plus one capture per pane every pass.
        assert!(
            QUESTION_POLL_INTERVAL > Duration::from_secs(1),
            "a poll must not spin: {QUESTION_POLL_INTERVAL:?}"
        );
    }

    /// The loop keeps ONE watcher across passes, so the memory that makes a
    /// question report-once lives outside the pass, not inside it.
    #[test]
    fn question_session_watch_is_reused_across_passes() {
        let mut watch = QuestionWatch::new();
        let screens = vec![("%1".to_string(), waiting_screen())];

        // What the loop does every 20 seconds: same watcher, same screens.
        assert_eq!(watch.sweep(&screens).len(), 1, "the first pass reports");
        for _ in 0..5 {
            assert_eq!(
                watch.sweep(&screens).len(),
                0,
                "a later pass with the same watcher stays silent"
            );
        }
        // A watcher built per pass would report every pass — the bug the
        // single long-lived watcher in `spawn_question_watch` avoids.
        let mut fresh = QuestionWatch::new();
        assert_eq!(fresh.sweep(&screens).len(), 1);
        assert_eq!(QuestionWatch::new().sweep(&screens).len(), 1);
    }

    /// The waiting screen from `waiting_screen` in the pane_question tests:
    /// prose ask, `─` rule, status line, idle composer with a U+00A0 after the
    /// caret.
    fn waiting_screen() -> String {
        [
            "I finished the calc crate, but two of the specs conflict.",
            "",
            "Should I clamp before or after the multiply? The brief says",
            "\"saturate at u32\" while the task says \"wrap like C\" \u{2014} please",
            "confirm which one you want before I touch div.rs.",
            "",
            "\u{2500}\u{2500}\u{2500}\u{2500}\u{2500}\u{2500}\u{2500}\u{2500}\u{2500}\u{2500}\u{2500}\u{2500}\u{2500}\u{2500}",
            "\u{273b} Baked for 14s \u{b7} done 9:21 PM",
            "\u{276f}\u{a0}",
        ]
        .join("\n")
    }

    /// A different ask on the same pane: the agent moved on and asked about
    /// indexes instead. From the ask screen in
    /// `pane_is_asking_matches_the_ask_phrase_case_insensitively`.
    fn other_ask_screen() -> String {
        [
            "Two index strategies fit the schema.",
            "",
            "Should I add a covering index or a partial one?",
            "\u{276f}\u{a0}",
        ]
        .join("\n")
    }

    /// A busy pane: mid-answer, so no idle composer line. From the screen in
    /// `pane_is_asking_needs_a_question_mark_or_an_ask_phrase`.
    fn busy_screen() -> String {
        [
            "I rewrote the parser and ran the suite.",
            "All 42 tests pass and the build is clean.",
            "",
            "\u{2500}\u{2500}\u{2500}\u{2500}\u{2500}\u{2500}\u{2500}\u{2500}\u{2500}\u{2500}\u{2500}\u{2500}",
            "\u{273b} Baked for 6s \u{b7} done 9:04 PM",
            "\u{276f}\u{a0}",
        ]
        .join("\n")
    }

    #[test]
    fn question_watch_reports_a_waiting_pane_once() {
        let mut watch = QuestionWatch::new();
        let screen = waiting_screen();

        let first = watch
            .poll("%1", &screen)
            .expect("an idle composer with an ask must be reported");
        assert!(
            first.ask.contains("please\nconfirm which one you want"),
            "the report must carry the agent's ask: {:?}",
            first.ask
        );

        // The same pane repainting the same screen, over and over.
        for _ in 0..50 {
            assert!(
                watch.poll("%1", &screen).is_none(),
                "a waiting pane repaints constantly and must not report twice"
            );
        }
        // A repaint that differs only in decoration is still the same ask.
        let decorated = format!("{screen}\u{276f}\u{a0}\u{273b}");
        assert!(
            watch.poll("%1", &decorated).is_none(),
            "a trailing glyph on the composer is the same question"
        );
    }

    #[test]
    fn question_watch_reports_a_new_question_from_the_same_pane() {
        let mut watch = QuestionWatch::new();
        assert!(watch.poll("%1", &waiting_screen()).is_some());
        assert!(watch.poll("%1", &waiting_screen()).is_none());

        // The agent was answered and asked something else: report it.
        let second = watch
            .poll("%1", &other_ask_screen())
            .expect("a different ask on the same pane is a new question");
        assert!(
            second.ask.contains("covering index or a partial one?"),
            "the report must carry the new ask: {:?}",
            second.ask
        );
        assert!(watch.poll("%1", &other_ask_screen()).is_none());

        // And the first ask can come back as a new question once reworded:
        // the digest is the identity, so a change in the text is a change in
        // the question.
        let reworded = waiting_screen().replace("div.rs", "mul.rs");
        let third = watch
            .poll("%1", &reworded)
            .expect("a reworded ask is a different digest");
        assert!(
            third.ask.contains("mul.rs"),
            "ask: {:?}",
            third.ask
        );
        assert!(watch.poll("%1", &reworded).is_none());
    }

    #[test]
    fn question_watch_keeps_panes_separate() {
        let mut watch = QuestionWatch::new();
        let screen = waiting_screen();

        assert!(watch.poll("%1", &screen).is_some());
        // The identical ask on another pane is another question.
        let other = watch
            .poll("%2", &screen)
            .expect("the same ask on a different pane is reported separately");
        assert_eq!(other.ask, pane_is_asking(&screen).unwrap().ask);
        assert!(watch.poll("%2", &screen).is_none());
        assert!(watch.poll("%1", &screen).is_none());

        // Pane ids are not prefixes of one another by accident: the key
        // separates the id from the digest.
        assert!(watch.poll("%12", &screen).is_some());
        assert!(watch.poll("%12", &screen).is_none());
    }

    #[test]
    fn question_watch_ignores_a_busy_pane() {
        let mut watch = QuestionWatch::new();
        for _ in 0..5 {
            assert!(
                watch.poll("%1", &busy_screen()).is_none(),
                "idle but not asking: nothing to report"
            );
        }
        // A busy pane is not remembered, so when it later asks the very same
        // thing it is still reported — the digest is the identity, not the
        // sequence.
        assert!(watch.poll("%1", &waiting_screen()).is_some());

        // And a pane that goes busy AFTER reporting keeps its memory: the same
        // ask when it returns is not reported again.
        let mut watch2 = QuestionWatch::new();
        assert!(watch2.poll("%7", &waiting_screen()).is_some());
        assert!(watch2.poll("%7", &busy_screen()).is_none());
        assert!(
            watch2.poll("%7", &waiting_screen()).is_none(),
            "asking the same thing again is the same question"
        );
    }

    // --- wiring the watcher to a real tmux poll ---------------------------
    //
    // Everything below stays pure: `capture_argv` is asserted as data, and the
    // sweeps are driven with screens handed in, so no tmux server is needed.

    /// The capture argv names the pane and asks for the visible screen only.
    #[test]
    fn question_poll_capture_argv_names_the_pane_and_omits_colour_and_scrollback() {
        let argv = capture_argv("%3");
        assert_eq!(
            argv.first().map(String::as_str),
            Some("capture-pane"),
            "must be a capture-pane invocation: {argv:?}"
        );
        assert_eq!(
            argv,
            vec![
                "capture-pane".to_string(),
                "-p".to_string(),
                "-N".to_string(),
                "-t".to_string(),
                "%3".to_string(),
            ],
            "the argv is exactly capture-pane -p -N -t <pane_id>"
        );
        assert!(
            argv.iter().any(|a| a == "-t")
                && argv.get(argv.len().saturating_sub(1)).map(String::as_str) == Some("%3"),
            "the pane must be named by -t: {argv:?}"
        );
        // `-e` would put colour escapes between the caret and the ask text.
        assert!(
            !argv.iter().any(|a| a == "-e"),
            "no -e: escape sequences would hide the ask: {argv:?}"
        );
        // `-S` would capture scrollback instead of the visible screen.
        assert!(
            !argv.iter().any(|a| a == "-S"),
            "no -S: the detector reads the visible screen: {argv:?}"
        );
        // `-J` would join wrapped lines and change what the detector reads.
        assert!(
            !argv.iter().any(|a| a == "-J"),
            "no -J: line breaks are part of what the detector reads: {argv:?}"
        );

        // The pane id is passed through untouched, whatever it looks like.
        let other = capture_argv("%12");
        assert_eq!(other.last().map(String::as_str), Some("%12"));
        assert_eq!(other.len(), argv.len(), "same shape for any pane id");
    }

    /// Three panes, only the middle one asking: exactly one entry, and it
    /// carries the MIDDLE pane's id.
    #[test]
    fn question_poll_sweep_reports_only_the_asking_pane_with_its_own_id() {
        let mut watch = QuestionWatch::new();
        let screens = vec![
            ("%1".to_string(), busy_screen()),
            ("%2".to_string(), waiting_screen()),
            ("%3".to_string(), busy_screen()),
        ];

        let reported = watch.sweep(&screens);
        assert_eq!(
            reported.len(),
            1,
            "only the asking pane is reported: {reported:?}"
        );
        let (pane_id, found) = &reported[0];
        assert_eq!(
            pane_id, "%2",
            "the entry must carry the middle pane's id, not the caller's index"
        );
        assert_eq!(
            found.ask,
            pane_is_asking(&waiting_screen()).unwrap().ask,
            "the entry must carry that pane's ask"
        );
    }

    /// The same sweep twice: entries the first time, nothing the second.
    #[test]
    fn question_poll_sweep_reports_once_across_repeated_sweeps() {
        let mut watch = QuestionWatch::new();
        let screens = vec![
            ("%1".to_string(), busy_screen()),
            ("%2".to_string(), waiting_screen()),
            ("%3".to_string(), other_ask_screen()),
        ];

        let first = watch.sweep(&screens);
        assert_eq!(
            first.len(),
            2,
            "both asking panes are reported on the first sweep: {first:?}"
        );
        let ids: Vec<&str> = first.iter().map(|(id, _)| id.as_str()).collect();
        assert_eq!(
            ids,
            vec!["%2", "%3"],
            "entries keep the order the screens were given in"
        );

        // The poller runs again on unchanged screens — a waiting pane repaints
        // constantly — and must stay silent.
        for _ in 0..10 {
            assert!(
                watch.sweep(&screens).is_empty(),
                "a repaint of an already-reported question is not a new ask"
            );
        }
    }

    /// A pane that moves on and asks something else is reported again.
    #[test]
    fn question_poll_sweep_reports_a_new_question_from_the_same_pane() {
        let mut watch = QuestionWatch::new();
        let first_sweep = vec![("%4".to_string(), waiting_screen())];
        let reported = watch.sweep(&first_sweep);
        assert_eq!(reported.len(), 1, "the first ask is reported");
        assert!(reported[0].1.ask.contains("clamp before or after"));

        // Answered, then asking about indexes instead: a new question.
        let second_sweep = vec![("%4".to_string(), other_ask_screen())];
        let again = watch.sweep(&second_sweep);
        assert_eq!(
            again.len(),
            1,
            "a different ask on the same pane must be reported again"
        );
        assert_eq!(again[0].0, "%4");
        assert!(
            again[0].1.ask.contains("covering index or a partial one?"),
            "the report must carry the NEW ask: {:?}",
            again[0].1.ask
        );

        // And that second ask is now remembered too.
        assert!(watch.sweep(&second_sweep).is_empty());
        // The old ask coming back verbatim is still the old question.
        assert!(watch.sweep(&first_sweep).is_empty());
    }

    /// Nothing to look at: an empty slice sweeps to an empty vec, no panic.
    #[test]
    fn question_poll_sweep_of_no_screens_is_empty() {
        let mut watch = QuestionWatch::new();
        let empty: Vec<(String, String)> = vec![];
        assert!(
            watch.sweep(&empty).is_empty(),
            "no screens can mean no reports"
        );
        // Repeatedly, and still no panic; the watcher is untouched.
        for _ in 0..5 {
            assert!(watch.sweep(&empty).is_empty());
        }

        // A sweep of only non-asking panes is just as empty...
        let quiet = vec![
            ("%1".to_string(), busy_screen()),
            ("%2".to_string(), String::new()),
        ];
        assert!(watch.sweep(&quiet).is_empty(), "no ask, no report");
        // ...and did not poison the watcher: a real ask after it is reported.
        let asking = vec![("%2".to_string(), waiting_screen())];
        assert_eq!(watch.sweep(&asking).len(), 1);
    }

    // --- finding the panes to sweep --------------------------------------
    //
    // Which panes to capture is a decision, so it is expressed as data (the
    // argv) and as a pure parse (the filter), both assertable without a tmux
    // server. The poller's shape is: `list_panes_argv()` -> tmux ->
    // `agent_pane_ids` -> `capture_argv` per pane -> `QuestionWatch::sweep`.

    /// The listing argv asks for every pane on the server in a parseable form.
    #[test]
    fn question_host_list_panes_argv_lists_every_pane_with_session_and_id() {
        let argv = list_panes_argv();
        assert_eq!(
            argv.first().map(String::as_str),
            Some("list-panes"),
            "must be a list-panes invocation: {argv:?}"
        );
        // `-a` is what widens the listing from the current session to the
        // whole server — without it the host cannot see its agent panes.
        assert!(
            argv.iter().any(|a| a == "-a"),
            "must carry -a so every session's panes are listed: {argv:?}"
        );
        // `-F` is what makes the output parseable instead of a padded table.
        assert!(
            argv.iter().any(|a| a == "-F"),
            "must carry -F so the output is one record per line: {argv:?}"
        );
        let format = argv
            .iter()
            .find(|a| a.contains("#{session_name}") && a.contains("#{pane_id}"))
            .unwrap_or_else(|| {
                panic!("the format must name both the session and the pane: {argv:?}")
            });
        // The session must be identifiable as the host's, and the pane must be
        // capturable, so both fields are required and separated by a tab.
        assert!(
            format.contains('\t'),
            "the two fields must be tab-separated: {format:?}"
        );
        assert!(
            format.find("#{session_name}").unwrap() < format.find("#{pane_id}").unwrap(),
            "the session name comes first so a name containing a tab cannot \
             confuse the parse: {format:?}"
        );
        // Nothing else: no `-a`-competing target, no `-t`, no `-s`.
        assert!(
            !argv.iter().any(|a| a == "-t" || a == "-s" || a == "-e"),
            "a whole-server listing names no target and wants no colour: {argv:?}"
        );
        // Stable across calls — the poller builds it every tick.
        assert_eq!(argv, list_panes_argv());
    }

    /// A real-looking mixed listing: only the owned panes come back, in order.
    #[test]
    fn question_host_agent_pane_ids_returns_only_the_host_panes_in_order() {
        // Modelled on the live host: the user's own sessions sit alongside the
        // `johnnyone_<session id>` ones this host spawned.
        let listing = concat!(
            "kloo\t%1\n",
            "johnnyone_768d2c33-d85a-4013-9cce-722948c87a69\t%2\n",
            "kloo-cli\t%3\n",
            "johnnyone_11111111-2222-3333-4444-555555555555\t%4\n",
            "my work\t%5\n",
            "johnnyone_dead-1\t%6\n",
        );

        let ids = agent_pane_ids(listing);
        assert_eq!(
            ids,
            vec!["%2".to_string(), "%4".to_string(), "%6".to_string()],
            "only the johnnyone_ panes, in the order tmux listed them"
        );

        // The user's panes are never in there, in any position.
        assert!(
            !ids.iter().any(|id| id == "%1" || id == "%3" || id == "%5"),
            "the user's own sessions must never be swept: {ids:?}"
        );

        // A single owned pane is still found, and a listing of only the
        // user's sessions yields nothing at all.
        assert_eq!(
            agent_pane_ids("johnnyone_abc\t%9\n"),
            vec!["%9".to_string()]
        );
        assert!(
            agent_pane_ids("kloo\t%1\nkloo-cli\t%3\n").is_empty(),
            "nothing owned means nothing to sweep"
        );
    }

    /// The prefix is the rule: `johnnyone_` anywhere else in the name is the
    /// user's session and must not be touched.
    #[test]
    fn question_host_agent_pane_ids_requires_the_prefix_not_a_substring() {
        let listing = concat!(
            "my_johnnyone_notes\t%1\n",
            "xjohnnyone_abc\t%2\n",
            "work-johnnyone_test\t%3\n",
            "JOHNNYONE_abc\t%4\n",
            "johnnyone_\t%5\n",
            "johnnyone_real\t%6\n",
        );

        let ids = agent_pane_ids(listing);
        assert_eq!(
            ids,
            vec!["%5".to_string(), "%6".to_string()],
            "only names STARTING with johnnyone_ are the host's: {ids:?}"
        );
        for not_ours in ["%1", "%2", "%3", "%4"] {
            assert!(
                !ids.iter().any(|id| id == not_ours),
                "{not_ours} sits in a session that merely contains the \
                 prefix (or differs in case) and must be left alone"
            );
        }
        // `johnnyone_` with an empty id is still an owned session name.
        assert!(ids.contains(&"%5".to_string()));
    }

    /// Ragged input is skipped, never a panic: blank lines, a trailing newline,
    /// a line with no tab, and padded fields.
    #[test]
    fn question_host_agent_pane_ids_tolerates_blank_tabless_and_padded_lines() {
        let listing = concat!(
            "\n",
            "   \n",
            "johnnyone_a\t%1\n",
            "no tab on this line at all\n",
            "johnnyone_b\n",
            "\t%2\n",
            "  johnnyone_c  \t  %3  \n",
            "kloo\t%4\n",
            "johnnyone_d\t\n",
            "johnnyone_e\t%5\n",
            "\n",
            "\n",
        );

        let ids = agent_pane_ids(listing);
        assert_eq!(
            ids,
            vec!["%1".to_string(), "%3".to_string(), "%5".to_string()],
            "blank, tabless, session-less and id-less lines are skipped; \
             padded fields are trimmed"
        );

        // The shapes that must never panic, on their own.
        assert!(agent_pane_ids("").is_empty(), "empty string");
        assert!(agent_pane_ids("\n").is_empty(), "just a newline");
        assert!(agent_pane_ids("\n\n\n").is_empty(), "just newlines");
        assert!(agent_pane_ids("\t").is_empty(), "just a tab");
        assert!(agent_pane_ids("no tab here").is_empty(), "no tab");
        assert!(agent_pane_ids("\t\t\t").is_empty(), "tabs only");
        // A pane id is the last field, so extra fields cannot panic; they are
        // simply part of what tmux put after the first tab.
        let extra = agent_pane_ids("johnnyone_x\t%1\textra\tfields");
        assert_eq!(extra.len(), 1, "one line, one pane id: {extra:?}");
        assert!(
            extra[0].starts_with("%1"),
            "the pane id still leads the remainder: {extra:?}"
        );
        // A trailing newline is not a phantom pane.
        assert_eq!(
            agent_pane_ids("johnnyone_x\t%1\n"),
            agent_pane_ids("johnnyone_x\t%1").to_vec(),
            "a trailing newline must not change the answer"
        );
    }

    /// Nothing listed — no tmux server, or a server with no panes — means
    /// nothing to sweep.
    #[test]
    fn question_host_agent_pane_ids_of_an_empty_listing_is_empty() {
        let mut watch = QuestionWatch::new();

        let ids = agent_pane_ids("");
        assert!(ids.is_empty(), "an empty listing gives no pane ids: {ids:?}");
        // And the whole poll collapses to a no-op rather than an error.
        let screens: Vec<(String, String)> = ids
            .into_iter()
            .map(|id| (id, String::new()))
            .collect();
        assert!(
            watch.sweep(&screens).is_empty(),
            "no panes means no reports"
        );
        // Repeatedly, and still nothing.
        for _ in 0..5 {
            assert!(agent_pane_ids("").is_empty());
        }
        // An empty listing is not the same as a lost memory: once a pane does
        // appear, its ask is reported.
        let later = agent_pane_ids("johnnyone_new\t%7\n");
        assert_eq!(later, vec!["%7".to_string()]);
        let screens = vec![("%7".to_string(), waiting_screen())];
        assert_eq!(watch.sweep(&screens).len(), 1);
    }

    // --- the summary line the console renders ----------------------------
    //
    // `question_summary` is the only part of the report the phone console
    // actually shows, and the line travels inside a GraphQL mutation that is
    // itself JSON inside a shell argument. So it is tested as text: exact
    // output, one line, no `"` or `\`, and a cut that never lands mid-UTF-8.
    // `sweep_and_report` needs a live tmux server and is deliberately not
    // tested here — these are the parts that decide what the user reads.

    /// A plain one-line ask: the line is exactly the documented text, with the
    /// pane id and the ask in it and nothing added.
    #[test]
    fn question_report_summary_is_the_exact_line_for_a_plain_ask() {
        let line = question_summary("%3", "Should I use a map or a vec?");
        assert_eq!(
            line,
            "pane %3 is waiting: Should I use a map or a vec?",
            "the line is exactly `pane <pane_id> is waiting: <ask>`"
        );
        // Shape guards for the console's renderer: one line, and it names the
        // pane that is blocked.
        assert_eq!(line.lines().count(), 1, "the console renders one line");
        assert!(line.starts_with("pane %3 is waiting: "), "{line}");
        assert!(line.ends_with("vec?"), "the ask is not clipped: {line}");
        assert!(!line.contains('\u{2026}'), "nothing was cut: {line}");
        // A short ask keeps its own punctuation and case untouched.
        assert!(line.contains('?'), "the ask mark survives: {line}");
    }

    /// The detector hands back the ask across the screen's wrapped lines, so a
    /// multi-line ask must collapse to one line with SINGLE spaces — including
    /// runs of tabs/spaces and a leading indent.
    #[test]
    fn question_report_summary_collapses_a_multi_line_ask_to_single_spaces() {
        let ask = "Should I clamp before or after the multiply?\n  \t The brief says saturate,\n\n the task says wrap.\n";
        let line = question_summary("%1", ask);
        assert_eq!(
            line,
            "pane %1 is waiting: Should I clamp before or after the multiply? \
             The brief says saturate, the task says wrap.",
            "newlines, tabs and runs of spaces all become one space, and the \
             ends are trimmed"
        );
        assert_eq!(line.lines().count(), 1, "no newline may survive");
        assert!(!line.contains('\n'), "one line only: {line:?}");
        assert!(!line.contains("  "), "no double space may survive: {line}");
        assert!(!line.contains('\t'), "no tab may survive: {line}");
        assert_eq!(
            line.matches(": ").count(),
            1,
            "only the `: ` after `waiting` separates: {line}"
        );
        // A trailing run of whitespace must not leave a dangling space either.
        let trailing = question_summary("%1", "wrap or saturate?   \n\n  ");
        assert_eq!(trailing, "pane %1 is waiting: wrap or saturate?");
        // And the real waiting screen's ask collapses the same way.
        let ask = pane_is_asking(&waiting_screen()).unwrap().ask;
        let real = question_summary("%2", &ask);
        assert_eq!(real.lines().count(), 1, "the real ask is one line");
        assert!(!real.contains("  "), "single spaces only: {real}");
    }

    /// `"` and `\` are removed ENTIRELY — not escaped — because one of them
    /// left in would break the JSON-in-shell mutation the line rides in.
    #[test]
    fn question_report_summary_strips_quotes_and_backslashes() {
        let line = question_summary(
            "%9",
            "The brief says \"saturate at u32\" while the task says \\wrap\\ — which?",
        );
        assert_eq!(
            line,
            "pane %9 is waiting: The brief says saturate at u32 while the task \
             says wrap — which?",
            "both quote styles are gone, and nothing replaces them"
        );
        assert!(!line.contains('"'), "no `\"` may reach the mutation: {line}");
        assert!(!line.contains('\\'), "no `\\` may reach the mutation: {line}");
        // Stripping must not invent a space: an escaped quote inside a word
        // leaves the word intact.
        let joined = question_summary("%1", "say \\\"hi\\\" now");
        assert_eq!(joined, "pane %1 is waiting: say hi now", "{joined}");
        assert!(!joined.contains("  "), "stripping cannot open a gap: {joined}");
        // A quote-only ask is emptied by stripping and so takes the no-colon
        // form rather than rendering `: ` out of nothing.
        assert_eq!(
            question_summary("%1", "\"\\\""),
            "pane %1 is waiting",
            "nothing left to show means no colon"
        );
        // An ask trying to close the mutation and inject its own field gets
        // neutered: neither character is present to close anything with.
        let hostile = question_summary("%1", "\" }, summary: \"pwned");
        assert!(!hostile.contains('"'), "inert: {hostile}");
    }

    /// An ask longer than the 160-character budget is cut to exactly 160
    /// characters plus ONE ellipsis — and a 160-character ask is not cut at all.
    #[test]
    fn question_report_summary_cuts_a_long_ask_to_160_plus_one_ellipsis() {
        // 200 ASCII characters: `word` + space, repeated, so the cut lands on a
        // word boundary and the count is easy to state exactly.
        let ask = "word ".repeat(40);
        assert_eq!(ask.chars().count(), 200);
        let line = question_summary("%5", ask.trim_end());
        let shown = line
            .strip_prefix("pane %5 is waiting: ")
            .expect("the line keeps its prefix");
        assert_eq!(
            shown.chars().count(),
            161,
            "160 kept characters plus a single ellipsis: {shown}"
        );
        assert!(
            shown.ends_with('\u{2026}'),
            "the cut is marked by one ellipsis: {shown}"
        );
        assert_eq!(
            shown.matches('\u{2026}').count(),
            1,
            "exactly one ellipsis, appended: {shown}"
        );
        assert_eq!(
            shown.trim_end_matches('\u{2026}'),
            ask.trim_end().chars().take(160).collect::<String>(),
            "the first 160 characters are kept verbatim"
        );
        assert!(!shown.ends_with(' '), "no dangling space before the …: {shown}");

        // Exactly 160 characters: nothing was cut, so no ellipsis.
        let exact = "a".repeat(160);
        let kept = question_summary("%5", &exact);
        assert_eq!(
            kept,
            format!("pane %5 is waiting: {exact}"),
            "a 160-character ask fits and must not be marked as cut"
        );
        // 161 characters: one is cut, so exactly one ellipsis.
        let over = question_summary("%5", &"a".repeat(161));
        let shown = over.strip_prefix("pane %5 is waiting: ").unwrap();
        assert_eq!(shown.chars().count(), 161, "160 + …: {shown}");
        assert!(shown.ends_with('\u{2026}'));
    }

    /// A multi-byte ask must be cut on a CHARACTER boundary — slicing at byte
    /// 160 would panic — and the budget counts characters, not bytes.
    #[test]
    fn question_report_summary_cuts_a_multibyte_ask_on_a_char_boundary() {
        // 4-byte characters: byte 160 is also a character start, but byte 158
        // and 159 are not, so a byte-based cut would panic.
        let emoji = "\u{1F600}".repeat(200);
        assert_eq!(emoji.len(), 800, "4 bytes each");
        let line = question_summary("%6", &emoji);
        let shown = line
            .strip_prefix("pane %6 is waiting: ")
            .expect("the line keeps its prefix");
        assert_eq!(
            shown.chars().count(),
            161,
            "160 characters plus the ellipsis: {}",
            shown.chars().count()
        );
        assert!(shown.ends_with('\u{2026}'), "marked as cut: {shown}");
        assert!(shown.is_ascii() == false, "the emoji survived: {shown}");
        // The kept part is exactly the first 160 emoji, unsplit.
        assert_eq!(
            shown.trim_end_matches('\u{2026}'),
            "\u{1F600}".repeat(160),
            "the cut never splits a character"
        );

        // 2-byte characters (é, ü, ñ, å) and 3-byte (CJK) — the shapes that a
        // byte-offset cut would land inside of.
        let two_byte = "é".repeat(300);
        assert_eq!(two_byte.len(), 600);
        let line = question_summary("%6", &two_byte);
        let shown = line.strip_prefix("pane %6 is waiting: ").unwrap();
        assert_eq!(shown.chars().count(), 161);
        assert_eq!(shown.trim_end_matches('\u{2026}'), "é".repeat(160));

        let three_byte = "漢字テスト".repeat(60);
        let line = question_summary("%6", &three_byte);
        let shown = line.strip_prefix("pane %6 is waiting: ").unwrap();
        assert_eq!(shown.chars().count(), 161);
        assert!(shown.is_char_boundary(shown.len()), "valid UTF-8");

        // Mixed multi-byte text with spaces, as a real wrapped ask looks: still
        // one line, still valid UTF-8, still 160 + … .
        let mixed = "Should I clamp before ".repeat(20) + "— 漢字 ünïcode \u{1F600} end";
        let line = question_summary("%6", &mixed);
        assert_eq!(line.lines().count(), 1, "one line: {line}");
        let shown = line.strip_prefix("pane %6 is waiting: ").unwrap();
        assert_eq!(shown.chars().count(), 161, "{}", shown.chars().count());
        assert!(shown.ends_with('\u{2026}'));
        // And a short multi-byte ask is untouched, no ellipsis.
        let short = question_summary("%6", "möchtest du das?");
        assert_eq!(short, "pane %6 is waiting: möchtest du das?");
    }

    /// An ask that is empty or only whitespace yields the no-colon form — a
    /// dangling `: ` would render as a broken line on the console.
    #[test]
    fn question_report_summary_of_an_empty_ask_has_no_colon() {
        for ask in ["", " ", "   ", "\n", "\n\n", "\t", " \t \n  ", "\u{a0}"] {
            let line = question_summary("%4", ask);
            assert_eq!(
                line,
                "pane %4 is waiting",
                "an ask of {ask:?} must yield the no-colon form"
            );
            assert!(!line.ends_with(':'), "no dangling colon: {line}");
            assert!(!line.ends_with(' '), "no trailing space: {line:?}");
            assert!(!line.contains(':'), "no colon at all: {line}");
            assert_eq!(line.lines().count(), 1);
        }
        // The pane id is still named, so the user knows which pane to look at.
        assert!(question_summary("%42", "  \n ").starts_with("pane %42 "));
        // An ask of only the characters that get stripped is empty too.
        assert_eq!(question_summary("%4", "  \"\" \\"), "pane %4 is waiting");
        // And the no-colon form is not applied to a real ask.
        assert!(question_summary("%4", "which one?").contains(": which one?"));
    }

    /// The line is built from the pane id and the ask the sweep actually
    /// returns, so the summary a report carries names the pane that asked.
    #[test]
    fn question_report_summary_names_the_pane_the_sweep_reported() {
        let mut watch = QuestionWatch::new();
        let screens = vec![
            ("%1".to_string(), busy_screen()),
            ("%2".to_string(), waiting_screen()),
            ("%3".to_string(), other_ask_screen()),
        ];
        let found = watch.sweep(&screens);
        assert_eq!(found.len(), 2, "both asking panes are reported");

        let lines: Vec<String> = found
            .iter()
            .map(|(pane_id, q)| question_summary(pane_id, &q.ask))
            .collect();
        // Each line carries its OWN pane id — never the caller's, never the
        // other pane's.
        assert!(lines[0].starts_with("pane %2 is waiting: "), "{}", lines[0]);
        assert!(lines[1].starts_with("pane %3 is waiting: "), "{}", lines[1]);
        assert!(!lines[0].contains("%3"), "no cross-pane id: {}", lines[0]);
        assert!(!lines[1].contains("%2"), "no cross-pane id: {}", lines[1]);
        // Both are single safe lines, and each carries its own ask.
        for line in &lines {
            assert_eq!(line.lines().count(), 1, "{line:?}");
            assert!(!line.contains('"') && !line.contains('\\'), "{line:?}");
            assert!(line.chars().count() <= 21 + 161, "bounded: {}", line);
        }
        assert!(lines[0].contains("clamp before or after"), "{}", lines[0]);
        assert!(
            lines[1].contains("covering index or a partial one?"),
            "{}",
            lines[1]
        );
        // A busy pane contributes no line at all.
        assert!(!lines.iter().any(|l| l.contains("%1")));
    }
}
