/**
 * Live UI bugs 2, item 1: the plan chat got no live events.
 *
 * The desktop's `chat_delta` / `chat_message` envelopes reached the ChatRelayDO and were only
 * re-broadcast to raw relay sockets. The web listens through graphql-ws subscriptions, which read
 * lokal's PubSubDO topics (`relay-chat-delta:<userId>:<relayId>`, `relay-chat-message:<userId>:<sessionId>`),
 * and nothing published there. These tests drive the REAL ChatRelayDO with a fake PUBSUB_DO
 * namespace (no network) and assert the publishes.
 */
import { describe, it, expect, vi, beforeEach } from 'vitest';
import { ChatRelayDO } from './chat-relay-do';
import { pubsubRoom, relayPublications } from './relay-pubsub';

function fakePubSub() {
  const published: { room: string; topic: string; data: any }[] = [];
  const ns = {
    idFromName: vi.fn((name: string) => name),
    get: vi.fn((room: string) => ({
      fetch: vi.fn(async (req: Request) => {
        const body = (await req.json()) as { topic: string; data: unknown };
        expect(new URL(req.url).pathname).toBe('/publish');
        expect(req.method).toBe('POST');
        published.push({ room, topic: body.topic, data: body.data });
        return new Response('ok');
      }),
    })),
  };
  return { ns, published };
}

function makeDo(pubsubNs: unknown) {
  const state = {
    acceptWebSocket: vi.fn(),
    getTags: vi.fn(() => ['desktop', 'tenant:t1', 'user:u1']),
    getWebSockets: vi.fn(() => []),
  };
  const env = { JWT_SECRET: 'x', DB: {} as any, PUBSUB_DO: pubsubNs };
  return { relay: new ChatRelayDO(state as any, env), state };
}

/** A desktop socket whose upgrade attachment says it belongs to user u1. */
function desktopWs() {
  return {
    send: vi.fn(),
    deserializeAttachment: () => ({ tenantId: 't1', userId: 'u1', clientType: 'desktop', desktopNodeId: 'n1' }),
  } as any;
}

const delta = (n: number, extra: Record<string, unknown> = {}) => ({
  type: 'chat_delta',
  data: { relayId: 'r1', sessionId: 's1', delta: `part${n}`, chunkType: 'text', isFinal: false, ...extra },
});

describe('relayPublications (pure)', () => {
  it('maps a delta to its relayId topic and a message to the owner+session topic', () => {
    expect(relayPublications(delta(1), 'u1')).toEqual([
      {
        topic: 'relay-chat-delta:u1:r1',
        data: { relayId: 'r1', sessionId: 's1', delta: 'part1', chunkType: 'text', isFinal: false },
      },
    ]);
    expect(
      relayPublications({ type: 'chat_message', data: { relayId: 'r1', sessionId: 's1', role: 'assistant', content: 'hi' } }, 'u1'),
    ).toEqual([{ topic: 'relay-chat-message:u1:s1', data: { relayId: 'r1', sessionId: 's1', role: 'assistant', content: 'hi' } }]);
  });

  it('publishes nothing without the keys, without an owner, or for other envelope types', () => {
    expect(relayPublications({ type: 'chat_delta', data: {} }, 'u1')).toEqual([]);
    expect(relayPublications(delta(1), undefined)).toEqual([]);
    expect(relayPublications({ type: 'chat_message', data: { sessionId: 's1', content: 'x' } }, undefined)).toEqual([]);
    expect(relayPublications({ type: 'chat_complete', data: { relayId: 'r1', sessionId: 's1' } }, 'u1')).toEqual([]);
    expect(relayPublications({ type: 'terminal_screen', data: { sessionId: 's1' } }, 'u1')).toEqual([]);
  });

  it('uses the same room rule as the generated pubsub (after the last colon)', () => {
    expect(pubsubRoom('relay-chat-delta:u1:r1')).toBe('r1');
    expect(pubsubRoom('relay-chat-message:u1:s1')).toBe('s1');
  });
});

