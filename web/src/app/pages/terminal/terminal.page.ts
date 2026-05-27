import {
  Component,
  OnInit,
  OnDestroy,
  inject,
  signal,
  computed,
} from '@angular/core';
import { CommonModule } from '@angular/common';
import { FormsModule } from '@angular/forms';
import { ActivatedRoute, Router } from '@angular/router';
import {
  IonMenuButton,
  IonModal,
  IonHeader,
  IonToolbar,
  IonTitle,
  IonButtons,
  IonButton,
  IonContent,
  IonList,
  IonItem,
  IonLabel,
  IonIcon,
  IonText,
  IonFooter,
  IonSegment,
  IonSegmentButton,
} from '@ionic/angular/standalone';
import { addIcons } from 'ionicons';
import {
  arrowUpOutline,
  closeOutline,
  folderOutline,
  documentOutline,
} from 'ionicons/icons';
import { AuthService } from '../../services/auth.service';
import { RelayTerminalService } from '../../services/relay-terminal.service';
import { MermaidZoomService } from '../../services/mermaid-zoom.service';
import {
  JohnnyApiService,
  ChatAttachment,
  TerminalScreenComponent,
  AiSession as SharedAiSession,
  AiMessage as SharedAiMessage,
  AiMessageDelta,
  AiChatComplete,
  DetectedCliTool,
  TerminalScreen,
  HostFileEntry,
} from '@johnnyone/ui';
import { firstValueFrom, Subscription } from 'rxjs';
import { WORKSPACE_MOBILE_MEDIA_QUERY } from '../../workspace-responsive';

interface Session {
  id: string;
  title: string;
  provider: string;
  model: string;
  working_directory: string;
  status: 'active' | 'archived' | 'completed';
  total_input_tokens: number;
  total_output_tokens: number;
  total_cost_cents: number;
  created_at: string;
  updated_at: string;
}

interface Message {
  id: string;
  session_id: string;
  role: 'user' | 'assistant' | 'system' | 'tool';
  content: string;
  tool_calls?: string;
  finish_reason?: string;
  input_tokens: number;
  output_tokens: number;
  cost_cents: number;
  created_at: string;
}

interface DetectedTool extends DetectedCliTool {}

interface PaneLayout {
  x: number;
  y: number;
  width: number;
  height: number;
}

interface PendingImageAttachment {
  id: string;
  file: File;
  previewUrl: string;
}

// Register Ionicons used by the workspace-picker modal once at module load.
addIcons({
  'arrow-up-outline': arrowUpOutline,
  'close-outline': closeOutline,
  'folder-outline': folderOutline,
  'document-outline': documentOutline,
});

@Component({
  selector: 'app-terminal',
  standalone: true,
  imports: [
    CommonModule,
    FormsModule,
    TerminalScreenComponent,
    IonMenuButton,
    IonModal,
    IonHeader,
    IonToolbar,
    IonTitle,
    IonButtons,
    IonButton,
    IonContent,
    IonList,
    IonItem,
    IonLabel,
    IonIcon,
    IonText,
    IonFooter,
    IonSegment,
    IonSegmentButton,
  ],
  templateUrl: './terminal.page.html',
  styleUrls: ['./terminal.page.scss'],
})
export class TerminalPage implements OnInit, OnDestroy {
  private static readonly SIDEBAR_WIDTH_KEY = 'johnnyone_desktop_sidebar_width';
  private static readonly STREAM_FIRST_TOKEN_NOTICE_MS = 10_000;
  private static readonly STREAM_IDLE_NOTICE_MS = 8_000;
  private readonly api = inject(JohnnyApiService);
  private readonly auth = inject(AuthService);
  private readonly route = inject(ActivatedRoute);
  private readonly relayTerminal = inject(RelayTerminalService);
  private readonly mermaidZoom = inject(MermaidZoomService);
  private readonly router = inject(Router);
  private readonly minSidebarWidth = 260;
  private readonly maxSidebarWidth = 560;
  private readonly defaultSidebarWidth = 360;
  private resizeMoveHandler: ((event: MouseEvent) => void) | null = null;
  private resizeUpHandler: (() => void) | null = null;
  private paneMoveHandler: ((event: PointerEvent) => void) | null = null;
  private paneUpHandler: (() => void) | null = null;
  private paneInteractionStart = { x: 0, y: 0, left: 0, top: 0, width: 0, height: 0 };
  private streamingStatusInterval: ReturnType<typeof setInterval> | null = null;
  private sessionIdCopiedTimeout: ReturnType<typeof setTimeout> | null = null;
  private deltaSubscription: Subscription | null = null;
  private completeSubscription: Subscription | null = null;
  private terminalSubscription: Subscription | null = null;
  private sessionUpdateSubscription: Subscription | null = null;
  private sessionDeleteSubscription: Subscription | null = null;
  private resizeTerminalTimeout: ReturnType<typeof setTimeout> | null = null;
  private compactWorkspaceMediaQuery: MediaQueryList | null = null;
  private compactWorkspaceListener: ((event: MediaQueryListEvent) => void) | null = null;
  private terminalVisualSubscriptions = new Set<string>();
  private readonly visibilityChangeHandler = () => {
    if (document.hidden) {
      void this.unsubscribeAllTerminalVisuals();
    } else {
      void this.syncTerminalVisualSubscriptions();
    }
  };

  // State
  sessions = signal<Session[]>([]);
  currentSession = signal<Session | null>(null);
  messages = signal<Message[]>([]);
  isStreaming = signal(false);
  streamingContent = signal('');
  streamingMessageId = signal<string | null>(null);
  private streamStartedAt = signal<number | null>(null);
  private lastStreamActivityAt = signal<number | null>(null);
  private streamingStatusTick = signal(0);
  currentMessage = '';
  searchQuery = '';
  detectedTools = signal<DetectedTool[]>([]);
  selectedProvider = signal('ollama');
  workingDirectory = signal('');

  // Workspace browser modal — opened before a fresh "New Terminal" so the user
  // picks the working directory. Mirrors the planner page's host-dir browser
  // but scoped to one mode ('workspace') and tied to session creation.
  newSessionBrowserOpen = signal(false);
  newSessionBrowsePath = signal('/');
  newSessionEntries = signal<HostFileEntry[]>([]);
  newSessionBrowserError = signal<string | null>(null);

