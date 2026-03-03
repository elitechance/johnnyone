import { Component, inject, signal, computed, OnInit, OnDestroy } from '@angular/core';
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
  AiMessage,
  AiMessageDelta,
  DesktopNode,
} from '@johnnyone/ui';
import { Subscription } from 'rxjs';

interface RelayMessage {
  id: string;
  role: 'user' | 'assistant';
  content: string;
  createdAt: string;
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
  ],
  templateUrl: './chat.page.html',
  styleUrls: ['./chat.page.scss'],
})
export class ChatPage implements OnInit, OnDestroy {
  private readonly api = inject(JohnnyApiService);
  private deltaSubscription: Subscription | null = null;

  messages = signal<RelayMessage[]>([]);
  isStreaming = signal(false);
  streamingContent = signal('');
  currentMessage = '';
  desktopOnline = signal(false);
  currentRelayId = signal<string | null>(null);

  ngOnInit(): void {
    this.checkDesktopStatus();
  }

  ngOnDestroy(): void {
    this.deltaSubscription?.unsubscribe();
  }

  async checkDesktopStatus(): Promise<void> {
    this.api.listDesktopNodes().subscribe({
      next: (nodes) => {
        const online = nodes.some((n) => n.status === 'online');
        this.desktopOnline.set(online);
      },
      error: () => this.desktopOnline.set(false),
    });
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
    const sessionId = this.getOrCreateSessionId();

    // TODO: Wire up to relayChatMessage mutation and onRelayChatDelta subscription
    // For now, we prepare the relay structure
    this.api
      .sendMessage({ sessionId, content: text })
      .subscribe({
        next: () => {
          // Message sent via relay — deltas will arrive via subscription
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

  onRefresh(event: CustomEvent): void {
    this.checkDesktopStatus();
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

  private getOrCreateSessionId(): string {
    // Simple session management — use a single session per mobile instance
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
}
