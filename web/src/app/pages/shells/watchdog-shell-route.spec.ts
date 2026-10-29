import { describe, it, expect } from 'vitest';
import { isShellView, SHELL_VIEWS, resolveShellRoute, matchShellRoute } from './shells-route';

describe('shells-route watchdog integration', () => {
  it('isShellView accepts watchdog', () => {
    expect(isShellView('watchdog')).toBe(true);
  });

  it('SHELL_VIEWS contains all three in toggle order', () => {
    expect(SHELL_VIEWS).toEqual(['transcript', 'raw', 'watchdog']);
  });

  it('matchShellRoute resolves watchdog view', () => {
    const segments = [
      { path: 'shells', parameters: {} },
      { path: 'abc', parameters: {} },
      { path: 'watchdog', parameters: {} },
    ] as unknown as any[];
    const result = matchShellRoute(segments as any);
    expect(result).not.toBeNull();
    expect(result!.posParams!['view'].path).toBe('watchdog');
  });

  it('resolveShellRoute canonicalises Watchdog spelling', () => {
    const r = resolveShellRoute(
      { sessionId: 'abc', view: 'Watchdog' },
      { currentSessionId: 'abc', pendingSessionId: null, knownSessionIds: ['abc'] },
    );
    expect(r.view).toBe('watchdog');
    expect(r.redirectTo).toBe('/shells/abc/watchdog');
  });

  it('resolveShellRoute defaults to transcript for bare route', () => {
    const r = resolveShellRoute(
      { sessionId: 'abc', view: null },
      { currentSessionId: null, pendingSessionId: null, knownSessionIds: [] },
    );
    expect(r.view).toBe('transcript');
  });
});