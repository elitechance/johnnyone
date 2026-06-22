import { describe, it, expect, vi, beforeEach, afterEach } from 'vitest';

// Ensure Angular JIT/compiler is available so the real @Injectable class can be
// imported as a value (per T2/QA/Lead requirement) rather than type-only. The
// service has no constructor deps, so a plain `new` is sufficient — no TestBed.
import '@angular/compiler';

import { TerminalScreenCacheService } from './terminal-screen-cache.service';
import type { TerminalScreen } from '@johnnyone/ui';

const STORAGE_KEY = 'johnnyone_terminal_screen_cache';

function makeScreen(sessionId: string, content = `screen-${sessionId}`): TerminalScreen {
  return {
    sessionId,
    paneId: `pane-${sessionId}`,
    cursor: 0,
    content,
    cursorX: 0,
    cursorY: 0,
    historyLines: 1,
    rows: 24,
    cols: 80,
    status: 'running',
  };
}

/** Seed the persisted store directly so we exercise readStore/writeStore paths. */
function seedStore(ids: string[]): void {
  const sessions: Record<string, unknown> = {};
  for (const id of ids) {
    sessions[id] = {
      screen: makeScreen(id),
      savedAt: Date.now(),
      contentLength: makeScreen(id).content.length,
    };
  }
  localStorage.setItem(STORAGE_KEY, JSON.stringify({ version: 1, sessions }));
}

function storedIds(): string[] {
  const raw = localStorage.getItem(STORAGE_KEY);
  if (!raw) return [];
  const parsed = JSON.parse(raw) as { sessions?: Record<string, unknown> };
  return Object.keys(parsed.sessions ?? {});
}

describe('TerminalScreenCacheService.retainOnly (liveness prune)', () => {
  let cache: TerminalScreenCacheService;

  beforeEach(() => {
    localStorage.clear();
    cache = new TerminalScreenCacheService();
  });

  afterEach(() => {
    vi.useRealTimers();
    localStorage.clear();
  });

  it('drops non-active entries and keeps active ones (store rewritten without the dead id)', () => {
    seedStore(['aaa', 'bbb', 'ccc']);

    cache.retainOnly(['aaa', 'ccc']);

    expect(cache.get('bbb')).toBeNull();
    expect(cache.get('aaa')).not.toBeNull();
    expect(cache.get('ccc')).not.toBeNull();
    expect(storedIds().sort()).toEqual(['aaa', 'ccc']);
    expect(localStorage.getItem(STORAGE_KEY)).not.toContain('bbb');
  });

  it('keeps all when every id is active and does not rewrite the store (idempotent no-op)', () => {
    seedStore(['aaa', 'bbb', 'ccc']);
    const before = localStorage.getItem(STORAGE_KEY)!;
    const setSpy = vi.spyOn(Storage.prototype, 'setItem');

    cache.retainOnly(['aaa', 'bbb', 'ccc']);

    // Nothing removed → no write at all (byte-identical store).
    expect(setSpy).not.toHaveBeenCalled();
    expect(localStorage.getItem(STORAGE_KEY)).toBe(before);
    setSpy.mockRestore();
  });

  it('empty active set clears all cached sessions', () => {
    seedStore(['aaa', 'bbb']);

    cache.retainOnly([]);

    expect(storedIds()).toEqual([]);
    expect(cache.get('aaa')).toBeNull();
    expect(cache.get('bbb')).toBeNull();
  });

  it('cancels the pending debounce for a dropped id so a queued write cannot resurrect it', () => {
    vi.useFakeTimers();
    seedStore(['aaa']);

    // Queue a debounced save for a session that is about to be pruned.
    cache.remember(makeScreen('bbb'));
    cache.retainOnly(['aaa']);

    // Fire any timers — the debounced bbb write must NOT re-add it.
    vi.runAllTimers();

    expect(cache.get('bbb')).toBeNull();
    expect(storedIds()).toEqual(['aaa']);
  });

  it('does not throw on an empty store', () => {
    expect(() => cache.retainOnly(['aaa'])).not.toThrow();
    expect(storedIds()).toEqual([]);
  });
});
