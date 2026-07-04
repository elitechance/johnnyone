// Pure decision layer behind the mock §02 initiative console (Overhaul P8 / phase 04). Kept
// Angular/Ionic/DOM-free so the spec runs under the plugin-less web vitest — mirroring the
// `terminal-transcript-tab.ts` / `validation-config-logic.ts` pure-seam pattern. Every transform is a
// projection over EXISTING client data (`listAgentPlans`, the loaded `gitDiff`, `validationConfig`);
// no color/lens logic is re-implemented here — it delegates to P2 `lifecycle-status` and P7
// `validation-config-logic` (D5/D7). Types are imported type-only (erased at build) at the depth the
// shipped pure siblings use, so no Angular/runtime dependency leaks into the plugin-less vitest.
import type { AgentPlan } from '../../../../../ui/src/services/johnny-api.service';
import {
  statusMeta,
  healthMeta,
  StatusMeta,
} from '../../../../../ui/src/lib/lifecycle-status';
import {
  defaultLenses,
  fromConfigJson,
  lensSourceOf,
  type LensSource,
} from '../validation-config/validation-config-logic';
import { formatRelTime } from '../shells/shells-page-logic';

/** One initiative master-list row (mock §02 `.init`). */
export interface InitiativeRow {
  id: string;
  title: string;
  /** Raw `initiativeStatus` / `health` — fed to `<johnny-status-pill [value]>`. */
  status: string;
  health: string;
  /** Resolved meta (P2 `lifecycle-status`) — for callers that want the token/class directly. */
  statusMeta: StatusMeta;
  healthMeta: StatusMeta;
  /** Show a health pill only when the health axis is noteworthy (not the baseline in-progress). */
  showHealth: boolean;
  /** Relative "updated … ago" string (deterministic — `nowIso` injected). */
  ago: string;
  selected: boolean;
}

/**
 * Project `listAgentPlans` rows to the master-list model. `nowIso` is injected (no `Date.now()` here)
 * so the spec is deterministic; status/health meta come from P2 `lifecycle-status.ts`.
 */
export function initiativeRows(
  plans: AgentPlan[] | null | undefined,
  selectedId: string | null,
  nowIso: string,
): InitiativeRow[] {
  return (plans ?? []).map((p) => {
    const status = p.initiativeStatus ?? '';
    const health = p.health ?? '';
    return {
      id: p.id,
      title: p.title || '(untitled)',
      status,
      health,
      statusMeta: statusMeta(status),
      healthMeta: healthMeta(health),
      showHealth: !!health && health !== 'in-progress',
      ago: formatRelTime(p.updatedAt, nowIso),
      selected: p.id === selectedId,
    };
  });
}

/** One validation-lens chip row (mock §02 right pane, 658-673). */
export interface LensChip {
  name: string;
  provider: string;
  model: string;
  blocking: boolean;
}

/**
 * Resolve the initiative's lenses for the read-only summary, REUSING P7's parser (D7): `fromConfigJson`
 * already returns `defaultLenses()` for null/empty/invalid config, so this is one call — no second
 * parser, no default-triad re-implementation.
 */
export function lensSummary(validationConfig: string | null | undefined): LensChip[] {
  return fromConfigJson(validationConfig).map((lens) => ({
    name: lens.name,
    provider: lens.provider,
    model: lens.model,
    blocking: lens.blocking,
  }));
}

/** Re-export so a caller can assert the reused default triad without importing the P7 module directly. */
export { defaultLenses };

/**
 * Whether the selected initiative's validation lenses come from its OWN saved config (`'custom'`) or
 * the shared default template (`'default'`) — the signal that makes per-initiative validation legible
 * (phase P3). Delegates to `lensSourceOf` (the P7 parse-boundary owner) so the console and the
 * Configure page share one rule and never drift. Pure/total.
 */
export function lensSource(validationConfig: string | null | undefined): LensSource {
  return lensSourceOf(validationConfig);
}

export type { LensSource };

/** The §08 mobile segment switcher: which console COLUMN to show on a narrow screen. `console` is the
 *  center pane (Raw terminal / Plan / Diff) — renamed from the old `transcript` id after the Transcript
 *  surface was removed. */
export const CONSOLE_SEGMENTS = ['console', 'files', 'validation'] as const;
export type ConsoleSegment = (typeof CONSOLE_SEGMENTS)[number];

/** Map a (possibly untrusted) segment id to the pane it shows; unknown → `'console'` (mobile default). */
export function consolePaneFor(segment: string | null | undefined): ConsoleSegment {
  return (CONSOLE_SEGMENTS as readonly string[]).includes(segment ?? '')
    ? (segment as ConsoleSegment)
    : 'console';
}
