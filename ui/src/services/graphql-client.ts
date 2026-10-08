import { Injectable, InjectionToken, inject } from '@angular/core';
import { Observable, from } from 'rxjs';
import { switchMap } from 'rxjs/operators';

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

  subscribe<T>(subscription: string, variables?: Record<string, unknown>): Observable<T> {
    return new Observable<T>((subscriber) => {
      let ws: WebSocket | null = null;
      const operationId = crypto.randomUUID();

      const connect = () => {
        ws = new WebSocket(this.wsUrl, 'graphql-transport-ws');

        ws.onopen = () => {
          const headers = this.buildHeaders(false);
          ws!.send(
            JSON.stringify({
              type: 'connection_init',
              payload: {
                headers,
              },
            })
          );
        };

        ws.onmessage = (event) => {
          const message = JSON.parse(event.data);

          switch (message.type) {
            case 'connection_ack':
              ws!.send(
                JSON.stringify({
                  id: operationId,
                  type: 'subscribe',
                  payload: { query: subscription, variables },
                })
              );
              break;

            case 'next':
              if (message.payload?.errors?.length) {
                subscriber.error(new GraphQLRequestError(message.payload.errors));
              } else if (message.payload?.data) {
                subscriber.next(message.payload.data as T);
              }
              break;

            case 'error':
              subscriber.error(
                new GraphQLRequestError(
                  Array.isArray(message.payload) ? message.payload : [{ message: 'Subscription error' }]
                )
              );
              break;

            case 'complete':
              subscriber.complete();
              break;
          }
        };

        ws.onerror = () => {
          subscriber.error(new Error('WebSocket connection error'));
        };

        ws.onclose = (event) => {
          if (!event.wasClean) {
            subscriber.error(new Error(`WebSocket closed unexpectedly: ${event.code}`));
          }
        };
      };

      connect();

      return () => {
        if (ws) {
          if (ws.readyState === WebSocket.OPEN) {
            ws.send(JSON.stringify({ id: operationId, type: 'complete' }));
          }
          ws.close();
          ws = null;
        }
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
      throw new GraphQLRequestError(result.errors, result.data != null);
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
    /** True when the response carried data alongside the errors. */
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
 * A *partial* response (data alongside the error) is never retried: part of the
 * operation may already have been applied, so replaying it could double a write.
 */
function isExpiredTokenError(error: unknown): boolean {
  if (error instanceof HttpStatusError) return error.status === 401;
  if (!(error instanceof GraphQLRequestError)) return false;
  if (error.hasPartialData) return false;
  return error.errors.some(isExpiredTokenGraphQLError);
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
