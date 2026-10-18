// Pure, framework-free decision layer for the per-initiative console tab shell (Overhaul P9 /
// phase P1). Kept Angular/Ionic/DOM-free so the spec runs under the plugin-less web vitest, mirroring
// the shipped `terminal-transcript-tab.ts` / `console-logic.ts` pure-seam pattern (D6). The `PaneTab`
// tab-id union + its default/lookup helper are REUSED from `terminal-transcript-tab.ts` — this module
// deliberately does not define a second tab type. Types are `import type` only (erased at build), so
// no `@johnnyone/ui` / Angular runtime dependency leaks into the plugin-less vitest.
import type { AgentPlan } from '../../../../../ui/src/services/johnny-api.service';
import { DEFAULT_PANE_TAB, paneTabOf, type PaneTab } from './terminal-transcript-tab';

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

/** Persist may remember `tasks`/`checks` after a failed bundle — show a visible tab. */
export function resolvedPaneTab(active: PaneTab, visible: readonly PaneTab[]): PaneTab {
  return visible.includes(active) ? active : DEFAULT_PANE_TAB;
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
  const base: PaneTab[] = ['raw', 'plan'];
  if (!isLocalSmall(init?.executorConfig)) {
    base.push('diff');
    return base;
  }
  const stage = (init?.initiativeStatus ?? init?.runType ?? '').toLowerCase();
  const planning = stage === 'planning';
  if (planning || hasPlanCheck || hasPreflight) base.push('checks');
  if (hasTaskRun) base.push('tasks');
  base.push('diff');
  return base;
}

/**
 * The strip the console actually renders: whatever `visibleConsoleTabs` already decided, with the
 * arena APPENDED. A separate function on purpose — `visibleConsoleTabs` is what five shipped specs
 * assert exact arrays against, and "Arena is an added tab, never a replacement" is better served by
 * leaving that function's answer alone than by editing the tests that hold it. This is the ONE place
 * the arena joins the strip, so `resolvedPaneTab` still sees it as visible and a persisted `arena`
 * selection is not bounced back to Raw.
 */
export function consoleTabsWithArena(
  init: AgentPlan | null | undefined,
  hasTaskRun: boolean,
  hasPlanCheck: boolean,
  hasPreflight: boolean,
): PaneTab[] {
  return [...visibleConsoleTabs(init, hasTaskRun, hasPlanCheck, hasPreflight), 'arena'];
}

/**
 * Whether the Raw tab should render the inline attach affordance instead of a live terminal screen:
 * true when the initiative has no primary session yet, or it has one but no attached screen for it.
 * `hasScreen` = the page already has an attached `terminalScreens()[primarySessionId]`.
 */
export function rawAttachNeeded(primarySessionId: string | null | undefined, hasScreen: boolean): boolean {
  return !primarySessionId || !hasScreen;
}

/**
 * Which tabs need the initiative's `planRun` loaded.
 *
 * ONE home for a condition the console used to spell out inline. The Plan tab has always needed it;
 * the ARENA tab needs the same rows (its entry points are per phase, and it is a tab a reader can
 * open first) and did not get them — measured in Chrome: the tab said "This initiative has no
 * phases yet" on an initiative with three, until the Plan tab had been opened once. A function
 * rather than a condition in an effect, because that condition is otherwise untestable: the console
 * page uses `templateUrl` and cannot be mounted under this project's vitest.
 */
export function tabNeedsPlanRun(tab: PaneTab): boolean {
  return tab === 'plan' || tab === 'arena';
}
