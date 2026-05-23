import { Injectable, NgZone, inject } from '@angular/core';
import { Observable, Subject, firstValueFrom } from 'rxjs';
import { AgentPlanRun, AiSession, DesktopNode, JohnnyApiService, TerminalScreen } from '@johnnyone/ui';

interface RelayEnvelope {
  type: string;
  data?: unknown;
}

interface TerminalCommandAck {
  requestId: string;
  sessionId: string;
  accepted: boolean;
  error?: string;
}

export interface TerminalInputAttachment {
  id: string;
  originalName: string;
  contentType: string;
  size: number;
}

export interface AgentPlanRunUpdate {
  planId: string;
  deleted: boolean;
  run?: AgentPlanRun;
}

export interface SessionUpdate {
  sessionId: string;
  session: AiSession;
}

@Injectable({ providedIn: 'root' })
export class RelayTerminalService {
  private static readonly INPUT_FLUSH_MS = 200;
  private readonly api = inject(JohnnyApiService);
  private readonly zone = inject(NgZone);
  private readonly screenSubject = new Subject<TerminalScreen>();
  private readonly agentPlanRunSubject = new Subject<AgentPlanRunUpdate>();
  private readonly sessionUpdatedSubject = new Subject<SessionUpdate>();
  private readonly sessionDeletedSubject = new Subject<string>();
  private socket: WebSocket | null = null;
  private connectedNodeId: string | null = null;
  private pendingInput = new Map<string, string>();
  private pendingTimers = new Map<string, ReturnType<typeof setTimeout>>();
  private visualSubscriptions = new Map<string, number>();
  private reconnectTimer: ReturnType<typeof setTimeout> | null = null;

  screens(): Observable<TerminalScreen> {
    return this.screenSubject.asObservable();
  }

  agentPlanRuns(): Observable<AgentPlanRunUpdate> {
    return this.agentPlanRunSubject.asObservable();
  }

  sessionUpdates(): Observable<SessionUpdate> {
    return this.sessionUpdatedSubject.asObservable();
  }

  sessionDeletes(): Observable<string> {
    return this.sessionDeletedSubject.asObservable();
  }

  async connect(): Promise<void> {
    await this.ensureConnected();
  }

  async attach(sessionId: string): Promise<void> {
    await this.refreshVisual(sessionId);
  }

  async subscribeVisual(sessionId: string): Promise<void> {
    const current = this.visualSubscriptions.get(sessionId) ?? 0;
    if (current > 0) {
      this.visualSubscriptions.set(sessionId, current + 1);
      return;
    }

    await this.sendVisualControl('visual_subscribe', sessionId);
    this.visualSubscriptions.set(sessionId, 1);
  }

  async refreshVisual(sessionId: string): Promise<void> {
    if (!this.visualSubscriptions.has(sessionId)) {
      await this.subscribeVisual(sessionId);
      return;
    }

    await this.sendVisualControl('visual_refresh', sessionId);
  }

  async unsubscribeVisual(sessionId: string): Promise<void> {
    const current = this.visualSubscriptions.get(sessionId) ?? 0;
    if (current <= 1) {
      this.visualSubscriptions.delete(sessionId);
      await this.sendVisualControl('visual_unsubscribe', sessionId);
      return;
    }

    this.visualSubscriptions.set(sessionId, current - 1);
  }

  async sendInput(sessionId: string, data: string): Promise<void> {
    if (this.shouldFlushImmediately(data)) {
      await this.flushInput(sessionId);
      await this.sendInputNow(sessionId, data);
      return;
    }

    this.pendingInput.set(sessionId, `${this.pendingInput.get(sessionId) ?? ''}${data}`);
    if (this.pendingTimers.has(sessionId)) return;

    const timer = setTimeout(() => {
      this.pendingTimers.delete(sessionId);
      void this.flushInput(sessionId);
    }, RelayTerminalService.INPUT_FLUSH_MS);
    this.pendingTimers.set(sessionId, timer);
  }

  async sendInputWithAttachments(
    sessionId: string,
    data: string,
    attachments: TerminalInputAttachment[],
  ): Promise<void> {
    await this.flushInput(sessionId);
    await this.sendInputNow(sessionId, data, attachments);
  }

  async flushInput(sessionId: string): Promise<void> {
    const data = this.pendingInput.get(sessionId);
    if (!data) return;

    this.pendingInput.delete(sessionId);
    const timer = this.pendingTimers.get(sessionId);
    if (timer) clearTimeout(timer);
    this.pendingTimers.delete(sessionId);
    await this.sendInputNow(sessionId, data);
  }

