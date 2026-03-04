import { Component, OnInit, OnDestroy, inject, signal, computed } from '@angular/core';
import { CommonModule } from '@angular/common';
import { FormsModule } from '@angular/forms';
import {
  IonButton,
  IonIcon,
} from '@ionic/angular/standalone';
import { Router } from '@angular/router';
import {
  TauriBridgeService,
  Session,
  Message,
  ChatDelta,
  DetectedTool,
} from '../../services/tauri-bridge.service';
import {
  SessionListComponent,
  MessageBubbleComponent,
  MessageComposerComponent,
  AiSession as AiSessionUI,
  AiMessage,
} from '@johnnyone/ui';
import { Subscription } from 'rxjs';

@Component({
  selector: 'app-chat',
  standalone: true,
  imports: [
    CommonModule,
    FormsModule,
    IonButton,
    IonIcon,
    SessionListComponent,
    MessageBubbleComponent,
    MessageComposerComponent,
  ],
  templateUrl: './chat.page.html',
  styleUrls: ['./chat.page.scss'],
})
export class ChatPage implements OnInit, OnDestroy {
  private readonly tauriBridge = inject(TauriBridgeService);
  private readonly router = inject(Router);
  private deltaSubscription: Subscription | null = null;
  private completeSubscription: Subscription | null = null;

  // State
  sessions = signal<Session[]>([]);
  currentSession = signal<Session | null>(null);
  messages = signal<Message[]>([]);
  isStreaming = signal(false);
  streamingContent = signal('');
  streamingMessageId = signal<string | null>(null);
  currentMessage = '';
  searchQuery = '';
  detectedTools = signal<DetectedTool[]>([]);
  selectedProvider = signal('claude_code');
  workingDirectory = signal('');
  sidebarOpen = signal(true);

  // Computed
  filteredSessions = computed(() => {
    const query = this.searchQuery.toLowerCase();
    const all = this.sessions();
    if (!query) return all;
    return all.filter(s => s.title.toLowerCase().includes(query));
  });

  aiSessions = computed<AiSessionUI[]>(() =>
    this.filteredSessions().map(s => this.mapSession(s))
  );

  aiMessages = computed<AiMessage[]>(() => {
    const streamingId = this.streamingMessageId();
    const content = this.streamingContent();
    return this.messages().map(m =>
      m.id === streamingId ? { ...this.mapMessage(m), content } : this.mapMessage(m)
    );
  });

  sessionTokens = computed(() => {
    const session = this.currentSession();
    if (!session) return { input: 0, output: 0, cost: 0 };
    return {
      input: session.total_input_tokens,
      output: session.total_output_tokens,
      cost: session.total_cost_cents / 100,
    };
  });

  ngOnInit(): void {
    this.loadSessions();
    this.detectTools();
    this.subscribeToDelta();
    this.subscribeToComplete();
    this.loadLastWorkingDirectory();
    this.ensureSettingsAndConnect();
  }

  ngOnDestroy(): void {
    this.deltaSubscription?.unsubscribe();
    this.completeSubscription?.unsubscribe();
  }

  // ── Type Mappers ───────────────────────────────────────────────────

  private mapSession(s: Session): AiSessionUI {
    return {
      id: s.id,
      title: s.title,
      provider: s.provider,
      model: s.model,
      workingDirectory: s.working_directory,
      status: s.status as 'active' | 'archived' | 'completed',
      totalInputTokens: s.total_input_tokens,
      totalOutputTokens: s.total_output_tokens,
      totalCostCents: s.total_cost_cents,
      createdAt: s.created_at,
      updatedAt: s.updated_at,
    };
  }

  private mapMessage(m: Message): AiMessage {
    return {
      id: m.id,
      sessionId: m.session_id,
      role: m.role,
      content: m.content,
      toolCalls: m.tool_calls,
      finishReason: m.finish_reason,
      inputTokens: m.input_tokens,
      outputTokens: m.output_tokens,
      costCents: m.cost_cents,
      createdAt: m.created_at,
    };
  }

  // ── Session Management ─────────────────────────────────────────────

  async loadSessions(): Promise<void> {
    try {
      const sessions = await this.tauriBridge.listSessions();
      this.sessions.set(sessions);
    } catch (err) {
      console.error('Failed to load sessions:', err);
    }
  }

