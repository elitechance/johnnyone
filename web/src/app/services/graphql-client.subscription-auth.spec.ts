/**
 * Live UI bug 2: the plan chat never updated live; every subscription answered `UNAUTHENTICATED`.
 *
 * Root cause: the worker's graphql-ws context (lokal `cloudflare/worker.ts`) reads the bearer from
 * `connectionParams.Authorization` (top level of the `connection_init` payload). The client sent it
 * nested as `payload.headers.Authorization`, so the worker never saw a token. Queries were fine
 * because they carry a real HTTP `Authorization` header.
 *
 * A fake `WebSocket` stands in for the socket (no network). The fallback (a turn settled from the
 * stored messages when live is down) runs through the REAL `AiChatService` with a fake API.
 */
import '@angular/compiler';
import { Injector, runInInjectionContext } from '@angular/core';
import { NEVER, Subject, of, throwError } from 'rxjs';
import { readFileSync } from 'node:fs';
import { resolve } from 'node:path';
import { afterEach, beforeEach, describe, expect, it, vi } from 'vitest';
import { GraphQLClient, GraphQLRequestError } from '../../../../ui/src/services/graphql-client';
import { connectionInitPayload, shouldRetrySubscriptionAuth } from '../../../../ui/src/services/graphql-ws-logic';
import { turnAnsweredInStore, storedCount, TURN_FALLBACK_POLL_MS } from '../../../../ui/src/services/ai-chat-logic';
import { AiChatService } from '../../../../ui/src/services/ai-chat.service';
import { JohnnyApiService } from '../../../../ui/src/services/johnny-api.service';
import type { AiMessage } from '../../../../ui/src/models/ai-message.model';

class FakeSocket {
  static OPEN = 1;
  static all: FakeSocket[] = [];
  readyState = 0;
  sent: any[] = [];
  closed = false;
  onopen: ((e?: unknown) => void) | null = null;
  onmessage: ((e: { data: string }) => void) | null = null;
  onerror: ((e?: unknown) => void) | null = null;
  onclose: ((e: { wasClean: boolean; code: number }) => void) | null = null;
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
  /** The server closes the socket (graphql-ws closes CLEANLY with its 44xx/4500 codes). */
  serverClose(code: number, wasClean = true) {
    this.readyState = 3;
    this.onclose?.({ wasClean, code });
  }
}

const UNAUTH = [{ message: 'UNAUTHENTICATED', extensions: { code: 'UNAUTHENTICATED' } }];

function makeClient(authRefresh: (() => Promise<void>) | null) {
  const c = Object.create(GraphQLClient.prototype) as GraphQLClient;
  (c as any).apiUrl = 'http://example.test/graphql';
  (c as any).hostApiUrl = '';
  (c as any).wsUrl = 'ws://example.test/graphql';
  (c as any).extraHeaders = {};
  (c as any).localHostHealth = null;
  (c as any).authRefresh = authRefresh;
  (c as any).inflightAuthRefresh = null;
  return c;
}

const flush = () => new Promise((r) => setTimeout(r, 0));

