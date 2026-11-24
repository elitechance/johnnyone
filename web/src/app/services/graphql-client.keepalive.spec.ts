/**
 * Live UI bugs 2, item 1: the plan chat's message subscription died with `SubscriptionClosedError
 * … 1006` (an idle socket dropped upstream), and the reply then came only from the store poll.
 *
 * The client now pings while a subscription socket is open, answers a server ping, and reconnects a
 * DROPPED socket (1006, …) with a bounded backoff, telling the caller so it can settle a turn in
 * flight from the store. A fake `WebSocket` stands in for the socket (no network).
 */
import '@angular/compiler';
import { Injector, runInInjectionContext } from '@angular/core';
import { NEVER, Observable, Subject, of } from 'rxjs';
import { afterEach, beforeEach, describe, expect, it, vi } from 'vitest';
import { GraphQLClient } from '../../../../ui/src/services/graphql-client';
import {
  KEEPALIVE_MS,
  RECONNECT_JITTER,
  RECONNECT_MAX_ATTEMPTS,
  RECONNECT_STABLE_MS,
  SubscriptionClosedError,
  reconnectDelayMs,
  shouldReconnectClose,
} from '../../../../ui/src/services/graphql-ws-logic';
import { TURN_FALLBACK_POLL_MS, classifyDelta, replyAlreadyStored } from '../../../../ui/src/services/ai-chat-logic';
import { AiChatService } from '../../../../ui/src/services/ai-chat.service';
import { JohnnyApiService, type RelayChatDeltaMsg, type RelayChatMessageMsg } from '../../../../ui/src/services/johnny-api.service';

class FakeSocket {
  static OPEN = 1;
  static all: FakeSocket[] = [];
  readyState = 0;
  sent: any[] = [];
  closed = false;
  onopen: ((e?: unknown) => void) | null = null;
  onmessage: ((e: { data: string }) => void) | null = null;
  onerror: ((e?: unknown) => void) | null = null;
  onclose: ((e: { wasClean: boolean; code: number; reason?: string }) => void) | null = null;
  constructor(public url: string, public protocol: string) {
    FakeSocket.all.push(this);
  }
  send(data: string) {
    this.sent.push(JSON.parse(data));
  }
  close() {
    this.closed = true;
  }
  open() {
    this.readyState = FakeSocket.OPEN;
    this.onopen?.();
  }
  receive(msg: unknown) {
    this.onmessage?.({ data: JSON.stringify(msg) });
  }
  /** The browser's view of a dropped connection: `error`, then `close` 1006 (never clean). */
  drop() {
    this.readyState = 3;
    this.onerror?.();
    this.onclose?.({ wasClean: false, code: 1006 });
  }
  /** Open + ack (the subscription is live). */
  live() {
    this.open();
    this.receive({ type: 'connection_ack' });
  }
}

function makeClient() {
  const c = Object.create(GraphQLClient.prototype) as GraphQLClient;
  (c as any).apiUrl = 'http://example.test/graphql';
  (c as any).hostApiUrl = '';
  (c as any).wsUrl = 'ws://example.test/graphql';
  (c as any).extraHeaders = {};
  (c as any).localHostHealth = null;
  (c as any).authRefresh = null;
  (c as any).inflightAuthRefresh = null;
  return c;
}

const types = (ws: FakeSocket) => ws.sent.map((m) => m.type);
/** The longest a reconnect may wait for `attempt` (jitter at its top). */
const maxDelay = (attempt: number) => reconnectDelayMs(attempt, 1);

