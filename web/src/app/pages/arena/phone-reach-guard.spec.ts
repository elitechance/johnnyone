// The phone-reach guard. This exists because of a MEASURED precedent in this codebase: the Plan
// tab's document navigator was `display: none` below 720px, so on a phone no plan document was
// reachable at all — the console is read on a phone, and that one rule made a whole surface
// unreachable without a single test failing.
//
// jsdom applies no stylesheets and its `matchMedia` matches nothing, so a mounted component can
// never prove CSS reachability. This guard is therefore a STATIC analysis of the stylesheet text,
// and it is run against the arena components' real styles in `arena-phone.spec.ts`.
import { describe, it, expect } from 'vitest';
import { phoneHiddenSelectors, overflowRisks, clipRisks, mediaQueryMatches } from './phone-reach-guard';

describe('mediaQueryMatches', () => {
  it('matches a max-width query at or below the width', () => {
    expect(mediaQueryMatches('(max-width: 720px)', 400)).toBe(true);
    expect(mediaQueryMatches('(max-width: 400px)', 400)).toBe(true);
    expect(mediaQueryMatches('(max-width: 380px)', 400)).toBe(false);
  });
  it('matches a min-width query only at or above the width', () => {
    expect(mediaQueryMatches('(min-width: 720px)', 400)).toBe(false);
    expect(mediaQueryMatches('(min-width: 320px)', 400)).toBe(true);
  });
  it('ands within a query and ors across a comma', () => {
    expect(mediaQueryMatches('(min-width: 320px) and (max-width: 500px)', 400)).toBe(true);
    expect(mediaQueryMatches('(min-width: 600px) and (max-width: 900px)', 400)).toBe(false);
    expect(mediaQueryMatches('(min-width: 900px), (max-width: 500px)', 400)).toBe(true);
  });
  it('ignores non-width queries rather than guessing', () => {
    expect(mediaQueryMatches('(prefers-color-scheme: dark)', 400)).toBe(false);
  });
});

describe('phoneHiddenSelectors — the regression that actually happened', () => {
  it('catches the plan-navigator bug: a surface display:none below 720px', () => {
    const css = `
      .docnav { display: grid; }
      @media (max-width: 720px) {
        .docnav { display: none; }
      }
    `;
    const found = phoneHiddenSelectors(css, 400);
    expect(found).toHaveLength(1);
    expect(found[0].selector).toBe('.docnav');
    expect(found[0].query).toContain('max-width: 720px');
  });

  it('catches visibility:hidden too, and a multi-selector rule', () => {
    const css = `@media (max-width: 500px) { .a, .b { visibility: hidden; } }`;
    expect(phoneHiddenSelectors(css, 400).map((v) => v.selector)).toEqual(['.a, .b']);
  });

  it('does NOT flag hiding at a width the phone never sees', () => {
    const css = `@media (min-width: 900px) { .wide-only { display: none; } }`;
    expect(phoneHiddenSelectors(css, 400)).toEqual([]);
  });

  it('does NOT flag a display:none outside any media query — that is a state, not a width', () => {
    // e.g. `.panel.collapsed { display: none }` is correct at every width.
    expect(phoneHiddenSelectors(`.panel.collapsed { display: none; }`, 400)).toEqual([]);
  });
});

describe('overflowRisks — nothing wider than the phone', () => {
  it('flags a fixed or minimum width wider than the viewport', () => {
    const risks = overflowRisks(`.slot { min-width: 640px; } .x { width: 520px; }`, 400);
    expect(risks).toHaveLength(2);
  });
  it('allows widths that fit, and percentage / viewport units', () => {
    expect(overflowRisks(`.a { width: 100%; } .b { min-width: 0; } .c { width: 320px; }`, 400))
      .toEqual([]);
  });
  it('allows a wide width inside a media query the phone never matches', () => {
    expect(overflowRisks(`@media (min-width: 900px) { .a { min-width: 640px; } }`, 400)).toEqual([]);
  });
});


// QA's three mutations, as unit cases. Each one was injected into the REAL ARENA_CSS and passed the
// first version of this guard 6/6, because the guard was reading 17 error-message strings and no
// stylesheet at all.
describe('the three mutations that used to pass', () => {
  it('catches a 600px minimum width', () => {
    expect(overflowRisks('.a-card { min-width: 600px; }', 400)).toHaveLength(1);
  });

  it('catches a viewport-relative width wider than the viewport', () => {
    expect(overflowRisks('.aw { width: 120vw; }', 400)).toHaveLength(1);
    expect(overflowRisks('.aw { width: 100vw; }', 400)).toEqual([]);
    expect(overflowRisks('.aw { max-width: 90vw; }', 400)).toEqual([]);
  });

  it('catches a fixed-height box that hides its own overflow', () => {
    const risks = clipRisks('.a-card { overflow: hidden; max-height: 80px; }');
    expect(risks).toHaveLength(1);
    expect(risks[0].selector).toBe('.a-card');
  });

  it('does not flag a scrollable fixed-height box, which is how `pre.a-out` works', () => {
    expect(clipRisks('pre.a-out { max-height: 220px; overflow: auto; }')).toEqual([]);
    expect(clipRisks('.x { overflow: hidden; }')).toEqual([]);
    expect(clipRisks('.y { max-height: 80px; }')).toEqual([]);
  });

  it('catches `height` as well as `max-height`, and `overflow-y`', () => {
    expect(clipRisks('.z { height: 80px; overflow-y: hidden; }')).toHaveLength(1);
  });
});
