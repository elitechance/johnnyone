import { describe, it, expect, vi } from 'vitest';
import { GraphQLError } from 'graphql';
import { signJwt, verifyJwt } from 'modules/auth/auth-middleware';
import { generateApiKey, sha256Hex, authorizeForAltToken } from './api-key';
import { ForbiddenScopeError } from './scopes';
import { requireIdentity, UnauthenticatedError } from './require-identity';

const SECRET = 'test-secret-for-phase-00';

function headers(map: Record<string, string | null>) {
  return {
    get(k: string) {
      const key = String(k).toLowerCase();
      for (const [n, v] of Object.entries(map)) {
        if (n.toLowerCase() === key) return v;
      }
      return null;
    },
  };
}

function stubDbForKey(row: any) {
  return {
    prepare: (_sql: string) => ({
      bind: (..._vals: any[]) => ({
        first: async () => row,
        run: async () => ({ success: true }),
      }),
    }),
  } as any;
}

async function expectUnauthenticated(p: Promise<unknown>) {
  try {
    await p;
    throw new Error('expected UNAUTHENTICATED');
  } catch (e) {
    expect(e).toBeInstanceOf(GraphQLError);
    expect(e).toBeInstanceOf(UnauthenticatedError);
    expect((e as GraphQLError).extensions.code).toBe('UNAUTHENTICATED');
    expect((e as Error).message).toBe('UNAUTHENTICATED');
  }
}