  private async sendInputNow(
    sessionId: string,
    data: string,
    attachments: TerminalInputAttachment[] = [],
  ): Promise<void> {
    await this.ensureConnected();

    const socket = this.socket;
    if (!socket || socket.readyState !== WebSocket.OPEN) {
      throw new Error('Relay terminal socket is not connected');
    }

    socket.send(JSON.stringify({
      type: 'terminal_command',
      data: {
        requestId: crypto.randomUUID(),
        sessionId,
        data,
        attachments,
      },
    }));
  }

  async resize(sessionId: string, cols: number, rows: number): Promise<void> {
    await this.ensureConnected();

    const socket = this.socket;
    if (!socket || socket.readyState !== WebSocket.OPEN) {
      throw new Error('Relay terminal socket is not connected');
    }

    socket.send(JSON.stringify({
      type: 'terminal_resize',
      data: {
        requestId: crypto.randomUUID(),
        sessionId,
        cols: Math.max(20, Math.floor(cols)),
        rows: Math.max(5, Math.floor(rows)),
      },
    }));
  }

  async kill(sessionId: string): Promise<void> {
    await this.sendControl('terminal_kill', sessionId);
  }

  disconnect(): void {
    for (const timer of this.pendingTimers.values()) {
      clearTimeout(timer);
    }
    this.pendingInput.clear();
    this.pendingTimers.clear();
    if (this.reconnectTimer) {
      clearTimeout(this.reconnectTimer);
      this.reconnectTimer = null;
    }
    for (const sessionId of this.visualSubscriptions.keys()) {
      this.sendVisualControlIfOpen('visual_unsubscribe', sessionId);
    }
    this.visualSubscriptions.clear();
    this.socket?.close();
    this.socket = null;
    this.connectedNodeId = null;
  }

  private async sendControl(type: 'terminal_visual_subscribe' | 'terminal_visual_unsubscribe' | 'terminal_kill', sessionId: string): Promise<void> {
    await this.ensureConnected();

    const socket = this.socket;
    if (!socket || socket.readyState !== WebSocket.OPEN) {
      throw new Error('Relay terminal socket is not connected');
    }

    socket.send(JSON.stringify({
      type,
      data: {
        requestId: crypto.randomUUID(),
        sessionId,
      },
    }));
  }

  private sendControlIfOpen(type: 'terminal_visual_subscribe' | 'terminal_visual_unsubscribe' | 'terminal_kill', sessionId: string): void {
    const socket = this.socket;
    if (!socket || socket.readyState !== WebSocket.OPEN) return;

    socket.send(JSON.stringify({
      type,
      data: {
        requestId: crypto.randomUUID(),
        sessionId,
      },
    }));
  }

  private async sendVisualControl(control: 'visual_subscribe' | 'visual_unsubscribe' | 'visual_refresh', sessionId: string): Promise<void> {
    await this.ensureConnected();

    const socket = this.socket;
    if (!socket || socket.readyState !== WebSocket.OPEN) {
      throw new Error('Relay terminal socket is not connected');
    }

    socket.send(JSON.stringify({
      type: 'terminal_command',
      data: {
        requestId: crypto.randomUUID(),
        sessionId,
        data: '',
        control,
      },
    }));
  }

  private sendVisualControlIfOpen(control: 'visual_subscribe' | 'visual_unsubscribe', sessionId: string): void {
    const socket = this.socket;
    if (!socket || socket.readyState !== WebSocket.OPEN) return;

    socket.send(JSON.stringify({
      type: 'terminal_command',
      data: {
        requestId: crypto.randomUUID(),
        sessionId,
        data: '',
        control,
      },
    }));
  }

  private shouldFlushImmediately(data: string): boolean {
    if (!data) return true;
    if (data === ' ') return true;
    if (data.includes('\r') || data.includes('\n')) return true;
    if (data.includes('\u001b')) return true;
    if (data.length > 1) return true;
    const code = data.charCodeAt(0);
    return code < 32 || code === 127;
  }

  private async ensureConnected(): Promise<void> {
    if (this.socket?.readyState === WebSocket.OPEN) return;

    const node = await this.findOnlineNode();
    const url = this.relayWsUrl(node.id);

    await new Promise<void>((resolve, reject) => {
      const socket = new WebSocket(url);
      const timeout = window.setTimeout(() => {
        socket.close();
        reject(new Error('Timed out connecting to relay terminal socket'));
      }, 10_000);

      socket.onopen = () => {
        window.clearTimeout(timeout);
        this.socket = socket;
        this.connectedNodeId = node.id;
        void this.replayVisualSubscriptions();
        resolve();
      };

      socket.onmessage = (event) => this.handleMessage(event.data);
      socket.onerror = () => {
        window.clearTimeout(timeout);
        reject(new Error('Relay terminal socket failed'));
      };
      socket.onclose = () => {
        if (this.socket === socket) {
          this.socket = null;
          this.connectedNodeId = null;
        }
        this.scheduleReconnectForVisualSubscriptions();
      };
    });
  }