describe('subscription auth on the graphql-ws connect (bug 2)', () => {
  beforeEach(() => {
    FakeSocket.all = [];
    vi.stubGlobal('WebSocket', FakeSocket as any);
    localStorage.clear();
    localStorage.setItem('johnnyone_access_token', 'tok-1');
    localStorage.setItem('johnnyone_tenant_id', 't1');
  });
  afterEach(() => {
    vi.unstubAllGlobals();
  });

  it('connectionInitPayload puts Authorization at the top level (what the worker reads)', () => {
    const p = connectionInitPayload({ Authorization: 'Bearer x', 'x-tenant-id': 't1' });
    expect(p['Authorization']).toBe('Bearer x');
    expect(p['x-tenant-id']).toBe('t1');
    expect((p['headers'] as Record<string, string>)['Authorization']).toBe('Bearer x');
  });

  it('the connection_init frame carries the bearer at payload.Authorization', () => {
    const sub = makeClient(null).subscribe('subscription { x }').subscribe({ error: () => undefined });
    const ws = FakeSocket.all[0];
    ws.open();
    expect(ws.sent[0].type).toBe('connection_init');
    expect(ws.sent[0].payload.Authorization).toBe('Bearer tok-1');
    sub.unsubscribe();
    expect(ws.closed).toBe(true);
  });

  it('an UNAUTHENTICATED answer refreshes the token and reconnects ONCE with the new one', async () => {
    const refresh = vi.fn(async () => {
      localStorage.setItem('johnnyone_access_token', 'tok-2');
    });
    const next = vi.fn();
    const error = vi.fn();
    makeClient(refresh).subscribe<{ x: number }>('subscription { x }').subscribe({ next, error });
    const first = FakeSocket.all[0];
    first.open();
    first.receive({ type: 'connection_ack' });
    first.receive({ id: 'op', type: 'error', payload: UNAUTH });
    await flush();
    expect(refresh).toHaveBeenCalledTimes(1);
    expect(error).not.toHaveBeenCalled();
    expect(first.closed).toBe(true);
    const second = FakeSocket.all[1];
    expect(second).toBeDefined();
    second.open();
    expect(second.sent[0].payload.Authorization).toBe('Bearer tok-2');
    second.receive({ type: 'connection_ack' });
    second.receive({ id: 'op', type: 'next', payload: { data: { x: 1 } } });
    expect(next).toHaveBeenCalledWith({ x: 1 });

    // A second auth failure is not retried again: it surfaces.
    second.receive({ id: 'op', type: 'next', payload: { errors: UNAUTH } });
    await flush();
    expect(refresh).toHaveBeenCalledTimes(1);
    expect(error).toHaveBeenCalledTimes(1);
    expect(error.mock.calls[0][0]).toBeInstanceOf(GraphQLRequestError);
  });

  it('without a refresh hook, or for a non-auth error, the error surfaces at once', () => {
    expect(shouldRetrySubscriptionAuth(false, false, true)).toBe(false);
    expect(shouldRetrySubscriptionAuth(false, true, false)).toBe(false);
    expect(shouldRetrySubscriptionAuth(true, true, true)).toBe(false);
    expect(shouldRetrySubscriptionAuth(false, true, true)).toBe(true);
    const error = vi.fn();
    makeClient(async () => undefined).subscribe('subscription { x }').subscribe({ error });
    const ws = FakeSocket.all[0];
    ws.open();
    ws.receive({ id: 'op', type: 'error', payload: [{ message: 'Cannot query field "x"' }] });
    expect(error).toHaveBeenCalledTimes(1);
    expect(FakeSocket.all.length).toBe(1);
  });
});

function m(id: string, role: AiMessage['role'], content = id, createdAt: string | undefined = undefined): AiMessage {
  return { id, sessionId: 'A', role, content, inputTokens: 0, outputTokens: 0, costCents: 0, createdAt: createdAt as string };
}

/** A chat service over a fake API whose store, subscriptions and send are controlled by the test. */
function chatWith(opts: {
  messages?: () => any;
  deltas?: () => any;
  store: () => AiMessage[];
}) {
  const listMessages = vi.fn(() => of(opts.store()));
  const onRelayChatMessage = vi.fn(opts.messages ?? (() => NEVER));
  const api = {
    getSession: (id: string) => of({ id, title: id }),
    listMessages,
    onRelayChatMessage,
    sendRelayMessage: () => of({ relayId: 'r1' }),
    onRelayChatDelta: opts.deltas ?? (() => NEVER),
  };
  const injector = Injector.create({ providers: [{ provide: JohnnyApiService, useValue: api }] });
  const chat = runInInjectionContext(injector, () => new AiChatService());
  return { chat, listMessages, onRelayChatMessage };
}

const nowUtc = () => new Date().toISOString();

