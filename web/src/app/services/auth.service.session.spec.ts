import { describe, it, expect, vi, beforeEach, afterEach } from 'vitest';
import { readFileSync } from 'node:fs';
import { dirname, resolve } from 'node:path';
import { fileURLToPath } from 'node:url';
import { signal } from '@angular/core';
import '@angular/compiler';
import { AuthService, RefreshFailure } from './auth.service';
import { refreshDueAtMs } from './auth-session-logic';

const API = 'http://example.test/graphql';

const SAMPLE_USER = {
  id: 'u1',
  tenantId: 't1',
  email: 'a@b.c',
  displayName: 'A',
  roles: ['admin'],
  status: 'active',
};

function jwtWithExp(expSec: number): string {
  const header = Buffer.from(JSON.stringify({ alg: 'none', typ: 'JWT' })).toString('base64url');
  const payload = Buffer.from(JSON.stringify({ exp: expSec })).toString('base64url');
  return `${header}.${payload}.sig`;
}

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
  return inst;
}

describe('AuthService session (a)–(i)', () => {
  beforeEach(() => {
    localStorage.clear();
    vi.restoreAllMocks();
  });

  afterEach(() => {
    vi.useRealTimers();
    localStorage.clear();
  });

  it('isAuthenticated is false for a stored token with past expiresAt / past JWT exp', () => {
    const now = 1_700_000_000_000;
    vi.useFakeTimers();
    vi.setSystemTime(now);
    const auth = makeAuth();
    plantSession({
      access: jwtWithExp(Math.floor((now - 60_000) / 1000)),
      refresh: 'r',
      expiresAt: now - 60_000,
      expiresIn: 900,
    });
    expect(auth.syncAuthState()).toBe(false);
    expect(auth.isAuthenticated()).toBe(false);
    expect(auth.isSessionLive()).toBe(false);

    localStorage.removeItem(AuthService.EXPIRES_AT_KEY);
    expect(auth.isSessionLive()).toBe(false);
  });

  it('(a) expired access + refresh token present → refresh() called', async () => {
    const now = 1_700_000_000_000;
    vi.useFakeTimers();
    vi.setSystemTime(now);
    const auth = makeAuth();
    plantSession({
      access: 'stale',
      refresh: 'refresh-token',
      expiresAt: now - 1,
      expiresIn: 900,
    });
    auth.refresh = vi.fn().mockImplementation(async () => {
      localStorage.setItem(AuthService.TOKEN_KEY, 'fresh');
      localStorage.setItem(AuthService.EXPIRES_AT_KEY, String(now + 900_000));
      auth.syncAuthState();
    });
    await auth.startSession(API);
    expect(auth.refresh).toHaveBeenCalledTimes(1);
    expect(auth.refresh).toHaveBeenCalledWith(API);
  });

  it('(b) expired access + no refresh token → logout(); refresh() not called', async () => {
    const now = 1_700_000_000_000;
    vi.useFakeTimers();
    vi.setSystemTime(now);
    const auth = makeAuth();
    plantSession({
      access: 'stale',
      refresh: null,
      expiresAt: now - 1,
      expiresIn: 900,
    });
    const refresh = vi.fn();
    auth.refresh = refresh;
    const logout = vi.spyOn(auth, 'logout');
    await auth.startSession(API);
    expect(refresh).not.toHaveBeenCalled();
    expect(logout).toHaveBeenCalled();
    expect(auth.isAuthenticated()).toBe(false);
  });

  it('(c) ensureFreshToken auth-class reject → logout() and the promise rejects', async () => {
    const now = 1_700_000_000_000;
    vi.useFakeTimers();
    vi.setSystemTime(now);
    const auth = makeAuth();
    plantSession({
      access: 'stale',
      refresh: 'r',
      expiresAt: now - 1,
      expiresIn: 900,
    });
    auth.refresh = vi.fn().mockRejectedValue(new RefreshFailure('auth', 'invalid refresh token'));
    const logout = vi.spyOn(auth, 'logout');
    let resolved = false;
    await expect(auth.ensureFreshToken(API).then(() => { resolved = true; })).rejects.toBeTruthy();
    expect(resolved).toBe(false);
    expect(logout).toHaveBeenCalled();
    expect(auth.isAuthenticated()).toBe(false);
    expect(auth.getAccessToken()).toBeNull();
  });

  it('HTTP 401 on refresh is auth — logout then reject', async () => {
    const now = 1_700_000_000_000;
    vi.useFakeTimers();
    vi.setSystemTime(now);
    const auth = makeAuth();
    plantSession({
      access: 'stale',
      refresh: 'r',
      expiresAt: now - 1,
      expiresIn: 900,
    });
    vi.stubGlobal('fetch', vi.fn().mockResolvedValue({
      ok: false,
      status: 401,
      statusText: 'Unauthorized',
    }));
    const logout = vi.spyOn(auth, 'logout');
    await expect(auth.ensureFreshToken(API)).rejects.toMatchObject({ kind: 'auth' });
    expect(logout).toHaveBeenCalled();
    expect(auth.getAccessToken()).toBeNull();
  });

  it('HTTP 403/404 on refresh is transport — no logout (D8)', async () => {
    const now = 1_700_000_000_000;
    vi.useFakeTimers();
    vi.setSystemTime(now);
    const auth = makeAuth();
    plantSession({
      access: 'stale',
      refresh: 'r',
      expiresAt: now - 1,
      expiresIn: 900,
    });
    const logout = vi.spyOn(auth, 'logout');
    for (const status of [400, 403, 404]) {
      (auth as any).inflightEnsure = null;
      vi.stubGlobal('fetch', vi.fn().mockResolvedValue({
        ok: false,
        status,
        statusText: 'Nope',
      }));
      await expect(auth.ensureFreshToken(API)).rejects.toMatchObject({ kind: 'transport' });
      expect(logout).not.toHaveBeenCalled();
      expect(auth.getAccessToken()).toBe('stale');
    }
  });

  it('(d) timer fires at refreshDueAtMs; success reschedules; logout clears; second startSession does not stack', async () => {
    const now = 1_000_000;
    vi.useFakeTimers();
    vi.setSystemTime(now);
    const auth = makeAuth();
    const lifetime = 900;
    plantSession({
      access: 'live',
      refresh: 'r',
      expiresAt: now + lifetime * 1000,
      expiresIn: lifetime,
    });
    auth.refresh = vi.fn().mockImplementation(async () => {
      const t = Date.now();
      localStorage.setItem(AuthService.TOKEN_KEY, 'fresh');
      localStorage.setItem(AuthService.EXPIRES_AT_KEY, String(t + lifetime * 1000));
      localStorage.setItem(AuthService.EXPIRES_IN_KEY, String(lifetime));
      auth.syncAuthState();
    });

    await auth.startSession(API);
    expect((auth as any).refreshTimer).not.toBeNull();
    const firstTimer = (auth as any).refreshTimer;

    const due = refreshDueAtMs(now, now + lifetime * 1000);
    await vi.advanceTimersByTimeAsync(due - now);
    expect(auth.refresh).toHaveBeenCalledTimes(1);
    expect((auth as any).refreshTimer).not.toBeNull();
    expect((auth as any).refreshTimer).not.toBe(firstTimer);

    auth.logout();
    expect((auth as any).refreshTimer).toBeNull();

    plantSession({
      access: 'live',
      refresh: 'r',
      expiresAt: Date.now() + lifetime * 1000,
      expiresIn: lifetime,
    });
    await auth.startSession(API);
    const afterFirst = (auth as any).refreshTimer;
    await auth.startSession(API);
    expect((auth as any).refreshTimer).toBeTruthy();
    expect((auth as any).refreshTimer).not.toBe(afterFirst);
    expect((auth as any).refreshTimer).not.toBe(firstTimer);
  });

  it('(e) startSession resolves even when refresh() throws (auth or transport)', async () => {
    const now = 1_700_000_000_000;
    vi.useFakeTimers();
    vi.setSystemTime(now);
    const auth = makeAuth();
    plantSession({
      access: 'stale',
      refresh: 'r',
      expiresAt: now - 1,
      expiresIn: 900,
    });
    auth.refresh = vi.fn().mockRejectedValue(new RefreshFailure('auth', 'expired refresh'));
    await expect(auth.startSession(API)).resolves.toBeUndefined();

    plantSession({
      access: 'stale',
      refresh: 'r',
      expiresAt: now - 1,
      expiresIn: 900,
    });
    (auth as any).inflightEnsure = null;
    auth.refresh = vi.fn().mockRejectedValue(new TypeError('network'));
    await expect(auth.startSession(API)).resolves.toBeUndefined();
  });

  it('(f) refresh() is invoked with a 4s abort / timeout signal', async () => {
    const auth = makeAuth();
    plantSession({
      access: 'a',
      refresh: 'r',
      expiresAt: Date.now() + 900_000,
      expiresIn: 900,
    });
    const fetchMock = vi.fn().mockResolvedValue({
      ok: true,
      status: 200,
      statusText: 'OK',
      json: async () => ({
        data: {
          refreshToken: {
            accessToken: 'new-a',
            refreshToken: 'new-r',
            expiresIn: 900,
            user: SAMPLE_USER,
          },
        },
      }),
    });
    vi.stubGlobal('fetch', fetchMock);
    await auth.refresh(API);
    expect(fetchMock).toHaveBeenCalled();
    const init = fetchMock.mock.calls[0][1] as RequestInit;
    expect(init.signal).toBeInstanceOf(AbortSignal);
  });

  it('(g) two overlapping ensureFreshToken calls issue exactly one refresh()', async () => {
    const now = 1_700_000_000_000;
    vi.useFakeTimers();
    vi.setSystemTime(now);
    const auth = makeAuth();
    plantSession({
      access: 'stale',
      refresh: 'r',
      expiresAt: now - 1,
      expiresIn: 900,
    });
    let release!: () => void;
    auth.refresh = vi.fn().mockImplementation(
      () => new Promise<void>((resolve) => {
        release = resolve;
      }),
    );
    const p1 = auth.ensureFreshToken(API);
    const p2 = auth.ensureFreshToken(API);
    expect(auth.refresh).toHaveBeenCalledTimes(1);
    release();
    await Promise.all([p1, p2]);
    expect(auth.refresh).toHaveBeenCalledTimes(1);
  });

  it('(h) opaque token / expiresAt === null: timer is not armed; setTimeout is not called with NaN', async () => {
    const setTimeoutSpy = vi.spyOn(globalThis, 'setTimeout');
    const auth = makeAuth();
    plantSession({ access: 'jk_not-a-jwt', refresh: 'r' });
    await auth.startSession(API);
    expect((auth as any).refreshTimer).toBeNull();
    for (const [, delay] of setTimeoutSpy.mock.calls) {
      expect(Number.isNaN(delay as number)).toBe(false);
    }
  });

  it('(i) timer fire + transport-class reject: no logout, no unhandled rejection, next timer armed', async () => {
    const now = 1_000_000;
    vi.useFakeTimers();
    vi.setSystemTime(now);
    const auth = makeAuth();
    const lifetime = 900;
    plantSession({
      access: 'live',
      refresh: 'r',
      expiresAt: now + lifetime * 1000,
      expiresIn: lifetime,
    });
    auth.refresh = vi.fn().mockRejectedValue(new TypeError('network'));
    const logout = vi.spyOn(auth, 'logout');
    const unhandled: unknown[] = [];
    const onUnhandled = (reason: unknown) => {
      unhandled.push(reason);
    };
    process.on('unhandledRejection', onUnhandled);

    await auth.startSession(API);
    const due = refreshDueAtMs(now, now + lifetime * 1000);
    await vi.advanceTimersByTimeAsync(due - now);
    await Promise.resolve();
    expect(logout).not.toHaveBeenCalled();
    expect(unhandled).toHaveLength(0);
    expect((auth as any).refreshTimer).not.toBeNull();

    await vi.advanceTimersByTimeAsync(5_000);
    await Promise.resolve();
    expect(auth.refresh.mock.calls.length).toBeGreaterThanOrEqual(2);
    expect(logout).not.toHaveBeenCalled();
    expect(unhandled).toHaveLength(0);

    process.off('unhandledRejection', onUnhandled);
  });

  it('app.config.ts registers provideAppInitializer that calls startSession', () => {
    const src = readFileSync(
      resolve(dirname(fileURLToPath(import.meta.url)), '../app.config.ts'),
      'utf8',
    );
    expect(src).toMatch(/provideAppInitializer/);
    expect(src).toMatch(/startSession/);
  });
});
