import { Injectable } from '@angular/core';
import { TerminalScreen } from '@johnnyone/ui';

interface CachedTerminalScreenEntry {
  screen: TerminalScreen;
  savedAt: number;
  contentLength: number;
}

interface TerminalScreenCacheStore {
  version: 1;
  sessions: Record<string, CachedTerminalScreenEntry>;
}

@Injectable({ providedIn: 'root' })
export class TerminalScreenCacheService {
  private static readonly STORAGE_KEY = 'johnnyone_terminal_screen_cache';
  private static readonly MAX_SESSIONS = 30;
  private static readonly MAX_CONTENT_CHARS = 400_000;
  private static readonly MAX_AGE_MS = 7 * 24 * 60 * 60 * 1000;
  private static readonly SAVE_DEBOUNCE_MS = 1_500;

  private readonly pendingSaves = new Map<string, ReturnType<typeof setTimeout>>();
  private readonly pendingScreen = new Map<string, TerminalScreen>();

  get(sessionId: string): TerminalScreen | null {
    const entry = this.readStore().sessions[sessionId];
    if (!entry) return null;
    if (Date.now() - entry.savedAt > TerminalScreenCacheService.MAX_AGE_MS) {
      this.remove(sessionId);
      return null;
    }
    return entry.screen;
  }

  remember(screen: TerminalScreen): void {
    this.pendingScreen.set(screen.sessionId, screen);
    if (this.pendingSaves.has(screen.sessionId)) return;

    const timer = setTimeout(() => {
      this.pendingSaves.delete(screen.sessionId);
      const latest = this.pendingScreen.get(screen.sessionId);
      this.pendingScreen.delete(screen.sessionId);
      if (latest) {
        this.persist(latest);
      }
    }, TerminalScreenCacheService.SAVE_DEBOUNCE_MS);
    this.pendingSaves.set(screen.sessionId, timer);
  }

  remove(sessionId: string): void {
    this.cancelPendingSave(sessionId);

    const store = this.readStore();
    if (!store.sessions[sessionId]) return;
    delete store.sessions[sessionId];
    this.writeStore(store);
  }

  /**
   * Liveness prune: drop every cached session whose id is not in `activeSessionIds`,
   * cancelling any pending debounced write for the dropped ids so it cannot re-add a
   * dead session after the prune. Rewrites localStorage only when at least one entry
   * was removed (idempotent no-op otherwise — the common steady-state path).
   */
  retainOnly(activeSessionIds: Iterable<string>): void {
    const activeIds = new Set(activeSessionIds);

    // Defuse any queued debounce for ids we are about to drop, so an in-flight
    // remember() cannot resurrect a dead session after the store is rewritten.
    for (const sessionId of this.pendingSaves.keys()) {
      if (!activeIds.has(sessionId)) {
        this.cancelPendingSave(sessionId);
      }
    }
    for (const sessionId of this.pendingScreen.keys()) {
      if (!activeIds.has(sessionId)) {
        this.cancelPendingSave(sessionId);
      }
    }

    const store = this.readStore();
    let changed = false;
    for (const sessionId of Object.keys(store.sessions)) {
      if (!activeIds.has(sessionId)) {
        delete store.sessions[sessionId];
        changed = true;
      }
    }

    if (changed) {
      this.writeStore(store);
    }
  }

  private cancelPendingSave(sessionId: string): void {
    const timer = this.pendingSaves.get(sessionId);
    if (timer) {
      clearTimeout(timer);
      this.pendingSaves.delete(sessionId);
    }
    this.pendingScreen.delete(sessionId);
  }

  flush(): void {
    for (const timer of this.pendingSaves.values()) {
      clearTimeout(timer);
    }
    this.pendingSaves.clear();
    for (const [sessionId, screen] of this.pendingScreen.entries()) {
      this.persist(screen);
      this.pendingScreen.delete(sessionId);
    }
  }

  private persist(incoming: TerminalScreen): void {
    const store = this.readStore();
    const existing = store.sessions[incoming.sessionId]?.screen ?? null;
    const merged = this.mergeForStorage(incoming, existing);
    const content = this.truncateContent(merged.content);
    const screen: TerminalScreen = {
      ...merged,
      content,
      historyLines: Math.max(merged.historyLines, this.lineCount(content)),
    };

    store.sessions[incoming.sessionId] = {
      screen,
      savedAt: Date.now(),
      contentLength: screen.content.length,
    };

    this.pruneStore(store);
    this.writeStore(store);
  }