describe('subscription failure modes (U-1-4, U-1-6, U-1-16)', () => {
  beforeEach(() => {
    FakeSocket.all = [];
    vi.stubGlobal('WebSocket', FakeSocket as any);
    localStorage.clear();
    localStorage.setItem('johnnyone_access_token', 'tok-1');
  });
  afterEach(() => vi.unstubAllGlobals());

  it('U-1-16: an auth failure in a `next` frame (payload.errors) also refreshes and retries', async () => {
    const refresh = vi.fn(async () => localStorage.setItem('johnnyone_access_token', 'tok-2'));
    const error = vi.fn();
    makeClient(refresh).subscribe('subscription { x }').subscribe({ error });
    const first = FakeSocket.all[0];
    first.open();
    first.receive({ type: 'connection_ack' });
    first.receive({ id: 'op', type: 'next', payload: { errors: UNAUTH } });
    await flush();
    expect(refresh).toHaveBeenCalledTimes(1);
    expect(error).not.toHaveBeenCalled();
    FakeSocket.all[1].open();
    expect(FakeSocket.all[1].sent[0].payload.Authorization).toBe('Bearer tok-2');
  });

  it('U-1-4: a CLEAN close before `complete` (4500) is an error, not a silent hang', () => {
    const error = vi.fn();
    const complete = vi.fn();
    makeClient(async () => undefined).subscribe('subscription { x }').subscribe({ error, complete });
    const ws = FakeSocket.all[0];
    ws.open();
    ws.serverClose(4500);
    expect(error).toHaveBeenCalledTimes(1);
    expect(String(error.mock.calls[0][0].message)).toContain('4500');
    expect(FakeSocket.all.length).toBe(1); // not an auth close: no retry
    expect(complete).not.toHaveBeenCalled();
  });

  it('U-1-4: a 4401/4403 close gets the one refresh-and-reconnect', async () => {
    for (const code of [4401, 4403]) {
      FakeSocket.all = [];
      const refresh = vi.fn(async () => undefined);
      const error = vi.fn();
      makeClient(refresh).subscribe('subscription { x }').subscribe({ error });
      FakeSocket.all[0].open();
      FakeSocket.all[0].serverClose(code);
      await flush();
      expect(refresh, String(code)).toHaveBeenCalledTimes(1);
      expect(error).not.toHaveBeenCalled();
      expect(FakeSocket.all.length).toBe(2);
    }
  });

  it('a close AFTER `complete` is the normal end', () => {
    const error = vi.fn();
    const complete = vi.fn();
    makeClient(null).subscribe('subscription { x }').subscribe({ error, complete });
    const ws = FakeSocket.all[0];
    ws.open();
    ws.receive({ id: 'op', type: 'complete' });
    ws.serverClose(1000);
    expect(complete).toHaveBeenCalledTimes(1);
    expect(error).not.toHaveBeenCalled();
  });

  it('U-1-6: unsubscribing while the refresh is in flight never reconnects or errors', async () => {
    let release!: () => void;
    const refresh = vi.fn(() => new Promise<void>((r) => (release = r)));
    const error = vi.fn();
    const sub = makeClient(refresh).subscribe('subscription { x }').subscribe({ error });
    FakeSocket.all[0].open();
    FakeSocket.all[0].receive({ id: 'op', type: 'error', payload: UNAUTH });
    await flush();
    expect(refresh).toHaveBeenCalledTimes(1);
    sub.unsubscribe();
    release();
    await flush();
    expect(FakeSocket.all.length).toBe(1);
    expect(error).not.toHaveBeenCalled();
  });

  it('U-1-6: a rejected refresh surfaces the ORIGINAL auth error', async () => {
    const refresh = vi.fn(async () => {
      throw new Error('refresh token dead');
    });
    const error = vi.fn();
    makeClient(refresh).subscribe('subscription { x }').subscribe({ error });
    FakeSocket.all[0].open();
    FakeSocket.all[0].receive({ id: 'op', type: 'error', payload: UNAUTH });
    await flush();
    expect(error).toHaveBeenCalledTimes(1);
    expect(error.mock.calls[0][0]).toBeInstanceOf(GraphQLRequestError);
    expect(error.mock.calls[0][0].message).toBe('UNAUTHENTICATED');
    expect(FakeSocket.all.length).toBe(1);
  });
});

