import { Injectable, inject, signal } from '@angular/core';
import { Router } from '@angular/router';

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

@Injectable({ providedIn: 'root' })
export class AuthService {
  static readonly TOKEN_KEY = 'johnnyone_access_token';
  static readonly REFRESH_TOKEN_KEY = 'johnnyone_refresh_token';
  static readonly TENANT_KEY = 'johnnyone_tenant_id';
  static readonly USER_ID_KEY = 'johnnyone_user_id';
  static readonly USER_KEY = 'johnnyone_auth_user';
  static readonly EXPIRES_AT_KEY = 'johnnyone_token_expires_at';
  static readonly EXPIRES_IN_KEY = 'johnnyone_expires_in';

  private readonly router = inject(Router);

  isAuthenticated = signal(this.hasToken());
  currentUser = signal<AuthUser | null>(this.loadUser());

  async login(apiUrl: string, email: string, password: string, tenantId: string): Promise<void> {
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
    localStorage.removeItem(AuthService.TOKEN_KEY);
    localStorage.removeItem(AuthService.REFRESH_TOKEN_KEY);
    localStorage.removeItem(AuthService.USER_KEY);
    localStorage.removeItem(AuthService.USER_ID_KEY);
    localStorage.removeItem(AuthService.EXPIRES_AT_KEY);
    localStorage.removeItem(AuthService.EXPIRES_IN_KEY);
    this.isAuthenticated.set(false);
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
    return raw ? parseInt(raw, 10) : null;
  }

  getExpiresIn(): number | null {
    const raw = localStorage.getItem(AuthService.EXPIRES_IN_KEY);
    return raw ? parseInt(raw, 10) : null;
  }

  isTokenNearExpiry(bufferMs = 60_000): boolean {
    const exp = this.getTokenExpiresAt();
    if (!exp) return !this.getAccessToken();
    return Date.now() > (exp - bufferMs);
  }

  async refresh(apiUrl: string): Promise<void> {
    const refreshToken = this.getRefreshToken();
    if (!refreshToken) {
      throw new Error('No refresh token available');
    }
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
    });
    if (!response.ok) {
      throw new Error(`Refresh failed: ${response.status} ${response.statusText}`);
    }
    const json = await response.json() as {
      data?: { refreshToken: AuthPayload };
      errors?: Array<{ message: string }>;
    };
    if (json.errors?.length) {
      throw new Error(json.errors[0].message);
    }
    if (!json.data?.refreshToken) {
      throw new Error('Refresh failed: empty auth response');
    }
    this.saveAuth(json.data.refreshToken);
  }

  private saveAuth(payload: AuthPayload): void {
    localStorage.setItem(AuthService.TOKEN_KEY, payload.accessToken);
    localStorage.setItem(AuthService.REFRESH_TOKEN_KEY, payload.refreshToken);
    localStorage.setItem(AuthService.TENANT_KEY, payload.user.tenantId);
    localStorage.setItem(AuthService.USER_ID_KEY, payload.user.id);
    localStorage.setItem(AuthService.USER_KEY, JSON.stringify(payload.user));
    // Store expiresIn (per decisions) and computed expiresAt for near-expiry checks.
    // Consult getExpiresIn() / isTokenNearExpiry() to decide proactive refresh only when missing/near-expiry.
    const expiresIn = payload.expiresIn || 0;
    localStorage.setItem(AuthService.EXPIRES_IN_KEY, String(expiresIn));
    const expiresAt = Date.now() + expiresIn * 1000;
    localStorage.setItem(AuthService.EXPIRES_AT_KEY, String(expiresAt));
    this.isAuthenticated.set(true);
    this.currentUser.set(payload.user);
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
