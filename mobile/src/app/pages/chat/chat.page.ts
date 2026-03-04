import {
  Component,
  inject,
  signal,
  computed,
  effect,
  ViewChild,
  OnInit,
  OnDestroy,
  AfterViewInit,
} from '@angular/core';
import { CommonModule } from '@angular/common';
import { FormsModule } from '@angular/forms';
import {
  IonHeader,
  IonToolbar,
  IonTitle,
  IonContent,
  IonButtons,
  IonButton,
  IonIcon,
  IonChip,
  IonLabel,
  IonRefresher,
  IonRefresherContent,
} from '@ionic/angular/standalone';
import {
  JohnnyApiService,
  AiSession,
  AiMessage,
  SessionListComponent,
  MessageBubbleComponent,
  MessageComposerComponent,
} from '@johnnyone/ui';

interface RelayMessage {
  id: string;
  role: 'user' | 'assistant';
  content: string;
  createdAt: string;
}

interface RelayEnvelope {
  type: string;
  data?: {
    relayId?: string;
    sessionId?: string;
    delta?: string;
    chunkType?: string;
    isFinal?: boolean;
    role?: string;
    content?: string;
    messageId?: string;
  };
}

@Component({
  selector: 'app-chat',
  standalone: true,
  imports: [
    CommonModule,
    FormsModule,
    IonHeader,
    IonToolbar,
    IonTitle,
    IonContent,
    IonButtons,
    IonButton,
    IonIcon,
    IonChip,
    IonLabel,
    IonRefresher,
    IonRefresherContent,
    SessionListComponent,
    MessageBubbleComponent,
    MessageComposerComponent,
  ],
  templateUrl: './chat.page.html',
  styleUrls: ['./chat.page.scss'],
})
export class ChatPage implements OnInit, OnDestroy, AfterViewInit {
  private static readonly ACTIVE_SESSION_KEY = 'johnnyone_mobile_active_session';
  private readonly api = inject(JohnnyApiService);
  private relayWs: WebSocket | null = null;
  private onlineNodeId: string | null = null;
  private streamingTimeout: ReturnType<typeof setTimeout> | null = null;
  @ViewChild('chatContent') private chatContent?: IonContent;
  private chatScrollEl: HTMLElement | null = null;
  private shouldAutoScroll = true;
  private autoScrollScheduled = false;
  private preferNewSession = false;
  private initializedSessionView = false;

  /** View state: 'sessions' shows session list, 'chat' shows messages */
  view = signal<'sessions' | 'chat'>('sessions');

  sessions = signal<AiSession[]>([]);
  activeSession = signal<AiSession | null>(null);
  messages = signal<RelayMessage[]>([]);
  isStreaming = signal(false);
  streamingContent = signal('');
  currentMessage = '';
  desktopOnline = signal(false);
  currentRelayId = signal<string | null>(null);
  loading = signal(false);

  aiMessages = computed<AiMessage[]>(() => {
    const msgs = this.messages().map(m => this.mapRelayMessage(m));
    const content = this.streamingContent();
    if (this.isStreaming() && content && msgs.length > 0) {
      const last = msgs[msgs.length - 1];
      if (last.role === 'assistant') {
        return [...msgs.slice(0, -1), { ...last, content }];
      }
    }
    return msgs;
  });

  // Keep viewport pinned while user stays near the bottom.
  private readonly autoScrollEffect = effect(() => {
    if (this.view() !== 'chat') return;
    this.aiMessages();
    this.streamingContent();
    if (this.shouldAutoScroll) {
      this.scheduleAutoScroll();
    }
  });

  ngOnInit(): void {
    this.checkDesktopStatus();
  }

  ngAfterViewInit(): void {
    this.chatContent?.getScrollElement().then((el) => {
      this.chatScrollEl = el;
      this.scheduleAutoScroll();
    });
    this.scheduleAutoScroll();
  }

  ngOnDestroy(): void {
    this.clearStreamingTimeout();
    this.disconnectRelayWs();
  }

  // ── Type Mapper ────────────────────────────────────────────────────

