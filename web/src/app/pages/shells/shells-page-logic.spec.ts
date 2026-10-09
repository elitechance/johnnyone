import { describe, it, expect } from 'vitest';

// The `/shells` DECISION logic is extracted into an Angular/Ionic-free module (overhaul P6, D4/D6) so it
// can be unit-tested directly under the plugin-less web vitest — the real `ShellsPage` pulls in Ionic +
// the Phase-01 launcher. The page delegates its filter/partition/dedupe/label/rel-time/open decisions to
// exactly these functions. This spec pins acceptance A.1–A.6 from the phase overview.
import {
  isShellSession,
  partitionShells,
  attachableTmux,
  shellSessionLabel,
  formatRelTime,
  openIntent,
} from './shells-page-logic';
import { plainTerminalRoute, attachTmuxInput } from '../../components/launcher-menu/launcher-logic';

// A minimal `AiSession`-shaped factory — only the fields the pure logic reads matter here.
function session(over: Partial<{
  id: string;
  title: string;
  provider: string;
  attachedTmux: boolean;
  createdAt: string;
  updatedAt: string;
}>): any {
  // Use `in` (not `??`) for the timestamp fields so a test can pass an explicit `undefined` to exercise
  // the `updatedAt ?? createdAt` fallback without the factory silently restoring the default.
  return {
    id: over.id ?? 'id',
    title: over.title ?? '',
    provider: over.provider ?? 'claude_code',
    model: '',
    status: 'active',
    totalInputTokens: 0,
    totalOutputTokens: 0,
    totalCostCents: 0,
    createdAt: 'createdAt' in over ? over.createdAt : '2026-07-03T10:00:00Z',
    updatedAt: 'updatedAt' in over ? over.updatedAt : '2026-07-03T10:00:00Z',
    attachedTmux: over.attachedTmux ?? false,
  };
}

const tmux = (name: string, windows = 1, attached = false) => ({ name, windows, attached });

