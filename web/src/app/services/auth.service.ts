import { Injectable, inject, signal } from '@angular/core';
import { Router } from '@angular/router';
import {
  issuedAtMs,
  isTokenExpired,
  resolveExpiresAtMs,
  shouldRefreshNow,
  timerDelayMs,
} from './auth-session-logic';

export interface AuthUser {
  id: string;
  tenantId: string;
  email: string;
  displayName: string | null;
  roles: string[];
  status: string;
}

interface AuthPayload {
  accessToken: string;
  refreshToken: string;
  expiresIn: number;
  user: AuthUser;
}

export type RefreshFailureKind = 'auth' | 'transport';

/** Classified refresh failure so callers can distinguish credential death from a blip (D8). */
export class RefreshFailure extends Error {
  readonly kind: RefreshFailureKind;

  constructor(kind: RefreshFailureKind, message: string, options?: ErrorOptions) {
    super(message, options);
    this.name = kind === 'auth' ? 'AuthRefreshError' : 'TransportRefreshError';
    this.kind = kind;
  }
}

export function classifyRefreshFailure(error: unknown): RefreshFailureKind {
  if (error instanceof RefreshFailure) return error.kind;
  if (error instanceof TypeError) return 'transport';
  if (error instanceof Error && (error.name === 'AbortError' || error.name === 'TimeoutError')) {
    return 'transport';
  }
  const msg = error instanceof Error ? error.message : String(error ?? '');
  if (/unauth|invalid refresh|expired refresh/i.test(msg)) return 'auth';
  if (/no refresh token/i.test(msg)) return 'auth';
  if (/\b401\b/.test(msg)) return 'auth';
  if (/\b5\d\d\b/.test(msg) || /\b408\b/.test(msg) || /\b429\b/.test(msg)) return 'transport';
  if (/abort|timeout|network/i.test(msg)) return 'transport';
  return 'transport';
}

@Injectable({ providedIn: 'root' })
export class AuthService {
  static readonly TOKEN_KEY = 'johnnyone_access_token';
  static readonly REFRESH_TOKEN_KEY = 'johnnyone_refresh_token';
  static readonly TENANT_KEY = 'johnnyone_tenant_id';
  static readonly USER_ID_KEY = 'johnnyone_user_id';
  static readonly USER_KEY = 'johnnyone_auth_user';
  static readonly EXPIRES_AT_KEY = 'johnnyone_token_expires_at';
  static readonly EXPIRES_IN_KEY = 'johnnyone_expires_in';

  private static readonly TIMER_BACKOFF_START_MS = 5_000;
  private static readonly TIMER_BACKOFF_MAX_MS = 60_000;

  private readonly router = inject(Router);

  /**
   * Writable session flag. Initial value is conservative; `syncAuthState()` /
   * `isSessionLive()` recompute against *now* (not `hasToken()` alone).
   */
  isAuthenticated = signal(false);
  currentUser = signal<AuthUser | null>(this.loadUser());

  private inflightEnsure: Promise<void> | null = null;
  private refreshTimer: ReturnType<typeof setTimeout> | null = null;
  private lastApiUrl: string | null = null;
  private timerBackoffMs = 0;

  constructor() {
    this.syncAuthState();
  }

  async login(apiUrl: string, email: string, password: string, tenantId: string): Promise<void> {
    this.lastApiUrl = apiUrl;
    const response = await fetch(apiUrl, {
      method: 'POST',
      headers: {
        'Content-Type': 'application/json',
        Accept: 'application/json',
        'x-tenant-id': tenantId,
      },
      body: JSON.stringify({
        query: `mutation Login($input: LoginInput!) {
          login(input: $input) {
            accessToken
            refreshToken
            expiresIn
            user { id tenantId email displayName roles status }
          }
        }`,
        variables: { input: { email, password, tenantId } },
      }),
    });

    if (!response.ok) {
      throw new Error(`Login failed: ${response.status} ${response.statusText}`);
    }

    const json = await response.json() as {
      data?: { login: AuthPayload };
      errors?: Array<{ message: string }>;
    };

    if (json.errors?.length) {
      throw new Error(json.errors[0].message);
    }

    if (!json.data?.login) {
      throw new Error('Login failed: empty auth response');
    }

    this.saveAuth(json.data.login);
  }

  logout(): void {
    this.clearRefreshTimer();
    localStorage.removeItem(AuthService.TOKEN_KEY);
    localStorage.removeItem(AuthService.REFRESH_TOKEN_KEY);
    localStorage.removeItem(AuthService.USER_KEY);
    localStorage.removeItem(AuthService.USER_ID_KEY);
    localStorage.removeItem(AuthService.EXPIRES_AT_KEY);
    localStorage.removeItem(AuthService.EXPIRES_IN_KEY);
    this.syncAuthState();
    this.currentUser.set(null);
    void this.router.navigateByUrl('/login');
  }

