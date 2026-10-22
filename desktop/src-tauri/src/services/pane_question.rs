//! Pane-question detector: recognise a captured pane screen that is WAITING
//! on a human, and recover the ask it is waiting with.
//!
//! Why this exists: the phone console only shows what reaches the
//! `reportAgentResult` mutation. An agent that asks the user a question and
//! then waits cannot report it — it is blocked at its own prompt — so the
//! user never sees the question. This module is the pure detector that makes
//! such a pane recognisable. It is deliberately free of tmux, `std::process`,
//! and `std::fs`: it takes the already-captured screen text.

/// A captured pane screen that is waiting on a human, and the ask it is
/// waiting with.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PaneQuestion {
    pub ask: String,
}

/// The glyphs that render an idle composer prompt on a real pane.
const COMPOSER_GLYPHS: [char; 2] = ['\u{276f}', '>'];

/// Box-drawing rule characters: a line made only of these (plus whitespace)
/// is a separator, not the agent's text.
const RULE_CHARS: [char; 1] = ['\u{2500}'];

/// Leading glyphs of the status line ("✻ Baked for 14s · done 9:21 PM").
const STATUS_GLYPHS: [char; 5] =
    ['\u{273b}', '\u{2733}', '\u{2736}', '\u{273d}', '\u{b7}'];

/// Asks that mean "waiting on a human" without a question mark, matched
/// case-insensitively.
const ASK_PHRASES: [&str; 15] = [
    "answer here",
    "let me know",
    "tell me",
    "say so",
    "please confirm",
    "please choose",
    "please decide",
    "rule on",
    "decide whether",
    "waiting on you",
    "blocked on",
    "which one",
    "do you want",
    "would you like",
    "should i",
];

/// Most agent text kept for `ask` — the last run of consecutive non-empty
/// lines before the composer.
const MAX_ASK_LINES: usize = 12;

/// True when a line is the idle composer: the caret glyph followed by
/// nothing but whitespace and/or trailing decoration glyphs.
///
/// The line is judged by its PREFIX — the caret plus the non-alphanumeric
/// run that follows it — so glyphs pasted *after* the caret (a status glyph
/// stuck onto the prompt, a spinner, a rule fragment) still read as idle,
/// while a typed word does not.
///
/// On a real pane the character following the glyph is U+00A0 (NO-BREAK
/// SPACE), which `char::is_whitespace()` accepts but which a `[[:space:]]`
/// style whitespace class would not — hence the non-alphanumeric class
/// rather than a whitespace class.
fn is_composer_line(line: &str) -> bool {
    let body = line.trim();
    let mut chars = body.chars();
    let Some(first) = chars.next() else {
        return false;
    };
    if !COMPOSER_GLYPHS.contains(&first) {
        return false;
    }
    // The run straight after the caret must be non-alphanumeric: U+00A0,
    // spaces, or decoration glyphs. A letter or a digit means the human has
    // started typing, so the pane is no longer waiting.
    chars.all(|c| !c.is_alphanumeric())
}

/// True when a line is only box-drawing rules and whitespace.
fn is_rule_line(line: &str) -> bool {
    let body = line.trim();
    !body.is_empty()
        && body
            .chars()
            .all(|c| RULE_CHARS.contains(&c) || c.is_whitespace())
}

/// True when a line is the status line: it starts with one of the status
/// glyphs, or reads "<verb> for <n>s".
fn is_status_line(line: &str) -> bool {
    let body = line.trim();
    if body.is_empty() {
        return false;
    }
    if STATUS_GLYPHS.contains(&body.chars().next().unwrap()) {
        return true;
    }
    // "<verb> for <n>s" — e.g. "Baked for 14s", "Thinking for 3s".
    match body.find(" for ") {
        Some(at) => {
            let after = &body[at + " for ".len()..];
            let digits_len = after.chars().take_while(|c| c.is_ascii_digit()).count();
            if digits_len == 0 {
                return false;
            }
            let rest = &after[digits_len..];
            match rest.strip_suffix('s') {
                // The duration must be the whole tail, or end a word
                // ("Baked for 14s · done 9:21 PM").
                Some(tail) => tail.is_empty() || tail.starts_with(' '),
                None => false,
            }
        }
        None => false,
    }
}

