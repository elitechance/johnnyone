import { describe, it, expect, vi } from 'vitest';
import { GraphQLError } from 'graphql';
import {
  extractPresentedToken,
  requireIdentity,
  UnauthenticatedError,
} from './require-identity';

const IDENTITY_MEMO = Symbol.for('johnnyone.requireIdentity');

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

describe('extractPresentedToken', () => {
  it('reads Authorization: Bearer x (prefix stripped)', () => {
    const ctx = { request: { headers: headers({ Authorization: 'Bearer x' }) } };
    expect(extractPresentedToken(ctx)).toBe('x');
  });

  it('reads Authorization: <raw jwt> with no Bearer prefix', () => {
    const raw = 'eyJhbGciOiJIUzI1NiJ9.payload.sig';
    const ctx = { request: { headers: headers({ Authorization: raw }) } };
    expect(extractPresentedToken(ctx)).toBe(raw);
  });

  it('does not return a ?token= query value', () => {
    const req = new Request('http://localhost/graphql?token=secret-from-query');
    expect(extractPresentedToken({ request: req })).toBe('');
  });

  it('ignores x-user-id / x-tenant-id', () => {
    const ctx = {
      request: {
        headers: headers({
          'x-user-id': 'spoof-user',
          'x-tenant-id': 'spoof-tenant',
        }),
      },
    };
    expect(extractPresentedToken(ctx)).toBe('');
  });
});

describe('requireIdentity smoke', () => {
  it('presented JWT + verifyJwt tenant A overwrites spoofed ctx.auth.tenantId B', async () => {
    const verifyJwt = vi.fn(async () => ({ sub: 'uA', tenantId: 'A' }));
    const ctx: any = {
      request: { headers: headers({ Authorization: 'Bearer dummy' }) },
      auth: { tenantId: 'B', userId: 'uB', isAuthenticated: true, verifyJwt },
    };
    const id = await requireIdentity(ctx);
    expect(id).toEqual({ tenantId: 'A', userId: 'uA' });
    expect(ctx.auth.tenantId).toBe('A');
    expect(ctx.auth.userId).toBe('uA');
    expect(ctx.auth.isAuthenticated).toBe(true);
  });

  it('Authorization: <raw jwt, no Bearer prefix> + x-tenant-id B / ctx.auth.tenantId B → A', async () => {
    const raw = 'eyJhbGciOiJIUzI1NiJ9.payload.sig';
    const verifyJwt = vi.fn(async () => ({ sub: 'uA', tenantId: 'A' }));
    const ctx: any = {
      request: {
        headers: headers({ Authorization: raw, 'x-tenant-id': 'B' }),
      },
      auth: { tenantId: 'B', userId: 'uB', isAuthenticated: false, verifyJwt },
    };
    const id = await requireIdentity(ctx);
    expect(id).toEqual({ tenantId: 'A', userId: 'uA' });
    expect(ctx.auth.tenantId).toBe('A');
    expect(verifyJwt).toHaveBeenCalledWith(raw);
  });

  it('isAuthenticated: true + both IDs + no Authorization (empty Request) → those ids (D13)', async () => {
    const ctx: any = {
      request: new Request('http://localhost'),
      auth: { tenantId: 't-ws', userId: 'u-ws', isAuthenticated: true },
    };
    const id = await requireIdentity(ctx);
    expect(id).toEqual({ tenantId: 't-ws', userId: 'u-ws' });
  });

  it('isAuthenticated: false + IDs present + no Authorization → UNAUTHENTICATED', async () => {
    const ctx: any = {
      request: new Request('http://localhost'),
      auth: { tenantId: 't', userId: 'u', isAuthenticated: false },
    };
    await expectUnauthenticated(requireIdentity(ctx));
  });

  it('jk_ token + missing ctx.db → UNAUTHENTICATED, not TypeError', async () => {
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

  it('memo: two requireIdentity calls invoke verifyJwt once', async () => {
    const verifyJwt = vi.fn(async () => ({ sub: 'u', tenantId: 't' }));
    const ctx: any = {
      request: { headers: headers({ Authorization: 'Bearer dummy' }) },
      auth: { verifyJwt },
    };
    const a = await requireIdentity(ctx);
    const b = await requireIdentity(ctx);
    expect(a).toEqual({ tenantId: 't', userId: 'u' });
    expect(b).toEqual(a);
    expect(verifyJwt).toHaveBeenCalledTimes(1);
  });

  it('pre-setting ctx.auth.userId without a successful verify does not populate the memo', async () => {
    const ctx: any = {
      auth: { userId: 'u', tenantId: 't', isAuthenticated: false },
    };
    await expectUnauthenticated(requireIdentity(ctx));
    expect(ctx[IDENTITY_MEMO]).toBeUndefined();
  });
});
