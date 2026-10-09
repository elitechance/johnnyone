/**
 * Regression: a session expiry must keep the URL the user was on so the login
 * page can send them back to it.
 *
 * The navigation-time case (authGuard) already worked. Two expiry cases did not:
 *
 * 1. **In-tab expiry.** `AuthService` used to end every logout with a bare
 *    `navigateByUrl('/login')`, throwing the deep URL away.
 * 2. **Bootstrap expiry** — a full reload of a deep URL with a dead session.
 *    `startSession()` runs inside `provideAppInitializer` (app.config.ts), which
 *    is awaited *before* the root component bootstraps, so the router has not
 *    read the address bar yet and `Router.url` is `'/'`. Worse, the initializer's
 *    own `navigateByUrl` bumps `navigationId`, and `Router.initialNavigation()`
 *    is gated on `!hasRequestedNavigation` — so it never calls
 *    `location.path(true)` and the deep URL is dropped before any
 *    `CanActivateFn` exists. The guard does NOT compensate.
 *
 * Hence the source of truth on the expiry path is the **browser** address bar
 * (`Location.path(true)`, exactly what `initialNavigation()` itself reads), not
 * `Router.url`.
 *
 * A deliberate sign-out must NOT capture a returnUrl.
 */
import '@angular/compiler';
import { describe, it, expect, vi, beforeEach, afterEach } from 'vitest';
import { createRequire } from 'node:module';
import { readFileSync, readdirSync } from 'node:fs';
import { dirname, resolve } from 'node:path';
import { fileURLToPath } from 'node:url';
import { Injector, runInInjectionContext, signal } from '@angular/core';
import { Router } from '@angular/router';
import { AuthService } from './auth.service';
import { authGuard } from './auth.guard';

const API = 'http://example.test/graphql';
const here = dirname(fileURLToPath(import.meta.url));
const require = createRequire(import.meta.url);

/**
 * The hash-named internal chunk of an Angular package (`location-XXXX.mjs`),
 * found next to the package entry point.
 */
function readAngularBundle(pkg: string, pattern: RegExp): string {
  const dir = dirname(require.resolve(pkg));
  const file = readdirSync(dir).find((f) => pattern.test(f));
  if (!file) throw new Error(`no ${String(pattern)} chunk in ${dir}`);
  return readFileSync(resolve(dir, file), 'utf8');
}

/**
 * AuthService with both URL sources stubbed independently.
 *
 * `browserUrl` is what `@angular/common`'s `Location.path(true)` reports (the
 * address bar). `routerUrl` is `Router.url`. In a live tab they agree; at
 * bootstrap they do not, which is the whole point of this spec.
 */
