/**
 * Live UI bugs 2, item 1: the plan chat got no live events.
 *
 * The desktop's `chat_delta` / `chat_message` envelopes reached the ChatRelayDO and were only
 * re-broadcast to raw relay sockets. The web listens through graphql-ws subscriptions, which read
 * lokal's PubSubDO topics (`relay-chat-delta:<relayId>`, `relay-chat-message:<userId>:<sessionId>`),
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
    deserializeAttachment: () => ({ tenantId: 't1', userId: 'u1', clientType: 'desktop' }),
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
        topic: 'relay-chat-delta:r1',
        data: { relayId: 'r1', sessionId: 's1', delta: 'part1', chunkType: 'text', isFinal: false },
      },
    ]);
    expect(
      relayPublications({ type: 'chat_message', data: { relayId: 'r1', sessionId: 's1', role: 'assistant', content: 'hi' } }, 'u1'),
    ).toEqual([{ topic: 'relay-chat-message:u1:s1', data: { relayId: 'r1', sessionId: 's1', role: 'assistant', content: 'hi' } }]);
  });

  it('publishes nothing without the keys, without an owner, or for other envelope types', () => {
    expect(relayPublications({ type: 'chat_delta', data: {} }, 'u1')).toEqual([]);
    expect(relayPublications({ type: 'chat_message', data: { sessionId: 's1', content: 'x' } }, undefined)).toEqual([]);
    expect(relayPublications({ type: 'chat_complete', data: { relayId: 'r1', sessionId: 's1' } }, 'u1')).toEqual([]);
    expect(relayPublications({ type: 'terminal_screen', data: { sessionId: 's1' } }, 'u1')).toEqual([]);
  });

  it('uses the same room rule as the generated pubsub (after the last colon)', () => {
    expect(pubsubRoom('relay-chat-delta:r1')).toBe('r1');
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
      ['r1', 'relay-chat-delta:r1'],
      ['r1', 'relay-chat-delta:r1'],
      ['r1', 'relay-chat-delta:r1'],
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
