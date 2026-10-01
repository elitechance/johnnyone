//! Typed key directives: `//key Escape`, `//key Down Down Enter`, `//key 2 Enter`.
//!
//! The phone console's composer is a text field, so it cannot produce Esc, Tab or arrows — yet
//! answering an agent CLI's prompt needs exactly those. Rather than build a key bar for every
//! surface, the keys are typed as text and parsed here, which serves the phone, the browser and
//! anything later from one implementation.
//!
//! Two deliberate choices:
//!
//! **Key NAMES, not raw bytes.** Each token resolves to a tmux key name handed to `send-keys`
//! without `-l`. Raw escape sequences would be wrong half the time: an arrow is `ESC [ A` in normal
//! mode but `ESC O A` in application cursor mode, and full-screen TUIs — the agent CLIs included —
//! switch into the latter. tmux knows the pane's mode and emits the right sequence. A bare escape
//! byte is worse still, since apps separate Esc from Alt-<key> by a timeout that a relay hop can
//! land inside.
//!
//! **An allowlist, not a passthrough.** This injects keystrokes into a live pane, so an unknown
//! token is an error the user sees, never text leaked into the terminal. A typo like `//key Esx`
//! must not type "Esx" into an agent's composer.

/// Prefix that marks a line as keys rather than text.
pub const KEY_PREFIX: &str = "//key";

/// Upper bound on expanded keys in one directive.
///
/// `Down*500` is either a mistake or abuse; either way it should not fire 500 keystrokes into a
/// pane. Generous enough for any real menu.
pub const MAX_KEYS: usize = 64;

/// What a `//key` line asked for.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum KeyDirective {
    /// `//key ?` — show the cheat sheet. A typed convention has no affordance, so it carries its
    /// own help.
    Help,
    /// Canonical tmux key names, in order, already expanded from any `*N` repeats.
    Keys(Vec<String>),
}

/// Parse a line of composer input.
///
/// Returns `None` when this is not a key directive, so the caller sends it as ordinary text.
/// `Some(Err)` means it *was* a directive but malformed — the caller must surface the error rather
/// than fall back to typing it, or a typo becomes terminal input.
pub fn parse_key_directive(input: &str) -> Option<Result<KeyDirective, String>> {
    // The composer appends a carriage return; strip trailing newlines before matching.
    let line = input.trim_end_matches(['\r', '\n']);

    // Must START with the prefix. A `//` elsewhere — a code comment, a URL — is untouched, which is
    // what makes pasting a snippet safe.
    let rest = line.strip_prefix(KEY_PREFIX)?;

    // `//keyboard foo` is not a directive; require a separator so the prefix is a whole word.
    if !rest.is_empty() && !rest.starts_with(|c: char| c.is_whitespace()) {
        return None;
    }

    // Keys are single-line by construction: there is no sense in a newline inside a key sequence,
    // and allowing one raises the question of whether it applies to one line or the block.
    if rest.contains('\n') {
        return Some(Err(
            "Keys must be on one line. Remove the line break.".to_string()
        ));
    }

    let body = rest.trim();
    if body.is_empty() {
        return Some(Err(format!(
            "No keys given. Try `{KEY_PREFIX} Escape` or `{KEY_PREFIX} ?` for the list."
        )));
    }
    if matches!(body, "?" | "help") {
        return Some(Ok(KeyDirective::Help));
    }

    let mut keys: Vec<String> = Vec::new();
    for token in body.split_whitespace() {
        let (name_part, repeat) = match split_repeat(token) {
            Ok(parsed) => parsed,
            Err(error) => return Some(Err(error)),
        };
        let Some(canonical) = canonical_key(name_part) else {
            return Some(Err(format!(
                "Unknown key `{name_part}`. Try `{KEY_PREFIX} ?` for the list."
            )));
        };
        if keys.len() + repeat > MAX_KEYS {
            return Some(Err(format!("Too many keys at once (limit {MAX_KEYS}).")));
        }
        for _ in 0..repeat {
            keys.push(canonical.clone());
        }
    }

    Some(Ok(KeyDirective::Keys(keys)))
}