describe('requireIdentity reject / happy / spoof cases', () => {
  it('absent header — no request, auth unauthenticated → UNAUTHENTICATED', async () => {
    const ctx: any = { auth: { isAuthenticated: false, tenantId: null, userId: null } };
    await expectUnauthenticated(requireIdentity(ctx));
  });

  it('malformed token — Authorization: Bearer not-a-jwt → UNAUTHENTICATED', async () => {
    const ctx: any = {
      request: { headers: headers({ Authorization: 'Bearer not-a-jwt' }) },
      auth: { verifyJwt: (t: string) => verifyJwt(t, SECRET), isAuthenticated: false },
    };
    await expectUnauthenticated(requireIdentity(ctx));
  });

  it('bad signature — signJwt with wrong secret; real verifyJwt rejects → UNAUTHENTICATED', async () => {
    const forged = await signJwt(
      { sub: 'u', tenantId: 't', exp: Math.floor(Date.now() / 1000) + 3600 },
      'a-completely-different-wrong-secret',
    );
    const ctx: any = {
      request: { headers: headers({ Authorization: `Bearer ${forged}` }) },
      auth: { verifyJwt: (t: string) => verifyJwt(t, SECRET), isAuthenticated: false },
    };
    await expectUnauthenticated(requireIdentity(ctx));
  });

  it('expired JWT — signJwt with past exp; verifyJwt rejects → UNAUTHENTICATED', async () => {
    const expired = await signJwt(
      { sub: 'u', tenantId: 't', exp: Math.floor(Date.now() / 1000) - 60 },
      SECRET,
    );
    const ctx: any = {
      request: { headers: headers({ Authorization: `Bearer ${expired}` }) },
      auth: { verifyJwt: (t: string) => verifyJwt(t, SECRET), isAuthenticated: false },
    };
    await expectUnauthenticated(requireIdentity(ctx));
  });

  it('valid JWT identity — Bearer + verifyJwt {sub, tenantId} → pair and ctx.auth.tenantId', async () => {
    const verify = vi.fn(async () => ({ sub: 'u1', tenantId: 't1' }));
    const ctx: any = {
      request: { headers: headers({ Authorization: 'Bearer dummy-jwt' }) },
      auth: { verifyJwt: verify },
    };
    const id = await requireIdentity(ctx);
    expect(id).toEqual({ tenantId: 't1', userId: 'u1' });
    expect(ctx.auth.tenantId).toBe('t1');
    expect(ctx.auth.userId).toBe('u1');
    expect(ctx.auth.isAuthenticated).toBe(true);
  });

  it('valid JWT + spoofed x-tenant-id — payload A, ctx.auth / header B → A', async () => {
    const verify = vi.fn(async () => ({ sub: 'uA', tenantId: 'A' }));
    const ctx: any = {
      request: {
        headers: headers({ Authorization: 'Bearer dummy-jwt', 'x-tenant-id': 'B' }),
      },
      auth: { tenantId: 'B', userId: 'uB', isAuthenticated: true, verifyJwt: verify },
    };
    const id = await requireIdentity(ctx);
    expect(id).toEqual({ tenantId: 'A', userId: 'uA' });
    expect(ctx.auth.tenantId).toBe('A');
    expect(ctx.auth.userId).toBe('uA');
  });

  it('bare Authorization (no Bearer prefix) — raw JWT + x-tenant-id B → A (not D13)', async () => {
    const raw = 'eyJhbGciOiJIUzI1NiJ9.payload.sig';
    const verify = vi.fn(async () => ({ sub: 'uA', tenantId: 'A' }));
    const ctx: any = {
      request: { headers: headers({ Authorization: raw, 'x-tenant-id': 'B' }) },
      auth: { tenantId: 'B', userId: 'uB', isAuthenticated: false, verifyJwt: verify },
    };
    const id = await requireIdentity(ctx);
    expect(id).toEqual({ tenantId: 'A', userId: 'uA' });
    expect(ctx.auth.tenantId).toBe('A');
    expect(verify).toHaveBeenCalledWith(raw);
  });

  it('graphql-ws shape — empty Request + isAuthenticated true + ids → those ids', async () => {
    const ctx: any = {
      request: new Request('http://localhost'),
      auth: { tenantId: 't-ws', userId: 'u-ws', isAuthenticated: true },
    };
    await expect(requireIdentity(ctx)).resolves.toEqual({ tenantId: 't-ws', userId: 'u-ws' });
  });

  it('graphql-ws shape — empty Request + isAuthenticated false → UNAUTHENTICATED', async () => {
    const ctx: any = {
      request: new Request('http://localhost'),
      auth: { tenantId: 't-ws', userId: 'u-ws', isAuthenticated: false },
    };
    await expectUnauthenticated(requireIdentity(ctx));
  });

  it('query token does not authenticate — ?token=<valid jwt>, no Authorization → UNAUTHENTICATED', async () => {
    const valid = await signJwt(
      { sub: 'u', tenantId: 't', exp: Math.floor(Date.now() / 1000) + 3600 },
      SECRET,
    );
    const ctx: any = {
      request: new Request(`http://localhost/graphql?token=${encodeURIComponent(valid)}`),
      auth: {
        isAuthenticated: false,
        verifyJwt: (t: string) => verifyJwt(t, SECRET),
      },
    };
    await expectUnauthenticated(requireIdentity(ctx));
  });

  it('valid jk_ — generated token + matching D1 row → identity and ctx.apiKey', async () => {
    const g = generateApiKey();
    const h = await sha256Hex(g.secret);
    const row = {
      id: g.keyId,
      tenant_id: 't1',
      user_id: 'u1',
      key_hash: h,
      scopes: '["sessions:read"]',
      revoked_at: null,
      expires_at: null,
      is_deleted: 0,
    };
    const ctx: any = {
      db: stubDbForKey(row),
      request: { headers: headers({ Authorization: `Bearer ${g.token}` }) },
      auth: { isAuthenticated: false },
    };
    const id = await requireIdentity(ctx);
    expect(id).toEqual({ tenantId: 't1', userId: 'u1' });
    expect(ctx.apiKey).toMatchObject({ id: g.keyId, scopes: ['sessions:read'] });
    expect(ctx.auth.tenantId).toBe('t1');
  });

  it('jk_ missing db — presented jk_, ctx.db absent → UNAUTHENTICATED, not TypeError', async () => {
    const ctx: any = {
      request: { headers: headers({ Authorization: 'Bearer jk_abc_secret' }) },
    };
    try {
      await requireIdentity(ctx);
      throw new Error('expected UNAUTHENTICATED');
    } catch (e) {
      expect(e).toBeInstanceOf(GraphQLError);
      expect((e as GraphQLError).extensions.code).toBe('UNAUTHENTICATED');
      expect(e).not.toBeInstanceOf(TypeError);
    }
  });

  it('jk_ missing scope — authorizeForAltToken(ctx, plans:write) with sessions:read only → FORBIDDEN_SCOPE', async () => {
    const g = generateApiKey();
    const h = await sha256Hex(g.secret);
    const row = {
      id: g.keyId,
      tenant_id: 't1',
      user_id: 'u1',
      key_hash: h,
      scopes: '["sessions:read"]',
      revoked_at: null,
      expires_at: null,
      is_deleted: 0,
    };
    const ctx: any = {
      db: stubDbForKey(row),
      request: { headers: headers({ Authorization: `Bearer ${g.token}` }) },
    };
    try {
      await authorizeForAltToken(ctx, 'plans:write');
      throw new Error('expected FORBIDDEN_SCOPE');
    } catch (e) {
      expect(e).toBeInstanceOf(ForbiddenScopeError);
      expect((e as any).code).toBe('FORBIDDEN_SCOPE');
    }
  });

  it('presented jk_ that validateApiKeyToken rejects (unknown id) → UNAUTHENTICATED', async () => {
    const ctx: any = {
      db: stubDbForKey(null),
      request: { headers: headers({ Authorization: 'Bearer jk_unknownid_secret' }) },
    };
    await expectUnauthenticated(requireIdentity(ctx));
    await expectUnauthenticated(authorizeForAltToken(ctx));
  });

  it('presented jk_ that validateApiKeyToken rejects (bad secret) → UNAUTHENTICATED', async () => {
    const g = generateApiKey();
    const h = await sha256Hex(g.secret);
    const row = {
      id: g.keyId,
      tenant_id: 't1',
      user_id: 'u1',
      key_hash: h,
      scopes: '[]',
      revoked_at: null,
      expires_at: null,
      is_deleted: 0,
    };
    const ctx: any = {
      db: stubDbForKey(row),
      request: { headers: headers({ Authorization: `Bearer jk_${g.keyId}_deadbeefdeadbeefdeadbeefdeadbeef` }) },
    };
    await expectUnauthenticated(requireIdentity(ctx));
  });

  it('presented jk_ that validateApiKeyToken rejects (revoked) → UNAUTHENTICATED', async () => {
    const g = generateApiKey();
    const h = await sha256Hex(g.secret);
    const row = {
      id: g.keyId,
      tenant_id: 't1',
      user_id: 'u1',
      key_hash: h,
      scopes: '[]',
      revoked_at: '2026-01-01T00:00:00.000Z',
      expires_at: null,
      is_deleted: 0,
    };
    const ctx: any = {
      db: stubDbForKey(row),
      request: { headers: headers({ Authorization: `Bearer ${g.token}` }) },
    };
    await expectUnauthenticated(requireIdentity(ctx));
  });

  it('presented jk_ that validateApiKeyToken rejects (expired) → UNAUTHENTICATED', async () => {
    const g = generateApiKey();
    const h = await sha256Hex(g.secret);
    const row = {
      id: g.keyId,
      tenant_id: 't1',
      user_id: 'u1',
      key_hash: h,
      scopes: '[]',
      revoked_at: null,
      expires_at: '2020-01-01T00:00:00.000Z',
      is_deleted: 0,
    };
    const ctx: any = {
      db: stubDbForKey(row),
      request: { headers: headers({ Authorization: `Bearer ${g.token}` }) },
    };
    await expectUnauthenticated(requireIdentity(ctx));
  });
});
