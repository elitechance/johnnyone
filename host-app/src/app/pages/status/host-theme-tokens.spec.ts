import { readFileSync } from 'node:fs';
import { fileURLToPath } from 'node:url';
import { dirname, resolve } from 'node:path';
import { describe, it, expect } from 'vitest';
import { HOST_THEME_TOKENS } from './host-theme-tokens';

// Pure spec (no Angular/Ionic) — runs under the plugin-less host-app/vitest.config.ts.
// Pins the amber accent + the mock's surface palette for BOTH blocks, asserts the
// Ionic-default blue is gone, and parses styles.scss so the SCSS and the TS map
// can't drift (Overhaul P9 / phase 01 — D1/D6).

const STYLES_SCSS = readFileSync(
  resolve(dirname(fileURLToPath(import.meta.url)), '../../../styles.scss'),
  'utf8',
);

/**
 * Extract the body of the first CSS block whose selector line matches `selector`,
 * with `/* … *\/` comments stripped so a hex mentioned in a comment is never
 * mistaken for a live token value.
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

// key -> the matching `--jo-*` custom-property name in the SCSS.
const cases: Array<[keyof HostThemePalette, string]> = [
  ['accent', 'jo-accent'],
  ['accentHover', 'jo-accent-hover'],
  ['accent2', 'jo-accent-2'],
  ['accentInk', 'jo-accent-ink'],
  ['ground', 'jo-ground'],
  ['panel', 'jo-panel'],
  ['panel2', 'jo-panel-2'],
  ['panel3', 'jo-panel-3'],
  ['ink', 'jo-ink'],
  ['ink2', 'jo-ink-2'],
  ['ink3', 'jo-ink-3'],
  ['line', 'jo-line'],
  ['line2', 'jo-line-2'],
  ['good', 'jo-good'],
  ['warn', 'jo-warn'],
  ['crit', 'jo-crit'],
  ['info', 'jo-info'],
  ['violet', 'jo-violet'],
];

describe('HOST_THEME_TOKENS map', () => {
  it('pins the amber accent + ink (dark, active) — identical to web P8', () => {
    expect(HOST_THEME_TOKENS.dark.accent).toBe('#e8a33d');
    expect(HOST_THEME_TOKENS.dark.accentHover).toBe('#f2b556');
    expect(HOST_THEME_TOKENS.dark.accent2).toBe('#f2b556');
    expect(HOST_THEME_TOKENS.dark.accentInk).toBe('#1a130a');
  });

  it("pins the mock's dark surface tokens", () => {
    expect(HOST_THEME_TOKENS.dark.ground).toBe('#0c1016');
    expect(HOST_THEME_TOKENS.dark.panel).toBe('#141a23');
    expect(HOST_THEME_TOKENS.dark.panel2).toBe('#1a212c');
    expect(HOST_THEME_TOKENS.dark.panel3).toBe('#222b38');
    expect(HOST_THEME_TOKENS.dark.ink).toBe('#e7ecf3');
    expect(HOST_THEME_TOKENS.dark.ink2).toBe('#9aa6b8');
    expect(HOST_THEME_TOKENS.dark.ink3).toBe('#657085');
    expect(HOST_THEME_TOKENS.dark.line).toBe('#232c39');
    expect(HOST_THEME_TOKENS.dark.line2).toBe('#2e3948');
  });

  it("pins the mock's dark state tokens", () => {
    expect(HOST_THEME_TOKENS.dark.good).toBe('#4bbd74');
    expect(HOST_THEME_TOKENS.dark.warn).toBe('#e59042');
    expect(HOST_THEME_TOKENS.dark.crit).toBe('#e86560');
    expect(HOST_THEME_TOKENS.dark.info).toBe('#5d8bf7');
    expect(HOST_THEME_TOKENS.dark.violet).toBe('#9a7ef0');
  });

  it("pins the dormant light palette (mock light hexes)", () => {
    expect(HOST_THEME_TOKENS.light.accent).toBe('#b9751a');
    expect(HOST_THEME_TOKENS.light.accentHover).toBe('#d68a24');
    expect(HOST_THEME_TOKENS.light.accent2).toBe('#d68a24');
    expect(HOST_THEME_TOKENS.light.accentInk).toBe('#ffffff');
    expect(HOST_THEME_TOKENS.light.ground).toBe('#f4f6f9');
    expect(HOST_THEME_TOKENS.light.panel).toBe('#ffffff');
    expect(HOST_THEME_TOKENS.light.panel2).toBe('#eef1f6');
    expect(HOST_THEME_TOKENS.light.panel3).toBe('#e6eaf1');
    expect(HOST_THEME_TOKENS.light.ink).toBe('#161b25');
    expect(HOST_THEME_TOKENS.light.ink2).toBe('#525c6e');
    expect(HOST_THEME_TOKENS.light.ink3).toBe('#8b95a6');
    expect(HOST_THEME_TOKENS.light.line).toBe('#dce1ea');
    expect(HOST_THEME_TOKENS.light.line2).toBe('#cbd2de');
    expect(HOST_THEME_TOKENS.light.good).toBe('#268a4d');
    expect(HOST_THEME_TOKENS.light.warn).toBe('#c96a1f');
    expect(HOST_THEME_TOKENS.light.crit).toBe('#c93f3a');
    expect(HOST_THEME_TOKENS.light.info).toBe('#3161d6');
    expect(HOST_THEME_TOKENS.light.violet).toBe('#6f4fd0');
  });

  it('is not the Ionic default primary blue', () => {
    for (const palette of [HOST_THEME_TOKENS.dark, HOST_THEME_TOKENS.light]) {
      expect(palette.accent).not.toBe('#3880ff');
      expect(palette.accent).not.toBe('#428cff');
    }
  });
});

describe('styles.scss mirrors HOST_THEME_TOKENS (no drift)', () => {
  it('imports dark.always.css (dark stays active — D1)', () => {
    expect(STYLES_SCSS).toContain("@import '@ionic/angular/css/palettes/dark.always.css'");
  });

  it('maps --ion-color-primary onto the amber accent', () => {
    expect(tokenOf(darkBody, 'ion-color-primary')).toBeUndefined(); // it's a var(), not a hex
    expect(darkBody).toMatch(/--ion-color-primary\s*:\s*var\(--jo-accent\)/);
  });

  it('has no Ionic-default blue as --jo-accent in either block', () => {
    expect(tokenOf(darkBody, 'jo-accent')).not.toBe('#3880ff');
    expect(tokenOf(darkBody, 'jo-accent')).not.toBe('#428cff');
    expect(tokenOf(lightBody, 'jo-accent')).not.toBe('#3880ff');
  });

  for (const [key, cssName] of cases) {
    it(`dark --${cssName} matches HOST_THEME_TOKENS.dark.${key}`, () => {
      expect(tokenOf(darkBody, cssName)).toBe(HOST_THEME_TOKENS.dark[key].toLowerCase());
    });
    it(`light --${cssName} matches HOST_THEME_TOKENS.light.${key}`, () => {
      expect(tokenOf(lightBody, cssName)).toBe(HOST_THEME_TOKENS.light[key].toLowerCase());
    });
  }
});
