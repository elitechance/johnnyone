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
 */

interface RelayEnvelope {
  type: 'chat_request' | 'chat_delta' | 'chat_complete' | 'chat_message' | 'heartbeat' | 'ping' | 'pong';
  relayId?: string;
  sessionId?: string;
  data?: unknown;
}

type ClientType = 'mobile' | 'desktop';

interface ClientMeta {
  clientType: ClientType;
  userId: string;
  tenantId: string;
}

export class ChatRelayDO implements DurableObject {
  private state: DurableObjectState;
  private clients: Map<WebSocket, ClientMeta> = new Map();

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
    const clientType = (url.searchParams.get('clientType') ?? 'mobile') as ClientType;
    const userId = url.searchParams.get('userId') ?? '';
    const tenantId = url.searchParams.get('tenantId') ?? '';

    this.state.acceptWebSocket(server);
    this.clients.set(server, { clientType, userId, tenantId });

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

  webSocketMessage(ws: WebSocket, message: string | ArrayBuffer): void {
    try {
      const raw = typeof message === 'string' ? message : new TextDecoder().decode(message);
      const envelope = JSON.parse(raw) as RelayEnvelope;

      const meta = this.clients.get(ws);
      if (!meta) return;

      switch (envelope.type) {
        case 'heartbeat':
          // Desktop keepalive — respond with pong
          ws.send(JSON.stringify({ type: 'heartbeat', data: { timestamp: new Date().toISOString() } }));
          break;

        case 'ping':
          ws.send(JSON.stringify({ type: 'pong', timestamp: Date.now() }));
          break;

        case 'chat_request':
          // Mobile → desktop: forward the chat request
          this.broadcastTo('desktop', envelope);
          break;

        case 'chat_delta':
        case 'chat_complete':
        case 'chat_message':
          // Desktop → mobile: broadcast response data
          this.broadcastTo('mobile', envelope);
          break;
      }
    } catch {
      // Ignore malformed messages
    }
  }

  webSocketClose(ws: WebSocket): void {
    this.clients.delete(ws);
  }

  webSocketError(ws: WebSocket): void {
    this.clients.delete(ws);
  }

  /**
   * Broadcast a message to all clients of a specific type.
   */
  private broadcastTo(targetType: ClientType, envelope: RelayEnvelope): void {
    const payload = JSON.stringify(envelope);
    for (const [ws, meta] of this.clients.entries()) {
      if (meta.clientType === targetType) {
        try {
          ws.send(payload);
        } catch {
          // Client disconnected
        }
      }
    }
  }
}
