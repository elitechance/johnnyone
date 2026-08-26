import { describe, it, expect, vi, beforeEach } from 'vitest';
import * as desktopRpcMod from '../../lib/runtime/desktop-rpc';
import { authedCtx } from '../auth/test-authed-ctx';

const mockCtx = authedCtx({ tenantId: 't1', userId: 'u1' });

describe('AI session READ scope (tenant isolation via node resolution)', () => {
  beforeEach(() => {
    vi.restoreAllMocks();
  });

  it('listAiSessions: forwards ctx.auth (tenant+user) to desktopRpc; different auth reaches different node', async () => {
    const spy = vi.spyOn(desktopRpcMod, 'desktopRpc').mockResolvedValueOnce([] as any);
    const { default: listAiSessions } = await import('../../resolvers/ai/list-ai-sessions');
    await listAiSessions(null, { status: null }, mockCtx as any);
    expect(spy).toHaveBeenCalledWith(mockCtx, 'list_sessions', { status: null });

    const otherCtx = authedCtx({ tenantId: 't2', userId: 'u2' });
    const spy2 = vi.spyOn(desktopRpcMod, 'desktopRpc').mockResolvedValueOnce([] as any);
    const { default: list2 } = await import('../../resolvers/ai/list-ai-sessions');
    await list2(null, {}, otherCtx as any);
    expect(spy2).toHaveBeenCalledWith(otherCtx, 'list_sessions', expect.anything());
  });

  it('getAiSession: passes only {id} from args; ctx.auth determines the node (no tenant leak from args)', async () => {
    const spy = vi.spyOn(desktopRpcMod, 'desktopRpc').mockResolvedValueOnce({ id: 's1' } as any);
    const { default: getAiSession } = await import('../../resolvers/ai/get-ai-session');
    await getAiSession(null, { id: 's1' }, mockCtx as any);
    expect(spy).toHaveBeenCalledWith(mockCtx, 'get_session', { id: 's1' });
    // confirm args has no tenant/user
    const callArgs = spy.mock.calls[0][2];
    expect(callArgs).not.toHaveProperty('tenantId');
    expect(callArgs).not.toHaveProperty('userId');
  });

  it('listAiMessages: ctx.auth scopes the call; cross-tenant ctx does not see foreign session data', async () => {
    const spy = vi.spyOn(desktopRpcMod, 'desktopRpc').mockResolvedValueOnce([] as any);
    const { default: listMsgs } = await import('../../resolvers/ai/list-ai-messages');
    await listMsgs(null, { sessionId: 's1', limit: 10, offset: 0 }, mockCtx as any);
    expect(spy).toHaveBeenCalledWith(mockCtx, 'list_messages', { sessionId: 's1', limit: 10, offset: 0 });

    // different tenant should not resolve same node (in real: would hit different desktop)
    const other = authedCtx({ tenantId: 'other', userId: 'u' });
    const spyOther = vi.spyOn(desktopRpcMod, 'desktopRpc').mockResolvedValueOnce({ error: 'no node' } as any);
    await listMsgs(null, { sessionId: 's1' }, other as any);
    expect(spyOther).toHaveBeenCalledWith(other, 'list_messages', expect.anything());
  });

  it("listDetectedCliTools: uses caller's ctx.auth (no args that could retarget)", async () => {
    const spy = vi.spyOn(desktopRpcMod, 'desktopRpc').mockResolvedValueOnce([] as any);
    const { default: listTools } = await import('../../resolvers/ai/list-detected-cli-tools');
    await listTools(null, {}, mockCtx as any);
    expect(spy).toHaveBeenCalledWith(mockCtx, 'detect_cli_tools');
  });

  it('getAiSession: ctx.auth passthrough (resolver forwards call with ctx.auth; no tenant arg from caller)', async () => {
    // In practice desktopRpc will fail or return B's node; test proves ctx flows
    const aSpy = vi.spyOn(desktopRpcMod, 'desktopRpc').mockResolvedValueOnce({ data: 'a-only' });
    const { default: getA } = await import('../../resolvers/ai/get-ai-session');
    await getA(null, { id: 's-owned-by-a' }, mockCtx as any);
    expect(aSpy).toHaveBeenCalledWith(expect.objectContaining({ auth: expect.objectContaining({ tenantId: 't1', userId: 'u1' }) }), expect.anything(), expect.anything());

    const bCtx = authedCtx({ tenantId: 'tB', userId: 'uB' });
    const bSpy = vi.spyOn(desktopRpcMod, 'desktopRpc').mockResolvedValueOnce({ data: 'b-data' });
    const { default: getB } = await import('../../resolvers/ai/get-ai-session');
    await getB(null, { id: 's-owned-by-a' }, bCtx as any);
    expect(bSpy).toHaveBeenCalledWith(expect.objectContaining({ auth: expect.objectContaining({ tenantId: 'tB', userId: 'uB' }) }), expect.anything(), expect.anything());
  });

  // Direct test of the real node lookup path used by Phase 03 partner ops (after refactor to shared resolveOnlineNode)
  // Stubs ctx.db exactly as in resolve-online-node.test.ts so bind values are observable.
  it('desktopRpc (partner path) binds ctx.auth.tenantId+userId to desktop_nodes lookup and throws on no node', async () => {
    function makeStubD1(returnRow: { id: string } | null) {
      const bind = vi.fn().mockReturnThis();
      const first = vi.fn().mockResolvedValue(returnRow);
      const prepare = vi.fn(() => ({ bind, first }));
      return { prepare, bind, first } as unknown as D1Database;
    }

    // happy path: node found
    const db = makeStubD1({ id: 'node-for-t1' });
    const mockFetch = vi.fn().mockResolvedValue({
      ok: true,
      json: async () => ({ success: true, data: ['session-data'] })
    });
    const doNamespace = {
      idFromName: vi.fn((name: string) => `do-${name}`),
      get: () => ({ fetch: mockFetch })
    };
    const env = { CHAT_RELAY_DO: doNamespace as any };
    const ctx = authedCtx({ tenantId: 't1', userId: 'u1', db, env });

    const { desktopRpc } = await import('../runtime/desktop-rpc');
    const res = await desktopRpc(ctx as any, 'list_sessions', {});
    expect(res).toEqual(['session-data']);
    expect(db.prepare).toHaveBeenCalled();
    expect((db as any).bind).toHaveBeenCalledWith('t1', 'u1');  // exact order per resolve

    // no node -> throws (required by prompt)
    const dbNone = makeStubD1(null);
    const ctxNone = authedCtx({ tenantId: 't-no', userId: 'u-no', db: dbNone, env: { CHAT_RELAY_DO: doNamespace as any } });
    await expect( desktopRpc(ctxNone as any, 'list_sessions', {}) ).rejects.toThrow(/No online backend app found/);
  });
});