// Pure decision layer for the console master-detail selection (console-features P02, C1). Resolves the
// selected initiative from EITHER id form and reports whether an unresolved selection is a recoverable
// "strand" — so the mobile list/back affordance is never hidden with nothing to show. Kept Angular/DOM-
// free so the spec runs under the plugin-less web vitest, mirroring `console-logic.ts`. Types are
// `import type` only (erased at build), so no `@johnnyone/ui`/Angular runtime dependency leaks in.
import type { AgentPlan } from '../../../../../ui/src/services/johnny-api.service';

/**
 * Resolve the selected plan-run. A selection value may be a plan-run `id` (plain nav auto-selects
 * `plans[0].id`) OR a group `initiativeId` (a `?initiativeId=<id>` deep-link). Match `id` first, then
 * `initiativeId`, else `null`. Total/null-safe.
 */
export function resolveSelectedInitiative(
  plans: AgentPlan[] | null | undefined,
  selectedId: string | null | undefined,
): AgentPlan | null {
  const id = (selectedId ?? '').trim();
  if (!id) return null;
  const list = plans ?? [];
  return list.find((p) => p.id === id) ?? list.find((p) => p.initiativeId === id) ?? null;
}

export interface SelectionState {
  /** A selection value is set (deep-link or click). */
  hasSelection: boolean;
  /** The plan it resolves to (by id or initiativeId), or null. */
  resolved: AgentPlan | null;
  /** The strand condition: a selection is set but resolves to no plan — the mobile list/back
   *  affordance must stay visible so the user can recover instead of being stuck on the empty state. */
  recoverableEmpty: boolean;
}

/** Selection state used to drive both the center pane and the mobile list-hiding decision. */
export function selectionState(
  plans: AgentPlan[] | null | undefined,
  selectedId: string | null | undefined,
): SelectionState {
  const hasSelection = !!(selectedId ?? '').trim();
  const resolved = resolveSelectedInitiative(plans, selectedId);
  return { hasSelection, resolved, recoverableEmpty: hasSelection && !resolved };
}