  getAccessToken(): string | null {
    return localStorage.getItem(AuthService.TOKEN_KEY);
  }

  getTenantId(): string | null {
    return localStorage.getItem(AuthService.TENANT_KEY);
  }

  getRefreshToken(): string | null {
    return localStorage.getItem(AuthService.REFRESH_TOKEN_KEY);
  }

  getTokenExpiresAt(): number | null {
    const raw = localStorage.getItem(AuthService.EXPIRES_AT_KEY);
    const parsed = raw ? parseInt(raw, 10) : NaN;
    const stored = Number.isFinite(parsed) ? parsed : null;
    return resolveExpiresAtMs(stored, this.getAccessToken());
  }

  getExpiresIn(): number | null {
    const raw = localStorage.getItem(AuthService.EXPIRES_IN_KEY);
    const parsed = raw ? parseInt(raw, 10) : NaN;
    return Number.isFinite(parsed) ? parsed : null;
  }

  isTokenNearExpiry(bufferMs = 60_000): boolean {
    return isTokenExpired(this.getTokenExpiresAt(), Date.now(), bufferMs);
  }

  /** Recompute the live predicate against *now*. Does not trust a stale signal. */
  isSessionLive(): boolean {
    return this.hasToken() && !this.isTokenNearExpiry(0);
  }

  /** Write `isAuthenticated` from `isSessionLive()`. Returns the new value. */
  syncAuthState(): boolean {
    const live = this.isSessionLive();
    this.isAuthenticated.set(live);
    return live;
  }

  /**
   * Single-flight near-expiry refresh (D9). Auth-class failure: `logout()` then
   * **reject** (Amendment 2). Transport: rethrow, do not logout (D8).
   */
  async ensureFreshToken(apiUrl: string): Promise<void> {
    if (this.inflightEnsure) return this.inflightEnsure;
    this.lastApiUrl = apiUrl;
    this.inflightEnsure = this.runEnsureFreshToken(apiUrl).finally(() => {
      this.inflightEnsure = null;
    });
    return this.inflightEnsure;
  }

  /**
   * Boot path. **Never rejects.** Catches `ensureFreshToken` (including the
   * auth-class reject after logout) so `provideAppInitializer` still resolves.
   */
  async startSession(apiUrl: string): Promise<void> {
    this.lastApiUrl = apiUrl;
    try {
      this.syncAuthState();
      const token = this.getAccessToken();
      const hasRefresh = !!this.getRefreshToken();
      const now = Date.now();
      const expiresAt = this.getTokenExpiresAt();
      const expired = isTokenExpired(expiresAt, now, 0);
      const issued = issuedAtMs(expiresAt, this.getExpiresIn(), now);
      const past80 = shouldRefreshNow(!!token, expiresAt, now, 0.8, issued);

      if (hasRefresh && (!token || expired || past80)) {
        await this.ensureFreshToken(apiUrl);
      } else if (token && expired && !hasRefresh) {
        this.logout();
      }
    } catch {
      this.syncAuthState();
    }
    this.armRefreshTimer();
  }

  async refresh(apiUrl: string): Promise<void> {
    this.lastApiUrl = apiUrl;
    const refreshToken = this.getRefreshToken();
    if (!refreshToken) {
      throw new RefreshFailure('auth', 'No refresh token available');
    }
    const signal = refreshAbortSignal();
    try {
      const response = await fetch(apiUrl, {
        method: 'POST',
        headers: {
          'Content-Type': 'application/json',
          Accept: 'application/json',
          ...(this.getTenantId() ? { 'x-tenant-id': this.getTenantId()! } : {}),
        },
        body: JSON.stringify({
          query: `mutation Refresh($input: RefreshTokenInput!) {
            refreshToken(input: $input) {
              accessToken
              refreshToken
              expiresIn
              user { id tenantId email displayName roles status }
            }
          }`,
          variables: { input: { refreshToken } },
        }),
        ...(signal ? { signal } : {}),
      });
      if (response.status === 401) {
        throw new RefreshFailure('auth', `Refresh failed: ${response.status} ${response.statusText}`);
      }
      if (!response.ok) {
        // Non-401 HTTP (400/403/404/408/429/5xx): transport. A misrouted
        // worker URL or edge 403 must not sign the operator out (D8 / Lead T2).
        throw new RefreshFailure('transport', `Refresh failed: ${response.status} ${response.statusText}`);
      }
      const json = await response.json() as {
        data?: { refreshToken: AuthPayload };
        errors?: Array<{ message: string }>;
      };
      if (json.errors?.length) {
        const msg = json.errors[0].message;
        const kind = /unauth|invalid refresh|expired refresh/i.test(msg) ? 'auth' : 'transport';
        throw new RefreshFailure(kind, msg);
      }
      if (!json.data?.refreshToken) {
        throw new RefreshFailure('auth', 'Refresh failed: empty auth response');
      }
      this.saveAuth(json.data.refreshToken);
    } catch (error) {
      if (error instanceof RefreshFailure) throw error;
      throw new RefreshFailure(
        classifyRefreshFailure(error),
        error instanceof Error ? error.message : String(error),
        { cause: error },
      );
    }
  }

