import { Injectable, InjectionToken, inject } from '@angular/core';
import { Observable, from } from 'rxjs';
import { switchMap } from 'rxjs/operators';
import {
  KEEPALIVE_MS,
  RECONNECT_STABLE_MS,
  SubscriptionClosedError,
  closeIsAuthFailure,
  connectionInitPayload,
  reconnectDelayMs,
  shouldReconnectClose,
  shouldRetrySubscriptionAuth,
} from './graphql-ws-logic';

/** Per-subscription options. */
export interface SubscribeOptions {
  /**
   * Called when a dropped socket (1006, …) reconnected and the subscription is live again. Events
   * published while it was down are lost: a caller that cares re-reads its state here.
   */
  onReconnected?: () => void;
  /** Called when the socket dropped and a reconnect is scheduled (the stream is not over yet). */
  onDropped?: () => void;
}

export const GRAPHQL_API_URL = new InjectionToken<string>('GRAPHQL_API_URL', {
  providedIn: 'root',
  factory: () => '/graphql',
});

export const GRAPHQL_EXTRA_HEADERS = new InjectionToken<Record<string, string>>('GRAPHQL_EXTRA_HEADERS', {
  providedIn: 'root',
  factory: () => ({}),
});

export const GRAPHQL_WS_URL = new InjectionToken<string>('GRAPHQL_WS_URL', {
  providedIn: 'root',
  factory: () => {
    const protocol = typeof window !== 'undefined' && window.location.protocol === 'https:' ? 'wss:' : 'ws:';
    const host = typeof window !== 'undefined' ? window.location.host : 'localhost';
    return `${protocol}//${host}/graphql`;
  },
});

/** Optional direct desktop host GraphQL URL used for local read-only queries. */
export const HOST_GRAPHQL_API_URL = new InjectionToken<string>('HOST_GRAPHQL_API_URL', {
  providedIn: 'root',
  factory: () => '',
});

/** Resolves once the stored access token has been renewed (or rejects). */
export type GraphQLAuthRefresh = () => Promise<void>;

/**
 * Optional hook that lets the host app renew an expired access token so the
 * client can replay a request that was rejected at the auth choke point.
 *
 * `ui/` is the shared library and must not depend on the Angular
 * `AuthService`, so the behaviour is injected as a callback and is **off**
 * unless the app provides one. `web/src/app/app.config.ts` wires it to
 * `AuthService.ensureFreshToken`.
 */
export const GRAPHQL_AUTH_REFRESH = new InjectionToken<GraphQLAuthRefresh | null>(
  'GRAPHQL_AUTH_REFRESH',
  { providedIn: 'root', factory: () => null },
);

export interface GraphQLResponse<T> {
  data: T;
  errors?: GraphQLError[];
}

export interface GraphQLError {
  message: string;
  locations?: { line: number; column: number }[];
  path?: (string | number)[];
  extensions?: Record<string, unknown>;
}

@Injectable({ providedIn: 'root' })
export class GraphQLClient {
  private readonly apiUrl = inject(GRAPHQL_API_URL);
  private readonly hostApiUrl = inject(HOST_GRAPHQL_API_URL);
  private readonly wsUrl = inject(GRAPHQL_WS_URL);
  private readonly extraHeaders = inject(GRAPHQL_EXTRA_HEADERS);
  private readonly authRefresh = inject(GRAPHQL_AUTH_REFRESH);
  private localHostHealth: Promise<boolean> | null = null;
  /** Shared so a burst of auth failures triggers one refresh, not a storm. */
  private inflightAuthRefresh: Promise<void> | null = null;

  query<T>(query: string, variables?: Record<string, unknown>): Observable<T> {
    return this.request<T>(query, variables);
  }

  queryPreferLocalHost<T>(
    workerQuery: string,
    localHostQuery: string,
    variables?: Record<string, unknown>,
  ): Observable<T> {
    return from(this.shouldUseLocalHost()).pipe(
      switchMap((useLocalHost) => this.requestAt<T>(
        useLocalHost ? this.hostApiUrl : this.apiUrl,
        useLocalHost ? localHostQuery : workerQuery,
        variables,
        !useLocalHost,
      )),
    );
  }

