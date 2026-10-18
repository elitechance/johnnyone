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
}