describe('shells page — pure logic', () => {
  // A.1 — shell predicate
  describe('isShellSession', () => {
    it('is true for a raw shell provider', () => {
      expect(isShellSession({ provider: 'shell', attachedTmux: false })).toBe(true);
    });

    it('is true for an attached tmux session even with a non-shell provider', () => {
      expect(isShellSession({ provider: 'codex', attachedTmux: true })).toBe(true);
    });

    it('excludes an agent session that is not attached (QA boundary)', () => {
      expect(isShellSession({ provider: 'claude_code', attachedTmux: false })).toBe(false);
    });
  });

  // A.2 — partition + sort (newest-first) + agent drop
  describe('partitionShells', () => {
    it('keeps only shell/attached rows, newest-first, dropping agent sessions', () => {
      const older = session({ id: 'a', provider: 'shell', updatedAt: '2026-07-03T09:00:00Z' });
      const newer = session({ id: 'b', provider: 'shell', updatedAt: '2026-07-03T11:00:00Z' });
      const attached = session({ id: 'c', provider: 'codex', attachedTmux: true, updatedAt: '2026-07-03T10:00:00Z' });
      const agent = session({ id: 'd', provider: 'claude_code', updatedAt: '2026-07-03T12:00:00Z' });

      const result = partitionShells([older, newer, attached, agent]);
      expect(result.map((s) => s.id)).toEqual(['b', 'c', 'a']); // agent 'd' dropped; sorted desc
    });

    it('falls back to createdAt when updatedAt is absent', () => {
      const a = session({ id: 'a', provider: 'shell', updatedAt: undefined as any, createdAt: '2026-07-03T08:00:00Z' });
      const b = session({ id: 'b', provider: 'shell', updatedAt: undefined as any, createdAt: '2026-07-03T09:00:00Z' });
      expect(partitionShells([a, b]).map((s) => s.id)).toEqual(['b', 'a']);
    });
  });

  // A.3 — attachable dedupe
  describe('attachableTmux', () => {
    it('removes a pane already attached by one of our sessions (join on title === name)', () => {
      const sessions = [session({ id: 'x', provider: 'codex', attachedTmux: true, title: 'kloo' })];
      const result = attachableTmux([tmux('kloo', 2), tmux('llm-app', 3)], sessions);
      expect(result.map((t) => t.name)).toEqual(['llm-app']);
    });

    it('returns every pane when no session is attached', () => {
      const result = attachableTmux([tmux('kloo'), tmux('llm-app')], []);
      expect(result.map((t) => t.name)).toEqual(['kloo', 'llm-app']);
    });

    it('does not dedupe against a non-attached session whose title collides', () => {
      const sessions = [session({ id: 'x', provider: 'shell', attachedTmux: false, title: 'kloo' })];
      expect(attachableTmux([tmux('kloo')], sessions).map((t) => t.name)).toEqual(['kloo']);
    });
  });

  // A.4 — label
  describe('shellSessionLabel', () => {
    it('defaults an empty title to "Shell" and tags a raw shell', () => {
      expect(shellSessionLabel({ title: '', attachedTmux: false })).toEqual({ title: 'Shell', tag: 'shell' });
    });

    it('keeps the title and tags an attached tmux pane', () => {
      expect(shellSessionLabel({ title: 'kloo', attachedTmux: true })).toEqual({ title: 'kloo', tag: 'tmux' });
    });

    it('treats a whitespace-only title as empty', () => {
      expect(shellSessionLabel({ title: '   ', attachedTmux: false })).toEqual({ title: 'Shell', tag: 'shell' });
    });
  });

  // A.5 — rel time (deterministic)
  describe('formatRelTime', () => {
    it('formats minutes', () => {
      expect(formatRelTime('2026-07-03T11:48:00Z', '2026-07-03T12:00:00Z')).toBe('12m ago');
    });

    it('formats "just now" under 60s', () => {
      expect(formatRelTime('2026-07-03T11:59:30Z', '2026-07-03T12:00:00Z')).toBe('just now');
    });

    it('formats hours', () => {
      expect(formatRelTime('2026-07-03T09:00:00Z', '2026-07-03T12:00:00Z')).toBe('3h ago');
    });

    it('formats days', () => {
      expect(formatRelTime('2026-07-01T12:00:00Z', '2026-07-03T12:00:00Z')).toBe('2d ago');
    });

    it('returns empty string on invalid input', () => {
      expect(formatRelTime('', '2026-07-03T12:00:00Z')).toBe('');
      expect(formatRelTime('2026-07-03T12:00:00Z', 'nonsense')).toBe('');
    });
  });

  // A.6 — open intent is the plain-shell route (shells always open plain, P4)
  describe('openIntent', () => {
    it('opens the shell on its own /shells/:sessionId destination (not the initiative console)', () => {
      expect(openIntent('abc')).toEqual({
        path: '/shells/abc',
        queryParams: {},
      });
    });

    it('is the same function as plainTerminalRoute (reused, not re-derived)', () => {
      expect(openIntent).toBe(plainTerminalRoute);
    });
  });

  // P4 — end-to-end: a pane attached via attachTmuxInput leaves the attachable list (finding #3a fix)
  describe('attach-then-dedupe (finding #3a)', () => {
    it('an attached session created from attachTmuxInput carries title===name, so its pane drops out', () => {
      const input = attachTmuxInput('kloo'); // { tmuxSessionName:'kloo', title:'kloo' }
      const attached = session({ id: 'x', provider: 'codex', attachedTmux: true, title: input.title });
      const result = attachableTmux([tmux('kloo', 2), tmux('llm-app', 3)], [attached]);
      expect(result.map((t) => t.name)).toEqual(['llm-app']); // 'kloo' no longer attachable
    });
  });
});

// ── Clear (archive-all) decision logic ───────────────────────────────────────────────────────────
// The /shells "Clear" affordance ARCHIVES rows; it never deletes them (see the comment on the
// archive call in shells.page.ts — `deleteAiSession` would permanently break `reportAgentResult`
// for that session id, and one of the live rows is the channel the console reports through).
// Everything decidable about it lives here: which ids to target, the confirm copy, the summary.
import { clearTargetIds, clearConfirmCopy, clearResultMessage } from './shells-page-logic';