  sidebarOpen = signal(true);
  sidebarWidth = signal(this.defaultSidebarWidth);
  isResizingSidebar = signal(false);
  sessionIdCopied = signal(false);
  terminalScreen = signal<TerminalScreen | null>(null);
  terminalScreens = signal<Record<string, TerminalScreen>>({});
  terminalError = signal<string | null>(null);
  paneLayouts = signal<Record<string, PaneLayout>>({});
  closedPaneIds = signal<Set<string>>(new Set());
  paneX = signal(44);
  paneY = signal(34);
  paneWidth = signal(860);
  paneHeight = signal(560);
  isDraggingPane = signal(false);
  isResizingPane = signal(false);
  isCompactWorkspace = signal(false);
  pendingAttachments = signal<PendingImageAttachment[]>([]);
  attachmentMessage = '';
  isSendingAttachments = signal(false);

  // Computed
  filteredSessions = computed(() => {
    const query = this.searchQuery.toLowerCase();
    const all = this.sessions();
    if (!query) return all;
    return all.filter(s => s.title.toLowerCase().includes(query));
  });

  aiSessions = computed<SharedAiSession[]>(() =>
    this.filteredSessions()
      .filter((session) => !this.closedPaneIds().has(session.id))
      .map(s => this.mapSession(s))
  );

  visiblePaneSessions = computed<SharedAiSession[]>(() =>
    this.isCompactWorkspace()
      ? this.aiSessions().filter((session) => session.id === this.currentSession()?.id)
      : this.aiSessions()
  );