  mutate<T>(mutation: string, variables?: Record<string, unknown>): Observable<T> {
    return this.request<T>(mutation, variables);
  }

  subscribe<T>(subscription: string, variables?: Record<string, unknown>, opts: SubscribeOptions = {}): Observable<T> {
    return new Observable<T>((subscriber) => {
      let ws: WebSocket | null = null;
      let closed = false;
      /** The server sent `complete`: a close after that is the normal end, not an error. */
      let completed = false;
      /** One refresh-and-reconnect per subscription after an auth failure (bug 2), never a loop. */
      let authRetried = false;
      /** Reconnects in a row after dropped sockets (bounded; reset once a connection is stable). */
      let dropAttempts = 0;
      /** This connection is a reconnect after a drop (tell the caller once it is acked). */
      let reconnecting = false;
      let ackedAt = 0;
      let keepalive: ReturnType<typeof setInterval> | null = null;
      let reconnectTimer: ReturnType<typeof setTimeout> | null = null;
      const operationId = crypto.randomUUID();

      const stopKeepalive = () => {
        if (keepalive) clearInterval(keepalive);
        keepalive = null;
      };

      /** Close a socket without its handlers firing (a replaced or finished connection). */
      const closeQuietly = (socket: WebSocket | null) => {
        stopKeepalive();
        if (!socket) return;
        socket.onopen = null;
        socket.onmessage = null;
        socket.onerror = null;
        socket.onclose = null;
        if (socket.readyState === WebSocket.OPEN) {
          socket.send(JSON.stringify({ id: operationId, type: 'complete' }));
        }
        socket.close();
      };

      /** An auth failure refreshes the token and reconnects once; anything else ends the stream. */
      const fail = (error: Error) => {
        if (closed) return;
        stopKeepalive();
        const refresh = this.authRefresh;
        if (refresh && shouldRetrySubscriptionAuth(authRetried, true, isSubscriptionAuthFailure(error))) {
          authRetried = true;
          closeQuietly(ws);
          ws = null;
          this.refreshAuthOnce(refresh).then(
            () => {
              if (!closed) connect();
            },
            () => {
              if (!closed) subscriber.error(error);
            },
          );
          return;
        }
        subscriber.error(error);
      };

      /** A dropped socket (1006, …): reconnect with backoff, a bounded number of times in a row. */
      const dropped = (code: number, reason: string) => {
        if (closed) return;
        stopKeepalive();
        if (ackedAt && Date.now() - ackedAt >= RECONNECT_STABLE_MS) dropAttempts = 0;
        if (!shouldReconnectClose(code, dropAttempts)) {
          fail(new SubscriptionClosedError(code, reason));
          return;
        }
        const delay = reconnectDelayMs(dropAttempts, Math.random());
        dropAttempts += 1;
        reconnecting = true;
        ws = null;
        opts.onDropped?.();
        reconnectTimer = setTimeout(() => {
          reconnectTimer = null;
          if (!closed) connect();
        }, delay);
      };

      const connect = () => {
        const socket = new WebSocket(this.wsUrl, 'graphql-transport-ws');
        ws = socket;
        ackedAt = 0;

        socket.onopen = () => {
          // The token is read at CONNECT time (so a reconnect after a refresh sends the new one) and
          // sent at the payload's top level, which is where the worker reads it (bug 2).
          socket.send(
            JSON.stringify({
              type: 'connection_init',
              payload: connectionInitPayload(this.buildHeaders(false)),
            })
          );
        };

        socket.onmessage = (event) => {
          const message = JSON.parse(event.data);

          switch (message.type) {
            case 'connection_ack':
              ackedAt = Date.now();
              socket.send(
                JSON.stringify({
                  id: operationId,
                  type: 'subscribe',
                  payload: { query: subscription, variables },
                })
              );
              // Keepalive: an idle socket is dropped upstream (the live 1006).
              stopKeepalive();
              keepalive = setInterval(() => {
                if (socket.readyState === WebSocket.OPEN) socket.send(JSON.stringify({ type: 'ping' }));
              }, KEEPALIVE_MS);
              if (reconnecting) {
                reconnecting = false;
                opts.onReconnected?.();
              }
              break;

            case 'ping':
              socket.send(JSON.stringify({ type: 'pong' }));
              break;

            case 'next':
              if (message.payload?.errors?.length) {
                fail(new GraphQLRequestError(message.payload.errors));
              } else if (message.payload?.data) {
                subscriber.next(message.payload.data as T);
              }
              break;

            case 'error':
              fail(
                new GraphQLRequestError(
                  Array.isArray(message.payload) ? message.payload : [{ message: 'Subscription error' }]
                )
              );
              break;

            case 'complete':
              completed = true;
              stopKeepalive();
              subscriber.complete();
              break;
          }
        };

        // A failed connection always closes next (WHATWG): the close decides drop vs refusal.
        socket.onerror = () => undefined;

        // ANY close before `complete` ends the stream as an error (U-1-4): graphql-ws closes CLEANLY
        // with 4400/4401/4403/4500, and treating a clean close as "fine" left the subscription hanging
        // with no error, so the chat's fallback never started. 4401/4403 get the one auth refresh; a
        // DROPPED socket (1006, …) reconnects with backoff first (item 1).
        socket.onclose = (event) => {
          if (completed) return;
          const reason = (event as CloseEvent).reason ?? '';
          if (shouldReconnectClose(event.code, 0)) {
            dropped(event.code, reason);
            return;
          }
          fail(new SubscriptionClosedError(event.code, reason));
        };
      };

      connect();

      return () => {
        closed = true;
        if (reconnectTimer) clearTimeout(reconnectTimer);
        reconnectTimer = null;
        closeQuietly(ws);
        ws = null;
      };
    });
  }