describe('shells page — clear (archive) logic', () => {
  describe('clearTargetIds', () => {
    it('targets every shell row, in list order', () => {
      const rows = [
        session({ id: 'a', provider: 'shell' }),
        session({ id: 'b', provider: 'codex', attachedTmux: true }),
      ];
      expect(clearTargetIds(rows)).toEqual(['a', 'b']);
    });

    it('is empty for an empty list (no work to do)', () => {
      expect(clearTargetIds([])).toEqual([]);
    });

    it('never targets an attachable tmux PANE — panes have no session id to archive', () => {
      // The clear acts on `shells()` only. A `TmuxSession` has no `id`, so feeding the attachable
      // group in could only ever produce junk; the contract is that callers pass shell rows.
      const rows = [session({ id: 'a', provider: 'shell' })];
      const ids = clearTargetIds(rows);
      expect(ids).toEqual(['a']);
      expect(ids).not.toContain('kloo'); // the pane name is not an id and is not reachable here
    });

    it('drops an agent session that is not a shell (same filter as the list)', () => {
      const rows = [
        session({ id: 'a', provider: 'shell' }),
        session({ id: 'd', provider: 'claude_code', attachedTmux: false }),
      ];
      expect(clearTargetIds(rows)).toEqual(['a']);
    });
  });

  // F1 — the copy branches on LIVENESS, not just on `attachedTmux`. A row can be attached to a tmux
  // session that has since died (measured: `kord-jar` and `kloo-imp` are DEAD while `j1`/`kloo`/
  // `kloo-cli`/`kord` are live). `list_external_tmux_sessions` (`terminal.rs:1355-1386`) shells out to
  // `tmux list-sessions`, so it can only ever return LIVE panes — a dead one cannot come back under
  // "Attachable", and promising that it will is a lie. The live-pane names are a PARAMETER so this
  // stays pure and the dead case is testable (the earlier version assumed liveness by handing
  // `attachableTmux` an array that already contained the names).
  describe('clearConfirmCopy', () => {
    it('is singular for one LIVE attached shell and promises it comes back', () => {
      const copy = clearConfirmCopy([session({ id: 'a', attachedTmux: true, title: 'kloo' })], ['kloo']);
      expect(copy.header).toBe('Clear 1 shell?');
      expect(copy.confirmText).toBe('Clear');
      expect(copy.message).toContain('1 shell');
      expect(copy.message).toContain('keeps running');
      expect(copy.message).toContain('Attachable tmux sessions');
      expect(copy.message).toContain('nothing is lost');
      expect(copy.message.toLowerCase()).not.toContain('delete');
    });

    it('is plural for N live attached shells and names the count', () => {
      const rows = Array.from({ length: 6 }, (_, i) =>
        session({ id: `s${i}`, attachedTmux: true, title: `t${i}` }),
      );
      const copy = clearConfirmCopy(rows, rows.map((r) => r.title));
      expect(copy.header).toBe('Clear 6 shells?');
      expect(copy.message).toContain('6 shells');
      expect(copy.message).toContain('keep running');
      expect(copy.message).toContain('Attachable tmux sessions');
      expect(copy.message.toLowerCase()).not.toContain('delete');
    });

    it('does NOT claim a dead pane comes back — it just leaves the list (F1)', () => {
      const copy = clearConfirmCopy(
        [session({ id: 'a', attachedTmux: true, title: 'kord-jar' })],
        ['j1', 'kloo'], // `kord-jar` is not live
      );
      expect(copy.message).toContain('already gone');
      expect(copy.message).toContain('leave the list');
      expect(copy.message).not.toContain('Attachable tmux sessions');
      expect(copy.message).not.toContain('nothing is lost');
      expect(copy.message).not.toContain('keeps running');
    });

    it('splits live from dead attached rows — the real six-row list (F1)', () => {
      const live = ['j1', 'kloo', 'kloo-cli', 'kord'];
      const rows = [...live, 'kord-jar', 'kloo-imp'].map((t, i) =>
        session({ id: `s${i}`, provider: 'codex', attachedTmux: true, title: t }),
      );
      const copy = clearConfirmCopy(rows, live);
      expect(copy.header).toBe('Clear 6 shells?');
      expect(copy.message).toContain('4 keep running');
      expect(copy.message).toContain('Attachable tmux sessions');
      expect(copy.message).toContain('2 are already gone');
      expect(copy.message).toContain('leave the list');
      // The blanket promise is gone: two of these are NOT coming back.
      expect(copy.message).not.toContain('nothing is lost');
      expect(copy.message.toLowerCase()).not.toContain('delete');
    });

    it('treats an attached row with no live pane as gone even when the live list is empty', () => {
      const copy = clearConfirmCopy([session({ id: 'a', attachedTmux: true, title: 'kloo' })], []);
      expect(copy.message).toContain('already gone');
      expect(copy.message).not.toContain('Attachable tmux sessions');
    });

    it('says a PLAIN shell closes — it has no external tmux to come back as', () => {
      const copy = clearConfirmCopy([session({ id: 'a', provider: 'shell', attachedTmux: false })], ['kloo']);
      expect(copy.header).toBe('Clear 1 shell?');
      expect(copy.message).toContain('closes');
      expect(copy.message).not.toContain('Attachable tmux sessions');
      expect(copy.message.toLowerCase()).not.toContain('delete');
    });

    // F3 — the transcript survives the archive (`list_session_reports` / `get_ai_session` have no
    // status filter) but NO list surface shows archived sessions: `/shells` and the terminal tab bar
    // both ask `listSessions('active')`. So it is reachable BY LINK, not still in front of you, and
    // the copy must not imply otherwise.
    it('says a kept transcript is reachable by link, not merely "kept" (F3)', () => {
      const copy = clearConfirmCopy([session({ id: 'a', provider: 'shell', attachedTmux: false })], []);
      expect(copy.message).toContain('reachable by link');
    });

    it('describes all three groups when the list is mixed', () => {
      const rows = [
        session({ id: 'a', provider: 'shell', attachedTmux: false }),
        session({ id: 'b', provider: 'codex', attachedTmux: true, title: 'kloo' }),
        session({ id: 'c', provider: 'codex', attachedTmux: true, title: 'kord-jar' }),
      ];
      const copy = clearConfirmCopy(rows, ['kloo']);
      expect(copy.header).toBe('Clear 3 shells?');
      expect(copy.message).toContain('1 keeps running');
      expect(copy.message).toContain('1 is already gone');
      expect(copy.message).toContain('1 plain shell closes');
      expect(copy.message).toContain('reachable by link');
    });

    it('yields no work for an empty list', () => {
      expect(clearConfirmCopy([], ['kloo']).header).toBe('Nothing to clear');
      expect(clearTargetIds([])).toHaveLength(0);
    });
  });

  describe('clearResultMessage', () => {
    it('is null when nothing failed (no error banner on a clean clear)', () => {
      expect(clearResultMessage(6, 0)).toBeNull();
      expect(clearResultMessage(0, 0)).toBeNull();
    });

    it('summarises a partial failure honestly', () => {
      expect(clearResultMessage(6, 2)).toBe('Cleared 4 of 6 shells — 2 could not be cleared.');
    });

    it('uses singular wording for a single surviving/failing row', () => {
      expect(clearResultMessage(2, 1)).toBe('Cleared 1 of 2 shells — 1 could not be cleared.');
      expect(clearResultMessage(1, 1)).toBe('The shell could not be cleared.');
    });

    it('reports a total failure without a misleading "cleared 0"', () => {
      expect(clearResultMessage(6, 6)).toBe('No shells could be cleared — all 6 failed.');
    });
  });

  // The "nothing is lost" promise, asserted rather than assumed: once the archived session rows are
  // gone from `listSessions('active')`, `attachableTmux` hands the very same panes back.
  describe('clear → the panes come back as attachable', () => {
    it('returns a pane again once its session is no longer in the list', () => {
      const attached = session({ id: 'x', provider: 'codex', attachedTmux: true, title: 'kloo' });
      const panes = [tmux('kloo', 2), tmux('j1', 1)];

      // Before the clear: `kloo` is attached, so only `j1` is attachable.
      expect(attachableTmux(panes, [attached]).map((t) => t.name)).toEqual(['j1']);

      // After the clear + refresh the archived row drops out of the active list entirely…
      const afterClear: any[] = [];
      expect(clearTargetIds([attached])).toEqual(['x']);
      // …and the copy only made the promise because `kloo` was in the LIVE pane list.
      expect(clearConfirmCopy([attached], ['kloo']).message).toContain('Attachable tmux sessions');
      // …and BOTH panes are offered again — the tmux sessions were never killed.
      expect(attachableTmux(panes, afterClear).map((t) => t.name)).toEqual(['kloo', 'j1']);
    });
  });
});

