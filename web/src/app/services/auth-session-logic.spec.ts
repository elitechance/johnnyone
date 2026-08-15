import { describe, it, expect } from 'vitest';
import { readFileSync } from 'node:fs';
import { dirname, resolve } from 'node:path';
import { fileURLToPath } from 'node:url';
import {
  issuedAtMs,
  isSessionAuthenticated,
  isTokenExpired,
  MAX_TIMER_DELAY_MS,
  readJwtExpMs,
  refreshDueAtMs,
  resolveExpiresAtMs,
  shouldRefreshNow,
  timerDelayMs,
} from './auth-session-logic';

function jwtWithExp(expSec: number): string {
  const header = Buffer.from(JSON.stringify({ alg: 'none', typ: 'JWT' })).toString('base64url');
  const payload = Buffer.from(JSON.stringify({ exp: expSec })).toString('base64url');
  return `${header}.${payload}.sig`;
}

describe('auth-session-logic', () => {
  it('has no Angular / Ionic / localStorage import', () => {
    const src = readFileSync(
      resolve(dirname(fileURLToPath(import.meta.url)), 'auth-session-logic.ts'),
      'utf8',
    );
    const imports = src.split('\n').filter((line) => /^\s*import\b/.test(line)).join('\n');
    expect(imports).not.toMatch(/@angular\/|@ionic\/|localStorage/);
  });

  it('token with exp in the past → isSessionAuthenticated is false even if hasToken', () => {
    const now = 1_700_000_000_000;
    const expMs = now - 60_000;
    expect(isSessionAuthenticated(true, expMs, now)).toBe(false);
    expect(readJwtExpMs(jwtWithExp(Math.floor(expMs / 1000)))).toBe(Math.floor(expMs / 1000) * 1000);
  });

  it('token with exp 10 minutes ahead → true at now, false with bufferMs covering that window', () => {
    const now = 1_700_000_000_000;
    const expMs = now + 10 * 60 * 1000;
    expect(isSessionAuthenticated(true, expMs, now)).toBe(true);
    expect(isTokenExpired(expMs, now, 0)).toBe(false);
    expect(isTokenExpired(expMs, now, 10 * 60 * 1000 + 1)).toBe(true);
  });

  it('stored expiresAt in the past, no JWT parse needed → expired', () => {
    const now = 1_700_000_000_000;
    const stored = now - 1;
    expect(resolveExpiresAtMs(stored, 'jk_not-a-jwt')).toBe(stored);
    expect(isTokenExpired(resolveExpiresAtMs(stored, 'jk_not-a-jwt'), now, 0)).toBe(true);
    expect(isSessionAuthenticated(true, resolveExpiresAtMs(stored, 'jk_not-a-jwt'), now)).toBe(false);
  });

  it('no stored expiresAt, JWT exp in the past → expired (988-hour case)', () => {
    const now = 1_700_000_000_000;
    const pastSec = Math.floor((now - 988 * 3600 * 1000) / 1000);
    const token = jwtWithExp(pastSec);
    const resolved = resolveExpiresAtMs(null, token);
    expect(resolved).toBe(pastSec * 1000);
    expect(isSessionAuthenticated(true, resolved, now)).toBe(false);
  });

  it('opaque string jk_not-a-jwt + no stored expiry → not expired', () => {
    const now = 1_700_000_000_000;
    expect(readJwtExpMs('jk_not-a-jwt')).toBeNull();
    expect(resolveExpiresAtMs(null, 'jk_not-a-jwt')).toBeNull();
    expect(isTokenExpired(null, now, 0)).toBe(false);
    expect(isSessionAuthenticated(true, null, now)).toBe(true);
  });

  it('refreshDueAtMs for a 900s lifetime is 720s after issue', () => {
    const issued = 1_000_000;
    const expiresAt = issued + 900_000;
    expect(refreshDueAtMs(issued, expiresAt)).toBe(issued + 720_000);
  });

  it('issuedAtMs(1_000_000, 900, now) is 1_000_000 - 900_000', () => {
    expect(issuedAtMs(1_000_000, 900, 99)).toBe(1_000_000 - 900_000);
  });

  it('issuedAtMs(null, 900, now) and issuedAtMs(1_000_000, null, now) equal now', () => {
    const now = 42_000;
    expect(issuedAtMs(null, 900, now)).toBe(now);
    expect(issuedAtMs(1_000_000, null, now)).toBe(now);
  });

  it('timerDelayMs(..., null, now) is null', () => {
    expect(timerDelayMs(1, null, 10)).toBeNull();
  });

  it('timerDelayMs never returns NaN or a value >= 2**31', () => {
    const huge = Number.MAX_SAFE_INTEGER;
    const delay = timerDelayMs(0, huge, 0);
    expect(delay).not.toBeNull();
    expect(Number.isNaN(delay as number)).toBe(false);
    expect(delay as number).toBeLessThan(2 ** 31);
    expect(delay).toBe(MAX_TIMER_DELAY_MS);
    expect(timerDelayMs(0, 10, 10)).toBe(0);
  });

  it('shouldRefreshNow is true at 81% of lifetime, false at 50%', () => {
    const issued = 1_000_000;
    const lifetime = 1_000_000;
    const expiresAt = issued + lifetime;
    expect(shouldRefreshNow(true, expiresAt, issued + 0.81 * lifetime, 0.8, issued)).toBe(true);
    expect(shouldRefreshNow(true, expiresAt, issued + 0.5 * lifetime, 0.8, issued)).toBe(false);
  });
});