  private request<T>(query: string, variables?: Record<string, unknown>): Observable<T> {
    return this.requestAt<T>(this.apiUrl, query, variables, true);
  }

  private requestAt<T>(
    apiUrl: string,
    query: string,
    variables: Record<string, unknown> | undefined,
    includeAuthHeaders: boolean,
  ): Observable<T> {
    // Eager, as before: the fetch starts when `requestAt` is called, not on
    // subscribe (`from(fetch(...))` had the same semantics).
    return from(this.sendWithAuthRetry<T>(apiUrl, query, variables, includeAuthHeaders));
  }

  /**
   * One send, and — only for an expired/invalid access token — one refresh and
   * one replay (fix/web-session-resume).
   *
   * `buildHeaders` attaches whatever bearer is in localStorage with no
   * freshness check, so a tab that woke with a dead 15-minute token sends it
   * and is rejected even though the 7-day refresh token beside it is valid.
   *
   * The replay calls `send` directly, never itself, so there is exactly one
   * retry and no recursion. Off entirely unless the app provided a refresh
   * hook.
   */
  private async sendWithAuthRetry<T>(
    apiUrl: string,
    query: string,
    variables: Record<string, unknown> | undefined,
    includeAuthHeaders: boolean,
  ): Promise<T> {
    try {
      return await this.send<T>(apiUrl, query, variables, includeAuthHeaders);
    } catch (error) {
      const refresh = this.authRefresh;
      if (!refresh || !includeAuthHeaders || !isExpiredTokenError(error)) {
        throw error;
      }
      try {
        await this.refreshAuthOnce(refresh);
      } catch {
        // A dead refresh token is the app's problem to report (it logs out);
        // surface the original auth error rather than the refresh failure.
        throw error;
      }
      return await this.send<T>(apiUrl, query, variables, includeAuthHeaders);
    }
  }

  /** Collapse concurrent refreshes onto one in-flight promise. */
  private refreshAuthOnce(refresh: GraphQLAuthRefresh): Promise<void> {
    if (!this.inflightAuthRefresh) {
      this.inflightAuthRefresh = Promise.resolve()
        .then(() => refresh())
        .finally(() => {
          this.inflightAuthRefresh = null;
        });
    }
    return this.inflightAuthRefresh;
  }

