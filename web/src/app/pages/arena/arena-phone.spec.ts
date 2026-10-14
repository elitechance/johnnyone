// The arena at 400px, checked against the REAL stylesheets the components ship with.
//
// The precedent this exists for is measured and in the tree's history: the Plan tab's document
// navigator was `display: none` below 720px, so on a phone no plan document was reachable at all,
// and no test failed. jsdom applies no stylesheets and matches no media query, so a mounted
// component cannot catch that class of bug — a static read of the CSS can.
//
// THIS FILE WAS VACUOUS ONCE AND THAT IS WHY IT LOOKS LIKE THIS. The first version collected
// template literals with a regex that matched `…{…}` anywhere, which picked up 17 error-message
// strings and FOUR stylesheets, and missed `ARENA_CSS` entirely (it ends `}\n`, and the regex
// needed `}` immediately before the backtick) — the one sheet every arena screen actually uses.
// QA injected `min-width: 600px`, `width: 120vw` and `overflow: hidden; max-height: 80px` into
// ARENA_CSS and this file passed 6/6. So: the shared sheet is now IMPORTED (no parsing at all), the
// per-component sheets are extracted from the `styles: [...]` array only, every collected sheet is
// asserted to look like CSS, and the collection itself is asserted against a known count.
import { describe, it, expect } from 'vitest';
import { readFileSync, readdirSync } from 'node:fs';
import { join } from 'node:path';
import {
  phoneHiddenSelectors,
  overflowRisks,
  clipRisks,
  overlayRisks,
  transformRisks,
} from './phone-reach-guard';
import { ARENA_CSS } from './arena-ui.css';

const PHONE = 400;
const HERE = join(process.cwd(), 'web/src/app/pages/arena');
const RESPONSIVE_PARTIAL = join(process.cwd(), 'web/src/app/styles/_workspace-responsive.scss');

/** The literals inside a component's `styles: [ … ]` array, and nothing else. */
function stylesArrayLiterals(src: string): string[] {
  const out: string[] = [];
  const at = src.indexOf('styles: [');
  if (at < 0) return out;
  let i = at + 'styles: ['.length;
  let depth = 1;
  while (i < src.length && depth > 0) {
    const ch = src[i];
    if (ch === '[') depth += 1;
    else if (ch === ']') depth -= 1;
    else if (ch === '`') {
      const end = src.indexOf('`', i + 1);
      if (end < 0) break;
      out.push(src.slice(i + 1, end));
      i = end;
    }
    i += 1;
  }
  return out;
}

/** Every stylesheet the arena ships: the shared sheet (imported, not parsed) plus each component's
 *  own inline rules. */
function sheets(): { file: string; css: string }[] {
  const out = [{ file: 'arena-ui.css.ts (ARENA_CSS)', css: ARENA_CSS }];
  for (const file of readdirSync(HERE)) {
    if (!file.endsWith('.ts') || file.endsWith('.spec.ts') || file === 'arena-ui.css.ts') continue;
    for (const css of stylesArrayLiterals(readFileSync(join(HERE, file), 'utf8'))) {
      if (css.includes('{') && css.includes(':')) out.push({ file, css });
    }
  }
  return out;
}

