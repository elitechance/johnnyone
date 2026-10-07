import '@angular/compiler';
import { describe, it, expect } from 'vitest';
import { UrlSegment } from '@angular/router';
import {
  DEFAULT_SHELL_VIEW,
  SHELL_VIEWS,
  isShellView,
  matchShellRoute,
  resolveShellRoute,
  shellRoutePath,
  shellViewTogglePath,
} from './shells-route';

// Pins the `/shells/:id[/transcript|raw]` URL contract and the route->state resolution the terminal
// page applies. A regression here is a dead or wrong-looking shared link, or a duplicate session
// fetch — all invisible from the page, which cannot be mounted under this vitest config.

const segs = (...paths: string[]) => paths.map((p) => new UrlSegment(p, {}));

describe('matchShellRoute', () => {
  it('matches the bare two-segment URL and exposes the session id', () => {
    const result = matchShellRoute(segs('shells', 'abc'));
    expect(result).not.toBeNull();
    expect(result!.consumed).toHaveLength(2);
    expect(result!.posParams!['sessionId'].path).toBe('abc');
    // No view named: the page resolves the default rather than the URL asserting one.
    expect(result!.posParams!['view']).toBeUndefined();
  });

  it('matches an explicit /transcript and /raw', () => {
    for (const view of SHELL_VIEWS) {
      const result = matchShellRoute(segs('shells', 'abc', view));
      expect(result).not.toBeNull();
      expect(result!.consumed).toHaveLength(3);
      expect(result!.posParams!['sessionId'].path).toBe('abc');
      expect(result!.posParams!['view'].path).toBe(view);
    }
  });

  it('matches a mis-cased view segment — autocapitalised links are the main use case', () => {
    // Case folding itself happens in `resolveShellRoute` (the matcher cannot construct a UrlSegment
    // without a runtime @angular/router import), so here we only assert that it MATCHES at all.
    for (const spelling of ['Raw', 'RAW', 'Transcript', 'TRANSCRIPT']) {
      const result = matchShellRoute(segs('shells', 'abc', spelling));
      expect(result, spelling).not.toBeNull();
      expect(result!.posParams!['view'].path).toBe(spelling);
    }
  });

  it('matches an unknown third segment instead of falling through to the ** wildcard', () => {
    // Returning null here would redirect the operator to the initiatives console with no shell and
    // no message. The page renders the default view and rewrites the URL instead.
    const result = matchShellRoute(segs('shells', 'abc', 'bogus'));
    expect(result).not.toBeNull();
    expect(result!.posParams!['sessionId'].path).toBe('abc');
    expect(result!.posParams!['view'].path).toBe('bogus');
  });

  it('rejects more than three segments', () => {
    expect(matchShellRoute(segs('shells', 'abc', 'raw', 'extra'))).toBeNull();
  });

  it('rejects a different first segment', () => {
    expect(matchShellRoute(segs('files', 'abc'))).toBeNull();
  });

  it('rejects an empty session id', () => {
    expect(matchShellRoute(segs('shells', ''))).toBeNull();
  });

  it('rejects `/shells` alone, leaving the list route to match it', () => {
    expect(matchShellRoute(segs('shells'))).toBeNull();
  });
});

