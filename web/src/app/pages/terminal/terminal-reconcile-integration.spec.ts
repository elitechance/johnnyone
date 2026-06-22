// Cross-task integration repro (phase-00 validation, task 03). NOT part of task 01/02's
// own unit specs — this proves the two halves COMPOSE: seeding BOTH persisted stores
// exactly as the user's brief observed (dead `bf949413` pinned, live `4d25e49a` hidden),
// then running the reconcile primitives the way `loadSessions` wires them, must leave
// ZERO `bf949413`-style references reachable on load, with `4d25e49a` selected.
//
// The live `TerminalPage.loadSessions` drive is not runnable (web/vitest.config.ts omits
// @analogjs/vitest-angular, no TestBed in src/app). Per the phase test policy + task-03
// fallback clause, this exercises the same primitives the page calls — the persisted-
// workspace reconcile, the screen-cache `retainOnly`, and `chooseSessionToSelect` — over
// real localStorage (jsdom), which is the grounded end-to-end proof of the complaint.
import { describe, it, expect, beforeEach, afterEach } from 'vitest';
import '@angular/compiler';

import { TerminalScreenCacheService } from '../../services/terminal-screen-cache.service';
import {
  reconcilePersistedWorkspaceState,
  chooseSessionToSelect,
  type PersistedTerminalWorkspaceState,
} from './terminal-state-reconcile';
import type { TerminalScreen } from '@johnnyone/ui';

const WORKSPACE_KEY = 'johnnyone_terminal_pane_workspace';
const CACHE_KEY = 'johnnyone_terminal_screen_cache';
const DEAD = 'bf949413';
const LIVE = '4d25e49a';

const layout = (x = 0) => ({ x, y: 0, width: 200, height: 200 });

function screen(sessionId: string): TerminalScreen {
  return {
    sessionId,
    paneId: `pane-${sessionId}`,
    cursor: 0,
    content: `cached scrollback for ${sessionId}`,
    cursorX: 0,
    cursorY: 0,
    historyLines: 1,
    rows: 24,
    cols: 80,
    status: sessionId === DEAD ? 'offline' : 'running',
  };
}

/** Mirror of the persisted-blob rewrite branch in TerminalPage.reconcilePersistedTerminalState. */
function reconcileWorkspaceStore(activeIds: Set<string>): void {
  const raw = localStorage.getItem(WORKSPACE_KEY);
  const parsed = raw ? (JSON.parse(raw) as PersistedTerminalWorkspaceState) : null;
  const { next, changed } = reconcilePersistedWorkspaceState(parsed, activeIds);
  if (changed) {
    if (next) localStorage.setItem(WORKSPACE_KEY, JSON.stringify(next));
    else localStorage.removeItem(WORKSPACE_KEY);
  }
}

describe('phase-00 cross-task integration — dead session pruned, live session appears', () => {
  let cache: TerminalScreenCacheService;

  beforeEach(() => {
    localStorage.clear();
    cache = new TerminalScreenCacheService();
  });
  afterEach(() => localStorage.clear());

  it('the brief scenario: bf949413 pinned in BOTH stores + 4d25e49a active → dead id gone everywhere, live selected', () => {
    // Seed exactly as the brief observed.
    localStorage.setItem(
      WORKSPACE_KEY,
      JSON.stringify({
        desktopLayouts: { [DEAD]: layout(1), [LIVE]: layout(2) },
        mobileLayouts: { [DEAD]: layout(3) },
        closedPaneIds: [DEAD],
      }),
    );
    localStorage.setItem(
      CACHE_KEY,
      JSON.stringify({
        version: 1,
        sessions: {
          [DEAD]: { screen: screen(DEAD), savedAt: Date.now(), contentLength: 10 },
          [LIVE]: { screen: screen(LIVE), savedAt: Date.now(), contentLength: 10 },
        },
      }),
    );

    // Active list from listSessions('active') — only the live session.
    const activeSessions = [{ id: LIVE }];
    const activeIds = new Set(activeSessions.map((s) => s.id));

    // Run the reconcile primitives the way loadSessions wires them (tasks 01 + 02).
    reconcileWorkspaceStore(activeIds);
    cache.retainOnly(activeIds);

    // ── Assert: zero bf949413 references reachable on load, anywhere. ──
    const wsRaw = localStorage.getItem(WORKSPACE_KEY) ?? '';
    const cacheRaw = localStorage.getItem(CACHE_KEY) ?? '';
    expect(wsRaw).not.toContain(DEAD);
    expect(cacheRaw).not.toContain(DEAD);

    // Live session survives in both stores.
    const ws = JSON.parse(wsRaw) as PersistedTerminalWorkspaceState;
    expect(ws.desktopLayouts[LIVE]).toEqual(layout(2));
    expect(ws.mobileLayouts).toEqual({});
    expect(ws.closedPaneIds).toEqual([]);
    expect(cache.get(LIVE)).not.toBeNull();
    expect(cache.get(DEAD)).toBeNull();

    // Half (b): the live active session is the one selected; no dead pin survives even if it
    // was the previous current selection.
    // A dead currentId must NOT keep the dead pane selected — caller clears, then the live
    // session is the only thing left to show.
    expect(chooseSessionToSelect(activeSessions, { currentId: DEAD })).toBeNull();
    expect(chooseSessionToSelect(activeSessions, {})).toBe(LIVE);
    // A stale dead deep-link is ignored; the live session still appears.
    expect(chooseSessionToSelect(activeSessions, { targetId: DEAD })).toBe(LIVE);
  });

  it('empty-state variant: dead id is the ONLY persisted session, no active session → both stores cleared, selection null', () => {
    localStorage.setItem(
      WORKSPACE_KEY,
      JSON.stringify({ desktopLayouts: { [DEAD]: layout() }, mobileLayouts: {}, closedPaneIds: [DEAD] }),
    );
    localStorage.setItem(
      CACHE_KEY,
      JSON.stringify({ version: 1, sessions: { [DEAD]: { screen: screen(DEAD), savedAt: Date.now(), contentLength: 10 } } }),
    );

    const activeIds = new Set<string>();
    reconcileWorkspaceStore(activeIds);
    cache.retainOnly(activeIds);

    // Fully stale workspace blob is removed (not left as a hollow {}); cache emptied.
    expect(localStorage.getItem(WORKSPACE_KEY)).toBeNull();
    expect(cache.get(DEAD)).toBeNull();
    // No active session → empty state (selection null), never a frozen dead pane.
    expect(chooseSessionToSelect([], { currentId: DEAD })).toBeNull();
  });
});
