import { describe, it, expect } from 'vitest';
import { readFileSync } from 'node:fs';
import { dirname, resolve } from 'node:path';
import { fileURLToPath } from 'node:url';
import {
  captureReturnUrl,
  DEFAULT_RETURN_URL,
  isSafeReturnUrl,
  loginUrlWithReturn,
  safeReturnUrl,
} from './return-url-logic';

const here = dirname(fileURLToPath(import.meta.url));

describe('safeReturnUrl', () => {
  const cases: Array<{ raw: string | null | undefined; want: string; why: string }> = [
    // Accepted: rooted in-app paths.
    { raw: '/chat', want: '/chat', why: 'any rooted path is accepted, declared route or not' },
    { raw: '/chat?x=1#y', want: '/chat?x=1#y', why: 'query + fragment preserved' },
    { raw: '/initiatives/abc-123?tab=phases', want: '/initiatives/abc-123?tab=phases', why: 'deep path' },
    { raw: '/', want: '/', why: 'root is a legal destination for the consumer' },
    { raw: '  /settings  ', want: '/settings', why: 'surrounding whitespace trimmed' },
    { raw: '/%2F%2Fevil.com', want: '/%2F%2Fevil.com', why: 'still a single-slash path' },

    // Rejected: off-origin or non-path.
    { raw: '//evil.com', want: DEFAULT_RETURN_URL, why: 'protocol-relative' },
    { raw: '///evil.com', want: DEFAULT_RETURN_URL, why: 'protocol-relative, extra slash' },
    { raw: '/\\evil.com', want: DEFAULT_RETURN_URL, why: 'backslash variant browsers normalise to //' },
    { raw: '/\\\\evil.com', want: DEFAULT_RETURN_URL, why: 'double backslash' },
    { raw: 'https://evil.com', want: DEFAULT_RETURN_URL, why: 'absolute URL' },
    { raw: 'http://evil.com', want: DEFAULT_RETURN_URL, why: 'absolute URL' },
    { raw: 'javascript:alert(1)', want: DEFAULT_RETURN_URL, why: 'script scheme' },
    { raw: 'data:text/html,<script>1</script>', want: DEFAULT_RETURN_URL, why: 'data scheme' },
    { raw: ' javascript:alert(1)', want: DEFAULT_RETURN_URL, why: 'leading space does not hide the scheme' },
    { raw: 'java\nscript:alert(1)', want: DEFAULT_RETURN_URL, why: 'control character' },
    { raw: '/chat\n/../evil', want: DEFAULT_RETURN_URL, why: 'embedded newline rejected outright' },
    { raw: 'chat', want: DEFAULT_RETURN_URL, why: 'relative path is not rooted' },
    { raw: './chat', want: DEFAULT_RETURN_URL, why: 'relative path is not rooted' },
    { raw: '', want: DEFAULT_RETURN_URL, why: 'empty string' },
    { raw: '   ', want: DEFAULT_RETURN_URL, why: 'whitespace only' },
    { raw: null, want: DEFAULT_RETURN_URL, why: 'absent query param' },
    { raw: undefined, want: DEFAULT_RETURN_URL, why: 'undefined' },
    { raw: '/x\u00a0y', want: DEFAULT_RETURN_URL, why: 'non-breaking space' },
    { raw: '/x\u2028y', want: DEFAULT_RETURN_URL, why: 'line separator' },
    { raw: '/x\ufeffy', want: DEFAULT_RETURN_URL, why: 'byte-order mark' },
    { raw: '/x y', want: DEFAULT_RETURN_URL, why: 'interior plain space' },
    { raw: '\u00a0//evil.com', want: DEFAULT_RETURN_URL, why: 'NBSP cannot hide a protocol-relative URL' },
  ];

  for (const { raw, want, why } of cases) {
    it(`${JSON.stringify(raw)} → ${want} (${why})`, () => {
      expect(safeReturnUrl(raw)).toBe(want);
    });
  }

  it('honours a custom fallback', () => {
    expect(safeReturnUrl('//evil.com', '/settings')).toBe('/settings');
    expect(safeReturnUrl(null, '/settings')).toBe('/settings');
    expect(safeReturnUrl('/chat', '/settings')).toBe('/chat');
  });

  it('the default fallback is a REAL route, not the wildcard', () => {
    // `/chat` was the old default and is not declared anywhere: it only ever
    // resolved via `{ path: '**', redirectTo: 'initiatives' }`.
    const routes = readFileSync(resolve(here, '../app.routes.ts'), 'utf8');
    expect(DEFAULT_RETURN_URL).toBe('/initiatives');
    expect(routes).toMatch(/path:\s*'initiatives'/);
    expect(routes).not.toMatch(/path:\s*'chat'/);
    // and it agrees with what the empty route redirects to
    expect(routes).toMatch(/redirectTo:\s*'initiatives'/);
  });

  it('isSafeReturnUrl agrees with safeReturnUrl on every case', () => {
    const SENTINEL = '/__fallback__';
    for (const { raw } of cases) {
      expect(isSafeReturnUrl(raw)).toBe(safeReturnUrl(raw, SENTINEL) !== SENTINEL);
    }
  });

  it('a non-string never slips through', () => {
    for (const bad of [0, 1, {}, [], true, false, NaN] as unknown[]) {
      expect(safeReturnUrl(bad as string)).toBe(DEFAULT_RETURN_URL);
    }
  });
});

