import { Injectable, inject } from '@angular/core';
import { Observable, Subject, firstValueFrom } from 'rxjs';
import { AgentPlanRun, DesktopNode, JohnnyApiService, TerminalScreen } from '@johnnyone/ui';

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

export interface AgentPlanRunUpdate {
  planId: string;
  deleted: boolean;
  run?: AgentPlanRun;
}

@Injectable({ providedIn: 'root' })
export class RelayTerminalService {
  private static readonly INPUT_FLUSH_MS = 200;
  private readonly api = inject(JohnnyApiService);
  private readonly screenSubject = new Subject<TerminalScreen>();
  private readonly agentPlanRunSubject = new Subject<AgentPlanRunUpdate>();
  private socket: WebSocket | null = null;
  private connectedNodeId: string | null = null;
  private pendingInput = new Map<string, string>();
  private pendingTimers = new Map<string, ReturnType<typeof setTimeout>>();

  screens(): Observable<TerminalScreen> {
    return this.screenSubject.asObservable();
  }

  agentPlanRuns(): Observable<AgentPlanRunUpdate> {
    return this.agentPlanRunSubject.asObservable();
  }

  async connect(): Promise<void> {
    await this.ensureConnected();
  }

  async attach(sessionId: string): Promise<void> {
    await this.ensureConnected();
    await this.sendInputNow(sessionId, '');
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

  async flushInput(sessionId: string): Promise<void> {
    const data = this.pendingInput.get(sessionId);
    if (!data) return;

    this.pendingInput.delete(sessionId);
    const timer = this.pendingTimers.get(sessionId);
    if (timer) clearTimeout(timer);
    this.pendingTimers.delete(sessionId);
    await this.sendInputNow(sessionId, data);
  }

  private async sendInputNow(sessionId: string, data: string): Promise<void> {
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
    await this.ensureConnected();

    const socket = this.socket;
    if (!socket || socket.readyState !== WebSocket.OPEN) {
      throw new Error('Relay terminal socket is not connected');
    }

    socket.send(JSON.stringify({
      type: 'terminal_kill',
      data: {
        requestId: crypto.randomUUID(),
        sessionId,
      },
    }));
  }

  disconnect(): void {
    for (const timer of this.pendingTimers.values()) {
      clearTimeout(timer);
    }
    this.pendingInput.clear();
    this.pendingTimers.clear();
    this.socket?.close();
    this.socket = null;
    this.connectedNodeId = null;
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
      };
    });
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
      this.screenSubject.next(envelope.data as TerminalScreen);
      return;
    }

    if (envelope.type === 'terminal_command_ack') {
      const ack = envelope.data as TerminalCommandAck;
      if (!ack.accepted) {
        console.error('Terminal command was rejected:', ack.error);
      }
    }

    if (envelope.type === 'agent_plan_run_updated') {
      this.agentPlanRunSubject.next(envelope.data as AgentPlanRunUpdate);
    }
  }

  private relayWsUrl(nodeId: string): string {
    const base = this.workerBaseUrl();
    const url = new URL('/api/relay/ws', base);
    url.searchParams.set('nodeId', nodeId);
    url.searchParams.set('clientType', 'mobile');
    return url.toString().replace(/^http:/, 'ws:').replace(/^https:/, 'wss:');
  }

  private workerBaseUrl(): string {
    const stored = window.localStorage.getItem('johnnyone_worker_url')?.trim();
    if (stored) return stored;

    if (!['localhost', '127.0.0.1'].includes(window.location.hostname)) {
      return window.location.hostname.endsWith('.pages.dev')
        ? 'https://johnnyone-dev-johnnyone-hub.cf-static-5f5.workers.dev'
        : window.location.origin;
    }

    return 'http://127.0.0.1:7714';
  }
}
