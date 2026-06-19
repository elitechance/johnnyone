import { describe, it, expect, beforeEach, vi } from 'vitest';
import { generateApiKey, sha256Hex, applyAltTokenToContext, validateApiKeyToken, isTokenLike } from './auth/api-key';
import { requireScope } from './auth/scopes';
import * as desktopRpcMod from './runtime/desktop-rpc';

function makeCtxWithDbAndReq(token?: string, apiKeyRow?: any) {
  const calls: any[] = [];
  const db: any = {
    prepare: (sql: string) => ({
      bind: (...vals: any[]) => {
        calls.push({ sql, vals });
        if (sql.includes('SELECT') && apiKeyRow) {
          return { first: async () => apiKeyRow };
        }
        return {
          run: async () => ({ success: true }),
          first: async () => apiKeyRow || null,
          all: async () => ({ results: [] }),
        };
      },
    }),
    _calls: calls,
  };
  const headers: any = { get: (k: string) => (k.toLowerCase() === 'authorization' ? (token ? `Bearer ${token}` : null) : null) };
  return { db, request: { headers }, auth: null, _calls: calls } as any;
}

describe('Phase 05 integration (jk_ on both paths + scopes + revoke effect)', () => {
  beforeEach(() => {});

  it('jk_ token parses, hashes, validates against row, sets ctx.auth+apiKey', async () => {
    const g = generateApiKey();
    const h = await sha256Hex(g.secret);
    const row = { id: g.keyId, tenant_id: 't1', user_id: 'u1', key_hash: h, scopes: '["sessions:read","terminal:write"]', revoked_at: null, expires_at: null, is_deleted: 0 };
    const ctx = makeCtxWithDbAndReq(g.token, row);
    await applyAltTokenToContext(ctx);
    expect(ctx.auth).toMatchObject({ tenantId: 't1', userId: 'u1' });
    expect(ctx.apiKey).toMatchObject({ id: g.keyId, scopes: ['sessions:read', 'terminal:write'] });
  });

  it('scope guard allows in-scope, denies out for key ctx', () => {
    const ctx = { apiKey: { id: 'k', scopes: ['sessions:read'] } };
    expect(() => requireScope(ctx as any, 'sessions:read')).not.toThrow();
    expect(() => requireScope(ctx as any, 'terminal:write')).toThrow();
  });

  it('revoked/expired keys are rejected by validate', async () => {
    const g = generateApiKey();
    const h = await sha256Hex(g.secret);
    const revoked = { id: g.keyId, tenant_id: 't', user_id: 'u', key_hash: h, scopes: '[]', revoked_at: '2026', expires_at: null, is_deleted: 0 };
    const v = await validateApiKeyToken(g.token, { prepare: () => ({ bind: () => ({ first: async () => revoked }) }) } as any);
    expect(v).toBeNull();
  });

  it('isTokenLike + validate rejects unknown id', async () => {
    expect(isTokenLike('jk_xxx_yyy')).toBe(true);
    const v = await validateApiKeyToken('jk_xxx_yyy', { prepare: () => ({ bind: () => ({ first: async () => null }) }) } as any);
    expect(v).toBeNull();
  });

  it('jk_ drives real GraphQL partner op (listAiSessions) with scope allow; deny for wrong scope', async () => {
    const g = generateApiKey();
    const h = await sha256Hex(g.secret);
    const row = { id: g.keyId, tenant_id: 't1', user_id: 'u1', key_hash: h, scopes: JSON.stringify(['sessions:read']), revoked_at: null, expires_at: null, is_deleted: 0 };
    const ctx = makeCtxWithDbAndReq(g.token, row);
    const spy = vi.spyOn(desktopRpcMod, 'desktopRpc').mockResolvedValueOnce([] as any);
    const { default: listAiSessions } = await import('../resolvers/ai/list-ai-sessions');
    const res = await listAiSessions(null, { status: null }, ctx as any);
    expect(spy).toHaveBeenCalled();
    expect(res).toEqual([]);

    // wrong scope key
    const rowWrite = { ...row, scopes: JSON.stringify(['sessions:write']) };
    const ctxWrite = makeCtxWithDbAndReq(g.token, rowWrite);
    await expect(listAiSessions(null, {}, ctxWrite as any)).rejects.toThrow(/FORBIDDEN_SCOPE|missing required scope/);
  });

  it('jk_ with plans scope drives agent plan op', async () => {
    const g = generateApiKey();
    const h = await sha256Hex(g.secret);
    const row = { id: g.keyId, tenant_id: 't1', user_id: 'u1', key_hash: h, scopes: JSON.stringify(['plans:read']), revoked_at: null, expires_at: null, is_deleted: 0 };
    const ctx = makeCtxWithDbAndReq(g.token, row);
    const spy = vi.spyOn(desktopRpcMod, 'desktopRpc').mockResolvedValueOnce([] as any);
    const { default: listPlans } = await import('../resolvers/ai/list-agent-plans');
    await listPlans(null, {}, ctx as any);
    expect(spy).toHaveBeenCalled();
  });

  // WSS drive coverage for Task 09: jk_ terminal path + scope + revoke kill on WSS
  // (real transport exercised in chat-relay-do.upgrade.test.ts and .test.ts; here prove using shared validate + scope for terminal)
  it('single jk_ drives WSS terminal path intent (validate succeeds for terminal scopes) + out-of-scope denied + revoke kills', async () => {
    const g = generateApiKey();
    const h = await sha256Hex(g.secret);
    const row = { id: g.keyId, tenant_id: 't1', user_id: 'u1', key_hash: h, scopes: JSON.stringify(['terminal:read', 'terminal:write']), revoked_at: null, expires_at: null, is_deleted: 0 };
    // simulate "WSS upgrade auth" path uses validate
    const v = await validateApiKeyToken(g.token, { prepare: () => ({ bind: () => ({ first: async () => row }) }) } as any);
    expect(v).not.toBeNull();
    expect(v!.apiKey.scopes).toContain('terminal:read');

    // out of scope terminal for subscribe? wait read allows subscribe, but deny write
    const readOnlyRow = { ...row, scopes: JSON.stringify(['terminal:read']) };
    const v2 = await validateApiKeyToken(g.token, { prepare: () => ({ bind: () => ({ first: async () => readOnlyRow }) }) } as any);
    const sctx = { apiKey: v2!.apiKey };
    expect(() => requireScope(sctx as any, 'terminal:read')).not.toThrow();
    try {
      requireScope(sctx as any, 'terminal:write');
      throw new Error('should have thrown for scope');
    } catch (e: any) {
      expect(String(e.code || e.message)).toMatch(/FORBIDDEN_SCOPE|missing required scope/);
    }

    // revoke kills "WSS" (validate now fails)
    const revokedRow = { ...row, revoked_at: new Date().toISOString() };
    const vRev = await validateApiKeyToken(g.token, { prepare: () => ({ bind: () => ({ first: async () => revokedRow }) }) } as any);
    expect(vRev).toBeNull();
  });
});
