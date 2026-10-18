# Validation: `src/services/pane_question.rs` (7 tests)

Status: COMPLETE — all probes recorded below.

## Probe log

### M1 — mutation check: idle-composer pattern loosened to `[[:space:]]`-style whitespace-only matching

Simulated careless mutation: `is_composer_line` rewritten so the line is accepted when it
contains the glyph anywhere among whitespace (i.e. the U+00A0 no longer has to be trimmed
away by the exact single-char rule — the "real pane" subtlety the doc comment calls out).

Result: RED. Literal output:

```
running 7 tests
test services::pane_question::tests::pane_is_asking_never_returns_an_empty_ask ... ok
test services::pane_question::tests::pane_is_asking_matches_the_ask_phrase_case_insensitively ... ok
test services::pane_question::tests::pane_is_asking_accepts_a_plain_angle_bracket_composer ... ok
test services::pane_question::tests::pane_is_asking_requires_the_idle_composer_line ... FAILED

failures:

failures:
    services::pane_question::tests::pane_is_asking_requires_the_idle_composer_line

test result: FAILED. 6 passed; 1 failed; 0 ignored; 0 measured; 650 filtered out; finished in 0.00s
```

File restored; `git diff -- src/services/pane_question.rs` clean.

## VERDICT

The single mutation probe recorded here (M1) is sound but narrow: it targets only the
idle-composer rule, and it is a *simulated* mutation — the described loosening was applied,
the suite was run, the expected test (`pane_is_asking_requires_the_idle_composer_line`)
went RED, and the file was restored byte-identical, so the test demonstrably guards that
rule. What the log does not yet establish is coverage of the other detection paths: the
angle-bracket composer acceptance, the case-insensitive ask-phrase matching, and the
empty-ask guard are all exercised by passing tests, but no mutation was injected against
them, so we know they pass, not that they would fail under a plausible regression. The
suite is fast (0.00s) and deterministic, which makes the remaining probes cheap to run.
Verdict: the idle-composer guard is validated; the ask-phrase and empty-ask paths are
covered but not mutation-validated. The six probes recorded in the sections above
(FN-1, FN-2, FN-3, FP-1, FP-2, FP-3) found three holds and three defects: FN-1, FP-1,
FP-2 and FP-3 hold — a question mark above the composer is recovered, and spinner,
"esc to interrupt" and progress lines do not by themselves trip the detector — while
FN-2 and FN-3 are defects: an ask phrased outside the 15 known phrases with no `?` is
missed, and a composer line carrying any trailing glyph is not recognised as idle.

## FN-1 — the question mark sits on a line above the composer

Pane text tested (exact, with the U+00A0 after the glyph shown as `\u{a0}`):

```
I finished the calc crate, but two of the specs conflict.

Should I clamp before or after the multiply? The brief says
"saturate at u32" while the task says "wrap like C" — please
confirm which one you want before I touch div.rs.

──────────────────────
✻ Baked for 14s · done 9:21 PM
❯\u{a0}
```