  private mapRelayMessage(m: RelayMessage): AiMessage {
    return {
      id: m.id,
      sessionId: '',
      role: m.role,
      content: m.content,
      inputTokens: 0,
      outputTokens: 0,
      costCents: 0,
      createdAt: m.createdAt,
    };
  }

  // ── Event Handlers (from shared components) ────────────────────────

  onSessionSelected(id: string): void {
    const session = this.sessions().find(s => s.id === id);
    if (session) this.selectSession(session);
  }

  onSessionDeleted(id: string): void {
    this.api.deleteSession(id).subscribe({
      next: () => {
        this.sessions.update(s => s.filter(item => item.id !== id));
        if (this.activeSession()?.id === id) {
          this.backToSessions();
        }
      },
      error: (err) => console.error('Failed to delete session:', err),
    });
  }

  onMessageSent(text: string): void {
    this.shouldAutoScroll = true;
    this.currentMessage = text;
    this.sendMessage();
  }

  onContentScroll(): void {
    if (this.view() !== 'chat') return;
    const el = this.chatScrollEl;
    if (!el) return;
    const threshold = 96;
    this.shouldAutoScroll = el.scrollTop + el.clientHeight >= el.scrollHeight - threshold;
  }

  // ── Desktop Status & WebSocket ─────────────────────────────────────

  async checkDesktopStatus(): Promise<void> {
    this.api.listDesktopNodes().subscribe({
      next: (nodes) => {
        const onlineNode = nodes.find((n) => n.status === 'online');
        this.desktopOnline.set(!!onlineNode);
        if (onlineNode) {
          this.onlineNodeId = onlineNode.id;
          this.connectRelayWs(onlineNode.id);
          this.loadSessions();
        }
      },
      error: () => this.desktopOnline.set(false),
    });
  }

  /** Connect to the ChatRelayDO WebSocket as a mobile client */
  private connectRelayWs(nodeId: string): void {
    this.disconnectRelayWs();

    const proto = location.protocol === 'https:' ? 'wss:' : 'ws:';
    const url = `${proto}//${location.host}/api/relay/ws?nodeId=${nodeId}&clientType=mobile&userId=00000000-0000-0000-0000-000000000002&tenantId=00000000-0000-0000-0000-000000000001`;

    this.relayWs = new WebSocket(url);

    this.relayWs.onmessage = (event) => {
      try {
        const envelope = JSON.parse(event.data) as RelayEnvelope;
        this.handleRelayMessage(envelope);
      } catch {
        // ignore malformed messages
      }
    };

    this.relayWs.onclose = () => {
      if (this.isStreaming()) {
        this.clearStreamingTimeout();
        this.isStreaming.set(false);
        this.streamingContent.set('');
      }

      if (this.desktopOnline() && this.onlineNodeId) {
        setTimeout(() => {
          if (this.onlineNodeId) this.connectRelayWs(this.onlineNodeId);
        }, 2000);
      }
    };
  }

  private disconnectRelayWs(): void {
    if (this.relayWs) {
      this.relayWs.onclose = null;
      this.relayWs.close();
      this.relayWs = null;
    }
  }

  private handleRelayMessage(envelope: RelayEnvelope): void {
    if (envelope.type === 'session_deleted') {
      const sessionId = (envelope.data as { sessionId?: string })?.sessionId;
      if (sessionId) {
        this.sessions.update((s) => s.filter((item) => item.id !== sessionId));
        if (this.activeSession()?.id === sessionId) {
          this.backToSessions();
        }
      }
      return;
    }

    const data = envelope.data;
    if (!data) return;

    switch (envelope.type) {
      case 'chat_delta': {
        if (data.chunkType === 'text' && data.delta) {
          this.bumpStreamingTimeout();
          this.streamingContent.update((c) => c + data.delta);

          const msgs = this.messages();
          const last = msgs[msgs.length - 1];
          if (!last || last.role !== 'assistant' || !this.isStreaming()) {
            this.messages.update((m) => [
              ...m,
              {
                id: data.sessionId ?? crypto.randomUUID(),
                role: 'assistant' as const,
                content: '',
                createdAt: new Date().toISOString(),
              },
            ]);
          }
        }

        if (data.isFinal) {
          this.finishStreaming();
        }
        break;
      }

      case 'chat_complete': {
        this.finishStreaming();
        break;
      }

      case 'chat_message': {
        if (data.role === 'assistant' && data.content) {
          this.messages.update((msgs) => {
            const updated = [...msgs];
            const lastIdx = updated.length - 1;
            if (lastIdx >= 0 && updated[lastIdx].role === 'assistant' && !updated[lastIdx].content) {
              updated[lastIdx] = { ...updated[lastIdx], content: data.content! };
            } else {
              updated.push({
                id: crypto.randomUUID(),
                role: 'assistant',
                content: data.content!,
                createdAt: new Date().toISOString(),
              });
            }
            return updated;
          });
          this.isStreaming.set(false);
          this.streamingContent.set('');
          this.clearStreamingTimeout();
        }
        break;
      }
    }
  }

