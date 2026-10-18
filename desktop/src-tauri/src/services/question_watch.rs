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

use crate::services::agent_plans::record_agent_report;
use crate::services::pane_question::{pane_is_asking, PaneQuestion};
use crate::state::app_state::AppState;
use crate::terminal::run_tmux;

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

/// One polling pass: find the host's agent panes, capture each, and report every
/// question that has not been reported yet.
///
/// The shape is exactly the one the pure helpers were cut to allow:
/// [`list_panes_argv`] through [`run_tmux`] -> [`agent_pane_ids`] ->
/// [`capture_argv`] per pane -> [`QuestionWatch::sweep`] -> one
/// [`record_agent_report`] per question, with kind `"update"` (narration — what
/// the console's transcript renders) and the [`question_summary`] line as the
/// summary. Every other optional field is `None`: this is a one-line narration,
/// not a verdict, so there are no findings, severity, reason or evidence to
/// carry.
///
/// Returns how many questions were reported. A pane whose capture FAILS is
/// skipped, not fatal — a pane can die between the listing and the capture, and
/// one dead pane must not silence the other panes' asks. A failure of the
/// LISTING itself is returned as an error: without it there is nothing to sweep
/// and the caller has to know the poll did not run.
///
/// Needs a live tmux server, so it is deliberately not unit-tested; the argv
/// builders, the pane filter, the sweep and [`question_summary`] above are the
/// tested parts.
pub async fn sweep_and_report(
    state: &AppState,
    watch: &mut QuestionWatch,
    session_id: &str,
) -> Result<usize, String> {
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

    // 4. Detect, de-duplicating against what this watcher already reported.
    let found = watch.sweep(&screens);

    // 5. Report each new question to the console.
    let mut reported = 0usize;
    for (pane_id, question) in &found {
        record_agent_report(
            state,
            session_id.to_string(),
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
