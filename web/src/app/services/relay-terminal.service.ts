import { Injectable, NgZone, inject } from '@angular/core';
import { Observable, Subject, firstValueFrom } from 'rxjs';
import { AgentPlanRun, AiSession, StreamEvent, TerminalScreen } from '@johnnyone/ui';
import { getWorkerBaseUrl, getWorkerGraphqlUrl } from '../../worker-url';
import { TerminalScreenCacheService } from './terminal-screen-cache.service';
import { AuthService } from './auth.service';

interface RelayEnvelope {
  type: string;
  data?: unknown;
}

const WS_OPEN = 1;

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
  private readonly zone = inject(NgZone);
  private readonly terminalScreenCache = inject(TerminalScreenCacheService);
  private readonly auth = inject(AuthService);
  private readonly screenSubject = new Subject<TerminalScreen>();
  // Structured provider/agent stream events (overhaul P2, D6). Parallel to the terminal screen lane;
  // the transcript renderer that consumes this is Phase 3 — no rendering here.
  private readonly streamEventSubject = new Subject<StreamEvent>();
  private readonly agentPlanRunSubject = new Subject<AgentPlanRunUpdate>();
  private readonly sessionUpdatedSubject = new Subject<SessionUpdate>();
  private readonly sessionDeletedSubject = new Subject<string>();
  private socket: WebSocket | null = null;
  private pendingInput = new Map<string, string>();
  private pendingTimers = new Map<string, ReturnType<typeof setTimeout>>();
  private visualSubscriptions = new Map<string, number>();
  private streamSubscriptions = new Map<string, number>();
  private reconnectTimer: ReturnType<typeof setTimeout> | null = null;
  private readonly errorSubject = new Subject<string>();
  private authFailureCount = 0;
  private connectAttemptEverOpened = false;
  private justRefreshedForReconnect = false; // to dedupe double refresh on reconnect path
  private lastDisconnectWasAuthRejection = false; // per Task 04 decision #2: branch refresh/reconnect for auth vs transient

  screens(): Observable<TerminalScreen> {
    return this.screenSubject.asObservable();
  }

  /** Structured stream events (overhaul P2, D6). Callers filter by `sessionId` as needed, mirroring
   *  how `screens()` is consumed. */
  streamEvents(): Observable<StreamEvent> {
    return this.streamEventSubject.asObservable();
  }

  /** Per-session stream-event subscription. Ref-counted + replayed on reconnect, exactly like
   *  `subscribeVisual`. Sends the dedicated `stream_subscribe` envelope the DO forwards to desktop
   *  through the same ownership gate as the terminal visual controls. */
  async subscribeStream(sessionId: string): Promise<void> {
    const current = this.streamSubscriptions.get(sessionId) ?? 0;
    if (current > 0) {
      this.streamSubscriptions.set(sessionId, current + 1);
      return;
    }
    await this.sendStreamControl('stream_subscribe', sessionId);
    this.streamSubscriptions.set(sessionId, 1);
  }

  async unsubscribeStream(sessionId: string): Promise<void> {
    const current = this.streamSubscriptions.get(sessionId) ?? 0;
    if (current <= 1) {
      this.streamSubscriptions.delete(sessionId);
      await this.sendStreamControl('stream_unsubscribe', sessionId);
      return;
    }
    this.streamSubscriptions.set(sessionId, current - 1);
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
    }

    await this.sendVisualControl('visual_refresh', sessionId);
  }

  async loadHistory(sessionId: string, rows: number): Promise<void> {
    await this.sendVisualControl(
      'visual_history',
      sessionId,
      Math.max(1, Math.floor(rows)),
    );
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
    if (!socket || socket.readyState !== WS_OPEN) {
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
    if (!socket || socket.readyState !== WS_OPEN) {
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

  cachedScreen(sessionId: string): TerminalScreen | null {
    return this.terminalScreenCache.get(sessionId);
  }

  forgetCachedScreen(sessionId: string): void {
    this.terminalScreenCache.remove(sessionId);
  }

  retainCachedScreens(activeSessionIds: Iterable<string>): void {
    this.terminalScreenCache.retainOnly(activeSessionIds);
  }

  disconnect(): void {
    this.terminalScreenCache.flush();
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
    for (const sessionId of this.streamSubscriptions.keys()) {
      this.sendStreamControlIfOpen('stream_unsubscribe', sessionId);
    }
    this.streamSubscriptions.clear();
    this.socket?.close();
    this.socket = null;
    this.connectAttemptEverOpened = false;
    this.lastDisconnectWasAuthRejection = false;
    this.justRefreshedForReconnect = false;
  }

  private async sendControl(type: 'terminal_visual_subscribe' | 'terminal_visual_unsubscribe' | 'terminal_kill', sessionId: string): Promise<void> {
    await this.ensureConnected();

    const socket = this.socket;
    if (!socket || socket.readyState !== WS_OPEN) {
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
    if (!socket || socket.readyState !== WS_OPEN) return;

    socket.send(JSON.stringify({
      type,
      data: {
        requestId: crypto.randomUUID(),
        sessionId,
      },
    }));
  }

  private async sendVisualControl(
    control: 'visual_subscribe' | 'visual_unsubscribe' | 'visual_refresh' | 'visual_history',
    sessionId: string,
    historyRows?: number,
  ): Promise<void> {
    await this.ensureConnected();

    const socket = this.socket;
    if (!socket || socket.readyState !== WS_OPEN) {
      throw new Error('Relay terminal socket is not connected');
    }

    socket.send(JSON.stringify({
      type: 'terminal_command',
      data: {
        requestId: crypto.randomUUID(),
        sessionId,
        data: '',
        control,
        historyRows,
      },
    }));
  }

  private sendVisualControlIfOpen(control: 'visual_subscribe' | 'visual_unsubscribe', sessionId: string): void {
    const socket = this.socket;
    if (!socket || socket.readyState !== WS_OPEN) return;

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

  private async sendStreamControl(
    type: 'stream_subscribe' | 'stream_unsubscribe',
    sessionId: string,
  ): Promise<void> {
    await this.ensureConnected();
    const socket = this.socket;
    if (!socket || socket.readyState !== WS_OPEN) {
      throw new Error('Relay terminal socket is not connected');
    }
    socket.send(JSON.stringify({
      type,
      data: { requestId: crypto.randomUUID(), sessionId },
    }));
  }

  private sendStreamControlIfOpen(type: 'stream_subscribe' | 'stream_unsubscribe', sessionId: string): void {
    const socket = this.socket;
    if (!socket || socket.readyState !== WS_OPEN) return;
    socket.send(JSON.stringify({
      type,
      data: { requestId: crypto.randomUUID(), sessionId },
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
    if (this.socket && this.socket.readyState === WS_OPEN) return;

    // Gate proactive refresh: only if no token OR near expiry (using stored expiresAt).
    // Per Task 04 decisions: reactive-on-rejection is source of truth; proactive only for
    // obviously expired (or absent). Avoids refresh roundtrip + token rotation on every connect.
    // After schedule's failure refresh, token will be present so ensure will not re-refresh (collapses double).
    const apiUrl = getWorkerGraphqlUrl();
    const hadToken = !!this.auth.getAccessToken();
    // Consult AuthService.expiresIn (via stored value + near-expiry helper) per Task 04 decisions.
    // Only refresh proactively when access token missing or near expiry (not on EVERY connect whenever refreshToken exists).
    const expiresIn = this.auth.getExpiresIn();
    const needsRefresh = !hadToken || (expiresIn != null && this.auth.isTokenNearExpiry());
    if (this.justRefreshedForReconnect) {
      this.justRefreshedForReconnect = false;
      // deduped: schedule already did the refresh for this reconnect; do not refresh again
    } else if (needsRefresh && this.auth.getRefreshToken()) {
      try {
        await this.auth.refresh(apiUrl);
      } catch (e) {
        if (!this.auth.getAccessToken()) {
          throw new Error('No valid authentication token. Please log in or refresh.');
        }
        // proceed with prior token if refresh failed but one still present
      }
    } else if (!hadToken) {
      throw new Error('No valid authentication token. Please log in or refresh.');
    }

    // No listDesktopNodes / findOnlineNode in connect path (Task 03): server resolves from JWT.
    // (api.listDesktopNodes still available for any display surfaces outside this service.)
    const url = this.relayWsUrl();

    this.connectAttemptEverOpened = false;

    await new Promise<void>((resolve, reject) => {
      const socket = new WebSocket(url);
      const timeout = window.setTimeout(() => {
        socket.close();
        reject(new Error('Timed out connecting to relay terminal socket'));
      }, 10_000);

      socket.onopen = () => {
        window.clearTimeout(timeout);
        this.socket = socket;
        this.connectAttemptEverOpened = true;
        this.lastDisconnectWasAuthRejection = false;
        this.authFailureCount = 0;
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
        }
        // Implement Task 04 decisions #2: distinguish AUTH rejection (close/error BEFORE onopen)
        // from transient network drop (after successful onopen). Branch reconnect/refresh accordingly.
        this.lastDisconnectWasAuthRejection = !this.connectAttemptEverOpened;
        if (this.lastDisconnectWasAuthRejection) {
          this.authFailureCount = Math.max(this.authFailureCount, 1);
        }
        this.scheduleReconnectForVisualSubscriptions();
      };
    });
  }

  private async replayVisualSubscriptions(): Promise<void> {
    const socket = this.socket;
    if (!socket || socket.readyState !== WS_OPEN) return;

    for (const sessionId of this.visualSubscriptions.keys()) {
      this.sendVisualControlIfOpen('visual_subscribe', sessionId);
    }
    // Replay stream subscriptions on the same reconnect so the transcript lane survives a drop.
    for (const sessionId of this.streamSubscriptions.keys()) {
      this.sendStreamControlIfOpen('stream_subscribe', sessionId);
    }
  }

  errors(): Observable<string> {
    return this.errorSubject.asObservable();
  }

  private scheduleReconnectForVisualSubscriptions(): void {
    if (this.reconnectTimer) return;

    const delay = 1000 * Math.min(this.authFailureCount + 1, 10); // backoff, bound max 10s
    this.reconnectTimer = setTimeout(async () => {
      this.reconnectTimer = null;
      // Reconnect if there is anything to replay — terminal visual OR stream subscriptions.
      if (this.visualSubscriptions.size === 0 && this.streamSubscriptions.size === 0) return;

      try {
        // Branch per Task 04 #2: for AUTH rejection (pre-open), do refresh then ensure.
        // For transient (post-open), just ensure (no forced refresh this cycle).
        if (this.lastDisconnectWasAuthRejection || this.authFailureCount > 0) {
          const apiUrl = getWorkerGraphqlUrl();
          await this.auth.refresh(apiUrl);
          this.authFailureCount = 0;
          this.justRefreshedForReconnect = true; // signal to ensureConnected to skip (dedupe double-refresh)
          this.lastDisconnectWasAuthRejection = false;
        }
        await this.ensureConnected();
      } catch (error) {
        console.error('Failed to reconnect relay terminal socket:', error);
        this.authFailureCount++;
        if (this.authFailureCount > 3) {
          // bounded: stop and surface via existing terminal error UI (no silent dead)
          this.zone.run(() => {
            this.errorSubject.next('Auth failures exceeded bound; not reconnecting further. Please re-login to restore terminal.');
          });
          return;
        }
        this.scheduleReconnectForVisualSubscriptions();
      }
    }, delay);
  }

  private handleMessage(raw: string): void {
    const envelope = JSON.parse(raw) as RelayEnvelope;

    if (envelope.type === 'terminal_screen') {
      const incoming = envelope.data as TerminalScreen;
      // Always render the live frame. Merging cached scrollback here hid new output
      // because viewport snapshots are shorter than a stored history snapshot.
      this.terminalScreenCache.remember(incoming);
      this.zone.run(() => this.screenSubject.next(incoming));
      return;
    }

    if (envelope.type === 'stream_event') {
      this.zone.run(() => this.streamEventSubject.next(envelope.data as StreamEvent));
      return;
    }

    if (envelope.type === 'terminal_command_ack') {
      const ack = envelope.data as TerminalCommandAck | undefined;
      if (ack && !ack.accepted) {
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

  private relayWsUrl(): string {
    const base = this.workerBaseUrl();
    const url = new URL('/api/relay/ws', base);
    // No client-supplied nodeId: server resolves from JWT (Phase 00)
    url.searchParams.set('clientType', 'mobile');
    const token = this.auth.getAccessToken();
    if (token) {
      url.searchParams.set('token', token);
    }
    return url.toString().replace(/^http:/, 'ws:').replace(/^https:/, 'wss:');
  }

  private workerBaseUrl(): string {
    // Keep in sync with web/src/worker-url.ts (legacy hub URLs auto-migrate).
    return getWorkerBaseUrl();
  }
}
