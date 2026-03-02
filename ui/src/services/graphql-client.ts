import { Injectable, InjectionToken, inject } from '@angular/core';
import { Observable, from, throwError } from 'rxjs';
import { map, switchMap } from 'rxjs/operators';

export const GRAPHQL_API_URL = new InjectionToken<string>('GRAPHQL_API_URL', {
  providedIn: 'root',
  factory: () => '/graphql',
});

export const GRAPHQL_WS_URL = new InjectionToken<string>('GRAPHQL_WS_URL', {
  providedIn: 'root',
  factory: () => {
    const protocol = typeof window !== 'undefined' && window.location.protocol === 'https:' ? 'wss:' : 'ws:';
    const host = typeof window !== 'undefined' ? window.location.host : 'localhost';
    return `${protocol}//${host}/graphql`;
  },
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
  private readonly wsUrl = inject(GRAPHQL_WS_URL);

  query<T>(query: string, variables?: Record<string, unknown>): Observable<T> {
    return this.request<T>(query, variables);
  }

  mutate<T>(mutation: string, variables?: Record<string, unknown>): Observable<T> {
    return this.request<T>(mutation, variables);
  }

  subscribe<T>(subscription: string, variables?: Record<string, unknown>): Observable<T> {
    return new Observable<T>((subscriber) => {
      let ws: WebSocket | null = null;

      const connect = () => {
        ws = new WebSocket(this.wsUrl, 'graphql-transport-ws');

        ws.onopen = () => {
          ws!.send(JSON.stringify({ type: 'connection_init' }));
        };

        ws.onmessage = (event) => {
          const message = JSON.parse(event.data);

          switch (message.type) {
            case 'connection_ack':
              ws!.send(
                JSON.stringify({
                  id: '1',
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
            ws.send(JSON.stringify({ id: '1', type: 'complete' }));
          }
          ws.close();
          ws = null;
        }
      };
    });
  }

  private request<T>(query: string, variables?: Record<string, unknown>): Observable<T> {
    const body = JSON.stringify({ query, variables });

    return from(
      fetch(this.apiUrl, {
        method: 'POST',
        headers: {
          'Content-Type': 'application/json',
          Accept: 'application/json',
        },
        body,
        credentials: 'include',
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
}

export class GraphQLRequestError extends Error {
  constructor(public readonly errors: GraphQLError[]) {
    super(errors.map((e) => e.message).join('; '));
    this.name = 'GraphQLRequestError';
  }
}