/// Split `Down*3` into `("Down", 3)`. A plain token repeats once.
fn split_repeat(token: &str) -> Result<(&str, usize), String> {
    // A lone `*` is a literal key (`send-keys '*'`), so only treat `*` as a repeat marker when it
    // separates two non-empty halves.
    let Some(idx) = token.rfind('*') else {
        return Ok((token, 1));
    };
    let (name, count) = token.split_at(idx);
    let count = &count[1..];
    if name.is_empty() || count.is_empty() {
        return Ok((token, 1));
    }
    match count.parse::<usize>() {
        Ok(0) => Err(format!("`{token}` repeats zero times.")),
        Ok(n) if n <= MAX_KEYS => Ok((name, n)),
        Ok(_) => Err(format!("`{token}` repeats too many times (limit {MAX_KEYS}).")),
        Err(_) => Err(format!("`{count}` is not a number in `{token}`.")),
    }
}

/// Resolve one token to its tmux key name.
///
/// Input is deliberately liberal — `esc`, `ESCAPE`, `ctrl+c`, `^c` and `C-c` all land on the same
/// place — because this is typed on a phone. Output is always the canonical tmux spelling.
pub fn canonical_key(token: &str) -> Option<String> {
    let lower = token.to_ascii_lowercase();

    // Modifier forms: ctrl+x / ctrl-x / c-x / ^x, and the alt/meta equivalents.
    for prefix in ["ctrl+", "ctrl-", "control+", "c-"] {
        if let Some(rest) = lower.strip_prefix(prefix) {
            return modified_key("C", rest);
        }
    }
    if let Some(rest) = lower.strip_prefix('^') {
        return modified_key("C", rest);
    }
    for prefix in ["alt+", "alt-", "meta+", "m-"] {
        if let Some(rest) = lower.strip_prefix(prefix) {
            return modified_key("M", rest);
        }
    }
    // Shift+Tab is its own tmux name (BTab) rather than a modifier form.
    if matches!(lower.as_str(), "shift+tab" | "shift-tab" | "shifttab" | "btab" | "backtab") {
        return Some("BTab".to_string());
    }

    let named = match lower.as_str() {
        "escape" | "esc" => "Escape",
        "enter" | "return" | "cr" | "newline" => "Enter",
        "tab" => "Tab",
        "space" | "spacebar" => "Space",
        "backspace" | "bspace" | "bs" => "BSpace",
        "delete" | "del" | "dc" => "DC",
        "insert" | "ic" => "IC",
        "up" | "arrow-up" | "arrowup" => "Up",
        "down" | "arrow-down" | "arrowdown" => "Down",
        "left" | "arrow-left" | "arrowleft" => "Left",
        "right" | "arrow-right" | "arrowright" => "Right",
        "home" => "Home",
        "end" => "End",
        "pageup" | "page-up" | "pgup" => "PageUp",
        "pagedown" | "page-down" | "pgdn" | "pgdown" => "PageDown",
        _ => {
            // Function keys.
            if let Some(n) = lower.strip_prefix('f') {
                if let Ok(n) = n.parse::<u8>() {
                    if (1..=12).contains(&n) {
                        return Some(format!("F{n}"));
                    }
                }
            }
            // A single printable character is itself — this is how `//key 2 Enter` answers a
            // numbered prompt, which is the common case on a phone.
            let mut chars = token.chars();
            if let (Some(c), None) = (chars.next(), chars.next()) {
                if c.is_ascii_graphic() {
                    return Some(c.to_string());
                }
            }
            return None;
        }
    };
    Some(named.to_string())
}

/// Build `C-x` / `M-x` from a modifier and its target, rejecting anything that is not a single key.
fn modified_key(modifier: &str, rest: &str) -> Option<String> {
    let mut chars = rest.chars();
    match (chars.next(), chars.next()) {
        // tmux spells control and meta combinations with the bare character: C-c, M-x.
        (Some(c), None) if c.is_ascii_graphic() => Some(format!("{modifier}-{c}")),
        // Named targets still combine, e.g. ctrl+left for word-wise movement.
        _ => canonical_key(rest).map(|named| format!("{modifier}-{named}")),
    }
}

