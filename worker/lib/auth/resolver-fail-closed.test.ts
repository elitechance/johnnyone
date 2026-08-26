import { describe, it, expect, vi } from 'vitest';
import { GraphQLError } from 'graphql';
import { signJwt, verifyJwt } from 'modules/auth/auth-middleware';
import { generateApiKey, sha256Hex } from './api-key';
import { ForbiddenScopeError } from './scopes';
import { authedCtx } from './test-authed-ctx';
import * as desktopRpcMod from '../runtime/desktop-rpc';

const SECRET = 'test-secret-for-phase-00';

const POISON_ROW = {
  id: 'poison-node',
  tenant_id: 'should-not-leak',
  user_id: 'should-not-leak',
  hostname: 'poison',
  os: null,
  arch: null,
  version: null,
  status: 'online',
  capabilities: '[]',
  last_heartbeat_at: null,
  is_deleted: 0,
  created_at: 'now',
  updated_at: 'now',
};

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

function poisonedDb(rows: any[] = [POISON_ROW]) {
  const binds: any[][] = [];
  return {
    binds,
    prepare: () => ({
      bind: (...vals: any[]) => {
        binds.push(vals);
        return {
          all: async () => ({ results: rows }),
          first: async () => rows[0] ?? null,
          run: async () => ({ success: true }),
        };
      },
    }),
  } as any;
}

function stubDbForKey(row: any) {
  return {
    prepare: (sql: string) => ({
      bind: (..._vals: any[]) => ({
        first: async () => (sql.includes('SELECT') ? row : null),
        all: async () => ({ results: [] }),
        run: async () => ({ success: true }),
      }),
    }),
  } as any;
}

async function expectUnauthenticated(p: Promise<unknown>) {
  let result: unknown = Symbol('no-return');
  try {
    result = await p;
  } catch (e) {
    expect(e).toBeInstanceOf(GraphQLError);
    expect((e as GraphQLError).extensions.code).toBe('UNAUTHENTICATED');
    expect(String((e as Error).message)).not.toMatch(/No online backend app found/);
    return;
  }
  throw new Error(`expected UNAUTHENTICATED, got ${JSON.stringify(result)}`);
}

