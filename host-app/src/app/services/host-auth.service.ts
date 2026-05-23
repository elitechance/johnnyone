import { Injectable, signal } from '@angular/core';

/**
 * Auth service for the Tauri control-panel host app.
 *
 * On real desktop builds this should invoke the worker's `login` mutation via the
 * configured worker URL, store tokens in the OS keyring (via Tauri's secure-storage
 * plugin), and register this machine as a desktop_node tagged with the user.
 *
 * For Phase 3 we ship the structural shell — login form posts to the worker
 * GraphQL endpoint, stores tokens in localStorage. Keyring migration is a
 * follow-up alongside the production installer (Phase 4 / signed binary work).
 */

const WORKER_URL = 'https://johnnyone-dev-hub.ethan-353.workers.dev/graphql';
const TOKEN_KEY = 'johnnyone_host_access_token';
const USER_KEY = 'johnnyone_host_user';

export interface HostAuthUser {
  id: string;
  tenantId: string;
  email: string;
  displayName: string | null;
}

interface LoginResponse {
  accessToken: string;
  refreshToken: string;
  expiresIn: number;
  user: {
    id: string;
    tenantId: string;
    email: string;
    displayName: string | null;
  };
}

@Injectable({ providedIn: 'root' })
export class HostAuthService {
  readonly currentUser = signal<HostAuthUser | null>(this.loadUser());
  readonly isAuthenticated = signal<boolean>(!!this.getAccessToken());

  async login(email: string, password: string, tenantId: string): Promise<void> {
    const res = await fetch(WORKER_URL, {
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
            user { id tenantId email displayName }
          }
        }`,
        variables: { input: { email, password, tenantId } },
      }),
    });

    if (!res.ok) {
      throw new Error(`Login failed: ${res.status} ${res.statusText}`);
    }
    const json = (await res.json()) as {
      data?: { login: LoginResponse };
      errors?: Array<{ message: string }>;
    };
    if (json.errors?.length) throw new Error(json.errors[0].message);
    if (!json.data?.login) throw new Error('Login failed: empty response');

    localStorage.setItem(TOKEN_KEY, json.data.login.accessToken);
    localStorage.setItem(USER_KEY, JSON.stringify(json.data.login.user));
    this.currentUser.set(json.data.login.user);
    this.isAuthenticated.set(true);
  }

  logout(): void {
    localStorage.removeItem(TOKEN_KEY);
    localStorage.removeItem(USER_KEY);
    this.currentUser.set(null);
    this.isAuthenticated.set(false);
  }

  getAccessToken(): string | null {
    return localStorage.getItem(TOKEN_KEY);
  }

  private loadUser(): HostAuthUser | null {
    const raw = localStorage.getItem(USER_KEY);
    if (!raw) return null;
    try { return JSON.parse(raw) as HostAuthUser; } catch { return null; }
  }
}
