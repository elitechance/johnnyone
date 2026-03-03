import { Component, OnInit, OnDestroy, inject, signal, computed } from '@angular/core';
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
  IonBadge,
  IonChip,
  IonLabel,
  IonSplitPane,
  IonMenu,
  IonList,
  IonItem,
  IonMenuButton,
  IonSearchbar,
  IonSelect,
  IonSelectOption,
  IonFooter,
} from '@ionic/angular/standalone';
import { Router } from '@angular/router';
import {
  TauriBridgeService,
  Session,
  Message,
  ChatDelta,
  StreamChunk,
  DetectedTool,
} from '../../services/tauri-bridge.service';
import { Subscription } from 'rxjs';

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
    IonBadge,
    IonChip,
    IonLabel,
    IonSplitPane,
    IonMenu,
    IonList,
    IonItem,
    IonMenuButton,
    IonSearchbar,
    IonSelect,
    IonSelectOption,
    IonFooter,
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
  }

  ngOnDestroy(): void {
    this.deltaSubscription?.unsubscribe();
    this.completeSubscription?.unsubscribe();
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

  async archiveSession(event: Event, id: string): Promise<void> {
    event.stopPropagation();
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
    // Find the last user message
    for (let i = msgs.length - 1; i >= 0; i--) {
      if (msgs[i].role === 'user') {
        // Remove all messages after this user message
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

        // Finalize the streaming message
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

        // Auto-title: if this is the first exchange, title the session
        this.autoTitleIfNeeded();

        // Refresh session to get updated token counts
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

    // Ensure assistant message exists in the list
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
    const session = this.currentSession();
    if (session) {
      // Update session provider
    }
  }

  async onWorkingDirectoryChange(dir: string): Promise<void> {
    this.workingDirectory.set(dir);
    await this.tauriBridge.setSetting('last_working_directory', dir);

    // Update the current session's working directory in the DB
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

  getDisplayContent(msg: Message): string {
    if (this.streamingMessageId() === msg.id) {
      return this.streamingContent();
    }
    return msg.content;
  }

  isStreamingMessage(msg: Message): boolean {
    return this.streamingMessageId() === msg.id;
  }

  formatDate(dateStr: string): string {
    if (!dateStr) return '';
    const date = new Date(dateStr);
    const now = new Date();
    const diffMs = now.getTime() - date.getTime();
    const diffDays = Math.floor(diffMs / (1000 * 60 * 60 * 24));
    if (diffDays === 0) return date.toLocaleTimeString([], { hour: '2-digit', minute: '2-digit' });
    if (diffDays === 1) return 'Yesterday';
    if (diffDays < 7) return date.toLocaleDateString([], { weekday: 'short' });
    return date.toLocaleDateString([], { month: 'short', day: 'numeric' });
  }

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

  onKeyDown(event: KeyboardEvent): void {
    // Ctrl+Enter to send
    if (event.key === 'Enter' && (event.ctrlKey || event.metaKey)) {
      event.preventDefault();
      this.sendMessage();
    }
    // Enter without shift to send (single line mode)
    if (event.key === 'Enter' && !event.shiftKey && !event.ctrlKey && !event.metaKey) {
      event.preventDefault();
      this.sendMessage();
    }
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

  trackByMessageId(_index: number, msg: Message): string {
    return msg.id;
  }

  trackBySessionId(_index: number, session: Session): string {
    return session.id;
  }
}