  async createNewSession(): Promise<void> {
    try {
      const session = await this.tauriBridge.createSession({
        provider: this.selectedProvider(),
        working_directory: this.workingDirectory(),
      });
      this.sessions.update(s => [session, ...s]);
      await this.selectSession(session.id);
    } catch (err) {
      console.error('Failed to create session:', err);
    }
  }

  async selectSession(id: string): Promise<void> {
    try {
      const session = await this.tauriBridge.getSession(id);
      this.currentSession.set(session);
      this.selectedProvider.set(session.provider);
      this.workingDirectory.set(session.working_directory);

      const messages = await this.tauriBridge.listMessages(id);
      this.messages.set(messages);

      this.isStreaming.set(false);
      this.streamingContent.set('');
      this.streamingMessageId.set(null);
    } catch (err) {
      console.error('Failed to select session:', err);
    }
  }

  async archiveSession(id: string): Promise<void> {
    try {
      await this.tauriBridge.archiveSession(id);
      this.sessions.update(s => s.filter(sess => sess.id !== id));
      if (this.currentSession()?.id === id) {
        this.currentSession.set(null);
        this.messages.set([]);
      }
    } catch (err) {
      console.error('Failed to archive session:', err);
    }
  }

  // ── Event Handlers (from shared components) ────────────────────────

  onSessionSelected(id: string): void {
    this.selectSession(id);
  }

  onSessionArchived(id: string): void {
    this.archiveSession(id);
  }

  async onSessionDeleted(id: string): Promise<void> {
    try {
      await this.tauriBridge.deleteSession(id);
      this.sessions.update(s => s.filter(sess => sess.id !== id));
      if (this.currentSession()?.id === id) {
        this.currentSession.set(null);
        this.messages.set([]);
      }
    } catch (err) {
      console.error('Failed to delete session:', err);
    }
  }

  onMessageSent(text: string): void {
    this.currentMessage = text;
    this.sendMessage();
  }

  // ── Chat ───────────────────────────────────────────────────────────

  async sendMessage(): Promise<void> {
    const text = this.currentMessage.trim();
    if (!text || this.isStreaming()) return;

    let session = this.currentSession();
    if (!session) {
      await this.createNewSession();
      session = this.currentSession();
      if (!session) return;
    }

    this.currentMessage = '';
    this.isStreaming.set(true);
    this.streamingContent.set('');

    try {
      const userMessage = await this.tauriBridge.sendChatMessage(session.id, text);
      this.messages.update(msgs => [...msgs, userMessage]);
    } catch (err) {
      console.error('Failed to send message:', err);
      this.isStreaming.set(false);
    }
  }

  async stopGeneration(): Promise<void> {
    const session = this.currentSession();
    if (!session) return;
    try {
      await this.tauriBridge.stopGeneration(session.id);
      this.isStreaming.set(false);
    } catch (err) {
      console.error('Failed to stop generation:', err);
    }
  }

  async retryLastMessage(): Promise<void> {
    const msgs = this.messages();
    for (let i = msgs.length - 1; i >= 0; i--) {
      if (msgs[i].role === 'user') {
        this.messages.set(msgs.slice(0, i));
        this.currentMessage = msgs[i].content;
        await this.sendMessage();
        return;
      }
    }
  }

  // ── Streaming ──────────────────────────────────────────────────────

  private subscribeToDelta(): void {
    this.deltaSubscription = this.tauriBridge.onChatDelta().subscribe({
      next: (delta: ChatDelta) => this.handleDelta(delta),
      error: (err) => console.error('Delta subscription error:', err),
    });
  }

  private subscribeToComplete(): void {
    this.completeSubscription = this.tauriBridge.onChatComplete().subscribe({
      next: (complete) => {
        this.isStreaming.set(false);
        this.streamingMessageId.set(null);

        const content = this.streamingContent();
        if (content && complete.message_id) {
          this.messages.update(msgs => {
            const existing = msgs.find(m => m.id === complete.message_id);
            if (existing) {
              return msgs.map(m =>
                m.id === complete.message_id ? { ...m, content, finish_reason: 'stop' } : m
              );
            }
            return msgs;
          });
        }

        this.streamingContent.set('');
        this.autoTitleIfNeeded();

        const session = this.currentSession();
        if (session) {
          this.tauriBridge.getSession(session.id).then(s => this.currentSession.set(s));
        }
      },
    });
  }

