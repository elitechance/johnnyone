//! The budget was guessed before anyone knew the conventions were a directory.
//! Measure the real material and size it to fit.
//!
//! THE MEASUREMENT, taken on 2026-10-24 against the live kart plan
//! (`e37cd849-…`), the real methodology and the real conventions store, by
//! rendering them through `expand_lens_docs` + `inline_material`:
//!
//! ```text
//! documents: 12        total body bytes: 111881        budget: 80000
//!   BRIEF         5467   .../plan/brief.md
//!   PLAN          6351   .../plan/overview.md
//!   METHODOLOGY  33093   lokal/agents/common/methodology.md
//!   CONVENTIONS   4354   conventions/README.md
//!   CONVENTIONS   6103   conventions/backend.md
//!   CONVENTIONS   7138   conventions/marketplace-deploy.md
//!   CONVENTIONS   7758   conventions/plan-validation.md
//!   CONVENTIONS  10860   conventions/review-lenses.md
//!   CONVENTIONS   4311   conventions/screenshot.mjs
//!   CONVENTIONS   4094   conventions/screenshots.md
//!   CONVENTIONS   7768   conventions/tooling.md
//!   CONVENTIONS  14584   conventions/ui.md
//! rendered: 79998 bytes
//! ```
//!
//! And here is what the reviewer actually receives:
//!
//! ```text
//!   === CONVENTIONS · conventions/review-lenses.md · PARTIAL: first 6834 of 10801 characters shown ===
//!   === CONVENTIONS · conventions/screenshot.mjs ===     (stub: whole body omitted)
//!   === CONVENTIONS · conventions/screenshots.md ===     (stub)
//!   === CONVENTIONS · conventions/tooling.md ===         (stub)
//!   === CONVENTIONS · conventions/ui.md ===              (stub)
//! ```
//!
//! So fixing the directory read bought the reviewer the first four conventions
//! files and then ran out of budget **inside `review-lenses.md`** — the one file
//! three planner prompts name as where the three review lenses are defined — and
//! `ui.md`, 14,584 bytes of the UI conventions a *visual identity* plan is
//! judged against, arrives as a header and a notice. 37% of the lens checklist
//! and 100% of the UI conventions, cut.
//!
//! WHY 80,000 WAS WRONG. It was set on 2026-10-24 alongside the directory bug,
//! when CONVENTIONS was 88 characters of `Is a directory (os error 21)` — so the
//! material it was sized against was a third of the material that exists. The
//! lens runs on `glm-5.3-flash`, configured at `maxContextTokens: 131072`
//! (`~/etc/kloo.json`). 111,881 characters is roughly 30–35k tokens of prose:
//! about a quarter of that window. The budget is not protecting anything here;
//! it is cutting the checklist to fit a limit nothing requires.
//!
//! WHAT THIS SPEC DOES NOT DO. It does not reorder the conventions.
//! `stream_lens_conventions.rs` (c1, c3) pins lexicographic order so two runs of
//! the same plan produce byte-identical leading prompt — the provider's prefix
//! cache depends on it. Hoisting `review-lenses.md` would fix the symptom at the
//! cost of that, and once nothing truncates, order stops deciding what the
//! reviewer sees. `b2` keeps the truncation rail alive for a store that really
//! is too big.

use johnnyone_desktop_lib::services::stream_lens::{
    inline_material, LensDoc, LENS_MATERIAL_BUDGET_BYTES,
};

/// Bodies at the real measured sizes, in the real order.
fn live_shaped_material() -> Vec<LensDoc> {
    let doc = |label: &str, name: &str, n: usize| LensDoc {
        label: label.to_string(),
        path: format!("/store/{name}"),
        // Distinct filler per document so a truncation cannot be mistaken for a
        // neighbour's text, and so each body is findable in the output.
        body: format!("<<{name}>>\n")
            + &"convention line that says something specific.\n".repeat(n / 46),
    };
    vec![
        doc("BRIEF", "brief.md", 5467),
        doc("PLAN", "overview.md", 6351),
        doc("METHODOLOGY", "methodology.md", 33093),
        doc("CONVENTIONS", "README.md", 4354),
        doc("CONVENTIONS", "backend.md", 6103),
        doc("CONVENTIONS", "marketplace-deploy.md", 7138),
        doc("CONVENTIONS", "plan-validation.md", 7758),
        doc("CONVENTIONS", "review-lenses.md", 10860),
        doc("CONVENTIONS", "screenshot.mjs", 4311),
        doc("CONVENTIONS", "screenshots.md", 4094),
        doc("CONVENTIONS", "tooling.md", 7768),
        doc("CONVENTIONS", "ui.md", 14584),
    ]
}