  aiMessages = computed<SharedAiMessage[]>(() => {
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

  sessionNumber = computed(() => {
    const session = this.currentSession();
    if (!session) return null;
    return this.toSessionNumber(session.id);
  });

  streamingStatusWarning = computed(() => {
    if (!this.isStreaming()) return false;

    this.streamingStatusTick();

    const startedAt = this.streamStartedAt();
    if (!startedAt) return false;

    const lastActivityAt = this.lastStreamActivityAt();
    const idleMs = Date.now() - (lastActivityAt ?? startedAt);

    if (!lastActivityAt) {
      return idleMs >= TerminalPage.STREAM_FIRST_TOKEN_NOTICE_MS;
    }

    return idleMs >= TerminalPage.STREAM_IDLE_NOTICE_MS;
  });

  streamingStatusText = computed(() => {
    if (!this.isStreaming()) return '';

    this.streamingStatusTick();

    const startedAt = this.streamStartedAt();
    if (!startedAt) return 'Waiting for CLI response…';

    const lastActivityAt = this.lastStreamActivityAt();
    const idleMs = Date.now() - (lastActivityAt ?? startedAt);
    const idleSeconds = Math.max(0, Math.floor(idleMs / 1000));

    if (!lastActivityAt) {
      if (idleMs >= TerminalPage.STREAM_FIRST_TOKEN_NOTICE_MS) {
        return `Still waiting for first token… ${idleSeconds}s`;
      }
      return 'Waiting for CLI response…';
    }

    if (idleMs >= TerminalPage.STREAM_IDLE_NOTICE_MS) {
      return `No new tokens yet… ${idleSeconds}s`;
    }

    return 'Streaming response…';
  });

  ngOnInit(): void {
    this.setupCompactWorkspaceMode();
    this.loadSidebarWidth();
    this.subscribeToTerminalEvents();
    void this.loadSessions(this.route.snapshot.queryParamMap.get('sessionId') ?? undefined);
    void this.detectTools();
    void this.loadLastWorkingDirectory();
    this.subscribeToRelaySessionEvents();
    document.addEventListener('visibilitychange', this.visibilityChangeHandler);
    void this.relayTerminal.connect();
  }

  ngOnDestroy(): void {
    document.removeEventListener('visibilitychange', this.visibilityChangeHandler);
    void this.unsubscribeAllTerminalVisuals();
    this.teardownChatSubscriptions();
    this.teardownTerminalSubscription();
    this.teardownRelaySessionEvents();
    this.clearSessionIdCopiedTimeout();
    this.resetStreamingStatus();
    this.stopSidebarResize();
    this.stopPaneInteraction();
    this.teardownCompactWorkspaceMode();
    this.clearPendingAttachments();
  }

  // ── Type Mappers ───────────────────────────────────────────────────

  private mapSession(s: Session): SharedAiSession {
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

  private mapMessage(m: Message): SharedAiMessage {
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

  private mapApiSessionToState(session: SharedAiSession): Session {
    return {
      id: session.id,
      title: session.title,
      provider: session.provider,
      model: session.model,
      working_directory: session.workingDirectory || '',
      status: session.status,
      total_input_tokens: session.totalInputTokens,
      total_output_tokens: session.totalOutputTokens,
      total_cost_cents: session.totalCostCents,
      created_at: session.createdAt,
      updated_at: session.updatedAt,
    };
  }

  private mapApiMessageToState(message: SharedAiMessage): Message {
    return {
      id: message.id,
      session_id: message.sessionId,
      role: message.role,
      content: message.content,
      tool_calls: message.toolCalls,
      finish_reason: message.finishReason,
      input_tokens: message.inputTokens,
      output_tokens: message.outputTokens,
      cost_cents: message.costCents,
      created_at: message.createdAt,
    };
  }

  // ── Session Management ─────────────────────────────────────────────

  async loadSessions(targetSessionId?: string): Promise<void> {
    try {
      const sessions = (await firstValueFrom(this.api.listSessions('active'))).map((session) =>
        this.mapApiSessionToState(session)
      );
      const sortedSessions = this.sortSessions(sessions);
      this.sessions.set(sortedSessions);
      this.ensurePaneLayouts(sortedSessions);
      await this.syncTerminalVisualSubscriptions();

      const current = this.currentSession();
      if (targetSessionId) {
        const target = sessions.find((session) => session.id === targetSessionId);
        if (target) {
          await this.selectSession(target.id);
          return;
        }
      }

      if (current) {
        const refreshed = sessions.find((s) => s.id === current.id);
        if (refreshed) {
          await this.selectSession(refreshed.id);
        } else {
          this.currentSession.set(null);
          this.messages.set([]);
        }
        return;
      }

      if (sessions.length > 0) {
        await this.selectSession(sessions[0].id);
      }
    } catch (err) {
      console.error('Failed to load sessions:', err);
    }
  }

  private removeInactivePaneState(activeIds: Set<string>): void {
    this.terminalScreens.update((screens) => {
      let changed = false;
      const next: Record<string, TerminalScreen> = {};
      for (const [sessionId, screen] of Object.entries(screens)) {
        if (activeIds.has(sessionId)) {
          next[sessionId] = screen;
        } else {
          changed = true;
        }
      }
      return changed ? next : screens;
    });

    this.paneLayouts.update((layouts) => {
      let changed = false;
      const next: Record<string, PaneLayout> = {};
      for (const [sessionId, layout] of Object.entries(layouts)) {
        if (activeIds.has(sessionId)) {
          next[sessionId] = layout;
        } else {
          changed = true;
        }
      }
      return changed ? next : layouts;
    });

    this.closedPaneIds.update((closedIds) => {
      const next = new Set<string>();
      let changed = false;
      for (const sessionId of closedIds) {
        if (activeIds.has(sessionId)) {
          next.add(sessionId);
        } else {
          changed = true;
        }
      }
      return changed ? next : closedIds;
    });
  }

  private subscribeToRelaySessionEvents(): void {
    if (!this.sessionUpdateSubscription) {
      this.sessionUpdateSubscription = this.relayTerminal.sessionUpdates().subscribe(({ session }) => {
        const mapped = this.mapApiSessionToState(session);
        if (mapped.status === 'active') {
          this.upsertSession(mapped);
          this.ensurePaneLayouts(this.sessions());
          void this.syncTerminalVisualSubscriptions();
        } else {
          void this.unsubscribeTerminalVisual(mapped.id);
          this.removeSessionLocally(mapped.id);
        }
        if (this.currentSession()?.id === mapped.id) {
          if (mapped.status === 'active') {
            this.currentSession.set(mapped);
            this.selectedProvider.set(mapped.provider);
            this.workingDirectory.set(mapped.working_directory);
          } else {
            this.clearCurrentSession(mapped.id);
          }
        }
      });
    }

    if (!this.sessionDeleteSubscription) {
      this.sessionDeleteSubscription = this.relayTerminal.sessionDeletes().subscribe((sessionId) => {
        void this.unsubscribeTerminalVisual(sessionId);
        this.removeSessionLocally(sessionId);
        this.clearCurrentSession(sessionId);
      });
    }
  }

  private teardownRelaySessionEvents(): void {
    this.sessionUpdateSubscription?.unsubscribe();
    this.sessionUpdateSubscription = null;
    this.sessionDeleteSubscription?.unsubscribe();
    this.sessionDeleteSubscription = null;
  }

  private removeSessionLocally(id: string): void {
    this.sessions.update((sessions) => sessions.filter((session) => session.id !== id));
    this.terminalScreens.update((screens) => this.omitRecordKey(screens, id));
    this.paneLayouts.update((layouts) => this.omitRecordKey(layouts, id));
    this.closedPaneIds.update((closedIds) => {
      if (!closedIds.has(id)) return closedIds;
      const next = new Set(closedIds);
      next.delete(id);
      return next;
    });
  }

  private clearCurrentSession(id: string): void {
    if (this.currentSession()?.id !== id) return;
    this.teardownChatSubscriptions();
    this.currentSession.set(null);
    this.messages.set([]);
    this.terminalScreen.set(null);
  }

  /**
   * "New Terminal" button entrypoint. Opens the workspace browser so the user
   * picks the working directory; actual session creation happens once they
   * confirm a path via `confirmNewSessionPath()`.
   */
  async createNewSession(): Promise<void> {
    await this.openNewSessionBrowser();
  }

  async openNewSessionBrowser(): Promise<void> {
    const start = this.workingDirectory().trim() || '/home/creepy/documents/workspace';
    this.newSessionBrowsePath.set(start);
    this.newSessionBrowserError.set(null);
    this.newSessionBrowserOpen.set(true);
    await this.loadNewSessionEntries(start);
  }

  closeNewSessionBrowser(): void {
    this.newSessionBrowserOpen.set(false);
  }

  async browseNewSessionTo(entry: HostFileEntry): Promise<void> {
    if (entry.kind !== 'directory') return;
    this.newSessionBrowsePath.set(entry.path);
    await this.loadNewSessionEntries(entry.path);
  }

  async browseNewSessionParent(): Promise<void> {
    const parent = this.newSessionParentPath(this.newSessionBrowsePath());
    if (!parent) return;
    this.newSessionBrowsePath.set(parent);
    await this.loadNewSessionEntries(parent);
  }

  canBrowseNewSessionUp(): boolean {
    return this.newSessionBrowsePath() !== '/';
  }

  async confirmNewSessionPath(path = this.newSessionBrowsePath()): Promise<void> {
    this.newSessionBrowserOpen.set(false);
    this.workingDirectory.set(path);
    // Persist as the "last working directory" so the picker reopens here next time.
    try {
      await firstValueFrom(this.api.setSetting('last_working_directory', path));
    } catch {
      // setting persistence is best-effort
    }
    try {
      const session = this.mapApiSessionToState(await firstValueFrom(this.api.createSession({
        provider: this.selectedProvider(),
        model: this.selectedProvider() === 'ollama' ? 'qwen3.5:2b' : undefined,
        workingDirectory: path,
      })));
      this.upsertSession(session);
      await this.selectSession(session.id);
    } catch (err) {
      console.error('Failed to create session:', err);
    }
  }

  private async loadNewSessionEntries(path: string): Promise<void> {
    try {
      this.newSessionBrowserError.set(null);
      this.newSessionEntries.set(
        await firstValueFrom(this.api.browseHostDirectory(path)),
      );
    } catch (err) {
      this.newSessionEntries.set([]);
      this.newSessionBrowserError.set(String(err));
    }
  }

  private newSessionParentPath(path: string): string | null {
    if (!path || path === '/') return null;
    const idx = path.replace(/\/+$/, '').lastIndexOf('/');
    return idx <= 0 ? '/' : path.slice(0, idx);
  }

  async selectSession(id: string): Promise<void> {
    try {
      const [apiSession, apiMessages] = await Promise.all([
        firstValueFrom(this.api.getSession(id)),
        firstValueFrom(this.api.listMessages(id)),
      ]);
      const session = this.mapApiSessionToState(apiSession);
      const messages = apiMessages.map((message) => this.mapApiMessageToState(message));

      this.upsertSession(session);
      this.currentSession.set(session);
      this.selectedProvider.set(session.provider);
      this.workingDirectory.set(session.working_directory);
      this.messages.set(messages);
      await this.attachTerminal(id);

      this.isStreaming.set(false);
      this.streamingContent.set('');
      this.streamingMessageId.set(null);
      this.resetStreamingStatus();
    } catch (err) {
      console.error('Failed to select session:', err);
    }
  }

  async archiveSession(id: string): Promise<void> {
    try {
      await firstValueFrom(this.api.archiveSession(id));
      this.sessions.update(s => s.filter(sess => sess.id !== id));
      if (this.currentSession()?.id === id) {
        this.teardownChatSubscriptions();
        this.teardownTerminalSubscription();
        this.currentSession.set(null);
        this.messages.set([]);
        this.terminalScreen.set(null);
        this.terminalScreens.update((screens) => this.omitRecordKey(screens, id));
        this.paneLayouts.update((layouts) => this.omitRecordKey(layouts, id));
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
      await firstValueFrom(this.api.deleteSession(id));
      this.sessions.update(s => s.filter(sess => sess.id !== id));
      if (this.currentSession()?.id === id) {
        this.teardownChatSubscriptions();
        this.teardownTerminalSubscription();
        this.currentSession.set(null);
        this.messages.set([]);
        this.terminalScreen.set(null);
        this.terminalScreens.update((screens) => this.omitRecordKey(screens, id));
        this.paneLayouts.update((layouts) => this.omitRecordKey(layouts, id));
      }
    } catch (err) {
      console.error('Failed to delete session:', err);
    }
  }

  async onSessionRenamed(id: string): Promise<void> {
    const target = this.sessions().find((session) => session.id === id);
    if (!target) return;

    const nextTitle = window.prompt('Enter a new session name:', target.title);
    if (nextTitle === null) return;

    const trimmedTitle = nextTitle.trim();
    if (!trimmedTitle || trimmedTitle === target.title) return;

    try {
      const updated = this.mapApiSessionToState(
        await firstValueFrom(this.api.updateSessionTitle(id, trimmedTitle))
      );
      this.upsertSession(updated);

      if (this.currentSession()?.id === id) {
        this.currentSession.set(updated);
      }
    } catch (err) {
      console.error('Failed to rename session:', err);
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

    const desiredProvider = this.selectedProvider();
    if (session.provider !== desiredProvider) {
      try {
        const updated = this.mapApiSessionToState(
          await firstValueFrom(this.api.updateSessionProvider(session.id, desiredProvider))
        );
        session = updated;
        this.currentSession.set(updated);
        this.upsertSession(updated);
      } catch (err) {
        console.error('Failed to sync provider before send:', err);
        return;
      }
    }

    this.currentMessage = '';
    this.isStreaming.set(true);
    this.streamingContent.set('');
    this.streamingMessageId.set(null);
    this.beginStreamingStatus();
    const optimisticUserMessage = this.createOptimisticUserMessage(session.id, text);
    this.messages.update((msgs) => [...msgs, optimisticUserMessage]);

    try {
      await this.relayTerminal.sendInput(session.id, `${text}\r`);
      this.finishStreamingState();
      await this.refreshSession(session.id);
      await this.autoTitleIfNeeded();
    } catch (err) {
      console.error('Failed to send message:', err);
      this.messages.update((msgs) => msgs.filter((msg) => msg.id !== optimisticUserMessage.id));
      this.finishStreamingState();
      this.currentMessage = text;
    }
  }

  onWorkspacePaste(event: ClipboardEvent): void {
    const files = Array.from(event.clipboardData?.files ?? []).filter((file) =>
      file.type.startsWith('image/')
    );
    if (files.length === 0) return;
    event.preventDefault();
    this.addPendingImageFiles(files);
  }

  onWorkspaceDragOver(event: DragEvent): void {
    if (this.dragEventHasImage(event)) {
      event.preventDefault();
    }
  }

  onWorkspaceDrop(event: DragEvent): void {
    const files = Array.from(event.dataTransfer?.files ?? []).filter((file) =>
      file.type.startsWith('image/')
    );
    if (files.length === 0) return;
    event.preventDefault();
    this.addPendingImageFiles(files);
  }

  removePendingAttachment(id: string): void {
    this.pendingAttachments.update((items) => {
      const target = items.find((item) => item.id === id);
      if (target) URL.revokeObjectURL(target.previewUrl);
      return items.filter((item) => item.id !== id);
    });
  }

  async sendAttachmentMessage(): Promise<void> {
    const session = this.currentSession();
    const attachments = this.pendingAttachments();
    const text = this.attachmentMessage.trim();
    if (!session || this.isSendingAttachments() || attachments.length === 0) return;

    this.isSendingAttachments.set(true);
    try {
      const uploaded: ChatAttachment[] = [];
      for (const item of attachments) {
        uploaded.push(
          await firstValueFrom(this.api.createChatAttachment({
            sessionId: session.id,
            originalName: item.file.name || 'clipboard-image.png',
            contentType: item.file.type || 'image/png',
            dataBase64: await this.fileToBase64(item.file),
          }))
        );
      }

      await this.relayTerminal.sendInputWithAttachments(
        session.id,
        `${text || 'Please review the attached image.'}\r`,
        uploaded.map((attachment) => ({
          id: attachment.id,
          originalName: attachment.originalName,
          contentType: attachment.contentType,
          size: attachment.size,
        })),
      );

      this.attachmentMessage = '';
      this.clearPendingAttachments();
    } catch (err) {
      console.error('Failed to send image attachment:', err);
    } finally {
      this.isSendingAttachments.set(false);
    }
  }

  private addPendingImageFiles(files: File[]): void {
    const items = files.map((file) => ({
      id: crypto.randomUUID(),
      file,
      previewUrl: URL.createObjectURL(file),
    }));
    this.pendingAttachments.update((current) => [...current, ...items]);
  }

  private clearPendingAttachments(): void {
    for (const item of this.pendingAttachments()) {
      URL.revokeObjectURL(item.previewUrl);
    }
    this.pendingAttachments.set([]);
  }

  private fileToBase64(file: File): Promise<string> {
    return new Promise((resolve, reject) => {
      const reader = new FileReader();
      reader.onerror = () => reject(reader.error ?? new Error('Failed to read image'));
      reader.onload = () => {
        const value = String(reader.result ?? '');
        resolve(value.includes(',') ? value.split(',').pop() ?? '' : value);
      };
      reader.readAsDataURL(file);
    });
  }

  private dragEventHasImage(event: DragEvent): boolean {
    return Array.from(event.dataTransfer?.items ?? []).some((item) =>
      item.kind === 'file' && item.type.startsWith('image/')
    );
  }

  async stopGeneration(): Promise<void> {
    const session = this.currentSession();
    if (!session) return;
    try {
      await this.relayTerminal.sendInput(session.id, '\u0003');
      this.isStreaming.set(false);
      this.streamingMessageId.set(null);
      this.resetStreamingStatus();
    } catch (err) {
      console.error('Failed to stop generation:', err);
    }
  }

  async retryLastMessage(): Promise<void> {
    const msgs = this.messages();
    for (let i = msgs.length - 1; i >= 0; i--) {
      if (msgs[i].role === 'user') {
        this.currentMessage = msgs[i].content;
        await this.sendMessage();
        return;
      }
    }
  }

  private subscribeToSessionEvents(sessionId: string): void {
    this.teardownChatSubscriptions();

    this.deltaSubscription = this.api.onMessageDelta(sessionId).subscribe({
      next: (delta) => this.handleDelta(delta),
      error: (err) => console.error('Failed to subscribe to ai chat deltas:', err),
    });

    this.completeSubscription = this.api.onNewMessage(sessionId).subscribe({
      next: (complete: any) => {
        void this.handleComplete(complete);
      },
      error: (err: unknown) => console.error('Failed to subscribe to ai chat completion:', err),
    });
  }

  private subscribeToTerminalEvents(): void {
    if (this.terminalSubscription) return;
    this.terminalSubscription = this.relayTerminal.screens().subscribe({
      next: (screen) => {
        this.terminalScreens.update((screens) => ({ ...screens, [screen.sessionId]: screen }));
        if (screen.sessionId === this.currentSession()?.id) {
          this.terminalScreen.set(screen);
          this.terminalError.set(null);
        }
      },
      error: (err) => {
        console.error('Failed to subscribe to terminal screen:', err);
        this.terminalError.set(String(err));
      },
    });
  }

  private teardownTerminalSubscription(): void {
    this.terminalSubscription?.unsubscribe();
    this.terminalSubscription = null;
    if (this.resizeTerminalTimeout) {
      clearTimeout(this.resizeTerminalTimeout);
      this.resizeTerminalTimeout = null;
    }
  }

  async attachTerminal(sessionId: string): Promise<void> {
    try {
      await this.relayTerminal.refreshVisual(sessionId);
      this.terminalVisualSubscriptions.add(sessionId);
      this.terminalError.set(null);
    } catch (err) {
      console.error('Failed to attach terminal:', err);
      this.terminalError.set(String(err));
    }
  }

  private async syncTerminalVisualSubscriptions(): Promise<void> {
    if (document.hidden) return;
    const visibleIds = new Set(this.visiblePaneSessions().map((session) => session.id));

    for (const sessionId of Array.from(this.terminalVisualSubscriptions)) {
      if (!visibleIds.has(sessionId)) {
        await this.unsubscribeTerminalVisual(sessionId);
      }
    }

    for (const sessionId of visibleIds) {
      await this.subscribeTerminalVisual(sessionId);
    }
  }

  private async subscribeTerminalVisual(sessionId: string): Promise<void> {
    if (this.terminalVisualSubscriptions.has(sessionId)) return;
    await this.relayTerminal.subscribeVisual(sessionId);
    this.terminalVisualSubscriptions.add(sessionId);
  }

  private async unsubscribeTerminalVisual(sessionId: string): Promise<void> {
    if (!this.terminalVisualSubscriptions.delete(sessionId)) return;
    await this.relayTerminal.unsubscribeVisual(sessionId);
  }

  private async unsubscribeAllTerminalVisuals(): Promise<void> {
    for (const sessionId of Array.from(this.terminalVisualSubscriptions)) {
      await this.unsubscribeTerminalVisual(sessionId);
    }
  }

  async onTerminalRawInput(data: string, sessionId = this.currentSession()?.id): Promise<void> {
    const session = this.sessions().find((item) => item.id === sessionId);
    if (!session) return;
    try {
      if (this.currentSession()?.id !== session.id) {
        await this.selectSession(session.id);
      }
      await this.relayTerminal.sendInput(session.id, data);
    } catch (err) {
      console.error('Failed to send terminal input:', err);
      this.terminalError.set(String(err));
    }
  }

  onTerminalResize(size: { cols: number; rows: number }, sessionId = this.currentSession()?.id): void {
    const session = this.sessions().find((item) => item.id === sessionId);
    if (!session) return;
    if (this.resizeTerminalTimeout) clearTimeout(this.resizeTerminalTimeout);
    this.resizeTerminalTimeout = setTimeout(() => {
      this.relayTerminal.resize(session.id, size.cols, size.rows).catch((err) => {
        console.error('Failed to resize terminal:', err);
      });
    }, 180);
  }

  onTerminalHistoryRequested(rows: number, sessionId = this.currentSession()?.id): void {
    const session = this.sessions().find((item) => item.id === sessionId);
    if (!session) return;

    this.relayTerminal.loadHistory(session.id, rows).catch((err) => {
      console.error('Failed to load terminal history:', err);
      this.terminalError.set(String(err));
    });
  }

  openTerminalMermaid(svg: string): void {
    this.mermaidZoom.open(svg);
  }

  startPaneDrag(event: PointerEvent, sessionId = this.currentSession()?.id): void {
    if (this.isCompactWorkspace()) return;
    const layout = sessionId ? this.paneLayout(sessionId) : null;
    if (!sessionId || !layout) return;
    if ((event.target as HTMLElement).closest('button, select, input')) return;

    event.preventDefault();
    this.stopPaneInteraction();
    this.paneInteractionStart = {
      x: event.clientX,
      y: event.clientY,
      left: layout.x,
      top: layout.y,
      width: layout.width,
      height: layout.height,
    };
    this.isDraggingPane.set(true);

    this.paneMoveHandler = (moveEvent) => {
      const maxLeft = Math.max(12, window.innerWidth - 180);
      const maxTop = Math.max(12, window.innerHeight - 120);
      this.updatePaneLayout(sessionId, {
        x: this.clamp(this.paneInteractionStart.left + moveEvent.clientX - this.paneInteractionStart.x, 12, maxLeft),
        y: this.clamp(this.paneInteractionStart.top + moveEvent.clientY - this.paneInteractionStart.y, 12, maxTop),
      });
    };
    this.paneUpHandler = () => this.stopPaneInteraction();

    window.addEventListener('pointermove', this.paneMoveHandler);
    window.addEventListener('pointerup', this.paneUpHandler, { once: true });
  }

  startPaneResize(event: PointerEvent, sessionId = this.currentSession()?.id): void {
    const layout = sessionId ? this.paneLayout(sessionId) : null;
    if (!sessionId || !layout) return;
    event.preventDefault();
    event.stopPropagation();
    this.stopPaneInteraction();
    this.paneInteractionStart = {
      x: event.clientX,
      y: event.clientY,
      left: layout.x,
      top: layout.y,
      width: layout.width,
      height: layout.height,
    };
    this.isResizingPane.set(true);

    this.paneMoveHandler = (moveEvent) => {
      const currentLayout = this.paneLayout(sessionId);
      if (this.isCompactWorkspace()) {
        const maxHeight = Math.max(420, Math.floor(window.innerHeight * 1.5));
        this.updatePaneLayout(sessionId, {
          height: this.clamp(this.paneInteractionStart.height + moveEvent.clientY - this.paneInteractionStart.y, 360, maxHeight),
        });
        return;
      }

      const maxWidth = Math.max(420, window.innerWidth - currentLayout.x - 18);
      const maxHeight = Math.max(260, window.innerHeight - currentLayout.y - 78);
      this.updatePaneLayout(sessionId, {
        width: this.clamp(this.paneInteractionStart.width + moveEvent.clientX - this.paneInteractionStart.x, 420, maxWidth),
        height: this.clamp(this.paneInteractionStart.height + moveEvent.clientY - this.paneInteractionStart.y, 260, maxHeight),
      });
    };
    this.paneUpHandler = () => this.stopPaneInteraction();

    window.addEventListener('pointermove', this.paneMoveHandler);
    window.addEventListener('pointerup', this.paneUpHandler, { once: true });
  }

  private stopPaneInteraction(): void {
    if (this.paneMoveHandler) {
      window.removeEventListener('pointermove', this.paneMoveHandler);
      this.paneMoveHandler = null;
    }
    if (this.paneUpHandler) {
      window.removeEventListener('pointerup', this.paneUpHandler);
      this.paneUpHandler = null;
    }
    this.isDraggingPane.set(false);
    this.isResizingPane.set(false);
  }

  private setupCompactWorkspaceMode(): void {
    if (typeof window === 'undefined' || !window.matchMedia) return;

    this.compactWorkspaceMediaQuery = window.matchMedia(WORKSPACE_MOBILE_MEDIA_QUERY);
    this.isCompactWorkspace.set(this.compactWorkspaceMediaQuery.matches);
    this.compactWorkspaceListener = (event) => {
      this.stopPaneInteraction();
      this.isCompactWorkspace.set(event.matches);
      void this.syncTerminalVisualSubscriptions();
    };
    this.compactWorkspaceMediaQuery.addEventListener('change', this.compactWorkspaceListener);
  }

  private teardownCompactWorkspaceMode(): void {
    if (this.compactWorkspaceMediaQuery && this.compactWorkspaceListener) {
      this.compactWorkspaceMediaQuery.removeEventListener('change', this.compactWorkspaceListener);
    }
    this.compactWorkspaceMediaQuery = null;
    this.compactWorkspaceListener = null;
  }

  private teardownChatSubscriptions(): void {
    this.deltaSubscription?.unsubscribe();
    this.deltaSubscription = null;
    this.completeSubscription?.unsubscribe();
    this.completeSubscription = null;
  }

  private handleDelta(delta: AiMessageDelta): void {
    const session = this.currentSession();
    if (!session || delta.sessionId !== session.id) return;

    if (delta.chunkType && delta.chunkType !== 'text') {
      if (delta.finishReason) {
        this.finishStreamingState();
      }
      return;
    }

    if (this.streamingMessageId() !== delta.messageId) {
      this.streamingContent.set('');
    }

    this.markStreamingActivity();
    this.isStreaming.set(true);
    this.streamingMessageId.set(delta.messageId);
    this.streamingContent.update((content) => content + delta.delta);

    this.messages.update((msgs) => {
      const existing = msgs.find((msg) => msg.id === delta.messageId);
      const nextContent = this.streamingContent();

      if (existing) {
        return msgs.map((msg) =>
          msg.id === delta.messageId
            ? {
                ...msg,
                content: nextContent,
                finish_reason: delta.finishReason ?? msg.finish_reason,
              }
            : msg
        );
      }

      return [
        ...msgs,
        {
          id: delta.messageId,
          session_id: delta.sessionId,
          role: 'assistant',
          content: nextContent,
          finish_reason: delta.finishReason,
          input_tokens: 0,
          output_tokens: 0,
          cost_cents: 0,
          created_at: new Date().toISOString(),
        },
      ];
    });

    if (delta.finishReason) {
      this.finishStreamingState();
    }
  }

  private async handleComplete(complete: AiChatComplete): Promise<void> {
    const session = this.currentSession();
    if (!session || complete.sessionId !== session.id) return;

    this.finishStreamingState();
    await this.refreshMessages(session.id);
    await this.refreshSession(session.id);
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
      const updated = this.mapApiSessionToState(
        await firstValueFrom(this.api.updateSessionTitle(session.id, title))
      );
      this.currentSession.set(updated);
      this.upsertSession(updated);
    } catch (err) {
      console.error('Failed to auto-title:', err);
    }
  }

  // ── Provider / Directory ───────────────────────────────────────────

  async detectTools(): Promise<void> {
    try {
      const tools = await firstValueFrom(this.api.detectCliTools());
      this.detectedTools.set(tools);
      if (!tools.some((tool) => tool.provider === this.selectedProvider() && tool.found)) {
        const preferred = tools.find((tool) => tool.provider === 'ollama' && tool.found)
          ?? tools.find((tool) => tool.found);
        if (preferred) this.selectedProvider.set(preferred.provider);
      }
    } catch (err) {
      console.error('Failed to detect tools:', err);
    }
  }

  async loadLastWorkingDirectory(): Promise<void> {
    try {
      const dir = await firstValueFrom(this.api.getSetting('last_working_directory'));
      if (dir) this.workingDirectory.set(dir);
    } catch {
      // Setting may not exist yet
    }
  }

  async onProviderChange(provider: string): Promise<void> {
    this.selectedProvider.set(provider);

    const session = this.currentSession();
    if (!session) return;

    try {
      const updated = this.mapApiSessionToState(
        await firstValueFrom(this.api.updateSessionProvider(session.id, provider))
      );
      this.currentSession.set(updated);
      this.upsertSession(updated);
    } catch (err) {
      console.error('Failed to update session provider:', err);
      // Revert UI selection to persisted value on error.
      this.selectedProvider.set(session.provider);
    }
  }

  async onWorkingDirectoryChange(dir: string): Promise<void> {
    this.workingDirectory.set(dir);
    const session = this.currentSession();
    if (session) {
      const updated = this.mapApiSessionToState(
        await firstValueFrom(this.api.updateSessionWorkingDirectory(session.id, dir))
      );
      this.currentSession.set(updated);
      this.upsertSession(updated);
    }
  }

  async pickWorkingDirectory(): Promise<void> {
    const dir = window.prompt('Working directory', this.workingDirectory());
    if (dir) {
      await this.onWorkingDirectoryChange(dir);
    }
  }

  async pickWorkingDirectoryForSession(sessionId: string): Promise<void> {
    if (this.currentSession()?.id !== sessionId) {
      await this.selectSession(sessionId);
    }
    await this.pickWorkingDirectory();
  }

  async killTerminal(sessionId: string): Promise<void> {
    await this.closeTerminalPane(sessionId, true);
  }

  async closeTerminalPane(sessionId: string, killTerminal = true): Promise<void> {
    const wasCurrentSession = this.currentSession()?.id === sessionId;
    await this.unsubscribeTerminalVisual(sessionId);

    try {
      if (killTerminal) {
        await this.relayTerminal.kill(sessionId);
      }
    } catch (err) {
      console.error('Failed to kill terminal:', err);
      this.terminalError.set(`Failed to kill terminal: ${String(err)}`);
    }

    try {
      await firstValueFrom(this.api.archiveSession(sessionId));
    } catch (err) {
      console.error('Failed to archive closed terminal session:', err);
      this.terminalError.set(`Failed to close terminal: ${String(err)}`);
      return;
    }

    const nextClosed = new Set(this.closedPaneIds());
    nextClosed.add(sessionId);
    this.closedPaneIds.set(nextClosed);
    this.sessions.update((sessions) => sessions.filter((session) => session.id !== sessionId));
    this.terminalScreens.update((screens) => this.omitRecordKey(screens, sessionId));
    this.paneLayouts.update((layouts) => this.omitRecordKey(layouts, sessionId));

    if (!wasCurrentSession) return;

    this.terminalScreen.set(null);
    const nextSession = this.sessions().find((session) => !nextClosed.has(session.id));
    if (nextSession) {
      await this.selectSession(nextSession.id);
    } else {
      this.teardownChatSubscriptions();
      this.teardownTerminalSubscription();
      this.currentSession.set(null);
      this.messages.set([]);
    }
  }

  // ── Sidebar Resizing ───────────────────────────────────────────────

  startSidebarResize(event: MouseEvent): void {
    if (!this.sidebarOpen()) return;

    event.preventDefault();
    this.stopSidebarResize();

    const startX = event.clientX;
    const startWidth = this.sidebarWidth();
    this.isResizingSidebar.set(true);

    this.resizeMoveHandler = (moveEvent: MouseEvent) => {
      const deltaX = moveEvent.clientX - startX;
      const nextWidth = this.clampSidebarWidth(startWidth + deltaX);
      this.sidebarWidth.set(nextWidth);
    };

    this.resizeUpHandler = () => {
      const width = this.sidebarWidth();
      this.saveSidebarWidth(width);
      this.stopSidebarResize();
    };

    window.addEventListener('mousemove', this.resizeMoveHandler);
    window.addEventListener('mouseup', this.resizeUpHandler);
  }

  resetSidebarWidth(): void {
    const width = this.defaultSidebarWidth;
    this.sidebarWidth.set(width);
    this.saveSidebarWidth(width);
  }

  private stopSidebarResize(): void {
    if (this.resizeMoveHandler) {
      window.removeEventListener('mousemove', this.resizeMoveHandler);
      this.resizeMoveHandler = null;
    }

    if (this.resizeUpHandler) {
      window.removeEventListener('mouseup', this.resizeUpHandler);
      this.resizeUpHandler = null;
    }

    this.isResizingSidebar.set(false);
  }

  private loadSidebarWidth(): void {
    try {
      const raw = localStorage.getItem(TerminalPage.SIDEBAR_WIDTH_KEY);
      if (!raw) return;

      const parsed = Number(raw);
      if (Number.isFinite(parsed)) {
        this.sidebarWidth.set(this.clampSidebarWidth(parsed));
      }
    } catch {
      // Ignore malformed or unavailable localStorage.
    }
  }

  private saveSidebarWidth(width: number): void {
    try {
      localStorage.setItem(TerminalPage.SIDEBAR_WIDTH_KEY, String(width));
    } catch {
      // Ignore localStorage write failures.
    }
  }

  private clampSidebarWidth(width: number): number {
    return Math.min(this.maxSidebarWidth, Math.max(this.minSidebarWidth, width));
  }

  private clamp(value: number, min: number, max: number): number {
    return Math.min(max, Math.max(min, value));
  }

  paneLayout(sessionId: string, index = 0): PaneLayout {
    return this.paneLayouts()[sessionId] ?? this.defaultPaneLayout(index);
  }

  private ensurePaneLayouts(sessions: Session[]): void {
    this.paneLayouts.update((layouts) => {
      let changed = false;
      const next = { ...layouts };
      sessions.forEach((session, index) => {
        if (!next[session.id]) {
          next[session.id] = this.defaultPaneLayout(index);
          changed = true;
        }
      });
      return changed ? next : layouts;
    });
  }

  private updatePaneLayout(sessionId: string, patch: Partial<PaneLayout>): void {
    this.paneLayouts.update((layouts) => {
      const current = layouts[sessionId] ?? this.defaultPaneLayout(0);
      return {
        ...layouts,
        [sessionId]: { ...current, ...patch },
      };
    });
  }

  private defaultPaneLayout(index: number): PaneLayout {
    if (this.isCompactWorkspace() && typeof window !== 'undefined') {
      return {
        x: 0,
        y: 0,
        width: Math.max(320, window.innerWidth - 20),
        height: this.defaultCompactPaneHeight(),
      };
    }

    return {
      x: 44 + index * 36,
      y: 34 + index * 30,
      width: 860,
      height: 560,
    };
  }

  private defaultCompactPaneHeight(): number {
    const workspaceHeight = document.querySelector('.terminal-workspace')?.clientHeight;
    const availableHeight = workspaceHeight && workspaceHeight > 0
      ? workspaceHeight - 20
      : window.innerHeight - 220;

    return this.clamp(Math.floor(availableHeight), 360, 680);
  }

  private omitRecordKey<T>(record: Record<string, T>, key: string): Record<string, T> {
    const { [key]: _ignored, ...rest } = record;
    return rest;
  }

  private beginStreamingStatus(): void {
    this.streamStartedAt.set(Date.now());
    this.lastStreamActivityAt.set(null);
    this.startStreamingStatusTicker();
  }

  private markStreamingActivity(): void {
    if (!this.isStreaming()) return;
    this.lastStreamActivityAt.set(Date.now());
  }

  private resetStreamingStatus(): void {
    this.streamStartedAt.set(null);
    this.lastStreamActivityAt.set(null);
    this.stopStreamingStatusTicker();
  }

  private startStreamingStatusTicker(): void {
    if (this.streamingStatusInterval) return;
    this.streamingStatusInterval = setInterval(() => {
      this.streamingStatusTick.update((n) => n + 1);
    }, 1000);
  }

  private stopStreamingStatusTicker(): void {
    if (!this.streamingStatusInterval) return;
    clearInterval(this.streamingStatusInterval);
    this.streamingStatusInterval = null;
  }

  private sortSessions(sessions: Session[]): Session[] {
    return [...sessions].sort((a, b) => b.updated_at.localeCompare(a.updated_at));
  }

  private upsertSession(session: Session): void {
    this.sessions.update((sessions) =>
      this.sortSessions([
        session,
        ...sessions.filter((existing) => existing.id !== session.id),
      ])
    );
    this.ensurePaneLayouts([session]);
  }

  private upsertMessage(messages: Message[], message: Message): Message[] {
    const existingIndex = messages.findIndex((item) => item.id === message.id);
    if (existingIndex === -1) {
      return [...messages, message];
    }

    return messages.map((item) => (item.id === message.id ? message : item));
  }

  private createOptimisticUserMessage(sessionId: string, content: string): Message {
    return {
      id: `temp-user-${Date.now()}`,
      session_id: sessionId,
      role: 'user',
      content,
      input_tokens: 0,
      output_tokens: 0,
      cost_cents: 0,
      created_at: new Date().toISOString(),
    };
  }

  private finishStreamingState(): void {
    this.isStreaming.set(false);
    this.streamingContent.set('');
    this.streamingMessageId.set(null);
    this.resetStreamingStatus();
  }

  private async refreshSession(sessionId: string): Promise<void> {
    const refreshed = this.mapApiSessionToState(await firstValueFrom(this.api.getSession(sessionId)));
    this.currentSession.set(refreshed);
    this.upsertSession(refreshed);
  }

  private async refreshMessages(sessionId: string): Promise<void> {
    const messages = await firstValueFrom(this.api.listMessages(sessionId));
    this.messages.set(messages.map((message) => this.mapApiMessageToState(message)));
  }

  // ── Helpers ────────────────────────────────────────────────────────

  formatTokens(n: number): string {
    if (n < 1000) return String(n);
    if (n < 1000000) return `${(n / 1000).toFixed(1)}k`;
    return `${(n / 1000000).toFixed(1)}M`;
  }

  async copyCurrentSessionId(): Promise<void> {
    const sessionId = this.currentSession()?.id;
    if (!sessionId) return;

    const copied = await this.copyTextToClipboard(sessionId);
    if (!copied) {
      console.error('Failed to copy session ID to clipboard');
      return;
    }

    this.sessionIdCopied.set(true);
    this.clearSessionIdCopiedTimeout();
    this.sessionIdCopiedTimeout = setTimeout(() => {
      this.sessionIdCopied.set(false);
      this.sessionIdCopiedTimeout = null;
    }, 1600);
  }

  toSessionNumber(sessionId: string): string {
    const [prefix] = sessionId.split('-');
    return prefix || sessionId;
  }

  private async copyTextToClipboard(value: string): Promise<boolean> {
    try {
      if (typeof navigator !== 'undefined' && navigator.clipboard?.writeText) {
        await navigator.clipboard.writeText(value);
        return true;
      }
    } catch {
      // Fall back to legacy copy path.
    }

    try {
      const textarea = document.createElement('textarea');
      textarea.value = value;
      textarea.setAttribute('readonly', '');
      textarea.style.position = 'absolute';
      textarea.style.left = '-9999px';
      document.body.appendChild(textarea);
      textarea.select();
      const copied = document.execCommand('copy');
      document.body.removeChild(textarea);
      return copied;
    } catch {
      return false;
    }
  }

  private clearSessionIdCopiedTimeout(): void {
    if (!this.sessionIdCopiedTimeout) return;
    clearTimeout(this.sessionIdCopiedTimeout);
    this.sessionIdCopiedTimeout = null;
  }

  providerIcon(provider: string): string {
    switch (provider) {
      case 'claude_code': return 'code-slash-outline';
      case 'codex': return 'terminal-outline';
      case 'cline': return 'git-branch-outline';
      case 'ollama': return 'hardware-chip-outline';
      case 'shell': return 'terminal-outline';
      default: return 'chatbubble-outline';
    }
  }

  providerLabel(provider: string): string {
    switch (provider) {
      case 'claude_code': return 'Claude Code';
      case 'codex': return 'Codex';
      case 'cline': return 'Cline';
      case 'ollama': return 'Ollama';
      case 'shell': return 'Shell';
      default: return provider;
    }
  }

  navigateTo(path: string): void {
    this.router.navigate([path]);
  }

  logout(): void {
    this.auth.logout();
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
