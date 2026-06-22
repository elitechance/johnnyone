// Pure, Angular-free helpers for reconciling persisted Terminal state against the
// authoritative active-session list. Kept out of `terminal.page.ts` so they unit-test
// in plain jsdom without a TestBed/Angular harness (web/vitest.config.ts intentionally
// omits @analogjs/vitest-angular). See the plan: terminal-stale-session-reconcile.

export interface PaneLayout {
  x: number;
  y: number;
  width: number;
  height: number;
}

export interface PersistedTerminalWorkspaceState {
  desktopLayouts: Record<string, PaneLayout>;
  mobileLayouts: Record<string, PaneLayout>;
  closedPaneIds: string[];
}

/**
 * Return a copy of `record` keeping only keys present in `activeIds`, plus whether
 * anything was dropped. Never mutates the input.
 */
export function pruneRecordByActiveIds<T>(
  record: Record<string, T>,
  activeIds: ReadonlySet<string>,
): { next: Record<string, T>; changed: boolean } {
  const next: Record<string, T> = {};
  let changed = false;
  for (const [id, value] of Object.entries(record)) {
    if (activeIds.has(id)) {
      next[id] = value;
    } else {
      changed = true;
    }
  }
  return { next, changed };
}

/**
 * Filter all three persisted workspace fields — `desktopLayouts`, `mobileLayouts`,
 * and `closedPaneIds` — against the active id set. `changed` is true iff any entry was
 * dropped from any field. When the cleaned state holds no layouts and no closed panes,
 * `next` is `null` so the caller can `removeItem` (matching the page's
 * `readPersistedWorkspaceState` "all-empty ⇒ null" semantics) rather than persisting a
 * hollow `{}` blob. A `null` input returns `{ next: null, changed: false }`.
 */
export function reconcilePersistedWorkspaceState(
  raw: PersistedTerminalWorkspaceState | null,
  activeIds: ReadonlySet<string>,
): { next: PersistedTerminalWorkspaceState | null; changed: boolean } {
  if (!raw) return { next: null, changed: false };

  const desktop = pruneRecordByActiveIds(raw.desktopLayouts ?? {}, activeIds);
  const mobile = pruneRecordByActiveIds(raw.mobileLayouts ?? {}, activeIds);

  const closedPaneIds: string[] = [];
  let closedChanged = false;
  for (const id of raw.closedPaneIds ?? []) {
    if (activeIds.has(id)) {
      closedPaneIds.push(id);
    } else {
      closedChanged = true;
    }
  }

  const changed = desktop.changed || mobile.changed || closedChanged;

  const isEmpty =
    Object.keys(desktop.next).length === 0 &&
    Object.keys(mobile.next).length === 0 &&
    closedPaneIds.length === 0;

  if (isEmpty) {
    return { next: null, changed };
  }

  return {
    next: {
      desktopLayouts: desktop.next,
      mobileLayouts: mobile.next,
      closedPaneIds,
    },
    changed,
  };
}

/**
 * Behavior-preserving extraction of the session-selection decision inlined in
 * `TerminalPage.loadSessions` (terminal.page.ts:473-495). Given the active-only session
 * list, decide which session id should become current:
 *   - a `targetId` that is active wins (deep-link / explicit target);
 *   - else an existing `currentId` is kept only if still active, otherwise `null`
 *     (a dead/stale current must NOT stay pinned — the caller clears to empty);
 *   - else the first active session, or `null` when the list is empty.
 *
 * Returning `null` collapses "clear the dead current" and "nothing to select" into one
 * value; both end states are "no current session", and the caller handles `null` by
 * clearing `currentSession`/`messages`.
 */
export function chooseSessionToSelect(
  activeSessions: { id: string }[],
  opts: { targetId?: string | null; currentId?: string | null },
): string | null {
  const activeIds = new Set(activeSessions.map((s) => s.id));

  if (opts.targetId && activeIds.has(opts.targetId)) {
    return opts.targetId;
  }

  if (opts.currentId) {
    return activeIds.has(opts.currentId) ? opts.currentId : null;
  }

  return activeSessions.length > 0 ? activeSessions[0].id : null;
}