  private async send<T>(
    apiUrl: string,
    query: string,
    variables: Record<string, unknown> | undefined,
    includeAuthHeaders: boolean,
  ): Promise<T> {
    const response = await fetch(apiUrl, {
      method: 'POST',
      headers: this.buildHeaders(true, includeAuthHeaders),
      body: JSON.stringify({ query, variables }),
      credentials: 'same-origin',
    });
    if (!response.ok) {
      throw new HttpStatusError(response.status, response.statusText);
    }
    const result = (await response.json()) as GraphQLResponse<T>;
    if (result.errors?.length) {
      // NOTE (QA F2) — a known, accepted trade. This throw used to live in an
      // rxjs `map`, so it only ran on subscribe. It now sits inside the eager
      // promise behind `requestAt`, so a `query()`/`mutate()` whose observable
      // is never subscribed raises an *unhandled rejection* instead of being
      // silently dropped. Do NOT "fix" this by restructuring the eager/lazy
      // split: `requestAt` deliberately preserves the request timing of the
      // original `from(fetch(...))`, where the fetch also started on call
      // rather than on subscribe, and chasing the rejection would change when
      // every request in the app fires.
      //
      // Latent today, and measured rather than assumed: every `query()` /
      // `mutate()` call in `web/` and `ui/` goes through
      // `johnny-api.service.ts`, which returns the observable, and all of its
      // consumers subscribe — 76 via `firstValueFrom(...)` and 24 via
      // `.subscribe(...)`, with no site dropping the result. Because the fetch
      // is eager, a new fire-and-forget call site would both leak a request
      // and raise this rejection: give it a subscriber or a `.catch`. In a
      // browser an unhandled rejection is a console error, not a crash.
      throw new GraphQLRequestError(result.errors, hasAppliedData(result.data));
    }
    return result.data;
  }

  private async shouldUseLocalHost(): Promise<boolean> {
    if (!this.hostApiUrl || typeof window === 'undefined') {
      return false;
    }

    if (!['localhost', '127.0.0.1'].includes(window.location.hostname)) {
      return false;
    }

    if (!this.localHostHealth) {
      this.localHostHealth = fetch(this.hostApiUrl, {
        method: 'POST',
        headers: {
          'Content-Type': 'application/json',
          Accept: 'application/json',
        },
        body: JSON.stringify({ query: '{ health }' }),
      })
        .then((response) => response.ok)
        .catch(() => false);
    }

    return this.localHostHealth;
  }

  private buildHeaders(includeContentHeaders: boolean, includeAuthHeaders = true): Record<string, string> {
    const headers: Record<string, string> = {
      ...this.extraHeaders,
    };

    if (includeContentHeaders) {
      headers['Content-Type'] = 'application/json';
      headers['Accept'] = 'application/json';
    }

    if (!includeAuthHeaders) {
      return headers;
    }

    const token = this.getStoredValue('johnnyone_access_token');
    if (token) {
      headers['Authorization'] = `Bearer ${token}`;
    }

    const tenantId = this.getStoredValue('johnnyone_tenant_id');
    if (tenantId) {
      headers['x-tenant-id'] = tenantId;
    }

    const userId = this.getStoredValue('johnnyone_user_id');
    if (userId) {
      headers['x-user-id'] = userId;
    }

    return headers;
  }

  private getStoredValue(key: string): string | null {
    if (typeof window === 'undefined') return null;

    const value = window.localStorage.getItem(key)?.trim();
    return value || null;
  }
}

export class GraphQLRequestError extends Error {
  constructor(
    public readonly errors: GraphQLError[],
    /**
     * True when the response carried data that reflects *applied* work — i.e.
     * at least one root field came back non-null. An all-null `data` object is
     * false: see `hasAppliedData`.
     */
    public readonly hasPartialData = false,
  ) {
    super(errors.map((e) => e.message).join('; '));
    this.name = 'GraphQLRequestError';
  }
}

