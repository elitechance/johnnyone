/**
 * Pure status/health → color-token + label map for the initiative lifecycle
 * (Overhaul P8 / phase 02 — D5). The single source the lifecycle bar, the status
 * pills, and the console master-list all consume, so the pill/bar/list stay
 * visually identical and unit-testable without a browser — mirroring the
 * `diff-parse.ts` pure-module pattern (no Angular/Ionic/DOM import here; the spec
 * lives under `web/` and runs on the plugin-less vitest).
 *
 * Token NAMES only — the hex values live in the P01 token layer
 * (`web/src/styles.scss`: `--jo-st-*`, `--jo-warn`/`--jo-bad`/`--jo-good`).
 */

export interface StatusMeta {
  /** CSS custom-property name carrying the color, e.g. `--jo-st-development`. */
  cssVar: string;
  /** Human label, e.g. `development` / `needs-attention`. */
  label: string;
  /** Space-separated class list mirroring the mock, e.g. `st-development` / `health att`. */
  className: string;
}

/** The five initiative stages, in lifecycle order (mock §01). */
export const LIFECYCLE_STAGES = [
  'briefing',
  'planning',
  'development',
  'review',
  'done',
] as const;

export type LifecycleStage = (typeof LIFECYCLE_STAGES)[number];

/** Per-stage color/label/class (mock `.st-*`). */
export const STATUS_META: Record<LifecycleStage, StatusMeta> = {
  briefing: { cssVar: '--jo-st-briefing', label: 'briefing', className: 'st-briefing' },
  planning: { cssVar: '--jo-st-planning', label: 'planning', className: 'st-planning' },
  development: { cssVar: '--jo-st-development', label: 'development', className: 'st-development' },
  review: { cssVar: '--jo-st-review', label: 'review', className: 'st-review' },
  done: { cssVar: '--jo-st-done', label: 'done', className: 'st-done' },
};

/** Health axis color/label/class (mock `.health` / `.health.att` / `.health.blk`). */
export const HEALTH_META: Record<string, StatusMeta> = {
  'in-progress': { cssVar: '--jo-good', label: 'in-progress', className: 'health' },
  // Terminal, healthy end state — a fully-approved run is done, not "in-progress".
  // Backend `health_from_status` maps approved/complete → 'complete'; 'done' is an alias.
  complete: { cssVar: '--jo-st-done', label: 'complete', className: 'health done' },
  done: { cssVar: '--jo-st-done', label: 'complete', className: 'health done' },
  'needs-attention': { cssVar: '--jo-warn', label: 'needs-attention', className: 'health att' },
  blocked: { cssVar: '--jo-bad', label: 'blocked', className: 'health blk' },
};

/** Benign fallback for an unknown status/health value — never throws. */
function unknownMeta(raw: string): StatusMeta {
  return { cssVar: '--jo-fg-muted', label: raw, className: 'st-unknown' };
}

/** Normalize a raw input for lookup: trim + lowercase, tolerating `null`/`undefined`. */
function key(value: string | null | undefined): string {
  return (value ?? '').trim().toLowerCase();
}

/** Map an `initiativeStatus` to its `{ cssVar, label, className }`; unknown → fallback. */
export function statusMeta(status: string | null | undefined): StatusMeta {
  return STATUS_META[key(status) as LifecycleStage] ?? unknownMeta((status ?? '').trim());
}

/** Map a `health` value to its `{ cssVar, label, className }`; unknown → fallback. */
export function healthMeta(health: string | null | undefined): StatusMeta {
  return HEALTH_META[key(health)] ?? unknownMeta((health ?? '').trim());
}

/** Index of a status within `LIFECYCLE_STAGES` (`-1` if unknown) — drives complete/active/inert bars. */
export function stageIndex(status: string | null | undefined): number {
  return (LIFECYCLE_STAGES as readonly string[]).indexOf(key(status));
}
