import { CommonModule, Location } from '@angular/common';
import { Component, HostListener, OnDestroy, OnInit, computed, effect, inject, signal } from '@angular/core';
import { DomSanitizer, SafeHtml, SafeResourceUrl } from '@angular/platform-browser';
import { FormsModule } from '@angular/forms';
import { Router } from '@angular/router';
import { ActivatedRoute } from '@angular/router';
import {
  IonMenuButton,
  IonModal,
  IonHeader,
  IonFooter,
  IonToolbar,
  IonTitle,
  IonButtons,
  IonButton,
  IonContent,
  IonIcon,
  IonText,
  IonLabel,
  IonList,
  IonItem,
  IonBadge,
  IonNote,
  IonInput,
  IonTextarea,
  IonSelect,
  IonSelectOption,
  IonCard,
  IonCardHeader,
  IonCardTitle,
  IonCardSubtitle,
  IonCardContent,
  IonChip,
} from '@ionic/angular/standalone';
import { addIcons } from 'ionicons';
import {
  codeSlashOutline,
  closeOutline,
  contractOutline,
  documentOutline,
  documentTextOutline,
  expandOutline,
  folderOpenOutline,
  folderOutline,
  imageOutline,
  trashOutline,
} from 'ionicons/icons';

// Register the close icon for the Files modal's close button.
addIcons({
  'code-slash-outline': codeSlashOutline,
  'close-outline': closeOutline,
  'contract-outline': contractOutline,
  'document-outline': documentOutline,
  'document-text-outline': documentTextOutline,
  'expand-outline': expandOutline,
  'folder-open-outline': folderOpenOutline,
  'folder-outline': folderOutline,
  'image-outline': imageOutline,
  'trash-outline': trashOutline,
});
import { firstValueFrom, Subscription } from 'rxjs';
import { Marked, marked } from 'marked';
import mermaid from 'mermaid';
import { AuthService } from '../../services/auth.service';
import { RelayTerminalService } from '../../services/relay-terminal.service';
import { MermaidZoomService } from '../../services/mermaid-zoom.service';
import { PendingImageAttachment } from '../../components/image-attachment-composer/image-attachment-composer.component';
import {
  AgentPlan,
  AgentPlanEvent,
  AgentPlanPhase,
  AgentPlanTask,
  AgentPlanRun,
  ChatAttachment,
  GitActionResult,
  GitFilesView,
  HostFileContent,
  HostFileEntry,
  JohnnyApiService,
  PlannerPromptSettings,
  TerminalScreen,
  TerminalScreenComponent,
  WorkspaceValidation,
} from '@johnnyone/ui';
import { WORKSPACE_MOBILE_MEDIA_QUERY } from '../../workspace-responsive';

type PlannerMobilePanel = 'worker' | 'reviewer' | 'coordinator';
type PlannerTerminalRole = 'worker' | 'reviewer';

interface LensChip {
  name: string;
  status: 'reviewing' | 'verdict';
  verdict?: string;
}

type LensActor = 'CO' | 'PR' | 'QA' | 'LE';

interface LensActivityItem {
  id: string;
  time: string;
  actor: LensActor;
  text: string;
  verdict?: string;
  findings?: string[];
  attention: boolean;
}

interface LensReviewView {
  scopeLabel: string;
  lenses: LensChip[];
  activity: LensActivityItem[];
}

interface RunStageStep {
  key: string;
  label: string;
}

interface RunStage {
  badge: string; // short label for headers/chips
  title: string; // longer one-line stage
  detail: string; // what's happening right now
  tone: 'active' | 'done' | 'attention';
  steps: RunStageStep[];
  activeIndex: number;
}

interface TaskStatusDetail {
  loading: boolean;
  error?: string;
  path?: string;
  kind?: 'markdown' | 'yaml' | 'raw';
  renderedHtml?: SafeHtml;
  rawText?: string;
  fields?: Array<{ label: string; value: string }>;
}

@Component({
  selector: 'app-planner',
  standalone: true,
  imports: [
    CommonModule,
    FormsModule,
    TerminalScreenComponent,
    IonMenuButton,
    IonModal,
    IonHeader,
    IonFooter,
    IonToolbar,
    IonTitle,
    IonButtons,
    IonButton,
    IonContent,
    IonIcon,
    IonText,
    IonLabel,
    IonList,
    IonItem,
    IonBadge,
    IonNote,
    IonInput,
    IonTextarea,
    IonSelect,
    IonSelectOption,
    IonCard,
    IonCardHeader,
    IonCardTitle,
    IonCardSubtitle,
    IonCardContent,
    IonChip,
  ],
  templateUrl: './planner.page.html',
  styleUrls: ['./planner.page.scss'],
})
export class PlannerPage implements OnInit, OnDestroy {
  private readonly api = inject(JohnnyApiService);
  private readonly auth = inject(AuthService);
  private readonly relayTerminal = inject(RelayTerminalService);
  private readonly mermaidZoom = inject(MermaidZoomService);
  private readonly router = inject(Router);
  private readonly route = inject(ActivatedRoute);
  private readonly location = inject(Location);
  private readonly sanitizer = inject(DomSanitizer);
  private readonly markdownParser = this.createMarkdownParser();
  private terminalSubscription: Subscription | null = null;
  private plannerSubscription: Subscription | null = null;
  private relayErrorSubscription: Subscription | null = null;
  private validationTimer: ReturnType<typeof setTimeout> | null = null;
  private coordinatorResizeCleanup: (() => void) | null = null;
  private fileResizeCleanup: (() => void) | null = null;
  private compactWorkspaceMediaQuery: MediaQueryList | null = null;
  private compactWorkspaceListener: ((event: MediaQueryListEvent) => void) | null = null;
  private selectedWorkspaceHtmlObjectUrl: string | null = null;
  private selectedPlanHtmlObjectUrl: string | null = null;
  private plannerVisualSubscriptions = new Set<string>();
  private lastPlanTerminalSyncKey = '';
  /** Anti-flap: a terminal leaving view is unsubscribed only after this delay, so a
   * quick re-sync (panel flutter / plan update) doesn't tear down + re-establish the
   * stream — which would spam the DO with re-subscribes + throttle-bypassing refreshes. */
  private pendingUnsubscribes = new Map<string, ReturnType<typeof setTimeout>>();
  /** Last time we sent a `visual_refresh` per session. `visual_refresh` BYPASSES the
   * desktop's 2s publish throttle, so we cool it down to avoid overworking the DO. */
  private lastVisualRefreshAt = new Map<string, number>();
  private static readonly UNSUBSCRIBE_DEBOUNCE_MS = 4000;
  private static readonly REFRESH_COOLDOWN_MS = 5000;
  /** Plan status last used to auto-follow the mobile panel, so we only switch on a
   *  real phase transition and don't fight a manual tab selection within a phase. */
  private lastFollowedRunStatus = '';
  private readonly visibilityChangeHandler = () => {
    if (document.hidden) {
      this.lastPlanTerminalSyncKey = '';
      void this.unsubscribeAllPlanTerminals();
    } else {
      const run = this.currentRun();
      if (run) void this.syncPlanTerminalSubscriptions(run, { refresh: true });
    }
  };

  plans = signal<AgentPlan[]>([]);
  currentRun = signal<AgentPlanRun | null>(null);
  terminalScreens = signal<Record<string, TerminalScreen>>({});
  setupOpen = signal(false);
  existingPlansOpen = signal(false);
  existingPlansLoading = signal(false);
  existingPlans = signal<AgentPlan[]>([]);
  renameOpen = signal(false);
  filesOpen = signal(false);
  amendOpen = signal(false);
  isAmendBusy = signal(false);
  isRefreshingPhases = signal(false);
  amendBrief = '';
  renameDraft = '';
  gitFilesView = signal<GitFilesView | null>(null);
  gitActionOutput = signal<GitActionResult | null>(null);
  gitActionBusy = signal<string | null>(null);
  gitCommitMessage = '';
  fileMode = signal<'changed' | 'all'>('all');
  workspaceBrowsePath = signal('');
  workspaceFiles = signal<HostFileEntry[]>([]);
  selectedWorkspaceFile = signal<HostFileEntry | null>(null);
  selectedWorkspaceContent = signal<HostFileContent | null>(null);
  renderedWorkspaceHtml = signal<SafeHtml>('');
  selectedWorkspaceHtmlUrl = signal<SafeResourceUrl | null>(null);
  selectedWorkspaceLoading = signal(false);
  workspaceDiff = signal<string>('');
  planBrowsePath = signal('');
  planFiles = signal<HostFileEntry[]>([]);
  selectedPlanFile = signal<HostFileEntry | null>(null);
  selectedPlanContent = signal<HostFileContent | null>(null);
  renderedPlanHtml = signal<SafeHtml>('');
  selectedPlanHtmlUrl = signal<SafeResourceUrl | null>(null);
  selectedPlanLoading = signal(false);
  selectedPlanError = signal<string | null>(null);
  selectedPlanNotice = signal<string | null>(null);
  filesError = signal<string | null>(null);
  planFilesError = signal<string | null>(null);
  browserOpen = signal(false);
  browserMode = signal<'workspace' | 'plan' | 'appScope' | 'docsScope' | 'reference'>('workspace');
  browserEntries = signal<HostFileEntry[]>([]);
  browserError = signal<string | null>(null);
  phaseTasksOpen = signal(false);
  selectedPhaseId = signal<string | null>(null);
  selectedStartPhaseId = signal<string>('');
  selectedPhaseRunMode = signal<'continue' | 'single'>('continue');
  taskStatusDetails = signal<Record<string, TaskStatusDetail>>({});
  workerMessage = signal('');
  reviewerMessage = signal('');
  workerPendingAttachments = signal<PendingImageAttachment[]>([]);
  reviewerPendingAttachments = signal<PendingImageAttachment[]>([]);
  isSendingWorkerAttachments = signal(false);
  isSendingReviewerAttachments = signal(false);
  coordinatorHeight = signal(280);
  fileSidebarWidth = signal(330);
  isCompactWorkspace = signal(false);
  activeMobilePanel = signal<PlannerMobilePanel>('worker');
  fullscreenTerminalRole = signal<PlannerTerminalRole | null>(null);
  validation = signal<WorkspaceValidation | null>(null);
  promptSettings = signal<PlannerPromptSettings | null>(null);
  error = signal<string | null>(null);
  coordinatorNotice = signal<string | null>(null);
  isBusy = signal(false);
  promptSettingsOpen = false;
  promptSettingsPath = '~/.johnnyone/planner-prompts.yml';

  setupTitle = '';
  workspacePath = '';
  planPath = '';
  browsePath = '';
  workerProvider = 'codex';
  reviewerProvider = 'codex';
  /** Setup commands for a shell worker — run in the spawned shell on first launch. */
  workerSetupCommands = '';
  /** Setup commands for a shell reviewer — run in each spawned reviewer shell. */
  reviewerSetupCommands = '';
  userBrief = '';
  appScope = 'personal/apps';
  // Editable app-repo path shown in Run Settings (dev runs); synced from the
  // selected run by `syncAppScopeEdit` (only when the plan id changes, so a live
  // refresh doesn't clobber an in-progress edit).
  appScopeEdit = '';
  savingAppScope = signal(false);
  private appScopeSyncedPlanId: string | null = null;
  docsScope = 'personal/docs';
  referencePaths = '';
  developmentWorkerPrompt = '';
  developmentReviewerPrompt = '';
  planningPlannerPrompt = '';
  planningReviewerPrompt = '';

  currentPlan = computed(() => this.currentRun()?.plan ?? null);

