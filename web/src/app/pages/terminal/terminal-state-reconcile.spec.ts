import { describe, it, expect } from 'vitest';

import {
  pruneRecordByActiveIds,
  reconcilePersistedWorkspaceState,
  chooseSessionToSelect,
  type PersistedTerminalWorkspaceState,
} from './terminal-state-reconcile';

const layout = (x = 0) => ({ x, y: 0, width: 100, height: 100 });

describe('pruneRecordByActiveIds', () => {
  it('drops only non-active keys, reports changed, and does not mutate input', () => {
    const input = { aaa: 1, bbb: 2, ccc: 3 };
    const { next, changed } = pruneRecordByActiveIds(input, new Set(['aaa', 'ccc']));

    expect(next).toEqual({ aaa: 1, ccc: 3 });
    expect(changed).toBe(true);
    expect(input).toEqual({ aaa: 1, bbb: 2, ccc: 3 }); // input untouched
    expect(next).not.toBe(input); // new object
  });

  it('reports changed:false when nothing is dropped', () => {
    const { next, changed } = pruneRecordByActiveIds({ aaa: 1 }, new Set(['aaa']));
    expect(next).toEqual({ aaa: 1 });
    expect(changed).toBe(false);
  });
});

describe('reconcilePersistedWorkspaceState', () => {
  it('filters all three fields against active ids and flags changed on any drop', () => {
    const raw: PersistedTerminalWorkspaceState = {
      desktopLayouts: { aaa: layout(1), bbb: layout(2) },
      mobileLayouts: { bbb: layout(3) },
      closedPaneIds: ['aaa', 'bbb'],
    };
    const { next, changed } = reconcilePersistedWorkspaceState(raw, new Set(['aaa']));

    expect(changed).toBe(true);
    expect(next).toEqual({
      desktopLayouts: { aaa: layout(1) },
      mobileLayouts: {},
      closedPaneIds: ['aaa'],
    });
  });

  it('returns changed:false when every entry is active', () => {
    const raw: PersistedTerminalWorkspaceState = {
      desktopLayouts: { aaa: layout() },
      mobileLayouts: { aaa: layout() },
      closedPaneIds: ['aaa'],
    };
    const { next, changed } = reconcilePersistedWorkspaceState(raw, new Set(['aaa']));
    expect(changed).toBe(false);
    expect(next).toEqual(raw);
  });

  it('returns {next:null, changed:false} for null input', () => {
    expect(reconcilePersistedWorkspaceState(null, new Set(['aaa']))).toEqual({
      next: null,
      changed: false,
    });
  });

  // Complaint repro — half (a): a dead id (bf949413) seeded in all three stores is
  // dropped everywhere while the live session (4d25e49a) survives intact.
  it('complaint repro (a): drops bf949413 from all three stores, keeps 4d25e49a', () => {
    const raw: PersistedTerminalWorkspaceState = {
      desktopLayouts: { bf949413: layout(1), '4d25e49a': layout(2) },
      mobileLayouts: { bf949413: layout(3) },
      closedPaneIds: ['bf949413'],
    };
    const { next, changed } = reconcilePersistedWorkspaceState(raw, new Set(['4d25e49a']));

    expect(changed).toBe(true);
    const serialized = JSON.stringify(next);
    expect(serialized).not.toContain('bf949413');
    expect(next?.desktopLayouts['4d25e49a']).toEqual(layout(2));
    expect(next?.desktopLayouts.bf949413).toBeUndefined();
    expect(next?.mobileLayouts).toEqual({});
    expect(next?.closedPaneIds).toEqual([]);
  });

  it('all-stale input with empty active set cleans to null (so caller can removeItem)', () => {
    const raw: PersistedTerminalWorkspaceState = {
      desktopLayouts: { bf949413: layout() },
      mobileLayouts: { bf949413: layout() },
      closedPaneIds: ['bf949413'],
    };
    const { next, changed } = reconcilePersistedWorkspaceState(raw, new Set());
    expect(changed).toBe(true);
    expect(next).toBeNull();
  });
});

describe('chooseSessionToSelect (complaint half b — the live session appears)', () => {
  const LIVE = '4d25e49a';
  const DEAD = 'bf949413';
  const OTHER = '9c0fdead';

  it('with no current/target, selects the live active session after the dead pin is pruned', () => {
    expect(chooseSessionToSelect([{ id: LIVE }], {})).toBe(LIVE);
  });

  it('a dead currentId does NOT stay pinned — returns null so the caller clears', () => {
    expect(chooseSessionToSelect([{ id: LIVE }], { currentId: DEAD })).toBeNull();
  });

  it('a stale (dead) targetId / deep-link is ignored and the live session still appears', () => {
    expect(chooseSessionToSelect([{ id: LIVE }], { targetId: DEAD })).toBe(LIVE);
  });

  it('honours a valid current and a valid target (active sessions unaffected)', () => {
    expect(chooseSessionToSelect([{ id: LIVE }, { id: OTHER }], { currentId: LIVE })).toBe(LIVE);
    expect(chooseSessionToSelect([{ id: LIVE }, { id: OTHER }], { targetId: OTHER })).toBe(OTHER);
  });

  it('a valid target wins over a different valid current', () => {
    expect(
      chooseSessionToSelect([{ id: LIVE }, { id: OTHER }], { targetId: OTHER, currentId: LIVE }),
    ).toBe(OTHER);
  });

  it('empty active list returns null (no session, empty state)', () => {
    expect(chooseSessionToSelect([], { currentId: DEAD })).toBeNull();
    expect(chooseSessionToSelect([], {})).toBeNull();
  });
});
