import { describe, it, expect, vi, beforeEach } from 'vitest';

const makeStubDb = (overrides: any = {}) => {
  const calls: any[] = [];
  return {
    calls,
    prepare: (sql: string) => ({
      bind: (...vals: any[]) => {
        calls.push({ sql, vals });
        return {
          run: async () => ({ success: true, ...(overrides.run || {}) }),
          first: async () => overrides.first || null,
          all: async () => ({ results: overrides.all || [] }),
        };
      },
    }),
  } as any;
};

describe('createApiKey (JWT-only + hash-only + scopes)', () => {
  beforeEach(() => vi.restoreAllMocks());

  it('rejects when ctx.apiKey present (no key-mints-key)', async () => {
    const { default: create } = await import('../../resolvers/create-api-key');
    const ctx = { db: makeStubDb(), auth: { tenantId: 't1', userId: 'u1' }, apiKey: { id: 'k1', scopes: [] } } as any;
    await expect(create(null, { input: { name: 'x', scopes: ['sessions:read'] } }, ctx)).rejects.toThrow(/cannot manage/);
  });

  it('creates, stores only hash, returns secret once', async () => {
    const db = makeStubDb();
    const { default: create } = await import('../../resolvers/create-api-key');
    const ctx = { db, auth: { tenantId: 't1', userId: 'u1' } } as any;
    const res = await create(null, { input: { name: ' CI bot ', scopes: ['sessions:read', 'terminal:write'] } }, ctx);
    expect(res.secret).toMatch(/^jk_/);
    expect(res.apiKey.keyPrefix).toMatch(/^jk_/);
    expect(res.apiKey.scopes).toEqual(['sessions:read', 'terminal:write']);
    // last insert had key_hash (sha) not secret
    const insert = db.calls.find((c: any) => c.sql.includes('INSERT INTO api_keys'));
    expect(insert).toBeTruthy();
    expect(insert.vals[4]).toMatch(/^[0-9a-f]{64}$/); // key_hash
    expect(insert.vals.includes(res.secret)).toBe(false);
  });

  it('rejects invalid name or scopes', async () => {
    const { default: create } = await import('../../resolvers/create-api-key');
    const ctx = { db: makeStubDb(), auth: { tenantId: 't1', userId: 'u1' } } as any;
    await expect(create(null, { input: { name: '', scopes: ['sessions:read'] } }, ctx)).rejects.toThrow(/name/);
    await expect(create(null, { input: { name: 'n', scopes: [] } }, ctx)).rejects.toThrow(/scope/);
    await expect(create(null, { input: { name: 'n', scopes: ['nope:foo'] } }, ctx)).rejects.toThrow(/invalid scope/);
  });

  it('rejects bad/missing expiresAt at create; normalizes valid future date', async () => {
    const { default: create } = await import('../../resolvers/create-api-key');
    const ctx = { db: makeStubDb(), auth: { tenantId: 't1', userId: 'u1' } } as any;
    await expect(create(null, { input: { name: 'n', scopes: ['sessions:read'], expiresAt: 'not-a-date' } }, ctx)).rejects.toThrow(/valid ISO/);
    await expect(create(null, { input: { name: 'n', scopes: ['sessions:read'], expiresAt: new Date(Date.now() - 1000).toISOString() } }, ctx)).rejects.toThrow(/future/);

    const res = await create(null, { input: { name: 'n', scopes: ['sessions:read'], expiresAt: '2099-01-01T00:00:00.000Z' } }, ctx);
    expect(res.apiKey.expiresAt).toBe('2099-01-01T00:00:00.000Z');
    const insert = (ctx.db as any).calls.find((c: any) => c.sql.includes('INSERT INTO api_keys'));
    expect(insert.vals[7]).toBe('2099-01-01T00:00:00.000Z');
  });
});
