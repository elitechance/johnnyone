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
    it('is the /terminal?sessionId=&surface=shell plain-shell nav shape', () => {
      expect(openIntent('abc')).toEqual({
        path: '/terminal',
        queryParams: { sessionId: 'abc', surface: 'shell' },
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
