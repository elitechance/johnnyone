interface HostGraphqlEnv {
  HOST_GRAPHQL_URL?: string;
  [key: string]: unknown;
}

interface GraphqlResponse<T> {
  data?: T;
  errors?: Array<{ message?: string }>;
}

export interface HostGraphqlSubscription<T> {
  completed: Promise<void>;
  unsubscribe: () => void;
}

export function getHostGraphqlUrl(env: HostGraphqlEnv): string {
  const configured = typeof env.HOST_GRAPHQL_URL === 'string' ? env.HOST_GRAPHQL_URL.trim() : '';
  return configured || 'http://127.0.0.1:7788/graphql';
}

export function getHostGraphqlWsUrl(env: HostGraphqlEnv): string {
  const url = new URL(getHostGraphqlUrl(env));
  url.protocol = url.protocol === 'https:' ? 'wss:' : 'ws:';

  if (url.pathname.endsWith('/graphql')) {
    url.pathname = `${url.pathname}/ws`;
  } else if (!url.pathname.endsWith('/graphql/ws')) {
    url.pathname = `${url.pathname.replace(/\/$/, '')}/graphql/ws`;
  }

  return url.toString();
}

export async function hostGraphqlRequest<T>(
  env: HostGraphqlEnv,
  query: string,
  variables?: Record<string, unknown>,
): Promise<T> {
  const response = await fetch(getHostGraphqlUrl(env), {
    method: 'POST',
    headers: {
      'Content-Type': 'application/json',
      Accept: 'application/json',
    },
    body: JSON.stringify({ query, variables }),
  });

  if (!response.ok) {
    const body = await response.text();
    throw new Error(`Host GraphQL request failed (${response.status}): ${body}`);
  }

  const result = (await response.json()) as GraphqlResponse<T>;
  if (result.errors?.length) {
    throw new Error(result.errors.map((error) => error.message || 'Host GraphQL error').join('; '));
  }

  if (typeof result.data === 'undefined') {
    throw new Error('Host GraphQL returned no data');
  }

  return result.data;
}

export async function hostGraphqlSubscribe<T>(
  env: HostGraphqlEnv,
  query: string,
  variables: Record<string, unknown> | undefined,
  onData: (data: T) => void,
): Promise<HostGraphqlSubscription<T>> {
  return new Promise<HostGraphqlSubscription<T>>((resolve, reject) => {
    const socket = new WebSocket(getHostGraphqlWsUrl(env), 'graphql-transport-ws');
    const operationId = crypto.randomUUID();
    let acknowledged = false;
    let settled = false;
    let unsubscribed = false;
    let completedResolve: (() => void) | null = null;
    const completed = new Promise<void>((complete) => {
      completedResolve = complete;
    });

    const completeOnce = () => {
      if (!completedResolve) return;
      completedResolve();
      completedResolve = null;
    };

    const fail = (error: Error) => {
      if (!settled) {
        settled = true;
        reject(error);
      }

      try {
        socket.close();
      } catch {
        // Ignore close failures.
      }

      completeOnce();
    };

    const unsubscribe = () => {
      if (unsubscribed) return;
      unsubscribed = true;

      try {
        if (socket.readyState === WebSocket.OPEN) {
          socket.send(JSON.stringify({ id: operationId, type: 'complete' }));
        }
      } catch {
        // Ignore websocket completion send failures.
      }

      try {
        socket.close();
      } catch {
        // Ignore close failures.
      }

      completeOnce();
    };

    socket.addEventListener('open', () => {
      socket.send(JSON.stringify({ type: 'connection_init' }));
    });

    socket.addEventListener('message', (event: MessageEvent) => {
      const payload =
        typeof event.data === 'string'
          ? event.data
          : new TextDecoder().decode(event.data as ArrayBuffer);

      const message = JSON.parse(payload) as {
        type: string;
        payload?: { data?: T; errors?: Array<{ message?: string }> } | Array<{ message?: string }>;
      };

      switch (message.type) {
        case 'connection_ack':
          acknowledged = true;
          socket.send(
            JSON.stringify({
              id: operationId,
              type: 'subscribe',
              payload: { query, variables },
            }),
          );

          if (!settled) {
            settled = true;
            resolve({ completed, unsubscribe });
          }
          break;

        case 'next': {
          const payloadData = Array.isArray(message.payload) ? undefined : message.payload;
          if (payloadData?.errors?.length) {
            fail(
              new Error(
                payloadData.errors
                  .map((error) => error.message || 'Host GraphQL subscription error')
                  .join('; '),
              ),
            );
            return;
          }

          if (payloadData?.data) {
            onData(payloadData.data);
          }
          break;
        }

        case 'error': {
          const errors = Array.isArray(message.payload) ? message.payload : [];
          fail(new Error(errors.map((error) => error.message || 'Host GraphQL subscription error').join('; ')));
          break;
        }

        case 'complete':
          completeOnce();
          break;
      }
    });

    socket.addEventListener('error', () => {
      fail(new Error('Host GraphQL subscription websocket error'));
    });

    socket.addEventListener('close', () => {
      if (!acknowledged && !unsubscribed) {
        fail(new Error('Host GraphQL subscription closed before acknowledgement'));
        return;
      }

      completeOnce();
    });
  });
}
