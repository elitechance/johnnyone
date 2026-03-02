import { runAgentLoop } from './agent-loop';

interface SessionState {
  sessionId: string;
  tenantId: string;
  userId: string;
  isProcessing: boolean;
}

interface ProcessMessagePayload {
  sessionId: string;
  messageId: string;
  tenantId: string;
  userId: string;
  content: string;
  sourceChannel: string | null;
}

interface ToolActionPayload {
  executionId: string;
  sessionId: string;
}

/**
 * AgentSessionDO - Durable Object that manages a single AI agent session.
 *
 * Responsibilities:
 * - Accepts WebSocket connections from clients for real-time updates
 * - Processes user messages through the agent loop (LLM -> tool use -> loop)
 * - Manages session state and timeouts via alarms
 * - Broadcasts events to connected WebSocket clients
 */
export class AgentSessionDO implements DurableObject {
  private state: DurableObjectState;
  private env: Record<string, unknown>;
  private sessions: Map<WebSocket, { userId: string; tenantId: string }> = new Map();
  private sessionState: SessionState | null = null;

  constructor(state: DurableObjectState, env: Record<string, unknown>) {
    this.state = state;
    this.env = env;

    // Restore WebSocket connections on wake
    this.state.getWebSockets().forEach((ws) => {
      const meta = this.state.getWebSocketAutoResponseTimestamp(ws);
      // Connections are restored automatically by the runtime
    });
  }

  async fetch(request: Request): Promise<Response> {
    const url = new URL(request.url);
    const path = url.pathname;

    switch (path) {
      case '/websocket':
        return this.handleWebSocketUpgrade(request);
      case '/process-message':
        return this.handleProcessMessage(request);
      case '/tool-approved':
        return this.handleToolApproved(request);
      case '/tool-rejected':
        return this.handleToolRejected(request);
      case '/tool-cancelled':
        return this.handleToolCancelled(request);
      case '/tool-result':
        return this.handleToolResult(request);
      default:
        return new Response('Not found', { status: 404 });
    }
  }

  private async handleWebSocketUpgrade(request: Request): Promise<Response> {
    const upgradeHeader = request.headers.get('Upgrade');
    if (upgradeHeader !== 'websocket') {
      return new Response('Expected WebSocket upgrade', { status: 426 });
    }

    const pair = new WebSocketPair();
    const [client, server] = [pair[0], pair[1]];

    // Extract auth info from query params or headers
    const url = new URL(request.url);
    const userId = url.searchParams.get('userId') ?? '';
    const tenantId = url.searchParams.get('tenantId') ?? '';

    this.state.acceptWebSocket(server);
    this.sessions.set(server, { userId, tenantId });

    // Send current session state
    const currentState = await this.state.storage.get<SessionState>('sessionState');
    if (currentState) {
      server.send(
        JSON.stringify({
          type: 'session_state',
          data: {
            sessionId: currentState.sessionId,
            isProcessing: currentState.isProcessing,
          },
        }),
      );
    }

    return new Response(null, { status: 101, webSocket: client });
  }

  private async handleProcessMessage(request: Request): Promise<Response> {
    const payload = (await request.json()) as ProcessMessagePayload;

    // Store session state
    this.sessionState = {
      sessionId: payload.sessionId,
      tenantId: payload.tenantId,
      userId: payload.userId,
      isProcessing: true,
    };
    await this.state.storage.put('sessionState', this.sessionState);

    // Broadcast processing start
    this.broadcast({
      type: 'processing_start',
      data: { sessionId: payload.sessionId, messageId: payload.messageId },
    });

    // Set an alarm for timeout (5 minutes)
    const alarmTime = Date.now() + 5 * 60 * 1000;
    await this.state.storage.setAlarm(alarmTime);

    // Run the agent loop
    try {
      await runAgentLoop({
        sessionId: payload.sessionId,
        messageId: payload.messageId,
        tenantId: payload.tenantId,
        userId: payload.userId,
        content: payload.content,
        env: this.env,
        broadcast: (msg) => this.broadcast(msg),
      });
    } catch (err) {
      const error = err instanceof Error ? err.message : 'Unknown error';
      console.error('Agent loop error:', error);
      this.broadcast({
        type: 'error',
        data: { sessionId: payload.sessionId, error },
      });
    } finally {
      // Mark processing complete
      if (this.sessionState) {
        this.sessionState.isProcessing = false;
        await this.state.storage.put('sessionState', this.sessionState);
      }

      this.broadcast({
        type: 'processing_complete',
        data: { sessionId: payload.sessionId },
      });

      // Cancel the timeout alarm
      await this.state.storage.deleteAlarm();
    }

    return new Response('OK', { status: 200 });
  }

