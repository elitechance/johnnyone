import { Injectable, InjectionToken, inject } from '@angular/core';
import { Observable, from, throwError } from 'rxjs';
import { map, switchMap } from 'rxjs/operators';

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
  private localHostHealth: Promise<boolean> | null = null;

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
    const body = JSON.stringify({ query, variables });

    return from(
      fetch(apiUrl, {
        method: 'POST',
        headers: this.buildHeaders(true, includeAuthHeaders),
        body,
        credentials: 'same-origin',
      })
    ).pipe(
      switchMap((response) => {
        if (!response.ok) {
          return throwError(() => new Error(`GraphQL request failed: ${response.status} ${response.statusText}`));
        }
        return from(response.json() as Promise<GraphQLResponse<T>>);
      }),
      map((result) => {
        if (result.errors?.length) {
          throw new GraphQLRequestError(result.errors);
        }
        return result.data;
      })
    );
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
  constructor(public readonly errors: GraphQLError[]) {
    super(errors.map((e) => e.message).join('; '));
    this.name = 'GraphQLRequestError';
  }
}