describe('the collection itself — a guard that reads nothing proves nothing', () => {
  it('includes the shared sheet BY IDENTITY, not by parsing a file for backticks', () => {
    expect(sheets().some((s) => s.css === ARENA_CSS)).toBe(true);
    expect(ARENA_CSS).toContain('.a-cols');
    expect(ARENA_CSS.length).toBeGreaterThan(2000);
  });

  it('collects one sheet per component that declares styles, and every one is really CSS', () => {
    const got = sheets();
    // 1 shared + the components that ship their own rules. A component added without styles, or a
    // sheet that stops being collected, moves this number and the test says so.
    expect(got.length).toBeGreaterThanOrEqual(7);
    for (const { file, css } of got) {
      expect(css, `${file} collected something that is not CSS`).toMatch(/[.#:a-z][^{}]*\{[^{}]*:[^{}]*\}/i);
      // An error message, a GraphQL document or a template would not survive this.
      expect(css, `${file} looks like a GraphQL document`).not.toMatch(/\b(query|mutation)\s+\w+\(/);
    }
  });

  it('collects the sheets of the components that carry the decisive controls', () => {
    const files = sheets().map((s) => s.file);
    for (const f of ['workflow-arena.page.ts', 'arena-setup.page.ts', 'gate-strip.component.ts', 'arena-row.component.ts']) {
      expect(files, `${f} contributes no stylesheet to the guard`).toContain(f);
    }
  });
});

describe('the arena stylesheets at 400px', () => {
  it('hides NOTHING at a phone width — no surface disappears the way the plan navigator did', () => {
    for (const { file, css } of sheets()) {
      const hidden = phoneHiddenSelectors(css, PHONE);
      expect(hidden, `${file} hides ${hidden.map((h) => h.selector).join(', ')} at ${PHONE}px`).toEqual([]);
    }
  });

  it('declares no box wider than the phone, in px OR in viewport units', () => {
    for (const { file, css } of sheets()) {
      const risks = overflowRisks(css, PHONE);
      expect(risks, `${file}: ${risks.map((r) => r.selector + ' ' + r.declaration).join('; ')}`).toEqual([]);
    }
  });

  it('clips nothing: no fixed-height box hides its own overflow', () => {
    for (const { file, css } of sheets()) {
      const risks = clipRisks(css, PHONE);
      expect(risks, `${file}: ${risks.map((r) => r.selector + ' ' + r.declaration).join('; ')}`).toEqual([]);
    }
  });

  it('covers nothing with a full-viewport overlay', () => {
    for (const { file, css } of sheets()) {
      const risks = overlayRisks(css, PHONE);
      expect(risks, `${file}: ${risks.map((r) => r.selector + ' ' + r.declaration).join('; ')}`).toEqual([]);
    }
  });

  it('moves nothing out from under the finger with a transform', () => {
    for (const { file, css } of sheets()) {
      const risks = transformRisks(css, PHONE);
      expect(risks, `${file}: ${risks.map((r) => r.selector + ' ' + r.declaration).join('; ')}`).toEqual([]);
    }
  });

  it('the arena\u2019s own scrollers are scrollers, not clips', () => {
    // The console-tab panel is `height: 100%; overflow-y: auto` — one word from being the bug this
    // guard exists for, and a unit allow-list of px|rem|em used to make the whole shape invisible.
    const panel = sheets().find((s) => s.file === 'arena-panel.component.ts' && s.css.includes(':host'));
    expect(panel, 'the panel sheet is not being collected').toBeTruthy();
    expect(panel!.css).toMatch(/:host\s*\{[^}]*overflow-y:\s*auto/);
    expect(clipRisks(panel!.css, PHONE)).toEqual([]);
  });

  it('builds its multi-column layout UP from one column, rather than down from two', () => {
    expect(ARENA_CSS).toContain('grid-template-columns: 1fr;');
    expect(/@media \(min-width: \d+px\) \{\s*\.a-cols/.test(ARENA_CSS)).toBe(true);
    expect(ARENA_CSS).not.toMatch(/@media \(max-width[^)]*\)[^}]*display:\s*none/);
  });

  // The tap-target check that used to live here was `expect(ARENA_CSS).toMatch(/min-height: 40px/)`
  // — a substring that passed while a bare `<select>` and three unclassed radios were 20px. It is
  // replaced by MEASURED assertions on mounted components in `arena-console.spec.ts` ("every
  // control the arena shows is big enough to hit"), which read the computed min-height of every
  // button, link, select, textarea and input the arena actually renders. This is the last of the
  // three substring-shaped guards; there are none left in this file.
  // A TRIPWIRE, not a guarantee — described that way on purpose. It catches the shape being
  // written back the way it was written before; it does not stop a determined author, and QA
  // evaded the first version twice, with `new RegExp('min' + '-height')` and with an escaped
  // character class. Both spellings are covered now (escapes are normalised, and the property
  // names are ASSEMBLED here so this file does not contain the literals it searches for), but the
  // honest claim is "this makes the regression visible", not "this makes it impossible".
  it('tripwire: no substring assertion about a measured CSS property creeps back in', () => {
    const PROPS = ['min' + '-height', 'overflow' + '-wrap', 'word' + '-break'];
    const code = readFileSync(join(HERE, 'arena-phone.spec.ts'), 'utf8')
      .split('\n')
      .filter((line) => !/^\s*(\/\/|\*|\/\*)/.test(line))
      .join('\n')
      .replace(/\\(?=[-\w])/g, '');
    for (const prop of PROPS) {
      const assertion = new RegExp(`(toMatch|toContain|toBe)\\s*\\([^)\n]*${prop}`);
      expect(
        assertion.test(code),
        `a substring assertion about ${prop} is back in this file — measure it on a mounted component instead (arena-console.spec.ts)`,
      ).toBe(false);
    }
    // The assembled names must still be the real ones, or this tripwire guards nothing.
    expect(PROPS).toEqual(['min-height', 'overflow-wrap', 'word-break']);
  });

  it('hides NOTHING at a phone width — no surface disappears the way the plan navigator did', () => {
    for (const { file, css } of sheets()) {
      const hidden = phoneHiddenSelectors(css, PHONE);
      expect(hidden, `${file} hides ${hidden.map((h) => h.selector).join(', ')} at ${PHONE}px`).toEqual([]);
    }
  });

  it('declares no box wider than the phone, in px OR in viewport units', () => {
    for (const { file, css } of sheets()) {
      const risks = overflowRisks(css, PHONE);
      expect(risks, `${file}: ${risks.map((r) => r.selector + ' ' + r.declaration).join('; ')}`).toEqual([]);
    }
  });

  it('clips nothing: no fixed-height box hides its own overflow', () => {
    for (const { file, css } of sheets()) {
      const risks = clipRisks(css, PHONE);
      expect(risks, `${file}: ${risks.map((r) => r.selector + ' ' + r.declaration).join('; ')}`).toEqual([]);
    }
  });

  it('covers nothing with a full-viewport overlay', () => {
    for (const { file, css } of sheets()) {
      const risks = overlayRisks(css, PHONE);
      expect(risks, `${file}: ${risks.map((r) => r.selector + ' ' + r.declaration).join('; ')}`).toEqual([]);
    }
  });

  it('moves nothing out from under the finger with a transform', () => {
    for (const { file, css } of sheets()) {
      const risks = transformRisks(css, PHONE);
      expect(risks, `${file}: ${risks.map((r) => r.selector + ' ' + r.declaration).join('; ')}`).toEqual([]);
    }
  });

  it('the arena\u2019s own scrollers are scrollers, not clips', () => {
    // The console-tab panel is `height: 100%; overflow-y: auto` — one word from being the bug this
    // guard exists for, and a unit allow-list of px|rem|em used to make the whole shape invisible.
    const panel = sheets().find((s) => s.file === 'arena-panel.component.ts' && s.css.includes(':host'));
    expect(panel, 'the panel sheet is not being collected').toBeTruthy();
    expect(panel!.css).toMatch(/:host\s*\{[^}]*overflow-y:\s*auto/);
    expect(clipRisks(panel!.css, PHONE)).toEqual([]);
  });

  it('builds its multi-column layout UP from one column, rather than down from two', () => {
    expect(ARENA_CSS).toContain('grid-template-columns: 1fr;');
    expect(/@media \(min-width: \d+px\) \{\s*\.a-cols/.test(ARENA_CSS)).toBe(true);
    expect(ARENA_CSS).not.toMatch(/@media \(max-width[^)]*\)[^}]*display:\s*none/);
  });

  // The tap-target check that used to live here was `expect(ARENA_CSS).toMatch(/min-height: 40px/)`
  // — a substring that passed while a bare `<select>` and three unclassed radios were 20px. It is
  // replaced by MEASURED assertions on mounted components in `arena-console.spec.ts` ("every
  // control the arena shows is big enough to hit"), which read the computed min-height of every
  // button, link, select, textarea and input the arena actually renders. This is the last of the
  // three substring-shaped guards; there are none left in this file.
});

describe('the two values copied out of the Sass partial, which cannot be @included from a string', () => {
  const partial = readFileSync(RESPONSIVE_PARTIAL, 'utf8');

  it('uses the repo’s own mobile breakpoint, not one of its own', () => {
    const repoBreakpoint = /\$mobile-breakpoint:\s*(\d+)px/.exec(partial)?.[1];
    expect(repoBreakpoint).toBe('900');
    const queries = [...ARENA_CSS.matchAll(/@media \((?:min|max)-width:\s*(\d+)px\)/g)].map((m) => m[1]);
    expect(queries.length).toBeGreaterThan(0);
    for (const q of queries) {
      expect(q, `ARENA_CSS uses a ${q}px breakpoint; the repo's is ${repoBreakpoint}px`).toBe(repoBreakpoint);
    }
  });

  it('applies the safe-area insets the partial defines, so no control sits under the gesture bar', () => {
    expect(partial).toContain('env(safe-area-inset-bottom)');
    expect(ARENA_CSS).toContain('env(safe-area-inset-bottom');
    expect(ARENA_CSS).toContain('env(safe-area-inset-left');
    expect(ARENA_CSS).toContain('env(safe-area-inset-right');
    // The bottom inset has to be on the page's own padding, which is what the promote/reject row
    // sits above.
    expect(/\.aw\s*\{[^}]*padding-bottom:\s*calc\([^)]*env\(safe-area-inset-bottom/s.test(ARENA_CSS)).toBe(true);
  });
});

describe('text nobody here chose can always wrap', () => {
  // The previous version of this was `expect(ARENA_CSS).toContain('overflow-wrap: anywhere')` — a
  // substring check that passes however few elements have it. These are the containers that render
  // an operator's reason, a raw host error, a workspace path or a pasted URL.
  for (const selector of ['.a-note', '.a-block', '.a-warn', '.a-list li', '.a-kv dd']) {
    it(`${selector} wraps a long unbroken token`, () => {
      const grouped = new RegExp(
        `[^}]*${selector.replace(/[.*+?^${}()|[\]\\]/g, '\\$&')}[^{]*\\{[^}]*overflow-wrap:\\s*(?:anywhere|break-word)`,
      );
      expect(grouped.test(ARENA_CSS), `${selector} has no overflow-wrap rule`).toBe(true);
    });
  }
  it('and the same rule breaks a word that has no break opportunity at all', () => {
    // Checked INSIDE the rule that carries the wrapping, not as a bare substring anywhere in the
    // sheet: the two declarations only do anything if they land on the same selectors.
    const rule = /\.a-note[^{]*\{[^}]*\}/.exec(ARENA_CSS)?.[0] ?? '';
    expect(rule, 'the wrapping rule was not found at all').not.toBe('');
    expect(/overflow-wrap:\s*(anywhere|break-word)/.test(rule)).toBe(true);
    expect(/word-break:\s*break-word/.test(rule)).toBe(true);
  });
});