/// Verbs that hand the decision to the reader — "let me know which one you
/// want", "tell me which", "advise which", "confirm whether". An ask phrased
/// as one of these imperatives carries no question mark and falls outside the
/// closed `ASK_PHRASES` set, so it needs this second, narrower sweep.
///
/// The sweep is keyed on the DECISION WORD that must follow the verb, which
/// is what keeps a busy screen out of it: a spinner line, an
/// "esc to interrupt" hint and a progress line never address the reader that
/// way, so none of them can trip the detector.
const ASK_VERBS: [&str; 6] = [
    "let me know",
    "tell me",
    "advise me",
    "advise",
    "confirm",
    "clarify",
];
const ASK_DECISION_WORDS: [&str; 8] = [
    "which",
    "whether",
    "if",
    "what",
    "how",
    "when",
    "where",
    "who",
];
/// "I need a ruling", "I require a decision", "your call", "over to you" —
/// statements that hand the decision over without a question mark and without
/// an imperative verb. Same safety property as `ASK_VERBS`: they are only ever
/// written by something waiting for the reader, never by a spinner, an
/// "esc to interrupt" hint or a progress line.
const ASK_HANDOFFS: [&str; 4] = ["i need", "i require", "your call", "over to you"];

/// True when the screen carries an ask: a question mark, one of the known
/// ask phrases, an ask imperative followed by a decision word, or one of the
/// decision hand-offs (all case-insensitively).
fn screen_has_ask(screen: &str) -> bool {
    if screen.contains('?') {
        return true;
    }
    let lower = screen.to_lowercase();
    ASK_PHRASES.iter().any(|p| lower.contains(p))
        || ask_imperative_in(&lower)
        || ask_handoff_in(&lower)
}

/// The lowercased screen split into alphanumeric words (U+00A0, box-drawing
/// rules and status glyphs are separators, so neither the composer line nor
/// the status line can contribute one).
fn words_of(lower_screen: &str) -> Vec<&str> {
    lower_screen
        .split(|c: char| !c.is_alphanumeric())
        .filter(|w| !w.is_empty())
        .collect()
}

/// True when the lowercased screen reads "<ask verb> <decision word>" — the
/// verb and the word adjacent, so "let me know which" and "CONFIRM  whether"
/// count while "i needed", "confirmed by CI" and "telling me" do not.
fn ask_imperative_in(lower_screen: &str) -> bool {
    let words = words_of(lower_screen);
    ASK_VERBS.iter().any(|verb| {
        let verb_words: Vec<&str> = verb.split(' ').collect();
        let want = verb_words.len() + 1;
        words.windows(want).any(|window| {
            window[..verb_words.len()] == verb_words[..]
                && ASK_DECISION_WORDS.contains(&window[want - 1])
        })
    })
}

/// True when the lowercased screen contains one of `ASK_HANDOFFS` as a whole
/// phrase — its words adjacent, so "i needed" and "your caller" do not count.
fn ask_handoff_in(lower_screen: &str) -> bool {
    let words = words_of(lower_screen);
    ASK_HANDOFFS.iter().any(|phrase| {
        let want = phrase.split(' ').count();
        words.windows(want).any(|window| window.join(" ") == *phrase)
    })
}

/// The last non-empty block of the agent's own text before the composer:
/// drop rule lines and the status line, keep the last run of up to
/// `MAX_ASK_LINES` consecutive non-empty lines.
fn agent_text_before_composer(lines: &[&str], composer_idx: usize) -> String {
    let mut kept: Vec<&str> = Vec::with_capacity(MAX_ASK_LINES);
    for &line in lines[..composer_idx].iter().rev() {
        let body = line.trim();
        if body.is_empty() {
            if !kept.is_empty() {
                break; // end of the last run
            }
            continue; // trailing blanks after the status line
        }
        if is_rule_line(body) || is_status_line(body) {
            if !kept.is_empty() {
                break;
            }
            continue;
        }
        kept.push(body);
        if kept.len() == MAX_ASK_LINES {
            break;
        }
    }
    kept.reverse();
    kept.join("\n").trim().to_string()
}

