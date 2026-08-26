/**
 * DOM-free session predicate + refresh-due math.
 * No Angular, Ionic, browser storage, or Date.now() — callers inject `nowMs`.
 */

const MAX_TIMER_DELAY_MS = 2 ** 31 - 1;

/** Split a JWT, base64url-decode the payload, return `exp` (seconds) as ms. Opaque / malformed → null. */
export function readJwtExpMs(token: string | null | undefined): number | null {
  if (!token || typeof token !== 'string') return null;
  const parts = token.split('.');
  if (parts.length < 2) return null;
  try {
    const normalized = parts[1].replace(/-/g, '+').replace(/_/g, '/');
    const padLen = (4 - (normalized.length % 4)) % 4;
    const json = atob(normalized + '='.repeat(padLen));
    const parsed = JSON.parse(json) as { exp?: unknown };
    if (typeof parsed.exp !== 'number' || !Number.isFinite(parsed.exp)) return null;
    return parsed.exp * 1000;
  } catch {
    return null;
  }
}

/** Prefer stored `expiresAt`; else JWT `exp`. Missing stored key is not "not expired" when the JWT is stale. */
export function resolveExpiresAtMs(
  storedExpiresAt: number | null,
  token: string | null | undefined,
): number | null {
  if (storedExpiresAt != null && Number.isFinite(storedExpiresAt)) {
    return storedExpiresAt;
  }
  return readJwtExpMs(token);
}

/** Null / non-finite expiry is not expired (opaque API keys, F28). */
export function isTokenExpired(
  expiresAtMs: number | null,
  nowMs: number,
  bufferMs = 0,
): boolean {
  if (expiresAtMs == null || !Number.isFinite(expiresAtMs)) return false;
  const buffer = bufferMs != null && Number.isFinite(bufferMs) ? bufferMs : 0;
  return nowMs > expiresAtMs - buffer;
}

export function isSessionAuthenticated(
  hasToken: boolean,
  expiresAtMs: number | null,
  nowMs: number,
): boolean {
  return !!hasToken && !isTokenExpired(expiresAtMs, nowMs, 0);
}

export function refreshDueAtMs(
  issuedOrNowMs: number,
  expiresAtMs: number,
  fraction = 0.8,
): number {
  return issuedOrNowMs + (expiresAtMs - issuedOrNowMs) * fraction;
}

/** `expiresAt - expiresIn*1000` when both are finite; otherwise `nowMs`. */
export function issuedAtMs(
  expiresAtMs: number | null,
  expiresInSec: number | null,
  nowMs: number,
): number {
  if (
    expiresAtMs != null &&
    Number.isFinite(expiresAtMs) &&
    expiresInSec != null &&
    Number.isFinite(expiresInSec)
  ) {
    return expiresAtMs - expiresInSec * 1000;
  }
  return nowMs;
}

/**
 * Delay until the 80% mark. `null` when there is no expiry (do not arm).
 * Clamped to `[0, 2**31 - 1]` so `setTimeout` never sees NaN or an overflow.
 */
export function timerDelayMs(
  issuedMs: number,
  expiresAtMs: number | null,
  nowMs: number,
): number | null {
  if (expiresAtMs == null || !Number.isFinite(expiresAtMs)) return null;
  const due = refreshDueAtMs(issuedMs, expiresAtMs);
  const delay = due - nowMs;
  if (!Number.isFinite(delay)) return 0;
  if (delay <= 0) return 0;
  if (delay > MAX_TIMER_DELAY_MS) return MAX_TIMER_DELAY_MS;
  return delay;
}

/**
 * Refresh when the access token is missing, already expired, or past `fraction`
 * of lifetime. `issuedMs` is required for the 80% check — assuming issued-at 0
 * would mark every live unix-timestamp JWT as past due.
 */
export function shouldRefreshNow(
  hasToken: boolean,
  expiresAtMs: number | null,
  nowMs: number,
  fraction = 0.8,
  issuedMs?: number | null,
): boolean {
  if (!hasToken) return true;
  if (expiresAtMs == null || !Number.isFinite(expiresAtMs)) return false;
  if (isTokenExpired(expiresAtMs, nowMs, 0)) return true;
  if (issuedMs == null || !Number.isFinite(issuedMs)) return false;
  return nowMs >= refreshDueAtMs(issuedMs, expiresAtMs, fraction);
}

export { MAX_TIMER_DELAY_MS };
