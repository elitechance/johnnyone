// Pure decision layer behind the mock §02 initiative console (Overhaul P8 / phase 04). Kept
// Angular/Ionic/DOM-free so the spec runs under the plugin-less web vitest — mirroring the
// `terminal-transcript-tab.ts` / `validation-config-logic.ts` pure-seam pattern. Every transform is a
// projection over EXISTING client data (`listAgentPlans`, the loaded `gitDiff`, `validationConfig`);
// no color/lens logic is re-implemented here — it delegates to P2 `lifecycle-status` and P7
// `validation-config-logic` (D5/D7). Types are imported type-only (erased at build) at the depth the
// shipped pure siblings use, so no Angular/runtime dependency leaks into the plugin-less vitest.
import type {
  AgentPlan,
  GitDiffView,
} from '../../../../../ui/src/services/johnny-api.service';
import {
  statusMeta,
  healthMeta,
  StatusMeta,
} from '../../../../../ui/src/lib/lifecycle-status';
import {
  defaultLenses,
  fromConfigJson,
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

/** One touched-file row (mock §02 `.filetree` `.fnode`, 674-684). */
export interface TouchedFile {
  path: string;
  adds: number;
  dels: number;
  badge: 'edited' | 'new';
}

/**
 * Map the ALREADY-LOADED `gitDiff` view (P7 `diffs` signal — no new data call, D8) to compact
 * touched-file rows. `badge:'new'` when the file only added lines (`deletions===0 && additions>0`),
 * else `'edited'`. A null / clean / empty view → `[]` (the benign empty state P7 already produces).
 */
export function touchedFiles(view: GitDiffView | null | undefined): TouchedFile[] {
  if (!view || view.clean || !view.files) return [];
  return view.files.map((f) => {
    const adds = f.additions ?? 0;
    const dels = f.deletions ?? 0;
    return {
      path: f.path,
      adds,
      dels,
      badge: dels === 0 && adds > 0 ? 'new' : 'edited',
    };
  });
}

/** The §08 mobile segment switcher (mock 1083-1085). */
export const CONSOLE_SEGMENTS = ['transcript', 'files', 'validation'] as const;
export type ConsoleSegment = (typeof CONSOLE_SEGMENTS)[number];

/** Map a (possibly untrusted) segment id to the pane it shows; unknown → `'transcript'` (mobile default). */
export function consolePaneFor(segment: string | null | undefined): ConsoleSegment {
  return (CONSOLE_SEGMENTS as readonly string[]).includes(segment ?? '')
    ? (segment as ConsoleSegment)
    : 'transcript';
}
