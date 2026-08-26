import { describe, it, expect, vi } from 'vitest';
import { GraphQLError } from 'graphql';
import { generateApiKey, sha256Hex } from './api-key';

function stubDbForKey(row: any) {
  return {
    prepare: () => ({
      bind: () => ({
        first: async () => row,
        run: async () => ({ success: true }),
      }),
    }),
  } as any;
}

function jwtWsFixture(pubsub: { subscribe: ReturnType<typeof vi.fn> }, extras: Record<string, unknown> = {}) {
  return {
    db: stubDbForKey(null),
    auth: {
      isAuthenticated: true,
      tenantId: 't1',
      userId: 'u1',
      verifyJwt: async () => {
        throw new Error('not used on D13 branch');
      },
    },
    request: new Request('http://localhost'),
    pubsub,
    ...extras,
  };
}

describe('subscription auth — generated graphql-ws fixture (D13)', () => {
  it('onDesktopNodeStatus.subscribe + JWT ws fixture → desktop-node-status:t1', async () => {
    const { default: onStatus } = await import('../../resolvers/ai/on-desktop-node-status');
    const subscribe = vi.fn().mockReturnValue('iter');
    const ctx = jwtWsFixture({ subscribe });
    await onStatus.subscribe(null, {} as any, ctx as any);
    expect(subscribe).toHaveBeenCalledWith('desktop-node-status:t1');
  });

  it('onRelayChatMessage.subscribe + JWT ws fixture → topic includes u1', async () => {
    const { default: onMsg } = await import('../../resolvers/ai/on-relay-chat-message');
    const subscribe = vi.fn().mockReturnValue('iter');
    const ctx = jwtWsFixture({ subscribe });
    await onMsg.subscribe(null, { sessionId: 'sess-1' }, ctx as any);
    expect(subscribe).toHaveBeenCalledWith('relay-chat-message:u1:sess-1');
  });

  it('onRelayChatDelta.subscribe + JWT ws fixture → subscribe called', async () => {
    const { default: onDelta } = await import('../../resolvers/ai/on-relay-chat-delta');
    const subscribe = vi.fn().mockReturnValue('iter');
    const ctx = jwtWsFixture({ subscribe });
    await onDelta.subscribe(null, { relayId: 'rel-1' }, ctx as any);
    expect(subscribe).toHaveBeenCalledWith('relay-chat-delta:rel-1');
  });

  it('graphql-ws isAuthenticated:false (jk_/failed JWT) → UNAUTHENTICATED even if db would validate a jk_ row', async () => {
    const g = generateApiKey();
    const h = await sha256Hex(g.secret);
    const row = {
      id: g.keyId,
      tenant_id: 't1',
      user_id: 'u1',
      key_hash: h,
      scopes: '["terminal:read"]',
      revoked_at: null,
      expires_at: null,
      is_deleted: 0,
    };
    const subscribe = vi.fn();
    const ctx = {
      db: stubDbForKey(row),
      auth: {
        isAuthenticated: false,
        tenantId: null,
        userId: null,
        verifyJwt: async () => {
          throw new Error('jk_ is not a JWT');
        },
      },
      request: new Request('http://localhost'),
      pubsub: { subscribe },
    };
    const onStatus = (await import('../../resolvers/ai/on-desktop-node-status')).default;
    const onMsg = (await import('../../resolvers/ai/on-relay-chat-message')).default;
    const onDelta = (await import('../../resolvers/ai/on-relay-chat-delta')).default;
    const calls = [
      () => onStatus.subscribe(null, {} as any, ctx as any),
      () => onMsg.subscribe(null, { sessionId: 's' }, ctx as any),
      () => onDelta.subscribe(null, { relayId: 'r' }, ctx as any),
    ];
    for (const call of calls) {
      try {
        await call();
        throw new Error('expected UNAUTHENTICATED');
      } catch (e) {
        expect(e).toBeInstanceOf(GraphQLError);
        expect((e as GraphQLError).extensions.code).toBe('UNAUTHENTICATED');
      }
    }
    expect(subscribe).not.toHaveBeenCalled();
  });
});
