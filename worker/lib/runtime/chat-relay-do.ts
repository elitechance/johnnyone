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
  type: 'chat_request' | 'chat_delta' | 'chat_complete' | 'chat_message' | 'session_deleted' | 'session_updated' | 'heartbeat' | 'ping' | 'pong' | 'rpc_request' | 'rpc_response' | 'terminal_screen' | 'terminal_command' | 'terminal_visual_subscribe' | 'terminal_visual_unsubscribe' | 'terminal_resize' | 'terminal_kill' | 'terminal_command_ack' | 'agent_plan_run_updated' | 'stream_event' | 'stream_subscribe' | 'stream_unsubscribe';
  relayId?: string;
  sessionId?: string;
  requestId?: string;
  data?: unknown;
}

type ClientType = 'mobile' | 'desktop' | 'api';
type PendingRpcResult = { success?: boolean; data?: unknown; error?: string; timedOut?: boolean };

import { verifyJwtWithFallback } from 'modules/auth/auth-middleware';
import { resolveOnlineNode } from '../auth/resolve-online-node';
import { getSocketAuth } from '../auth/socket-auth-context';
import { isTokenLike, validateApiKeyToken, updateLastUsed } from '../auth/api-key';
import { requireScope } from '../auth/scopes';
import { verifySessionOwnership, invalidateSessionOwnership } from './session-ownership';

export class ChatRelayDO implements DurableObject {
  private state: DurableObjectState;
  private env: Record<string, unknown>;
  private pendingRpc: Map<string, {
    resolve: (value: PendingRpcResult) => void;
    timer: ReturnType<typeof setTimeout>;
  }> = new Map();

  constructor(state: DurableObjectState, env: Record<string, unknown>) {
    this.state = state;
    this.env = env || {};
  }

  private acceptAndTag(server: WebSocket, clientType: ClientType, auth: { tenantId: string; userId: string; apiKey?: { id: string; scopes: string[] } }) {
    const tags = [clientType, `tenant:${auth.tenantId}`, `user:${auth.userId}`];
    this.state.acceptWebSocket(server, tags);
    try {
      server.serializeAttachment({ tenantId: auth.tenantId, userId: auth.userId, clientType, apiKey: auth.apiKey });
    } catch {}
  }

