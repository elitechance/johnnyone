/**
 * ChatRelayDO - Thin relay Durable Object.
 *
 * Two connection types:
 *  - Mobile clients (subscribed via WebSocket for real-time deltas)
 *  - Desktop socket (the desktop Tauri app, which processes all AI)
 *
 * Mobile → desktop: forward relay envelope with chat request
 * Desktop → mobile: broadcast relay envelope with deltas/responses
 * No AI logic — pure message relay.
 *
 * Uses WebSocket Hibernation API with tags so that client metadata
 * survives DO hibernation/wake cycles.
 */

interface RelayEnvelope {
  type: 'chat_request' | 'chat_delta' | 'chat_complete' | 'chat_message' | 'session_deleted' | 'session_updated' | 'heartbeat' | 'ping' | 'pong' | 'rpc_request' | 'rpc_response' | 'terminal_screen' | 'terminal_command' | 'terminal_visual_subscribe' | 'terminal_visual_unsubscribe' | 'terminal_resize' | 'terminal_kill' | 'terminal_command_ack' | 'agent_plan_run_updated';
  relayId?: string;
  sessionId?: string;
  requestId?: string;
  data?: unknown;
}

type ClientType = 'mobile' | 'desktop';
type PendingRpcResult = { success?: boolean; data?: unknown; error?: string; timedOut?: boolean };

export class ChatRelayDO implements DurableObject {
  private state: DurableObjectState;
  private pendingRpc: Map<string, {
    resolve: (value: PendingRpcResult) => void;
    timer: ReturnType<typeof setTimeout>;
  }> = new Map();

  constructor(state: DurableObjectState, _env: Record<string, unknown>) {
    this.state = state;
  }

  async fetch(request: Request): Promise<Response> {
    const url = new URL(request.url);
    const path = url.pathname;

    if (path === '/websocket') {
      return this.handleWebSocketUpgrade(request);
    }

    if (path === '/relay') {
      return this.handleRelayPost(request);
    }

    if (path === '/relay-rpc') {
      return this.handleRelayRpc(request);
    }

    return new Response('Not found', { status: 404 });
  }

  private handleWebSocketUpgrade(request: Request): Response {
    const upgradeHeader = request.headers.get('Upgrade');
    if (upgradeHeader !== 'websocket') {
      return new Response('Expected WebSocket upgrade', { status: 426 });
    }

    const pair = new WebSocketPair();
    const [client, server] = [pair[0], pair[1]];

    const url = new URL(request.url);
    const clientType = url.searchParams.get('clientType') ?? 'mobile';

    // Tag the WebSocket with its client type so it survives hibernation
    this.state.acceptWebSocket(server, [clientType]);

    return new Response(null, { status: 101, webSocket: client });
  }

  /**
   * HTTP POST relay endpoint for mutations that don't need WebSocket.
   * Worker resolver posts the chat request here, DO forwards to desktop.
   */
  private async handleRelayPost(request: Request): Promise<Response> {
    const envelope = (await request.json()) as RelayEnvelope;

    if (envelope.type === 'chat_request') {
      // Forward to desktop client(s)
      this.broadcastTo('desktop', envelope);
      return Response.json({ success: true });
    }

    return new Response('Unknown relay type', { status: 400 });
  }

  /**
   * RPC endpoint: forward a query to the desktop and wait for the response.
   * The connection stays open until the desktop replies or a 15s timeout.
   */
  private async handleRelayRpc(request: Request): Promise<Response> {
    const body = (await request.json()) as { method: string; params?: unknown };

    // Check that at least one desktop client is connected (uses tags, survives hibernation)
    const desktopSockets = this.state.getWebSockets('desktop');
    if (desktopSockets.length === 0) {
      return Response.json({ success: false, error: 'Desktop not connected' }, { status: 503 });
    }

    const requestId = crypto.randomUUID();

    // Create a promise that will be resolved when the desktop responds
    const result = await new Promise<PendingRpcResult>((resolve) => {
      const timer = setTimeout(() => {
        this.pendingRpc.delete(requestId);
        resolve({ timedOut: true });
      }, 15_000);

      this.pendingRpc.set(requestId, { resolve, timer });

      // Forward the RPC request to the desktop
      const envelope: RelayEnvelope = {
        type: 'rpc_request',
        requestId,
        data: {
          requestId,
          method: body.method,
          params: body.params ?? {},
        },
      };
      this.broadcastTo('desktop', envelope);
    });

    if (result.timedOut) {
      return Response.json({ success: false, error: 'Desktop RPC timed out' }, { status: 504 });
    }

    return Response.json(result);
  }

  webSocketMessage(ws: WebSocket, message: string | ArrayBuffer): void {
    try {
      const raw = typeof message === 'string' ? message : new TextDecoder().decode(message);
      const envelope = JSON.parse(raw) as RelayEnvelope;

      // Determine client type from WebSocket tags (survives hibernation)
      const tags = this.state.getTags(ws);
      const clientType = tags.includes('desktop') ? 'desktop' : 'mobile';

      switch (envelope.type) {
        case 'heartbeat':
          // Desktop keepalive — respond with pong
          ws.send(JSON.stringify({ type: 'heartbeat', data: { timestamp: new Date().toISOString() } }));
          break;

        case 'ping':
          ws.send(JSON.stringify({ type: 'pong', timestamp: Date.now() }));
          break;

        case 'chat_request':
        case 'terminal_command':
        case 'terminal_visual_subscribe':
        case 'terminal_visual_unsubscribe':
        case 'terminal_resize':
        case 'terminal_kill':
          // Mobile → desktop: forward the chat request
          if (clientType === 'mobile') {
            this.broadcastTo('desktop', envelope);
          }
          break;

        case 'chat_delta':
        case 'chat_complete':
        case 'chat_message':
        case 'session_deleted':
        case 'session_updated':
        case 'terminal_screen':
        case 'terminal_command_ack':
        case 'agent_plan_run_updated':
          // Desktop → mobile: broadcast response data
          if (clientType === 'desktop') {
            this.broadcastTo('mobile', envelope);
          }
          break;

        case 'rpc_response': {
          // Desktop responding to an RPC query — resolve the pending promise
          const rpcData = envelope.data as { requestId?: string; success?: boolean; data?: unknown; error?: string } | undefined;
          const rpcRequestId = rpcData?.requestId;
          if (rpcRequestId && this.pendingRpc.has(rpcRequestId)) {
            const pending = this.pendingRpc.get(rpcRequestId)!;
            clearTimeout(pending.timer);
            this.pendingRpc.delete(rpcRequestId);
            pending.resolve({ success: rpcData?.success, data: rpcData?.data, error: rpcData?.error });
          }
          break;
        }
      }
    } catch {
      // Ignore malformed messages
    }
  }

  webSocketClose(): void {
    // No cleanup needed — runtime manages tagged WebSockets
  }

  webSocketError(): void {
    // No cleanup needed — runtime manages tagged WebSockets
  }

  /**
   * Broadcast a message to all clients of a specific type using WebSocket tags.
   */
  private broadcastTo(targetType: ClientType, envelope: RelayEnvelope): void {
    const payload = JSON.stringify(envelope);
    const sockets = this.state.getWebSockets(targetType);
    for (const ws of sockets) {
      try {
        ws.send(payload);
      } catch {
        // Client disconnected — runtime will clean up
      }
    }
  }
}