  private async replayVisualSubscriptions(): Promise<void> {
    const socket = this.socket;
    if (!socket || socket.readyState !== WebSocket.OPEN) return;

    for (const sessionId of this.visualSubscriptions.keys()) {
      this.sendVisualControlIfOpen('visual_subscribe', sessionId);
    }
  }

  private scheduleReconnectForVisualSubscriptions(): void {
    if (this.visualSubscriptions.size === 0 || this.reconnectTimer) return;

    this.reconnectTimer = setTimeout(() => {
      this.reconnectTimer = null;
      if (this.visualSubscriptions.size === 0) return;
      void this.ensureConnected().catch((error) => {
        console.error('Failed to reconnect relay terminal socket:', error);
        this.scheduleReconnectForVisualSubscriptions();
      });
    }, 1_000);
  }

  private async findOnlineNode(): Promise<DesktopNode> {
    const nodes = await firstValueFrom(this.api.listDesktopNodes());
    const node = nodes.find((item) => item.status === 'online') ?? nodes[0];
    if (!node) {
      throw new Error('No backend app is online');
    }
    return node;
  }

  private handleMessage(raw: string): void {
    const envelope = JSON.parse(raw) as RelayEnvelope;

    if (envelope.type === 'terminal_screen') {
      this.zone.run(() => this.screenSubject.next(envelope.data as TerminalScreen));
      return;
    }

    if (envelope.type === 'terminal_command_ack') {
      const ack = envelope.data as TerminalCommandAck;
      if (!ack.accepted) {
        console.error('Terminal command was rejected:', ack.error);
      }
    }

    if (envelope.type === 'agent_plan_run_updated') {
      this.zone.run(() => this.agentPlanRunSubject.next(envelope.data as AgentPlanRunUpdate));
    }

    if (envelope.type === 'session_updated') {
      const data = envelope.data as { sessionId?: string; session?: Record<string, unknown> };
      if (data.sessionId && data.session) {
        const sessionId = data.sessionId;
        const session = data.session;
        this.zone.run(() => this.sessionUpdatedSubject.next({
          sessionId,
          session: this.normalizeSession(session),
        }));
      }
    }

    if (envelope.type === 'session_deleted') {
      const data = envelope.data as { sessionId?: string };
      if (data.sessionId) {
        const sessionId = data.sessionId;
        this.zone.run(() => this.sessionDeletedSubject.next(sessionId));
      }
    }
  }

  private normalizeSession(session: Record<string, unknown>): AiSession {
    return {
      id: String(session['id'] ?? ''),
      title: String(session['title'] ?? ''),
      provider: String(session['provider'] ?? ''),
      model: String(session['model'] ?? ''),
      workingDirectory: String(session['workingDirectory'] ?? session['working_directory'] ?? ''),
      status: (session['status'] as AiSession['status']) ?? 'active',
      totalInputTokens: Number(session['totalInputTokens'] ?? session['total_input_tokens'] ?? 0),
      totalOutputTokens: Number(session['totalOutputTokens'] ?? session['total_output_tokens'] ?? 0),
      totalCostCents: Number(session['totalCostCents'] ?? session['total_cost_cents'] ?? 0),
      createdAt: String(session['createdAt'] ?? session['created_at'] ?? ''),
      updatedAt: String(session['updatedAt'] ?? session['updated_at'] ?? ''),
    };
  }

  private relayWsUrl(nodeId: string): string {
    const base = this.workerBaseUrl();
    const url = new URL('/api/relay/ws', base);
    url.searchParams.set('nodeId', nodeId);
    url.searchParams.set('clientType', 'mobile');
    return url.toString().replace(/^http:/, 'ws:').replace(/^https:/, 'wss:');
  }

  private workerBaseUrl(): string {
    // Mirror the env-detection in web/src/main.ts so the WS endpoint always
    // matches the GraphQL endpoint that the rest of the app is talking to.
    const stored = window.localStorage.getItem('johnnyone_worker_url')?.trim();
    if (stored) return stored;

    if (!['localhost', '127.0.0.1'].includes(window.location.hostname)) {
      return window.location.hostname.endsWith('.pages.dev')
        ? 'https://johnnyone-dev-hub.ethan-353.workers.dev'
        : window.location.origin;
    }

    return 'http://127.0.0.1:7714';
  }
}
