/**
 * Pure, framework-free mirror of the amber theme-token palette defined in
 * `web/src/styles.scss` (Overhaul P8 / D2).
 *
 * The plugin-less web vitest (`web/vitest.config.ts`) cannot resolve compiled
 * CSS custom properties, so the SCSS `:root` blocks and this map are kept as
 * two views of ONE source of truth: every hex here must equal the matching
 * `--jo-*` value in `styles.scss`. `theme-tokens.spec.ts` pins these values so
 * the two never drift.
 *
 *   dark  -> the active palette (`:root`, rendered under `dark.always.css`)
 *   light -> the dormant palette (`:root[data-theme="light"]`, authored per the
 *            mock but not activated — dark stays the rendered theme, D1)
 */
export interface ThemePalette {
  /** Interactive accent (mock `--accent`). */
  accent: string;
  /** Accent hover state (mock `--accent-hover`). */
  accentHover: string;
  /** On-accent text/ink (mock `--accent-ink`). */
  accentInk: string;
  /** Semantic per-stage status colors (mock `--st-*`). */
  stBriefing: string;
  stPlanning: string;
  stDevelopment: string;
  stReview: string;
  stDone: string;
  /** Health tokens (attention / blocked / done|pass). */
  warn: string;
  bad: string;
  good: string;
}

export const THEME_TOKENS: { readonly dark: ThemePalette; readonly light: ThemePalette } = {
  dark: {
    accent: '#e8a33d',
    accentHover: '#f2b556',
    accentInk: '#1a130a',
    stBriefing: '#2fbcd8',
    stPlanning: '#5d8bf7',
    stDevelopment: '#9a7ef0',
    stReview: '#e070a8',
    stDone: '#4bbd74',
    warn: '#ffb454',
    bad: '#ff6868',
    good: '#5fd78f',
  },
  light: {
    accent: '#b9751a',
    accentHover: '#d68a24',
    accentInk: '#ffffff',
    stBriefing: '#0e8aa6',
    stPlanning: '#3161d6',
    stDevelopment: '#6f4fd0',
    stReview: '#bd3f82',
    stDone: '#268a4d',
    warn: '#c96a1f',
    bad: '#c93f3a',
    good: '#268a4d',
  },
} as const;