describe('captureReturnUrl (producer side)', () => {
  it('keeps a deep in-app URL', () => {
    expect(captureReturnUrl('/initiatives/abc?tab=phases')).toBe('/initiatives/abc?tab=phases');
    expect(captureReturnUrl('/terminal')).toBe('/terminal');
  });

  it('captures nothing for the empty URL or the root', () => {
    expect(captureReturnUrl('')).toBeUndefined();
    expect(captureReturnUrl('/')).toBeUndefined();
    expect(captureReturnUrl(null)).toBeUndefined();
    expect(captureReturnUrl(undefined)).toBeUndefined();
  });

  it('never self-references /login, so a second expiry cannot nest returnUrls', () => {
    expect(captureReturnUrl('/login')).toBeUndefined();
    expect(captureReturnUrl('/login?returnUrl=%2Fterminal')).toBeUndefined();
    expect(captureReturnUrl('/login/')).toBeUndefined();
    expect(captureReturnUrl('/login#x')).toBeUndefined();
  });

  it('the /login check is case-insensitive', () => {
    // Unreachable today (Angular route matching is case-sensitive, so `/LOGIN`
    // hits the `**` wildcard) but the predicate should not lie about its name.
    expect(captureReturnUrl('/LOGIN')).toBeUndefined();
    expect(captureReturnUrl('/Login?returnUrl=%2Fterminal')).toBeUndefined();
    expect(captureReturnUrl('/LoGiN#x')).toBeUndefined();
  });

  it('a path that merely starts with the letters "login" is still captured', () => {
    expect(captureReturnUrl('/loginhistory')).toBe('/loginhistory');
  });

  it('refuses to emit an off-origin returnUrl even if the router handed one over', () => {
    expect(captureReturnUrl('//evil.com')).toBeUndefined();
    expect(captureReturnUrl('https://evil.com')).toBeUndefined();
  });
});

describe('loginUrlWithReturn', () => {
  it('produces the same shape the guard does', () => {
    expect(loginUrlWithReturn('/initiatives/abc?tab=phases')).toBe(
      '/login?returnUrl=' + encodeURIComponent('/initiatives/abc?tab=phases'),
    );
  });

  it('is a bare /login when there is nothing to capture', () => {
    expect(loginUrlWithReturn('/')).toBe('/login');
    expect(loginUrlWithReturn('/login?returnUrl=%2Fterminal')).toBe('/login');
    expect(loginUrlWithReturn(undefined)).toBe('/login');
  });

  it('round-trips through safeReturnUrl', () => {
    const deep = '/terminal?session=s1#pane';
    const url = loginUrlWithReturn(deep);
    const raw = decodeURIComponent(url.slice('/login?returnUrl='.length));
    expect(safeReturnUrl(raw)).toBe(deep);
  });
});