describe('resolver fail-closed (F31 at the resolver)', () => {
  it('listDesktopNodes + absent Authorization → UNAUTHENTICATED; does not return an array', async () => {
    const { default: listDesktopNodes } = await import('../../resolvers/ai/list-desktop-nodes');
    const ctx = { db: poisonedDb(), auth: { isAuthenticated: false } };
    await expectUnauthenticated(listDesktopNodes(null, {} as any, ctx as any));
  });

  it('listDesktopNodes + malformed token → UNAUTHENTICATED', async () => {
    const { default: listDesktopNodes } = await import('../../resolvers/ai/list-desktop-nodes');
    const ctx = {
      db: poisonedDb(),
      request: { headers: headers({ Authorization: 'Bearer not-a-jwt' }) },
      auth: { isAuthenticated: false, verifyJwt: (t: string) => verifyJwt(t, SECRET) },
    };
    await expectUnauthenticated(listDesktopNodes(null, {} as any, ctx as any));
  });

  it('listDesktopNodes + bad-signature JWT → UNAUTHENTICATED', async () => {
    const { default: listDesktopNodes } = await import('../../resolvers/ai/list-desktop-nodes');
    const forged = await signJwt(
      { sub: 'u', tenantId: 't', exp: Math.floor(Date.now() / 1000) + 3600 },
      'wrong-secret',
    );
    const ctx = {
      db: poisonedDb(),
      request: { headers: headers({ Authorization: `Bearer ${forged}` }) },
      auth: { isAuthenticated: false, verifyJwt: (t: string) => verifyJwt(t, SECRET) },
    };
    await expectUnauthenticated(listDesktopNodes(null, {} as any, ctx as any));
  });

  it('listDesktopNodes + expired JWT → UNAUTHENTICATED', async () => {
    const { default: listDesktopNodes } = await import('../../resolvers/ai/list-desktop-nodes');
    const expired = await signJwt(
      { sub: 'u', tenantId: 't', exp: Math.floor(Date.now() / 1000) - 60 },
      SECRET,
    );
    const ctx = {
      db: poisonedDb(),
      request: { headers: headers({ Authorization: `Bearer ${expired}` }) },
      auth: { isAuthenticated: false, verifyJwt: (t: string) => verifyJwt(t, SECRET) },
    };
    await expectUnauthenticated(listDesktopNodes(null, {} as any, ctx as any));
  });

  it('createAgentPlan + absent cred → UNAUTHENTICATED, not offline-node', async () => {
    const { default: createAgentPlan } = await import('../../resolvers/ai/create-agent-plan');
    const ctx = {
      db: poisonedDb(),
      env: { CHAT_RELAY_DO: {} },
      auth: { isAuthenticated: false },
    };
    await expectUnauthenticated(
      createAgentPlan(null, { input: { title: 'p', brief: 'b', runType: 'development' } }, ctx as any),
    );
  });

  it('listDesktopNodes + valid JWT tenant T + spoofed ctx.auth.tenantId returns T rows', async () => {
    const { default: listDesktopNodes } = await import('../../resolvers/ai/list-desktop-nodes');
    const tRow = { ...POISON_ROW, id: 'node-T', tenant_id: 'T', user_id: 'uT', hostname: 'host-T' };
    const db = poisonedDb([tRow]);
    const ctx: any = {
      db,
      request: { headers: headers({ Authorization: 'Bearer dummy', 'x-tenant-id': 'other' }) },
      auth: {
        tenantId: 'other',
        userId: 'other-u',
        isAuthenticated: true,
        verifyJwt: async () => ({ sub: 'uT', tenantId: 'T' }),
      },
    };
    const out = await listDesktopNodes(null, {} as any, ctx);
    expect(Array.isArray(out)).toBe(true);
    expect(out).toHaveLength(1);
    expect(out[0].id).toBe('node-T');
    expect(out[0].tenantId).toBe('T');
    expect(out[0].hostname).toBe('host-T');
    expect(db.binds[0]).toEqual(['T', 'uT']);
  });

  it('createAgentPlan + valid jk_ with plans:write reaches desktopRpc', async () => {
    const g = generateApiKey();
    const h = await sha256Hex(g.secret);
    const row = {
      id: g.keyId,
      tenant_id: 't1',
      user_id: 'u1',
      key_hash: h,
      scopes: JSON.stringify(['plans:write']),
      revoked_at: null,
      expires_at: null,
      is_deleted: 0,
    };
    const spy = vi.spyOn(desktopRpcMod, 'desktopRpc').mockResolvedValueOnce({ ok: true } as any);
    const { default: createAgentPlan } = await import('../../resolvers/ai/create-agent-plan');
    const ctx = {
      db: stubDbForKey(row),
      env: { CHAT_RELAY_DO: {} },
      request: { headers: headers({ Authorization: `Bearer ${g.token}` }) },
      auth: { isAuthenticated: false },
    };
    await createAgentPlan(null, { input: { title: 'p', brief: 'b', runType: 'development' } }, ctx as any);
    expect(spy).toHaveBeenCalledWith(ctx, 'create_agent_plan', expect.anything());
    spy.mockRestore();
  });

  it('startAgentPlan + valid jk_ with plans:write reaches desktopRpc', async () => {
    const g = generateApiKey();
    const h = await sha256Hex(g.secret);
    const row = {
      id: g.keyId,
      tenant_id: 't1',
      user_id: 'u1',
      key_hash: h,
      scopes: JSON.stringify(['plans:write']),
      revoked_at: null,
      expires_at: null,
      is_deleted: 0,
    };
    const spy = vi.spyOn(desktopRpcMod, 'desktopRpc').mockResolvedValueOnce({ ok: true } as any);
    const { default: startAgentPlan } = await import('../../resolvers/ai/start-agent-plan');
    const ctx = {
      db: stubDbForKey(row),
      env: { CHAT_RELAY_DO: {} },
      request: { headers: headers({ Authorization: `Bearer ${g.token}` }) },
      auth: { isAuthenticated: false },
    };
    await startAgentPlan(null, { id: 'p1', phaseId: 'ph1', phaseRunMode: 'auto' }, ctx as any);
    expect(spy).toHaveBeenCalledWith(ctx, 'start_agent_plan', expect.anything());
    spy.mockRestore();
  });

  it('runInitiativeFromPhase + valid jk_ with plans:write reaches desktopRpc', async () => {
    const g = generateApiKey();
    const h = await sha256Hex(g.secret);
    const row = {
      id: g.keyId,
      tenant_id: 't1',
      user_id: 'u1',
      key_hash: h,
      scopes: JSON.stringify(['plans:write']),
      revoked_at: null,
      expires_at: null,
      is_deleted: 0,
    };
    const spy = vi.spyOn(desktopRpcMod, 'desktopRpc').mockResolvedValueOnce({ ok: true } as any);
    const { default: runInitiativeFromPhase } = await import('../../resolvers/ai/run-initiative-from-phase');
    const ctx = {
      db: stubDbForKey(row),
      env: { CHAT_RELAY_DO: {} },
      request: { headers: headers({ Authorization: `Bearer ${g.token}` }) },
      auth: { isAuthenticated: false },
    };
    await runInitiativeFromPhase(null, { id: 'p1', phaseId: 'ph1' }, ctx as any);
    expect(spy).toHaveBeenCalledWith(ctx, 'run_initiative_from_phase', expect.anything());
    spy.mockRestore();
  });

  it('createAgentPlan + valid jk_ without plans:write → FORBIDDEN_SCOPE', async () => {
    const g = generateApiKey();
    const h = await sha256Hex(g.secret);
    const row = {
      id: g.keyId,
      tenant_id: 't1',
      user_id: 'u1',
      key_hash: h,
      scopes: JSON.stringify(['sessions:read']),
      revoked_at: null,
      expires_at: null,
      is_deleted: 0,
    };
    const { default: createAgentPlan } = await import('../../resolvers/ai/create-agent-plan');
    const ctx = {
      db: stubDbForKey(row),
      env: { CHAT_RELAY_DO: {} },
      request: { headers: headers({ Authorization: `Bearer ${g.token}` }) },
      auth: { isAuthenticated: false },
    };
    try {
      await createAgentPlan(null, { input: { title: 'p', brief: 'b', runType: 'development' } }, ctx as any);
      throw new Error('expected FORBIDDEN_SCOPE');
    } catch (e) {
      expect(e).toBeInstanceOf(ForbiddenScopeError);
      expect((e as any).code).toBe('FORBIDDEN_SCOPE');
    }
  });
});

