// Pure, framework-free decision layer for the per-initiative console tab shell (Overhaul P9 /
// phase P1). Kept Angular/Ionic/DOM-free so the spec runs under the plugin-less web vitest, mirroring
// the shipped `terminal-transcript-tab.ts` / `console-logic.ts` pure-seam pattern (D6). The `PaneTab`
// tab-id union + its default/lookup helper are REUSED from `terminal-transcript-tab.ts` — this module
// deliberately does not define a second tab type. Types are `import type` only (erased at build), so
// no `@johnnyone/ui` / Angular runtime dependency leaks into the plugin-less vitest.
import type { AgentPlan } from '../../../../../ui/src/services/johnny-api.service';
import { paneTabOf, type PaneTab } from './terminal-transcript-tab';

export { DEFAULT_PANE_TAB, type PaneTab } from './terminal-transcript-tab';

/**
 * The session whose Transcript / Raw terminal / Diff the initiative's tabs display (D2; see
 * `artifacts/tab-dataflow.md`). Precedence: the development worker, then the reviewer, then the
 * briefing chat session. Returns `null` for a null/undefined initiative or one with no session yet
 * (e.g. a freshly-selected initiative with no live terminal).
 */
export function resolvePrimarySessionId(init: AgentPlan | null | undefined): string | null {
  return init?.workerSessionId ?? init?.reviewerSessionId ?? init?.briefingSessionId ?? null;
}

/**
 * Active tab for an initiative, defaulting to the Transcript view. Kept a DISTINCT map from the
 * page's session-keyed `paneTabs` (D2) but reuses `paneTabOf` so the default/lookup rule stays in one
 * place.
 */
export function initiativeTabOf(tabs: Record<string, PaneTab>, initiativeId: string): PaneTab {
  return paneTabOf(tabs, initiativeId);
}

export function isLocalSmall(executorConfig: string | null | undefined): boolean {
  if (!executorConfig) return false;
  try {
    const parsed = JSON.parse(executorConfig) as { mode?: string };
    return parsed?.mode === 'local-small';
  } catch {
    return false;
  }
}

/** Single function the tab strip uses (05-08). */
export function visibleConsoleTabs(
  init: AgentPlan | null | undefined,
  hasTaskRun: boolean,
  hasPlanCheck: boolean,
  hasPreflight: boolean,
): PaneTab[] {
  const base: PaneTab[] = ['raw', 'plan', 'diff'];
  if (!isLocalSmall(init?.executorConfig)) return base;
  const stage = (init?.initiativeStatus ?? init?.runType ?? '').toLowerCase();
  const planning = stage === 'planning';
  if (planning || hasPlanCheck || hasPreflight) base.push('checks');
  if (hasTaskRun) base.push('tasks');
  return base;
}

/**
 * Whether the Raw tab should render the inline attach affordance instead of a live terminal screen:
 * true when the initiative has no primary session yet, or it has one but no attached screen for it.
 * `hasScreen` = the page already has an attached `terminalScreens()[primarySessionId]`.
 */
export function rawAttachNeeded(primarySessionId: string | null | undefined, hasScreen: boolean): boolean {
  return !primarySessionId || !hasScreen;
}