  /** Storage-only merge: keep the richest scrollback without blocking live display. */
  private mergeForStorage(live: TerminalScreen, cached: TerminalScreen | null): TerminalScreen {
    if (!cached) return live;

    const liveLines = this.lineCount(live.content);
    const cachedLines = this.lineCount(cached.content);
    const liveLonger = live.content.length > cached.content.length;
    const liveMoreLines = liveLines > cachedLines;
    const liveNewer = live.cursor !== cached.cursor
      || live.cursorX !== cached.cursorX
      || live.cursorY !== cached.cursorY;

    if (liveLonger || liveMoreLines || (liveNewer && liveLines >= cachedLines - 1)) {
      return {
        ...live,
        historyLines: Math.max(live.historyLines, cached.historyLines),
      };
    }

    return {
      ...cached,
      status: live.status || cached.status,
      cursor: live.cursor,
      cursorX: live.cursorX,
      cursorY: live.cursorY,
      rows: live.rows || cached.rows,
      cols: live.cols || cached.cols,
      paneId: live.paneId || cached.paneId,
      tmuxSessionName: live.tmuxSessionName || cached.tmuxSessionName,
      historyLines: Math.max(live.historyLines, cached.historyLines),
    };
  }

  private pruneStore(store: TerminalScreenCacheStore): void {
    const now = Date.now();
    for (const [sessionId, entry] of Object.entries(store.sessions)) {
      if (now - entry.savedAt > TerminalScreenCacheService.MAX_AGE_MS) {
        delete store.sessions[sessionId];
      }
    }

    const entries = Object.entries(store.sessions);
    if (entries.length <= TerminalScreenCacheService.MAX_SESSIONS) return;

    entries
      .sort((a, b) => a[1].savedAt - b[1].savedAt)
      .slice(0, entries.length - TerminalScreenCacheService.MAX_SESSIONS)
      .forEach(([sessionId]) => {
        delete store.sessions[sessionId];
      });
  }

  private truncateContent(content: string): string {
    if (content.length <= TerminalScreenCacheService.MAX_CONTENT_CHARS) return content;
    return content.slice(content.length - TerminalScreenCacheService.MAX_CONTENT_CHARS);
  }

  private lineCount(content: string): number {
    if (!content) return 0;
    return content.replace(/\r\n/g, '\n').replace(/\r/g, '\n').split('\n').length;
  }

  private readStore(): TerminalScreenCacheStore {
    if (typeof window === 'undefined') {
      return { version: 1, sessions: {} };
    }

    try {
      const raw = window.localStorage.getItem(TerminalScreenCacheService.STORAGE_KEY);
      if (!raw) return { version: 1, sessions: {} };

      const parsed = JSON.parse(raw) as Partial<TerminalScreenCacheStore>;
      if (!parsed || parsed.version !== 1 || !parsed.sessions || typeof parsed.sessions !== 'object') {
        return { version: 1, sessions: {} };
      }

      const sessions: Record<string, CachedTerminalScreenEntry> = {};
      for (const [sessionId, entry] of Object.entries(parsed.sessions)) {
        if (!entry || typeof entry !== 'object') continue;
        const candidate = entry as Partial<CachedTerminalScreenEntry>;
        if (!candidate.screen || typeof candidate.savedAt !== 'number') continue;
        const screen = candidate.screen as TerminalScreen;
        if (!screen.sessionId || typeof screen.content !== 'string') continue;
        sessions[sessionId] = {
          screen,
          savedAt: candidate.savedAt,
          contentLength: Number(candidate.contentLength) || screen.content.length,
        };
      }

      return { version: 1, sessions };
    } catch {
      return { version: 1, sessions: {} };
    }
  }

  private writeStore(store: TerminalScreenCacheStore): void {
    if (typeof window === 'undefined') return;

    try {
      window.localStorage.setItem(TerminalScreenCacheService.STORAGE_KEY, JSON.stringify(store));
    } catch {
      const entries = Object.entries(store.sessions)
        .sort((a, b) => b[1].savedAt - a[1].savedAt)
        .slice(0, Math.max(5, Math.floor(TerminalScreenCacheService.MAX_SESSIONS / 2)));
      const trimmed: TerminalScreenCacheStore = {
        version: 1,
        sessions: Object.fromEntries(entries),
      };
      try {
        window.localStorage.setItem(TerminalScreenCacheService.STORAGE_KEY, JSON.stringify(trimmed));
      } catch {
        // Ignore localStorage quota failures.
      }
    }
  }
}