describe('D1-direct identity (01-02)', () => {
  it('listDesktopNodes with authenticated caller still returns mapped rows', async () => {
    const { default: listDesktopNodes } = await import('../../resolvers/ai/list-desktop-nodes');
    const row = { ...POISON_ROW, id: 'n1', tenant_id: 't1', user_id: 'u1', hostname: 'box' };
    const db = poisonedDb([row]);
    const ctx = authedCtx({ tenantId: 't1', userId: 'u1', db });
    const out = await listDesktopNodes(null, {} as any, ctx);
    expect(out[0]).toMatchObject({ id: 'n1', tenantId: 't1', userId: 'u1', hostname: 'box' });
    expect(db.binds[0]).toEqual(['t1', 'u1']);
  });

  it('registerDesktopNode binds tenant+user from identity on write', async () => {
    const { default: registerDesktopNode } = await import('../../resolvers/ai/register-desktop-node');
    const binds: any[][] = [];
    const db = {
      prepare: (sql: string) => ({
        bind: (...vals: any[]) => {
          binds.push(vals);
          return {
            first: async () =>
              sql.includes('SELECT *')
                ? { ...POISON_ROW, id: vals[0] ?? 'new', tenant_id: 't1', user_id: 'u1', hostname: 'h' }
                : null,
            run: async () => ({ success: true }),
          };
        },
      }),
    } as any;
    const ctx = authedCtx({ tenantId: 't1', userId: 'u1', db });
    const out = await registerDesktopNode(null, { input: { hostname: 'h' } }, ctx);
    expect(out.hostname).toBe('h');
    expect(binds.some((b) => b.includes('t1') && b.includes('u1'))).toBe(true);
  });

  it('updateDesktopNodeStatus binds tenant from identity on write', async () => {
    const { default: updateDesktopNodeStatus } = await import('../../resolvers/ai/update-desktop-node-status');
    const binds: any[][] = [];
    const db = {
      prepare: (sql: string) => ({
        bind: (...vals: any[]) => {
          binds.push(vals);
          return {
            first: async () =>
              sql.includes('SELECT *')
                ? { ...POISON_ROW, id: 'n1', tenant_id: 't1', status: 'busy' }
                : { id: 'n1' },
            run: async () => ({ success: true }),
          };
        },
      }),
    } as any;
    const ctx = authedCtx({ tenantId: 't1', userId: 'u1', db });
    await updateDesktopNodeStatus(null, { id: 'n1', status: 'busy' }, ctx);
    expect(binds.some((b) => b.includes('t1') && b.includes('n1'))).toBe(true);
  });

  it('subscription subscribe throws UNAUTHENTICATED when identity is missing', async () => {
    const onStatus = (await import('../../resolvers/ai/on-desktop-node-status')).default;
    const onMsg = (await import('../../resolvers/ai/on-relay-chat-message')).default;
    const onDelta = (await import('../../resolvers/ai/on-relay-chat-delta')).default;
    const pubsub = { subscribe: vi.fn() };
    const ctx = { auth: { isAuthenticated: false }, pubsub, request: new Request('http://localhost') };
    await expectUnauthenticated(onStatus.subscribe(null, {} as any, ctx as any));
    await expectUnauthenticated(onMsg.subscribe(null, { sessionId: 's1' }, ctx as any));
    await expectUnauthenticated(onDelta.subscribe(null, { relayId: 'r1' }, ctx as any));
    expect(pubsub.subscribe).not.toHaveBeenCalled();
  });
});