/// b1 — the material the host really assembles must arrive WHOLE.
///
/// Not "mostly": no `PARTIAL` heading and no stub anywhere. A reviewer asked to
/// judge a plan against `review-lenses.md` must have all of `review-lenses.md`.
#[test]
fn b1_the_real_material_is_not_truncated() {
    let docs = live_shaped_material();
    let total: usize = docs.iter().map(|d| d.body.len()).sum();
    let out = inline_material(&docs, LENS_MATERIAL_BUDGET_BYTES);

    assert!(
        !out.contains("PARTIAL"),
        "the live-shaped material ({total} bytes) still truncates a document at a \
         budget of {LENS_MATERIAL_BUDGET_BYTES}. On the real store the casualty is \
         review-lenses.md, the file the planner prompts name as the definition of \
         the three review lenses."
    );
    for name in [
        "README.md",
        "backend.md",
        "marketplace-deploy.md",
        "plan-validation.md",
        "review-lenses.md",
        "screenshot.mjs",
        "screenshots.md",
        "tooling.md",
        "ui.md",
    ] {
        assert!(
            out.contains(&format!("<<{name}>>")),
            "{name} reached the reviewer as a stub — header and an omission notice, \
             no body. ui.md is 14,584 bytes of the UI conventions a visual-identity \
             plan is judged against."
        );
    }
    assert!(
        !out.contains("omitted"),
        "something was still omitted from material that now fits"
    );
}

/// b2 — THE CONTROL ARM. The rail must survive the raise.
///
/// The cheapest way to make b1 green is `usize::MAX`, and then a conventions
/// store nobody pruned becomes the whole prompt with no notice at all. A store
/// genuinely larger than the budget must still truncate, must still NAME the
/// cut, and every later document must still appear rather than silently
/// vanishing — the three properties `inline_material` already has, which b1 must
/// not be allowed to buy its way out of.
#[test]
fn b2_a_genuinely_oversized_store_still_truncates_and_says_so() {
    let mut docs = live_shaped_material();
    // Three times the budget, whatever the budget now is.
    let huge = LENS_MATERIAL_BUDGET_BYTES * 3;
    docs.push(LensDoc {
        label: "CONVENTIONS".to_string(),
        path: "/store/enormous.md".to_string(),
        body: "x".repeat(huge),
    });
    docs.push(LensDoc {
        label: "CONVENTIONS".to_string(),
        path: "/store/zz-last.md".to_string(),
        body: "<<zz-last.md>>\nthe last convention.\n".to_string(),
    });

    let out = inline_material(&docs, LENS_MATERIAL_BUDGET_BYTES);
    assert!(
        out.len() <= LENS_MATERIAL_BUDGET_BYTES,
        "the budget was not enforced at all: {} bytes rendered against {}",
        out.len(),
        LENS_MATERIAL_BUDGET_BYTES
    );
    assert!(
        out.contains("omitted"),
        "a store three times the budget was cut and the prompt never said so. A \
         reviewer judging a silently partial checklist reports a PASS it has not \
         earned."
    );
    assert!(
        out.contains("BLOCKED"),
        "the omission notice no longer tells the reviewer to return BLOCKED \
         rather than report the missing part as absent"
    );
    assert!(
        out.contains("/store/zz-last.md"),
        "the document after the oversized one disappeared from the prompt \
         entirely. Every later document must still get a section — the silent \
         skip inline_material exists to ban."
    );
    // And the brief is never the casualty: budget is spent in order on purpose.
    assert!(
        out.contains("<<brief.md>>") && out.contains("<<overview.md>>"),
        "the user's own brief or the plan itself was truncated to make room for \
         a conventions store, which inverts the whole point of the ordering"
    );
}

/// b3 — the budget is a number someone chose, and the choice is recorded.
///
/// This is the one assertion here that is about the constant rather than the
/// behaviour, and it is deliberately a FLOOR with the measurement in the
/// message: 111,881 bytes of real material plus ~1,500 bytes of section headers
/// and trailers. A later edit that quietly halves the constant fails here with
/// the reason it must not.
#[test]
fn b3_the_budget_covers_the_measured_material_with_headroom() {
    const MEASURED_BODY_BYTES: usize = 111_881;
    assert!(
        LENS_MATERIAL_BUDGET_BYTES >= MEASURED_BODY_BYTES + 8_000,
        "the budget is {LENS_MATERIAL_BUDGET_BYTES}, under the {MEASURED_BODY_BYTES} \
         bytes of material the host assembled for the kart plan on 2026-10-24 plus \
         room for 12 section headers and a plan that grows. Measured, not guessed: \
         brief 5467, plan 6351, methodology 33093, nine conventions files 66970."
    );
    // glm-5.3-flash is configured at maxContextTokens 131072. At ~3.5 characters
    // per token this budget must stay well inside that, or the fix for a cut
    // checklist becomes a provider error.
    assert!(
        LENS_MATERIAL_BUDGET_BYTES <= 300_000,
        "a budget of {LENS_MATERIAL_BUDGET_BYTES} characters is ~{}k tokens, which \
         will not fit the 131,072-token window the lens model is configured with",
        LENS_MATERIAL_BUDGET_BYTES / 3500
    );
}