describe('ChatRelayDO feeds the graphql subscriptions', () => {
  let pubsub: ReturnType<typeof fakePubSub>;
  beforeEach(() => {
    pubsub = fakePubSub();
  });

  it('a desktop turn (deltas, then the final message) is published in order to the subscription topics', async () => {
    const { relay } = makeDo(pubsub.ns);
    const ws = desktopWs();
    await relay.webSocketMessage(ws, JSON.stringify(delta(1)));
    await relay.webSocketMessage(ws, JSON.stringify(delta(2)));
    await relay.webSocketMessage(ws, JSON.stringify(delta(3, { chunkType: 'result', isFinal: true, delta: '' })));
    await relay.webSocketMessage(
      ws,
      JSON.stringify({ type: 'chat_message', data: { relayId: 'r1', sessionId: 's1', role: 'assistant', content: 'part1part2' } }),
    );
    await relay.webSocketMessage(ws, JSON.stringify({ type: 'chat_complete', data: { relayId: 'r1', sessionId: 's1', messageId: 'm' } }));

    expect(pubsub.published.map((p) => [p.room, p.topic])).toEqual([
      ['r1', 'relay-chat-delta:u1:r1'],
      ['r1', 'relay-chat-delta:u1:r1'],
      ['r1', 'relay-chat-delta:u1:r1'],
      ['s1', 'relay-chat-message:u1:s1'],
    ]);
    expect(pubsub.published.slice(0, 2).map((p) => p.data.delta)).toEqual(['part1', 'part2']);
    expect(pubsub.published[2].data.isFinal).toBe(true);
    expect(pubsub.published[3].data).toEqual({ relayId: 'r1', sessionId: 's1', role: 'assistant', content: 'part1part2' });
  });

  it('a non-desktop socket cannot inject a delta or message into the topics', async () => {
    const { relay, state } = makeDo(pubsub.ns);
    state.getTags.mockReturnValue(['mobile', 'tenant:t1', 'user:u2']);
    const ws = { send: vi.fn(), deserializeAttachment: () => ({ tenantId: 't1', userId: 'u2', clientType: 'mobile' }) } as any;
    await relay.webSocketMessage(ws, JSON.stringify(delta(1)));
    await relay.webSocketMessage(ws, JSON.stringify({ type: 'chat_message', data: { relayId: 'r1', sessionId: 's1', content: 'x' } }));
    expect(pubsub.published).toEqual([]);
  });

  it('a pubsub failure does not break the relay (best effort)', async () => {
    const ns = { idFromName: (n: string) => n, get: () => ({ fetch: async () => { throw new Error('down'); } }) };
    const { relay } = makeDo(ns);
    await expect(relay.webSocketMessage(desktopWs(), JSON.stringify(delta(1)))).resolves.toBeUndefined();
  });
});

