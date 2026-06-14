import { Injectable, inject, signal } from '@angular/core';
import { HostRelayService } from './host-relay.service';
import { HostSettingsService } from './host-settings.service';

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
  user: HostAuthUser;
}

@Injectable({ providedIn: 'root' })
export class HostAuthService {
  private readonly settings = inject(HostSettingsService);
  private readonly relay = inject(HostRelayService);

  readonly currentUser = signal<HostAuthUser | null>(this.loadUser());
  readonly isAuthenticated = signal<boolean>(!!this.getAccessToken());

  async login(
    email: string,
    password: string,
    tenantId: string,
    workerUrlOverride?: string,
  ): Promise<void> {
    const hostSettings = await this.settings.load();
    const workerUrl = (workerUrlOverride ?? hostSettings.workerUrl).trim();
    const resolvedTenantId = tenantId.trim() || hostSettings.tenantId.trim();
    const graphqlUrl = this.settings.workerGraphqlUrl(workerUrl);

    const res = await fetch(graphqlUrl, {
      method: 'POST',
      headers: {
        'Content-Type': 'application/json',
        Accept: 'application/json',
        'x-tenant-id': resolvedTenantId,
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
        variables: {
          input: { email, password, tenantId: resolvedTenantId },
        },
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

    const login = json.data.login;
    localStorage.setItem(TOKEN_KEY, login.accessToken);
    localStorage.setItem(USER_KEY, JSON.stringify(login.user));
    this.currentUser.set(login.user);
    this.isAuthenticated.set(true);

    await this.settings.save({
      ...hostSettings,
      workerUrl,
      tenantId: login.user.tenantId,
      userId: login.user.id,
    });
    await this.relay.connect();
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
    try {
      return JSON.parse(raw) as HostAuthUser;
    } catch {
      return null;
    }
  }
}