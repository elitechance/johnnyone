/**
 * Pure, framework-free mirror of the amber theme-token palette defined in
 * `host-app/src/styles.scss` (Overhaul P9 / phase 01 / D1).
 *
 * The plugin-less host-app vitest (`host-app/vitest.config.ts`) cannot resolve
 * compiled CSS custom properties, so the SCSS `:root` blocks and this map are
 * kept as two views of ONE source of truth: every hex here must equal the
 * matching `--jo-*` value in `styles.scss`. `host-theme-tokens.spec.ts` pins
 * these values AND parses the SCSS so the two never drift.
 *
 * The accent matches the web app (P8, `web/.../theme-tokens.ts`); the surface
 * tokens come from the host mock (`artifacts/host-app-mock.html` :root blocks).
 *
 *   dark  -> the active palette (`:root`, rendered under `dark.always.css`)
 *   light -> the dormant palette (`:root[data-theme="light"]`, authored per the
 *            mock but not activated — dark stays the rendered theme, D1)
 */
export interface HostThemePalette {
  /** Interactive accent (mock `--accent`). */
  accent: string;
  /** Accent hover state (mock `--accent-hover`). */
  accentHover: string;
  /** Second accent stop — the mock glyph gradient (mock `--accent-2`). */
  accent2: string;
  /** On-accent text/ink (mock `--accent-ink`). */
  accentInk: string;
  /** Surface scale (mock `--ground`/`--panel`/`--panel-2`/`--panel-3`). */
  ground: string;
  panel: string;
  panel2: string;
  panel3: string;
  /** Foreground scale (mock `--ink`/`--ink-2`/`--ink-3`). */
  ink: string;
  ink2: string;
  ink3: string;
  /** Borders / dividers (mock `--line`/`--line-2`). */
  line: string;
  line2: string;
  /** State tokens (mock `--good`/`--warn`/`--crit`/`--info`/`--violet`). */
  good: string;
  warn: string;
  crit: string;
  info: string;
  violet: string;
}

export const HOST_THEME_TOKENS: { readonly dark: HostThemePalette; readonly light: HostThemePalette } = {
  dark: {
    accent: '#e8a33d',
    accentHover: '#f2b556',
    accent2: '#f2b556',
    accentInk: '#1a130a',
    ground: '#0c1016',
    panel: '#141a23',
    panel2: '#1a212c',
    panel3: '#222b38',
    ink: '#e7ecf3',
    ink2: '#9aa6b8',
    ink3: '#657085',
    line: '#232c39',
    line2: '#2e3948',
    good: '#4bbd74',
    warn: '#e59042',
    crit: '#e86560',
    info: '#5d8bf7',
    violet: '#9a7ef0',
  },
  light: {
    accent: '#b9751a',
    accentHover: '#d68a24',
    accent2: '#d68a24',
    accentInk: '#ffffff',
    ground: '#f4f6f9',
    panel: '#ffffff',
    panel2: '#eef1f6',
    panel3: '#e6eaf1',
    ink: '#161b25',
    ink2: '#525c6e',
    ink3: '#8b95a6',
    line: '#dce1ea',
    line2: '#cbd2de',
    good: '#268a4d',
    warn: '#c96a1f',
    crit: '#c93f3a',
    info: '#3161d6',
    violet: '#6f4fd0',
  },
} as const;
