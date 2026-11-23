/**
 * Live updates of the plan detail (Plans tab, initiative 8ad3ae71 phase 03 task 09; D7: push first,
 * poll fallback). Every brief write already publishes `agent_plan_run_updated` over the relay, so
 * the page refetches on that push; a 5 s poll runs ONLY while the relay socket is down and the tab
 * is visible. DOM-free and Angular-free.
 */

/** The subset of `RelayTerminalService.agentPlanRuns()` updates this logic reads. */
export interface PlanRunUpdateLike {
  planId: string;
  deleted: boolean;
  run?: { plan?: { brief?: string | null; initiativeStatus?: string | null; status?: string | null } | null } | null;
}

/** What the page last rendered. */
export interface ShownPlan {
  brief?: string | null;
  initiativeStatus?: string | null;
}

/** Poll interval while the push channel is unavailable. */
export const POLL_MS = 5_000;

/**
 * Refetch the brief for this update? Only for THIS plan, and only when it was deleted, its rendered
 * brief text changed (every brief version re-renders it), or it left briefing / was closed.
 */
export function shouldRefetch(update: PlanRunUpdateLike | null | undefined, planId: string, shown: ShownPlan | null | undefined): boolean {
  if (!update || update.planId !== planId) return false;
  if (update.deleted) return true;
  const plan = update.run?.plan;
  if (!plan) return false;
  if ((plan.brief ?? '') !== (shown?.brief ?? '')) return true;
  if ((plan.initiativeStatus ?? '') !== (shown?.initiativeStatus ?? '')) return true;
  return plan.status === 'closed';
}

/** Is this update the plan being deleted (navigate back to the list)? */
export function isDeletedUpdate(update: PlanRunUpdateLike | null | undefined, planId: string): boolean {
  return !!update && update.planId === planId && (update.deleted || update.run?.plan?.status === 'closed');
}

/** Poll only when the push channel is down AND the page is visible. */
export function pollEnabled(relayConnected: boolean, pageVisible: boolean): boolean {
  return !relayConnected && pageVisible;
}

/** A chat turn just finished (streaming went true → false): refetch messages (they carry tool_calls). */
export function turnCompleted(wasStreaming: boolean, isStreaming: boolean): boolean {
  return wasStreaming && !isStreaming;
}

/** A poll tick found a newer version than the one shown. */
export function hasNewerVersion(latestListed: number | null | undefined, shownVersion: number | null | undefined): boolean {
  return (latestListed ?? 0) > (shownVersion ?? 0);
}