  /**
   * Derives the live 3-lens (Product/QA/Lead) review panel for the current run —
   * chips + an activity feed — from the run's events, scoped to the most recent
   * review round (everything at/after the last `*review_started`). Returns null
   * when that round produced no lens events (legacy single-reviewer runs, or a run
   * that hasn't reached review yet). Renders to match the lens-review mock.
   */
  lensReview = computed<LensReviewView | null>(() => {
    const all = [...(this.currentRun()?.events ?? [])].sort((a, b) =>
      a.createdAt < b.createdAt ? -1 : a.createdAt > b.createdAt ? 1 : 0,
    );
    if (!all.length) return null;

    // Scope to the current review round: from the last `*review_started` onward.
    let roundStart = 0;
    for (let i = all.length - 1; i >= 0; i--) {
      if (
        all[i].eventType === 'planning_review_started' ||
        all[i].eventType === 'agent_phase_review_started'
      ) {
        roundStart = i;
        break;
      }
    }
    const scope = all.slice(roundStart);
    if (!scope.some((e) => e.eventType.startsWith('agent_lens'))) return null;

    // Chips: latest event per lens wins (started → reviewing, verdict → verdict).
    const latestByLens = new Map<string, AgentPlanEvent>();
    let phaseId: string | undefined;
    for (const e of scope) {
      if (
        e.eventType !== 'agent_lens_review_started' &&
        e.eventType !== 'agent_lens_verdict'
      )
        continue;
      const lens = this.lensName(e);
      if (!lens) continue;
      latestByLens.set(lens, e); // scope is sorted ascending, so last write wins
      if (e.phaseId) phaseId = e.phaseId;
    }
    const order = ['Product', 'QA', 'Lead'];
    const lenses: LensChip[] = order
      .filter((name) => latestByLens.has(name))
      .map((name) => {
        const e = latestByLens.get(name)!;
        return e.eventType === 'agent_lens_verdict'
          ? { name, status: 'verdict', verdict: this.lensVerdict(e) }
          : { name, status: 'reviewing', verdict: undefined };
      });
    if (!lenses.length) return null;

    // Events feed (the "why") — built from the WHOLE run, newest-first so the latest
    // is on top (no scrolling). This is the event log relocated from the CO panel.
    const activity = [...all]
      .reverse()
      .map((e) => this.lensActivityItem(e))
      .filter((item): item is LensActivityItem => item !== null)
      .slice(0, 80);

    const scopeLabel = phaseId ? `Phase ${phaseId} review` : 'Plan review';
    return { scopeLabel, lenses, activity };
  });

  /** Display label for a verdict token (NEEDS_CHANGES reads as FAILED). */
  verdictLabel(verdict: string | undefined | null): string {
    if (!verdict) return '';
    return verdict === 'NEEDS_CHANGES' ? 'FAILED' : verdict;
  }