  private async handleToolApproved(request: Request): Promise<Response> {
    const payload = (await request.json()) as ToolActionPayload;

    this.broadcast({
      type: 'tool_approved',
      data: { executionId: payload.executionId, sessionId: payload.sessionId },
    });

    // Store the approval signal for the agent loop to pick up
    await this.state.storage.put(`tool-signal:${payload.executionId}`, 'approved');

    return new Response('OK', { status: 200 });
  }

  private async handleToolRejected(request: Request): Promise<Response> {
    const payload = (await request.json()) as ToolActionPayload;

    this.broadcast({
      type: 'tool_rejected',
      data: { executionId: payload.executionId, sessionId: payload.sessionId },
    });

    await this.state.storage.put(`tool-signal:${payload.executionId}`, 'rejected');

    return new Response('OK', { status: 200 });
  }

  private async handleToolCancelled(request: Request): Promise<Response> {
    const payload = (await request.json()) as ToolActionPayload;

    this.broadcast({
      type: 'tool_cancelled',
      data: { executionId: payload.executionId, sessionId: payload.sessionId },
    });

    await this.state.storage.put(`tool-signal:${payload.executionId}`, 'cancelled');

    return new Response('OK', { status: 200 });
  }

  private async handleToolResult(request: Request): Promise<Response> {
    const payload = (await request.json()) as {
      executionId: string;
      sessionId: string;
      output: string;
      status: string;
      durationMs: number;
    };

    // Store the tool result for the agent loop
    await this.state.storage.put(`tool-result:${payload.executionId}`, payload);

    this.broadcast({
      type: 'tool_result',
      data: payload,
    });

    return new Response('OK', { status: 200 });
  }

  async alarm(): Promise<void> {
    // Timeout alarm - agent has been processing too long
    if (this.sessionState?.isProcessing) {
      console.error(
        `Agent session ${this.sessionState.sessionId} timed out after 5 minutes`,
      );

      this.sessionState.isProcessing = false;
      await this.state.storage.put('sessionState', this.sessionState);

      this.broadcast({
        type: 'timeout',
        data: {
          sessionId: this.sessionState.sessionId,
          error: 'Agent processing timed out after 5 minutes',
        },
      });
    }
  }

  webSocketMessage(ws: WebSocket, message: string | ArrayBuffer): void {
    try {
      const data = JSON.parse(typeof message === 'string' ? message : new TextDecoder().decode(message));

      // Handle ping/pong for keepalive
      if (data.type === 'ping') {
        ws.send(JSON.stringify({ type: 'pong', timestamp: Date.now() }));
        return;
      }

      // Handle client-side events if needed
      if (data.type === 'subscribe') {
        // Client subscribing to specific events
        const meta = this.sessions.get(ws);
        if (meta) {
          this.sessions.set(ws, { ...meta, ...data.filters });
        }
      }
    } catch {
      // Ignore malformed messages
    }
  }

  webSocketClose(ws: WebSocket, code: number, reason: string, wasClean: boolean): void {
    this.sessions.delete(ws);
  }

  webSocketError(ws: WebSocket, error: unknown): void {
    console.error('WebSocket error:', error);
    this.sessions.delete(ws);
  }

  private broadcast(message: Record<string, unknown>): void {
    const payload = JSON.stringify(message);
    for (const ws of this.state.getWebSockets()) {
      try {
        ws.send(payload);
      } catch {
        // Client disconnected, will be cleaned up
      }
    }
  }
}