// Whether a per-row ✕ must confirm is a DECISION, so it lives in the pure module too rather than as
// an inline `!== true` in the component (this page's convention).
import { needsClearConfirm } from './shells-page-logic';

describe('needsClearConfirm', () => {
  it('does not confirm an attached row — one tap to undo via "Attachable tmux sessions"', () => {
    expect(needsClearConfirm(session({ id: 'a', provider: 'codex', attachedTmux: true }))).toBe(false);
  });

  it('confirms a plain shell — archiving it really closes its terminal (sessions.rs:221-225)', () => {
    expect(needsClearConfirm(session({ id: 'a', provider: 'shell', attachedTmux: false }))).toBe(true);
  });

  it('confirms when attachedTmux is missing from the payload (fail safe, not fail silent)', () => {
    expect(needsClearConfirm({ attachedTmux: undefined } as any)).toBe(true);
  });
});

// ── Optimistic local drop ────────────────────────────────────────────────────────────────────────
// The worker caches `list_sessions` for 5s with no cross-isolate invalidation, so on a phone the
// post-clear read can legitimately return the rows we just archived and the button reads as "did
// nothing". The list is therefore corrected LOCALLY the instant the archives resolve, and `refresh()`
// becomes reconciliation rather than the thing correctness depends on.
import { fulfilledClearIds, sessionsWithoutIds } from './shells-page-logic';

