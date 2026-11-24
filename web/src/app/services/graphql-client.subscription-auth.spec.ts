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

function m(id: string, role: AiMessage['role']): AiMessage {
  return { id, sessionId: 'A', role, content: id, inputTokens: 0, outputTokens: 0, costCents: 0, createdAt: '2026-11-24 13:14:38' };
}

describe('turn fallback when live updates are down (bug 2)', () => {
  it('turnAnsweredInStore: the reply row after the baseline settles the turn', () => {
    const before = [m('1', 'user'), m('2', 'assistant')];
    expect(storedCount([...before, m('temp-9', 'user'), m('relay-r-assistant', 'assistant')])).toBe(2);
    expect(turnAnsweredInStore(before, 2)).toBe(false);
    expect(turnAnsweredInStore([...before, m('3', 'user')], 2)).toBe(false);
    expect(turnAnsweredInStore([...before, m('3', 'user'), m('4', 'assistant')], 2)).toBe(true);
  });

  it('a message-subscription error no longer leaves the reply "working" until reload', async () => {
    vi.useFakeTimers();
    try {
      let stored: AiMessage[] = [];
      const api = {
        getSession: (id: string) => of({ id, title: id }),
        listMessages: () => of(stored),
        onRelayChatMessage: () => throwError(() => new GraphQLRequestError(UNAUTH)),
        sendRelayMessage: () => of({ relayId: 'r1' }),
        onRelayChatDelta: () => NEVER,
      };
      const injector = Injector.create({ providers: [{ provide: JohnnyApiService, useValue: api }] });
      const chat = runInInjectionContext(injector, () => new AiChatService());
      const err = vi.spyOn(console, 'error').mockImplementation(() => undefined);
      chat.loadSession('A');
      chat.sendMessage('hello');
      expect(chat.isStreaming()).toBe(true);
      // The engine stored the user row, then the reply.
      stored = [m('u1', 'user')];
      await vi.advanceTimersByTimeAsync(TURN_FALLBACK_POLL_MS);
      expect(chat.isStreaming()).toBe(true);
      stored = [m('u1', 'user'), m('a1', 'assistant')];
      await vi.advanceTimersByTimeAsync(TURN_FALLBACK_POLL_MS);
      expect(chat.isStreaming()).toBe(false);
      expect(chat.messages().map((x) => x.id)).toEqual(['u1', 'a1']);
      err.mockRestore();
    } finally {
      vi.useRealTimers();
    }
  });

  it('with live working, nothing polls', async () => {
    vi.useFakeTimers();
    try {
      const listMessages = vi.fn(() => of([] as AiMessage[]));
      const api = {
        getSession: (id: string) => of({ id, title: id }),
        listMessages,
        onRelayChatMessage: () => NEVER,
        sendRelayMessage: () => of({ relayId: 'r1' }),
        onRelayChatDelta: () => new Subject(),
      };
      const injector = Injector.create({ providers: [{ provide: JohnnyApiService, useValue: api }] });
      const chat = runInInjectionContext(injector, () => new AiChatService());
      chat.loadSession('A');
      chat.sendMessage('hello');
      await vi.advanceTimersByTimeAsync(TURN_FALLBACK_POLL_MS * 3);
      expect(listMessages).toHaveBeenCalledTimes(1); // the load only
    } finally {
      vi.useRealTimers();
    }
  });
});
