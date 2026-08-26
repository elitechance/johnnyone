import '@angular/compiler';
import { describe, it, expect, vi, beforeEach, afterEach } from 'vitest';
import { readFileSync } from 'node:fs';
import { dirname, resolve } from 'node:path';
import { fileURLToPath } from 'node:url';
import { Injector, runInInjectionContext, signal } from '@angular/core';
import { Router } from '@angular/router';
import { authGuard } from './auth.guard';
import { AuthService } from './auth.service';
import { isSessionAuthenticated } from './auth-session-logic';

const here = dirname(fileURLToPath(import.meta.url));

function jwtWithExp(expSec: number): string {
  const header = Buffer.from(JSON.stringify({ alg: 'none', typ: 'JWT' })).toString('base64url');
  const payload = Buffer.from(JSON.stringify({ exp: expSec })).toString('base64url');
  return `${header}.${payload}.sig`;
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

function invokeGuard(auth: Pick<AuthService, 'syncAuthState' | 'isAuthenticated'>): unknown {
  const routerStub = {
    createUrlTree(commands: unknown[], extras?: { queryParams?: Record<string, string> }) {
      return {
        commands,
        queryParams: extras?.queryParams,
        toString() {
          const path = '/' + (commands as string[]).filter(Boolean).join('/').replace(/^\//, '');
          const ru = extras?.queryParams?.['returnUrl'];
          return ru ? `${path}?returnUrl=${encodeURIComponent(ru)}` : path;
        },
      };
    },
  };
  const injector = {
    get(token: unknown) {
      if (token === AuthService) return auth;
      if (token === Router) return routerStub;
      throw new Error(`unexpected inject token: ${String(token)}`);
    },
  };
  return runInInjectionContext(injector as Injector, () =>
    authGuard({} as never, { url: '/settings' } as never),
  );
}

describe('authGuard expired-token redirect (988-hour complaint)', () => {
  beforeEach(() => {
    localStorage.clear();
  });

  afterEach(() => {
    vi.useRealTimers();
    localStorage.clear();
  });

  it('plants a token with exp in the past and proves the session is not authenticated', () => {
    const now = 1_700_000_000_000;
    const pastExp = now - 988 * 3600 * 1000;
    expect(isSessionAuthenticated(true, pastExp, now)).toBe(false);
  });

  it('authGuard returns /login when not authenticated', () => {
    const result = invokeGuard({
      isAuthenticated: signal(false),
      syncAuthState: () => false,
    } as Pick<AuthService, 'syncAuthState' | 'isAuthenticated'>);
    expect(result).not.toBe(true);
    expect(String(result)).toMatch(/^\/login/);
    expect((result as { commands: string[] }).commands).toEqual(['/login']);
  });

  it('live token with future exp returns true', () => {
    const now = 1_700_000_000_000;
    vi.useFakeTimers();
    vi.setSystemTime(now);
    const auth = makeAuth();
    localStorage.setItem(AuthService.TOKEN_KEY, jwtWithExp(Math.floor((now + 600_000) / 1000)));
    localStorage.setItem(AuthService.EXPIRES_AT_KEY, String(now + 600_000));
    localStorage.setItem(AuthService.EXPIRES_IN_KEY, '900');
    expect(invokeGuard(auth)).toBe(true);
  });

  it('past-exp stored token → authGuard UrlTree to /login', () => {
    const now = 1_700_000_000_000;
    vi.useFakeTimers();
    vi.setSystemTime(now);
    const auth = makeAuth();
    localStorage.setItem(AuthService.TOKEN_KEY, jwtWithExp(Math.floor((now - 60_000) / 1000)));
    localStorage.setItem(AuthService.EXPIRES_AT_KEY, String(now - 60_000));
    localStorage.setItem(AuthService.EXPIRES_IN_KEY, '900');
    auth.isAuthenticated.set(true);
    const result = invokeGuard(auth);
    expect(result).not.toBe(true);
    expect(String(result)).toMatch(/^\/login/);
  });

  it('(j) in-tab expiry after transport-class refresh still redirects to /login', async () => {
    const now = 1_000_000;
    vi.useFakeTimers();
    vi.setSystemTime(now);
    const auth = makeAuth();
    localStorage.setItem(AuthService.TOKEN_KEY, jwtWithExp(Math.floor((now + 60_000) / 1000)));
    localStorage.setItem(AuthService.REFRESH_TOKEN_KEY, 'r');
    localStorage.setItem(AuthService.EXPIRES_AT_KEY, String(now + 60_000));
    localStorage.setItem(AuthService.EXPIRES_IN_KEY, '60');
    auth.syncAuthState();
    expect(auth.isAuthenticated()).toBe(true);
    expect(invokeGuard(auth)).toBe(true);

    auth.refresh = vi.fn().mockRejectedValue(new TypeError('network'));
    const logout = vi.spyOn(auth, 'logout');
    vi.setSystemTime(now + 60_001);
    await expect(auth.ensureFreshToken('http://example.test/graphql')).rejects.toBeTruthy();
    expect(logout).not.toHaveBeenCalled();
    expect(auth.isAuthenticated()).toBe(false);
    expect(auth.isSessionLive()).toBe(false);

    const result = invokeGuard(auth);
    expect(result).not.toBe(true);
    expect(String(result)).toMatch(/^\/login/);
  });

  it('wiring: AuthService.isAuthenticated is no longer hasToken() alone', () => {
    const serviceSrc = readFileSync(resolve(here, 'auth.service.ts'), 'utf8');
    const guardSrc = readFileSync(resolve(here, 'auth.guard.ts'), 'utf8');
    expect(serviceSrc).toMatch(/syncAuthState\s*\(/);
    expect(serviceSrc).toMatch(/isSessionLive\s*\(|isTokenNearExpiry\(\s*0\s*\)|isSessionAuthenticated/);
    expect(serviceSrc).not.toMatch(/isAuthenticated\s*=\s*signal\(\s*this\.hasToken\(\)\s*\)/);
    expect(guardSrc).toMatch(/syncAuthState\s*\(|isSessionLive\s*\(/);
  });
});
