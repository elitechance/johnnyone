import { readFileSync } from 'node:fs';
import { fileURLToPath } from 'node:url';
import { dirname, resolve } from 'node:path';
import { describe, it, expect } from 'vitest';
import { THEME_TOKENS } from './theme-tokens';

// Pure spec (no Angular/Ionic) — runs under the plugin-less web/vitest.config.ts.
// Pins the amber accent + semantic status palette for BOTH blocks, asserts the
// old blue is gone, and parses styles.scss so the SCSS and the TS map can't drift
// (Overhaul P8 / phase 01 — D2/D11).

const STYLES_SCSS = readFileSync(
  resolve(dirname(fileURLToPath(import.meta.url)), '../../../styles.scss'),
  'utf8',
);

/**
 * Extract the body of the first CSS block whose selector line matches `selector`,
 * with `/* … *\/` comments stripped so a hex mentioned in a comment (e.g. "old
 * blue #5096ff") is never mistaken for a live token value.
 */
function block(css: string, selector: string): string {
  const start = css.indexOf(selector);
  if (start === -1) throw new Error(`selector not found: ${selector}`);
  const open = css.indexOf('{', start);
  const close = css.indexOf('}', open);
  return css.slice(open + 1, close).replace(/\/\*[\s\S]*?\*\//g, '');
}

/** Read a `--jo-*: #hex;` value out of a block body (whitespace-insensitive). */
function tokenOf(body: string, name: string): string | undefined {
  const m = body.match(new RegExp(`--${name}\\s*:\\s*(#[0-9a-fA-F]{3,8})`));
  return m?.[1]?.toLowerCase();
}

// The active dark block is the bare `:root {`; the dormant light block is
// `:root[data-theme="light"]`. Anchor on the exact selectors.
const darkBody = block(STYLES_SCSS, ':root {');
const lightBody = block(STYLES_SCSS, ':root[data-theme="light"]');

describe('THEME_TOKENS map', () => {
  it('pins the amber accent + ink (dark, active)', () => {
    expect(THEME_TOKENS.dark.accent).toBe('#e8a33d');
    expect(THEME_TOKENS.dark.accentHover).toBe('#f2b556');
    expect(THEME_TOKENS.dark.accentInk).toBe('#1a130a');
  });

  it('pins the five semantic status tokens (dark)', () => {
    expect(THEME_TOKENS.dark.stBriefing).toBe('#2fbcd8');
    expect(THEME_TOKENS.dark.stPlanning).toBe('#5d8bf7');
    expect(THEME_TOKENS.dark.stDevelopment).toBe('#9a7ef0');
    expect(THEME_TOKENS.dark.stReview).toBe('#e070a8');
    expect(THEME_TOKENS.dark.stDone).toBe('#4bbd74');
  });

  it('keeps the health tokens (attention/blocked/done)', () => {
    expect(THEME_TOKENS.dark.warn).toBe('#ffb454');
    expect(THEME_TOKENS.dark.bad).toBe('#ff6868');
    expect(THEME_TOKENS.dark.good).toBe('#5fd78f');
  });

  it('pins the dormant light palette', () => {
    expect(THEME_TOKENS.light.accent).toBe('#b9751a');
    expect(THEME_TOKENS.light.accentHover).toBe('#d68a24');
    expect(THEME_TOKENS.light.accentInk).toBe('#ffffff');
    expect(THEME_TOKENS.light.stBriefing).toBe('#0e8aa6');
    expect(THEME_TOKENS.light.stPlanning).toBe('#3161d6');
    expect(THEME_TOKENS.light.stDevelopment).toBe('#6f4fd0');
    expect(THEME_TOKENS.light.stReview).toBe('#bd3f82');
    expect(THEME_TOKENS.light.stDone).toBe('#268a4d');
    expect(THEME_TOKENS.light.warn).toBe('#c96a1f');
    expect(THEME_TOKENS.light.bad).toBe('#c93f3a');
    expect(THEME_TOKENS.light.good).toBe('#268a4d');
  });

  it('has dropped the old blue accent', () => {
    for (const palette of [THEME_TOKENS.dark, THEME_TOKENS.light]) {
      expect(palette.accent).not.toBe('#5096ff');
      expect(palette.accentHover).not.toBe('#73abff');
    }
  });
});

describe('styles.scss mirrors THEME_TOKENS (no drift)', () => {
  it('still imports dark.always.css (dark stays active — D1)', () => {
    expect(STYLES_SCSS).toContain('dark.always.css');
  });

  it('has no old blue accent literal left in the :root token blocks', () => {
    expect(darkBody).not.toContain('#5096ff');
    expect(darkBody).not.toContain('#73abff');
    expect(lightBody).not.toContain('#5096ff');
    expect(lightBody).not.toContain('#73abff');
  });

  const cases: Array<[keyof typeof THEME_TOKENS.dark, string]> = [
    ['accent', 'jo-accent'],
    ['accentHover', 'jo-accent-hover'],
    ['accentInk', 'jo-accent-ink'],
    ['stBriefing', 'jo-st-briefing'],
    ['stPlanning', 'jo-st-planning'],
    ['stDevelopment', 'jo-st-development'],
    ['stReview', 'jo-st-review'],
    ['stDone', 'jo-st-done'],
    ['warn', 'jo-warn'],
    ['bad', 'jo-bad'],
    ['good', 'jo-good'],
  ];

  for (const [key, cssName] of cases) {
    it(`dark --${cssName} matches THEME_TOKENS.dark.${key}`, () => {
      expect(tokenOf(darkBody, cssName)).toBe(THEME_TOKENS.dark[key].toLowerCase());
    });
    it(`light --${cssName} matches THEME_TOKENS.light.${key}`, () => {
      expect(tokenOf(lightBody, cssName)).toBe(THEME_TOKENS.light[key].toLowerCase());
    });
  }
});