describe('optimistic local drop', () => {
  describe('fulfilledClearIds', () => {
    it('keeps only the ids whose archive actually fulfilled', () => {
      expect(fulfilledClearIds(['a', 'b', 'c'], ['fulfilled', 'rejected', 'fulfilled'])).toEqual(['a', 'c']);
    });

    it('is empty when every archive rejected (nothing may disappear)', () => {
      expect(fulfilledClearIds(['a', 'b'], ['rejected', 'rejected'])).toEqual([]);
    });

    it('is empty for no attempts', () => {
      expect(fulfilledClearIds([], [])).toEqual([]);
    });

    it('ignores ids with no matching outcome rather than assuming success', () => {
      expect(fulfilledClearIds(['a', 'b'], ['fulfilled'])).toEqual(['a']);
    });
  });

  describe('sessionsWithoutIds', () => {
    it('drops the cleared rows and keeps the rest', () => {
      const rows = [session({ id: 'a' }), session({ id: 'b' }), session({ id: 'c' })];
      expect(sessionsWithoutIds(rows, ['a', 'c']).map((s) => s.id)).toEqual(['b']);
    });

    it('keeps a row whose archive REJECTED — it still needs clearing', () => {
      const rows = [session({ id: 'a' }), session({ id: 'b' })];
      const cleared = fulfilledClearIds(['a', 'b'], ['fulfilled', 'rejected']);
      expect(sessionsWithoutIds(rows, cleared).map((s) => s.id)).toEqual(['b']);
    });

    it('is a filter, never a patch — surviving rows are the SAME objects, so no field can be lost', () => {
      // Deliberate: patching from a mutation result is the trap that loses `attachedTmux` and makes
      // `isShellSession` drop the row. Identity-equality pins that this filters instead.
      const keep = session({ id: 'b', provider: 'codex', attachedTmux: true, title: 'kloo' });
      const out = sessionsWithoutIds([session({ id: 'a' }), keep], ['a']);
      expect(out[0]).toBe(keep);
      expect(out[0].attachedTmux).toBe(true);
    });

    it('returns the list unchanged for an empty id set', () => {
      const rows = [session({ id: 'a' })];
      expect(sessionsWithoutIds(rows, []).map((s) => s.id)).toEqual(['a']);
    });
  });
});

// ── Suppression window ──────────────────────────────────────────────────────────────────────────
// The optimistic drop makes the tap right immediately, but `refresh()` replaces `sessions()`
// wholesale, so a worker read served from the 5s `list_sessions` cache would bring the cleared rows
// straight back — the same wrong conclusion ("it didn't work") arriving two seconds later. A cleared
// id is therefore suppressed for a window just longer than that cache's TTL.
import {
  SUPPRESSION_TTL_MS,
  addSuppressed,
  suppressed,
  applySuppression,
  pruneSuppressed,
} from './shells-page-logic';

