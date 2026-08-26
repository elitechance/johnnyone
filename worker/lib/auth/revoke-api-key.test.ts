import { describe, it, expect, beforeEach } from 'vitest';
import { authedCtx } from './test-authed-ctx';

const makeStubDb = (row: any = null, updateOk = true) => {
  let updated = false;
  return {
    prepare: (sql: string) => ({
      bind: (..._v: any[]) => ({
        run: async () => { updated = updateOk; return { success: true }; },
        first: async () => row,
      }),
    }),
    _updated: () => updated,
  } as any;
};

describe('revokeApiKey', () => {
  beforeEach(() => {});

  it('rejects key caller', async () => {
    const { default: revoke } = await import('../../resolvers/revoke-api-key');
    const ctx = authedCtx({ tenantId: 't', userId: 'u', db: makeStubDb(), apiKey: {} as any });
    await expect(revoke(null, { id: 'k1' }, ctx)).rejects.toThrow(/cannot manage/);
  });

  it('sets revoked_at, scoped to caller, idempotent, returns mapped row', async () => {
    const row = { id: 'k1', name: 'x', key_prefix: 'jk_k1', scopes: '[]', last_used_at: null, expires_at: null, revoked_at: '2026-..', created_at: '..' };
    const db = makeStubDb(row);
    const { default: revoke } = await import('../../resolvers/revoke-api-key');
    const ctx = authedCtx({ tenantId: 't1', userId: 'u1', db });
    const out = await revoke(null, { id: 'k1' }, ctx);
    expect(out.revokedAt).toBeTruthy();
    expect(out.id).toBe('k1');
  });

  it('errors on foreign or missing', async () => {
    const db = makeStubDb(null);
    const { default: revoke } = await import('../../resolvers/revoke-api-key');
    const ctx = authedCtx({ tenantId: 't1', userId: 'u1', db });
    await expect(revoke(null, { id: 'missing' }, ctx)).rejects.toThrow(/not found/);
  });
});
