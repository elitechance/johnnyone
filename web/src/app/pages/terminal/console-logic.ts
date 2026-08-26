// Pure decision layer behind the mock §02 initiative console (Overhaul P8 / phase 04). Kept
// Angular/Ionic/DOM-free so the spec runs under the plugin-less web vitest — mirroring the
// `terminal-transcript-tab.ts` / `validation-config-logic.ts` pure-seam pattern. Every transform is a
// projection over EXISTING client data (`listAgentPlans`, the loaded `gitDiff`, `validationConfig`);
// no color/lens logic is re-implemented here — it delegates to P2 `lifecycle-status` and P7
// `validation-config-logic` (D5/D7). Types are imported type-only (erased at build) at the depth the
// shipped pure siblings use, so no Angular/runtime dependency leaks into the plugin-less vitest.
import type { AgentPlan } from '../../../../../ui/src/services/johnny-api.service';
import { isLocalSmall } from './console-tabs-logic';
import {
  statusMeta,
  healthMeta,
  StatusMeta,
  LIFECYCLE_STAGES,
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
  executorConfig?: string | null;
}

/** Lifecycle progression rank; unknown/blank sorts LEAST advanced (-1). */
function stageRank(status: string | null | undefined): number {
  return (LIFECYCLE_STAGES as readonly string[]).indexOf(status ?? '');
}

/**
 * Project `listAgentPlans` rows to the master-list model. `nowIso` is injected (no `Date.now()` here)
 * so the spec is deterministic; status/health meta come from P2 `lifecycle-status.ts`.
 *
 * An Initiative can span TWO plan-runs (planning + development) that share one `initiativeId`, and
 * `listAgentPlans` returns one row PER run. Collapse them to ONE master-list row per initiative,
 * represented by the run FURTHEST along the lifecycle (the development run once it exists — it carries
 * the live status/health while the planning run stays pinned at `planning`). So the console shows one
 * entity, not a planning + development duplicate. Selection matches EITHER run's `id` or the shared
 * `initiativeId`, so a deep-link to either still highlights the merged row.
 */
export function initiativeRows(
  plans: AgentPlan[] | null | undefined,
  selectedId: string | null,
  nowIso: string,
): InitiativeRow[] {
  const groups = new Map<string, AgentPlan[]>();
  const order: string[] = [];
  for (const p of plans ?? []) {
    const key = p.initiativeId || p.id;
    if (!groups.has(key)) {
      groups.set(key, []);
      order.push(key);
    }
    groups.get(key)!.push(p);
  }
  return order.map((key) => {
    const group = groups.get(key)!;
    // Representative = most-advanced lifecycle stage, then most-recently updated.
    const rep = group.reduce((best, p) => {
      const delta = stageRank(p.initiativeStatus) - stageRank(best.initiativeStatus);
      if (delta > 0) return p;
      if (delta < 0) return best;
      return (p.updatedAt ?? '') > (best.updatedAt ?? '') ? p : best;
    });
    const status = rep.initiativeStatus ?? '';
    const health = rep.health ?? '';
    return {
      id: rep.id,
      title: rep.title || '(untitled)',
      status,
      health,
      statusMeta: statusMeta(status),
      healthMeta: healthMeta(health),
      showHealth: !!health && health !== 'in-progress',
      ago: formatRelTime(rep.updatedAt, nowIso),
      selected: key === selectedId || group.some((p) => p.id === selectedId),
      executorConfig: rep.executorConfig,
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

/** S10: purple kloo chip only for local-small. */
export function modeChip(executorConfig: string | null | undefined): 'kloo' | null {
  return isLocalSmall(executorConfig) ? 'kloo' : null;
}

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