/// The cheat sheet reported back for `//key ?`.
pub fn help_markdown() -> String {
    format!(
        "## Sending keys\n\n\
         Start a message with `{KEY_PREFIX}` to send keys instead of text. Several keys in one \
         message are sent in order, which is what you want for a menu — `Down Down Enter` as three \
         separate messages is where you overshoot.\n\n\
         ```\n\
         {KEY_PREFIX} Escape\n\
         {KEY_PREFIX} Down Down Enter\n\
         {KEY_PREFIX} 2 Enter\n\
         {KEY_PREFIX} Down*3 Enter\n\
         {KEY_PREFIX} C-c\n\
         ```\n\n\
         | Keys | Names you can type |\n\
         |---|---|\n\
         | Arrows | `Up` `Down` `Left` `Right` |\n\
         | Escape | `Escape`, `Esc` |\n\
         | Enter | `Enter`, `Return` |\n\
         | Tab | `Tab`, `Shift+Tab` |\n\
         | Control | `C-c`, `ctrl+c`, `^c` |\n\
         | Alt | `M-x`, `alt+x` |\n\
         | Editing | `BSpace` `Delete` `Home` `End` `PageUp` `PageDown` |\n\
         | Function | `F1`–`F12` |\n\
         | A character | any single character, e.g. `2` or `y` |\n\n\
         Repeat with `*N` (`Down*3`), up to {MAX_KEYS} keys per message. Case does not matter.\n\n\
         Anything else is sent as ordinary text, so `/compact` still reaches the agent as a slash \
         command. A `//` inside a message — a code comment, a URL — is left alone; only a message \
         that *starts* with `{KEY_PREFIX}` is treated as keys.\n"
    )
}

