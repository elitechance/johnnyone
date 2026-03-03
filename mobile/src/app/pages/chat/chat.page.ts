import { Component, inject, signal, OnInit, OnDestroy } from '@angular/core';
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
  IonList,
  IonItem,
  IonNote,
  IonItemSliding,
  IonItemOptions,
  IonItemOption,
} from '@ionic/angular/standalone';
import {
  JohnnyApiService,
  AiSession,
  DesktopNode,
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
    IonList,
    IonItem,
    IonNote,
    IonItemSliding,
    IonItemOptions,
    IonItemOption,
  ],
  templateUrl: './chat.page.html',
  styleUrls: ['./chat.page.scss'],
})
export class ChatPage implements OnInit, OnDestroy {
  private readonly api = inject(JohnnyApiService);
  private relayWs: WebSocket | null = null;
  private onlineNodeId: string | null = null;

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

  ngOnInit(): void {
    this.checkDesktopStatus();
  }

  ngOnDestroy(): void {
    this.disconnectRelayWs();
  }

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
      // Reconnect after a short delay if desktop is still online
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
    const data = envelope.data;
    if (!data) return;

    switch (envelope.type) {
      case 'chat_delta': {
        if (data.chunkType === 'text' && data.delta) {
          this.streamingContent.update((c) => c + data.delta);

          // Ensure we have an assistant message placeholder
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
          // Full message received — replace streaming content
          this.messages.update((msgs) => {
            const updated = [...msgs];
            const lastIdx = updated.length - 1;
            if (lastIdx >= 0 && updated[lastIdx].role === 'assistant' && !updated[lastIdx].content) {
              updated[lastIdx] = {
                ...updated[lastIdx],
                content: data.content!,
              };
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
        }
        break;
      }
    }
  }

  private finishStreaming(): void {
    const content = this.streamingContent();
    if (content) {
      // Update the last assistant message with accumulated content
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
  }

  loadSessions(): void {
    this.loading.set(true);
    this.api.listSessions().subscribe({
      next: (sessions) => {
        this.sessions.set(sessions);
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
    this.view.set('chat');
    this.messages.set([]);
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

  deleteSession(event: Event, session: AiSession): void {
    event.stopPropagation();
    this.api.deleteSession(session.id).subscribe({
      next: () => {
        this.sessions.update((s) => s.filter((item) => item.id !== session.id));
      },
      error: (err) => {
        console.error('Failed to delete session:', err);
      },
    });
  }

  backToSessions(): void {
    this.view.set('sessions');
    this.activeSession.set(null);
    this.messages.set([]);
    this.loadSessions();
  }

  async sendMessage(): Promise<void> {
    const text = this.currentMessage.trim();
    if (!text || this.isStreaming()) return;

    // Add user message to local list
    const userMsg: RelayMessage = {
      id: crypto.randomUUID(),
      role: 'user',
      content: text,
      createdAt: new Date().toISOString(),
    };
    this.messages.update((msgs) => [...msgs, userMsg]);
    this.currentMessage = '';
    this.isStreaming.set(true);
    this.streamingContent.set('');

    // Send via relay
    const sessionId = this.activeSession()?.id ?? this.getOrCreateSessionId();

    this.api
      .sendRelayMessage({ sessionId, content: text })
      .subscribe({
        next: (result) => {
          this.currentRelayId.set(result.relayId);
        },
        error: (err) => {
          console.error('Failed to relay message:', err);
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

  startNewChat(): void {
    const sessionId = crypto.randomUUID();
    localStorage.setItem('johnnyone_mobile_session', sessionId);
    this.activeSession.set(null);
    this.messages.set([]);
    this.view.set('chat');
  }

  onRefresh(event: CustomEvent): void {
    this.checkDesktopStatus();
    if (this.view() === 'sessions') {
      this.loadSessions();
    }
    const refresher = event.target as HTMLIonRefresherElement;
    setTimeout(() => refresher.complete(), 1000);
  }

  onKeyDown(event: KeyboardEvent): void {
    if (event.key === 'Enter' && !event.shiftKey) {
      event.preventDefault();
      this.sendMessage();
    }
  }

  getDisplayContent(msg: RelayMessage): string {
    if (msg.role === 'assistant' && this.isStreaming() && !msg.content && this.streamingContent()) {
      return this.streamingContent();
    }
    return msg.content;
  }

  formatTime(dateStr: string): string {
    const date = new Date(dateStr);
    return date.toLocaleTimeString([], { hour: '2-digit', minute: '2-digit' });
  }

  formatDate(dateStr: string): string {
    const date = new Date(dateStr);
    return date.toLocaleDateString([], { month: 'short', day: 'numeric' });
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

  trackByMessageId(_index: number, msg: RelayMessage): string {
    return msg.id;
  }

  trackBySessionId(_index: number, session: AiSession): string {
    return session.id;
  }
}