  private async runEnsureFreshToken(apiUrl: string): Promise<void> {
    try {
      const now = Date.now();
      const token = this.getAccessToken();
      const expiresAt = this.getTokenExpiresAt();
      const issued = issuedAtMs(expiresAt, this.getExpiresIn(), now);
      const needs = !token || shouldRefreshNow(!!token, expiresAt, now, 0.8, issued);
      if (!needs) {
        this.syncAuthState();
        return;
      }
      await this.refresh(apiUrl);
      this.syncAuthState();
    } catch (error) {
      this.syncAuthState();
      if (classifyRefreshFailure(error) === 'auth') {
        this.logout();
        throw error instanceof RefreshFailure
          ? error
          : new RefreshFailure('auth', error instanceof Error ? error.message : String(error), {
              cause: error,
            });
      }
      throw error;
    }
  }

  private armRefreshTimer(opts?: { backoff?: boolean }): void {
    this.clearRefreshTimer();
    const apiUrl = this.lastApiUrl;
    if (!apiUrl) return;
    const now = Date.now();
    const expiresAt = this.getTokenExpiresAt();
    const issued = issuedAtMs(expiresAt, this.getExpiresIn(), now);
    let delay = timerDelayMs(issued, expiresAt, now);
    if (delay == null) return;
    if (opts?.backoff) {
      this.timerBackoffMs = this.timerBackoffMs > 0
        ? Math.min(this.timerBackoffMs * 2, AuthService.TIMER_BACKOFF_MAX_MS)
        : AuthService.TIMER_BACKOFF_START_MS;
      delay = Math.max(delay, this.timerBackoffMs);
    } else {
      this.timerBackoffMs = 0;
    }
    this.refreshTimer = setTimeout(() => {
      void this.onRefreshTimerFire();
    }, delay);
  }

  private async onRefreshTimerFire(): Promise<void> {
    const apiUrl = this.lastApiUrl;
    if (!apiUrl) return;
    try {
      await this.ensureFreshToken(apiUrl);
      this.syncAuthState();
      this.armRefreshTimer();
    } catch (error) {
      this.syncAuthState();
      if (classifyRefreshFailure(error) === 'auth') {
        this.clearRefreshTimer();
        return;
      }
      this.armRefreshTimer({ backoff: true });
    }
  }

  private clearRefreshTimer(): void {
    if (this.refreshTimer != null) {
      clearTimeout(this.refreshTimer);
      this.refreshTimer = null;
    }
  }

  private saveAuth(payload: AuthPayload): void {
    localStorage.setItem(AuthService.TOKEN_KEY, payload.accessToken);
    localStorage.setItem(AuthService.REFRESH_TOKEN_KEY, payload.refreshToken);
    localStorage.setItem(AuthService.TENANT_KEY, payload.user.tenantId);
    localStorage.setItem(AuthService.USER_ID_KEY, payload.user.id);
    localStorage.setItem(AuthService.USER_KEY, JSON.stringify(payload.user));
    const expiresIn = payload.expiresIn || 0;
    localStorage.setItem(AuthService.EXPIRES_IN_KEY, String(expiresIn));
    const expiresAt = Date.now() + expiresIn * 1000;
    localStorage.setItem(AuthService.EXPIRES_AT_KEY, String(expiresAt));
    this.syncAuthState();
    this.currentUser.set(payload.user);
    this.armRefreshTimer();
  }

  private hasToken(): boolean {
    return !!localStorage.getItem(AuthService.TOKEN_KEY);
  }

  private loadUser(): AuthUser | null {
    const raw = localStorage.getItem(AuthService.USER_KEY);
    if (!raw) return null;

    try {
      return JSON.parse(raw) as AuthUser;
    } catch {
      return null;
    }
  }
}

function refreshAbortSignal(): AbortSignal | undefined {
  if (typeof AbortSignal !== 'undefined' && typeof AbortSignal.timeout === 'function') {
    return AbortSignal.timeout(4_000);
  }
  return undefined;
}
