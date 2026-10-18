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

use crate::services::pane_question::{pane_is_asking, PaneQuestion};

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
}