  private async computeInternalMarker(nodeId: string, userId: string, secret: string): Promise<string> {
    const input = `${nodeId}|${userId}`;
    const enc = new TextEncoder();
    const key = await crypto.subtle.importKey(
      'raw',
      enc.encode(secret),
      { name: 'HMAC', hash: 'SHA-256' },
      false,
      ['sign']
    );
    const signature = await crypto.subtle.sign('HMAC', key, enc.encode(input));
    const bytes = new Uint8Array(signature);
    let binary = '';
    for (let i = 0; i < bytes.byteLength; i++) {
      binary += String.fromCharCode(bytes[i]);
    }
    return btoa(binary)
      .replace(/\+/g, '-')
      .replace(/\//g, '_')
      .replace(/=+$/, '');
  }

  private constantTimeEqual(a: string, b: string): boolean {
    if (a.length !== b.length) return false;
    let result = 0;
    for (let i = 0; i < a.length; i++) {
      result |= a.charCodeAt(i) ^ b.charCodeAt(i);
    }
    return result === 0;
  }

  async fetch(request: Request): Promise<Response> {
    const url = new URL(request.url);
    const path = url.pathname;

    if (path === '/websocket' || path === '/websocket-resolved') {
      return await this.handleWebSocketUpgrade(request);
    }

    if (path === '/relay') {
      return this.handleRelayPost(request);
    }

    if (path === '/relay-rpc') {
      return this.handleRelayRpc(request);
    }

    return new Response('Not found', { status: 404 });
  }

  private async handleWebSocketUpgrade(request: Request): Promise<Response> {
    const upgradeHeader = request.headers.get('Upgrade');
    if (upgradeHeader !== 'websocket') {
      return new Response('Expected WebSocket upgrade', { status: 426 });
    }

    const url = new URL(request.url);
    const clientTypeFromQuery = url.searchParams.get('clientType') || 'mobile';

    // Per decision: require authentication on EVERY /api/relay/ws upgrade.
    // Run verify for all clientTypes — never gate it on clientType.
    // Resolve the node server-side from the JWT (tenantId+userId), never from client-supplied nodeId.
    // clientType is kept only as a socket tag.
    let token = request.headers.get('Authorization')?.replace(/^Bearer\s+/i, '').trim();
    if (!token) {
      token = url.searchParams.get('token') || '';
    }
    if (!token) {
      return new Response('Unauthorized', { status: 401 });
    }

    let jwtSecret = String((this.env as any).JWT_SECRET || (this.env as any).jwt_secret || '');
    let jwtSecretPrev: string | undefined;
    const d1ForConfig: any = (this.env as any).DB;
    if (d1ForConfig && !jwtSecret) {
      try {
        const cfg = await d1ForConfig
          .prepare('SELECT key, value FROM config WHERE key IN (?, ?)')
          .bind('jwt_secret', 'jwt_secret_prev')
          .all();
        const map: Record<string, string> = {};
        for (const r of (cfg.results || [])) {
          map[r.key] = r.value;
        }
        if (map.jwt_secret) jwtSecret = map.jwt_secret;
        jwtSecretPrev = map.jwt_secret_prev;
      } catch {}
    }
    if (!jwtSecret) {
      return new Response('Unauthorized', { status: 401 });
    }

    const envAny: any = this.env || {};
    const db: D1Database = envAny.DB as D1Database;

    let tenantId = '';
    let userId = '';
    let apiKeyInfo: { id: string; scopes: string[] } | undefined;

    if (isTokenLike(token)) {
      // Alternate token path (Phase 05): jk_ key
      if (!db) {
        return new Response('Desktop not connected', { status: 503 });
      }
      const v = await validateApiKeyToken(token, db);
      if (!v) {
        return new Response('Unauthorized', { status: 401 });
      }
      tenantId = v.tenantId;
      userId = v.userId;
      apiKeyInfo = v.apiKey;
      updateLastUsed(db, v.apiKey.id); // best-effort, shared helper
    } else {
      // Standard JWT path (Phase 00)
      let payload: any;
      try {
        payload = await verifyJwtWithFallback(token, jwtSecret, jwtSecretPrev);
      } catch {
        return new Response('Unauthorized', { status: 401 });
      }

      userId = payload.sub || payload.userId || payload.uid || '';
      tenantId = payload.tenantId || payload.tid || payload.tenant_id || '';
      if (!userId || !tenantId) {
        return new Response('Unauthorized', { status: 401 });
      }
    }
    const auth = { tenantId, userId, apiKey: apiKeyInfo };

    if (!db) {
      return new Response('Desktop not connected', { status: 503 });
    }

    // Resolve the node server-side (from JWT or key); never from client-supplied nodeId.
    const node = await resolveOnlineNode(db, { tenantId: auth.tenantId, userId: auth.userId });

    if (!node) {
      return new Response('Desktop not connected', { status: 503 });
    }

    // Re-dispatch logic: always ensure the client ends up connected to the DO for the *resolved* node.id
    // (from JWT), ignoring whatever client nodeId the framework used to bind us.
    // This is done by forwarding the upgrade to idFromName(node.id) so accept happens in the right instance.
    // The marker is an HMAC using the server secret so clients cannot forge x-internal-forward to bypass
    // the server-side node resolution / re-dispatch (P0 cross-tenant bypass).
    // clientTypeFromQuery is used *only* for the socket tag (never for auth).
    const providedMarker = request.headers.get('x-internal-forward') || '';
    let isResolvedForward = false;
    if (providedMarker) {
      const expectedMarker = await this.computeInternalMarker(node.id, auth.userId, jwtSecret);
      if (this.constantTimeEqual(providedMarker, expectedMarker)) {
        isResolvedForward = true;
      } else {
        // Forged or invalid marker from client - reject to prevent taking accept-here on wrong DO instance
        return new Response('Forbidden', { status: 403 });
      }
    }
    if (isResolvedForward) {
      // Already forwarded to the target node DO: auth/resolve done upstream, just accept here.
      const pair = new WebSocketPair();
      const [client, server] = [pair[0], pair[1]];
      const tagType = clientTypeFromQuery === 'desktop' ? 'desktop' : (clientTypeFromQuery === 'mobile' ? 'mobile' : 'api') as ClientType;
      this.acceptAndTag(server, tagType, auth);
      return new Response(null, { status: 101, webSocket: client });
    }

    // Normal entrypoint: re-dispatch to correct DO instance (calls idFromName(resolved) here)
    const doNs: any = envAny.CHAT_RELAY_DO;
    if (doNs && doNs.idFromName && doNs.get) {
      const targetId = doNs.idFromName(node.id);
      const targetStub = doNs.get(targetId);
      const fwdUrl = new URL(request.url);
      fwdUrl.pathname = '/websocket-resolved';
      // do not forward client _resolved or nodeId that could be used for bypass; set internal marker via header
      fwdUrl.searchParams.delete('_resolved');
      fwdUrl.searchParams.delete('nodeId');
      fwdUrl.searchParams.set('nodeId', node.id);  // set to resolved
      const fwdHeaders = new Headers(request.headers);
      const marker = await this.computeInternalMarker(node.id, auth.userId, jwtSecret);
      fwdHeaders.set('x-internal-forward', marker);
      const fwdReq = new Request(fwdUrl.toString(), { headers: fwdHeaders });
      return await targetStub.fetch(fwdReq);
    }

    // Fallback (e.g. some tests without full ns): accept here
    const pair = new WebSocketPair();
    const [client, server] = [pair[0], pair[1]];
    const tagType = clientTypeFromQuery === 'desktop' ? 'desktop' : (clientTypeFromQuery === 'mobile' ? 'mobile' : 'api') as ClientType;
    this.acceptAndTag(server, tagType, auth);
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

  async webSocketMessage(ws: WebSocket, message: string | ArrayBuffer): Promise<void> {
    try {
      const raw = typeof message === 'string' ? message : new TextDecoder().decode(message);
      const envelope = JSON.parse(raw) as RelayEnvelope;

      // Determine identity + client type. Prefer the auth context reader (Task 05) so api sockets
      // carry tenant/user. Fall back to legacy tag logic for desktop/mobile.
      const authCtx = getSocketAuth(ws, this.state);
      let clientType: 'mobile' | 'desktop' | 'api' = 'mobile';
      if (authCtx) {
        clientType = authCtx.clientType;
      } else {
        const tags = this.state.getTags(ws);
        clientType = tags.includes('desktop') ? 'desktop' : 'mobile';
      }

      switch (envelope.type) {
        case 'heartbeat':
          // Desktop keepalive — respond with pong
          ws.send(JSON.stringify({ type: 'heartbeat', data: { timestamp: new Date().toISOString() } }));
          // Persist liveness so listDesktopNodes.lastHeartbeatAt advances and the
          // 'online' flag stays trustworthy, instead of freezing at registration
          // time. Best-effort: never break the relay on a heartbeat write.
          if (clientType === 'desktop' && authCtx?.tenantId && authCtx?.userId) {
            try {
              await ((this.env as any).DB as D1Database)
                .prepare(
                  `UPDATE desktop_nodes SET last_heartbeat_at = datetime('now'), status = 'online', updated_at = datetime('now')
                   WHERE tenant_id = ? AND user_id = ? AND is_deleted = 0`,
                )
                .bind(authCtx.tenantId, authCtx.userId)
                .run();
            } catch {
              // ignore — heartbeat persistence is best-effort
            }
          }
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
        // Stream-event per-session subscription controls (overhaul P2, D6) ride the SAME
        // client→desktop forward + ownership gate as the terminal visual controls.
        case 'stream_subscribe':
        case 'stream_unsubscribe':
          // Mobile/api → desktop: forward ONLY after ownership gate (Task 02)
          if (clientType !== 'desktop') {
            const sid = ((envelope.data as any)?.sessionId as string) || (envelope.sessionId as string) || '';
            if (!authCtx || !authCtx.tenantId || !authCtx.userId) {
              // no auth ctx (should not happen post-Phase 00) → deny
              try {
                ws.send(JSON.stringify({ type: 'terminal_command_ack', error: 'forbidden_session', requestId: envelope.requestId, sessionId: sid }));
              } catch {}
              break;
            }

            // Scope guard for api key sockets (JWT unrestricted)
            if ((authCtx as any).apiKey) {
              const sctx = { apiKey: (authCtx as any).apiKey };
              // Read-only controls (visual subscribe, stream subscribe/unsubscribe) need
              // terminal:read; the mutating terminal controls need terminal:write. No new scope
              // is invented — a per-session stream subscription is a read (overhaul P2, D6/D8).
              const readOnlyControls = ['terminal_visual_subscribe', 'stream_subscribe', 'stream_unsubscribe'];
              const needed = readOnlyControls.includes(envelope.type) ? 'terminal:read' : 'terminal:write';
              try {
                requireScope(sctx as any, needed as any);
              } catch {
                try {
                  ws.send(JSON.stringify({ type: 'terminal_command_ack', error: 'FORBIDDEN_SCOPE', requestId: envelope.requestId, sessionId: sid, scope: needed }));
                } catch {}
                break;
              }
            }

            const rpcCtx = {
              db: (this.env as any).DB as D1Database,
              env: { CHAT_RELAY_DO: (this.env as any).CHAT_RELAY_DO },
              auth: { userId: authCtx.userId, tenantId: authCtx.tenantId },
            };
            try {
              const owned = await verifySessionOwnership(rpcCtx as any, sid);
              if (owned) {
                this.broadcastTo('desktop', envelope);
              } else {
                try {
                  ws.send(JSON.stringify({ type: 'terminal_command_ack', error: 'forbidden_session', requestId: envelope.requestId, sessionId: sid }));
                } catch {}
              }
            } catch {
              try {
                ws.send(JSON.stringify({ type: 'terminal_command_ack', error: 'forbidden_session', requestId: envelope.requestId, sessionId: sid }));
              } catch {}
            }
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
        // Structured stream events are desktop→clients only (overhaul P2, D6). A client must not
        // be able to inject one to others — the `clientType === 'desktop'` guard below enforces it.
        case 'stream_event':
          // Desktop → mobile + api: broadcast response data (api partners must receive terminal_screen etc.)
          if (clientType === 'desktop') {
            this.broadcastTo('mobile', envelope);
            this.broadcastTo('api', envelope);
          }
          // Invalidate ownership cache on delete/archive so revoked sessions stop streaming (Task 05)
          if ((envelope.type === 'session_deleted' ||
               (envelope.type === 'session_updated' && (envelope.data as any)?.status === 'archived')) &&
              envelope.sessionId) {
            invalidateSessionOwnership(envelope.sessionId as string);
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
