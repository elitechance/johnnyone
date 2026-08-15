import { describe, it, expect, vi } from 'vitest';
import { GraphQLError } from 'graphql';
import { desktopRpc } from './desktop-rpc';
import { relayRpc } from './relay-rpc';
import { authorizeForAltToken } from '../auth/api-key';
import { authedCtx } from '../auth/test-authed-ctx';

function emptyDb() {
  return {
    prepare: () => ({
      bind: () => ({
        first: async () => null,
        all: async () => ({ results: [] }),
        run: async () => ({ success: true }),
      }),
    }),
  } as any;
}

function bindRecordingDb(returnRow: { id: string } | null) {
  const bind = vi.fn().mockReturnThis();
  const first = vi.fn().mockResolvedValue(returnRow);
  const prepare = vi.fn(() => ({ bind, first }));
  return { prepare, bind, first } as any;
}

describe('RPC helpers fail closed', () => {
  it('desktopRpc with no Authorization rejects UNAUTHENTICATED', async () => {
    const ctx = { db: emptyDb(), env: { CHAT_RELAY_DO: {} }, auth: {} };
    try {
      await desktopRpc(ctx as any, 'create_agent_plan', { input: {} });
      throw new Error('expected throw');
    } catch (e) {
      expect(e).toBeInstanceOf(GraphQLError);
      expect((e as GraphQLError).extensions.code).toBe('UNAUTHENTICATED');
      expect(String((e as Error).message)).not.toMatch(/No online backend app found/);
    }
  });

  it('relayRpc with no Authorization rejects UNAUTHENTICATED', async () => {
    const ctx = { db: emptyDb(), env: { CHAT_RELAY_DO: {} }, auth: {} };
    try {
      await relayRpc(ctx as any, 'get_session', { id: 's1' });
      throw new Error('expected throw');
    } catch (e) {
      expect(e).toBeInstanceOf(GraphQLError);
      expect((e as GraphQLError).extensions.code).toBe('UNAUTHENTICATED');
    }
  });

  it('authenticated ctx + no online node still throws the offline-node message', async () => {
    const ctx = authedCtx({ tenantId: 't1', userId: 'u1', db: emptyDb() });
    await expect(desktopRpc(ctx, 'list_sessions', {})).rejects.toThrow(
      /No online backend app found/,
    );
    await expect(relayRpc(ctx, 'get_session', { id: 's1' })).rejects.toThrow(
      /No online desktop node found/,
    );
  });

  it('authenticated ctx binds resolveOnlineNode to payload tenant/user, not x-tenant-id', async () => {
    const db = bindRecordingDb(null);
    const verifyJwt = vi.fn(async () => ({ sub: 'uA', tenantId: 'A' }));
    const ctx: any = {
      db,
      env: { CHAT_RELAY_DO: {} },
      request: {
        headers: {
          get(k: string) {
            const key = String(k).toLowerCase();
            if (key === 'authorization') return 'Bearer dummy';
            if (key === 'x-tenant-id') return 'B';
            return null;
          },
        },
      },
      auth: { tenantId: 'B', userId: 'uB', isAuthenticated: true, verifyJwt },
    };
    await expect(desktopRpc(ctx, 'list_sessions', {})).rejects.toThrow(
      /No online backend app found/,
    );
    expect(db.bind).toHaveBeenCalledWith('A', 'uA');
  });

  it('memo: authorizeForAltToken then desktopRpc invokes verifyJwt once', async () => {
    const verifyJwt = vi.fn(async () => ({ sub: 'u', tenantId: 't' }));
    const ctx = authedCtx({ tenantId: 't', userId: 'u', db: emptyDb() });
    ctx.auth.verifyJwt = verifyJwt;
    await authorizeForAltToken(ctx, 'plans:write');
    await expect(desktopRpc(ctx, 'create_agent_plan', { input: {} })).rejects.toThrow(
      /No online backend app found/,
    );
    expect(verifyJwt).toHaveBeenCalledTimes(1);
  });
});