  /**
   * Lightweight markdown → HTML for lens activity text/findings (agents emit
   * markdown-ish prose). HTML is escaped FIRST so only our own formatting tags
   * are produced, and the result is bound via `[innerHTML]` which Angular's
   * sanitizer runs over — so no XSS even on untrusted agent output.
   */
  renderMd(raw: string | undefined | null): string {
    if (!raw) return '';
    let s = raw.replace(/&/g, '&amp;').replace(/</g, '&lt;').replace(/>/g, '&gt;');
    s = s.replace(/```([\s\S]*?)```/g, (_m, c: string) => `<pre class="md-pre"><code>${c.replace(/^\n|\n$/g, '')}</code></pre>`);
    s = s.replace(/`([^`\n]+)`/g, '<code class="md-code">$1</code>');
    s = s.replace(/\*\*([^*\n]+)\*\*/g, '<strong>$1</strong>');
    s = s.replace(/(^|[^*\w])\*([^*\n]+)\*/g, '$1<em>$2</em>');
    s = s.replace(/(^|[^_\w])_([^_\n]+)_/g, '$1<em>$2</em>');
    s = s.replace(/\[([^\]]+)\]\((https?:\/\/[^)\s]+)\)/g, '<a href="$2" target="_blank" rel="noopener noreferrer">$1</a>');
    s = s.replace(/\n/g, '<br>');
    return s;
  }

  /** Maps a round event to a styled activity row, or null if not lens-relevant. */
  private lensActivityItem(event: AgentPlanEvent): LensActivityItem | null {
    const time = this.formatLensTime(event.createdAt);
    switch (event.eventType) {
      case 'planning_review_started':
      case 'agent_phase_review_started':
        return {
          id: event.id,
          time,
          actor: 'CO',
          text: 'Review started — spawning Product / QA / Lead',
          attention: false,
        };
      case 'agent_lens_review_started': {
        const lens = this.lensName(event);
        if (!lens) return null;
        return {
          id: event.id,
          time,
          actor: this.lensActor(lens),
          text: 'Review started',
          attention: false,
        };
      }
      case 'agent_lens_verdict': {
        const lens = this.lensName(event);
        if (!lens) return null;
        const verdict = this.lensVerdict(event);
        const payload = this.parseEventPayload(event);
        const summary =
          typeof payload['summary'] === 'string' ? (payload['summary'] as string).trim() : '';
        const findings = Array.isArray(payload['findings'])
          ? (payload['findings'] as unknown[]).filter(
              (f): f is string =>
                typeof f === 'string' && !!f.trim() && f.trim().toLowerCase() !== 'none',
            )
          : [];
        return {
          id: event.id,
          time,
          actor: this.lensActor(lens),
          text: summary || 'reported its verdict',
          verdict,
          findings,
          attention: !!verdict && verdict !== 'PASS',
        };
      }
      case 'planning_gate_result':
      case 'agent_phase_gate_result': {
        const verdict = this.lensVerdict(event);
        const tail = verdict === 'PASS' ? 'approved' : 'sent back to T1';
        return {
          id: event.id,
          time,
          actor: 'CO',
          text: `Merged verdict — ${tail}`,
          verdict,
          attention: !!verdict && verdict !== 'PASS',
        };
      }
      default: {
        // Generic fallback so the rest of the run's events (planner ready, feedback
        // sent, attention, etc.) still show in this feed — it's the CO event log,
        // relocated here. Drop pure-noise types.
        const NOISE = new Set([
          'planning_start_nudge',
          'planning_verdict_clarification_requested',
          'agent_phase_verdict_clarification_requested',
        ]);
        if (NOISE.has(event.eventType)) return null;
        const text = event.summary?.trim() || event.eventType.replace(/_/g, ' ');
        return {
          id: event.id,
          time,
          actor: 'CO',
          text,
          verdict: this.lensVerdict(event),
          attention:
            event.eventType.includes('needs_attention') ||
            event.eventType.includes('blocked') ||
            event.eventType.includes('coordinator_failed'),
        };
      }
    }
  }

  private lensActor(lensName: string): LensActor {
    switch (lensName) {
      case 'Product':
        return 'PR';
      case 'QA':
        return 'QA';
      case 'Lead':
        return 'LE';
      default:
        return 'CO';
    }
  }

  private formatLensTime(createdAt: string): string {
    // SQLite emits naive UTC ("YYYY-MM-DD HH:MM:SS"); tag it so it parses as UTC.
    const iso = /\d{4}-\d{2}-\d{2} \d{2}:\d{2}:\d{2}/.test(createdAt)
      ? createdAt.replace(' ', 'T') + 'Z'
      : createdAt;
    const d = new Date(iso);
    if (isNaN(d.getTime())) return createdAt;
    return d.toLocaleTimeString([], { hour: 'numeric', minute: '2-digit' });
  }

  private parseEventPayload(event: AgentPlanEvent): Record<string, unknown> {
    try {
      return event.payloadJson ? JSON.parse(event.payloadJson) : {};
    } catch {
      return {};
    }
  }

  private lensName(event: AgentPlanEvent): string | null {
    const lens = this.parseEventPayload(event)['lens'];
    return typeof lens === 'string' ? lens : null;
  }

  private lensVerdict(event: AgentPlanEvent): string | undefined {
    const verdict = this.parseEventPayload(event)['verdict'];
    if (typeof verdict === 'string') return verdict;
    return event.verdict ?? undefined;
  }

  /** One-line summary of the lens chips, e.g. "Product PASS · QA reviewing…". */
  private lensSummaryLine(): string {
    const lr = this.lensReview();
    if (!lr) return '';
    return lr.lenses
      .map((c) => `${c.name} ${c.status === 'reviewing' ? 'reviewing…' : c.verdict || 'reported'}`)
      .join(' · ');
  }

  /**
   * Human-readable current stage of the run, derived from `plan.status` (+ mode,
   * phases, lens chips). Drives the stage chips in the panel headers, the
   * coordinator stage stepper, and the T2 placeholder — so "what stage are we in"
   * is answerable at a glance instead of from a raw status string.
   */
  runStage = computed<RunStage | null>(() => {
    const run = this.currentRun();
    if (!run) return null;
    const status = run.plan.status;
    const lens = this.lensSummaryLine();
    const err = run.plan.error?.trim();

    if (this.mode() === 'planning') {
      const steps: RunStageStep[] = [
        { key: 'draft', label: 'Draft' },
        { key: 'review', label: 'Review' },
        { key: 'approved', label: 'Approved' },
      ];
      switch (status) {
        case 'planning_planner_running':
          return { badge: 'Drafting', title: 'T1 drafting the plan', detail: 'The planner is writing the plan files. The 3-lens T2 review starts when the draft is ready.', tone: 'active', steps, activeIndex: 0 };
        case 'planning_review_running':
          return { badge: 'Reviewing', title: 'T2 reviewing — 3 lenses', detail: lens || 'Product / QA / Lead are reviewing the plan.', tone: 'active', steps, activeIndex: 1 };
        case 'approved':
          return { badge: 'Approved', title: 'Plan approved', detail: 'The plan passed T2 review.', tone: 'done', steps, activeIndex: 2 };
        case 'needs_attention':
          return { badge: 'Attention', title: 'Needs your attention', detail: err || 'The coordinator paused — open the run to resolve and continue.', tone: 'attention', steps, activeIndex: 1 };
        case 'blocked':
          return { badge: 'Blocked', title: 'Blocked — needs your decision', detail: err || 'Reply in the T1 message bar to unblock.', tone: 'attention', steps, activeIndex: 0 };
        case 'stopped':
          return { badge: 'Stopped', title: 'Run stopped', detail: 'This planning run was stopped.', tone: 'attention', steps, activeIndex: 0 };
        default:
          return { badge: status, title: status, detail: '', tone: 'active', steps, activeIndex: 0 };
      }
    }

    // development — show phase position in the title
    const phases = run.phases ?? [];
    const total = phases.length;
    const idx = phases.findIndex((p) => p.phaseId === run.plan.currentPhaseId);
    const pos = run.plan.currentPhaseId
      ? `Phase ${run.plan.currentPhaseId}${total ? ` (${idx >= 0 ? idx + 1 : '?'}/${total})` : ''}`
      : 'Phase';
    const steps: RunStageStep[] = [
      { key: 'work', label: 'Work' },
      { key: 'review', label: 'Review' },
      { key: 'done', label: 'Done' },
    ];
    switch (status) {
      case 'phase_worker_running':
        return { badge: 'Working', title: `${pos} · T1 working`, detail: 'The worker is implementing this phase.', tone: 'active', steps, activeIndex: 0 };
      case 'phase_review_running':
        return { badge: 'Reviewing', title: `${pos} · T2 reviewing`, detail: lens || 'Product / QA / Lead are reviewing this phase.', tone: 'active', steps, activeIndex: 1 };
      case 'approved': {
        // The docs-commit agent runs AFTER approval — reflect its state so the user
        // knows whether docs are still committing / done / failed.
        switch (this.docsCommitState()) {
          case 'committing':
            return { badge: 'Docs…', title: 'All phases complete — committing docs', detail: 'The docs agent is updating and committing the app-repo docs.', tone: 'active', steps, activeIndex: 2 };
          case 'committed':
            return { badge: 'Done', title: 'All phases complete — docs committed', detail: 'Every phase passed; docs were committed to the app repo.', tone: 'done', steps, activeIndex: 2 };
          case 'failed':
            return { badge: 'Attention', title: 'Approved — docs commit failed', detail: 'Phases passed, but the docs commit failed (see the activity feed).', tone: 'attention', steps, activeIndex: 2 };
          case 'skipped':
            return { badge: 'Done', title: 'All phases complete', detail: 'Every phase passed. Docs commit skipped — no app repo path set.', tone: 'done', steps, activeIndex: 2 };
          default:
            return { badge: 'Done', title: 'All phases complete', detail: 'Every phase passed review.', tone: 'done', steps, activeIndex: 2 };
        }
      }
      case 'needs_attention':
        return { badge: 'Attention', title: 'Needs your attention', detail: err || 'The coordinator paused — open the run to resolve and continue.', tone: 'attention', steps, activeIndex: 1 };
      case 'blocked':
        return { badge: 'Blocked', title: 'Blocked — needs your decision', detail: err || 'Reply in the T1 message bar to unblock.', tone: 'attention', steps, activeIndex: 0 };
      case 'stopped':
        return { badge: 'Stopped', title: 'Run stopped', detail: 'This development run was stopped.', tone: 'attention', steps, activeIndex: 0 };
      default:
        return { badge: status, title: status, detail: '', tone: 'active', steps, activeIndex: 0 };
    }
  });

  /** Placeholder shown in the T2 panel when there is no lens review to display. */
  t2Placeholder = computed(() => {
    const status = this.currentRun()?.plan.status;
    switch (status) {
      case 'planning_planner_running':
      case 'phase_worker_running':
        return 'Review has not started — T1 is still working. The 3-lens review (Product / QA / Lead) will appear here once T2 begins.';
      case 'planning_review_running':
      case 'phase_review_running':
        return 'Spawning the Product / QA / Lead reviewers…';
      case 'approved':
        return 'Approved — no review is pending.';
      case 'needs_attention':
        return this.currentRun()?.plan.error?.trim() || 'This run needs your attention.';
      default:
        return 'No review activity yet.';
    }
  });

  /** Latest state of the dev docs-commit agent (Feature 3), derived from its events,
   * so the stage indicator can show committing/committed/failed/skipped after approval. */
  private docsCommitState(): 'committing' | 'committed' | 'failed' | 'skipped' | null {
    const events = this.currentRun()?.events ?? [];
    let latest: AgentPlanEvent | null = null;
    for (const e of events) {
      if (!e.eventType.startsWith('agent_docs_commit') && e.eventType !== 'agent_docs_committed') {
        continue;
      }
      if (!latest || e.createdAt >= latest.createdAt) latest = e;
    }
    switch (latest?.eventType) {
      case 'agent_docs_committed':
        return 'committed';
      case 'agent_docs_commit_failed':
        return 'failed';
      case 'agent_docs_commit_skipped':
        return 'skipped';
      case 'agent_docs_commit_started':
        return 'committing';
      default:
        return null;
    }
  }

  mode = computed<'planning' | 'development'>(() => {
    const mode = this.route.snapshot.data['mode'];
    return mode === 'planning' ? 'planning' : 'development';
  });
  modeTitle = computed(() => this.mode() === 'planning' ? 'Planning' : 'Development');
  runType = computed<'planning' | 'development'>(() => this.mode());
  primaryActionLabel = computed(() => this.mode() === 'planning' ? 'New Planner' : 'New Development');
  emptyTitle = computed(() => this.mode() === 'planning' ? 'No planning run selected' : 'No development run selected');
  emptyText = computed(() => this.mode() === 'planning'
    ? 'Start a T1/T2 planning run to create and review methodology plans.'
    : 'Start a T1/T2 development run from an approved plan.');
  t1Label = computed(() => this.mode() === 'planning' ? 'Planner' : 'Worker');
  t2Label = computed(() => this.mode() === 'planning' ? 'Plan Reviewer' : 'Reviewer');
  fullscreenTerminalTitle = computed(() => {
    const role = this.fullscreenTerminalRole();
    if (!role) return 'Terminal';
    const badge = role === 'worker' ? 'T1' : 'T2';
    const label = role === 'worker' ? this.t1Label() : this.t2Label();
    return `${badge} · ${label}`;
  });
  currentPhase = computed(() => {
    const run = this.currentRun();
    if (!run) return null;
    return run.phases.find((phase) => phase.phaseId === run.plan.currentPhaseId) ?? run.phases[0] ?? null;
  });
  selectedPhase = computed(() => {
    const run = this.currentRun();
    const phaseId = this.selectedPhaseId();
    if (!run || !phaseId) return null;
    return run.phases.find((phase) => phase.phaseId === phaseId) ?? null;
  });
  selectedPhaseTasks = computed(() => {
    const run = this.currentRun();
    const phaseId = this.selectedPhaseId();
    if (!run || !phaseId) return [];
    return run.tasks
      .filter((task) => task.phaseId === phaseId)
      .sort((a, b) => a.taskIndex - b.taskIndex);
  });
  coordinatorRows = computed(() =>
    this.isCompactWorkspace()
      ? 'auto minmax(0, 1fr)'
      : `minmax(0, 1fr) 8px ${this.coordinatorHeight()}px`
  );
  workerScreen = computed(() => {
    const sessionId = this.currentRun()?.plan.workerSessionId;
    return sessionId ? this.terminalScreens()[sessionId] ?? null : null;
  });
  reviewerScreen = computed(() => {
    const sessionId = this.currentRun()?.plan.reviewerSessionId;
    return sessionId ? this.terminalScreens()[sessionId] ?? null : null;
  });
  canManualPass = computed(() => {
    const phase = this.currentPhase();
    return !!phase && phase.clarificationAttempts >= 5;
  });
  canAmend = computed(() => {
    const run = this.currentRun();
    if (this.mode() !== 'planning' || !run) return false;
    return !['blocked', 'stopped'].includes(run.plan.status);
  });
  startPlanLabel = computed(() => {
    const status = this.currentRun()?.plan.status;
    if (!status) return 'Start';
    if (this.mode() === 'planning') {
      if (status === 'planning_planner_running') return 'Nudge T1';
      if (status === 'planning_review_running') return 'Restart T2';
      if (status === 'approved') return 'Approved';
    }
    if (this.mode() === 'development' && status === 'approved' && !this.selectedStartPhaseId()) {
      return 'Pick phase';
    }
    return 'Start';
  });
  startPlanDisabled = computed(() => {
    const run = this.currentRun();
    if (!run || this.isBusy()) return true;
    const status = run.plan.status;
    if (this.mode() === 'planning' && ['approved', 'blocked', 'stopped'].includes(status)) {
      return status === 'approved' ? false : true;
    }
    if (this.mode() === 'development' && ['blocked', 'stopped'].includes(status)) return true;
    return false;
  });
  planRootPath = computed(() => {
    const plan = this.currentRun()?.plan;
    if (!plan) return '';
    if (plan.planPath.startsWith('/')) return plan.planPath;
    return `${plan.workspacePath.replace(/\/+$/, '')}/${plan.planPath.replace(/^\/+/, '')}`;
  });
  workspaceRootPath = computed(() => this.currentRun()?.plan.workspacePath ?? '');
  selectedPlanPreviewKind = computed(() => {
    const content = this.selectedPlanContent();
    if (!content) return 'empty';
    if (content.contentType.startsWith('image/')) return 'image';
    if (content.contentType === 'text/html') return 'html';
    if (content.contentType === 'text/markdown') return 'markdown';
    return 'raw';
  });
  selectedPlanImageSrc = computed(() => {
    const content = this.selectedPlanContent();
    if (!content || !content.contentType.startsWith('image/')) return '';
    return `data:${content.contentType};base64,${content.content}`;
  });
  selectedWorkspacePreviewKind = computed(() => {
    const content = this.selectedWorkspaceContent();
    if (!content) return 'diff';
    if (content.contentType.startsWith('image/')) return 'image';
    if (content.contentType === 'text/html') return 'html';
    if (content.contentType === 'text/markdown') return 'markdown';
    return 'diff';
  });
  selectedWorkspaceImageSrc = computed(() => {
    const content = this.selectedWorkspaceContent();
    if (!content || !content.contentType.startsWith('image/')) return '';
    return `data:${content.contentType};base64,${content.content}`;
  });
  selectedPlanRawText = computed(() => {
    const content = this.selectedPlanContent();
    if (!content) return '';
    if (content.encoding === 'utf8') return content.content;
    return `[base64 ${content.contentType}, ${content.size} bytes]`;
  });
  diffLines = computed(() => this.workspaceDiff().split('\n').map((line, index) => ({
    index: index + 1,
    text: line,
    kind: line.startsWith('+') && !line.startsWith('+++')
      ? 'add'
      : line.startsWith('-') && !line.startsWith('---')
        ? 'remove'
        : line.startsWith('@@')
          ? 'hunk'
          : 'context',
  })));
  fileViewerColumns = computed(() => `${this.fileSidebarWidth()}px 8px minmax(0, 1fr)`);

  constructor() {
    // Keep the editable app-repo field in sync with the selected run (id-gated so a
    // live refresh doesn't overwrite what the user is typing).
    effect(() => this.syncAppScopeEdit());
  }

  private syncAppScopeEdit(): void {
    const plan = this.currentRun()?.plan;
    if (!plan) {
      this.appScopeSyncedPlanId = null;
      return;
    }
    if (this.appScopeSyncedPlanId !== plan.id) {
      this.appScopeSyncedPlanId = plan.id;
      this.appScopeEdit = plan.appScope ?? '';
    }
  }

  async saveAppScope(): Promise<void> {
    const id = this.currentRun()?.plan.id;
    if (!id || this.savingAppScope()) return;
    this.savingAppScope.set(true);
    this.error.set(null);
    try {
      const run = await firstValueFrom(
        this.api.updateAgentPlanAppScope(id, this.appScopeEdit.trim()),
      );
      this.appScopeSyncedPlanId = null; // force re-sync from the saved value
      this.currentRun.set(run);
      this.coordinatorNotice.set(
        this.appScopeEdit.trim()
          ? 'App repo path saved — docs will be committed there on completion.'
          : 'App repo path cleared — docs commit will be skipped.',
      );
    } catch (err) {
      this.error.set(err instanceof Error ? err.message : String(err));
    } finally {
      this.savingAppScope.set(false);
    }
  }

  ngOnInit(): void {
    this.setupCompactWorkspaceMode();
    mermaid.initialize({
      startOnLoad: false,
      securityLevel: 'strict',
      theme: 'dark',
    });
    this.subscribeToRelayEvents();
    this.subscribeToRelayErrorEvents();
    document.addEventListener('visibilitychange', this.visibilityChangeHandler);
    void this.relayTerminal.connect();
    void this.loadPlans();
    void this.loadPromptSettings();
    this.scheduleValidation();
  }

  ngOnDestroy(): void {
    document.removeEventListener('visibilitychange', this.visibilityChangeHandler);
    void this.unsubscribeAllPlanTerminals();
    if (this.validationTimer) clearTimeout(this.validationTimer);
    this.coordinatorResizeCleanup?.();
    this.fileResizeCleanup?.();
    this.teardownCompactWorkspaceMode();
    this.terminalSubscription?.unsubscribe();
    this.plannerSubscription?.unsubscribe();
    this.relayErrorSubscription?.unsubscribe();
    this.relayErrorSubscription = null;
    this.clearPendingAttachments('worker');
    this.clearPendingAttachments('reviewer');
    this.clearSelectedWorkspaceHtmlUrl();
    this.clearSelectedPlanHtmlUrl();
  }

  navigateTo(path: string): void {
    void this.router.navigateByUrl(path);
  }

  logout(): void {
    this.auth.logout();
  }

  openSetup(): void {
    this.error.set(null);
    this.setupOpen.set(true);
    void this.loadPromptSettings();
    this.scheduleValidation();
  }

  closeSetup(): void {
    this.setupOpen.set(false);
  }

  async openExistingPlans(): Promise<void> {
    this.error.set(null);
    this.existingPlansOpen.set(true);
    await this.loadExistingPlans();
  }

  closeExistingPlans(): void {
    this.existingPlansOpen.set(false);
  }

  async loadExistingPlans(): Promise<void> {
    this.existingPlansLoading.set(true);
    try {
      const plans = await firstValueFrom(this.api.listAgentPlans(undefined, this.runType(), true));
      this.existingPlans.set(plans);
    } catch (err) {
      this.error.set(String(err));
      this.existingPlans.set([]);
    } finally {
      this.existingPlansLoading.set(false);
    }
  }

  async openExistingPlan(id: string): Promise<void> {
    this.closeExistingPlans();
    await this.selectPlan(id);
    const run = this.currentRun();
    if (run && !this.plans().some((plan) => plan.id === run.plan.id)) {
      this.plans.update((plans) => [run.plan, ...plans]);
    }
  }

  async openPathBrowser(mode: 'workspace' | 'plan' | 'appScope' | 'docsScope' | 'reference'): Promise<void> {
    this.browserMode.set(mode);
    this.browserOpen.set(true);
    const startPath = this.browserStartPath(mode);
    const loaded = await this.loadBrowsePath(startPath);
    if (!loaded && mode === 'plan') {
      await this.loadNearestBrowseParent(startPath);
    }
  }

  closePathBrowser(): void {
    this.browserOpen.set(false);
  }

  async loadBrowsePath(path: string): Promise<boolean> {
    this.browsePath = path || '/';
    this.browserError.set(null);
    try {
      this.browserEntries.set(await firstValueFrom(this.api.browseHostDirectory(this.browsePath)));
      return true;
    } catch (err) {
      this.browserEntries.set([]);
      this.browserError.set(String(err));
      return false;
    }
  }

  private async loadNearestBrowseParent(path: string): Promise<void> {
    let current = this.clampToWorkspace(path);
    const workspace = this.workspacePath.replace(/\/+$/, '');
    while (current && current !== '/') {
      current = this.parentPath(current);
      if (workspace && !current.startsWith(workspace)) {
        current = workspace;
      }
      if (await this.loadBrowsePath(current)) return;
      if (current === workspace) break;
    }
    if (workspace) {
      await this.loadBrowsePath(workspace);
    }
  }

  async browseParent(): Promise<void> {
    const parent = this.parentPath(this.browsePath);
    await this.loadBrowsePath(this.clampToWorkspace(parent));
  }

  async browseInto(entry: HostFileEntry): Promise<void> {
    if (entry.kind !== 'directory') return;
    await this.loadBrowsePath(entry.path);
  }

  useBrowsePath(path = this.browsePath): void {
    if (this.browserMode() === 'workspace') {
      this.workspacePath = path;
    } else if (this.browserMode() === 'plan') {
      this.planPath = this.planPathFromAbsolute(path);
    } else if (this.browserMode() === 'appScope') {
      this.appScope = this.planPathFromAbsolute(path);
    } else if (this.browserMode() === 'docsScope') {
      this.docsScope = this.planPathFromAbsolute(path);
    } else {
      const next = this.planPathFromAbsolute(path);
      const existing = this.referencePaths
        .split('\n')
        .map((line) => line.trim())
        .filter(Boolean);
      if (!existing.includes(next)) {
        this.referencePaths = [...existing, next].join('\n');
      }
    }
    this.browserOpen.set(false);
    this.scheduleValidation();
  }

  browserTitle(): string {
    switch (this.browserMode()) {
      case 'workspace':
        return 'Choose Workspace Directory';
      case 'plan':
        return this.mode() === 'planning' ? 'Choose Output Plan Directory' : 'Choose Plan Directory';
      case 'appScope':
        return 'Choose App/Source Scope';
      case 'docsScope':
        return 'Choose Docs Scope';
      case 'reference':
        return 'Choose Reference Path';
    }
  }

  canBrowseUp(): boolean {
    if (this.browserMode() === 'workspace') return this.browsePath !== '/';
    const workspace = this.workspacePath.replace(/\/+$/, '');
    const current = this.browsePath.replace(/\/+$/, '');
    return !!workspace && current !== workspace && current.startsWith(`${workspace}/`);
  }

  async loadPlans(selectFirst = true): Promise<void> {
    try {
      const plans = await firstValueFrom(this.api.listAgentPlans(undefined, this.runType()));
      this.plans.set(plans);
      if (selectFirst && !this.currentRun() && plans.length) {
        // Deep link: if the URL names a plan that exists, open it; else the first.
        const routePlanId = this.route.snapshot.paramMap.get('planId');
        const target = routePlanId && plans.some((plan) => plan.id === routePlanId)
          ? routePlanId
          : plans[0].id;
        await this.selectPlan(target);
      }
    } catch (err) {
      this.error.set(String(err));
    }
  }

  async selectPlan(id: string): Promise<void> {
    try {
      const run = await firstValueFrom(this.api.getAgentPlan(id));

      // Wipe any state tied to the previous plan BEFORE swapping currentRun
      // so derived signals (workerScreen, planFiles, etc.) don't briefly
      // show last plan's data against the new plan's identity.
      this.resetPerPlanState();

      this.currentRun.set(run);
      // Reflect the selected run in the URL (deep-linkable, refresh-safe) without
      // a router navigation, so the component isn't torn down/recreated.
      this.location.replaceState(`/${this.mode()}/${id}`);
      this.ensureActiveMobilePanel(run);
      this.syncMobilePanelToRunningPhase(run);
      await this.syncPlanTerminalSubscriptions(run, { refresh: true });

      // If the Files modal is open, refresh its contents for the new plan.
      if (this.filesOpen()) {
        await this.loadGitFiles(this.workspaceRootPath());
      }
    } catch (err) {
      this.error.set(String(err));
    }
  }

  /**
   * Clear all signals scoped to the currently-selected plan. Called when the
   * user switches between plan tabs so the UI doesn't briefly display the old
   * plan's terminal screens / file list / selected diff against the new
   * plan's identity.
   */
  private resetPerPlanState(): void {
    this.lastPlanTerminalSyncKey = '';
    // Re-evaluate the mobile panel against the newly selected plan's phase.
    this.lastFollowedRunStatus = '';
    this.fullscreenTerminalRole.set(null);

    // Terminal screens are keyed by session_id and the new plan will have its
    // own session_ids — keeping stale entries means workerScreen/reviewerScreen
    // could resolve to a previous plan's frame if lookups accidentally hit.
    this.terminalScreens.set({});

    // Files modal state.
    this.workspaceBrowsePath.set('');
    this.workspaceFiles.set([]);
    this.gitFilesView.set(null);
    this.gitActionOutput.set(null);
    this.gitActionBusy.set(null);
    this.gitCommitMessage = '';
    this.selectedWorkspaceFile.set(null);
    this.selectedWorkspaceContent.set(null);
    this.renderedWorkspaceHtml.set('');
    this.clearSelectedWorkspaceHtmlUrl();
    this.selectedWorkspaceLoading.set(false);
    this.workspaceDiff.set('');
    this.filesError.set(null);

    this.planBrowsePath.set('');
    this.planFiles.set([]);
    this.selectedPlanFile.set(null);
    this.selectedPlanContent.set(null);
    this.selectedPlanLoading.set(false);
    this.selectedPlanError.set(null);
    this.selectedPlanNotice.set(null);
    this.renderedPlanHtml.set('');
    this.clearSelectedPlanHtmlUrl();
    this.planFilesError.set(null);
  }

  async createPlanner(): Promise<void> {
    const validation = this.validation();
    if (!validation?.valid || this.isBusy()) return;
    this.isBusy.set(true);
    this.error.set(null);
    try {
      await this.savePromptSettings();
      const run = await firstValueFrom(this.api.createAgentPlan({
        runType: this.runType(),
        title: this.setupTitle.trim() || validation.title || undefined,
        workspacePath: this.workspacePath.trim(),
        planPath: this.planPath.trim(),
        workerProvider: this.workerProvider,
        reviewerProvider: this.reviewerProvider,
        workerSetupCommands:
          this.workerProvider === 'shell' && this.workerSetupCommands.trim()
            ? this.workerSetupCommands
            : undefined,
        reviewerSetupCommands:
          this.reviewerProvider === 'shell' && this.reviewerSetupCommands.trim()
            ? this.reviewerSetupCommands
            : undefined,
        brief: this.userBrief.trim() || undefined,
        appScope: this.appScope.trim() || undefined,
        docsScope: this.docsScope.trim() || undefined,
        referencePaths: this.referencePaths.trim() || undefined,
      }));
      this.currentRun.set(run);
      this.ensureActiveMobilePanel(run);
      this.setupOpen.set(false);
      await this.loadPlans(false);
      await this.startPlan(run.plan.id);
    } catch (err) {
      this.error.set(String(err));
    } finally {
      this.isBusy.set(false);
    }
  }

  async loadPromptSettings(): Promise<void> {
    try {
      const settings = await firstValueFrom(this.api.getPlannerPromptSettings());
      this.promptSettings.set(settings);
      this.developmentWorkerPrompt = settings.development.worker;
      this.developmentReviewerPrompt = settings.development.reviewer;
      this.planningPlannerPrompt = settings.planning.planner;
      this.planningReviewerPrompt = settings.planning.reviewer;
    } catch (err) {
      this.error.set(`Failed to load planner prompts: ${String(err)}`);
    }
  }

  async savePromptSettings(): Promise<void> {
    const saved = await firstValueFrom(this.api.updatePlannerPromptSettings({
      development: {
        worker: this.developmentWorkerPrompt,
        reviewer: this.developmentReviewerPrompt,
      },
      planning: {
        planner: this.planningPlannerPrompt,
        reviewer: this.planningReviewerPrompt,
      },
    }));
    this.promptSettings.set(saved);
  }

  async savePromptSettingsFromSetup(): Promise<void> {
    if (this.isBusy()) return;
    this.isBusy.set(true);
    this.error.set(null);
    try {
      await this.savePromptSettings();
    } catch (err) {
      this.error.set(String(err));
    } finally {
      this.isBusy.set(false);
    }
  }

  async startPlan(id = this.currentRun()?.plan.id): Promise<void> {
    const current = this.currentRun();
    if (!id || !current || this.isBusy()) return;

    const status = current.plan.status;
    if (this.mode() === 'planning' && status === 'approved') {
      this.coordinatorNotice.set('Plan is approved. Use Amend to revise it, or start a Development run from this plan.');
      this.openAmend();
      return;
    }
    if (['blocked', 'stopped'].includes(status)) {
      this.error.set(`Run is ${status}. Stop/Block cleared the active coordinator; create a new run or unblock before starting again.`);
      return;
    }
    if (
      this.mode() === 'development'
      && status === 'approved'
      && !this.selectedStartPhaseId()
    ) {
      this.coordinatorNotice.set('Select a phase in the coordinator header, then click Start to rerun it.');
      this.error.set('Pick a phase from the dropdown before starting an approved development run.');
      return;
    }

    this.isBusy.set(true);
    this.error.set(null);
    this.coordinatorNotice.set(null);
    try {
      const run = await firstValueFrom(this.api.startAgentPlan(
        id,
        this.mode() === 'development' ? this.selectedStartPhaseId() || undefined : undefined,
        this.mode() === 'development' ? this.selectedPhaseRunMode() : undefined,
      ));
      this.currentRun.set(run);
      this.ensureActiveMobilePanel(run);
      if (this.mode() === 'development') {
        this.syncMobilePanelToRunningPhase(run);
      }
      if (this.mode() === 'planning') {
        if (status === 'planning_planner_running') {
          this.coordinatorNotice.set('Nudged T1 — switch to the Planner tab to watch output.');
          this.activeMobilePanel.set('worker');
        } else if (status === 'planning_review_running') {
          this.coordinatorNotice.set('Restarted T2 review — switch to the Plan Reviewer tab.');
          this.activeMobilePanel.set('reviewer');
        } else if (run.plan.status === 'planning_planner_running') {
          this.coordinatorNotice.set('Planner started — switch to the T1 tab to watch output.');
          this.activeMobilePanel.set('worker');
        }
      }
      await this.syncPlanTerminalSubscriptions(run, { refresh: true });
      await this.loadPlans(false);
    } catch (err) {
      this.error.set(String(err));
    } finally {
      this.isBusy.set(false);
    }
  }

  async refreshPlanPhases(): Promise<void> {
    const id = this.currentRun()?.plan.id;
    if (!id || this.isBusy() || this.isRefreshingPhases()) return;
    this.isRefreshingPhases.set(true);
    this.error.set(null);
    try {
      const run = await firstValueFrom(this.api.refreshAgentPlanPhases(id));
      this.currentRun.set(run);
      if (this.selectedStartPhaseId() && !run.phases.some((phase) => phase.phaseId === this.selectedStartPhaseId())) {
        this.selectedStartPhaseId.set('');
      }
      await this.loadPlans(false);
    } catch (err) {
      this.error.set(String(err));
    } finally {
      this.isRefreshingPhases.set(false);
    }
  }

  openRenameCurrentPlan(): void {
    const run = this.currentRun();
    if (!run || this.isBusy()) return;
    this.renameDraft = run.plan.title;
    this.renameOpen.set(true);
  }

  closeRenameCurrentPlan(): void {
    this.renameOpen.set(false);
    this.renameDraft = '';
  }

  async renameCurrentPlan(): Promise<void> {
    const run = this.currentRun();
    if (!run || this.isBusy()) return;

    const trimmedTitle = this.renameDraft.trim();
    if (!trimmedTitle || trimmedTitle === run.plan.title) return;

    this.isBusy.set(true);
    this.error.set(null);
    try {
      const updated = await firstValueFrom(this.api.updateAgentPlanTitle(run.plan.id, trimmedTitle));
      this.currentRun.set(updated);
      this.upsertPlanSummary(updated.plan);
      this.ensureActiveMobilePanel(updated);
      this.closeRenameCurrentPlan();
    } catch (err) {
      this.error.set(String(err));
    } finally {
      this.isBusy.set(false);
    }
  }

  /** Open the amend modal, pre-filling the brief textarea with the in-flight
   * amendment brief if one exists on the current plan. */
  openAmend(): void {
    if (this.mode() !== 'planning') return;
    this.amendBrief = this.currentRun()?.plan.amendBrief ?? '';
    this.amendOpen.set(true);
  }

  closeAmend(): void {
    this.amendOpen.set(false);
  }

  /** Send the amendment brief to the worker. The host sets `amend_brief` on
   * the plan + re-triggers T1 in edit mode; status flows through the same
   * planning pipeline back to T2 PASS, which commits the diff. */
  async submitAmend(): Promise<void> {
    const id = this.currentRun()?.plan.id;
    const brief = this.amendBrief.trim();
    if (!id || !brief || this.isAmendBusy()) return;
    this.isAmendBusy.set(true);
    this.error.set(null);
    try {
      const run = await firstValueFrom(this.api.amendAgentPlan(id, brief));
      this.currentRun.set(run);
      this.amendBrief = '';
      this.amendOpen.set(false);
      await this.syncPlanTerminalSubscriptions(run, { refresh: true });
    } catch (err) {
      this.error.set(String(err));
    } finally {
      this.isAmendBusy.set(false);
    }
  }

  async stopPlan(): Promise<void> {
    const id = this.currentRun()?.plan.id;
    if (!id || this.isBusy()) return;
    this.isBusy.set(true);
    try {
      const run = await firstValueFrom(this.api.stopAgentPlan(id));
      this.currentRun.set(run);
      this.ensureActiveMobilePanel(run);
      await this.loadPlans(false);
    } catch (err) {
      this.error.set(String(err));
    } finally {
      this.isBusy.set(false);
    }
  }

  async deletePlan(id: string, event?: Event): Promise<void> {
    event?.stopPropagation();
    if (!id || this.isBusy()) return;
    if (!confirm('Delete this run? This permanently removes it.')) return;
    this.isBusy.set(true);
    this.error.set(null);
    try {
      await firstValueFrom(this.api.deleteAgentPlan(id));
      // Drop it from the existing-plans picker as well as the open tab strip.
      this.existingPlans.update((list) => list.filter((plan) => plan.id !== id));
      const nextPlans = this.plans().filter((plan) => plan.id !== id);
      this.plans.set(nextPlans);
      if (this.currentRun()?.plan.id === id) {
        await this.unsubscribeAllPlanTerminals();
        this.currentRun.set(null);
        if (nextPlans[0]) {
          await this.selectPlan(nextPlans[0].id);
        } else {
          // No runs left — drop the plan id from the URL.
          this.location.replaceState(`/${this.mode()}`);
        }
      }
    } catch (err) {
      this.error.set(String(err));
    } finally {
      this.isBusy.set(false);
    }
  }

  async blockPlan(): Promise<void> {
    const id = this.currentRun()?.plan.id;
    if (!id) return;
    const reason = window.prompt('Block reason', 'Blocked by user');
    if (!reason) return;
    const run = await firstValueFrom(this.api.blockAgentPlan(id, reason));
    this.currentRun.set(run);
    this.ensureActiveMobilePanel(run);
    await this.loadPlans(false);
  }

  async sendBackToWorker(): Promise<void> {
    const id = this.currentRun()?.plan.id;
    if (!id) return;
    const run = await firstValueFrom(this.api.sendAgentFeedbackToWorker(id));
    this.currentRun.set(run);
    this.ensureActiveMobilePanel(run);
  }

  async rerunReviewer(): Promise<void> {
    const id = this.currentRun()?.plan.id;
    if (!id) return;
    const run = await firstValueFrom(this.api.rerunAgentReviewer(id));
    this.currentRun.set(run);
    this.ensureActiveMobilePanel(run);
  }

  async manualPass(): Promise<void> {
    const id = this.currentRun()?.plan.id;
    const phaseId = this.currentPhase()?.phaseId;
    if (!id || !phaseId || !this.canManualPass()) return;
    const run = await firstValueFrom(this.api.manualPassAgentPhase(id, phaseId));
    this.currentRun.set(run);
    this.ensureActiveMobilePanel(run);
  }

  scheduleValidation(): void {
    if (this.validationTimer) clearTimeout(this.validationTimer);
    this.validationTimer = setTimeout(() => {
      void this.validateSetup();
    }, 300);
  }

  async validateSetup(): Promise<void> {
    const workspace = this.workspacePath.trim();
    const plan = this.planPath.trim();
    if (!workspace || !plan) {
      this.validation.set(null);
      return;
    }
    if (this.mode() === 'planning') {
      this.validation.set({
        valid: true,
        workspacePath: workspace,
        planPath: plan,
        title: this.setupTitle.trim() || this.planSlugTitle(plan),
        phaseCount: 0,
        taskCount: 0,
      });
      return;
    }
    try {
      this.validation.set(await firstValueFrom(this.api.validateWorkspacePlan(workspace, plan)));
    } catch (err) {
      this.validation.set({
        valid: false,
        workspacePath: workspace,
        planPath: plan,
        phaseCount: 0,
        taskCount: 0,
        error: String(err),
      });
    }
  }

  async openFiles(mode: 'changed' | 'all' = 'all'): Promise<void> {
    const planId = this.currentRun()?.plan.id;
    if (!planId) return;
    this.fileMode.set(mode);
    this.filesOpen.set(true);
    this.filesError.set(null);
    await this.loadGitFiles(this.workspaceBrowsePath() || this.workspaceRootPath());
  }

  async loadGitFiles(path?: string): Promise<void> {
    const planId = this.currentRun()?.plan.id;
    if (!planId) return;
    this.filesError.set(null);
    try {
      const browsePath = path || this.workspaceBrowsePath() || this.workspaceRootPath();
      const view = await firstValueFrom(this.api.gitFileView(planId, browsePath));
      this.workspaceBrowsePath.set(view.path);
      this.gitFilesView.set(view);
      this.workspaceFiles.set(view.entries);
      this.selectedWorkspaceFile.set(null);
      this.selectedWorkspaceContent.set(null);
      this.renderedWorkspaceHtml.set('');
      this.clearSelectedWorkspaceHtmlUrl();
      this.workspaceDiff.set('');
    } catch (err) {
      this.filesError.set(String(err));
      this.gitFilesView.set(null);
      this.workspaceFiles.set([]);
      this.selectedWorkspaceFile.set(null);
      this.selectedWorkspaceContent.set(null);
      this.renderedWorkspaceHtml.set('');
      this.clearSelectedWorkspaceHtmlUrl();
      this.workspaceDiff.set('');
    }
  }

  async loadWorkspaceFiles(mode: 'changed' | 'all' = this.fileMode(), path?: string): Promise<void> {
    this.fileMode.set(mode);
    await this.loadGitFiles(path);
  }

  closeFiles(): void {
    this.filesOpen.set(false);
  }

  async selectWorkspaceFile(file: HostFileEntry): Promise<void> {
    const planId = this.currentRun()?.plan.id;
    if (!planId || file.kind !== 'file') return;
    this.selectedWorkspaceFile.set(file);
    this.selectedWorkspaceContent.set(null);
    this.renderedWorkspaceHtml.set('');
    this.clearSelectedWorkspaceHtmlUrl();
    this.selectedWorkspaceLoading.set(true);
    this.filesError.set(null);
    try {
      const [result, content] = await Promise.all([
        firstValueFrom(this.api.getWorkspaceFileDiff(planId, file.path)),
        firstValueFrom(this.api.readHostFile(planId, file.path)).catch(() => null),
      ]);
      this.workspaceDiff.set(result.diff);
      this.selectedWorkspaceContent.set(content);
      if (content?.contentType === 'text/markdown' && content.encoding === 'utf8') {
        this.clearSelectedWorkspaceHtmlUrl();
        this.renderedWorkspaceHtml.set(this.sanitizer.bypassSecurityTrustHtml(this.markdownParser.parse(content.content) as string));
        window.setTimeout(() => void this.renderMermaid(), 0);
      } else if (content?.contentType === 'text/html' && content.encoding === 'utf8') {
        this.renderedWorkspaceHtml.set('');
        this.setSelectedWorkspaceHtmlUrl(content.content);
      } else {
        this.clearSelectedWorkspaceHtmlUrl();
        this.renderedWorkspaceHtml.set('');
      }
    } catch (err) {
      this.workspaceDiff.set('');
      this.clearSelectedWorkspaceHtmlUrl();
      this.filesError.set(String(err));
    } finally {
      this.selectedWorkspaceLoading.set(false);
    }
  }

  async browseWorkspaceParent(): Promise<void> {
    const current = this.workspaceBrowsePath() || this.workspaceRootPath();
    if (!current) return;
    await this.loadGitFiles(this.parentPath(current));
  }

  async selectWorkspaceEntry(entry: HostFileEntry): Promise<void> {
    if (entry.kind === 'directory') {
      await this.loadGitFiles(entry.path);
      return;
    }
    await this.selectWorkspaceFile(entry);
  }

  async runGitAction(action: 'fetch' | 'pull' | 'push' | 'commit'): Promise<void> {
    const planId = this.currentRun()?.plan.id;
    if (!planId || this.gitActionBusy()) return;
    this.gitActionBusy.set(action);
    this.gitActionOutput.set(null);
    this.filesError.set(null);
    try {
      const result = await firstValueFrom(this.api.runGitAction(planId, {
        path: this.workspaceBrowsePath() || this.workspaceRootPath(),
        action,
        message: action === 'commit' ? this.gitCommitMessage : undefined,
      }));
      this.gitActionOutput.set(result);
      if (result.success && action === 'commit') {
        this.gitCommitMessage = '';
      }
      await this.loadGitFiles(this.workspaceBrowsePath() || this.workspaceRootPath());
    } catch (err) {
      this.filesError.set(String(err));
    } finally {
      this.gitActionBusy.set(null);
    }
  }

  async loadPlanFiles(path: string, options: { autoSelectFirst?: boolean } = {}): Promise<void> {
    if (!path) return;
    this.planBrowsePath.set(path);
    this.planFilesError.set(null);
    try {
      const entries = await firstValueFrom(this.api.browseHostDirectory(path));
      this.planFiles.set(entries);
      const firstFile = entries.find((entry) => entry.kind === 'file');
      if (options.autoSelectFirst !== false && firstFile && !this.selectedPlanFile()) {
        await this.selectPlanFile(firstFile);
      }
    } catch (err) {
      this.planFiles.set([]);
      this.planFilesError.set(String(err));
    }
  }

  async browsePlanParent(): Promise<void> {
    const root = this.planRootPath();
    const current = this.planBrowsePath() || root;
    if (!current || current === root) return;
    const parent = this.parentPath(current);
    await this.loadPlanFiles(parent.startsWith(root) ? parent : root);
  }

  async selectPlanEntry(entry: HostFileEntry): Promise<void> {
    if (entry.kind === 'directory') {
      await this.loadPlanFiles(entry.path);
      return;
    }
    await this.selectPlanFile(entry);
  }

  /**
   * Intercept clicks on `<a>` elements inside the markdown preview. Mermaid
   * outputs and plan markdown often contain relative paths like
   * `../../common/methodology.md` — those don't resolve through the SPA
   * router and would 404 if clicked. Resolve them against the currently-open
   * plan file's directory and re-open the result inside this same preview
   * pane.
   */
  // Document-level click listener — needed because `<ion-modal>` teleports
  // its content into <body>, outside this component's host element, so
  // bubbling through @HostListener('click') on PlannerPage misses it.
  @HostListener('document:click', ['$event'])
  onPreviewLinkClick(event: MouseEvent): void {
    const target = event.target as HTMLElement | null;
    if (!target) return;
    const anchor = target.closest('a');
    if (!anchor) return;
    const preview = anchor.closest('.plan-markdown-preview');
    // Only intercept inside our markdown previews — leaves left-nav etc. alone.
    if (!preview) return;

    const rawHref = anchor.getAttribute('href');
    if (!rawHref) return;

    if (/^(mailto|tel|sms):/i.test(rawHref) || rawHref.startsWith('#')) {
      return;
    }

    const href = this.normalizeMarkdownHref(rawHref);
    if (/^https?:\/\//i.test(href)) {
      anchor.setAttribute('target', '_blank');
      anchor.setAttribute('rel', 'noopener noreferrer');
      return;
    }

    const currentPath = this.markdownPreviewBasePath(preview);
    if (!currentPath) return;

    const resolved = this.resolveRelativeHostPath(currentPath, href);
    if (!resolved) return;

    event.preventDefault();
    event.stopPropagation();
    void this.openHostPath(resolved, rawHref.replace(/[#?].*$/, '').endsWith('/'));
  }

  /** Same-origin absolute URLs (e.g. `https://johnnyone.pages.dev/phases/...`
   * from the browser resolving a relative plan link against the SPA origin)
   * are plan-relative host paths, not external navigation targets. */
  private normalizeMarkdownHref(href: string): string {
    if (!/^https?:\/\//i.test(href)) return href;
    try {
      const url = new URL(href);
      if (url.origin !== window.location.origin) return href;
      const path = url.pathname.replace(/^\/+/, '');
      return path ? `${path}${url.search}${url.hash}` : href;
    } catch {
      return href;
    }
  }

  private markdownPreviewBasePath(preview: Element): string | null {
    const fromAttr = preview.getAttribute('data-markdown-base')?.trim();
    if (fromAttr) return fromAttr;
    return this.selectedWorkspaceContent()?.path ?? this.selectedPlanContent()?.path ?? null;
  }

  /** Resolve `rel` (which may start with `./`, `../`, or be a bare segment)
   * against the directory of `basePath`. `basePath` is workspace-relative
   * (e.g. `lokal/docs/.../overview.md`), so the result is also
   * workspace-relative; the host joins it with the plan's workspace root
   * in `resolve_workspace_file_path`. Returns null if the relative path
   * walks above the workspace root. */
  private resolveRelativeHostPath(basePath: string, rel: string): string | null {
    // Strip any URL hash/query — host paths never carry them.
    rel = rel.replace(/[#?].*$/, '');
    // Strip a leading slash on `basePath` if some caller passed an absolute
    // form — treat both consistently.
    const base = basePath.replace(/^\/+/, '');
    const baseAbsolute = rel.startsWith('/');
    const combined = baseAbsolute
      ? rel.replace(/^\/+/, '')  // absolute rel — drop the leading slash so it joins as workspace-relative
      : `${base.replace(/\/+[^/]*$/, '')}/${rel}`;
    const parts: string[] = [];
    for (const seg of combined.split('/')) {
      if (seg === '' || seg === '.') continue;
      if (seg === '..') {
        if (parts.length === 0) return null;
        parts.pop();
        continue;
      }
      parts.push(seg);
    }
    return parts.join('/');
  }

  /** Open a workspace-relative host path in the Files modal preview pane. */
  private async openHostPath(path: string, isDirectoryLink = false): Promise<void> {
    if (!this.filesOpen()) {
      this.filesOpen.set(true);
    }

    const absolutePath = this.hostAbsolutePath(path);
    if (isDirectoryLink || !this.pathLooksLikeFile(path)) {
      await this.openHostDirectory(absolutePath);
      return;
    }

    const filename = path.split('/').pop() || path;
    await this.selectWorkspaceFile({
      path: absolutePath,
      name: filename,
      kind: 'file',
      status: 'unchanged',
    } as HostFileEntry);
  }

  private pathLooksLikeFile(path: string): boolean {
    const leaf = path.split('/').pop() || '';
    return leaf.includes('.') && !leaf.endsWith('.');
  }

  private async openHostDirectory(path: string): Promise<void> {
    await this.loadGitFiles(path);
    const preferredNames = ['prompt.md', 'overview.md', 'status.md', 'decisions.md'];
    const pick =
      preferredNames
        .map((name) => this.workspaceFiles().find((entry) => entry.kind === 'file' && entry.name === name))
        .find((entry): entry is HostFileEntry => !!entry) ??
      this.workspaceFiles().find((entry) => entry.kind === 'file' && /\.md$/i.test(entry.name));
    if (pick) {
      await this.selectWorkspaceFile(pick);
    }
  }

  async selectPlanFile(file: HostFileEntry, options: { directoryFallbackPath?: string } = {}): Promise<void> {
    const planId = this.currentRun()?.plan.id;
    if (!planId) return;
    this.selectedPlanFile.set(file);
    this.selectedPlanLoading.set(true);
    this.selectedPlanError.set(null);
    this.selectedPlanNotice.set(null);
    this.selectedPlanContent.set(null);
    this.renderedPlanHtml.set('');
    this.clearSelectedPlanHtmlUrl();
    try {
      const content = await firstValueFrom(this.api.readHostFile(planId, file.path));
      this.selectedPlanContent.set(content);
      if (content.contentType === 'text/markdown' && content.encoding === 'utf8') {
        this.clearSelectedPlanHtmlUrl();
        this.renderedPlanHtml.set(this.sanitizer.bypassSecurityTrustHtml(this.markdownParser.parse(content.content) as string));
        window.setTimeout(() => void this.renderMermaid(), 0);
      } else if (content.contentType === 'text/html' && content.encoding === 'utf8') {
        this.renderedPlanHtml.set('');
        this.setSelectedPlanHtmlUrl(content.content);
      } else {
        this.clearSelectedPlanHtmlUrl();
        this.renderedPlanHtml.set('');
      }
    } catch (err) {
      this.selectedPlanContent.set(null);
      this.renderedPlanHtml.set('');
      this.clearSelectedPlanHtmlUrl();
      if (options.directoryFallbackPath && this.isNotFilePreviewError(err)) {
        await this.openPlanDirectoryFromPreviewLink(options.directoryFallbackPath);
        return;
      }
      this.selectedPlanError.set(this.formatPreviewError(err));
    } finally {
      this.selectedPlanLoading.set(false);
    }
  }

  private async openPlanDirectoryFromPreviewLink(path: string): Promise<void> {
    this.selectedPlanFile.set(null);
    this.selectedPlanContent.set(null);
    this.renderedPlanHtml.set('');
    this.clearSelectedPlanHtmlUrl();
    this.selectedPlanError.set(null);
    this.selectedPlanNotice.set(null);
    await this.loadPlanFiles(path, { autoSelectFirst: false });
    if (this.planFilesError()) {
      this.selectedPlanError.set(this.planFilesError());
      return;
    }
    this.selectedPlanNotice.set(`Opened directory: ${this.workspaceRelativePath(path)}`);
  }

  private hostAbsolutePath(path: string): string {
    if (path.startsWith('/')) return path;
    const root = this.workspaceRootPath().replace(/\/+$/, '');
    const relative = path.replace(/^\/+/, '');
    return root ? `${root}/${relative}` : relative;
  }

  private isNotFilePreviewError(err: unknown): boolean {
    return /not a file|is a directory|directory/i.test(this.formatPreviewError(err));
  }

  private formatPreviewError(err: unknown): string {
    const raw = String(err);
    return raw.replace(/^GraphQL errors?:\s*/i, '').trim() || 'Unable to load preview.';
  }

  private setSelectedPlanHtmlUrl(html: string): void {
    this.clearSelectedPlanHtmlUrl();
    const blob = new Blob([html], { type: 'text/html' });
    this.selectedPlanHtmlObjectUrl = URL.createObjectURL(blob);
    this.selectedPlanHtmlUrl.set(this.sanitizer.bypassSecurityTrustResourceUrl(this.selectedPlanHtmlObjectUrl));
  }

  private setSelectedWorkspaceHtmlUrl(html: string): void {
    this.clearSelectedWorkspaceHtmlUrl();
    const blob = new Blob([html], { type: 'text/html' });
    this.selectedWorkspaceHtmlObjectUrl = URL.createObjectURL(blob);
    this.selectedWorkspaceHtmlUrl.set(this.sanitizer.bypassSecurityTrustResourceUrl(this.selectedWorkspaceHtmlObjectUrl));
  }

  private clearSelectedWorkspaceHtmlUrl(): void {
    if (this.selectedWorkspaceHtmlObjectUrl) {
      URL.revokeObjectURL(this.selectedWorkspaceHtmlObjectUrl);
      this.selectedWorkspaceHtmlObjectUrl = null;
    }
    this.selectedWorkspaceHtmlUrl.set(null);
  }

  private clearSelectedPlanHtmlUrl(): void {
    if (this.selectedPlanHtmlObjectUrl) {
      URL.revokeObjectURL(this.selectedPlanHtmlObjectUrl);
      this.selectedPlanHtmlObjectUrl = null;
    }
    this.selectedPlanHtmlUrl.set(null);
  }

  planRelativePath(path: string): string {
    const root = this.planRootPath().replace(/\/+$/, '');
    if (path === root) return '.';
    if (path.startsWith(`${root}/`)) return path.slice(root.length + 1);
    return path;
  }

  workspaceRelativePath(path: string): string {
    const root = this.workspaceRootPath().replace(/\/+$/, '');
    if (!path) return root || '.';
    if (path === root) return '.';
    if (root && path.startsWith(`${root}/`)) return path.slice(root.length + 1);
    return path;
  }

  workspaceEntryLabel(file: HostFileEntry): string {
    return file.name;
  }

  fileStatusClass(file: HostFileEntry): string {
    const status = file.status || file.kind;
    if (status.includes('A') || status === 'added') return 'added';
    if (status.includes('D') || status === 'deleted') return 'deleted';
    if (status.includes('M') || status === 'modified') return 'modified';
    if (status === 'changed') return 'modified';
    return this.statusClass(status);
  }

  fileIcon(file: HostFileEntry): string {
    if (file.kind === 'directory') {
      return this.hasGitChange(file) ? 'folder-open-outline' : 'folder-outline';
    }
    if (this.isImageEntry(file.name)) return 'image-outline';
    if (this.isMarkdownEntry(file.name)) return 'document-text-outline';
    if (/\.(js|ts|tsx|jsx|json|rs|go|py|java|kt|swift|c|cc|cpp|h|hpp|css|scss|html|xml|yaml|yml|toml|sql|sh)$/i.test(file.name)) {
      return 'code-slash-outline';
    }
    return 'document-outline';
  }

  fileStatusTitle(file: HostFileEntry): string {
    if (!file.status) {
      return file.kind === 'directory' ? 'Directory' : 'File';
    }
    if (file.kind === 'directory') {
      return 'Directory contains git changes';
    }
    if (file.status.includes('A') || file.status === 'added') return 'Added file';
    if (file.status.includes('D') || file.status === 'deleted') return 'Deleted file';
    if (file.status.includes('M') || file.status === 'modified') return 'Modified file';
    return `Git status: ${file.status}`;
  }

  hasGitChange(file: HostFileEntry): boolean {
    return !!file.status && file.status !== 'clean';
  }

  fileKind(entry: HostFileEntry): string {
    if (entry.kind === 'directory') return 'dir';
    const ext = entry.name.includes('.') ? entry.name.split('.').pop() : 'file';
    return ext?.toLowerCase() || 'file';
  }

  private isImageEntry(name: string): boolean {
    return /\.(avif|gif|jpe?g|png|svg|webp)$/i.test(name);
  }

  private isMarkdownEntry(name: string): boolean {
    return /\.(markdown|md|mdown|mkd)$/i.test(name);
  }

  openPhaseTasks(phase: AgentPlanPhase): void {
    this.selectedPhaseId.set(phase.phaseId);
    this.phaseTasksOpen.set(true);
    void this.loadPhaseTaskStatuses(phase.phaseId);
  }

  closePhaseTasks(): void {
    this.phaseTasksOpen.set(false);
  }

  selectMobilePanel(panel: PlannerMobilePanel): void {
    this.activeMobilePanel.set(panel);
    const run = this.currentRun();
    if (run) void this.syncPlanTerminalSubscriptions(run);
  }

  async sendWorkerMessage(): Promise<void> {
    await this.sendPlannerMessage('worker');
  }

  async sendReviewerMessage(): Promise<void> {
    await this.sendPlannerMessage('reviewer');
  }

  async sendPlannerAttachmentMessage(role: PlannerTerminalRole, message: string): Promise<void> {
    await this.sendPlannerMessage(role, message);
  }

  onPlannerImagePaste(role: PlannerTerminalRole, event: ClipboardEvent): void {
    const files = Array.from(event.clipboardData?.files ?? []).filter((file) =>
      file.type.startsWith('image/')
    );
    if (files.length === 0) return;
    event.preventDefault();
    this.addPendingImageFiles(role, files);
  }

  onPlannerImageDragOver(event: DragEvent): void {
    if (!this.dragEventHasImage(event)) return;
    event.preventDefault();
  }

  onPlannerImageDrop(role: PlannerTerminalRole, event: DragEvent): void {
    const files = Array.from(event.dataTransfer?.files ?? []).filter((file) =>
      file.type.startsWith('image/')
    );
    if (files.length === 0) return;
    event.preventDefault();
    this.addPendingImageFiles(role, files);
  }

  onPlannerImageBrowse(role: PlannerTerminalRole, event: Event): void {
    const input = event.target as HTMLInputElement;
    const files = Array.from(input.files ?? []).filter((file) => file.type.startsWith('image/'));
    if (files.length > 0) {
      this.addPendingImageFiles(role, files);
    }
    input.value = '';
  }

  addPendingImageFiles(role: PlannerTerminalRole, files: File[]): void {
    const items = files.map((file) => ({
      id: crypto.randomUUID(),
      file,
      previewUrl: URL.createObjectURL(file),
    }));
    this.pendingAttachmentsSignal(role).update((current) => [...current, ...items]);
  }

  removePendingAttachment(role: PlannerTerminalRole, id: string): void {
    this.pendingAttachmentsSignal(role).update((items) => {
      const target = items.find((item) => item.id === id);
      if (target) URL.revokeObjectURL(target.previewUrl);
      return items.filter((item) => item.id !== id);
    });
  }

  private async sendPlannerMessage(role: PlannerTerminalRole, messageOverride?: string): Promise<void> {
    const sessionId = role === 'worker'
      ? this.currentRun()?.plan.workerSessionId
      : this.currentRun()?.plan.reviewerSessionId;
    const messageSignal = role === 'worker' ? this.workerMessage : this.reviewerMessage;
    const sendingSignal = role === 'worker' ? this.isSendingWorkerAttachments : this.isSendingReviewerAttachments;
    const attachments = this.pendingAttachmentsSignal(role)();
    const message = (messageOverride ?? messageSignal()).trim();
    if (!sessionId || sendingSignal() || (!message && attachments.length === 0)) return;

    if (attachments.length === 0) {
      messageSignal.set('');
      await this.relayTerminal.sendInput(sessionId, `${message}\r`);
      return;
    }

    sendingSignal.set(true);
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
        `${message || 'Please review the attached image.'}\r`,
        uploaded.map((attachment) => ({
          id: attachment.id,
          originalName: attachment.originalName,
          contentType: attachment.contentType,
          size: attachment.size,
        })),
      );

      messageSignal.set('');
      this.clearPendingAttachments(role);
    } catch (err) {
      this.error.set(`Failed to send image attachment: ${String(err)}`);
    } finally {
      sendingSignal.set(false);
    }
  }

  openTerminalFullscreen(role: PlannerTerminalRole): void {
    this.fullscreenTerminalRole.set(role);
    const run = this.currentRun();
    if (run) void this.syncPlanTerminalSubscriptions(run, { refresh: true });
  }

  closeTerminalFullscreen(): void {
    this.fullscreenTerminalRole.set(null);
    const run = this.currentRun();
    if (run) void this.syncPlanTerminalSubscriptions(run);
  }

  async sendPlannerRawInput(role: 'worker' | 'reviewer', data: string): Promise<void> {
    const plan = this.currentRun()?.plan;
    const sessionId = role === 'worker' ? plan?.workerSessionId : plan?.reviewerSessionId;
    if (!sessionId) return;
    await this.relayTerminal.sendInput(sessionId, data);
  }

  async resizePlannerTerminal(role: 'worker' | 'reviewer', size: { cols: number; rows: number }): Promise<void> {
    const plan = this.currentRun()?.plan;
    const sessionId = role === 'worker' ? plan?.workerSessionId : plan?.reviewerSessionId;
    if (!sessionId) return;
    await this.relayTerminal.resize(sessionId, size.cols, size.rows);
  }

  private pendingAttachmentsSignal(role: PlannerTerminalRole) {
    return role === 'worker' ? this.workerPendingAttachments : this.reviewerPendingAttachments;
  }

  private clearPendingAttachments(role: PlannerTerminalRole): void {
    for (const item of this.pendingAttachmentsSignal(role)()) {
      URL.revokeObjectURL(item.previewUrl);
    }
    this.pendingAttachmentsSignal(role).set([]);
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

  phaseTaskCount(phaseId: string): number {
    return this.currentRun()?.tasks.filter((task) => task.phaseId === phaseId).length ?? 0;
  }

  taskStatusDetail(task: AgentPlanTask): TaskStatusDetail | null {
    return this.taskStatusDetails()[task.id] ?? null;
  }

  statusClass(status: string): string {
    return status.replace(/[^a-z0-9]+/gi, '-').toLowerCase();
  }

  startCoordinatorResize(event: PointerEvent): void {
    if (this.isCompactWorkspace()) return;
    if (event.button !== 0) return;
    event.preventDefault();
    const startY = event.clientY;
    const startHeight = this.coordinatorHeight();
    const move = (moveEvent: PointerEvent): void => {
      const delta = startY - moveEvent.clientY;
      const next = Math.max(180, Math.min(window.innerHeight - 180, startHeight + delta));
      this.coordinatorHeight.set(Math.round(next));
    };
    const up = (): void => {
      window.removeEventListener('pointermove', move);
      window.removeEventListener('pointerup', up);
      this.coordinatorResizeCleanup = null;
    };
    this.coordinatorResizeCleanup?.();
    this.coordinatorResizeCleanup = up;
    window.addEventListener('pointermove', move);
    window.addEventListener('pointerup', up);
  }

  startFileResize(event: PointerEvent): void {
    if (this.isCompactWorkspace()) return;
    if (event.button !== 0) return;
    event.preventDefault();
    const startX = event.clientX;
    const startWidth = this.fileSidebarWidth();
    const move = (moveEvent: PointerEvent): void => {
      const delta = moveEvent.clientX - startX;
      const next = Math.max(220, Math.min(window.innerWidth - 420, startWidth + delta));
      this.fileSidebarWidth.set(Math.round(next));
    };
    const up = (): void => {
      window.removeEventListener('pointermove', move);
      window.removeEventListener('pointerup', up);
      this.fileResizeCleanup = null;
    };
    this.fileResizeCleanup?.();
    this.fileResizeCleanup = up;
    window.addEventListener('pointermove', move);
    window.addEventListener('pointerup', up);
  }

  eventDetails(event: { payloadJson: string }): string {
    try {
      return JSON.stringify(JSON.parse(event.payloadJson), null, 2);
    } catch {
      return event.payloadJson;
    }
  }

  eventHasDetails(event: { payloadJson: string }): boolean {
    const raw = event.payloadJson?.trim();
    return !!raw && raw !== '{}' && raw !== 'null';
  }

  eventPhaseLabel(event: { phaseIndex?: number | null; phaseTitle?: string | null; phaseId?: string | null }): string {
    const prefix = event.phaseIndex !== undefined && event.phaseIndex !== null
      ? `P${event.phaseIndex + 1}`
      : 'Run';
    const suffix = event.phaseTitle || event.phaseId;
    return suffix ? `${prefix} · ${suffix}` : prefix;
  }

  private async loadPhaseTaskStatuses(phaseId: string): Promise<void> {
    const planId = this.currentRun()?.plan.id;
    const tasks = this.currentRun()?.tasks.filter((task) => task.phaseId === phaseId) ?? [];
    if (!planId) return;

    const loading = Object.fromEntries(tasks.map((task) => [
      task.id,
      { loading: true, path: task.statusPath } satisfies TaskStatusDetail,
    ]));
    this.taskStatusDetails.set(loading);

    await Promise.all(tasks.map(async (task) => {
      if (!task.statusPath) {
        this.setTaskStatusDetail(task.id, { loading: false, error: 'No status file found.' });
        return;
      }

      try {
        const content = await firstValueFrom(this.api.readHostFile(planId, task.statusPath));
        this.setTaskStatusDetail(task.id, this.createTaskStatusDetail(content));
      } catch (err) {
        this.setTaskStatusDetail(task.id, {
          loading: false,
          path: task.statusPath,
          error: String(err),
        });
      }
    }));
  }

  private setTaskStatusDetail(taskId: string, detail: TaskStatusDetail): void {
    this.taskStatusDetails.update((details) => ({
      ...details,
      [taskId]: detail,
    }));
  }

  private createTaskStatusDetail(content: HostFileContent): TaskStatusDetail {
    if (content.encoding !== 'utf8') {
      return {
        loading: false,
        path: content.path,
        kind: 'raw',
        rawText: `[${content.contentType}, ${content.size} bytes]`,
      };
    }

    if (content.contentType === 'text/markdown') {
      return {
        loading: false,
        path: content.path,
        kind: 'markdown',
        renderedHtml: this.sanitizer.bypassSecurityTrustHtml(this.markdownParser.parse(content.content) as string),
      };
    }

    if (content.name.endsWith('.yml') || content.name.endsWith('.yaml')) {
      return {
        loading: false,
        path: content.path,
        kind: 'yaml',
        rawText: content.content,
        fields: this.taskStatusYamlFields(content.content),
      };
    }

    return {
      loading: false,
      path: content.path,
      kind: 'raw',
      rawText: content.content,
    };
  }

  private taskStatusYamlFields(content: string): Array<{ label: string; value: string }> {
    const simpleField = (key: string): string => {
      const match = content.match(new RegExp(`^${key}:\\s*(.*)$`, 'm'));
      return match?.[1]?.trim() || '';
    };
    const validationSummary = content.match(/^validation:\s*$(?:\n(?:  .*)*)/m)?.[0]
      .split('\n')
      .find((line) => line.trim().startsWith('summary:'))
      ?.replace(/^\s*summary:\s*/, '')
      .trim() || '';

    return [
      { label: 'State', value: simpleField('state') },
      { label: 'Owner', value: simpleField('owner') },
      { label: 'Started', value: simpleField('started') },
      { label: 'Completed', value: simpleField('completed') },
      { label: 'Validation', value: validationSummary || simpleField('validation') },
    ].filter((field) => field.value && field.value !== 'null');
  }

  private subscribeToRelayEvents(): void {
    this.terminalSubscription = this.relayTerminal.screens().subscribe((screen) => {
      this.terminalScreens.update((screens) => ({ ...screens, [screen.sessionId]: screen }));
    });
    this.plannerSubscription = this.relayTerminal.agentPlanRuns().subscribe((update) => {
      if (update.deleted) {
        this.plans.update((plans) => plans.filter((plan) => plan.id !== update.planId));
        if (this.currentRun()?.plan.id === update.planId) {
          void this.unsubscribeAllPlanTerminals();
          this.currentRun.set(null);
          this.location.replaceState(`/${this.mode()}`);
        }
        return;
      }
      if (!update.run) return;
      if (update.run.plan.runType !== this.runType()) return;
      this.currentRun.update((current) => current?.plan.id === update.planId ? update.run! : current);
      this.upsertPlanSummary(update.run.plan);
      if (this.currentRun()?.plan.id === update.planId) {
        this.ensureActiveMobilePanel(update.run);
        this.syncMobilePanelToRunningPhase(update.run);
        void this.syncPlanTerminalSubscriptions(update.run);
      }
    });
  }

  private subscribeToRelayErrorEvents(): void {
    if (this.relayErrorSubscription) return;
    // Surface service-level bound failures (authFailureCount >3 etc) into planner's existing error banner/UI.
    // Mirrors terminal.page to ensure no silent dead socket on planner surface (the other terminal host).
    this.relayErrorSubscription = this.relayTerminal.errors().subscribe({
      next: (err) => this.error.set(String(err)),
      error: (e) => console.error('relay error sub (planner):', e),
    });
  }

  private setupCompactWorkspaceMode(): void {
    if (typeof window === 'undefined' || !window.matchMedia) return;

    this.compactWorkspaceMediaQuery = window.matchMedia(WORKSPACE_MOBILE_MEDIA_QUERY);
    this.isCompactWorkspace.set(this.compactWorkspaceMediaQuery.matches);
    this.compactWorkspaceListener = (event) => {
      this.coordinatorResizeCleanup?.();
      this.fileResizeCleanup?.();
      this.isCompactWorkspace.set(event.matches);
      const run = this.currentRun();
      if (run) {
        this.ensureActiveMobilePanel(run);
        void this.syncPlanTerminalSubscriptions(run);
      }
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

  private ensureActiveMobilePanel(run: AgentPlanRun): void {
    const active = this.activeMobilePanel();
    if (active === 'worker' && !run.plan.workerSessionId && run.plan.reviewerSessionId) {
      this.activeMobilePanel.set('reviewer');
    }
  }

  /**
   * On mobile, only the active panel's terminal is subscribed (unlike desktop,
   * which follows the running phase). So when the coordinator advances T1→T2 the
   * reviewer would silently stop updating. Auto-follow the running phase, but only
   * on an actual status transition so a manual tab switch within a phase sticks.
   */
  private syncMobilePanelToRunningPhase(run: AgentPlanRun): void {
    if (!this.isCompactWorkspace() || this.fullscreenTerminalRole()) return;
    const status = run.plan.status;
    if (status === this.lastFollowedRunStatus) return;
    this.lastFollowedRunStatus = status;
    const reviewerRunning = status === 'planning_review_running' || status === 'phase_review_running';
    const workerRunning = status === 'planning_planner_running' || status === 'phase_worker_running';
    if (reviewerRunning && run.plan.reviewerSessionId) {
      this.activeMobilePanel.set('reviewer');
    } else if (workerRunning && run.plan.workerSessionId) {
      this.activeMobilePanel.set('worker');
    }
  }

  private upsertPlanSummary(plan: AgentPlan): void {
    this.plans.update((plans) => {
      const index = plans.findIndex((item) => item.id === plan.id);
      if (index === -1) return [plan, ...plans].sort((a, b) => b.updatedAt.localeCompare(a.updatedAt));
      const next = [...plans];
      next[index] = plan;
      return next.sort((a, b) => b.updatedAt.localeCompare(a.updatedAt));
    });
    this.existingPlans.update((plans) => {
      const index = plans.findIndex((item) => item.id === plan.id);
      if (index === -1) return plans;
      const next = [...plans];
      next[index] = plan;
      return next.sort((a, b) => b.updatedAt.localeCompare(a.updatedAt));
    });
  }

  private planTerminalSyncKey(run: AgentPlanRun): string {
    const visibleIds = this.visiblePlanTerminalSessionIds(run).slice().sort().join(',');
    return [
      run.plan.id,
      run.plan.status,
      run.plan.workerSessionId ?? '',
      run.plan.reviewerSessionId ?? '',
      visibleIds,
      this.activeMobilePanel(),
      this.fullscreenTerminalRole() ?? '',
      this.isCompactWorkspace() ? '1' : '0',
    ].join('|');
  }

  private visiblePlanTerminalSessionIds(run: AgentPlanRun): string[] {
    const { workerSessionId, reviewerSessionId, status } = run.plan;

    const fullscreenRole = this.fullscreenTerminalRole();
    if (fullscreenRole) {
      const sessionId = fullscreenRole === 'worker' ? workerSessionId : reviewerSessionId;
      return sessionId ? [sessionId] : [];
    }

    if (this.isCompactWorkspace()) {
      const panel = this.activeMobilePanel();
      if (panel === 'worker' && workerSessionId) return [workerSessionId];
      if (panel === 'reviewer' && reviewerSessionId) return [reviewerSessionId];
      return [];
    }

    const workerRunning = status === 'planning_planner_running' || status === 'phase_worker_running';
    const reviewerRunning = status === 'planning_review_running' || status === 'phase_review_running';
    if (workerRunning && workerSessionId) return [workerSessionId];
    if (reviewerRunning && reviewerSessionId) return [reviewerSessionId];

    const panel = this.activeMobilePanel();
    if (panel === 'worker' && workerSessionId) return [workerSessionId];
    if (panel === 'reviewer' && reviewerSessionId) return [reviewerSessionId];
    if (workerSessionId) return [workerSessionId];
    if (reviewerSessionId) return [reviewerSessionId];
    return [];
  }

  private async syncPlanTerminalSubscriptions(
    run: AgentPlanRun,
    options?: { refresh?: boolean },
  ): Promise<void> {
    if (document.hidden) return;

    const syncKey = this.planTerminalSyncKey(run);
    if (syncKey === this.lastPlanTerminalSyncKey && !options?.refresh) return;
    this.lastPlanTerminalSyncKey = syncKey;

    const visibleIds = new Set(this.visiblePlanTerminalSessionIds(run));

    for (const sessionId of Array.from(this.plannerVisualSubscriptions)) {
      if (!visibleIds.has(sessionId)) {
        // Debounced — a brief re-sync (panel flutter, plan update) shouldn't flap the
        // stream. If it comes back into view first, the unsubscribe is cancelled.
        // Last frame is kept so the now-idle panel isn't blank.
        this.scheduleDebouncedUnsubscribe(sessionId);
      }
    }

    for (const sessionId of visibleIds) {
      this.cancelPendingUnsubscribe(sessionId);
      this.hydrateTerminalScreenFromCache(sessionId);
      try {
        const alreadyLive = this.plannerVisualSubscriptions.has(sessionId);
        if (options?.refresh && this.refreshCooldownElapsed(sessionId)) {
          // `visual_refresh` bypasses the desktop publish throttle — only send it on a
          // genuine (un-cooled-down) refresh request, not on every steady re-sync.
          await this.relayTerminal.refreshVisual(sessionId);
          this.plannerVisualSubscriptions.add(sessionId);
          this.lastVisualRefreshAt.set(sessionId, Date.now());
        } else if (!alreadyLive) {
          await this.subscribePlanTerminal(sessionId);
        }
        // else: already live and within cooldown → leave the steady stream alone.
      } catch {
        // The coordinator may still be starting the host/tmux session.
      }
    }

    // Keep the idle (non-streaming) panel showing its last screen (from cache) so
    // it isn't blank after a refresh, without disturbing the live subscription.
    this.seedIdlePlanTerminals(run, visibleIds);
  }

  /**
   * Populate the last-known screen for the plan's terminals that aren't actively
   * streaming (the idle panel) from the persisted cache, so the idle pane shows
   * its most recent screen instead of going blank after a refresh. Cache-only on
   * purpose: a live subscribe/unsubscribe here would race with — and tear down —
   * the active panel's live subscription.
   */
  private seedIdlePlanTerminals(run: AgentPlanRun, visibleIds: Set<string>): void {
    const ids = [run.plan.workerSessionId, run.plan.reviewerSessionId]
      .filter((id): id is string => !!id);
    for (const sessionId of ids) {
      if (visibleIds.has(sessionId)) continue; // streaming live; leave it alone
      this.hydrateTerminalScreenFromCache(sessionId);
    }
  }

  private hydrateTerminalScreenFromCache(sessionId: string): void {
    if (this.terminalScreens()[sessionId]) return;

    const cached = this.relayTerminal.cachedScreen(sessionId);
    if (!cached) return;

    this.terminalScreens.update((screens) => ({ ...screens, [sessionId]: cached }));
  }

  private refreshCooldownElapsed(sessionId: string): boolean {
    const last = this.lastVisualRefreshAt.get(sessionId) ?? 0;
    return Date.now() - last > PlannerPage.REFRESH_COOLDOWN_MS;
  }

  private scheduleDebouncedUnsubscribe(sessionId: string): void {
    if (this.pendingUnsubscribes.has(sessionId)) return;
    const timer = setTimeout(() => {
      this.pendingUnsubscribes.delete(sessionId);
      void this.unsubscribePlanTerminal(sessionId);
    }, PlannerPage.UNSUBSCRIBE_DEBOUNCE_MS);
    this.pendingUnsubscribes.set(sessionId, timer);
  }

  private cancelPendingUnsubscribe(sessionId: string): void {
    const timer = this.pendingUnsubscribes.get(sessionId);
    if (timer) {
      clearTimeout(timer);
      this.pendingUnsubscribes.delete(sessionId);
    }
  }

  private async subscribePlanTerminal(sessionId: string): Promise<void> {
    if (this.plannerVisualSubscriptions.has(sessionId)) return;
    await this.relayTerminal.subscribeVisual(sessionId);
    this.plannerVisualSubscriptions.add(sessionId);
    // `visual_subscribe` only streams FUTURE frames, so a session that already has
    // output (e.g. a just-started T1 planner after Start) stays blank until its next
    // throttled publish — which is why the page needed a manual refresh to show it.
    // Pull a single initial snapshot when we have no frame yet. Cooldown-guarded so
    // this can't spam the DO (visual_refresh bypasses the publish throttle).
    if (!this.terminalScreens()[sessionId] && this.refreshCooldownElapsed(sessionId)) {
      this.lastVisualRefreshAt.set(sessionId, Date.now());
      try {
        await this.relayTerminal.refreshVisual(sessionId);
      } catch {
        // Host/tmux may still be coming up — the live stream will fill in.
      }
    }
  }

  private async unsubscribePlanTerminal(sessionId: string): Promise<void> {
    if (!this.plannerVisualSubscriptions.delete(sessionId)) return;
    await this.relayTerminal.unsubscribeVisual(sessionId);
  }

  private async unsubscribeAllPlanTerminals(): Promise<void> {
    for (const timer of this.pendingUnsubscribes.values()) {
      clearTimeout(timer);
    }
    this.pendingUnsubscribes.clear();
    for (const sessionId of Array.from(this.plannerVisualSubscriptions)) {
      await this.unsubscribePlanTerminal(sessionId);
    }
  }

  private absolutePlanPath(): string {
    const plan = this.planPath.trim();
    if (!plan) return this.workspacePath.trim() || '/';
    if (plan.startsWith('/')) return plan;
    return `${this.workspacePath.replace(/\/+$/, '')}/${plan.replace(/^\/+/, '')}`;
  }

  private planPathFromAbsolute(path: string): string {
    const workspace = this.workspacePath.replace(/\/+$/, '');
    if (path === workspace) return '.';
    if (path.startsWith(`${workspace}/`)) return path.slice(workspace.length + 1);
    return path;
  }

  private browserStartPath(mode: 'workspace' | 'plan' | 'appScope' | 'docsScope' | 'reference'): string {
    if (mode === 'workspace') return this.workspacePath.trim() || '~';
    if (mode === 'plan') return this.absolutePlanPath();
    if (mode === 'appScope') return this.absoluteWorkspacePath(this.appScope.trim()) || this.workspacePath.trim() || '~';
    if (mode === 'docsScope') return this.absoluteWorkspacePath(this.docsScope.trim()) || this.workspacePath.trim() || '~';
    return this.workspacePath.trim() || '~';
  }

  private absoluteWorkspacePath(path: string): string {
    if (!path) return '';
    if (path.startsWith('/')) return path;
    return `${this.workspacePath.replace(/\/+$/, '')}/${path.replace(/^\/+/, '')}`;
  }

  private clampToWorkspace(path: string): string {
    if (this.browserMode() === 'workspace') return path;
    const workspace = this.workspacePath.replace(/\/+$/, '');
    if (!workspace) return path;
    const normalized = path.replace(/\/+$/, '') || '/';
    return normalized === workspace || normalized.startsWith(`${workspace}/`) ? normalized : workspace;
  }

  private parentPath(path: string): string {
    const normalized = path.replace(/\/+$/, '') || '/';
    if (normalized === '/') return '/';
    const index = normalized.lastIndexOf('/');
    return index <= 0 ? '/' : normalized.slice(0, index);
  }

  private planSlugTitle(path: string): string {
    const leaf = path.replace(/\/+$/, '').split('/').pop() || 'New Plan';
    return leaf.replace(/[-_]+/g, ' ').replace(/\b\w/g, (letter) => letter.toUpperCase());
  }

  private createMarkdownParser(): Marked {
    const renderer = new marked.Renderer();
    renderer.code = ({ text, lang }) => {
      const language = (lang || '').trim().toLowerCase().split(/\s+/)[0];
      if (language === 'mermaid') {
        return `<pre class="mermaid">${this.escapeHtml(text)}</pre>`;
      }
      return `<pre><code>${this.escapeHtml(text)}</code></pre>`;
    };
    renderer.table = (token) => {
      const renderCell = (cell: (typeof token.header)[number], tag: 'th' | 'td'): string => {
        const align = cell.align ? ` style="text-align: ${cell.align}"` : '';
        const content = marked.parseInline(cell.text, { gfm: true, breaks: true }) as string;
        return `<${tag}${align}>${content}</${tag}>`;
      };
      const header = token.header.map((cell) => renderCell(cell, 'th')).join('');
      const rows = token.rows
        .map((row) => `<tr>${row.map((cell) => renderCell(cell, 'td')).join('')}</tr>`)
        .join('');
      return `<div class="markdown-table-wrap"><table><thead><tr>${header}</tr></thead><tbody>${rows}</tbody></table></div>`;
    };
    return new Marked({ gfm: true, breaks: true, renderer });
  }

  private async renderMermaid(): Promise<void> {
    const nodes = Array.from(document.querySelectorAll<HTMLElement>('.plan-markdown-preview pre.mermaid'));
    for (const node of nodes) {
      const source = node.textContent?.trim();
      if (!source) continue;
      try {
        const id = `planner-mermaid-${crypto.randomUUID().replace(/-/g, '')}`;
        const rendered = await mermaid.render(id, source);
        const container = document.createElement('div');
        container.className = 'mermaid-rendered';
        container.innerHTML = rendered.svg;
        node.replaceWith(container);
        rendered.bindFunctions?.(container);
      } catch (error) {
        node.classList.add('mermaid-error');
        node.setAttribute('data-error', String(error));
      }
    }
  }

  private escapeHtml(raw: string): string {
    return raw
      .replace(/&/g, '&amp;')
      .replace(/</g, '&lt;')
      .replace(/>/g, '&gt;')
      .replace(/"/g, '&quot;')
      .replace(/'/g, '&#39;');
  }
}