/**
 * Non-2xx transport failure. The message is byte-for-byte what this client
 * threw before, so existing callers and specs matching on it are unaffected;
 * `status` is added so the auth check does not have to parse the text.
 */
export class HttpStatusError extends Error {
  constructor(
    public readonly status: number,
    public readonly statusText: string,
  ) {
    super(`GraphQL request failed: ${status} ${statusText}`);
    this.name = 'HttpStatusError';
  }
}

/**
 * Does this failure mean "the access token you sent is expired or invalid"?
 *
 * The JohnnyOne worker's single auth choke point answers a dead bearer with an
 * HTTP **200** whose body is
 *   `errors: [{ message: 'UNAUTHENTICATED', extensions: { code: 'UNAUTHENTICATED' } }]`
 * (`worker/lib/auth/require-identity.ts:19`), so this cannot key on HTTP 401
 * alone — though a 401 is accepted too, for hosts that do answer that way.
 *
 * lokal's `verifyJwt` throws the literal `'Token expired'`
 * (`modules/auth/auth-middleware.ts:155`), but `buildAuthContext` swallows it
 * (`:64`) and `requireIdentity` swallows it again (`:95`), so that string never
 * reaches the client. It is matched anyway, cheaply, in case a host surfaces it.
 *
 * A response carrying *applied* data is never retried: part of the operation
 * may already have been performed, so replaying it could double a write.
 */
/** An auth failure on a subscription: an UNAUTHENTICATED payload, or a 4401/4403 close (U-1-4). */
function isSubscriptionAuthFailure(error: unknown): boolean {
  if (error instanceof SubscriptionClosedError) return closeIsAuthFailure(error.code);
  return isExpiredTokenError(error);
}

function isExpiredTokenError(error: unknown): boolean {
  if (error instanceof HttpStatusError) return error.status === 401;
  if (!(error instanceof GraphQLRequestError)) return false;
  if (error.hasPartialData) return false;
  return error.errors.some(isExpiredTokenGraphQLError);
}

/**
 * Did this response carry work that was actually applied? (QA F1.)
 *
 * The three nullable root fields in the schema — `getAiSession`,
 * `getPlanCheck`, `getTaskRun` (`worker/schema/johnnyone-ai.graphql:544`,
 * `:551`, `:552`) — answer an `UNAUTHENTICATED` by nulling just that field, so
 * the body is `{data: {getAiSession: null}, errors: [...]}`. `data` is then a
 * non-null *object* that nonetheless reflects nothing applied, and `getAiSession`
 * is on the phone's hot path. Reading `data != null` as "partial" blocked the
 * retry exactly where it was most needed.
 *
 * So: `null`/absent `data`, or an object whose every own enumerable value is
 * `null`, carries nothing and may be retried. One non-null value and it is
 * applied work — including a falsy one like `0`, `''` or `false` — so the
 * double-write protection is unchanged for every mixed response. A non-object
 * `data` (scalar, array) is treated as applied, the conservative direction.
 */
function hasAppliedData(data: unknown): boolean {
  if (data == null) return false;
  if (typeof data !== 'object' || Array.isArray(data)) return true;
  return Object.values(data as Record<string, unknown>).some((v) => v !== null);
}

function isExpiredTokenGraphQLError(error: GraphQLError): boolean {
  const code = String(error.extensions?.['code'] ?? '');
  if (/^(UNAUTHENTICATED|UNAUTHORIZED)$/i.test(code)) return true;

  const message = String(error.message ?? '');
  // An authorization (role/scope) refusal is not a stale token; refreshing
  // would not change the answer. e.g. 'Unauthorized: ADMIN role required'.
  if (/\b(role|scope|permission)s?\b/i.test(message)) return false;
  return (
    /\bUNAUTHENTICATED\b/i.test(message) ||
    /\bnot authenticated\b/i.test(message) ||
    /\bunauthorized\b/i.test(message) ||
    /\btoken expired\b/i.test(message) ||
    /\bjwt expired\b/i.test(message) ||
    /\binvalid token\b/i.test(message)
  );
}