describe('subscription keepalive + reconnect (item 1)', () => {
  beforeEach(() => {
    FakeSocket.all = [];
    vi.useFakeTimers();
    vi.stubGlobal('WebSocket', FakeSocket as any);
    localStorage.clear();
    localStorage.setItem('johnnyone_access_token', 'tok-1');
  });
  afterEach(() => {
    vi.useRealTimers();
    vi.unstubAllGlobals();
  });

  it('pings while the socket is open, and stops when unsubscribed', () => {
    const sub = makeClient().subscribe('subscription { x }').subscribe({ error: () => undefined });
    const ws = FakeSocket.all[0];
    ws.live();
    expect(types(ws)).toEqual(['connection_init', 'subscribe']);
    vi.advanceTimersByTime(KEEPALIVE_MS);
    expect(types(ws)).toEqual(['connection_init', 'subscribe', 'ping']);
    vi.advanceTimersByTime(KEEPALIVE_MS);
    expect(types(ws).filter((t) => t === 'ping').length).toBe(2);
    sub.unsubscribe();
    const before = ws.sent.length;
    vi.advanceTimersByTime(KEEPALIVE_MS * 3);
    expect(ws.sent.length).toBe(before);
  });

  it('answers a server ping with a pong', () => {
    makeClient().subscribe('subscription { x }').subscribe({ error: () => undefined });
    const ws = FakeSocket.all[0];
    ws.live();
    ws.receive({ type: 'ping' });
    expect(types(ws)).toContain('pong');
  });

  it('a 1006 drop reconnects after a backoff, resubscribes, says so, and keeps delivering', () => {
    const next = vi.fn();
    const error = vi.fn();
    const onDropped = vi.fn();
    const onReconnected = vi.fn();
    makeClient()
      .subscribe<{ x: number }>('subscription { x }', { a: 1 }, { onDropped, onReconnected })
      .subscribe({ next, error });
    const first = FakeSocket.all[0];
    first.live();
    first.drop();
    expect(error).not.toHaveBeenCalled();
    expect(onDropped).toHaveBeenCalledTimes(1);
    expect(FakeSocket.all.length).toBe(1);
    vi.advanceTimersByTime(maxDelay(0));
    expect(FakeSocket.all.length).toBe(2);
    const second = FakeSocket.all[1];
    second.live();
    expect(onReconnected).toHaveBeenCalledTimes(1);
    expect(second.sent[1]).toMatchObject({ type: 'subscribe', payload: { variables: { a: 1 } } });
    second.receive({ id: 'op', type: 'next', payload: { data: { x: 7 } } });
    expect(next).toHaveBeenCalledWith({ x: 7 });
    // The dropped socket's keepalive is gone: only the live socket pings.
    vi.advanceTimersByTime(KEEPALIVE_MS);
    expect(types(first)).not.toContain('ping');
    expect(types(second)).toContain('ping');
  });

  it('reconnects are bounded (no storm): the error surfaces after the last attempt', () => {
    const error = vi.fn();
    makeClient().subscribe('subscription { x }').subscribe({ error });
    FakeSocket.all[0].drop(); // never even connected
    for (let i = 0; i < RECONNECT_MAX_ATTEMPTS; i++) {
      vi.advanceTimersByTime(maxDelay(i));
      expect(FakeSocket.all.length).toBe(i + 2);
      FakeSocket.all[i + 1].drop();
    }
    expect(error).toHaveBeenCalledTimes(1);
    expect(error.mock.calls[0][0]).toBeInstanceOf(SubscriptionClosedError);
    expect(error.mock.calls[0][0].code).toBe(1006);
    vi.advanceTimersByTime(60_000);
    expect(FakeSocket.all.length).toBe(1 + RECONNECT_MAX_ATTEMPTS);
  });

  it('a connection that stayed up resets the attempt budget', () => {
    const error = vi.fn();
    makeClient().subscribe('subscription { x }').subscribe({ error });
    for (let i = 0; i < RECONNECT_MAX_ATTEMPTS + 2; i++) {
      const ws = FakeSocket.all[FakeSocket.all.length - 1];
      ws.live();
      vi.advanceTimersByTime(RECONNECT_STABLE_MS);
      ws.drop();
      vi.advanceTimersByTime(maxDelay(0));
    }
    expect(error).not.toHaveBeenCalled();
  });

  it('L-1-4: the reconnect waits its jittered backoff (±20%), never sooner', () => {
    expect(reconnectDelayMs(0, 0)).toBe(800);
    expect(reconnectDelayMs(0, 1)).toBe(1200);
    expect(reconnectDelayMs(4, 0)).toBe(15000 * (1 - RECONNECT_JITTER));
    expect(reconnectDelayMs(2, 0.5)).toBe(4000);
    for (const r of [0, 0.999]) {
      const rnd = vi.spyOn(Math, 'random').mockReturnValue(r);
      FakeSocket.all = [];
      makeClient().subscribe('subscription { x }').subscribe({ error: () => undefined });
      FakeSocket.all[0].live();
      FakeSocket.all[0].drop();
      const wait = reconnectDelayMs(0, r);
      expect(wait).toBeGreaterThanOrEqual(800);
      vi.advanceTimersByTime(wait - 1);
      expect(FakeSocket.all.length, `no socket before ${wait} ms`).toBe(1);
      vi.advanceTimersByTime(1);
      expect(FakeSocket.all.length).toBe(2);
      rnd.mockRestore();
    }
  });

  it('unsubscribing during the backoff never reconnects', () => {
    const error = vi.fn();
    const sub = makeClient().subscribe('subscription { x }').subscribe({ error });
    FakeSocket.all[0].live();
    FakeSocket.all[0].drop();
    sub.unsubscribe();
    vi.advanceTimersByTime(60_000);
    expect(FakeSocket.all.length).toBe(1);
    expect(error).not.toHaveBeenCalled();
  });

  it('only drops reconnect; refusals (4400/4500, a normal 1000) do not', () => {
    expect(shouldReconnectClose(1006, 0)).toBe(true);
    expect(shouldReconnectClose(1001, 0)).toBe(true);
    expect(shouldReconnectClose(1006, RECONNECT_MAX_ATTEMPTS)).toBe(false);
    for (const code of [1000, 4400, 4401, 4403, 4500]) expect(shouldReconnectClose(code, 0)).toBe(false);
    expect([0, 1, 2, 3, 4, 10].map((a) => reconnectDelayMs(a))).toEqual([1000, 2000, 4000, 8000, 15000, 15000]);
    expect(KEEPALIVE_MS).toBeLessThan(100_000);
  });
});