describe('shellRoutePath', () => {
  it('builds the bare shareable URL when no view is asked for', () => {
    expect(shellRoutePath('abc')).toBe('/shells/abc');
    expect(shellRoutePath('abc', null)).toBe('/shells/abc');
  });

  it('builds the explicit per-view URLs', () => {
    expect(shellRoutePath('abc', 'transcript')).toBe('/shells/abc/transcript');
    expect(shellRoutePath('abc', 'raw')).toBe('/shells/abc/raw');
  });

  it('round-trips through the matcher for every view', () => {
    for (const view of SHELL_VIEWS) {
      const path = shellRoutePath('abc', view);
      const result = matchShellRoute(segs(...path.replace(/^\//, '').split('/')));
      expect(result!.posParams!['view'].path).toBe(view);
    }
  });
});

/** A page that has resolved nothing yet: no session selected, none in flight, none in the list. */
const fresh = { currentSessionId: undefined, pendingSessionId: null, knownSessionIds: [] };

describe('shellViewTogglePath', () => {
  it('sends the default view back to the BARE URL, not /transcript', () => {
    // The toggle navigates with `replaceUrl`, so minting `/shells/x/transcript` here would destroy
    // the short shareable URL in place for no visible difference.
    expect(shellViewTogglePath('x', 'transcript')).toBe('/shells/x');
  });

  it('sends the non-default view to its explicit URL', () => {
    expect(shellViewTogglePath('x', 'raw')).toBe('/shells/x/raw');
  });

  it('makes the default-view toggle a navigation whose RESOLVED view does not change', () => {
    // Documents the premise behind the tab-title effect's dependency choice in TerminalPage. Tapping
    // Transcript on `/shells/x/transcript` navigates to the bare `/shells/x`: a real navigation (so
    // AppTitleStrategy rewrites the tab to "Shell") across which the resolved view is IDENTICAL.
    // Only the route-level value moves ('transcript' -> absent), which is why the effect depends on
    // the route signal and not on the resolved `shellView()`.
    const from = resolveShellRoute({ sessionId: 'x', view: 'transcript' }, fresh);
    const to = resolveShellRoute({ sessionId: 'x', view: null }, fresh);
    expect(shellViewTogglePath('x', 'transcript')).toBe('/shells/x');
    expect(to.view).toBe(from.view);
  });

  it('produces URLs the matcher accepts, for every view', () => {
    for (const view of SHELL_VIEWS) {
      const path = shellViewTogglePath('x', view);
      expect(matchShellRoute(segs(...path.replace(/^\//, '').split('/')))).not.toBeNull();
      // ...and the page resolves each one back to the view the toggle asked for.
      const url = path.replace(/^\//, '').split('/');
      expect(resolveShellRoute({ sessionId: url[1], view: url[2] ?? null }, fresh).view).toBe(view);
    }
  });
});

describe('DEFAULT_SHELL_VIEW', () => {
  it('is the transcript — raw is asked for by URL, never landed on', () => {
    expect(DEFAULT_SHELL_VIEW).toBe('transcript');
  });
});

describe('isShellView', () => {
  it('accepts exactly the two views and nothing else', () => {
    expect(SHELL_VIEWS.every((v) => isShellView(v))).toBe(true);
    // Case folding is the caller's job (the guard narrows an already-canonical value).
    for (const bad of ['', 'Raw', 'bogus', null, undefined]) {
      expect(isShellView(bad)).toBe(false);
    }
  });
});

describe('resolveShellRoute — view', () => {
  it('resolves no view segment to the default, with nothing to redirect', () => {
    const r = resolveShellRoute({ sessionId: 'x', view: null }, fresh);
    expect(r.view).toBe('transcript');
    expect(r.redirectTo).toBeNull();
  });

  it('resolves an explicit view and leaves a canonical URL alone', () => {
    expect(resolveShellRoute({ sessionId: 'x', view: 'raw' }, fresh)).toMatchObject({
      view: 'raw',
      redirectTo: null,
    });
    expect(resolveShellRoute({ sessionId: 'x', view: 'transcript' }, fresh)).toMatchObject({
      view: 'transcript',
      redirectTo: null,
    });
  });

  it('case-folds a mis-cased view and heals the URL to the canonical spelling', () => {
    expect(resolveShellRoute({ sessionId: 'x', view: 'Raw' }, fresh)).toMatchObject({
      view: 'raw',
      redirectTo: '/shells/x/raw',
    });
    expect(resolveShellRoute({ sessionId: 'x', view: 'TRANSCRIPT' }, fresh)).toMatchObject({
      view: 'transcript',
      redirectTo: '/shells/x/transcript',
    });
  });

  it('falls back to the default and heals to the bare URL for a non-view segment', () => {
    expect(resolveShellRoute({ sessionId: 'x', view: 'bogus' }, fresh)).toMatchObject({
      view: 'transcript',
      redirectTo: '/shells/x',
    });
  });

  it('never proposes a redirect it cannot build (no session id in the URL)', () => {
    expect(resolveShellRoute({ sessionId: null, view: 'bogus' }, fresh).redirectTo).toBeNull();
  });
});

describe('resolveShellRoute — session', () => {
  it('does nothing when the URL names no session (the initiative console)', () => {
    expect(resolveShellRoute({ sessionId: null, view: null }, fresh).session).toBe('none');
  });

  it('loads a session it has never seen', () => {
    expect(resolveShellRoute({ sessionId: 'x', view: null }, fresh).session).toBe('load');
  });

  it('selects a session already in the list instead of refetching', () => {
    const r = resolveShellRoute(
      { sessionId: 'x', view: null },
      { ...fresh, knownSessionIds: ['y', 'x'] },
    );
    expect(r.session).toBe('select');
  });

  it('does nothing when the URL names the session already rendered', () => {
    const r = resolveShellRoute(
      { sessionId: 'x', view: 'raw' },
      { ...fresh, currentSessionId: 'x', knownSessionIds: ['x'] },
    );
    expect(r.session).toBe('none');
    // ...but the view is still resolved: this is exactly the toggle case.
    expect(r.view).toBe('raw');
  });

  it('does NOT fire a second load while one is already in flight for that id', () => {
    // The regression: toggling Transcript/Raw during boot on a slow relay. `currentSessionId` is
    // still undefined, the session list is still empty, and the URL names the same id ngOnInit is
    // already fetching — an unguarded handler starts a second concurrent `loadSessions`.
    const booting = { currentSessionId: undefined, pendingSessionId: 'x', knownSessionIds: [] };
    expect(resolveShellRoute({ sessionId: 'x', view: 'raw' }, booting).session).toBe('none');
    expect(resolveShellRoute({ sessionId: 'x', view: 'raw' }, booting).view).toBe('raw');
    // A DIFFERENT id is still loaded — the guard is about the id, not about being busy.
    expect(resolveShellRoute({ sessionId: 'y', view: null }, booting).session).toBe('load');
  });

  it('resolves the view on every emission, never gated behind the session check', () => {
    // The other ordering trap: the handler early-returns when the session is unchanged, which is
    // every single view toggle. Whatever the session branch decides, `view` must be set.
    for (const state of [
      fresh,
      { ...fresh, currentSessionId: 'x' },
      { ...fresh, pendingSessionId: 'x' },
      { ...fresh, knownSessionIds: ['x'] },
    ]) {
      expect(resolveShellRoute({ sessionId: 'x', view: 'raw' }, state).view).toBe('raw');
    }
  });
});
