// The arena at 400px, checked against the REAL stylesheets the components ship with.
//
// The precedent this exists for is measured and in the tree's history: the Plan tab's document
// navigator was `display: none` below 720px, so on a phone no plan document was reachable at all,
// and no test failed. jsdom applies no stylesheets and matches no media query, so a mounted
// component cannot catch that class of bug — a static read of the CSS can.
import { describe, it, expect } from 'vitest';
import { readFileSync, readdirSync } from 'node:fs';
import { join } from 'node:path';
import { phoneHiddenSelectors, overflowRisks } from './phone-reach-guard';
import { ARENA_CSS } from './arena-ui.css';

const PHONE = 400;
const HERE = join(process.cwd(), 'web/src/app/pages/arena');

/** Every inline `styles: [...]` string in this folder's components, read off disk. One of them is
 *  ARENA_CSS (shared); the rest are each component's own few rules. */
function componentStyleText(): { file: string; css: string }[] {
  const out: { file: string; css: string }[] = [];
  for (const file of readdirSync(HERE)) {
    if (!file.endsWith('.ts') || file.endsWith('.spec.ts')) continue;
    const src = readFileSync(join(HERE, file), 'utf8');
    // Backtick-quoted blocks inside a `styles: [...]` array, plus the shared sheet's own template
    // literal. Crude on purpose: it over-collects rather than under-collects.
    for (const m of src.matchAll(/`([^`]*\{[^`]*\})`/g)) out.push({ file, css: m[1] });
  }
  return out;
}

describe('the arena stylesheets at 400px', () => {
  it('collected real CSS to check — a guard that found no stylesheet proves nothing', () => {
    const sheets = componentStyleText();
    expect(sheets.length).toBeGreaterThanOrEqual(5);
    expect(ARENA_CSS.length).toBeGreaterThan(500);
  });

  it('hides NOTHING at a phone width — no surface disappears the way the plan navigator did', () => {
    for (const { file, css } of componentStyleText()) {
      const hidden = phoneHiddenSelectors(css, PHONE);
      expect(hidden, `${file} hides ${hidden.map((h) => h.selector).join(', ')} at ${PHONE}px`).toEqual([]);
    }
    expect(phoneHiddenSelectors(ARENA_CSS, PHONE)).toEqual([]);
  });

  it('declares no box wider than the phone, so nothing sits off to the right', () => {
    for (const { file, css } of componentStyleText()) {
      const risks = overflowRisks(css, PHONE);
      expect(risks, `${file}: ${risks.map((r) => r.selector + ' ' + r.declaration).join('; ')}`).toEqual([]);
    }
  });

  it('builds its multi-column layout UP from one column, rather than down from two', () => {
    // `.a-cols` is the only grid that becomes columns, and it does so inside a MIN-width query —
    // so the phone gets the single-column default instead of a two-column layout being undone.
    expect(ARENA_CSS).toContain('grid-template-columns: 1fr;');
    expect(/@media \(min-width: \d+px\) \{\s*\.a-cols/.test(ARENA_CSS)).toBe(true);
    expect(ARENA_CSS).not.toMatch(/@media \(max-width[^)]*\)[^}]*display:\s*none/);
  });

  it('keeps every tap target at least 40px tall', () => {
    // A 40px minimum on the controls the arena adds; the phone is the primary surface.
    expect(ARENA_CSS).toMatch(/min-height:\s*40px/);
  });

  it('lets long paths and prompts wrap instead of forcing a sideways scroll', () => {
    expect(ARENA_CSS).toContain('overflow-wrap: anywhere');
  });
});