/// Detect a pane screen waiting on a human.
///
/// `Some` only when the screen shows an IDLE COMPOSER line (the caret `❯` or
/// `>` followed by nothing but whitespace and/or trailing decoration glyphs —
/// note that on a real pane the character after the glyph is U+00A0) *and*
/// the screen carries an ask (a `?`, one of `ASK_PHRASES`, or one of the
/// `ASK_VERBS` hand-offs in `ask_imperative_in`) *and* some of the agent's
/// own text survives filtering above the composer. A pane mid-answer never
/// shows an idle composer line, so it yields `None`.
pub fn pane_is_asking(screen: &str) -> Option<PaneQuestion> {
    let lines: Vec<&str> = screen.lines().collect();
    let composer_idx = lines.iter().position(|l| is_composer_line(l))?;
    // Only the text ABOVE the composer can carry the ask: a `?` or a phrase
    // sitting on the composer line itself is chrome, not a question.
    let above = lines[..composer_idx].join("\n");
    if !screen_has_ask(&above) {
        return None;
    }
    let ask = agent_text_before_composer(&lines, composer_idx);
    if ask.is_empty() {
        return None;
    }
    Some(PaneQuestion { ask })
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A real waiting screen: prose ask, `─` rule, status line, and an idle
    /// composer whose trailing character is U+00A0 (NO-BREAK SPACE), not an
    /// ordinary space.
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

    #[test]
    fn pane_is_asking_real_waiting_screen() {
        let screen = waiting_screen();
        let found = pane_is_asking(&screen).expect("idle composer + explicit ask must be detected");
        assert!(
            found.ask.contains("please\nconfirm which one you want"),
            "ask must carry the prose ask: {:?}",
            found.ask
        );
        assert!(
            !found.ask.contains('\u{2500}'),
            "ask must not carry the rule line: {:?}",
            found.ask
        );
        assert!(
            !found.ask.contains("Baked for 14s"),
            "ask must not carry the status line: {:?}",
            found.ask
        );
        assert!(
            !found.ask.contains('\u{276f}'),
            "ask must not carry the composer: {:?}",
            found.ask
        );
    }

    #[test]
    fn pane_is_asking_requires_the_idle_composer_line() {
        // The same screen with the composer line removed.
        let no_composer = waiting_screen().replace("\n\u{276f}\u{a0}", "");
        assert!(
            pane_is_asking(&no_composer).is_none(),
            "a pane mid-answer never shows an idle composer line"
        );
        // A composer line already carrying typed text is not idle either.
        let typing = waiting_screen().replace("\u{276f}\u{a0}", "\u{276f}\u{a0}wrap like C");
        assert!(pane_is_asking(&typing).is_none());
    }

    #[test]
    fn pane_is_asking_needs_a_question_mark_or_an_ask_phrase() {
        let screen = [
            "I rewrote the parser and ran the suite.",
            "All 42 tests pass and the build is clean.",
            "",
            "\u{2500}\u{2500}\u{2500}\u{2500}\u{2500}\u{2500}\u{2500}\u{2500}\u{2500}\u{2500}\u{2500}\u{2500}",
            "\u{273b} Baked for 6s \u{b7} done 9:04 PM",
            "\u{276f}\u{a0}",
        ]
        .join("\n");
        assert!(pane_is_asking(&screen).is_none(), "idle but not asking");
    }

    #[test]
    fn pane_is_asking_matches_the_ask_phrase_case_insensitively() {
        let screen = [
            "Two index strategies fit the schema.",
            "",
            "Should I add a covering index or a partial one?",
            "\u{276f}\u{a0}",
        ]
        .join("\n");
        let found = pane_is_asking(&screen).expect("\"Should I\" is an ask phrase");
        assert!(found.ask.contains("Should I"), "ask: {:?}", found.ask);
    }

    #[test]
    fn pane_is_asking_never_returns_an_empty_ask() {
        // Idle composer and a question mark, but nothing of the agent's own
        // text survives the filtering.
        let screen = [
            "\u{2500}\u{2500}\u{2500}\u{2500}\u{2500}\u{2500}\u{2500}\u{2500}\u{2500}\u{2500}\u{2500}\u{2500}",
            "\u{273b} Baked for 14s \u{b7} done 9:21 PM",
            "",
            "\u{276f}\u{a0}",
        ]
        .join("\n");
        assert!(pane_is_asking(&screen).is_none(), "must not yield an empty ask");
    }

    #[test]
    fn pane_is_asking_keeps_the_last_run_of_at_most_twelve_lines() {
        let mut lines: Vec<String> = vec![
            "I checked the earlier notes.".to_string(),
            String::new(),
        ];
        for i in 0..15 {
            lines.push(format!("choice {i}: should I take the narrow path"));
        }
        lines.push("\u{276f}\u{a0}".to_string());
        let screen = lines.join("\n");
        let found = pane_is_asking(&screen).expect("ask phrase present");
        let kept: Vec<&str> = found.ask.lines().collect();
        assert_eq!(kept.len(), 12, "ask: {:?}", found.ask);
        assert!(kept[0].starts_with("choice 3"), "ask: {:?}", found.ask);
        assert!(kept[11].starts_with("choice 14"), "ask: {:?}", found.ask);
    }

    #[test]
    fn pane_is_asking_accepts_a_plain_angle_bracket_composer() {
        let screen = "Which backend should I wire up?\n> \n";
        let found = pane_is_asking(screen).expect("`>` is an idle composer glyph");
        assert_eq!(found.ask, "Which backend should I wire up?");
    }

    #[test]
    fn pane_is_asking_catches_an_ask_with_no_question_mark_or_known_phrase() {
        // FN-2: an agent that ends its turn handing the decision over, with no
        // `?` anywhere and none of the 15 `ASK_PHRASES` present.
        let screen = [
            "Let me know which one you want and I will run it.",
            "",
            "\u{2500}\u{2500}\u{2500}\u{2500}\u{2500}\u{2500}\u{2500}\u{2500}\u{2500}\u{2500}\u{2500}\u{2500}\u{2500}\u{2500}",
            "\u{273b} Baked for 14s \u{b7} done 9:21 PM",
            "\u{276f}\u{a0}",
        ]
        .join("\n");
        let found = pane_is_asking(&screen)
            .expect("an imperative hand-off is an ask even with no `?` and no known phrase");
        assert!(
            found.ask.starts_with("Let me know which one you want"),
            "ask must carry the hand-off: {:?}",
            found.ask
        );
        assert!(
            !found.ask.contains('\u{2500}') && !found.ask.contains("Baked for 14s"),
            "ask must not carry the rule or the status line: {:?}",
            found.ask
        );

        // The FN-2 screen from the findings file: no `?`, no known phrase, and
        // the hand-off is a statement rather than an imperative.
        let ruling = [
            "I finished the calc crate, but two of the specs conflict.",
            "",
            "The brief says \"saturate at u32\" while the task says \"wrap like C\".",
            "I need a ruling before I touch div.rs.",
            "",
            "\u{2500}\u{2500}\u{2500}\u{2500}\u{2500}\u{2500}\u{2500}\u{2500}\u{2500}\u{2500}\u{2500}\u{2500}\u{2500}\u{2500}",
            "\u{273b} Baked for 14s \u{b7} done 9:21 PM",
            "\u{276f}\u{a0}",
        ]
        .join("\n");
        let found = pane_is_asking(&ruling)
            .expect("\"I need a ruling\" hands the decision over without a `?`");
        assert!(
            found.ask.contains("I need a ruling"),
            "ask must carry the prose ask: {:?}",
            found.ask
        );

        // The three false-positive probes from the findings file must still
        // hold: widening the ask test must not swallow a busy screen.
        let spinner = "\u{273b} Thinking for 3s\n\u{276f}\u{a0}";
        assert!(
            pane_is_asking(spinner).is_none(),
            "a spinner line alone is idle but not asking"
        );
        let interrupt = [
            "I rewrote the parser and ran the suite. esc to interrupt",
            "\u{276f}\u{a0}",
        ]
        .join("\n");
        assert!(
            pane_is_asking(&interrupt).is_none(),
            "an \"esc to interrupt\" hint does not by itself trip the detector"
        );
        let progress = [
            "\u{2500}\u{2500}\u{2500}\u{2500}\u{2500}\u{2500}\u{2500}\u{2500}\u{2500}\u{2500}\u{2500}\u{2500}\u{2500}\u{2500}",
            "\u{273b} Baked for 14s \u{b7} done 9:21 PM",
            "\u{276f}\u{a0}",
        ]
        .join("\n");
        assert!(
            pane_is_asking(&progress).is_none(),
            "a progress line above an idle composer yields no ask"
        );
    }

    #[test]
    fn pane_is_asking_tolerates_trailing_glyphs_on_the_composer_line() {
        // FN-3: the composer line carries a status glyph pasted after the
        // caret, so the line is judged by its prefix and the trailing glyph is
        // ignored.
        let screen = [
            "Two index strategies fit the schema.",
            "",
            "Should I add a covering index or a partial one?",
            "\u{2500}\u{2500}\u{2500}\u{2500}\u{2500}\u{2500}\u{2500}\u{2500}\u{2500}\u{2500}\u{2500}\u{2500}\u{2500}\u{2500}",
            "\u{273b} Baked for 14s \u{b7} done 9:21 PM",
            "\u{276f}\u{a0}\u{273b}",
        ]
        .join("\n");
        let found = pane_is_asking(&screen)
            .expect("a status glyph pasted after the caret must not hide the idle composer");
        assert!(
            found.ask.contains("covering index or a partial one?"),
            "ask must carry the question: {:?}",
            found.ask
        );
        assert!(
            !found.ask.contains('\u{276f}') && !found.ask.contains("Baked for 14s"),
            "ask must not carry the composer or the status line: {:?}",
            found.ask
        );

        // Several trailing decoration glyphs at once, and the same for `>`.
        let decorated = format!(
            "Which migration do you want first?\n\u{276f}\u{a0}\u{273b}\u{2500}\u{b7} "
        );
        let found =
            pane_is_asking(&decorated).expect("trailing decoration glyphs still read as idle");
        assert_eq!(found.ask, "Which migration do you want first?");
        let angle = "Which migration do you want first?\n> \u{273b}\n";
        assert!(pane_is_asking(angle).is_some(), "`>` with a trailing glyph is idle");

        // A trailing glyph is decoration, not typed text: a real word after the
        // caret means the human is typing and the pane is not waiting.
        let typing = format!("Which migration do you want first?\n\u{276f}\u{a0}\u{273b}yes");
        assert!(
            pane_is_asking(&typing).is_none(),
            "a typed word after the caret is not an idle composer"
        );
    }
}