  private finishStreaming(): void {
    const content = this.streamingContent();
    if (content) {
      this.messages.update((msgs) => {
        const updated = [...msgs];
        const lastIdx = updated.length - 1;
        if (lastIdx >= 0 && updated[lastIdx].role === 'assistant') {
          updated[lastIdx] = { ...updated[lastIdx], content };
        }
        return updated;
      });
    }
    this.isStreaming.set(false);
    this.streamingContent.set('');
    this.clearStreamingTimeout();
  }

  // ── Sessions ───────────────────────────────────────────────────────

  loadSessions(): void {
    this.loading.set(true);
    this.api.listSessions().subscribe({
      next: (sessions) => {
        this.sessions.set(sessions);
        const preferredSession = this.syncActiveSession(sessions);

        // On first load, behave like desktop and reopen the latest/preferred session.
        if (!this.initializedSessionView && preferredSession && !this.preferNewSession) {
          this.initializedSessionView = true;
          this.selectSession(preferredSession);
          return;
        }

        if (!this.initializedSessionView) {
          this.initializedSessionView = true;
        }

        this.loading.set(false);
      },
      error: (err) => {
        console.error('Failed to load sessions:', err);
        this.loading.set(false);
      },
    });
  }

  selectSession(session: AiSession): void {
    this.activeSession.set(session);
    this.setStoredActiveSessionId(session.id);
    this.preferNewSession = false;
    this.view.set('chat');
    this.messages.set([]);
    this.shouldAutoScroll = true;
    this.loading.set(true);

    this.api.listMessages(session.id).subscribe({
      next: (msgs) => {
        const relayMsgs: RelayMessage[] = msgs
          .filter((m) => m.role === 'user' || m.role === 'assistant')
          .map((m) => ({
            id: m.id,
            role: m.role as 'user' | 'assistant',
            content: m.content,
            createdAt: m.createdAt,
          }));
        this.messages.set(relayMsgs);
        this.loading.set(false);
      },
      error: (err) => {
        console.error('Failed to load messages:', err);
        this.loading.set(false);
      },
    });
  }

  backToSessions(): void {
    this.view.set('sessions');
    this.activeSession.set(null);
    this.messages.set([]);
    this.loadSessions();
  }

  startNewChat(): void {
    const sessionId = crypto.randomUUID();
    localStorage.setItem('johnnyone_mobile_session', sessionId);
    this.activeSession.set(null);
    this.setStoredActiveSessionId(null);
    this.preferNewSession = true;
    this.messages.set([]);
    this.view.set('chat');
    this.shouldAutoScroll = true;
  }

  onRefresh(event: CustomEvent): void {
    this.checkDesktopStatus();
    if (this.view() === 'sessions') {
      this.loadSessions();
    }
    const refresher = event.target as HTMLIonRefresherElement;
    setTimeout(() => refresher.complete(), 1000);
  }

  // ── Chat ───────────────────────────────────────────────────────────

