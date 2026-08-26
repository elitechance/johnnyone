import { describe, it, expect } from 'vitest';
import login from '../../../../../../lokal/apps/lokal-infra/packages/cli/cloudflare/modules/auth/resolvers/login';
import refreshToken from '../../../../../../lokal/apps/lokal-infra/packages/cli/cloudflare/modules/auth/resolvers/refresh-token';

const TENANT = '00000000-0000-0000-0000-000000000001';
const USER = '00000000-0000-0000-0000-000000000002';
const SEED_EMAIL = 'admin@johnnyone.local';
const SEED_PASSWORD = 'johnnyone-dev';
const SEED_HASH = '100000:XjiIcbojqYp8Is1OVL4kfA==:koyPbWNdgLUwy1Codv2+ZQbtZ0f7cGTaABDz0hJhL6c=';

const userRow = {
  id: USER,
  tenant_id: TENANT,
  email: SEED_EMAIL,
  display_name: 'Admin',
  password_hash: SEED_HASH,
  status: 'active',
  created_at: '2026-01-01T00:00:00.000Z',
  updated_at: '2026-01-01T00:00:00.000Z',
};

const sessionRow = {
  id: 'sess-1',
  user_id: USER,
  tenant_id: TENANT,
  refresh_token: 'refresh-seed',
  auth_method: 'password',
  expires_at: new Date(Date.now() + 86400_000).toISOString(),
  created_at: '2026-01-01T00:00:00.000Z',
};

function unauthenticatedCtx(db: any) {
  return {
    db,
    request: new Request('http://localhost'),
    auth: {
      isAuthenticated: false,
      tenantId: null,
      userId: null,
      signJwt: async () => 'signed',
    },
  };
}

function stubDb() {
  return {
    prepare(sql: string) {
      return {
        bind(..._vals: any[]) {
          return {
            first: async () => {
              if (sql.includes('FROM tenants')) return { tenant_id: TENANT };
              if (sql.includes('FROM users')) return userRow;
              if (sql.includes('FROM role_assignments')) return { roles: '["ADMIN"]' };
              if (sql.includes('FROM sessions')) return sessionRow;
              if (sql.includes('FROM passkey_credentials')) return { cnt: 0 };
              return null;
            },
            all: async () => ({ results: [] }),
            run: async () => ({ success: true }),
          };
        },
      };
    },
  };
}

describe('public ops stay public (acceptance 4)', () => {
  it('login with unauthenticated ctx and no Authorization returns accessToken', async () => {
    const ctx = unauthenticatedCtx(stubDb());
    const result = await login(
      null,
      { input: { email: SEED_EMAIL, password: SEED_PASSWORD, tenantId: TENANT } },
      ctx as any,
    );
    expect(result.accessToken).toBe('signed');
    expect(result.refreshToken).toBeTruthy();
    expect(ctx.request.headers.get('Authorization')).toBeNull();
    expect(ctx.auth.isAuthenticated).toBe(false);
  });

  it('refreshToken with unauthenticated ctx and a valid sessions row returns accessToken', async () => {
    const ctx = unauthenticatedCtx(stubDb());
    const result = await refreshToken(
      null,
      { input: { refreshToken: 'refresh-seed' } },
      ctx as any,
    );
    expect(result.accessToken).toBe('signed');
    expect(result.refreshToken).toBeTruthy();
    expect(result.refreshToken).not.toBe('refresh-seed');
    expect(ctx.request.headers.get('Authorization')).toBeNull();
    expect(ctx.auth.isAuthenticated).toBe(false);
  });
});
