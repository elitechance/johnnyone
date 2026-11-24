/**
 * Hotfix live-chat-2, part 2: after #83 the subscriptions looked healthy but delivered nothing, so
 * the store-poll fallback (which started only on a subscription ERROR) never ran, and the plan chat
 * sat on "Waiting for the plan agent…" for minutes with the input disabled.
 *
 * Now a turn in flight that hears nothing live for TURN_LIVE_SILENCE_MS is settled from the store
 * anyway; every live event re-arms that watchdog. The REAL AiChatService over a fake API (no network).
 */
import '@angular/compiler';
import { Injector, runInInjectionContext } from '@angular/core';
import { NEVER, Observable, Subject, of } from 'rxjs';
import { afterEach, beforeEach, describe, expect, it, vi } from 'vitest';
import { TURN_FALLBACK_POLL_MS, TURN_LIVE_SILENCE_MS } from '../../../../ui/src/services/ai-chat-logic';
import { AiChatService } from '../../../../ui/src/services/ai-chat.service';
import { JohnnyApiService, type RelayChatDeltaMsg, type RelayChatMessageMsg } from '../../../../ui/src/services/johnny-api.service';

const stamp = () => new Date().toISOString().replace('T', ' ').slice(0, 19);

function harness(opts: { deltas?: Subject<RelayChatDeltaMsg>; messages?: Subject<RelayChatMessageMsg> } = {}) {
  let served: unknown[] = [];
  const listMessages = vi.fn(() => of(served));
  const api = {
    getSession: (id: string) => of({ id, title: id }),
    listMessages,
    onRelayChatMessage: () => opts.messages ?? NEVER,
    onRelayChatDelta: () => opts.deltas ?? NEVER,
    sendRelayMessage: () => of({ relayId: 'r1' }),
  };
  const injector = Injector.create({ providers: [{ provide: JohnnyApiService, useValue: api }] });
  const chat = runInInjectionContext(injector, () => new AiChatService());
  const answer = () => {
    served = [
      { id: 'u', sessionId: 'S', role: 'user', content: 'hi', createdAt: stamp() },
      { id: 'a', sessionId: 'S', role: 'assistant', content: 'stored answer', createdAt: stamp() },
    ];
  };
  return { chat, listMessages, answer };
}

describe('live-silence watchdog (hotfix live-chat-2)', () => {
  beforeEach(() => vi.useFakeTimers());
  afterEach(() => vi.useRealTimers());

  it('healthy-looking but silent subscriptions: the turn settles from the store', () => {
    const { chat, listMessages, answer } = harness();
    chat.loadSession('S');
    listMessages.mockClear();
    chat.sendMessage('hi');
    expect(chat.isStreaming()).toBe(true);
    vi.advanceTimersByTime(TURN_LIVE_SILENCE_MS - 1);
    vi.advanceTimersByTime(TURN_FALLBACK_POLL_MS);
    expect(listMessages).not.toHaveBeenCalled(); // not yet: still inside the silence window
    answer();
    vi.advanceTimersByTime(1 + TURN_FALLBACK_POLL_MS);
    expect(listMessages).toHaveBeenCalled();
    expect(chat.isStreaming()).toBe(false);
    expect(chat.messages().at(-1)?.content).toBe('stored answer');
  });

  it('live deltas keep re-arming it: no store poll while the reply streams', () => {
    const deltas = new Subject<RelayChatDeltaMsg>();
    const { chat, listMessages } = harness({ deltas });
    chat.loadSession('S');
    listMessages.mockClear();
    chat.sendMessage('hi');
    for (let i = 0; i < 5; i++) {
      vi.advanceTimersByTime(TURN_LIVE_SILENCE_MS - 1000);
      deltas.next({ relayId: 'r1', sessionId: 'S', delta: `p${i}`, chunkType: 'text', isFinal: false });
    }
    vi.advanceTimersByTime(TURN_FALLBACK_POLL_MS);
    expect(listMessages).not.toHaveBeenCalled();
    expect(chat.isStreaming()).toBe(true);
  });

  it('a live final message ends the turn and the watchdog with it', () => {
    const messages = new Subject<RelayChatMessageMsg>();
    const { chat, listMessages } = harness({ messages });
    chat.loadSession('S');
    listMessages.mockClear();
    chat.sendMessage('hi');
    messages.next({ relayId: 'r1', sessionId: 'S', role: 'assistant', content: 'live' });
    expect(chat.isStreaming()).toBe(false);
    vi.advanceTimersByTime(TURN_LIVE_SILENCE_MS * 3);
    expect(listMessages).not.toHaveBeenCalled();
  });

  it('the turn\'s delta subscription ends with the turn (its socket closes, it does not linger)', () => {
    let active = 0;
    const deltas = new Subject<RelayChatDeltaMsg>();
    const { chat } = harness();
    (chat as any).api.onRelayChatDelta = () =>
      new Observable<RelayChatDeltaMsg>((sub) => {
        active++;
        const inner = deltas.subscribe(sub);
        return () => {
          active--;
          inner.unsubscribe();
        };
      });
    chat.loadSession('S');
    chat.sendMessage('hi');
    expect(active).toBe(1);
    deltas.next({ relayId: 'r1', sessionId: 'S', delta: 'done', chunkType: 'result', isFinal: true });
    expect(chat.isStreaming()).toBe(false);
    expect(active).toBe(0);
    // A late relayId for an already-settled turn opens nothing.
    chat.subscribeToDeltas('r-late');
    expect(active).toBe(0);
  });

  it('the silence window is short (seconds, not minutes)', () => {
    expect(TURN_LIVE_SILENCE_MS).toBeLessThanOrEqual(10_000);
  });
});

describe('subscription sockets close cleanly (hotfix live-chat-2)', () => {
  it('unsubscribing closes with 1000, and the keepalive pings inside the observed ~20 s drop', async () => {
    const { GraphQLClient } = await import('../../../../ui/src/services/graphql-client');
    const { KEEPALIVE_MS, CLIENT_CLOSE_CODE } = await import('../../../../ui/src/services/graphql-ws-logic');
    const closes: Array<number | undefined> = [];
    class FakeSocket {
      static OPEN = 1;
      readyState = 1;
      onopen: any = null;
      onmessage: any = null;
      onerror: any = null;
      onclose: any = null;
      send() {}
      close(code?: number) {
        closes.push(code);
      }
    }
    vi.stubGlobal('WebSocket', FakeSocket as any);
    try {
      const c = Object.create(GraphQLClient.prototype) as any;
      Object.assign(c, { wsUrl: 'ws://x/graphql', extraHeaders: {}, authRefresh: null, inflightAuthRefresh: null });
      const sub = c.subscribe('subscription { x }').subscribe({ error: () => undefined });
      sub.unsubscribe();
      expect(closes).toEqual([CLIENT_CLOSE_CODE]);
      expect(CLIENT_CLOSE_CODE).toBe(1000);
      expect(KEEPALIVE_MS).toBeLessThan(20_000);
    } finally {
      vi.unstubAllGlobals();
    }
  });
});
