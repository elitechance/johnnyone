import { describe, it, expect, beforeEach } from 'vitest';

const makeStubDb = (results: any[] = []) => ({
  prepare: () => ({
    bind: () => ({
      all: async () => ({ results }),
    }),
  }),
}) as any;

describe('listApiKeys', () => {
  beforeEach(() => {});

  it('rejects key-auth caller', async () => {
    const { default: list } = await import('../../resolvers/list-api-keys');
    const ctx = { db: makeStubDb(), auth: { tenantId: 't', userId: 'u' }, apiKey: { id: 'k' } } as any;
    await expect(list(null, {}, ctx)).rejects.toThrow(/cannot manage/);
  });

  it('returns only caller rows, maps fields, parses scopes', async () => {
    const rows = [
      { id: 'k1', name: 'bot', key_prefix: 'jk_k1', scopes: '["sessions:read"]', last_used_at: null, expires_at: null, revoked_at: null, created_at: '2026-01-01' },
    ];
    const { default: list } = await import('../../resolvers/list-api-keys');
    const ctx = { db: makeStubDb(rows), auth: { tenantId: 't1', userId: 'u1' } } as any;
    const out = await list(null, {}, ctx);
    expect(out).toHaveLength(1);
    expect(out[0].id).toBe('k1');
    expect(out[0].scopes).toEqual(['sessions:read']);
    expect(out[0].keyPrefix).toBe('jk_k1');
  });
});
