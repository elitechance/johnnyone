//! QA A's ROUND-5 tests: the fourth audit of the invisibility rule, the table's edges, and the
//! single-path check on the contention verdict.
//!
//! Round 3 found nine holes in a hand-written denylist, round 4 found ~370 more, and that triggered
//! the flip to `Default_Ignorable_Code_Point`. This is the audit of the property itself. The
//! question is no longer "is there another hole" but "does this converge" — a rule derived from a
//! Unicode property either matches the property or it does not, and that is checkable rather than
//! guessable.
//!
//! It does match. `DEFAULT_IGNORABLE` is verified here range-for-range against an independent
//! derivation, and the three classes the property deliberately omits are complete. The remaining
//! tests are about the table's EDGES, which is where the one real bug of the round lived.

#![cfg(test)]

use crate::services::workflows::*;
use crate::state::app_state::AppState;
use crate::test_support::test_state;

const KEY: (&str, &str, &str) = ("repo", "dev-phase", "glm");

fn repo_path(name: &str) -> String {
    format!("/nonexistent-arena-qa-r5/{}", name)
}

fn version(repo: &str, phase_type: &str, family: &str) -> NewWorkflowVersion {
    NewWorkflowVersion {
        workspace_path: repo_path(repo),
        phase_type: phase_type.to_string(),
        family: family.to_string(),
        parent_id: None,
        role: "role".to_string(),
        instruction: "instruction".to_string(),
        contract: "contract".to_string(),
        hop: r#"{"steps":["edit"]}"#.to_string(),
        fan_out: 2,
    }
}

fn workflow(state: &AppState, repo: &str, phase_type: &str, family: &str) -> Workflow {
    create_workflow_version(state, version(repo, phase_type, family)).expect("create")
}

fn run_under(state: &AppState, workflow_id: &str, seats: &str, fleet: &str) -> WorkflowRun {
    start_workflow_run_with_budget(
        state,
        NewWorkflowRun {
            workflow_id: workflow_id.to_string(),
            plan_id: None,
            seed: 0,
            seats: None,
        },
        RunBudget {
            seats_held: seats.to_string(),
            fleet_busy: fleet.to_string(),
        },
    )
    .expect("start run")
}

/// Whether a reason consisting only of `code` is refused as invisible. Driven through the PUBLIC
/// door (`reject_workflow_run`) rather than the private predicate, because that is the surface a
/// client reaches and the one the guard exists to protect.
fn reason_is_refused(state: &AppState, run_id: &str, code: u32) -> bool {
    let character = char::from_u32(code).expect("a scalar value");
    reject_workflow_run(state, run_id, &character.to_string()).is_err()
}

// =================================================================================================
// THE FOURTH AUDIT — the property itself, its edges, and the classes it omits
// =================================================================================================