describe('turn fallback when live updates are down (bug 2)', () => {
  it('turnAnsweredInStore: only this turn\'s own user row, then an assistant row after it, settles it', () => {
    const t0 = Date.UTC(2026, 10, 24, 13, 14, 0);
    const turn = { content: 'hello', sentAtMs: t0, baseline: 2 };
    const before = [m('1', 'user', 'earlier'), m('2', 'assistant')];
    expect(storedCount([...before, m('temp-9', 'user'), m('relay-r-assistant', 'assistant')])).toBe(2);
    expect(turnAnsweredInStore(before, turn)).toBe(false);
    expect(turnAnsweredInStore([...before, m('3', 'user', 'hello', '2026-11-24 13:14:01')], turn)).toBe(false);
    expect(turnAnsweredInStore([...before, m('3', 'user', 'hello', '2026-11-24 13:14:01'), m('4', 'assistant')], turn)).toBe(true);
  });

  it('U-1-2: an earlier reply that was still a local row never settles the new turn early', () => {
    // At send the chat showed: stored u1, a LOCAL relay- reply (already stored server-side), the new temp bubble.
    const local = [m('u1', 'user', 'first'), m('relay-r0-assistant', 'assistant'), m('temp-1', 'user', 'second')];
    const turn = { content: 'second', sentAtMs: Date.UTC(2026, 10, 24, 13, 14, 0), baseline: storedCount(local) };
    expect(turn.baseline).toBe(1);
    // The store now holds the earlier turn's reply: row count passed the baseline and the last row is
    // an assistant's. Counting alone settled here; the new turn's own user row is not there yet.
    const stored = [m('u1', 'user', 'first'), m('a0', 'assistant', 'reply one')];
    expect(turnAnsweredInStore(stored, turn)).toBe(false);
    // The same text sent long before the turn does not count either.
    const old = [m('u0', 'user', 'second', '2026-11-24 12:00:00'), m('a', 'assistant')];
    expect(turnAnsweredInStore(old, { ...turn, baseline: 0 })).toBe(false);
    expect(turnAnsweredInStore([...stored, m('u2', 'user', 'second', '2026-11-24 13:14:02'), m('a2', 'assistant')], turn)).toBe(true);
  });

  it('a message-subscription error no longer leaves the reply "working" until reload', async () => {
    vi.useFakeTimers();
    const err = vi.spyOn(console, 'error').mockImplementation(() => undefined);
    try {
      let stored: AiMessage[] = [];
      const { chat } = chatWith({ messages: () => throwError(() => new GraphQLRequestError(UNAUTH)), store: () => stored });
      chat.loadSession('A');
      chat.sendMessage('hello');
      expect(chat.isStreaming()).toBe(true);
      stored = [m('u1', 'user', 'hello', nowUtc())];
      await vi.advanceTimersByTimeAsync(TURN_FALLBACK_POLL_MS);
      expect(chat.isStreaming()).toBe(true);
      stored = [m('u1', 'user', 'hello', nowUtc()), m('a1', 'assistant')];
      await vi.advanceTimersByTimeAsync(TURN_FALLBACK_POLL_MS);
      expect(chat.isStreaming()).toBe(false);
      expect(chat.messages().map((x) => x.id)).toEqual(['u1', 'a1']);
    } finally {
      err.mockRestore();
      vi.useRealTimers();
    }
  });

  it('U-1-15: a subscription that errors AFTER the send starts the fallback', async () => {
    vi.useFakeTimers();
    const err = vi.spyOn(console, 'error').mockImplementation(() => undefined);
    try {
      let stored: AiMessage[] = [];
      const deltas = new Subject<any>();
      const { chat, listMessages } = chatWith({ deltas: () => deltas, store: () => stored });
      chat.loadSession('A');
      chat.sendMessage('hello');
      await vi.advanceTimersByTimeAsync(TURN_FALLBACK_POLL_MS * 2);
      expect(listMessages).toHaveBeenCalledTimes(1); // live is fine: no polling yet
      deltas.error(new Error('WebSocket connection error'));
      stored = [m('u1', 'user', 'hello', nowUtc()), m('a1', 'assistant')];
      await vi.advanceTimersByTimeAsync(TURN_FALLBACK_POLL_MS);
      expect(listMessages).toHaveBeenCalledTimes(2);
      expect(chat.isStreaming()).toBe(false);
    } finally {
      err.mockRestore();
      vi.useRealTimers();
    }
  });

  it('with live working, nothing polls', async () => {
    vi.useFakeTimers();
    try {
      const { chat, listMessages } = chatWith({ deltas: () => new Subject(), store: () => [] });
      chat.loadSession('A');
      chat.sendMessage('hello');
      await vi.advanceTimersByTimeAsync(TURN_FALLBACK_POLL_MS * 3);
      expect(listMessages).toHaveBeenCalledTimes(1); // the load only
    } finally {
      vi.useRealTimers();
    }
  });

  it('U-2-21: coming back while a turn is in flight settles it from the store (no stuck spinner)', async () => {
    vi.useFakeTimers();
    try {
      let stored: AiMessage[] = [];
      const { chat, listMessages } = chatWith({ deltas: () => new Subject(), store: () => stored });
      chat.loadSession('A');
      chat.sendMessage('hello');
      chat.teardownSubscriptions(); // the page was left mid-turn: the delta feed is gone
      chat.resumeSession('A');
      expect(chat.isStreaming()).toBe(true);
      stored = [m('u1', 'user', 'hello', nowUtc()), m('a1', 'assistant')];
      const before = listMessages.mock.calls.length;
      await vi.advanceTimersByTimeAsync(TURN_FALLBACK_POLL_MS);
      expect(listMessages.mock.calls.length).toBe(before + 1);
      expect(chat.isStreaming()).toBe(false);
    } finally {
      vi.useRealTimers();
    }
  });

  it('turnAnsweredInStore: a recent user row with DIFFERENT text is not this turn', () => {
    const turn = { content: 'hello', sentAtMs: Date.UTC(2026, 10, 24, 13, 14, 0), baseline: 0 };
    expect(turnAnsweredInStore([m('u', 'user', 'something else', '2026-11-24 13:14:05'), m('a', 'assistant')], turn)).toBe(false);
  });

  it('U-1-3: coming back to the same session re-subscribes and re-reads the store', () => {
    const { chat, listMessages, onRelayChatMessage } = chatWith({ store: () => [] });
    chat.loadSession('A');
    expect(onRelayChatMessage).toHaveBeenCalledTimes(1);
    chat.teardownSubscriptions(); // the page was left
    chat.resumeSession('A');
    expect(onRelayChatMessage).toHaveBeenCalledTimes(2);
    expect(listMessages).toHaveBeenCalledTimes(2);
    chat.resumeSession('B'); // another session is a full load
    expect(chat.currentSession()?.id).toBe('B');
    const page = readFileSync(resolve(__dirname, '../pages/plans/plan-detail.page.ts'), 'utf8');
    expect(page).toMatch(/^\s*if \(sid\) this\.chat\.resumeSession\(sid\);$/m);
  });
});
