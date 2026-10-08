/**
 * Tab-wake refresh (fix/web-session-resume, part 1).
 *
 * A phone browser suspends a background tab, so the 80%-of-lifetime
 * `setTimeout` armed by `armRefreshTimer` cannot fire. On resume after more
 * than the access token's 15-minute life the stored access token is dead, yet
 * nothing calls `ensureFreshToken` because a resume is not a cold boot. These
 * specs pin the wake handlers that close that hole.
 *
 * Fixture pattern follows `auth.service.session.spec.ts`: the service is built
 * with `Object.create` so no Angular injector is needed (the web vitest config
 * has no Angular plugin).
 */
import { describe, it, expect, vi, beforeEach, afterEach } from 'vitest';
import { signal } from '@angular/core';
import '@angular/compiler';
import { AuthService } from './auth.service';

const API = 'http://example.test/graphql';

const SAMPLE_USER = {
  id: 'u1',
  tenantId: 't1',
  email: 'a@b.c',
  displayName: 'A',
  roles: ['admin'],
  status: 'active',
};

function plantSession(opts: {
  access?: string;
  refresh?: string | null;
  expiresAt?: number | null;
  expiresIn?: number | null;
}): void {
  localStorage.clear();
  if (opts.access !== undefined) {
    localStorage.setItem(AuthService.TOKEN_KEY, opts.access);
  }
  if (opts.refresh) {
    localStorage.setItem(AuthService.REFRESH_TOKEN_KEY, opts.refresh);
  }
  if (opts.expiresAt != null) {
    localStorage.setItem(AuthService.EXPIRES_AT_KEY, String(opts.expiresAt));
  }
  if (opts.expiresIn != null) {
    localStorage.setItem(AuthService.EXPIRES_IN_KEY, String(opts.expiresIn));
  }
  localStorage.setItem(AuthService.USER_KEY, JSON.stringify(SAMPLE_USER));
  localStorage.setItem(AuthService.USER_ID_KEY, SAMPLE_USER.id);
  localStorage.setItem(AuthService.TENANT_KEY, SAMPLE_USER.tenantId);
}

function makeAuth(): AuthService {
  const inst = Object.create(AuthService.prototype) as AuthService;
  (inst as any).router = { navigateByUrl: vi.fn().mockResolvedValue(true) };
  (inst as any).isAuthenticated = signal(false);
  (inst as any).currentUser = signal(null);
  (inst as any).inflightEnsure = null;
  (inst as any).refreshTimer = null;
  (inst as any).lastApiUrl = null;
  (inst as any).timerBackoffMs = 0;
  (inst as any).wakeHandler = null;
  return inst;
}

/** Arm the wake handlers the way `startSession()` does, with a known apiUrl. */
function installWake(auth: AuthService, apiUrl: string | null = API): void {
  (auth as any).lastApiUrl = apiUrl;
  (auth as any).installWakeHandlers();
}

function setVisibility(state: 'visible' | 'hidden'): void {
  Object.defineProperty(document, 'visibilityState', {
    value: state,
    configurable: true,
  });
}

function wake(kind: 'visibilitychange' | 'pageshow' = 'visibilitychange'): void {
  if (kind === 'visibilitychange') {
    document.dispatchEvent(new Event('visibilitychange'));
  } else {
    window.dispatchEvent(new Event('pageshow'));
  }
}

/** Let the ensureFreshToken promise chain settle (no real timers involved). */
async function settle(): Promise<void> {
  for (let i = 0; i < 6; i++) await Promise.resolve();
}