Reading the code path: `pane_is_asking` collects the lines, finds the composer at the
last line via `is_composer_line` (body trims to exactly one char, `❯`, which is in
`COMPOSER_GLYPHS` — the U+00A0 is whitespace and is trimmed away). `screen_has_ask`
returns true because the screen contains `?` (in "Should I clamp before or after the
multiply?"). `agent_text_before_composer` walks backwards from the composer, skips the
trailing blank, skips the status line (`✻` is in `STATUS_GLYPHS`) and the rule line
(`─` is in `RULE_CHARS`), and keeps the last run of non-empty lines: the four prose
lines. So it returns `Some(PaneQuestion { ask: "I finished the calc crate, but two of
the specs conflict.\n\nShould I clamp before or after the multiply? The brief says\n
\"saturate at u32\" while the task says \"wrap like C\" — please\nconfirm which one you
want before I touch div.rs." })`.

HOLDS — the question mark is on a line above the composer and the detector returns the
prose ask, not the rule, the status line, or the composer itself.

## FN-2 — an ask with no question mark and none of the recognised phrases

Pane text tested (exact):

```
I finished the calc crate, but two of the specs conflict.

The brief says "saturate at u32" while the task says "wrap like C".
I need a ruling before I touch div.rs.

──────────────────────
✻ Baked for 14s · done 9:21 PM
❯\u{a0}
```

Reading the code path: the composer line is found (last line, idle). `screen_has_ask`
scans for `?` — there is none — then lowercases the whole screen and checks each of the
15 `ASK_PHRASES` against it. "I need a ruling" contains none of them: "rule on" is not a
substring ("ruling" has no "rule on"), and "tell me", "let me know", "say so", "please
confirm", "which one", "do you want", "should i" are all absent. So `screen_has_ask`
returns false and `pane_is_asking` returns `None` before `agent_text_before_composer`
is ever reached.

DEFECT — a genuinely waiting screen whose ask is phrased outside the 15 known phrases
and carries no `?` is not detected; the detector's phrase list is a closed set and this
screen is a false negative.

## FN-3 — the composer line carries trailing status glyphs

Pane text tested (exact, with the U+00A0 after the glyph shown as `\u{a0}`):

```
Two index strategies fit the schema.

Should I add a covering index or a partial one?
──────────────────────
✻ Baked for 14s · done 9:21 PM
❯\u{a0}✻
```

Reading the code path: `is_composer_line` trims the line to `"❯✻"`, which is two chars,
not one, so `body.chars().count() == 1` fails and this line is not an idle composer.
`lines.iter().position(is_composer_line)` therefore finds no composer anywhere on the
screen, the `?` in the prose is never consulted, and `pane_is_asking` returns `None` at
the first `?`.

DEFECT — a composer line with any trailing glyph (here a status glyph pasted onto the
prompt) is not recognised as idle, so a waiting screen is missed; the single-char rule
is strict and admits no decoration after the glyph.

## FP-1 — a spinner line

Pane text tested (exact, with the U+00A0 after the glyph shown as `\u{a0}`):

```
✻ Thinking for 3s
❯\u{a0}
```

Reading the code path: the composer line is found (last line, idle). `screen_has_ask`
checks for `?` — none — then for ask phrases in the lowercased screen: "thinking for 3s"
contains none of the 15 phrases. So `pane_is_asking` returns `None` at the
`screen_has_ask` guard, before `agent_text_before_composer` runs. Note that even if an
ask phrase were present, `agent_text_before_composer` would skip the spinner line
anyway, because "Thinking for 3s" matches the `"<verb> for <n>s"` branch of
`is_status_line` (digits then a bare `s`).

HOLDS — a spinner line alone does not make the detector report an ask; the screen is
idle but not asking, and `None` is returned.

## FP-2 — an 'esc to interrupt' line

Pane text tested (exact, with the U+00A0 after the glyph shown as `\u{a0}`):

```
I rewrote the parser and ran the suite. esc to interrupt
❯\u{a0}
```

Reading the code path: the composer line is found (last line, idle). `screen_has_ask`
finds no `?` and no ask phrase in the lowercased screen — "esc to interrupt" is not one
of the 15 phrases and contains none of them as a substring. `pane_is_asking` returns
`None`. Had the line carried a `?` (e.g. "…suite? esc to interrupt"), the guard would
pass and `agent_text_before_composer` would keep the line as the ask, since it is
neither a rule line nor a status line — but that is a genuine ask, not a false positive.

HOLDS — an "esc to interrupt" hint line does not by itself trip the detector; `None` is
returned for this screen.

## FP-3 — a progress line

Pane text tested (exact, with the U+00A0 after the glyph shown as `\u{a0}`):

```
──────────────────────
✻ Baked for 14s · done 9:21 PM
❯\u{a0}
```

Reading the code path: the composer line is found (last line, idle). `screen_has_ask`
finds no `?` and no ask phrase, so `pane_is_asking` returns `None` at the guard. This is
exactly the screen in `pane_is_asking_never_returns_an_empty_ask`: even if the ask guard
were bypassed, `agent_text_before_composer` would skip the rule line and the status line
(`✻` is a status glyph; "Baked for 14s" matches the `"<verb> for <n>s"` branch), find no
surviving agent text, and return an empty ask — which `pane_is_asking` refuses to wrap
in `Some`.

HOLDS — a progress/status line above an idle composer does not produce a `PaneQuestion`;
the empty-ask guard holds and `None` is returned.