/// **The table IS `Default_Ignorable_Code_Point`, verified range-for-range against an independent
/// derivation.**
///
/// Not sampled — enumerated. The expected set below was derived separately from Python's UCD
/// 16.0.0 as `(Cf ∪ Variation_Selector ∪ Other_Default_Ignorable) − White_Space −
/// Prepended_Concatenation_Mark − U+FFF9..FFFB`, which is the Unicode derivation, and it produced
/// these 18 ranges and no others. Every boundary is checked from both sides, so an off-by-one at
/// either end of any range fails here.
///
/// This is what makes the answer to "is there a fifth class" checkable instead of a matter of how
/// hard someone looked: the rule now either agrees with a published property or it does not.
#[test]
fn qa_a5_the_table_matches_the_unicode_property_range_for_range() {
    let (state, _root) = test_state();
    let w = workflow(&state, KEY.0, KEY.1, KEY.2);
    let run = run_under(&state, &w.id, r#"{"glm":2}"#, "{}");

    // Independently derived; see the doc comment. Order is irrelevant to this test.
    const EXPECTED: &[(u32, u32)] = &[
        (0x00AD, 0x00AD),
        (0x034F, 0x034F),
        (0x061C, 0x061C),
        (0x115F, 0x1160),
        (0x17B4, 0x17B5),
        (0x180B, 0x180F),
        (0x200B, 0x200F),
        (0x202A, 0x202E),
        (0x2060, 0x206F),
        (0x3164, 0x3164),
        (0xFE00, 0xFE0F),
        (0xFEFF, 0xFEFF),
        (0xFFA0, 0xFFA0),
        (0xFFF0, 0xFFF8),
        (0x13430, 0x1343F),
        (0x1BCA0, 0x1BCA3),
        (0x1D173, 0x1D17A),
        (0xE0000, 0xE0FFF),
    ];
    // The three groups the property deliberately omits and the rule adds back, each for a reason
    // written on the constant: a graphic that renders as nothing, the interlinear controls, and
    // `Prepended_Concatenation_Mark=Yes` — which is COMPLETE here, all eleven code points.
    const EXPECTED_ADDITIONS: &[(u32, u32)] = &[
        (0x0600, 0x0605),
        (0x06DD, 0x06DD),
        (0x070F, 0x070F),
        (0x0890, 0x0891),
        (0x08E2, 0x08E2),
        (0x2800, 0x2800),
        (0xFFF9, 0xFFFB),
        (0x110BD, 0x110BD),
        (0x110CD, 0x110CD),
    ];

    // Every code point in every expected range is refused: first, last, and the interior sampled
    // (bounded, because E0000..E0FFF alone is 4096).
    let mut checked = 0_usize;
    for (start, end) in EXPECTED.iter().chain(EXPECTED_ADDITIONS) {
        let mut probes = vec![*start, *end, start + (end - start) / 2];
        probes.dedup();
        for code in probes {
            assert!(
                reason_is_refused(&state, &run.id, code),
                "U+{code:04X} is in the Unicode property (range U+{start:04X}..U+{end:04X}) and \
                 must be refused as invisible"
            );
            checked += 1;
        }
    }
    assert!(checked >= 54, "the audit must actually probe: only {checked}");

    // And every boundary from OUTSIDE. `start - 1` and `end + 1` must be accepted unless they
    // belong to another expected range — which is what catches an off-by-one or a range that
    // silently swallows its neighbour.
    let covered: Vec<(u32, u32)> = EXPECTED
        .iter()
        .chain(EXPECTED_ADDITIONS)
        .copied()
        .collect();
    let inside = |code: u32| covered.iter().any(|(s, e)| code >= *s && code <= *e);
    for (start, end) in &covered {
        for neighbour in [start.checked_sub(1), Some(end + 1)].into_iter().flatten() {
            if inside(neighbour) {
                continue;
            }
            let Some(character) = char::from_u32(neighbour) else { continue };
            // Whitespace and control characters are refused by a DIFFERENT clause, which is
            // correct and not what this check is about.
            if character.is_whitespace() || character.is_control() {
                continue;
            }
            assert!(
                !reason_is_refused(&state, &run.id, neighbour),
                "U+{neighbour:04X} sits just outside U+{start:04X}..U+{end:04X} and is not in any \
                 expected range, so it must be ACCEPTED — a range that swallows its neighbour is \
                 how a visible character starts being refused"
            );
        }
    }
}

/// **Cross-table disjointness — the one thing the author's own sortedness test does not cover.**
///
/// Theirs checks inversion, sortedness and disjointness WITHIN each table, and that every boundary
/// is found by the binary search. It does not check that the two tables are disjoint FROM EACH
/// OTHER. That matters for the same reason sortedness does: an entry claimed by both is dead in one
/// of them, and a reader cannot tell which — which is precisely the state the out-of-order append
/// left three code points in this round.
///
/// Checked as arithmetic over the two independently-derived sets, not by probing: an overlap is a
/// property of the ranges, and 11,000 database inserts cannot prove it better than subtraction can.
///
/// **A note on a test of mine that did not earn its place.** The first version of this scanned
/// U+0000..U+3200 and U+FE00..U+10200 through `reject_workflow_run` to derive the refused set from
/// BEHAVIOUR rather than from the table. It took 194 seconds — four times the entire rest of the
/// suite — and on reading it back, almost everything it asserted afterwards was self-consistency of
/// its own output: that its derived runs were sorted (they were, by construction of the scan) and
/// non-empty (likewise). The content was the overlap check, which is free. The edge probing it was
/// also meant to provide is already done exactly, from both sides of every boundary, by
/// `qa_a5_the_table_matches_the_unicode_property_range_for_range` in under half a second.
#[test]
fn qa_a5_the_two_tables_are_disjoint_from_each_other() {
    // The same two sets as the range-for-range audit, independently derived from UCD 16.0.0.
    const PROPERTY: &[(u32, u32)] = &[
        (0x00AD, 0x00AD),
        (0x034F, 0x034F),
        (0x061C, 0x061C),
        (0x115F, 0x1160),
        (0x17B4, 0x17B5),
        (0x180B, 0x180F),
        (0x200B, 0x200F),
        (0x202A, 0x202E),
        (0x2060, 0x206F),
        (0x3164, 0x3164),
        (0xFE00, 0xFE0F),
        (0xFEFF, 0xFEFF),
        (0xFFA0, 0xFFA0),
        (0xFFF0, 0xFFF8),
        (0x13430, 0x1343F),
        (0x1BCA0, 0x1BCA3),
        (0x1D173, 0x1D17A),
        (0xE0000, 0xE0FFF),
    ];
    const ADDITIONS: &[(u32, u32)] = &[
        (0x0600, 0x0605),
        (0x06DD, 0x06DD),
        (0x070F, 0x070F),
        (0x0890, 0x0891),
        (0x08E2, 0x08E2),
        (0x2800, 0x2800),
        (0xFFF9, 0xFFFB),
        (0x110BD, 0x110BD),
        (0x110CD, 0x110CD),
    ];

    // No range in either table overlaps any range in the other.
    let mut overlaps = Vec::new();
    for (ps, pe) in PROPERTY {
        for (as_, ae) in ADDITIONS {
            if ps <= ae && as_ <= pe {
                overlaps.push(format!(
                    "U+{ps:04X}..U+{pe:04X} (property) overlaps U+{as_:04X}..U+{ae:04X} (addition)"
                ));
            }
        }
    }
    assert!(
        overlaps.is_empty(),
        "a code point claimed by both tables is dead in one of them and a reader cannot tell \
         which: {overlaps:?}"
    );

    // And each table is internally sorted, disjoint and non-inverted — restated here against the
    // derived sets rather than the crate constants, so a table edited to match a WRONG expectation
    // fails the range-for-range audit instead of quietly agreeing with itself.
    for (label, table) in [("property", PROPERTY), ("additions", ADDITIONS)] {
        for (start, end) in table {
            assert!(start <= end, "{label}: U+{start:04X}..U+{end:04X} is inverted");
        }
        for pair in table.windows(2) {
            assert!(
                pair[0].1 < pair[1].0,
                "{label}: U+{:04X}..U+{:04X} then U+{:04X}..U+{:04X} — unsorted or overlapping, \
                 which makes a binary search miss entries",
                pair[0].0,
                pair[0].1,
                pair[1].0,
                pair[1].1
            );
        }
    }

    // Non-vacuous: the overlap detector fires on an overlap, and the sortedness one on a swap.
    let a = [(0x10_u32, 0x20_u32)];
    let b = [(0x18_u32, 0x30_u32)];
    assert!(
        a.iter().any(|(s, e)| b.iter().any(|(s2, e2)| s <= e2 && s2 <= e)),
        "the overlap predicate must detect an overlap"
    );
    let unsorted = [(0x30_u32, 0x40_u32), (0x10, 0x20)];
    assert!(
        unsorted.windows(2).any(|p| p[0].1 >= p[1].0),
        "the sortedness predicate must detect a swap"
    );
}

/// **The hunt for a fifth class, and why I am reporting that there isn't one.**
///
/// Four classes are now covered: `Default_Ignorable_Code_Point`, `char::is_whitespace`,
/// `char::is_control`, and the three named additions. The remaining candidates for "renders as
/// nothing" are enumerated here and each is asserted ACCEPTED, with the reason — so that a future
/// over-broad fix that starts refusing legitimate text turns this red rather than passing quietly.
///
/// The closest call is the lone nonspacing mark. `U+0301` on its own renders as a dotted circle
/// plus an acute in every conforming renderer, and as a bare accent in the ones that are not: in
/// both cases something appears, so accepting it is correct. Refusing it would be the first time
/// this guard rejected text a human could read, which is the direction the function's own comment
/// says it must never take.
#[test]
fn qa_a5_the_remaining_render_as_nothing_candidates_are_correctly_accepted() {
    let (state, _root) = test_state();
    let w = workflow(&state, KEY.0, KEY.1, KEY.2);
    let run = run_under(&state, &w.id, r#"{"glm":2}"#, "{}");

    let candidates: &[(&str, u32, &str)] = &[
        // Nonspacing marks alone: a dotted circle is drawn, so something is visible.
        ("combining acute", 0x0301, "renders as a dotted circle plus the accent"),
        ("hebrew point", 0x05B0, "same"),
        ("arabic fatha", 0x064E, "same"),
        ("thai sara i", 0x0E34, "same"),
        ("devanagari anusvara", 0x0901, "same"),
        // Egyptian MODIFIERS, added after the format controls and deliberately outside them.
        ("egyptian mirror vertical", 0x13440, "Mn, not a format control"),
        ("egyptian top start", 0x13447, "Mn"),
        // Enclosing marks draw a visible enclosure.
        ("enclosing circle", 0x20DD, "draws a circle"),
        // Private use: whatever the font says, and .notdef when it says nothing.
        ("private use start", 0xE000, "font-dependent, .notdef box when unmapped"),
        ("private use end", 0xF8FF, "same"),
        ("plane 15 private use", 0xF0000, "same"),
        // Noncharacters and unassigned: .notdef box, which is visible.
        ("noncharacter FFFE", 0xFFFE, ".notdef"),
        ("unassigned 0378", 0x0378, ".notdef"),
        // Braille with dots is visible; only the BLANK is not, and that one is listed.
        ("braille with one dot", 0x2801, "has a raised dot"),
        // Modifier symbols and spacing modifiers are visible glyphs.
        ("modifier letter small h", 0x02B0, "a visible glyph"),
        ("skin tone modifier", 0x1F3FB, "a colour swatch"),
        // Hangul jamo that are not fillers.
        ("hangul choseong kiyeok", 0x1100, "a visible jamo"),
    ];

    let mut wrongly_refused = Vec::new();
    for (label, code, _why) in candidates {
        if reason_is_refused(&state, &run.id, *code) {
            wrongly_refused.push(format!("{label} (U+{code:04X})"));
        }
    }
    assert!(
        wrongly_refused.is_empty(),
        "these render as SOMETHING and must be accepted — refusing visible text is the one \
         direction this guard must never take: {wrongly_refused:?}"
    );

    // Non-vacuous: the guard is live on this run and still refuses what it should, including one
    // representative from each of the four covered classes.
    for (label, code) in [
        ("default-ignorable (tag char)", 0xE0020_u32),
        ("named addition (braille blank)", 0x2800),
        ("named addition (kaithi)", 0x110CD),
        ("named addition (interlinear)", 0xFFF9),
    ] {
        assert!(
            reason_is_refused(&state, &run.id, code),
            "{label} U+{code:04X} must still be refused"
        );
    }
    // And a mixture of a covered invisible with a visible candidate is ACCEPTED, because the rule
    // is "contains anything visible" and not "contains only visible things".
    reject_workflow_run(&state, &run.id, "\u{e0020}\u{0301}x\u{2800}")
        .expect("visible content among invisibles is accepted");
}

// =================================================================================================
// ITEM 3 — is there a third path to a contention verdict?
// =================================================================================================

/// **Every path to a contention verdict agrees, because there is only one.**
///
/// The bug this closes was two implementations of one rule: the signature clause lived in
/// `run_is_scoreable` while `WorkflowRun.not_scoreable_reason` — the field a client reads — was
/// computed from `fleet_busy` alone, so the clause was absent on the path that mattered.
///
/// I traced every producer. `scoreability` has exactly one caller (`contention_block`);
/// `contention_block` has two (`run_is_scoreable` and `with_scoreability`); `with_scoreability` is
/// called from `map_run` and nowhere else, as the final `.map()` of the only row mapper, so it
/// cannot be skipped by a query; `preference_comparability` and `promote_workflow` both go through
/// `run_is_scoreable`. There is no third path. This asserts the agreement end to end rather than
/// relying on that trace staying true.
#[test]
fn qa_a5_every_path_to_a_contention_verdict_gives_the_same_answer() {
    let (state, _root) = test_state();
    let w = workflow(&state, KEY.0, KEY.1, KEY.2);
    let seats = r#"{"glm":2}"#;

    for fleet in ["{}", r#"{"glm":0}"#, r#"{"glm":2}"#, r#"{"qwen":5}"#] {
        let fresh = run_under(&state, &w.id, seats, fleet);
        // 1. the struct returned by the WRITE path
        let from_write = (fresh.scoreable, fresh.not_scoreable_reason.clone());
        // 2. the struct re-read by the single-row READ path
        let reread = get_workflow_run(&state, &fresh.id).expect("read");
        // 3. the struct from the LIST path
        let listed = list_workflow_runs(&state, &w.id)
            .expect("list")
            .into_iter()
            .find(|candidate| candidate.id == fresh.id)
            .expect("listed");
        // 4. the public predicate
        let predicate = run_is_scoreable(&reread);

        assert_eq!(
            from_write,
            (reread.scoreable, reread.not_scoreable_reason.clone()),
            "write and read disagree for fleet {fleet}"
        );
        assert_eq!(
            from_write,
            (listed.scoreable, listed.not_scoreable_reason.clone()),
            "write and list disagree for fleet {fleet}"
        );
        assert_eq!(
            reread.scoreable,
            predicate.is_none(),
            "the derived field and the public predicate disagree for fleet {fleet}"
        );
        assert_eq!(
            reread.not_scoreable_reason, predicate,
            "the derived REASON and the predicate's reason disagree for fleet {fleet}"
        );

        // 5. and the comparability policy, which must refuse whenever the run is unscoreable.
        let partner = run_under(&state, &w.id, seats, "{}");
        let partner = record_workflow_run_gate(&state, &partner.id, 0, None).expect("gate");
        let scored = record_workflow_run_gate(&state, &fresh.id, 0, None).expect("gate");
        let pref = record_workflow_pref(&state, &scored.id, &partner.id, None).expect("pref");
        assert_eq!(
            pref.comparable,
            scored.scoreable,
            "a run that is not scoreable must not produce a comparable preference ({fleet})"
        );
    }

    // The signature clause really is present on the field a client reads — the exact thing that was
    // missing. Asserted on the DERIVED field, not the predicate.
    let signature = run_under(&state, &w.id, seats, seats);
    let reason = get_workflow_run(&state, &signature.id)
        .expect("read")
        .not_scoreable_reason
        .expect("unscoreable");
    assert!(
        reason.contains("inclusive-totals"),
        "the signature clause must be on the derived field: {reason}"
    );
    assert!(
        reason.contains("seatsHeld"),
        "and must name the run's own budget: {reason}"
    );
    // While real foreign load gets the plainer reason, so the two are distinguishable.
    let real = run_under(&state, &w.id, seats, r#"{"qwen":5}"#);
    let real_reason = get_workflow_run(&state, &real.id)
        .expect("read")
        .not_scoreable_reason
        .expect("unscoreable");
    assert!(
        !real_reason.contains("inclusive-totals"),
        "real contention must NOT be labelled a convention mismatch: {real_reason}"
    );
}

/// The two derived fields are PUBLIC and plain, so an inconsistent pair is representable even
/// though nothing in production constructs one: `scoreable: false` with `not_scoreable_reason:
/// None` is "cannot compete, no explanation", which is the exact state the reason field was added
/// to prevent.
///
/// `map_run` is the only production construction site (the other two matches in the file are the
/// struct declarations), so this is a shape note rather than a defect — but it is the shape the
/// two-paths bug took, and making it unrepresentable is cheap: compute the pair behind an accessor,
/// or `#[serde(skip_deserializing)]` so a relayed row cannot carry a hand-made verdict.
#[test]
fn qa_a5_an_inconsistent_scoreability_pair_is_still_representable() {
    let inconsistent = WorkflowRun {
        id: "r".to_string(),
        workflow_id: "w".to_string(),
        plan_id: None,
        seed: 0,
        gate_exit: Some(0),
        gate_collected: None,
        seats_held: r#"{"glm":2}"#.to_string(),
        fleet_busy: r#"{"qwen":9}"#.to_string(),
        started_at: "now".to_string(),
        ended_at: None,
        // Contradicts `fleet_busy`: a busy fleet, claimed scoreable, with no reason.
        scoreable: true,
        not_scoreable_reason: None,
    };
    // The RULE still refuses it, which is what keeps this harmless today: every decision recomputes
    // from `fleet_busy` rather than trusting the field.
    assert!(
        run_is_scoreable(&inconsistent).is_some(),
        "the rule recomputes and does not trust the stored verdict"
    );
    // But the struct itself is happily inconsistent, and that is what a client would be served.
    assert!(
        inconsistent.scoreable && inconsistent.not_scoreable_reason.is_none(),
        "DEFECT PIN (shape, not a live bug): the pair is representable. If this stops compiling or \
         stops being constructible, the fields were made derived-only and this test should go."
    );
    let json = serde_json::to_value(&inconsistent).expect("serialise");
    assert_eq!(json["scoreable"].as_bool(), Some(true));
    assert!(json["notScoreableReason"].is_null());
}

// =================================================================================================
// ITEM 6 — do the two citation gates close it? Not quite, and here is the third.
// =================================================================================================

/// **A citation can still be written in a form neither existing gate sees, and it is the form the
/// original false citation actually took.**
///
/// The two gates close the BRACKETED form completely. `cargo doc` with
/// `-D rustdoc::broken_intra_doc_links` I verified non-vacuous and crate-wide (a planted
/// `[`no_such_function_at_all`]` in the arena and another in `sessions.rs` both exit 101, baseline
/// exits 0), and the round-4 sweep covers the same form over the arena files.
///
/// Neither sees a name cited in plain BACKTICKS. That is not a hypothetical: the citation that was
/// false for three rounds appeared twice, once bracketed and once as prose, and the prose one
/// survived the round-4 fix because my own sweep was deliberately scoped to brackets.
///
/// Worse, the doc gate pushes citations INTO that form. A test function is `#[cfg(test)]` and so
/// absent from a normal doc build, which is why their first run of the gate found a bracketed link
/// to a test and had to unbracket it. So for exactly the thing that went wrong twice — citing a
/// TEST by name before writing it — the gated form is unavailable and the ungated one is the only
/// option.
///
/// A general prose sweep is not the answer: measured over these five files, 27 names are cited in
/// prose with no matching `fn`, and 26 are legitimate — struct fields, SQL tables, the partial
/// index, std, tokio, rusqlite, and functions in sibling modules. The allowlist would be as long as
/// the thing it checks.
///
/// This rule is narrow instead, and keys on the shape a TEST name has: four or more underscores.
/// Field names in this crate have at most two (`not_scoreable_reason`, `workflows_one_champion`);
/// test names have six to ten. Measured: 8 such citations at this tip, **zero** false positives —
/// and run against the tree as it stood before the contract test was written, it catches
/// `the_fleet_busy_contract_is_foreign_contention_only` on the prose line that both other gates
/// miss.
#[test]
fn qa_a5_no_doc_comment_cites_a_test_shaped_name_that_does_not_exist() {
    let sources: [(&str, &str); 6] = [
        ("services/workflows.rs", include_str!("workflows.rs")),
        (
            "services/workflows_qa_attacks.rs",
            include_str!("workflows_qa_attacks.rs"),
        ),
        (
            "services/workflows_qa_attacks_r2.rs",
            include_str!("workflows_qa_attacks_r2.rs"),
        ),
        (
            "services/workflows_qa_attacks_r3.rs",
            include_str!("workflows_qa_attacks_r3.rs"),
        ),
        (
            "services/workflows_qa_attacks_r4.rs",
            include_str!("workflows_qa_attacks_r4.rs"),
        ),
        (
            "services/workflows_qa_attacks_r5.rs",
            include_str!("workflows_qa_attacks_r5.rs"),
        ),
    ];
    let corpus: String = sources.iter().map(|(_, text)| *text).collect();

    // A test-shaped name: snake_case with four or more underscores. The threshold is measured, not
    // chosen: the widest FIELD name in this crate has two.
    const MIN_UNDERSCORES: usize = 4;

    let mut dangling: Vec<String> = Vec::new();
    let mut examined = 0_usize;
    for (file, text) in sources {
        for (index, line) in text.lines().enumerate() {
            let trimmed = line.trim_start();
            if !(trimmed.starts_with("///") || trimmed.starts_with("//!")) {
                continue;
            }
            // Every backticked run on the line, bracketed or not.
            for chunk in trimmed.split('`').skip(1).step_by(2) {
                let name = chunk.trim_end_matches(|c: char| !c.is_ascii_alphanumeric() && c != '_');
                if name.len() < 8
                    || name.matches('_').count() < MIN_UNDERSCORES
                    || !name
                        .chars()
                        .all(|c| c.is_ascii_lowercase() || c == '_' || c.is_ascii_digit())
                {
                    continue;
                }
                examined += 1;
                // Assembled, never written as a literal: this file is in the corpus it searches,
                // and a literal here would make the cited name count as defined. That mistake has
                // been made twice in this suite, the second time inside a comment explaining the
                // first.
                if !corpus.contains(&format!("fn {}", name)) {
                    dangling.push(format!("{file}:{} cites `{name}`", index + 1));
                }
            }
        }
    }

    assert!(
        examined >= 8,
        "the sweep must actually find test-shaped citations: only {examined}"
    );
    assert_eq!(
        dangling,
        Vec::<String>::new(),
        "a doc comment cites a test-shaped name that nothing defines. This is the gap the other \
         two gates leave: `cargo doc` cannot resolve a #[cfg(test)] item so citations to tests must \
         be unbracketed, and an unbracketed citation is invisible to the bracket sweep"
    );

    // Non-vacuous, three ways: the corpus resolves a name that exists, the threshold really does
    // exclude the widest field name, and a synthetic test-shaped name is NOT found.
    assert!(
        corpus.contains(&format!("fn {}", "the_fleet_busy_contract_is_foreign_contention_only")),
        "the test this suite spent a round on must now exist"
    );
    assert_eq!("not_scoreable_reason".matches('_').count(), 2);
    assert!("a_name_shaped_exactly_like_a_test_function".matches('_').count() >= MIN_UNDERSCORES);
    assert!(!corpus.contains(&format!("fn {}", "a_name_shaped_exactly_like_a_test_function")));
}