describe('AuthService wake refresh', () => {
  let auth: AuthService;

  beforeEach(() => {
    localStorage.clear();
    vi.restoreAllMocks();
    setVisibility('visible');
  });

  afterEach(() => {
    try {
      (auth as any)?.removeWakeHandlers?.();
    } catch {
      /* ignore */
    }
    vi.useRealTimers();
    localStorage.clear();
  });

  it('wake with a fresh token → no refresh call (ensureFreshToken is a no-op)', async () => {
    const now = 1_700_000_000_000;
    vi.useFakeTimers();
    vi.setSystemTime(now);
    auth = makeAuth();
    // Issued now, expires in 900s → 0% through its life, far from the 80% mark.
    plantSession({
      access: 'fresh',
      refresh: 'r',
      expiresAt: now + 900_000,
      expiresIn: 900,
    });
    auth.refresh = vi.fn().mockResolvedValue(undefined);
    const ensureSpy = vi.spyOn(auth, 'ensureFreshToken');
    installWake(auth);

    wake();
    await settle();

    expect(ensureSpy).toHaveBeenCalledTimes(1);
    expect(auth.refresh).toHaveBeenCalledTimes(0);
  });

  it('wake with a fresh token re-arms the refresh timer (a suspended timer has an unreliable delay)', async () => {
    const now = 1_700_000_000_000;
    vi.useFakeTimers();
    vi.setSystemTime(now);
    auth = makeAuth();
    plantSession({
      access: 'fresh',
      refresh: 'r',
      expiresAt: now + 900_000,
      expiresIn: 900,
    });
    auth.refresh = vi.fn().mockResolvedValue(undefined);
    installWake(auth);
    const armSpy = vi.spyOn(auth as any, 'armRefreshTimer');

    wake();
    await settle();

    expect(armSpy).toHaveBeenCalled();
  });

  it('wake with a near-expiry token (past the 80% mark) → exactly one refresh', async () => {
    const now = 1_700_000_000_000;
    vi.useFakeTimers();
    vi.setSystemTime(now);
    auth = makeAuth();
    // issued = expiresAt - 900_000; 850s elapsed of 900s → 94% > 80%.
    plantSession({
      access: 'stale-ish',
      refresh: 'r',
      expiresAt: now + 50_000,
      expiresIn: 900,
    });
    auth.refresh = vi.fn().mockImplementation(async () => {
      localStorage.setItem(AuthService.TOKEN_KEY, 'renewed');
      localStorage.setItem(AuthService.EXPIRES_AT_KEY, String(now + 900_000));
    });
    installWake(auth);

    wake();
    await settle();

    expect(auth.refresh).toHaveBeenCalledTimes(1);
    expect(auth.refresh).toHaveBeenCalledWith(API);
  });

  it('wake with an already-expired token → exactly one refresh', async () => {
    const now = 1_700_000_000_000;
    vi.useFakeTimers();
    vi.setSystemTime(now);
    auth = makeAuth();
    plantSession({
      access: 'dead',
      refresh: 'r',
      expiresAt: now - 20 * 60_000,
      expiresIn: 900,
    });
    auth.refresh = vi.fn().mockImplementation(async () => {
      localStorage.setItem(AuthService.TOKEN_KEY, 'renewed');
      localStorage.setItem(AuthService.EXPIRES_AT_KEY, String(now + 900_000));
    });
    installWake(auth);

    wake();
    await settle();

    expect(auth.refresh).toHaveBeenCalledTimes(1);
  });

  it('two wake events while the refresh is in flight → one refresh, not two', async () => {
    const now = 1_700_000_000_000;
    vi.useFakeTimers();
    vi.setSystemTime(now);
    auth = makeAuth();
    plantSession({
      access: 'dead',
      refresh: 'r',
      expiresAt: now - 20 * 60_000,
      expiresIn: 900,
    });
    let release: (() => void) | null = null;
    auth.refresh = vi.fn().mockImplementation(
      () =>
        new Promise<void>((resolve) => {
          release = () => {
            localStorage.setItem(AuthService.TOKEN_KEY, 'renewed');
            localStorage.setItem(AuthService.EXPIRES_AT_KEY, String(now + 900_000));
            resolve();
          };
        }),
    );
    installWake(auth);

    wake('visibilitychange');
    wake('pageshow');
    wake('visibilitychange');

    expect(auth.refresh).toHaveBeenCalledTimes(1);
    release!();
    await settle();
    expect(auth.refresh).toHaveBeenCalledTimes(1);
  });

  it('two wake events after the refresh settled → still one refresh (token is now fresh)', async () => {
    const now = 1_700_000_000_000;
    vi.useFakeTimers();
    vi.setSystemTime(now);
    auth = makeAuth();
    plantSession({
      access: 'dead',
      refresh: 'r',
      expiresAt: now - 20 * 60_000,
      expiresIn: 900,
    });
    auth.refresh = vi.fn().mockImplementation(async () => {
      localStorage.setItem(AuthService.TOKEN_KEY, 'renewed');
      localStorage.setItem(AuthService.EXPIRES_AT_KEY, String(now + 900_000));
    });
    installWake(auth);

    wake();
    await settle();
    wake();
    await settle();

    expect(auth.refresh).toHaveBeenCalledTimes(1);
  });

  it('pageshow alone (bfcache restore, no visibilitychange) triggers the refresh', async () => {
    const now = 1_700_000_000_000;
    vi.useFakeTimers();
    vi.setSystemTime(now);
    auth = makeAuth();
    plantSession({
      access: 'dead',
      refresh: 'r',
      expiresAt: now - 20 * 60_000,
      expiresIn: 900,
    });
    auth.refresh = vi.fn().mockResolvedValue(undefined);
    installWake(auth);

    wake('pageshow');
    await settle();

    expect(auth.refresh).toHaveBeenCalledTimes(1);
  });

  it('becoming hidden is not a wake → no refresh', async () => {
    const now = 1_700_000_000_000;
    vi.useFakeTimers();
    vi.setSystemTime(now);
    auth = makeAuth();
    plantSession({
      access: 'dead',
      refresh: 'r',
      expiresAt: now - 20 * 60_000,
      expiresIn: 900,
    });
    auth.refresh = vi.fn().mockResolvedValue(undefined);
    installWake(auth);
    setVisibility('hidden');

    wake();
    await settle();

    expect(auth.refresh).toHaveBeenCalledTimes(0);
  });

  it('no stored session (signed out) → wake does nothing', async () => {
    const now = 1_700_000_000_000;
    vi.useFakeTimers();
    vi.setSystemTime(now);
    auth = makeAuth();
    localStorage.clear();
    auth.refresh = vi.fn().mockResolvedValue(undefined);
    const ensureSpy = vi.spyOn(auth, 'ensureFreshToken');
    installWake(auth);

    wake();
    await settle();

    expect(ensureSpy).toHaveBeenCalledTimes(0);
    expect(auth.refresh).toHaveBeenCalledTimes(0);
  });

  it('unknown lastApiUrl → wake does nothing', async () => {
    const now = 1_700_000_000_000;
    vi.useFakeTimers();
    vi.setSystemTime(now);
    auth = makeAuth();
    plantSession({
      access: 'dead',
      refresh: 'r',
      expiresAt: now - 20 * 60_000,
      expiresIn: 900,
    });
    auth.refresh = vi.fn().mockResolvedValue(undefined);
    const ensureSpy = vi.spyOn(auth, 'ensureFreshToken');
    installWake(auth, null);

    wake();
    await settle();

    expect(ensureSpy).toHaveBeenCalledTimes(0);
  });

  it('an auth-class refresh failure on wake does not throw out of the handler', async () => {
    const now = 1_700_000_000_000;
    vi.useFakeTimers();
    vi.setSystemTime(now);
    auth = makeAuth();
    plantSession({
      access: 'dead',
      refresh: 'r',
      expiresAt: now - 20 * 60_000,
      expiresIn: 900,
    });
    auth.refresh = vi.fn().mockRejectedValue(new Error('Unauthorized'));
    auth.logout = vi.fn();
    installWake(auth);

    expect(() => wake()).not.toThrow();
    await settle();

    expect(auth.refresh).toHaveBeenCalledTimes(1);
    expect(auth.logout).toHaveBeenCalled();
  });

  it('ngOnDestroy removes the listeners → a later wake does nothing', async () => {
    const now = 1_700_000_000_000;
    vi.useFakeTimers();
    vi.setSystemTime(now);
    auth = makeAuth();
    plantSession({
      access: 'dead',
      refresh: 'r',
      expiresAt: now - 20 * 60_000,
      expiresIn: 900,
    });
    auth.refresh = vi.fn().mockResolvedValue(undefined);
    installWake(auth);
    (auth as any).ngOnDestroy();

    wake('visibilitychange');
    wake('pageshow');
    await settle();

    expect(auth.refresh).toHaveBeenCalledTimes(0);
  });

  it('installWakeHandlers is idempotent → one wake event still refreshes once', async () => {
    const now = 1_700_000_000_000;
    vi.useFakeTimers();
    vi.setSystemTime(now);
    auth = makeAuth();
    plantSession({
      access: 'dead',
      refresh: 'r',
      expiresAt: now - 20 * 60_000,
      expiresIn: 900,
    });
    auth.refresh = vi.fn().mockResolvedValue(undefined);
    installWake(auth);
    (auth as any).installWakeHandlers();
    (auth as any).installWakeHandlers();

    wake();
    await settle();

    expect(auth.refresh).toHaveBeenCalledTimes(1);
  });

  it('startSession installs the wake handlers (production entry point)', async () => {
    const now = 1_700_000_000_000;
    vi.useFakeTimers();
    vi.setSystemTime(now);
    auth = makeAuth();
    plantSession({
      access: 'fresh',
      refresh: 'r',
      expiresAt: now + 900_000,
      expiresIn: 900,
    });
    auth.refresh = vi.fn().mockResolvedValue(undefined);
    await auth.startSession(API);
    expect((auth as any).wakeHandler).toBeTruthy();

    // Now expire the token and wake.
    localStorage.setItem(AuthService.EXPIRES_AT_KEY, String(now - 60_000));
    wake();
    await settle();

    expect(auth.refresh).toHaveBeenCalledTimes(1);
  });

  describe('ensureFreshToken single-flight (verified, not assumed)', () => {
    /**
     * MEASURED, not assumed: `ensureFreshToken` is declared `async`, so each
     * call returns its own wrapper promise that *adopts* `inflightEnsure`
     * (`expect(second).toBe(first)` fails on Object.is — checked). The dedupe
     * property that actually holds, and the one the wake handler relies on, is
     * that the guard is assigned synchronously before any await, so a
     * concurrent call performs no second network refresh.
     */
    it('a second call while the first is in flight shares the in-flight refresh and does not re-refresh', async () => {
      const now = 1_700_000_000_000;
      vi.useFakeTimers();
      vi.setSystemTime(now);
      auth = makeAuth();
      plantSession({
        access: 'dead',
        refresh: 'r',
        expiresAt: now - 20 * 60_000,
        expiresIn: 900,
      });
      let resolveRefresh: (() => void) | null = null;
      auth.refresh = vi.fn().mockImplementation(
        () => new Promise<void>((resolve) => { resolveRefresh = resolve; }),
      );

      const first = auth.ensureFreshToken(API);
      const second = auth.ensureFreshToken(API);
      const third = auth.ensureFreshToken(API);

      // The guard is set synchronously before any await, so calls 2 and 3 take
      // the early-return branch and never start a second refresh.
      expect(auth.refresh).toHaveBeenCalledTimes(1);
      expect((auth as any).inflightEnsure).not.toBeNull();

      localStorage.setItem(AuthService.TOKEN_KEY, 'renewed');
      localStorage.setItem(AuthService.EXPIRES_AT_KEY, String(now + 900_000));
      resolveRefresh!();
      await Promise.all([first, second, third]);
      expect(auth.refresh).toHaveBeenCalledTimes(1);
    });
  });
});