/// How the key send is echoed into the transcript.
///
/// The console shows only reported content, so without this a key send is invisible: you tap, the
/// mirror looks unchanged, and you cannot tell whether it landed or is merely stale.
pub fn echo_line(keys: &[String]) -> String {
    format!("⌨ {}", keys.join(" "))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn keys(input: &str) -> Vec<String> {
        match parse_key_directive(input) {
            Some(Ok(KeyDirective::Keys(k))) => k,
            other => panic!("expected keys, got {other:?}"),
        }
    }

    fn error(input: &str) -> String {
        match parse_key_directive(input) {
            Some(Err(e)) => e,
            other => panic!("expected an error, got {other:?}"),
        }
    }

    // ── Not a directive ──────────────────────────────────────────────────────

    /// Ordinary text must pass straight through.
    #[test]
    fn plain_text_is_not_a_directive() {
        assert!(parse_key_directive("hello there").is_none());
        assert!(parse_key_directive("").is_none());
    }

    /// `/` belongs to the agent CLIs. Claiming it would silently eat `/compact`.
    #[test]
    fn agent_slash_commands_pass_through() {
        assert!(parse_key_directive("/compact\r").is_none());
        assert!(parse_key_directive("/clear").is_none());
    }

    /// A `//` that is not the prefix must be left alone, or pasting code breaks.
    #[test]
    fn comments_and_urls_pass_through() {
        assert!(parse_key_directive("// TODO: fix this").is_none());
        assert!(parse_key_directive("see https://example.com/x").is_none());
        assert!(parse_key_directive("const x = 1; // keyed").is_none());
    }

    /// The prefix must be a whole word.
    #[test]
    fn prefix_must_be_a_whole_word() {
        assert!(parse_key_directive("//keyboard shortcuts").is_none());
        assert!(parse_key_directive("//keys Up").is_none());
    }

    /// Only a leading prefix counts, so a key name mentioned mid-sentence is text.
    #[test]
    fn prefix_must_lead() {
        assert!(parse_key_directive("press //key Escape to exit").is_none());
    }

    // ── Keys ─────────────────────────────────────────────────────────────────

    /// The composer appends a carriage return; it must not break matching.
    #[test]
    fn trailing_carriage_return_is_stripped() {
        assert_eq!(keys("//key Escape\r"), vec!["Escape"]);
    }

    /// The case this exists for: answering a menu in one message, in order.
    #[test]
    fn sequences_keep_their_order() {
        assert_eq!(keys("//key Down Down Enter"), vec!["Down", "Down", "Enter"]);
    }

    /// Typed on a phone, so input is liberal and output canonical.
    #[test]
    fn aliases_resolve_to_tmux_names() {
        assert_eq!(keys("//key esc"), vec!["Escape"]);
        assert_eq!(keys("//key ESCAPE"), vec!["Escape"]);
        assert_eq!(keys("//key return"), vec!["Enter"]);
        assert_eq!(keys("//key pgup"), vec!["PageUp"]);
        assert_eq!(keys("//key arrow-up"), vec!["Up"]);
        assert_eq!(keys("//key del"), vec!["DC"]);
        assert_eq!(keys("//key shift+tab"), vec!["BTab"]);
    }

    /// Control and alt accept every spelling someone might reach for.
    #[test]
    fn modifier_spellings_agree() {
        for spelling in ["C-c", "c-c", "ctrl+c", "ctrl-c", "control+c", "^c"] {
            assert_eq!(keys(&format!("//key {spelling}")), vec!["C-c"], "{spelling}");
        }
        for spelling in ["M-x", "alt+x", "meta+x"] {
            assert_eq!(keys(&format!("//key {spelling}")), vec!["M-x"], "{spelling}");
        }
    }

    /// Modifiers combine with named keys too, e.g. word-wise movement.
    #[test]
    fn modifier_plus_named_key() {
        assert_eq!(keys("//key ctrl+left"), vec!["C-Left"]);
    }

    /// The common phone case: pick option 2 without driving a highlight bar.
    #[test]
    fn single_characters_are_literal() {
        assert_eq!(keys("//key 2 Enter"), vec!["2", "Enter"]);
        assert_eq!(keys("//key y"), vec!["y"]);
    }

    /// Case is preserved for literal characters even though key names ignore it.
    #[test]
    fn literal_characters_keep_their_case() {
        assert_eq!(keys("//key G"), vec!["G"]);
    }

    #[test]
    fn function_keys_resolve() {
        assert_eq!(keys("//key F1 f12"), vec!["F1", "F12"]);
        assert!(parse_key_directive("//key F13").unwrap().is_err());
    }

    /// Long menus without repeating yourself.
    #[test]
    fn repeat_expands() {
        assert_eq!(keys("//key Down*3"), vec!["Down", "Down", "Down"]);
        assert_eq!(keys("//key Down*2 Enter"), vec!["Down", "Down", "Enter"]);
    }

    /// Whitespace between tokens is not significant.
    #[test]
    fn extra_whitespace_is_ignored() {
        assert_eq!(keys("//key   Down    Enter  "), vec!["Down", "Enter"]);
    }

    /// `*` on its own is a key, not a repeat marker.
    #[test]
    fn bare_star_is_a_literal_key() {
        assert_eq!(keys("//key *"), vec!["*"]);
    }

    // ── Errors ───────────────────────────────────────────────────────────────

    /// The point of the allowlist: a typo must never be typed into the pane.
    #[test]
    fn unknown_key_is_an_error_not_text() {
        assert!(error("//key Esx").contains("Unknown key"));
        assert!(error("//key hello").contains("Unknown key"));
    }

    /// An empty directive should teach rather than do nothing.
    #[test]
    fn bare_prefix_explains_itself() {
        assert!(error("//key").contains("No keys"));
        assert!(error("//key   \r").contains("No keys"));
    }

    #[test]
    fn repeat_must_be_a_sane_number() {
        assert!(error("//key Down*0").contains("zero"));
        assert!(error("//key Down*999").contains("too many"));
        assert!(error("//key Down*x").contains("not a number"));
    }

    /// Guards a pane against a flood of keystrokes.
    #[test]
    fn total_keys_are_capped() {
        let within = format!("//key {}", vec!["Down"; MAX_KEYS].join(" "));
        assert_eq!(keys(&within).len(), MAX_KEYS);
        let over = format!("//key {}", vec!["Down"; MAX_KEYS + 1].join(" "));
        assert!(error(&over).contains("Too many keys"));
    }

    /// Multiline is rejected rather than guessed at.
    #[test]
    fn multiline_directive_is_rejected() {
        assert!(error("//key Down\nEnter").contains("one line"));
    }

    // ── Help ─────────────────────────────────────────────────────────────────

    #[test]
    fn help_is_requestable() {
        assert_eq!(parse_key_directive("//key ?"), Some(Ok(KeyDirective::Help)));
        assert_eq!(parse_key_directive("//key help\r"), Some(Ok(KeyDirective::Help)));
    }

    /// The cheat sheet must actually document the syntax it describes.
    #[test]
    fn help_mentions_the_essentials() {
        let help = help_markdown();
        for expected in ["//key Escape", "Down*3", "C-c", "Shift+Tab", "F1"] {
            assert!(help.contains(expected), "help should mention {expected}");
        }
    }

    // ── Echo ─────────────────────────────────────────────────────────────────

    /// Without an echo a key send is invisible on a console that shows only reported content.
    #[test]
    fn echo_names_the_keys_sent() {
        assert_eq!(echo_line(&["Down".into(), "Enter".into()]), "⌨ Down Enter");
    }
}