function makeAuth(
  browserUrl: string,
  routerUrl: string = browserUrl,
): {
  auth: AuthService;
  navigateByUrl: ReturnType<typeof vi.fn>;
} {
  const navigateByUrl = vi.fn().mockResolvedValue(true);
  const inst = Object.create(AuthService.prototype) as AuthService;
  (inst as any).router = { navigateByUrl, url: routerUrl };
  (inst as any).location = { path: (includeHash?: boolean) => (includeHash ? browserUrl : browserUrl.split(/[?#]/)[0]) };
  (inst as any).isAuthenticated = signal(false);
  (inst as any).currentUser = signal(null);
  (inst as any).inflightEnsure = null;
  (inst as any).refreshTimer = null;
  (inst as any).lastApiUrl = null;
  (inst as any).timerBackoffMs = 0;
  (inst as any).wakeHandler = null;
  return { auth: inst, navigateByUrl };
}

function plantDeadSession(now: number): void {
  localStorage.clear();
  localStorage.setItem(AuthService.TOKEN_KEY, 'stale');
  localStorage.setItem(AuthService.REFRESH_TOKEN_KEY, 'r');
  localStorage.setItem(AuthService.EXPIRES_AT_KEY, String(now - 1));
  localStorage.setItem(AuthService.EXPIRES_IN_KEY, '900');
  localStorage.setItem(AuthService.TENANT_KEY, 't1');
}

/** The single destination the service navigated to. */
function destination(navigateByUrl: ReturnType<typeof vi.fn>): string {
  expect(navigateByUrl).toHaveBeenCalledTimes(1);
  return String(navigateByUrl.mock.calls[0][0]);
}

describe('session expiry keeps the current URL (returnUrl)', () => {
  beforeEach(() => {
    localStorage.clear();
  });

  afterEach(() => {
    vi.useRealTimers();
    vi.unstubAllGlobals();
    localStorage.clear();
  });

  it('expiry on a deep URL captures it as returnUrl', () => {
    const { auth, navigateByUrl } = makeAuth('/initiatives/abc-123?tab=phases');
    auth.logout('expired');
    expect(destination(navigateByUrl)).toBe(
      '/login?returnUrl=' + encodeURIComponent('/initiatives/abc-123?tab=phases'),
    );
  });

  it('a 401 on refresh (the real in-tab expiry path) captures the deep URL', async () => {
    const now = 1_700_000_000_000;
    vi.useFakeTimers();
    vi.setSystemTime(now);
    const { auth, navigateByUrl } = makeAuth('/terminal?session=s1');
    plantDeadSession(now);
    vi.stubGlobal(
      'fetch',
      vi.fn().mockResolvedValue({ ok: false, status: 401, statusText: 'Unauthorized' }),
    );

    await expect(auth.ensureFreshToken(API)).rejects.toMatchObject({ kind: 'auth' });

    expect(auth.getAccessToken()).toBeNull();
    expect(destination(navigateByUrl)).toBe(
      '/login?returnUrl=' + encodeURIComponent('/terminal?session=s1'),
    );
  });

  it('expiry while already on /login does not self-reference', () => {
    for (const at of ['/login', '/login?returnUrl=%2Fterminal', '/login#x']) {
      const { auth, navigateByUrl } = makeAuth(at);
      auth.logout('expired');
      expect(destination(navigateByUrl)).toBe('/login');
    }
  });

  it('expiry while the user is GENUINELY on the root URL captures nothing', () => {
    // Both sources agree on root, so this is the real "user is on /" case and
    // not the bootstrap one where Router.url is '/' only because the router has
    // not read the address bar yet.
    for (const at of ['/', '']) {
      const { auth, navigateByUrl } = makeAuth(at, at);
      auth.logout('expired');
      expect(destination(navigateByUrl)).toBe('/login');
    }
  });

  it('the browser URL is the source of truth, NOT Router.url', () => {
    // Bootstrap shape: the address bar holds the deep URL, Router.url is '/'.
    const { auth, navigateByUrl } = makeAuth('/shells/s1/transcript#tail', '/');
    auth.logout('expired');
    expect(destination(navigateByUrl)).toBe(
      '/login?returnUrl=' + encodeURIComponent('/shells/s1/transcript#tail'),
    );
  });

  it('a stale Router.url cannot resurrect a URL the browser has left', () => {
    // Inverse of the above: if Router.url were ever read, this would capture.
    const { auth, navigateByUrl } = makeAuth('/', '/initiatives/abc?tab=phases');
    auth.logout('expired');
    expect(destination(navigateByUrl)).toBe('/login');
  });

  it('BOOTSTRAP: a reload of a deep URL with a dead session and no refresh token keeps the URL', async () => {
    // The phone case. `startSession` is what `provideAppInitializer` awaits; it
    // takes the `token && expired && !hasRefresh` branch straight to
    // logout('expired'). Router.url is '/' here because the router has not run
    // `initialNavigation()` yet — and it never will, because this very
    // navigation sets `navigationId = 1` and the gate closes.
    const now = 1_700_000_000_000;
    vi.useFakeTimers();
    vi.setSystemTime(now);
    const deep = '/initiatives/abc-123?tab=phases';
    const { auth, navigateByUrl } = makeAuth(deep, '/');
    localStorage.clear();
    localStorage.setItem(AuthService.TOKEN_KEY, 'stale');
    localStorage.setItem(AuthService.EXPIRES_AT_KEY, String(now - 1));
    localStorage.setItem(AuthService.EXPIRES_IN_KEY, '900');
    localStorage.setItem(AuthService.TENANT_KEY, 't1');
    expect(auth.getRefreshToken()).toBeNull();
    const fetchSpy = vi.fn();
    vi.stubGlobal('fetch', fetchSpy);

    await auth.startSession(API);

    expect(fetchSpy).not.toHaveBeenCalled();
    expect(auth.isAuthenticated()).toBe(false);
    expect(destination(navigateByUrl)).toBe(
      '/login?returnUrl=' + encodeURIComponent(deep),
    );
  });

  it('BOOTSTRAP: a reload whose refresh also fails with 401 keeps the URL', async () => {
    const now = 1_700_000_000_000;
    vi.useFakeTimers();
    vi.setSystemTime(now);
    const deep = '/shells/s1/raw';
    const { auth, navigateByUrl } = makeAuth(deep, '/');
    plantDeadSession(now);
    vi.stubGlobal(
      'fetch',
      vi.fn().mockResolvedValue({ ok: false, status: 401, statusText: 'Unauthorized' }),
    );

    // startSession never rejects (it swallows the auth-class reject).
    await auth.startSession(API);

    expect(auth.getAccessToken()).toBeNull();
    expect(destination(navigateByUrl)).toBe(
      '/login?returnUrl=' + encodeURIComponent(deep),
    );
  });

  it('BOOTSTRAP: a reload of the root URL with a dead session captures nothing', async () => {
    const now = 1_700_000_000_000;
    vi.useFakeTimers();
    vi.setSystemTime(now);
    const { auth, navigateByUrl } = makeAuth('/', '/');
    localStorage.clear();
    localStorage.setItem(AuthService.TOKEN_KEY, 'stale');
    localStorage.setItem(AuthService.EXPIRES_AT_KEY, String(now - 1));
    localStorage.setItem(AuthService.EXPIRES_IN_KEY, '900');
    vi.stubGlobal('fetch', vi.fn());

    await auth.startSession(API);

    expect(destination(navigateByUrl)).toBe('/login');
  });

  it('a deliberate sign-out does NOT capture a returnUrl', () => {
    // settings.page.ts / terminal.page.ts both call `auth.logout()` with no argument.
    const { auth, navigateByUrl } = makeAuth('/settings');
    auth.logout();
    expect(destination(navigateByUrl)).toBe('/login');
  });

  it("an explicit 'user' reason does NOT capture a returnUrl", () => {
    const { auth, navigateByUrl } = makeAuth('/terminal?session=s1');
    auth.logout('user');
    expect(destination(navigateByUrl)).toBe('/login');
  });

  it('expiry still clears the stored session', () => {
    const now = 1_700_000_000_000;
    vi.useFakeTimers();
    vi.setSystemTime(now);
    const { auth } = makeAuth('/settings');
    plantDeadSession(now);
    auth.isAuthenticated.set(true);
    auth.logout('expired');
    expect(auth.getAccessToken()).toBeNull();
    expect(auth.getRefreshToken()).toBeNull();
    expect(auth.isAuthenticated()).toBe(false);
    expect(auth.currentUser()).toBeNull();
  });

  it('wiring: the two user-initiated call sites still call logout() with no reason', () => {
    for (const rel of ['../pages/settings/settings.page.ts', '../pages/terminal/terminal.page.ts']) {
      const src = readFileSync(resolve(here, rel), 'utf8');
      expect(src).toMatch(/this\.auth\.logout\(\s*\)/);
      expect(src).not.toMatch(/this\.auth\.logout\(\s*['"]expired['"]/);
    }
  });

  it('wiring: the expiry path reads Location, never Router.url', () => {
    const src = readFileSync(resolve(here, 'auth.service.ts'), 'utf8');
    expect(src).toMatch(/import \{ Location \} from '@angular\/common';/);
    expect(src).toMatch(/inject\(Location\)/);
    expect(src).toMatch(/this\.location\.path\(true\)/);
    // `Router.url` must not appear anywhere in the service: at bootstrap it is
    // '/' and would silently discard the deep URL.
    expect(src).not.toMatch(/this\.router\.url/);
  });

  it('wiring: startSession is still what provideAppInitializer awaits', () => {
    // The ordering this spec's BOOTSTRAP cases model. If this moves out of the
    // initializer, re-derive which URL source is correct before trusting them.
    const src = readFileSync(resolve(here, '../app.config.ts'), 'utf8');
    expect(src).toMatch(/provideAppInitializer\(/);
    expect(src).toMatch(/startSession\(/);
  });

  it("wiring: the Location stub above matches what @angular/common's Location.path(true) really returns", () => {
    // These specs stub `Location`, because the real one cannot be constructed
    // in bare jsdom (`BrowserPlatformLocation` needs a browser platform's DOM
    // adapter). So pin the contract against Angular's own source instead: the
    // stub is only trustworthy while this holds.
    //   PathLocationStrategy.path(includeHash):
    //     pathname + normalizeQueryParams(search) (+ hash when includeHash)
    //   Location.path(includeHash): normalize(strategy.path(includeHash))
    // i.e. pathname + search + hash, base href stripped — exactly the string
    // `Router.initialNavigation()` would have navigated to.
    const bundle = readAngularBundle('@angular/common', /^location-.*\.mjs$/);
    expect(bundle).toMatch(
      /path\(includeHash = false\) \{\s*const pathname = this\._platformLocation\.pathname \+ normalizeQueryParams\(this\._platformLocation\.search\);\s*const hash = this\._platformLocation\.hash;\s*return hash && includeHash \? `\$\{pathname\}\$\{hash\}` : pathname;/,
    );
    expect(bundle).toMatch(
      /path\(includeHash = false\) \{\s*return this\.normalize\(this\._locationStrategy\.path\(includeHash\)\);/,
    );
  });

  it('wiring: Router.initialNavigation is still gated on hasRequestedNavigation', () => {
    // The reason the guard cannot compensate at bootstrap: the expiry logout's
    // own navigateByUrl bumps navigationId, closing this gate before the
    // address bar is ever read. If Angular drops the gate, revisit.
    const bundle = readAngularBundle('@angular/router', /^router-.*\.mjs$/);
    expect(bundle).toMatch(/if \(!this\.navigationTransitions\.hasRequestedNavigation\) \{/);
    expect(bundle).toMatch(/get hasRequestedNavigation\(\) \{\s*return this\.navigationId !== 0;/);
  });

  it('wiring: LoginPage validates the returnUrl query param instead of navigating to it raw', () => {
    const src = readFileSync(resolve(here, '../pages/login/login.page.ts'), 'utf8');
    expect(src).toMatch(/safeReturnUrl\(/);
    expect(src).not.toMatch(/queryParamMap\.get\('returnUrl'\)\s*\|\|/);
  });

  it('the guard still produces the same /login?returnUrl UrlTree (unchanged)', () => {
    const createUrlTree = vi.fn(
      (commands: unknown[], extras?: { queryParams?: Record<string, string> }) => ({
        commands,
        queryParams: extras?.queryParams,
      }),
    );
    const injector = {
      get(token: unknown) {
        if (token === AuthService) {
          return { syncAuthState: () => false, isAuthenticated: signal(false) };
        }
        if (token === Router) return { createUrlTree };
        throw new Error(`unexpected inject token: ${String(token)}`);
      },
    };
    runInInjectionContext(injector as Injector, () =>
      authGuard({} as never, { url: '/initiatives/abc?tab=phases' } as never),
    );
    expect(createUrlTree).toHaveBeenCalledWith(['/login'], {
      queryParams: { returnUrl: '/initiatives/abc?tab=phases' },
    });
  });
});