describe('suppression window', () => {
  const T0 = 1_000_000;

  it('outlives the worker list_sessions cache TTL (5000ms)', () => {
    expect(SUPPRESSION_TTL_MS).toBeGreaterThan(5_000);
  });

  describe('addSuppressed', () => {
    it('records each id with an expiry one TTL out', () => {
      expect(addSuppressed({}, ['a', 'b'], T0)).toEqual({
        a: T0 + SUPPRESSION_TTL_MS,
        b: T0 + SUPPRESSION_TTL_MS,
      });
    });

    it('keeps existing entries and refreshes a re-cleared id', () => {
      const first = addSuppressed({}, ['a'], T0);
      const second = addSuppressed(first, ['b'], T0 + 100);
      expect(second['a']).toBe(T0 + SUPPRESSION_TTL_MS);
      expect(second['b']).toBe(T0 + 100 + SUPPRESSION_TTL_MS);
    });

    it('does not mutate the input', () => {
      const entries = {};
      addSuppressed(entries, ['a'], T0);
      expect(entries).toEqual({});
    });

    it('is a no-op for no ids', () => {
      expect(addSuppressed({ a: T0 }, [], T0)).toEqual({ a: T0 });
    });
  });

  describe('suppressed', () => {
    it('lists only un-expired ids', () => {
      const entries = { a: T0 + 1_000, b: T0 - 1 };
      expect(suppressed(T0, entries)).toEqual(['a']);
    });

    it('treats the exact expiry instant as expired (a window, not a trap)', () => {
      expect(suppressed(T0, { a: T0 })).toEqual([]);
    });
  });

  describe('applySuppression', () => {
    it('hides a cleared row that a stale read brought back', () => {
      const rows = [session({ id: 'a' }), session({ id: 'b' })];
      const entries = addSuppressed({}, ['a'], T0);
      expect(applySuppression(rows, entries, T0 + 1_000).map((s) => s.id)).toEqual(['b']);
    });

    // THE case that proves this is a window and not a permanent filter.
    it('lets the row back once the TTL has passed', () => {
      const rows = [session({ id: 'a' }), session({ id: 'b' })];
      const entries = addSuppressed({}, ['a'], T0);
      const after = T0 + SUPPRESSION_TTL_MS + 1;
      expect(applySuppression(rows, entries, after).map((s) => s.id)).toEqual(['a', 'b']);
    });

    it('returns the rows untouched when nothing is suppressed', () => {
      const rows = [session({ id: 'a' })];
      expect(applySuppression(rows, {}, T0)).toBe(rows);
    });
  });

  describe('pruneSuppressed', () => {
    it('drops an id the moment a read comes back without it (truth agrees — job done)', () => {
      const entries = addSuppressed({}, ['a'], T0);
      const read = [session({ id: 'b' })]; // 'a' really is gone now
      expect(pruneSuppressed(entries, read, T0 + 1_000)).toEqual({});
    });

    it('keeps an id the read still (wrongly) contains, until it expires', () => {
      const entries = addSuppressed({}, ['a'], T0);
      const read = [session({ id: 'a' })]; // stale cache still serving it
      expect(pruneSuppressed(entries, read, T0 + 1_000)).toEqual({ a: T0 + SUPPRESSION_TTL_MS });
    });

    it('drops an expired id even while the read still contains it (never permanent)', () => {
      const entries = addSuppressed({}, ['a'], T0);
      const read = [session({ id: 'a' })];
      expect(pruneSuppressed(entries, read, T0 + SUPPRESSION_TTL_MS + 1)).toEqual({});
    });

    it('cannot hide a re-created row: a re-attached pane gets a NEW id', () => {
      // Archiving never mints an id and ids are uuids, so a suppressed id can only ever name the
      // exact row that was cleared. Re-attaching `kloo` creates a different session id.
      const entries = addSuppressed({}, ['old-uuid'], T0);
      const read = [session({ id: 'new-uuid', attachedTmux: true, title: 'kloo' })];
      expect(applySuppression(read, entries, T0 + 1_000).map((s) => s.id)).toEqual(['new-uuid']);
      expect(pruneSuppressed(entries, read, T0 + 1_000)).toEqual({});
    });
  });
});