/** Review findings L-1-1 / L-1-2 / L-1-3 (fence tests folded in from the reviewer). */
describe('relay publish hardening (L-1-1, L-1-2, L-1-3)', () => {
  function mk(fetchImpl: (req: Request) => Promise<Response>, attachment: Record<string, unknown> = { tenantId: 't1', userId: 'u1', clientType: 'desktop', desktopNodeId: 'n1' }) {
    const published: string[] = [];
    const ns = {
      idFromName: (n: string) => n,
      get: () => ({
        fetch: async (req: Request) => {
          const b: any = await req.clone().json();
          const r = await fetchImpl(req);
          published.push(b.topic + '|' + (b.data.delta ?? b.data.content));
          return r;
        },
      }),
    };
    const state = { acceptWebSocket: vi.fn(), getTags: vi.fn(() => ['desktop']), getWebSockets: vi.fn(() => []) };
    const relay = new ChatRelayDO(state as any, { JWT_SECRET: 'x', DB: {} as any, PUBSUB_DO: ns });
    const ws = { send: vi.fn(), deserializeAttachment: () => attachment } as any;
    return { relay, ws, published };
  }
  const d = (n: string) => JSON.stringify({ type: 'chat_delta', data: { relayId: 'r1', sessionId: 's1', delta: n, chunkType: 'text' } });

  it('L-1-1: another user subscribing to the same relayId gets nothing (owner-scoped topic)', async () => {
    const { relay, ws, published } = mk(async () => new Response('ok'));
    await relay.webSocketMessage(ws, d('secret'));
    const { default: onDelta } = await import('../../resolvers/ai/on-relay-chat-delta');
    const topicFor = async (userId: string) => {
      const subscribe = vi.fn().mockReturnValue('iter');
      const ctx = { auth: { isAuthenticated: true, tenantId: 't1', userId }, request: new Request('http://localhost'), pubsub: { subscribe } };
      await onDelta.subscribe(null, { relayId: 'r1' }, ctx as any);
      return subscribe.mock.calls[0][0] as string;
    };
    const topics = published.map((p) => p.split('|')[0]);
    expect(topics).toEqual([await topicFor('u1')]);
    expect(topics).not.toContain(await topicFor('u2'));
    expect(pubsubRoom(await topicFor('u2'))).toBe('r1'); // same room, different topic: nothing delivered
  });

  it('hotfix: the registered desktop node publishes even when it authenticated with a jk_ API key', async () => {
    const { relay, ws, published } = mk(async () => new Response('ok'), {
      tenantId: 't1', userId: 'u1', clientType: 'desktop', desktopNodeId: 'n1', apiKey: { id: 'k', scopes: ['terminal:write'] },
    });
    await relay.webSocketMessage(ws, d('live'));
    expect(published).toEqual(['relay-chat-delta:u1:r1|live']);
  });

  it('L-1-2: a socket flagged desktop that is NOT a verified node (jk_ or JWT) cannot inject', async () => {
    for (const apiKey of [{ id: 'k', scopes: ['terminal:write'] }, undefined]) {
      const { relay, ws, published } = mk(async () => new Response('ok'), {
        tenantId: 't1', userId: 'u1', clientType: 'desktop', apiKey,
      });
      await relay.webSocketMessage(ws, d('fake'));
      await relay.webSocketMessage(ws, JSON.stringify({ type: 'chat_message', data: { relayId: 'r1', sessionId: 's1', content: 'fake' } }));
      expect(published).toEqual([]);
    }
  });

  it('L-1-3 (W5): concurrent deliveries keep order when the first publish is slow', async () => {
    let first = true;
    const { relay, ws, published } = mk(async () => {
      if (first) {
        first = false;
        await new Promise((r) => setTimeout(r, 30));
      }
      return new Response('ok');
    });
    await Promise.all([relay.webSocketMessage(ws, d('a')), relay.webSocketMessage(ws, d('b'))]);
    expect(published.map((p) => p.split('|')[1])).toEqual(['a', 'b']);
  });

  it('L-1-3 (W4): a failed publish does not stop later publishes', async () => {
    let n = 0;
    const { relay, ws, published } = mk(async () => {
      if (n++ === 0) throw new Error('down');
      return new Response('ok');
    });
    await relay.webSocketMessage(ws, d('a'));
    await relay.webSocketMessage(ws, d('b'));
    expect(published.map((p) => p.split('|')[1])).toEqual(['b']);
  });

  it('L-1-3 (W3): a payload userId is ignored for both topics', async () => {
    const { relay, ws, published } = mk(async () => new Response('ok'));
    await relay.webSocketMessage(
      ws,
      JSON.stringify({ type: 'chat_message', data: { relayId: 'r1', sessionId: 's1', content: 'x', userId: 'victim' } }),
    );
    await relay.webSocketMessage(
      ws,
      JSON.stringify({ type: 'chat_delta', data: { relayId: 'r1', sessionId: 's1', delta: 'y', chunkType: 'text', userId: 'victim' } }),
    );
    expect(published).toEqual(['relay-chat-message:u1:s1|x', 'relay-chat-delta:u1:r1|y']);
  });
});
