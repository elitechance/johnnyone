import {
  AfterViewInit,
  Component,
  ElementRef,
  HostListener,
  OnDestroy,
  OnInit,
  ViewChild,
  inject,
  signal,
  computed,
  effect,
  type WritableSignal,
} from '@angular/core';
import { CommonModule } from '@angular/common';
import { FormsModule } from '@angular/forms';
import { ActivatedRoute, Router, RouterLink } from '@angular/router';
import {
  IonModal,
  IonHeader,
  IonToolbar,
  IonTitle,
  IonButtons,
  IonButton,
  IonContent,
  IonList,
  IonListHeader,
  IonInput,
  IonItem,
  IonLabel,
  IonIcon,
  IonText,
  IonFooter,
  IonSegment,
  IonSegmentButton,
  IonRadioGroup,
  IonRadio,
  IonTextarea,
  AlertController,
  ToastController,
} from '@ionic/angular/standalone';
import { addIcons } from 'ionicons';
import {
  arrowUpOutline,
  closeOutline,
  contractOutline,
  expandOutline,
  folderOutline,
  documentOutline,
} from 'ionicons/icons';
import { AuthService } from '../../services/auth.service';
import { RelayTerminalService } from '../../services/relay-terminal.service';
import { MermaidZoomService } from '../../services/mermaid-zoom.service';
import { PendingImageAttachment } from '../../components/image-attachment-composer/image-attachment-composer.component';
import {
  JohnnyApiService,
  ChatAttachment,
  TerminalScreenComponent,
  TranscriptViewComponent,
  DiffViewComponent,
  MarkdownViewComponent,
  LifecycleBarComponent,
  StatusPillComponent,
  GitDiffView,
  AgentPlan,
  AgentPlanRun,
  AgentPlanEvent,
  FileContent,
  AiSession as SharedAiSession,
  AiMessage as SharedAiMessage,
  AiMessageDelta,
  AiChatComplete,
  DetectedCliTool,
  TerminalScreen,
  StreamEvent,
  HostFileEntry,
  TmuxSession,
} from '@johnnyone/ui';
import {
  PaneTab,
  paneTabOf,
  eventsAreStreaming,
  appendTranscriptEvent,
  diffStreamSubscriptions,
} from './terminal-transcript-tab';

// Re-export so existing/future importers of `PaneTab` from the page keep resolving.
export type { PaneTab } from './terminal-transcript-tab';
import {
  initiativeRows,
  lensSummary,
  lensSource,
  consolePaneFor,
  CONSOLE_SEGMENTS,
  InitiativeRow,
  LensChip,
  ConsoleSegment,
  LensSource,
} from './console-logic';
import { initiativeTimeline, TimelineEvent } from './initiative-events-logic';
import {
  resolvePrimarySessionId,
  initiativeTabOf,
  rawAttachNeeded,
} from './console-tabs-logic';
import {
  planCounts,
  docNavModel,
  defaultPlanDoc,
  phaseCards,
  planDocPath,
  taskStatusLabel,
  PlanCounts,
  DocNavEntry,
  PhaseCard,
  TaskStatusView,
} from './plan-tab-logic';
import {
  panelVisible,
  bannerModel,
  runButtonLabel,
  defaultSelectedPhaseId,
  defaultRunMode,
  phasePickerRows,
  validateComment,
  buildRunFromPhaseArgs,
  PhasePickerRow,
} from './run-resume-logic';
import { resolveSelectedInitiative } from './console-selection-logic';
import { consoleCaptureLines } from '../../../../../ui/src/components/terminal-screen/terminal-scroll-logic';
import { isPlainShellSurface } from '../../components/launcher-menu/launcher-logic';
import {
  clampRailWidth,
  clampRightWidth,
  consoleColumns as consoleColumnsTemplate,
  parseStoredWidth,
} from './console-layout-logic';
import { firstValueFrom, Subscription } from 'rxjs';
import { WORKSPACE_MOBILE_MEDIA_QUERY } from '../../workspace-responsive';
import {
  chooseSessionToSelect,
  reconcilePersistedWorkspaceState,
  type PaneLayout,
  type PersistedTerminalWorkspaceState,
} from './terminal-state-reconcile';

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
  attached_tmux: boolean;
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

// Register Ionicons used by the workspace-picker modal once at module load.
addIcons({
  'arrow-up-outline': arrowUpOutline,
  'close-outline': closeOutline,
  'contract-outline': contractOutline,
  'expand-outline': expandOutline,
  'folder-outline': folderOutline,
  'document-outline': documentOutline,
});

