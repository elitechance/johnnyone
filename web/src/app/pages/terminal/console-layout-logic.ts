// Pure, framework/DOM-free helpers behind the resizable initiative-console dividers (Overhaul P9 /
// phase P5, D6/D7). Kept Angular/Ionic/DOM-free so the spec runs under the plugin-less web vitest,
// mirroring the shipped `console-tabs-logic.ts` / `plan-tab-logic.ts` pure-seam pattern. The page owns
// the pointer handlers + signals and delegates clamp / grid-template / persistence-parse to these.

/** Rail (initiative master-list) width bounds. Default 216px = the shipped `.console` first track. */
export const RAIL_MIN = 180;
export const RAIL_MAX = 420;
export const RAIL_DEFAULT = 216;

/** Validation right-column width bounds. Default 262px = the shipped `.console` third track. */
export const RIGHT_MIN = 220;
export const RIGHT_MAX = 520;
export const RIGHT_DEFAULT = 262;

/** The divider track width (px) — matches the `.console-divider` grid track and §02/console-states. */
export const DIVIDER_PX = 6;

/** Clamp the rail width to `[RAIL_MIN, RAIL_MAX]`; a non-finite value → `RAIL_DEFAULT`. */
export function clampRailWidth(px: number): number {
  if (!Number.isFinite(px)) return RAIL_DEFAULT;
  return Math.min(RAIL_MAX, Math.max(RAIL_MIN, px));
}

/** Clamp the right-column width to `[RIGHT_MIN, RIGHT_MAX]`; a non-finite value → `RIGHT_DEFAULT`. */
export function clampRightWidth(px: number): number {
  if (!Number.isFinite(px)) return RIGHT_DEFAULT;
  return Math.min(RIGHT_MAX, Math.max(RIGHT_MIN, px));
}

/**
 * The `grid-template-columns` value for the 3-pane console INCLUDING the two divider tracks.
 *
 * Track order matches the real desktop DOM (the persistent `.console-seg` is `display:none` on
 * desktop, so it occupies NO track): `rail | divider | center | divider | right`, i.e.
 * `${rail}px ${dividerPx}px minmax(0, 1fr) ${dividerPx}px ${right}px`. The center track is
 * `minmax(0, 1fr)` so it flexes and can shrink below its content width.
 */
export function consoleColumns(rail: number, right: number, dividerPx: number = DIVIDER_PX): string {
  return `${rail}px ${dividerPx}px minmax(0, 1fr) ${dividerPx}px ${right}px`;
}

/**
 * Parse a `localStorage` width string and clamp it with the provided clamp fn. Empty / null / junk →
 * the clamp fn's default (via `NaN`); an out-of-range but numeric string is clamped to the bound.
 */
export function parseStoredWidth(
  raw: string | null | undefined,
  clamp: (px: number) => number,
): number {
  if (raw == null || raw.trim() === '') return clamp(NaN);
  return clamp(Number(raw));
}