describe('the plan chat reply arrives live (item 1)', () => {
  function chat(api: Partial<Record<keyof JohnnyApiService, unknown>>) {
    const full = {
      getSession: (id: string) => of({ id, title: id }),
      listMessages: vi.fn(() => of([])),
      onRelayChatMessage: () => NEVER,
      onRelayChatDelta: () => NEVER,
      sendRelayMessage: () => of({ relayId: 'r1' }),
      ...api,
    };
    const injector = Injector.create({ providers: [{ provide: JohnnyApiService, useValue: full }] });
    return { chat: runInInjectionContext(injector, () => new AiChatService()), api: full };
  }

  afterEach(() => vi.useRealTimers());

  it('deltas stream the reply and the final message settles it, with no store poll', () => {
    vi.useFakeTimers();
    const deltas = new Subject<RelayChatDeltaMsg>();
    const messages = new Subject<RelayChatMessageMsg>();
    const { chat: c, api } = chat({ onRelayChatDelta: () => deltas, onRelayChatMessage: () => messages });
    c.loadSession('S');
    (api.listMessages as any).mockClear();
    c.sendMessage('hi');
    expect(c.isStreaming()).toBe(true);
    deltas.next({ relayId: 'r1', sessionId: 'S', delta: 'Hel', chunkType: 'text', isFinal: false });
    deltas.next({ relayId: 'r1', sessionId: 'S', delta: '{"field":"why"}', chunkType: 'tooluse', isFinal: false });
    deltas.next({ relayId: 'r1', sessionId: 'S', delta: 'lo', chunkType: 'text', isFinal: false });
    expect(c.messages().at(-1)?.content).toBe('Hello');
    messages.next({ relayId: 'r1', sessionId: 'S', role: 'assistant', content: 'Hello.' });
    expect(c.isStreaming()).toBe(false);
    expect(c.messages().map((m) => [m.role, m.content])).toEqual([
      ['user', 'hi'],
      ['assistant', 'Hello.'],
    ]);
    vi.advanceTimersByTime(TURN_FALLBACK_POLL_MS * 5);
    expect(api.listMessages).not.toHaveBeenCalled();
  });

  it('a final message alone (deltas missed) still settles the reply live', () => {
    const messages = new Subject<RelayChatMessageMsg>();
    const { chat: c } = chat({ onRelayChatMessage: () => messages });
    c.loadSession('S');
    c.sendMessage('hi');
    messages.next({ relayId: 'r1', sessionId: 'S', role: 'assistant', content: 'Answer' });
    expect(c.isStreaming()).toBe(false);
    expect(c.messages().at(-1)).toMatchObject({ role: 'assistant', content: 'Answer' });
  });

  it('a dropped message socket settles the turn in flight from the store while it reconnects', () => {
    vi.useFakeTimers();
    let hooks: { onDropped?: () => void } | undefined;
    const store = [
      { id: 'u', sessionId: 'S', role: 'user', content: 'hi', createdAt: new Date().toISOString().replace('T', ' ').slice(0, 19) },
      { id: 'a', sessionId: 'S', role: 'assistant', content: 'stored answer', createdAt: new Date().toISOString().replace('T', ' ').slice(0, 19) },
    ];
    let served: unknown[] = [];
    const { chat: c } = chat({
      listMessages: vi.fn(() => of(served)),
      onRelayChatMessage: (_id: string, opts: { onDropped?: () => void }) => {
        hooks = opts;
        return new Observable(() => undefined);
      },
    });
    c.loadSession('S');
    c.sendMessage('hi');
    served = store;
    hooks?.onDropped?.();
    vi.advanceTimersByTime(TURN_FALLBACK_POLL_MS);
    expect(c.isStreaming()).toBe(false);
    expect(c.messages().at(-1)?.content).toBe('stored answer');
  });

  it('L-1-5: a final message after the store already showed the reply never duplicates it', () => {
    vi.useFakeTimers();
    let hooks: { onDropped?: () => void } | undefined;
    const now = new Date().toISOString().replace('T', ' ').slice(0, 19);
    let served: unknown[] = [];
    const messages = new Subject<RelayChatMessageMsg>();
    const { chat: c } = chat({
      listMessages: vi.fn(() => of(served)),
      onRelayChatMessage: (_id: string, opts: { onDropped?: () => void }) => {
        hooks = opts;
        return messages;
      },
    });
    c.loadSession('S');
    c.sendMessage('hi');
    served = [
      { id: 'u', sessionId: 'S', role: 'user', content: 'hi', createdAt: now },
      { id: 'a', sessionId: 'S', role: 'assistant', content: 'Answer', createdAt: now },
    ];
    hooks?.onDropped?.();
    vi.advanceTimersByTime(TURN_FALLBACK_POLL_MS); // the store poll settles the turn first
    messages.next({ relayId: 'r1', sessionId: 'S', role: 'assistant', content: 'Answer' }); // then live
    expect(c.messages().map((m) => [m.role, m.content])).toEqual([
      ['user', 'hi'],
      ['assistant', 'Answer'],
    ]);
  });

  it('L-1-5: replyAlreadyStored only counts stored assistant rows after the last user message', () => {
    const r = (id: string, role: 'user' | 'assistant') => ({ id, role });
    expect(replyAlreadyStored([r('u1', 'user'), r('a1', 'assistant')])).toBe(true);
    expect(replyAlreadyStored([r('a0', 'assistant'), r('u1', 'user')])).toBe(false);
    expect(replyAlreadyStored([r('u1', 'user'), r('relay-r1-assistant', 'assistant')])).toBe(false);
    expect(replyAlreadyStored([r('temp-1', 'user')])).toBe(false);
    expect(replyAlreadyStored([])).toBe(false);
  });

  it('tool and stderr deltas are not reply text', () => {
    for (const k of ['tooluse', 'toolresult', 'stderr', 'system', 'result']) {
      expect(classifyDelta({ chunkType: k, isFinal: false })).toBe('control');
    }
    expect(classifyDelta({ chunkType: 'text' })).toBe('text');
    expect(classifyDelta({ chunkType: 'assistant' })).toBe('text');
  });
});