  async sendMessage(): Promise<void> {
    const text = this.currentMessage.trim();
    if (!text || this.isStreaming()) return;

    const userMsg: RelayMessage = {
      id: crypto.randomUUID(),
      role: 'user',
      content: text,
      createdAt: new Date().toISOString(),
    };
    this.messages.update((msgs) => [...msgs, userMsg]);
    this.currentMessage = '';
    this.shouldAutoScroll = true;
    this.isStreaming.set(true);
    this.streamingContent.set('');
    this.bumpStreamingTimeout();

    let sessionId = this.activeSession()?.id;
    if (!sessionId && !this.preferNewSession) {
      const existing = this.sessions()[0];
      if (existing) {
        sessionId = existing.id;
        this.activeSession.set(existing);
        this.setStoredActiveSessionId(existing.id);
      }
    }
    sessionId = sessionId ?? this.getOrCreateSessionId();
    this.preferNewSession = false;

    const active = this.activeSession();
    const relayInput: {
      sessionId: string;
      content: string;
      provider?: string;
      model?: string;
      workingDirectory?: string;
    } = {
      sessionId,
      content: text,
      provider: active?.provider || undefined,
      model: active?.model || undefined,
      workingDirectory: active?.workingDirectory || undefined,
    };

    this.api
      .sendRelayMessage(relayInput)
      .subscribe({
        next: (result) => {
          this.currentRelayId.set(result.relayId);
        },
        error: (err) => {
          console.error('Failed to relay message:', err);
          this.clearStreamingTimeout();
          this.isStreaming.set(false);
          this.messages.update((msgs) => [
            ...msgs,
            {
              id: crypto.randomUUID(),
              role: 'assistant' as const,
              content: 'Failed to reach desktop. Is it running?',
              createdAt: new Date().toISOString(),
            },
          ]);
        },
      });
  }

  private bumpStreamingTimeout(): void {
    this.clearStreamingTimeout();
    this.streamingTimeout = setTimeout(() => {
      if (!this.isStreaming()) return;
      this.isStreaming.set(false);
      this.streamingContent.set('');
      this.messages.update((msgs) => [
        ...msgs,
        {
          id: crypto.randomUUID(),
          role: 'assistant',
          content: 'Response timed out. Please try again.',
          createdAt: new Date().toISOString(),
        },
      ]);
    }, 45000);
  }

  private clearStreamingTimeout(): void {
    if (this.streamingTimeout) {
      clearTimeout(this.streamingTimeout);
      this.streamingTimeout = null;
    }
  }

  private scheduleAutoScroll(): void {
    if (this.autoScrollScheduled) return;
    this.autoScrollScheduled = true;

    requestAnimationFrame(() => {
      this.autoScrollScheduled = false;
      const el = this.chatScrollEl;
      if (!el || !this.shouldAutoScroll || this.view() !== 'chat') return;
      el.scrollTop = el.scrollHeight;
    });
  }

  private syncActiveSession(sessions: AiSession[]): AiSession | null {
    const current = this.activeSession();
    if (current) {
      const refreshed = sessions.find((s) => s.id === current.id);
      if (refreshed) {
        this.activeSession.set(refreshed);
        return refreshed;
      }
    }

    const storedId = this.getStoredActiveSessionId();
    if (storedId) {
      const stored = sessions.find((s) => s.id === storedId);
      if (stored) {
        this.activeSession.set(stored);
        return stored;
      }
    }

    if (!this.preferNewSession && sessions.length > 0) {
      this.activeSession.set(sessions[0]);
      this.setStoredActiveSessionId(sessions[0].id);
      return sessions[0];
    }

    return null;
  }

  private getStoredActiveSessionId(): string | null {
    return localStorage.getItem(ChatPage.ACTIVE_SESSION_KEY);
  }

  private setStoredActiveSessionId(id: string | null): void {
    if (id) {
      localStorage.setItem(ChatPage.ACTIVE_SESSION_KEY, id);
    } else {
      localStorage.removeItem(ChatPage.ACTIVE_SESSION_KEY);
    }
  }

  private getOrCreateSessionId(): string {
    const key = 'johnnyone_mobile_session';
    let sessionId = localStorage.getItem(key);
    if (!sessionId) {
      sessionId = crypto.randomUUID();
      localStorage.setItem(key, sessionId);
    }
    return sessionId;
  }
}