  private handleDelta(delta: ChatDelta): void {
    const session = this.currentSession();
    if (!session || delta.session_id !== session.id) return;

    this.streamingMessageId.set(delta.message_id);

    if (delta.chunk.chunk_type === 'text') {
      this.streamingContent.update(c => c + delta.chunk.content);
    }

    this.messages.update(msgs => {
      const exists = msgs.some(m => m.id === delta.message_id);
      if (!exists) {
        return [
          ...msgs,
          {
            id: delta.message_id,
            session_id: delta.session_id,
            role: 'assistant' as const,
            content: '',
            input_tokens: 0,
            output_tokens: 0,
            cost_cents: 0,
            created_at: new Date().toISOString(),
          },
        ];
      }
      return msgs;
    });
  }

  // ── Auto-title ─────────────────────────────────────────────────────

  private async autoTitleIfNeeded(): Promise<void> {
    const session = this.currentSession();
    if (!session || session.title !== 'New Session') return;

    const msgs = this.messages();
    const firstUserMsg = msgs.find(m => m.role === 'user');
    if (!firstUserMsg) return;

    const title = firstUserMsg.content.slice(0, 40) + (firstUserMsg.content.length > 40 ? '...' : '');
    try {
      const updated = await this.tauriBridge.updateSessionTitle(session.id, title);
      this.currentSession.set(updated);
      this.sessions.update(s =>
        s.map(sess => (sess.id === updated.id ? { ...sess, title: updated.title } : sess))
      );
    } catch (err) {
      console.error('Failed to auto-title:', err);
    }
  }

  // ── Agent Auto-Connect ─────────────────────────────────────────────

  private async ensureSettingsAndConnect(): Promise<void> {
    try {
      const status = await this.tauriBridge.getConnectionStatus();
      if (!status.connected) {
        await this.tauriBridge.connectAgent();
      }
    } catch (err) {
      console.error('Agent auto-connect failed:', err);
    }
  }

  // ── Provider / Directory ───────────────────────────────────────────

  async detectTools(): Promise<void> {
    try {
      const tools = await this.tauriBridge.detectCliTools();
      this.detectedTools.set(tools);
    } catch (err) {
      console.error('Failed to detect tools:', err);
    }
  }

  async loadLastWorkingDirectory(): Promise<void> {
    try {
      const dir = await this.tauriBridge.getSetting('last_working_directory');
      if (dir) this.workingDirectory.set(dir);
    } catch {
      // Setting may not exist yet
    }
  }

  async onProviderChange(provider: string): Promise<void> {
    this.selectedProvider.set(provider);
  }

  async onWorkingDirectoryChange(dir: string): Promise<void> {
    this.workingDirectory.set(dir);
    await this.tauriBridge.setSetting('last_working_directory', dir);

    const session = this.currentSession();
    if (session) {
      const updated = await this.tauriBridge.updateSessionWorkingDirectory(session.id, dir);
      this.currentSession.set(updated);
    }
  }

  async pickWorkingDirectory(): Promise<void> {
    const dir = await this.tauriBridge.pickDirectory();
    if (dir) {
      await this.onWorkingDirectoryChange(dir);
    }
  }

  // ── Helpers ────────────────────────────────────────────────────────

  formatTokens(n: number): string {
    if (n < 1000) return String(n);
    if (n < 1000000) return `${(n / 1000).toFixed(1)}k`;
    return `${(n / 1000000).toFixed(1)}M`;
  }

  providerIcon(provider: string): string {
    switch (provider) {
      case 'claude_code': return 'code-slash-outline';
      case 'codex': return 'terminal-outline';
      case 'cline': return 'git-branch-outline';
      case 'ollama': return 'hardware-chip-outline';
      default: return 'chatbubble-outline';
    }
  }

  providerLabel(provider: string): string {
    switch (provider) {
      case 'claude_code': return 'Claude Code';
      case 'codex': return 'Codex';
      case 'cline': return 'Cline';
      case 'ollama': return 'Ollama';
      default: return provider;
    }
  }

  navigateTo(path: string): void {
    this.router.navigate([path]);
  }

  // Keyboard shortcuts (handled at document level)
  onGlobalKeyDown(event: KeyboardEvent): void {
    if (event.ctrlKey || event.metaKey) {
      switch (event.key) {
        case 'n':
          event.preventDefault();
          this.createNewSession();
          break;
        case 'k':
          event.preventDefault();
          // Focus search
          break;
        case ',':
          event.preventDefault();
          this.navigateTo('/settings');
          break;
      }
    }
    if (event.key === 'Escape' && this.isStreaming()) {
      this.stopGeneration();
    }
  }
}