@Component({
  selector: 'app-terminal',
  standalone: true,
  imports: [
    CommonModule,
    FormsModule,
    RouterLink,
    TerminalScreenComponent,
    TranscriptViewComponent,
    DiffViewComponent,
    MarkdownViewComponent,
    LifecycleBarComponent,
    StatusPillComponent,
    IonModal,
    IonHeader,
    IonToolbar,
    IonTitle,
    IonButtons,
    IonButton,
    IonContent,
    IonList,
    IonListHeader,
    IonInput,
    IonItem,
    IonLabel,
    IonIcon,
    IonText,
    IonFooter,
    IonSegment,
    IonSegmentButton,
    IonRadioGroup,
    IonRadio,
    IonTextarea,
  ],
  templateUrl: './terminal.page.html',
  styleUrls: ['./terminal.page.scss'],
})
export class TerminalPage implements OnInit, AfterViewInit, OnDestroy {
  private static readonly SIDEBAR_WIDTH_KEY = 'johnnyone_desktop_sidebar_width';
  // P5: persisted console divider widths (johnnyone_* convention, mirrors SIDEBAR_WIDTH_KEY).
  private static readonly CONSOLE_RAIL_WIDTH_KEY = 'johnnyone_console_rail_width';
  private static readonly CONSOLE_RIGHT_WIDTH_KEY = 'johnnyone_console_right_width';
  private static readonly PANE_WORKSPACE_STATE_KEY = 'johnnyone_terminal_pane_workspace';
  private static readonly WORKSPACE_PADDING_PX = 12;
  private static readonly STREAM_FIRST_TOKEN_NOTICE_MS = 10_000;
  private static readonly STREAM_IDLE_NOTICE_MS = 8_000;
  /** Benign empty diff for a no-cwd / non-repo / failed-load session (overhaul P7, D11). */
  private static readonly EMPTY_DIFF: GitDiffView = {
    repoRoot: null,
    branch: null,
    clean: true,
    files: [],
  };
  private readonly api = inject(JohnnyApiService);
  private readonly auth = inject(AuthService);
  private readonly route = inject(ActivatedRoute);
  private readonly relayTerminal = inject(RelayTerminalService);
  // Public so the transcript tab template can wire `(svgZoom)="mermaidZoom.open(...)"`
  // — reuses the existing shared modal service (decision D4), no new zoom UI.
  readonly mermaidZoom = inject(MermaidZoomService);
  private readonly router = inject(Router);
  private readonly alertCtrl = inject(AlertController);
  private readonly toastCtrl = inject(ToastController);
  private readonly minSidebarWidth = 260;
  private readonly maxSidebarWidth = 560;
  private readonly defaultSidebarWidth = 360;
  private resizeMoveHandler: ((event: MouseEvent) => void) | null = null;
  private resizeUpHandler: (() => void) | null = null;
  private paneMoveHandler: ((event: PointerEvent) => void) | null = null;
  private paneUpHandler: (() => void) | null = null;
  private paneInteractionStart = { x: 0, y: 0, left: 0, top: 0, width: 0, height: 0 };
  private streamingStatusInterval: ReturnType<typeof setInterval> | null = null;
  /** Events-pane poll: refreshes the selected initiative's timeline while it's live. */
  private eventsPollInterval: ReturnType<typeof setInterval> | null = null;
  /** The initiative id the events pane currently tracks (dedupes redundant reloads/polls). */
  private eventsInitiativeId: string | null = null;
  private eventsStamp: string | null = null;
  /** Raw-terminal fallback poll: pulls the primary session's screen via the reliable `captureTerminal`
   *  request/response path (the push visual-stream doesn't deliver for coordinator-spawned agent
   *  sessions — planner/worker/reviewer aren't in listAiSessions), so the pane isn't black. */
  private primaryScreenPollInterval: ReturnType<typeof setInterval> | null = null;
  private primaryScreenSessionId: string | null = null;
  private sessionIdCopiedTimeout: ReturnType<typeof setTimeout> | null = null;
  private deltaSubscription: Subscription | null = null;
  private completeSubscription: Subscription | null = null;
  private terminalSubscription: Subscription | null = null;
  private sessionUpdateSubscription: Subscription | null = null;
  private sessionDeleteSubscription: Subscription | null = null;
  private relayErrorSubscription: Subscription | null = null;
  private streamEventsSubscription: Subscription | null = null;
  private resizeTerminalTimeout: ReturnType<typeof setTimeout> | null = null;
  private saveWorkspaceStateTimeout: ReturnType<typeof setTimeout> | null = null;
  private workspaceResizeObserver: ResizeObserver | null = null;
  private layoutBeforeFullscreen: Record<string, PaneLayout> = {};
  private compactWorkspaceMediaQuery: MediaQueryList | null = null;
  private compactWorkspaceListener: ((event: MediaQueryListEvent) => void) | null = null;
  private terminalVisualSubscriptions = new Set<string>();
  // Parallel "stream lane" mirroring the visual-subscription lifecycle (decision D6):
  // subscribe/unsubscribe the per-session structured StreamEvent channel at the same
  // points the screen-visual lane does.
  private streamSubscriptions = new Set<string>();
  private terminalVisualSync: Promise<void> = Promise.resolve();
  private queryParamSub: Subscription | null = null;
  private paramSub: Subscription | null = null;
  private readonly visibilityChangeHandler = () => {
    if (document.hidden) {
      this.enqueueTerminalVisualSync(() => this.unsubscribeAllTerminalVisuals());
    } else {
      this.enqueueTerminalVisualSync(() => this.syncTerminalVisualSubscriptions({ refresh: true }));
    }
  };
  private readonly pageShowHandler = (event: PageTransitionEvent) => {
    if (event.persisted) {
      void this.syncTerminalVisualSubscriptions({ refresh: true });
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
  newSessionTitle = signal('');
  tmuxSessions = signal<TmuxSession[]>([]);

  sidebarOpen = signal(true);
  sidebarWidth = signal(this.defaultSidebarWidth);
  isResizingSidebar = signal(false);
  sessionIdCopied = signal(false);
  terminalScreen = signal<TerminalScreen | null>(null);
  terminalScreens = signal<Record<string, TerminalScreen>>({});
  terminalError = signal<string | null>(null);

  // Per-session pane tab (Transcript default · Raw terminal; Plan/Diff reserved, D7)
  // and the accumulated structured stream feeding the Transcript surface (D6).
  paneTabs = signal<Record<string, PaneTab>>({});
  // Per-INITIATIVE console tab (P1): a DISTINCT map from the session-keyed `paneTabs` (D2), keyed by
  // initiative id, driving the four tabs in the center pane's per-initiative shell.
  initiativeTabs = signal<Record<string, PaneTab>>({});

  // Plan tab (P2, mock §03): the selected initiative's structured run + the currently-rendered doc.
  // Loaded lazily from the EXISTING `getAgentPlan` + `readFile` when the Plan tab first opens for an
  // initiative — no new GraphQL. Rendering delegates to `johnny-markdown-view` (no second renderer).
  planRun = signal<AgentPlanRun | null>(null);
  planSelectedDoc = signal<string>('overview.md');
  planDocMarkdown = signal<string>('');
  planLoading = signal<boolean>(false);
  planError = signal<string | null>(null);
  /** Guard so the plan is fetched once per initiative (re-fetches only when the initiative changes). */
  private planLoadedForInitiative: string | null = null;
  /** Per-session working-tree diff (overhaul P7, D10), loaded lazily on Diff-tab activation. */
  diffs = signal<Record<string, GitDiffView | null>>({});
  transcriptEvents = signal<Record<string, StreamEvent[]>>({});
  paneLayouts = signal<Record<string, PaneLayout>>({});
  closedPaneIds = signal<Set<string>>(new Set());
  paneX = signal(44);
  paneY = signal(34);
  paneWidth = signal(860);
  paneHeight = signal(560);
  isDraggingPane = signal(false);
  isResizingPane = signal(false);
  fullscreenPaneId = signal<string | null>(null);
  isCompactWorkspace = signal(false);

  // ── Initiative console (overhaul P8 §02, phase 04) ─────────────────────────
  // The Work console wraps this page's existing pane shell in a master-detail grid. All row/summary/
  // file/segment decisions delegate to the pure `console-logic.ts` (D3/D6/D7/D8); the primitives are the
  // P2 `johnny-status-pill`/`johnny-lifecycle-bar`. No transcript/diff/pane-shell code is forked.
  /** Initiatives from `listAgentPlans` (best-effort; empty when none/unauthorized). */
  protected readonly agentPlans = signal<AgentPlan[]>([]);
  /** The selected initiative id (drives the center lifecycle bar + right validation column). */
  protected readonly selectedInitiativeId = signal<string | null>(null);
  /** The `surface` query param (P4): `'shell'` opens the plain terminal surface (no initiative chrome).
   *  Read synchronously in `ngOnInit` so plain mode renders from first paint (no console flash). */
  protected readonly surfaceParam = signal<string | null>(null);
  /** "now" captured once per initiative load so `formatRelTime` stays deterministic. */
  protected readonly consoleNow = signal<string>(new Date(0).toISOString());
  /** §08 mobile segment (Transcript default). */
  protected readonly consoleSegment = signal<ConsoleSegment>('console');
  protected readonly consoleSegments = CONSOLE_SEGMENTS;

  /** Master-list rows (mock §02 left `.pane.rail`). */
  protected readonly consoleRows = computed<InitiativeRow[]>(() =>
    initiativeRows(this.agentPlans(), this.selectedInitiativeId(), this.consoleNow()),
  );
  /** The selected initiative record (for the lifecycle bar + validation config). */
  // C1: resolve the selection by plan-run `id` OR group `initiativeId` so a `?initiativeId=<id>`
  // deep-link renders the terminal instead of stranding on the empty state (console-selection-logic).
  protected readonly selectedInitiative = computed<AgentPlan | null>(() =>
    resolveSelectedInitiative(this.agentPlans(), this.selectedInitiativeId()),
  );
  // Mobile master-detail hides the list only when the selection actually RESOLVES to a plan. A
  // `recoverableEmpty` selection (set but unresolved) keeps the list/back affordance visible so the
  // user can recover instead of being stranded (C1).
  protected readonly hasResolvedSelection = computed<boolean>(() => !!this.selectedInitiative());
  /** The session the selected initiative's tabs display: worker ?? reviewer ?? briefing (P1/D2). */
  protected readonly primarySessionId = computed<string | null>(() =>
    resolvePrimarySessionId(this.selectedInitiative()),
  );
  /** The primary session record (for the Raw tab's terminal-screen title/provider), if in the list. */
  protected readonly primarySession = computed<SharedAiSession | null>(() => {
    const id = this.primarySessionId();
    return id ? this.aiSessions().find((s) => s.id === id) ?? null : null;
  });
  /** Active console tab for the selected initiative; defaults to Transcript. */
  protected readonly activeTab = computed<PaneTab>(() =>
    initiativeTabOf(this.initiativeTabs(), this.selectedInitiativeId() ?? ''),
  );

  // P5: resizable console divider widths, seeded from localStorage (clamped on read) and persisted on
  // drag-end. Bound to `.console` via the `--console-cols` custom property (see `consoleColumns`).
  protected readonly railWidth = signal<number>(
    this.loadConsoleWidth(TerminalPage.CONSOLE_RAIL_WIDTH_KEY, clampRailWidth),
  );
  protected readonly rightWidth = signal<number>(
    this.loadConsoleWidth(TerminalPage.CONSOLE_RIGHT_WIDTH_KEY, clampRightWidth),
  );
  /** `grid-template-columns` for the 3-pane console incl. the two divider tracks (pure T01 helper). */
  protected readonly consoleColumns = computed<string>(() =>
    consoleColumnsTemplate(this.railWidth(), this.rightWidth()),
  );
  /** True when the `.console` container is ≤760px wide — the collapsed single-column layout. Drives
   *  the inline grid to `1fr` (so stored widths don't leak) at the SAME threshold the `@container`
   *  query hides the dividers. A `container-type` element can't restyle itself via `@container`, so
   *  the collapse grid is bound inline instead. */
  protected readonly consoleCompact = signal<boolean>(false);
  private static readonly CONSOLE_COLLAPSE_PX = 760;
  private consoleResizeObserver: ResizeObserver | null = null;
  /** Cleanup for an in-flight divider drag (removes the window pointer listeners). */
  private consoleResizeCleanup: (() => void) | null = null;

  /** Plain shell surface: true ONLY when the route asks for it (`surface=shell` — set by the
   *  dedicated `/shells/:sessionId` route via `data.surface`, or a legacy `?surface=shell` query).
   *  Deliberately does NOT key off the auto-selected `currentSession`: bare `/terminal` may have a
   *  shell as the most-recent session, and keying off it wrongly flipped the initiative console into
   *  plain-shell mode. A shell always opens on `/shells/:id`, so the route flag is authoritative. */
  protected readonly plainShellMode = computed<boolean>(
    () => isPlainShellSurface(this.surfaceParam(), null),
  );

  // Plan-tab projections over `planRun` (P2, pure `plan-tab-logic`).
  protected readonly planNav = computed<DocNavEntry[]>(() => docNavModel(this.planRun()));
  protected readonly planCards = computed<PhaseCard[]>(() => phaseCards(this.planRun()));
  protected readonly planTotals = computed<PlanCounts>(() => planCounts(this.planRun()));
  /** True once a run with at least one phase is loaded (else the Plan tab shows the empty state). */
  protected readonly planHasContent = computed<boolean>(
    () => (this.planRun()?.phases?.length ?? 0) > 0,
  );

  // Run/Resume panel (console-features P01) — all decisions come from the pure `run-resume-logic`
  // seam; these signals hold only the form state the user edits. Pre-seeded from the run when the
  // Plan tab (re)loads via `applyPlanRun`.
  /** The phase the picker currently has selected (radio value). */
  protected readonly runFromPhaseId = signal<string | null>(null);
  /** The optional comment/guidance textarea value. */
  protected readonly runComment = signal<string>('');
  /** The run mode toggle: continue (from here) / single (only this phase). */
  protected readonly runMode = signal<'continue' | 'single'>(defaultRunMode());
  /** True while a run/resume submit is in flight (disables the button). */
  protected readonly runSubmitting = signal<boolean>(false);
  /** Whole-panel visibility — dev run with ≥1 phase, else hidden. */
  protected readonly runResumeVisible = computed<boolean>(() => panelVisible(this.planRun()));
  /** Error banner model (shown above the panel when the run is paused). */
  protected readonly runBanner = computed(() => bannerModel(this.planRun()));
  /** Primary button label: Run / Resume. */
  protected readonly runBtnLabel = computed<'Run' | 'Resume'>(() => runButtonLabel(this.planRun()));
  /** Radio rows for the Start-at-phase picker, reactive to the current selection. */
  protected readonly runPhaseRows = computed<PhasePickerRow[]>(() =>
    phasePickerRows(this.planRun(), this.runFromPhaseId()),
  );
  /** Comment validation (drives the inline error + button disabled state). */
  protected readonly runCommentValid = computed(() => validateComment(this.runComment()));
  /** Right-column validation lens summary (mock 658-673), reusing P7's parser. */
  protected readonly consoleLenses = computed<LensChip[]>(() =>
    lensSummary(this.selectedInitiative()?.validationConfig ?? null),
  );
  /** Whether the selected initiative's lenses are its own saved config or the shared default template
   *  (P3) — drives the "configured" / "default template" badge. Reads the per-initiative field. */
  protected readonly consoleLensSource = computed<LensSource>(() =>
    lensSource(this.selectedInitiative()?.validationConfig ?? null),
  );
  /** Right-column SDLC Events timeline (replaces the old Host-files pane). The initiative-level events
   *  (planning-run + development-run merged, oldest-first) loaded for the selected initiative. */
  protected readonly initiativeEvents = signal<AgentPlanEvent[]>([]);
  protected readonly eventsLoading = signal(false);
  /** Projected timeline rows (stage/lens/verdict/normalized-ISO) — reverse to newest-first for display. */
  protected readonly consoleTimeline = computed<TimelineEvent[]>(() =>
    [...initiativeTimeline(this.initiativeEvents())].reverse(),
  );
  /** Which pane the §08 mobile switcher currently shows. */
  protected readonly mobileConsolePane = computed(() => consolePaneFor(this.consoleSegment()));

  @ViewChild('terminalWorkspace', { static: true })
  private terminalWorkspace?: ElementRef<HTMLElement>;
  @ViewChild('consoleRoot', { static: true })
  private consoleRoot?: ElementRef<HTMLElement>;
  pendingAttachmentsBySession = signal<Record<string, PendingImageAttachment[]>>({});
  sendingAttachmentsBySession = signal<Record<string, boolean>>({});

  // Computed
  filteredSessions = computed(() => {
    const query = this.searchQuery.toLowerCase();
    const all = this.sessions();
    if (!query) return all;
    return all.filter(s => s.title.toLowerCase().includes(query));
  });

  aiSessions = computed<SharedAiSession[]>(() =>
    // Source of truth is the API session list. Never hide an active terminal
    // behind the local `closedPaneIds` set — that's a legacy multi-pane artifact
    // that can desync from the server and empty the entire tab bar.
    this.filteredSessions().map(s => this.mapSession(s))
  );

  visiblePaneSessions = computed<SharedAiSession[]>(() => {
    const currentId = this.currentSession()?.id;
    if (!currentId) return [];
    return this.aiSessions().filter((session) => session.id === currentId);
  });

  // Persisted, manually-arrangeable tab order. The tab bar renders by this order
  // (not by updated_at), so selecting a tab never rearranges them, and drag-drop
  // reorders persist. New sessions append; removed ones drop (kept in sync below).
  private static readonly TAB_ORDER_KEY = 'johnnyone_terminal_tab_order';
  tabOrder = signal<string[]>([]);
  draggingTabId: string | null = null;

  orderedAiSessions = computed<SharedAiSession[]>(() => {
    const list = this.aiSessions();
    const byId = new Map(list.map((s) => [s.id, s] as const));
    const ordered: SharedAiSession[] = [];
    for (const id of this.tabOrder()) {
      const s = byId.get(id);
      if (s) {
        ordered.push(s);
        byId.delete(id);
      }
    }
    // Sessions not yet in tabOrder (created since the last sync) go to the end.
    for (const s of list) {
      if (byId.has(s.id)) ordered.push(s);
    }
    return ordered;
  });

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

  constructor() {
    // Keep the persisted tab order in sync with the live session set: append new
    // sessions, drop removed ones, preserve the manual (drag) arrangement.
    // Selecting a tab never reorders — orderedAiSessions is driven by tabOrder.
    effect(
      () => {
        const ids = this.aiSessions().map((s) => s.id);
        const idSet = new Set(ids);
        const current = this.tabOrder();
        const kept = current.filter((id) => idSet.has(id));
        const added = ids.filter((id) => !current.includes(id));
        if (added.length > 0 || kept.length !== current.length) {
          const next = [...kept, ...added];
          this.tabOrder.set(next);
          this.persistTabOrder(next);
        }
      },
      { allowSignalWrites: true },
    );

    // P1: when the selected initiative's primary session changes, keep its visual + stream lanes
    // subscribed on the SAME machinery as the visible pane (via `lanedSessionIds`), so its
    // Transcript/Raw populate without attaching a pane. Refresh so a live screen shows immediately.
    effect(() => {
      this.primarySessionId();
      // Load the initiative's primary session into `sessions` (like /shells' selectSession does) so
      // its live screen is RETAINED (in activeIds) instead of purged — otherwise a briefing agent
      // session that isn't in listAiSessions renders a blank Raw terminal even though it's streaming.
      void this.ensurePrimarySessionLoaded();
      this.enqueueTerminalVisualSync(() =>
        this.syncTerminalVisualSubscriptions({ refresh: true }),
      );
      void this.syncStreamSubscriptions();
    });

    // P2: lazily load the Plan tab's document model the first time the Plan tab is opened for the
    // selected initiative (and again when the selection changes while on the Plan tab). Reuses
    // `getAgentPlan` + `readFile` — no new GraphQL. allowSignalWrites: `ensurePlanLoaded` resets the
    // plan signals synchronously before its first await.
    effect(
      () => {
        const isPlan = this.activeTab() === 'plan';
        const init = this.selectedInitiative();
        if (isPlan && init) {
          void this.ensurePlanLoaded();
        }
      },
      { allowSignalWrites: true },
    );

    // Events pane: (re)load the selected initiative's full SDLC timeline whenever the selection or its
    // `updatedAt` changes (the latter bumps as the run progresses / after a mutation), and run a gentle
    // poll while the initiative is live so the timeline follows planning → review loop → done without a
    // manual refresh. Keyed on `initiativeId` (the group), not the plan-run id.
    effect(
      () => {
        const init = this.selectedInitiative();
        // Track `updatedAt` so a status change re-reads events even if the id is unchanged.
        const stamp = init?.updatedAt;
        const initiativeId = init?.initiativeId ?? null;
        const isLive = !!init && init.initiativeStatus !== 'done';
        this.syncEventsWatch(initiativeId, stamp, isLive);
      },
      { allowSignalWrites: true },
    );

    // Raw-terminal fallback: while the Raw tab is showing a primary agent session (planner/worker/
    // reviewer), keep its screen fresh via `captureTerminal` — the push visual-stream doesn't deliver
    // for these coordinator-spawned sessions, so without this the pane is black.
    effect(
      () => {
        const onRaw = this.activeTab() === 'raw';
        const id = this.primarySessionId();
        this.syncPrimaryScreenPoll(onRaw ? id : null);
      },
      { allowSignalWrites: true },
    );
  }

  private loadTabOrder(): void {
    try {
      const raw = localStorage.getItem(TerminalPage.TAB_ORDER_KEY);
      if (!raw) return;
      const parsed = JSON.parse(raw);
      if (Array.isArray(parsed)) {
        this.tabOrder.set(parsed.filter((x): x is string => typeof x === 'string'));
      }
    } catch {
      // best-effort
    }
  }

  private persistTabOrder(order: string[]): void {
    try {
      localStorage.setItem(TerminalPage.TAB_ORDER_KEY, JSON.stringify(order));
    } catch {
      // best-effort
    }
  }

  onTabDragStart(sessionId: string): void {
    this.draggingTabId = sessionId;
  }

  onTabDragOver(event: DragEvent): void {
    event.preventDefault(); // permit drop
  }

  // Drop the dragged tab before the target tab and persist the new order.
  onTabDrop(targetId: string): void {
    const from = this.draggingTabId;
    this.draggingTabId = null;
    if (!from || from === targetId) return;
    const order = [...this.tabOrder()];
    const fromIdx = order.indexOf(from);
    const toIdx = order.indexOf(targetId);
    if (fromIdx < 0 || toIdx < 0) return;
    order.splice(fromIdx, 1);
    order.splice(toIdx, 0, from);
    this.tabOrder.set(order);
    this.persistTabOrder(order);
  }

  ngOnInit(): void {
    this.setupCompactWorkspaceMode();
    this.loadSidebarWidth();
    this.loadTabOrder();
    this.loadPersistedWorkspaceState();
    this.subscribeToTerminalEvents();
    this.subscribeToRelayErrorEvents();
    // P4: read `surface` synchronously so plain-shell mode is decided before first paint (no
    // console-chrome flash). The `/shells/:sessionId` route sets `data.surface = 'shell'`; a legacy
    // `?surface=shell` query is still honored as a fallback. `routeSurface` (static for the activated
    // route) is the fallback for the query-param subscription below so a shell open stays plain even
    // though the URL carries no `surface` query.
    const routeSurface = (this.route.snapshot.data['surface'] as string | undefined) ?? null;
    this.surfaceParam.set(routeSurface ?? this.route.snapshot.queryParamMap.get('surface'));
    // Deep-link: select the linked initiative + tab from the URL BEFORE the list loads, so
    // `loadInitiatives` (which otherwise defaults to the first initiative) honors the link.
    const linkedInitiative = this.route.snapshot.queryParamMap.get('initiativeId');
    if (linkedInitiative) {
      this.selectedInitiativeId.set(linkedInitiative);
      const linkedTab = this.route.snapshot.queryParamMap.get('tab');
      if (this.isPaneTab(linkedTab)) {
        this.initiativeTabs.update((m) => ({ ...m, [linkedInitiative]: linkedTab }));
      }
    }
    // Session id arrives as a `:sessionId` path segment on `/shells/:sessionId`, or as `?sessionId` on
    // the initiative console — accept either.
    const initialSessionId =
      this.route.snapshot.paramMap.get('sessionId') ??
      this.route.snapshot.queryParamMap.get('sessionId') ??
      undefined;
    void this.loadSessions(initialSessionId);
    // Skip the initiative master-list entirely in plain-shell mode — a shell has no initiative chrome.
    if (!this.plainShellMode()) {
      void this.loadInitiatives();
    }
    void this.detectTools();
    void this.loadLastWorkingDirectory();
    this.subscribeToRelaySessionEvents();

    // React to the `:sessionId` PATH segment changing while this component is reused (e.g. navigating
    // from one `/shells/:id` to another). Mirrors the `?sessionId` handling below.
    if (!this.paramSub) {
      this.paramSub = this.route.paramMap.subscribe((map) => {
        const sid = map.get('sessionId');
        if (!sid || sid === this.currentSession()?.id) return;
        if (this.sessions().some((s) => s.id === sid)) {
          void this.selectSession(sid);
        } else {
          void this.loadSessions(sid);
        }
      });
    }

    // Support ?sessionId deep links after initial mount (e.g. e2e harness goto after create, or direct nav)
    if (!this.queryParamSub) {
      this.queryParamSub = this.route.queryParamMap.subscribe((map) => {
        // Keep plain-shell mode in sync with later navigations (P4). Fall back to the route's static
        // `data.surface` so a `/shells/:sessionId` open (no `surface` query) stays plain.
        this.surfaceParam.set(map.get('surface') ?? routeSurface);
        const sid = map.get('sessionId');
        if (sid) {
          const current = this.currentSession()?.id;
          if (sid !== current) {
            const has = this.sessions().some((s) => s.id === sid);
            if (has) {
              void this.selectSession(sid);
            } else {
              void this.loadSessions(sid);
            }
          }
        }
        // Deep-link: react to `?initiativeId`/`?tab` changing (back/forward, external links).
        // Set the signals DIRECTLY (not via the setters, which push back to the URL) — the
        // diff checks below make this a no-op when the URL merely echoes our own push.
        const linkedInit = map.get('initiativeId');
        if (linkedInit && linkedInit !== this.selectedInitiativeId()) {
          this.selectedInitiativeId.set(linkedInit);
        }
        const active = this.selectedInitiativeId();
        const linkedTab = map.get('tab');
        if (active && this.isPaneTab(linkedTab) && this.initiativeTab(active) !== linkedTab) {
          this.initiativeTabs.update((m) => ({ ...m, [active]: linkedTab }));
        }
      });
    }
    document.addEventListener('visibilitychange', this.visibilityChangeHandler);
    window.addEventListener('pageshow', this.pageShowHandler);
    void this.relayTerminal.connect();
  }

  ngAfterViewInit(): void {
    this.setupWorkspaceResizeObserver();
    this.setupConsoleCollapseObserver();
    if (this.sessions().length > 0) {
      this.restorePaneLayoutsFromStorage(this.sessions());
    }
  }

  /** Track the console's AVAILABLE width so it collapses to a single column at
   *  `CONSOLE_COLLAPSE_PX` (P5) — mirrors `@container console (max-width: 760px)`.
   *
   *  #10 fix: observe `.console`'s PARENT (the display:block page host filling the
   *  router-outlet content area — a stable width unaffected by the collapse), NOT `.console`
   *  itself. The collapse binding shrinks `.console`, so observing the element being restyled
   *  feedback-loops: once collapsed, `.console` measures ≤ threshold forever and can never
   *  climb back out (opening a second terminal permanently latched the narrow layout). The
   *  parent's width comes from the app shell (viewport − nav rail) and does not shrink. */
  private setupConsoleCollapseObserver(): void {
    const el = this.consoleRoot?.nativeElement;
    // Measure a stable ancestor, never the collapsing `.console` itself (else it latches).
    const target = el?.parentElement ?? el;
    if (!target || typeof ResizeObserver === 'undefined') return;
    const update = (width: number): void => {
      this.consoleCompact.set(width > 0 && width <= TerminalPage.CONSOLE_COLLAPSE_PX);
    };
    update(target.getBoundingClientRect().width);
    this.consoleResizeObserver?.disconnect();
    this.consoleResizeObserver = new ResizeObserver((entries) => {
      for (const entry of entries) {
        update(entry.contentRect.width);
      }
    });
    this.consoleResizeObserver.observe(target);
  }

  @HostListener('document:keydown.escape')
  onEscapeKey(): void {
    if (this.fullscreenPaneId()) {
      this.closePaneFullscreen();
    }
  }

  ngOnDestroy(): void {
    this.flushWorkspaceState();
    this.workspaceResizeObserver?.disconnect();
    this.workspaceResizeObserver = null;
    if (this.saveWorkspaceStateTimeout) {
      clearTimeout(this.saveWorkspaceStateTimeout);
      this.saveWorkspaceStateTimeout = null;
    }
    document.removeEventListener('visibilitychange', this.visibilityChangeHandler);
    window.removeEventListener('pageshow', this.pageShowHandler);
    this.consoleResizeCleanup?.(); // remove any in-flight divider drag listeners (P5)
    this.consoleResizeCleanup = null;
    this.consoleResizeObserver?.disconnect();
    this.consoleResizeObserver = null;
    this.queryParamSub?.unsubscribe();
    this.queryParamSub = null;
    this.paramSub?.unsubscribe();
    this.paramSub = null;
    void this.unsubscribeAllTerminalVisuals();
    void this.unsubscribeAllStreamLanes();
    this.teardownChatSubscriptions();
    this.teardownTerminalSubscription();
    this.teardownRelaySessionEvents();
    this.clearSessionIdCopiedTimeout();
    this.resetStreamingStatus();
    this.stopEventsPoll();
    this.stopPrimaryScreenPoll();
    this.stopSidebarResize();
    this.stopPaneInteraction();
    this.teardownCompactWorkspaceMode();
    this.clearAllPendingAttachments();
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
      attachedTmux: s.attached_tmux,
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
      attached_tmux: session.attachedTmux ?? false,
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
      // Reconcile ALL persisted/cached Terminal state against the authoritative active
      // list BEFORE layout restore or selection, so a deleted/archived session can never
      // pin a pane, hold the screen cache, or be selected across a reload.
      this.reconcilePersistedTerminalState(sortedSessions);
      this.restorePaneLayoutsFromStorage(sortedSessions);
      await this.syncTerminalVisualSubscriptions();
      void this.syncStreamSubscriptions();

      const toSelect = chooseSessionToSelect(sortedSessions, {
        targetId: targetSessionId,
        currentId: this.currentSession()?.id,
      });
      if (toSelect) {
        await this.selectSession(toSelect);
      } else {
        this.currentSession.set(null);
        this.messages.set([]);
      }
    } catch (err) {
      console.error('Failed to load sessions:', err);
    }
  }

  /**
   * Single entry point for reconciling every persisted/cached Terminal store against the
   * authoritative active session list. Drops any entry whose session id is not active from:
   *   1. the in-memory signals (`terminalScreens`, `paneLayouts`, `closedPaneIds`) — via the
   *      reused `removeInactivePaneState`;
   *   2. the persisted `johnnyone_terminal_pane_workspace` localStorage blob (rewritten or
   *      removed so the corruption cannot reload); and
   *   3. the screen cache, via `relayTerminal.retainCachedScreens`.
   * Called from `loadSessions` after the active list is fetched and before layout restore /
   * selection, so a dead/deleted session can never pin or corrupt the Terminal UI.
   */
  private reconcilePersistedTerminalState(activeSessions: Session[]): void {
    // Retain the console's laned sessions (the selected initiative's primary session + visible panes)
    // in addition to the AI-session list. A briefing initiative's agent session may not appear in
    // `listAiSessions`, and without this its live screen would be purged here → the Raw terminal
    // renders blank even though the desktop is publishing it.
    const activeIds = new Set([
      ...activeSessions.map((s) => s.id),
      ...this.lanedSessionIds(),
    ]);

    // 1. In-memory signals — reuse the existing prune (terminalScreens + paneLayouts + closedPaneIds).
    this.removeInactivePaneState(activeIds);

    // 2. Persisted workspace blob — rewrite cleaned state, or remove it when fully stale.
    try {
      const persisted = this.readPersistedWorkspaceState();
      const { next, changed } = reconcilePersistedWorkspaceState(persisted, activeIds);
      if (changed) {
        if (next) {
          localStorage.setItem(TerminalPage.PANE_WORKSPACE_STATE_KEY, JSON.stringify(next));
        } else {
          localStorage.removeItem(TerminalPage.PANE_WORKSPACE_STATE_KEY);
        }
      }
    } catch {
      // Ignore localStorage read/write failures (match the other workspace-state methods).
    }

    // 3. Screen cache — prune via the relay boundary (never reach into the cache service directly).
    this.relayTerminal.retainCachedScreens(activeIds);
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
          this.restorePaneLayoutsFromStorage(this.sessions());
          void this.syncTerminalVisualSubscriptions();
          void this.syncStreamSubscriptions();
        } else {
          void this.unsubscribeTerminalVisual(mapped.id);
          void this.unsubscribeStreamLane(mapped.id);
          this.dropTranscriptState(mapped.id);
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
        void this.unsubscribeStreamLane(sessionId);
        this.dropTranscriptState(sessionId);
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
    this.relayTerminal.forgetCachedScreen(id);
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
    const start = this.workingDirectory().trim() || '~';
    this.newSessionBrowsePath.set(start);
    this.newSessionBrowserError.set(null);
    this.newSessionTitle.set('');
    this.tmuxSessions.set([]);
    this.newSessionBrowserOpen.set(true);
    await this.loadNewSessionEntries(start);
    // Load attachable external tmux sessions (kloo, llm-app, …) for the picker.
    try {
      this.tmuxSessions.set(await firstValueFrom(this.api.listTmuxSessions()));
    } catch (err) {
      console.error('Failed to list tmux sessions:', err);
    }
  }

  /** Create a session that ATTACHES to an existing external tmux session. */
  async attachTmuxSession(name: string): Promise<void> {
    this.newSessionBrowserOpen.set(false);
    try {
      const session = this.mapApiSessionToState(
        await firstValueFrom(
          this.api.createSession({
            title: this.newSessionTitle().trim() || name,
            tmuxSessionName: name,
          }),
        ),
      );
      this.upsertSession(session);
      await this.selectSession(session.id);
    } catch (err) {
      console.error('Failed to attach tmux session:', err);
      this.terminalError.set(String(err));
    }
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
        provider: 'shell',
        title: this.newSessionTitle().trim() || undefined,
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

  /** Ensure the selected initiative's primary session is present in `sessions` (so it survives the
   *  reconcile purge and its terminal screen renders). Mirrors the load part of `selectSession`
   *  WITHOUT taking over `currentSession` — the console just needs the session known + retained. */
  private async ensurePrimarySessionLoaded(): Promise<void> {
    const id = this.primarySessionId();
    if (!id || this.sessions().some((s) => s.id === id)) return;
    try {
      const session = this.mapApiSessionToState(await firstValueFrom(this.api.getSession(id)));
      this.upsertSession(session);
      this.hydrateTerminalScreenFromCache(id);
    } catch (err) {
      console.error('Failed to load initiative primary session:', err);
    }
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
      this.hydrateTerminalScreenFromCache(id);
      await this.syncTerminalVisualSubscriptions();
      void this.syncStreamSubscriptions();

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
    const session = this.currentSession();
    if (!session) return;
    const files = Array.from(event.clipboardData?.files ?? []).filter((file) =>
      file.type.startsWith('image/')
    );
    if (files.length === 0) return;
    event.preventDefault();
    this.addPendingImageFiles(session.id, files);
  }

  onWorkspaceDragOver(event: DragEvent): void {
    if (this.dragEventHasImage(event)) {
      event.preventDefault();
    }
  }

  onWorkspaceDrop(event: DragEvent): void {
    const session = this.currentSession();
    if (!session) return;
    const files = Array.from(event.dataTransfer?.files ?? []).filter((file) =>
      file.type.startsWith('image/')
    );
    if (files.length === 0) return;
    event.preventDefault();
    this.addPendingImageFiles(session.id, files);
  }

  pendingAttachmentsForSession(sessionId: string): PendingImageAttachment[] {
    return this.pendingAttachmentsBySession()[sessionId] ?? [];
  }

  isSendingAttachmentsForSession(sessionId: string): boolean {
    return !!this.sendingAttachmentsBySession()[sessionId];
  }

  removePendingAttachment(sessionId: string, id: string): void {
    const current = this.pendingAttachmentsForSession(sessionId);
    this.pendingAttachmentsBySession.update((bySession) => {
      const target = current.find((item) => item.id === id);
      if (target) URL.revokeObjectURL(target.previewUrl);
      const nextItems = current.filter((item) => item.id !== id);
      return this.setSessionAttachments(bySession, sessionId, nextItems);
    });
  }

  async sendAttachmentMessage(sessionId: string, message: string): Promise<void> {
    const attachments = this.pendingAttachmentsForSession(sessionId);
    const text = message.trim();
    if (this.isSendingAttachmentsForSession(sessionId) || (!text && attachments.length === 0)) return;

    if (attachments.length === 0) {
      await this.relayTerminal.sendInput(sessionId, `${text}\r`);
      return;
    }

    this.setSendingAttachments(sessionId, true);
    try {
      const uploaded: ChatAttachment[] = [];
      for (const item of attachments) {
        uploaded.push(
          await firstValueFrom(this.api.createChatAttachment({
            sessionId,
            originalName: item.file.name || 'clipboard-image.png',
            contentType: item.file.type || 'image/png',
            dataBase64: await this.fileToBase64(item.file),
          }))
        );
      }

      await this.relayTerminal.sendInputWithAttachments(
        sessionId,
        `${text || 'Please review the attached image.'}\r`,
        uploaded.map((attachment) => ({
          id: attachment.id,
          originalName: attachment.originalName,
          contentType: attachment.contentType,
          size: attachment.size,
        })),
      );

      this.clearPendingAttachments(sessionId);
    } catch (err) {
      console.error('Failed to send image attachment:', err);
    } finally {
      this.setSendingAttachments(sessionId, false);
    }
  }

  addPendingImageFiles(sessionId: string, files: File[]): void {
    const items = files.map((file) => ({
      id: crypto.randomUUID(),
      file,
      previewUrl: URL.createObjectURL(file),
    }));
    this.pendingAttachmentsBySession.update((bySession) => ({
      ...bySession,
      [sessionId]: [...(bySession[sessionId] ?? []), ...items],
    }));
  }

  private clearPendingAttachments(sessionId: string): void {
    for (const item of this.pendingAttachmentsForSession(sessionId)) {
      URL.revokeObjectURL(item.previewUrl);
    }
    this.pendingAttachmentsBySession.update((bySession) => this.setSessionAttachments(bySession, sessionId, []));
  }

  private clearAllPendingAttachments(): void {
    for (const items of Object.values(this.pendingAttachmentsBySession())) {
      for (const item of items) {
        URL.revokeObjectURL(item.previewUrl);
      }
    }
    this.pendingAttachmentsBySession.set({});
  }

  private setSessionAttachments(
    bySession: Record<string, PendingImageAttachment[]>,
    sessionId: string,
    items: PendingImageAttachment[],
  ): Record<string, PendingImageAttachment[]> {
    const next = { ...bySession };
    if (items.length === 0) {
      delete next[sessionId];
    } else {
      next[sessionId] = items;
    }
    return next;
  }

  private setSendingAttachments(sessionId: string, sending: boolean): void {
    this.sendingAttachmentsBySession.update((current) => {
      const next = { ...current };
      if (sending) {
        next[sessionId] = true;
      } else {
        delete next[sessionId];
      }
      return next;
    });
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

  private hydrateTerminalScreenFromCache(sessionId: string): void {
    if (this.terminalScreens()[sessionId]) return;

    const cached = this.relayTerminal.cachedScreen(sessionId);
    if (!cached) return;

    this.terminalScreens.update((screens) => ({ ...screens, [sessionId]: cached }));
    if (this.currentSession()?.id === sessionId) {
      this.terminalScreen.set(cached);
    }
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

    // Parallel structured-stream lane feeding the Transcript tab (D6). Accumulate
    // per session, bounded to MAX_TRANSCRIPT_EVENTS to cap long-session growth.
    if (!this.streamEventsSubscription) {
      this.streamEventsSubscription = this.relayTerminal.streamEvents().subscribe((event) => {
        this.transcriptEvents.update((bySession) => appendTranscriptEvent(bySession, event));
      });
    }
  }

  private teardownTerminalSubscription(): void {
    this.terminalSubscription?.unsubscribe();
    this.terminalSubscription = null;
    this.streamEventsSubscription?.unsubscribe();
    this.streamEventsSubscription = null;
    this.relayErrorSubscription?.unsubscribe();
    this.relayErrorSubscription = null;
    if (this.resizeTerminalTimeout) {
      clearTimeout(this.resizeTerminalTimeout);
      this.resizeTerminalTimeout = null;
    }
  }

  private subscribeToRelayErrorEvents(): void {
    if (this.relayErrorSubscription) return;
    // Surface service-level bound failures etc into the existing terminalError UI channel
    this.relayErrorSubscription = this.relayTerminal.errors().subscribe({
      next: (err) => this.terminalError.set(String(err)),
      error: (e) => console.error('relay error sub:', e),
    });
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

  /**
   * Sessions whose screen-visual + structured-stream lanes must stay live: the visible pane(s) PLUS
   * the selected initiative's primary session (P1), so the console tabs' Transcript/Raw populate even
   * when the primary session isn't the current pane. Reuses the existing lanes — no new channel.
   */
  private lanedSessionIds(): Set<string> {
    const ids = new Set(this.visiblePaneSessions().map((session) => session.id));
    const primary = this.primarySessionId();
    if (primary) ids.add(primary);
    return ids;
  }

  private async syncTerminalVisualSubscriptions(options?: { refresh?: boolean }): Promise<void> {
    if (document.hidden) return;
    const visibleIds = this.lanedSessionIds();

    for (const sessionId of Array.from(this.terminalVisualSubscriptions)) {
      if (!visibleIds.has(sessionId)) {
        await this.unsubscribeTerminalVisual(sessionId);
      }
    }

    for (const sessionId of visibleIds) {
      await this.subscribeTerminalVisual(sessionId);
      if (options?.refresh) {
        await this.relayTerminal.refreshVisual(sessionId);
      }
    }
  }

  private enqueueTerminalVisualSync(task: () => Promise<void>): void {
    this.terminalVisualSync = this.terminalVisualSync
      .then(task)
      .catch((err) => {
        console.error('Terminal visual sync failed:', err);
      });
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

  // ── Transcript tab state + stream lane ─────────────────────────────────

  /** Active tab for a pane; defaults to the Transcript view (D6). */
  paneTab(id: string): PaneTab {
    return paneTabOf(this.paneTabs(), id);
  }

  setPaneTab(id: string, tab: PaneTab): void {
    this.paneTabs.update((m) => ({ ...m, [id]: tab }));
  }

  /** Active console tab for an initiative; defaults to Transcript (P1, distinct from `paneTab`). */
  initiativeTab(id: string): PaneTab {
    return initiativeTabOf(this.initiativeTabs(), id);
  }

  setInitiativeTab(id: string | null, tab: PaneTab): void {
    if (!id) return;
    this.initiativeTabs.update((m) => ({ ...m, [id]: tab }));
    if (id === this.selectedInitiativeId()) this.syncConsoleUrl();
  }

  /** Deep-linking: mirror the selected initiative + its active tab into the URL
   *  (`?initiativeId&tab`) so the console is linkable and back/forward works. Loop-safe: the
   *  query-param subscription only re-applies when the URL value differs from the signal, and we
   *  always set the signal BEFORE pushing here, so the echo is a no-op. Plain-shell mode (a raw
   *  shell) carries no console/initiative state, so it never writes these params. */
  private syncConsoleUrl(): void {
    if (this.plainShellMode()) return;
    const initiativeId = this.selectedInitiativeId();
    if (!initiativeId) return;
    void this.router.navigate([], {
      relativeTo: this.route,
      queryParams: { initiativeId, tab: this.initiativeTab(initiativeId) },
      queryParamsHandling: 'merge',
      replaceUrl: true,
    });
  }

  private isPaneTab(value: string | null): value is PaneTab {
    return value === 'raw' || value === 'plan' || value === 'diff';
  }

  /** Template wrapper for the pure predicate — the Raw tab shows the inline attach card when true. */
  rawAttachNeeded(primarySessionId: string | null, hasScreen: boolean): boolean {
    return rawAttachNeeded(primarySessionId, hasScreen);
  }

  /**
   * Diff view for the selected initiative's Diff tab. Prefers the primary session's diff (keyed by
   * session id, as `loadDiff` stores it); for a sessionless initiative it falls back to the
   * workspace-path diff keyed by the initiative id.
   */
  initiativeDiffView(): GitDiffView | null {
    const key = this.primarySessionId() ?? this.selectedInitiativeId();
    return key ? this.diffs()[key] ?? null : null;
  }

  /**
   * Load the selected initiative's working-tree diff on Diff-tab activation. Reuses `loadDiff` when a
   * primary session exists in the list; otherwise diffs the initiative's `workspacePath` via the
   * existing `api.gitDiff`, keyed by the primary session id when known, else the initiative id.
   */
  async openInitiativeDiff(): Promise<void> {
    const sid = this.primarySessionId();
    if (sid) {
      const session = this.aiSessions().find((s) => s.id === sid);
      if (session) {
        await this.loadDiff(session);
        return;
      }
    }
    const init = this.selectedInitiative();
    const dir = init?.workspacePath?.trim();
    const key = sid ?? init?.id;
    if (!key || !dir) return;
    try {
      const view = await firstValueFrom(this.api.gitDiff(dir));
      this.diffs.update((m) => ({ ...m, [key]: view }));
    } catch {
      this.diffs.update((m) => ({ ...m, [key]: TerminalPage.EMPTY_DIFF }));
    }
  }

  // ── Plan tab (P2, §03) ─────────────────────────────────────────────────

  /** Template wrapper for the pure status-chip mapping (label + class token). */
  taskStatus(status: string): TaskStatusView {
    return taskStatusLabel(status);
  }

  /** Decode a `readFile` result to text (host returns utf8; base64 tolerated defensively). */
  private decodeFileContent(fc: FileContent): string {
    if (fc.encoding === 'base64') {
      try {
        return atob(fc.content ?? '');
      } catch {
        return '';
      }
    }
    return fc.content ?? '';
  }

  /**
   * Lazily load the selected initiative's plan when the Plan tab first opens for it (once per
   * initiative). Reuses the EXISTING `getAgentPlan` (structured phases/tasks) + `readFile` (markdown
   * bodies) — no new GraphQL. A plan-less initiative (e.g. briefing) or a failed fetch resolves to a
   * benign empty state, never a crash.
   */
  private async ensurePlanLoaded(): Promise<void> {
    const init = this.selectedInitiative();
    if (!init || this.planLoadedForInitiative === init.id) return;
    this.planLoadedForInitiative = init.id;
    // Reset per-initiative plan state on (re)load. Default to the planning artifact (plan.md) so the
    // tab isn't empty during planning; switched to overview.md below once the run reports phases.
    this.planRun.set(null);
    this.planSelectedDoc.set('plan.md');
    this.planDocMarkdown.set('');
    this.planError.set(null);
    if (!init.planPath?.trim()) return; // benign empty (no plan to read)
    this.planLoading.set(true);
    try {
      const run = await firstValueFrom(this.api.getAgentPlan(init.id));
      this.applyPlanRun(run ?? null);
      // Stage-aware default: overview.md when the plan has phases (development), else plan.md (planning).
      await this.loadPlanDoc(defaultPlanDoc(run));
    } catch {
      this.applyPlanRun(null); // benign empty state, no crash
    } finally {
      this.planLoading.set(false);
    }
  }

  /** Load one navigator doc's markdown via the safe fixed-suffix path builder (T01). */
  private async loadPlanDoc(sel: string): Promise<void> {
    this.planSelectedDoc.set(sel);
    const path = planDocPath(this.selectedInitiative()?.planPath, sel);
    if (!path) {
      this.planDocMarkdown.set('');
      return;
    }
    try {
      const fc = await firstValueFrom(this.api.readFile(path));
      this.planDocMarkdown.set(this.decodeFileContent(fc));
      this.planError.set(null);
    } catch {
      this.planDocMarkdown.set('');
      this.planError.set(`Could not read ${sel}`);
    }
  }

  /** Doc-navigator click: switch the rendered doc (overview.md / status.md / a phase's overview). */
  openPlanDoc(sel: string): void {
    void this.loadPlanDoc(sel);
  }

  /**
   * Load the initiative master-list for the console (overhaul P8, phase 04). Best-effort: a failure
   * (unauthorized / host down) leaves an empty list and the benign empty state — the console degrades to
   * the plain pane shell. `consoleNow` is stamped once here so `formatRelTime` stays deterministic.
   */
  private async loadInitiatives(): Promise<void> {
    try {
      const plans = await firstValueFrom(this.api.listAgentPlans());
      this.consoleNow.set(new Date().toISOString());
      this.agentPlans.set(plans ?? []);
      // Default the selection to the first initiative so the header/right column have context.
      if (!this.selectedInitiativeId() && plans && plans.length > 0) {
        this.selectedInitiativeId.set(plans[0].id);
      }
    } catch {
      this.agentPlans.set([]);
    }
  }

  /**
   * Reconcile the Events pane with the selected initiative. Reloads the timeline when the initiative
   * (or its `updatedAt` stamp) changes, and keeps a gentle 6s poll running while the initiative is live
   * (not 'done') so the SDLC timeline follows planning → review loop → done on its own. Idempotent —
   * called from an effect on every selection/stamp change.
   */
  private syncEventsWatch(
    initiativeId: string | null,
    stamp: string | null | undefined,
    isLive: boolean,
  ): void {
    const changedInitiative = initiativeId !== this.eventsInitiativeId;
    if (changedInitiative) {
      this.eventsInitiativeId = initiativeId;
      this.initiativeEvents.set([]);
      this.stopEventsPoll();
    }
    if (!initiativeId) return;
    // Reload on first selection or when the run advanced (updatedAt changed).
    if (changedInitiative || stamp !== this.eventsStamp) {
      this.eventsStamp = stamp ?? null;
      void this.loadInitiativeEvents(initiativeId, changedInitiative);
    }
    if (isLive) {
      this.startEventsPoll(initiativeId);
    } else {
      this.stopEventsPoll();
    }
  }

  /** Fetch the initiative's full SDLC timeline (planning + development runs merged). Best-effort:
   *  a failure leaves the last-loaded events and the benign empty state. */
  private async loadInitiativeEvents(initiativeId: string, showSpinner: boolean): Promise<void> {
    if (showSpinner) this.eventsLoading.set(true);
    try {
      const events = await firstValueFrom(this.api.listInitiativeEvents(initiativeId));
      // Guard against a late response for a since-changed selection.
      if (this.eventsInitiativeId === initiativeId) {
        this.initiativeEvents.set(events ?? []);
      }
    } catch {
      // keep prior events
    } finally {
      if (showSpinner) this.eventsLoading.set(false);
    }
  }

  private startEventsPoll(initiativeId: string): void {
    if (this.eventsPollInterval) return;
    this.eventsPollInterval = setInterval(() => {
      if (this.eventsInitiativeId !== initiativeId) {
        this.stopEventsPoll();
        return;
      }
      void this.loadInitiativeEvents(initiativeId, false);
    }, 6000);
  }

  private stopEventsPoll(): void {
    if (!this.eventsPollInterval) return;
    clearInterval(this.eventsPollInterval);
    this.eventsPollInterval = null;
  }

  /** Start/stop the Raw-terminal capture poll for the given primary session (null = stop). Seeds the
   *  pane immediately, then refreshes every 2.5s so a mostly-idle agent session still renders live. */
  private syncPrimaryScreenPoll(sessionId: string | null): void {
    if (sessionId === this.primaryScreenSessionId) return;
    this.primaryScreenSessionId = sessionId;
    this.stopPrimaryScreenPoll();
    if (!sessionId) return;
    void this.capturePrimaryScreen(sessionId); // seed immediately (no black-frame wait)
    this.primaryScreenPollInterval = setInterval(() => {
      if (this.primaryScreenSessionId !== sessionId || document.hidden) return;
      void this.capturePrimaryScreen(sessionId);
    }, 2500);
  }

  /** Pull one screen snapshot via the reliable `captureTerminal` request/response path and inject it
   *  into `terminalScreens` (same shape the push stream produces), unless the selection has moved on.
   *  C2b: capture `CONSOLE_CAPTURE_LINES` (1500) of history — not 200 — so the home-anchored repaint
   *  carries real scrollback the user can scroll within. The widget's content-change gate keeps a deep
   *  repaint from running while idle, and the `\x1b[3J`+home repaint (never `\x1b[2J`) stays black-free. */
  private async capturePrimaryScreen(sessionId: string): Promise<void> {
    try {
      const screen = await firstValueFrom(this.api.captureTerminal(sessionId, consoleCaptureLines()));
      if (screen && this.primaryScreenSessionId === sessionId && this.primarySessionId() === sessionId) {
        this.terminalScreens.update((screens) => ({ ...screens, [sessionId]: screen }));
      }
    } catch {
      // best-effort — a transient relay/host hiccup just skips this tick
    }
  }

  private stopPrimaryScreenPoll(): void {
    if (!this.primaryScreenPollInterval) return;
    clearInterval(this.primaryScreenPollInterval);
    this.primaryScreenPollInterval = null;
  }

  /** Select an initiative row (highlights it + drives the lifecycle bar / validation column). */
  selectInitiative(row: InitiativeRow): void {
    this.selectedInitiativeId.set(row.id);
    this.syncConsoleUrl();
  }

  /** CRUD — Create: start a new initiative (the create form provisions it and goes straight to
   *  planning; same entry as the launcher). */
  newInitiative(): void {
    void this.router.navigateByUrl('/briefing/new');
  }

  /** Mobile master-detail: clear the selection to return to the initiatives list, and drop the
   *  `?initiativeId&tab` deep-link params so a refresh/back stays on the list. */
  backToInitiativesList(): void {
    this.selectedInitiativeId.set(null);
    void this.router.navigate([], {
      relativeTo: this.route,
      queryParams: {},
      replaceUrl: true,
    });
  }

  /** CRUD — Rename: prompt for a new title, then `updateAgentPlanTitle` and refresh the list. */
  async renameInitiative(row: InitiativeRow): Promise<void> {
    const alert = await this.alertCtrl.create({
      header: 'Rename initiative',
      inputs: [{ name: 'title', type: 'text', value: row.title, attributes: { maxlength: 200 } }],
      buttons: [
        { text: 'Cancel', role: 'cancel' },
        { text: 'Save', role: 'confirm' },
      ],
    });
    await alert.present();
    const { role, data } = await alert.onDidDismiss();
    if (role !== 'confirm') return;
    const title = String(data?.values?.title ?? '').trim();
    if (!title || title === row.title) return;
    try {
      await firstValueFrom(this.api.updateAgentPlanTitle(row.id, title));
      await this.loadInitiatives();
    } catch (err) {
      await this.showInitiativeError('Failed to rename initiative', err);
    }
  }

  /** CRUD — Delete: confirm, then `deleteAgentPlan` (soft-delete) and refresh; clears selection if it was this one. */
  async deleteInitiative(row: InitiativeRow): Promise<void> {
    const alert = await this.alertCtrl.create({
      header: 'Delete initiative',
      message: `Delete "${row.title}"? Its plan files on disk are kept; this removes it from the list.`,
      buttons: [
        { text: 'Cancel', role: 'cancel' },
        { text: 'Delete', role: 'confirm' },
      ],
    });
    await alert.present();
    const { role } = await alert.onDidDismiss();
    if (role !== 'confirm') return;
    try {
      await firstValueFrom(this.api.deleteAgentPlan(row.id));
      if (this.selectedInitiativeId() === row.id) {
        this.selectedInitiativeId.set(null);
      }
      await this.loadInitiatives();
    } catch (err) {
      await this.showInitiativeError('Failed to delete initiative', err);
    }
  }

  /** Run control — Stop: halt the selected initiative's running agents (folded from the legacy
   *  planner page; the console is now the single run surface). Targets the selected run. */
  async stopInitiative(): Promise<void> {
    const init = this.selectedInitiative();
    if (!init) return;
    const alert = await this.alertCtrl.create({
      header: 'Stop initiative',
      message: `Stop "${init.title || 'this initiative'}"? The running agents are halted. Its plan and files are kept.`,
      buttons: [
        { text: 'Cancel', role: 'cancel' },
        { text: 'Stop', role: 'confirm' },
      ],
    });
    await alert.present();
    const { role } = await alert.onDidDismiss();
    if (role !== 'confirm') return;
    try {
      await firstValueFrom(this.api.stopAgentPlan(init.id));
      await this.loadInitiatives();
    } catch (err) {
      await this.showInitiativeError('Failed to stop initiative', err);
    }
  }

  /** Run control — Amend: revise an approved/complete plan in place. The planner edits the plan,
   *  T2 re-reviews it, and on PASS the automatic handoff continues development. */
  async amendInitiative(): Promise<void> {
    const init = this.selectedInitiative();
    if (!init) return;
    const alert = await this.alertCtrl.create({
      header: 'Amend plan',
      message:
        'Describe the change. The planner revises the plan in place and re-reviews it; on pass, development continues automatically.',
      inputs: [
        {
          name: 'brief',
          type: 'textarea',
          placeholder: 'e.g. Also create a README.md containing the project name',
        },
      ],
      buttons: [
        { text: 'Cancel', role: 'cancel' },
        { text: 'Amend', role: 'confirm' },
      ],
    });
    await alert.present();
    const { role, data } = await alert.onDidDismiss();
    if (role !== 'confirm') return;
    const brief = String(data?.values?.brief ?? '').trim();
    if (!brief) return;
    try {
      await firstValueFrom(this.api.amendAgentPlan(init.id, brief));
      await this.loadInitiatives();
    } catch (err) {
      await this.showInitiativeError('Failed to amend plan', err);
    }
  }

  /** Set `planRun` and (re)seed the Run/Resume form to its defaults for that run — the pre-selected
   *  phase (paused/current/first), a cleared comment, and the default `continue` mode. Called on plan
   *  (re)load and after a successful submit so the panel always reflects the freshest run. */
  private applyPlanRun(run: AgentPlanRun | null): void {
    this.planRun.set(run);
    this.runFromPhaseId.set(defaultSelectedPhaseId(run));
    this.runComment.set('');
    this.runMode.set(defaultRunMode());
  }

  /** Radio-group change → selected phase id. */
  protected setRunPhase(event: Event): void {
    const value = (event as CustomEvent<{ value?: unknown }>).detail?.value;
    this.runFromPhaseId.set(typeof value === 'string' && value ? value : null);
  }

  /** Textarea input → comment text (kept verbatim; blank/whitespace handled by `validateComment`). */
  protected setRunComment(event: Event): void {
    const value = (event as CustomEvent<{ value?: unknown }>).detail?.value;
    this.runComment.set(typeof value === 'string' ? value : '');
  }

  /** Mode segment change → run mode ('continue' default; only 'single' flips it). */
  protected setRunMode(event: Event): void {
    const value = (event as CustomEvent<{ value?: unknown }>).detail?.value;
    this.runMode.set(value === 'single' ? 'single' : 'continue');
  }

  /**
   * Run control — Run/Resume: (re)start the selected development run at the chosen phase, with the
   * optional comment/guidance, in ONE action (Phase 00's `runInitiativeFromPhase`). Mirrors the
   * `stopInitiative`/`amendInitiative` handler shape. Guarded by the button's disabled state
   * (`validateComment`); on success refreshes the plan + events timeline, on error toasts.
   */
  async runResumeInitiative(): Promise<void> {
    const run = this.planRun();
    const init = this.selectedInitiative();
    if (!run || !init || this.runSubmitting()) return;
    if (!this.runCommentValid().ok) return; // whitespace-only comment — button already disabled
    let args: ReturnType<typeof buildRunFromPhaseArgs>;
    try {
      args = buildRunFromPhaseArgs(init.id, this.runFromPhaseId(), this.runComment(), this.runMode());
    } catch {
      return; // defensive: validateComment already gates the button
    }
    this.runSubmitting.set(true);
    try {
      await firstValueFrom(
        this.api.runInitiativeFromPhase(
          args.id,
          args.phaseId ?? undefined,
          args.phaseRunMode,
          args.comment ?? undefined,
        ),
      );
      // Refresh the plan (new status/phase) + the events timeline (the human_comment row, if any).
      const fresh = await firstValueFrom(this.api.getAgentPlan(init.id));
      this.applyPlanRun(fresh ?? null);
      await this.loadInitiativeEvents(init.initiativeId || init.id, false);
      await this.loadInitiatives();
    } catch (err) {
      await this.showRunResumeToast(err);
    } finally {
      this.runSubmitting.set(false);
    }
  }

  /** Error toast for a failed run/resume (does not throw to the console). */
  private async showRunResumeToast(err: unknown): Promise<void> {
    const message = err instanceof Error ? err.message : String(err);
    const toast = await this.toastCtrl.create({
      message: `Run/Resume failed: ${message}`,
      duration: 4000,
      color: 'danger',
      position: 'bottom',
    });
    await toast.present();
  }

  /** Whether the selected initiative has agents actively running (drives the Stop control). */
  protected initiativeRunning(status: string | null | undefined): boolean {
    return ['planning', 'development', 'review'].includes((status ?? '').trim().toLowerCase());
  }

  /** Whether the selected initiative has a settled plan to revise (drives the Amend control). */
  protected initiativeAmendable(status: string | null | undefined): boolean {
    return ['planning', 'done'].includes((status ?? '').trim().toLowerCase());
  }

  private async showInitiativeError(header: string, err: unknown): Promise<void> {
    const message = err instanceof Error ? err.message : String(err);
    const alert = await this.alertCtrl.create({ header, message, buttons: ['OK'] });
    await alert.present();
  }

  /** Switch the §08 mobile console segment (Transcript/Files/Validation). */
  setConsoleSegment(segment: string): void {
    this.consoleSegment.set(consolePaneFor(segment));
  }

  /**
   * Load the working-tree diff for a session's Diff tab (overhaul P7, D10/D11). Best-effort: a
   * session with no `workingDirectory`, a non-repo cwd, or a failed RPC resolves to a benign
   * clean/empty view (no error toast — `johnny-diff-view` shows its empty state). Refetches on each
   * activation so a stale diff refreshes on tab click.
   */
  async loadDiff(session: SharedAiSession): Promise<void> {
    const dir = session.workingDirectory?.trim();
    if (!dir) {
      this.diffs.update((m) => ({ ...m, [session.id]: TerminalPage.EMPTY_DIFF }));
      return;
    }
    try {
      const view = await firstValueFrom(this.api.gitDiff(dir));
      this.diffs.update((m) => ({ ...m, [session.id]: view }));
    } catch {
      this.diffs.update((m) => ({ ...m, [session.id]: TerminalPage.EMPTY_DIFF }));
    }
  }

  /** Accumulated structured events for a session's transcript (empty if none yet). */
  transcriptEventsFor(id: string): StreamEvent[] {
    return this.transcriptEvents()[id] ?? [];
  }

  /** Live until the last event of the turn (`final`) — drives the tab's `.live` dot. */
  isSessionStreaming(id: string): boolean {
    return eventsAreStreaming(this.transcriptEventsFor(id));
  }

  /**
   * Mirror `syncTerminalVisualSubscriptions` for the structured stream lane:
   * subscribe visible sessions, unsubscribe ones no longer visible (D6).
   */
  private async syncStreamSubscriptions(): Promise<void> {
    if (document.hidden) return;
    const visible = this.lanedSessionIds();
    const { toSubscribe, toUnsubscribe } = diffStreamSubscriptions(this.streamSubscriptions, visible);
    for (const id of toSubscribe) {
      await this.relayTerminal.subscribeStream(id);
      this.streamSubscriptions.add(id);
    }
    for (const id of toUnsubscribe) {
      await this.relayTerminal.unsubscribeStream(id);
      this.streamSubscriptions.delete(id);
    }
  }

  private async unsubscribeStreamLane(id: string): Promise<void> {
    if (!this.streamSubscriptions.delete(id)) return;
    await this.relayTerminal.unsubscribeStream(id);
  }

  private async unsubscribeAllStreamLanes(): Promise<void> {
    for (const id of Array.from(this.streamSubscriptions)) {
      await this.unsubscribeStreamLane(id);
    }
  }

  /** Drop a removed session's accumulated transcript + pane-tab state. */
  private dropTranscriptState(id: string): void {
    this.transcriptEvents.update((m) => this.omitRecordKey(m, id));
    this.paneTabs.update((m) => this.omitRecordKey(m, id));
  }

  async onTerminalRawInput(data: string, sessionId = this.currentSession()?.id): Promise<void> {
    const session = this.sessions().find((item) => item.id === sessionId);
    if (!session) return;
    try {
      if (this.currentSession()?.id !== session.id) {
        await this.selectSession(session.id);
      }
      await this.relayTerminal.sendInput(session.id, data);
      // No manual refresh nudge here: the desktop wakes its capture loop on
      // input and publishes through the shared throttle (≤1 per ~2s), so the
      // echo shows promptly without adding traffic to the relay Durable Object.
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

  startPaneDrag(event: PointerEvent, sessionId = this.currentSession()?.id): void {
    if (this.isCompactWorkspace() || this.fullscreenPaneId()) return;
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
      const bounds = this.workspaceBounds();
      const maxLeft = Math.max(bounds.minX, bounds.width - this.paneInteractionStart.width);
      const maxTop = Math.max(bounds.minY, bounds.height - this.paneInteractionStart.height);
      this.updatePaneLayout(sessionId, {
        x: this.clamp(
          this.paneInteractionStart.left + moveEvent.clientX - this.paneInteractionStart.x,
          bounds.minX,
          maxLeft,
        ),
        y: this.clamp(
          this.paneInteractionStart.top + moveEvent.clientY - this.paneInteractionStart.y,
          bounds.minY,
          maxTop,
        ),
      });
    };
    this.paneUpHandler = () => this.stopPaneInteraction();

    window.addEventListener('pointermove', this.paneMoveHandler);
    window.addEventListener('pointerup', this.paneUpHandler, { once: true });
  }

  startPaneResize(event: PointerEvent, sessionId = this.currentSession()?.id): void {
    if (this.fullscreenPaneId()) return;
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

      const bounds = this.workspaceBounds();
      const maxWidth = Math.max(420, bounds.width - currentLayout.x);
      const maxHeight = Math.max(260, bounds.height - currentLayout.y);
      this.updatePaneLayout(sessionId, {
        width: this.clamp(
          this.paneInteractionStart.width + moveEvent.clientX - this.paneInteractionStart.x,
          420,
          maxWidth,
        ),
        height: this.clamp(
          this.paneInteractionStart.height + moveEvent.clientY - this.paneInteractionStart.y,
          260,
          maxHeight,
        ),
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
    this.flushWorkspaceState();
  }

  paneLayoutForDisplay(sessionId: string, index = 0): PaneLayout {
    if (this.fullscreenPaneId() === sessionId) {
      return this.fullscreenPaneLayout();
    }
    return this.paneLayout(sessionId, index);
  }

  isPaneFullscreen(sessionId: string): boolean {
    return this.fullscreenPaneId() === sessionId;
  }

  togglePaneFullscreen(sessionId: string, event?: Event): void {
    event?.preventDefault();
    event?.stopPropagation();
    if (this.fullscreenPaneId() === sessionId) {
      this.closePaneFullscreen();
      return;
    }
    this.openPaneFullscreen(sessionId);
  }

  closePaneFullscreen(): void {
    const sessionId = this.fullscreenPaneId();
    if (!sessionId) return;

    const restored = this.layoutBeforeFullscreen[sessionId];
    delete this.layoutBeforeFullscreen[sessionId];
    this.fullscreenPaneId.set(null);
    if (restored) {
      this.updatePaneLayout(sessionId, restored);
      return;
    }
    this.scheduleWorkspaceStateSave();
  }

  private openPaneFullscreen(sessionId: string): void {
    const current = this.paneLayout(sessionId);
    this.layoutBeforeFullscreen[sessionId] = { ...current };
    this.fullscreenPaneId.set(sessionId);
    void this.selectSession(sessionId);
    this.paneLayouts.update((layouts) => ({
      ...layouts,
      [sessionId]: this.fullscreenPaneLayout(),
    }));
  }

  private setupCompactWorkspaceMode(): void {
    if (typeof window === 'undefined' || !window.matchMedia) return;

    this.compactWorkspaceMediaQuery = window.matchMedia(WORKSPACE_MOBILE_MEDIA_QUERY);
    this.isCompactWorkspace.set(this.compactWorkspaceMediaQuery.matches);
    this.compactWorkspaceListener = (event) => {
      this.stopPaneInteraction();
      this.closePaneFullscreen();
      this.saveWorkspaceState();
      this.isCompactWorkspace.set(event.matches);
      // Switching desktop↔mobile uses a different saved layout set, so clear the
      // live layouts first (restore now only seeds missing ones, not overwrites).
      this.paneLayouts.set({});
      this.restorePaneLayoutsFromStorage(this.sessions());
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

    // "Close" archives the session server-side (above), which removes it from the
    // authoritative active list. No need to also track it in a local closed set.
    this.sessions.update((sessions) => sessions.filter((session) => session.id !== sessionId));
    this.terminalScreens.update((screens) => this.omitRecordKey(screens, sessionId));
    this.paneLayouts.update((layouts) => this.omitRecordKey(layouts, sessionId));
    if (this.fullscreenPaneId() === sessionId) {
      this.fullscreenPaneId.set(null);
      delete this.layoutBeforeFullscreen[sessionId];
    }
    this.scheduleWorkspaceStateSave();
    this.clearPendingAttachments(sessionId);
    this.setSendingAttachments(sessionId, false);

    if (!wasCurrentSession) return;

    this.terminalScreen.set(null);
    const nextSession = this.sessions().find((session) => session.id !== sessionId);
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

  // ── P5: resizable console dividers ─────────────────────────────────────
  // Mirrors the planner grid-resize idiom (`startFileResize`) + the sidebar-width persistence idiom
  // (`load/saveSidebarWidth`). Reuses the pure T01 clamp/parse helpers; no splitter library.

  /** Rail↔center handle: dragging right widens the rail (`+1`). */
  startRailResize(ev: PointerEvent): void {
    this.startConsoleResize(
      ev,
      this.railWidth,
      clampRailWidth,
      TerminalPage.CONSOLE_RAIL_WIDTH_KEY,
      1,
    );
  }

  /** Center↔right handle: dragging LEFT widens the validation column (`-1`). */
  startRightResize(ev: PointerEvent): void {
    this.startConsoleResize(
      ev,
      this.rightWidth,
      clampRightWidth,
      TerminalPage.CONSOLE_RIGHT_WIDTH_KEY,
      -1,
    );
  }

  private startConsoleResize(
    ev: PointerEvent,
    width: WritableSignal<number>,
    clamp: (px: number) => number,
    key: string,
    sign: 1 | -1,
  ): void {
    if (ev.button !== 0) return;
    ev.preventDefault();
    const startX = ev.clientX;
    const startWidth = width();
    const move = (moveEvent: PointerEvent): void => {
      const delta = (moveEvent.clientX - startX) * sign;
      width.set(clamp(startWidth + delta));
    };
    const up = (): void => {
      window.removeEventListener('pointermove', move);
      window.removeEventListener('pointerup', up);
      this.consoleResizeCleanup = null;
      this.saveConsoleWidth(key, width());
    };
    this.consoleResizeCleanup?.();
    this.consoleResizeCleanup = up;
    window.addEventListener('pointermove', move);
    window.addEventListener('pointerup', up);
  }

  /** Seed a divider width from localStorage, clamped on read (junk/out-of-range → clamp default). */
  private loadConsoleWidth(key: string, clamp: (px: number) => number): number {
    try {
      return parseStoredWidth(localStorage.getItem(key), clamp);
    } catch {
      return clamp(NaN);
    }
  }

  private saveConsoleWidth(key: string, width: number): void {
    try {
      localStorage.setItem(key, String(width));
    } catch {
      // Ignore localStorage write failures.
    }
  }

  private clamp(value: number, min: number, max: number): number {
    return Math.min(max, Math.max(min, value));
  }

  paneLayout(sessionId: string, index = 0): PaneLayout {
    return this.paneLayouts()[sessionId] ?? this.defaultPaneLayout(index);
  }

  private restorePaneLayoutsFromStorage(sessions: Session[]): void {
    const persisted = this.readPersistedWorkspaceState();
    const savedLayouts = this.isCompactWorkspace()
      ? persisted?.mobileLayouts
      : persisted?.desktopLayouts;
    const activeIds = new Set(sessions.map((session) => session.id));

    this.paneLayouts.update((layouts) => {
      let changed = false;
      const next = { ...layouts };

      sessions.forEach((session, index) => {
        // Keep the live in-memory layout (including a just-made drag/resize) —
        // only SEED sessions that don't have one yet (fresh load / new session).
        // Overwriting here is what reverted a pane's size/position on tab switch.
        if (next[session.id]) return;
        const saved = savedLayouts?.[session.id];
        const normalized = saved ? this.normalizePaneLayout(saved, index) : null;
        next[session.id] = normalized ?? this.defaultPaneLayout(index);
        changed = true;
      });

      for (const sessionId of Object.keys(next)) {
        if (!activeIds.has(sessionId)) {
          delete next[sessionId];
          changed = true;
        }
      }

      return changed ? next : layouts;
    });
  }

  private updatePaneLayout(sessionId: string, patch: Partial<PaneLayout>): void {
    this.paneLayouts.update((layouts) => {
      const current = layouts[sessionId] ?? this.defaultPaneLayout(0);
      const merged = { ...current, ...patch };
      const normalized = this.normalizePaneLayout(merged, 0) ?? merged;
      return {
        ...layouts,
        [sessionId]: normalized,
      };
    });
    if (this.fullscreenPaneId() !== sessionId) {
      this.scheduleWorkspaceStateSave();
    }
  }

  private defaultPaneLayout(index: number): PaneLayout {
    if (this.isCompactWorkspace()) {
      const bounds = this.workspaceBounds();
      return {
        x: bounds.minX,
        y: bounds.minY,
        width: bounds.width,
        height: this.defaultCompactPaneHeight(),
      };
    }

    const bounds = this.workspaceBounds();
    const width = Math.min(860, bounds.width);
    const height = Math.min(560, bounds.height);
    return this.normalizePaneLayout({
      x: bounds.minX + index * 36,
      y: bounds.minY + index * 30,
      width,
      height,
    }, index) ?? {
      x: bounds.minX + index * 36,
      y: bounds.minY + index * 30,
      width,
      height,
    };
  }

  private fullscreenPaneLayout(): PaneLayout {
    const bounds = this.workspaceBounds();
    return {
      x: bounds.minX,
      y: bounds.minY,
      width: bounds.width,
      height: bounds.height,
    };
  }

  private workspaceBounds(): {
    minX: number;
    minY: number;
    width: number;
    height: number;
  } {
    const padding = TerminalPage.WORKSPACE_PADDING_PX;
    const workspace = this.terminalWorkspace?.nativeElement;
    const workspaceWidth = workspace?.clientWidth && workspace.clientWidth > 0
      ? workspace.clientWidth
      : Math.max(420, (typeof window !== 'undefined' ? window.innerWidth : 1280) - 40);
    const workspaceHeight = workspace?.clientHeight && workspace.clientHeight > 0
      ? workspace.clientHeight
      : Math.max(260, (typeof window !== 'undefined' ? window.innerHeight : 800) - 180);
    const innerWidth = Math.max(320, workspaceWidth - padding * 2);
    const innerHeight = Math.max(260, workspaceHeight - padding * 2);

    return {
      minX: padding,
      minY: padding,
      width: innerWidth,
      height: innerHeight,
    };
  }

  private normalizePaneLayout(layout: PaneLayout, index: number): PaneLayout | null {
    const x = Number(layout.x);
    const y = Number(layout.y);
    const width = Number(layout.width);
    const height = Number(layout.height);
    if (![x, y, width, height].every(Number.isFinite)) return null;

    const bounds = this.workspaceBounds();
    const minWidth = this.isCompactWorkspace() ? 320 : 420;
    const minHeight = this.isCompactWorkspace() ? 360 : 260;
    const normalizedWidth = this.clamp(width, minWidth, bounds.width);
    const normalizedHeight = this.clamp(height, minHeight, bounds.height);
    const maxX = bounds.minX + Math.max(0, bounds.width - normalizedWidth);
    const maxY = bounds.minY + Math.max(0, bounds.height - normalizedHeight);

    return {
      x: this.clamp(x, bounds.minX, maxX),
      y: this.clamp(y, bounds.minY, maxY),
      width: normalizedWidth,
      height: normalizedHeight,
    };
  }

  private loadPersistedWorkspaceState(): void {
    const persisted = this.readPersistedWorkspaceState();
    if (!persisted) return;
    if (persisted.closedPaneIds.length > 0) {
      this.closedPaneIds.set(new Set(persisted.closedPaneIds));
    }
  }

  private parsePersistedLayouts(raw: unknown): Record<string, PaneLayout> {
    const layouts: Record<string, PaneLayout> = {};
    if (!raw || typeof raw !== 'object') return layouts;

    for (const [sessionId, layout] of Object.entries(raw)) {
      if (!layout || typeof layout !== 'object') continue;
      const candidate = layout as Partial<PaneLayout>;
      if (
        Number.isFinite(candidate.x)
        && Number.isFinite(candidate.y)
        && Number.isFinite(candidate.width)
        && Number.isFinite(candidate.height)
      ) {
        layouts[sessionId] = {
          x: Number(candidate.x),
          y: Number(candidate.y),
          width: Number(candidate.width),
          height: Number(candidate.height),
        };
      }
    }

    return layouts;
  }

  private readPersistedWorkspaceState(): PersistedTerminalWorkspaceState | null {
    try {
      const raw = localStorage.getItem(TerminalPage.PANE_WORKSPACE_STATE_KEY);
      if (!raw) return null;

      const parsed = JSON.parse(raw) as Partial<PersistedTerminalWorkspaceState> & {
        layouts?: Record<string, PaneLayout>;
      };
      const desktopFromField = this.parsePersistedLayouts(parsed.desktopLayouts);
      const legacyLayouts = this.parsePersistedLayouts(parsed.layouts);
      const desktopLayouts = Object.keys(desktopFromField).length > 0
        ? desktopFromField
        : legacyLayouts;
      const mobileLayouts = this.parsePersistedLayouts(parsed.mobileLayouts);

      const closedPaneIds = Array.isArray(parsed.closedPaneIds)
        ? parsed.closedPaneIds.filter((id): id is string => typeof id === 'string')
        : [];

      if (
        Object.keys(desktopLayouts).length === 0
        && Object.keys(mobileLayouts).length === 0
        && closedPaneIds.length === 0
      ) {
        return null;
      }

      return { desktopLayouts, mobileLayouts, closedPaneIds };
    } catch {
      return null;
    }
  }

  private scheduleWorkspaceStateSave(): void {
    if (this.saveWorkspaceStateTimeout) {
      clearTimeout(this.saveWorkspaceStateTimeout);
    }
    this.saveWorkspaceStateTimeout = setTimeout(() => {
      this.saveWorkspaceStateTimeout = null;
      this.saveWorkspaceState();
    }, 250);
  }

  private flushWorkspaceState(): void {
    if (this.saveWorkspaceStateTimeout) {
      clearTimeout(this.saveWorkspaceStateTimeout);
      this.saveWorkspaceStateTimeout = null;
    }
    this.saveWorkspaceState();
  }

  private buildPersistableLayouts(): Record<string, PaneLayout> {
    const activeIds = new Set(this.sessions().map((session) => session.id));
    const layouts: Record<string, PaneLayout> = {};
    for (const [sessionId, layout] of Object.entries(this.paneLayouts())) {
      if (!activeIds.has(sessionId)) continue;
      if (this.fullscreenPaneId() === sessionId) {
        const restored = this.layoutBeforeFullscreen[sessionId];
        if (restored) {
          layouts[sessionId] = restored;
        }
        continue;
      }
      layouts[sessionId] = layout;
    }
    return layouts;
  }

  private saveWorkspaceState(): void {
    try {
      const persisted = this.readPersistedWorkspaceState();
      const currentLayouts = this.buildPersistableLayouts();
      const state: PersistedTerminalWorkspaceState = {
        desktopLayouts: persisted?.desktopLayouts ?? {},
        mobileLayouts: persisted?.mobileLayouts ?? {},
        closedPaneIds: Array.from(this.closedPaneIds()),
      };

      if (this.isCompactWorkspace()) {
        state.mobileLayouts = currentLayouts;
      } else {
        state.desktopLayouts = currentLayouts;
      }

      localStorage.setItem(TerminalPage.PANE_WORKSPACE_STATE_KEY, JSON.stringify(state));
    } catch {
      // Ignore localStorage write failures.
    }
  }

  private setupWorkspaceResizeObserver(): void {
    const workspace = this.terminalWorkspace?.nativeElement;
    if (!workspace || typeof ResizeObserver === 'undefined') return;

    this.workspaceResizeObserver?.disconnect();
    this.workspaceResizeObserver = new ResizeObserver(() => {
      const fullscreenId = this.fullscreenPaneId();
      if (!fullscreenId) return;
      this.updatePaneLayout(fullscreenId, this.fullscreenPaneLayout());
    });
    this.workspaceResizeObserver.observe(workspace);
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
    // Pass the FULL session list, not [session]: restorePaneLayoutsFromStorage
    // prunes layouts for any session not in the list it's given. Passing only the
    // just-upserted session deleted every OTHER tab's pane layout — which is what
    // reset a pane's location/size on every tab switch (selectSession upserts the
    // selected session each time). The list above already includes `session`, so
    // this seeds the new one and prunes only genuinely-gone sessions.
    this.restorePaneLayoutsFromStorage(this.sessions());
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

  mobileSessionLabel(session: SharedAiSession): string {
    const raw = (session.title || '').trim() || 'Terminal';
    return raw.length > 16 ? `${raw.slice(0, 16)}…` : raw;
  }

  mobileWorkingDirectoryLabel(path?: string | null): string {
    const normalized = (path || '~').replace(/\\/g, '/').replace(/\/+$/, '');
    if (normalized === '~') return '~';

    let display = normalized;
    const homeMatch = normalized.match(/^\/home\/[^/]+(?:\/(.*))?$/);
    if (homeMatch) {
      display = homeMatch[1] ? `~/${homeMatch[1]}` : '~';
    }

    if (display.length <= 22) return display;
    const parts = display.split('/').filter(Boolean);
    if (parts.length <= 2) return `${display.slice(0, 21)}…`;
    return `~/${parts[parts.length - 2]}/${parts[parts.length - 1]}`;
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
