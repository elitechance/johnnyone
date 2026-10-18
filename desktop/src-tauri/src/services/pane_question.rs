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

/// True when a line is the idle composer: its only content is the glyph.
///
/// On a real pane the character following the glyph is U+00A0 (NO-BREAK
/// SPACE), which `char::is_whitespace()` accepts and `trim()` removes — a
/// byte comparison against b' ' would not.
fn is_composer_line(line: &str) -> bool {
    let body = line.trim();
    body.chars().count() == 1 && COMPOSER_GLYPHS.contains(&body.chars().next().unwrap())
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

/// True when the screen carries an ask: a question mark, or one of the
/// known ask phrases (case-insensitively).
fn screen_has_ask(screen: &str) -> bool {
    if screen.contains('?') {
        return true;
    }
    let lower = screen.to_lowercase();
    ASK_PHRASES.iter().any(|p| lower.contains(p))
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
/// `Some` only when the screen shows an IDLE COMPOSER line (its only content
/// is `❯` or `>`, optionally surrounded by whitespace — note that on a real
/// pane the character after the glyph is U+00A0) *and* the screen carries an
/// ask (a `?` or one of `ASK_PHRASES`) *and* some of the agent's own text
/// survives filtering above the composer. A pane mid-answer never shows an
/// idle composer line, so it yields `None`.
pub fn pane_is_asking(screen: &str) -> Option<PaneQuestion> {
    let lines: Vec<&str> = screen.lines().collect();
    let composer_idx = lines.iter().position(|l| is_composer_line(l))?;
    if !screen_has_ask(screen) {
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
}
