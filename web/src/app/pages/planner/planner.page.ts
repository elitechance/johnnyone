import { CommonModule } from '@angular/common';
import { Component, HostListener, OnDestroy, OnInit, computed, inject, signal } from '@angular/core';
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
  IonSegment,
  IonSegmentButton,
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
import { closeOutline } from 'ionicons/icons';

// Register the close icon for the Files modal's close button.
addIcons({ 'close-outline': closeOutline });
import { firstValueFrom, Subscription } from 'rxjs';
import { Marked, marked } from 'marked';
import mermaid from 'mermaid';
import { AuthService } from '../../services/auth.service';
import { RelayTerminalService } from '../../services/relay-terminal.service';
import { MermaidZoomService } from '../../services/mermaid-zoom.service';
import {
  AgentPlan,
  AgentPlanPhase,
  AgentPlanTask,
  AgentPlanRun,
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
    IonSegment,
    IonSegmentButton,
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
  private readonly sanitizer = inject(DomSanitizer);
  private readonly markdownParser = this.createMarkdownParser();
  private terminalSubscription: Subscription | null = null;
  private plannerSubscription: Subscription | null = null;
  private validationTimer: ReturnType<typeof setTimeout> | null = null;
  private coordinatorResizeCleanup: (() => void) | null = null;
  private fileResizeCleanup: (() => void) | null = null;
  private compactWorkspaceMediaQuery: MediaQueryList | null = null;
  private compactWorkspaceListener: ((event: MediaQueryListEvent) => void) | null = null;
  private selectedPlanHtmlObjectUrl: string | null = null;
  private plannerVisualSubscriptions = new Set<string>();
  private readonly visibilityChangeHandler = () => {
    if (document.hidden) {
      void this.unsubscribeAllPlanTerminals();
    } else {
      const run = this.currentRun();
      if (run) void this.attachPlanTerminals(run);
    }
  };

  plans = signal<AgentPlan[]>([]);
  currentRun = signal<AgentPlanRun | null>(null);
  terminalScreens = signal<Record<string, TerminalScreen>>({});
  setupOpen = signal(false);
  filesOpen = signal(false);
  amendOpen = signal(false);
  isAmendBusy = signal(false);
  isRefreshingPhases = signal(false);
  amendBrief = '';
  filesTab = signal<'changes' | 'plan'>('changes');
  fileMode = signal<'changed' | 'all'>('changed');
  workspaceBrowsePath = signal('');
  workspaceFiles = signal<HostFileEntry[]>([]);
  selectedWorkspaceFile = signal<HostFileEntry | null>(null);
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
  taskStatusDetails = signal<Record<string, TaskStatusDetail>>({});
  workerMessage = signal('');
  reviewerMessage = signal('');
  coordinatorHeight = signal(280);
  fileSidebarWidth = signal(330);
  isCompactWorkspace = signal(false);
  activeMobilePanel = signal<PlannerMobilePanel>('worker');
  validation = signal<WorkspaceValidation | null>(null);
  promptSettings = signal<PlannerPromptSettings | null>(null);
  error = signal<string | null>(null);
  isBusy = signal(false);
  promptSettingsOpen = false;
  promptSettingsPath = '~/.johnnyone/planner-prompts.yml';

  setupTitle = '';
  workspacePath = '/home/creepy/documents/workspace';
  planPath = 'personal/docs/johnnyone/plans/agent-validation-loop';
  browsePath = '/home/creepy/documents/workspace';
  workerProvider = 'codex';
  reviewerProvider = 'codex';
  userBrief = '';
  appScope = 'personal/apps';
  docsScope = 'personal/docs';
  referencePaths = '';
  developmentWorkerPrompt = '';
  developmentReviewerPrompt = '';
  planningPlannerPrompt = '';
  planningReviewerPrompt = '';

  currentPlan = computed(() => this.currentRun()?.plan ?? null);
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

  ngOnInit(): void {
    this.setupCompactWorkspaceMode();
    mermaid.initialize({
      startOnLoad: false,
      securityLevel: 'strict',
      theme: 'dark',
    });
    this.subscribeToRelayEvents();
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

  async openPathBrowser(mode: 'workspace' | 'plan' | 'appScope' | 'docsScope' | 'reference'): Promise<void> {
    this.browserMode.set(mode);
    this.browserOpen.set(true);
    const startPath = this.browserStartPath(mode);
    await this.loadBrowsePath(startPath);
  }

  closePathBrowser(): void {
    this.browserOpen.set(false);
  }

  async loadBrowsePath(path: string): Promise<void> {
    this.browsePath = path || '/';
    this.browserError.set(null);
    try {
      this.browserEntries.set(await firstValueFrom(this.api.browseHostDirectory(this.browsePath)));
    } catch (err) {
      this.browserEntries.set([]);
      this.browserError.set(String(err));
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
      if (selectFirst && !this.currentRun() && plans[0]) {
        await this.selectPlan(plans[0].id);
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
      this.ensureActiveMobilePanel(run);
      await this.attachPlanTerminals(run);

      // If the Files modal is open, refresh its contents for the new plan.
      if (this.filesOpen()) {
        await Promise.all([
          this.loadWorkspaceFiles(this.fileMode()),
          this.loadPlanFiles(this.planRootPath()),
        ]);
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
    // Terminal screens are keyed by session_id and the new plan will have its
    // own session_ids — keeping stale entries means workerScreen/reviewerScreen
    // could resolve to a previous plan's frame if lookups accidentally hit.
    this.terminalScreens.set({});

    // Files modal state.
    this.workspaceBrowsePath.set('');
    this.workspaceFiles.set([]);
    this.selectedWorkspaceFile.set(null);
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
    if (!id || this.isBusy()) return;
    this.isBusy.set(true);
    this.error.set(null);
    try {
      const run = await firstValueFrom(this.api.startAgentPlan(
        id,
        this.mode() === 'development' ? this.selectedStartPhaseId() || undefined : undefined,
      ));
      this.currentRun.set(run);
      this.ensureActiveMobilePanel(run);
      await this.attachPlanTerminals(run);
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
      await this.attachPlanTerminals(run);
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
    this.isBusy.set(true);
    this.error.set(null);
    try {
      await firstValueFrom(this.api.deleteAgentPlan(id));
      const nextPlans = this.plans().filter((plan) => plan.id !== id);
      this.plans.set(nextPlans);
      if (this.currentRun()?.plan.id === id) {
        await this.unsubscribeAllPlanTerminals();
        this.currentRun.set(null);
        if (nextPlans[0]) {
          await this.selectPlan(nextPlans[0].id);
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
    this.filesTab.set('changes');
    this.filesError.set(null);
    await Promise.all([
      this.loadWorkspaceFiles(mode),
      this.loadPlanFiles(this.planBrowsePath() || this.planRootPath()),
    ]);
  }

  async setFilesTab(tab: 'changes' | 'plan'): Promise<void> {
    this.filesTab.set(tab);
    if (tab === 'changes' && this.workspaceFiles().length === 0) {
      await this.loadWorkspaceFiles(this.fileMode());
    }
    if (tab === 'plan' && this.planFiles().length === 0) {
      await this.loadPlanFiles(this.planBrowsePath() || this.planRootPath());
    }
  }

  async loadWorkspaceFiles(mode: 'changed' | 'all' = this.fileMode(), path?: string): Promise<void> {
    const planId = this.currentRun()?.plan.id;
    if (!planId) return;
    this.fileMode.set(mode);
    this.filesError.set(null);
    try {
      const browsePath = path || this.workspaceBrowsePath() || this.workspaceRootPath();
      const files = mode === 'changed'
        ? await firstValueFrom(this.api.listWorkspaceFiles(planId, mode))
        : await firstValueFrom(this.api.browseHostDirectory(browsePath));
      if (mode === 'all') {
        this.workspaceBrowsePath.set(browsePath);
      }
      this.workspaceFiles.set(files);
      const firstFile = files.find((entry) => entry.kind === 'file');
      if (mode === 'changed' && firstFile) {
        await this.selectWorkspaceFile(firstFile);
      } else if (mode === 'all') {
        this.selectedWorkspaceFile.set(null);
        this.workspaceDiff.set('');
      } else if (firstFile) {
        await this.selectWorkspaceFile(firstFile);
      } else {
        this.selectedWorkspaceFile.set(null);
        this.workspaceDiff.set('');
      }
    } catch (err) {
      this.filesError.set(String(err));
      this.workspaceFiles.set([]);
      this.selectedWorkspaceFile.set(null);
      this.workspaceDiff.set('');
    }
  }

  closeFiles(): void {
    this.filesOpen.set(false);
  }

  async selectWorkspaceFile(file: HostFileEntry): Promise<void> {
    const planId = this.currentRun()?.plan.id;
    if (!planId || file.kind !== 'file') return;
    this.selectedWorkspaceFile.set(file);
    this.filesError.set(null);
    try {
      const result = await firstValueFrom(this.api.getWorkspaceFileDiff(planId, file.path));
      this.workspaceDiff.set(result.diff);
    } catch (err) {
      this.workspaceDiff.set('');
      this.filesError.set(String(err));
    }
  }

  async browseWorkspaceParent(): Promise<void> {
    const current = this.workspaceBrowsePath() || this.workspaceRootPath();
    if (!current) return;
    await this.loadWorkspaceFiles('all', this.parentPath(current));
  }

  async selectWorkspaceEntry(entry: HostFileEntry): Promise<void> {
    if (this.fileMode() === 'all' && entry.kind === 'directory') {
      await this.loadWorkspaceFiles('all', entry.path);
      return;
    }
    await this.selectWorkspaceFile(entry);
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
    // Only intercept inside our markdown previews — leaves left-nav etc. alone.
    if (!anchor.closest('.plan-markdown-preview')) return;

    const href = anchor.getAttribute('href');
    if (!href) return;

    // Let true external links + in-page anchors + protocol-handlers through.
    if (/^https?:\/\//i.test(href) || /^(mailto|tel|sms):/i.test(href) || href.startsWith('#')) {
      anchor.setAttribute('target', '_blank');
      anchor.setAttribute('rel', 'noopener noreferrer');
      return;
    }

    const currentPath = this.selectedPlanContent()?.path;
    if (!currentPath) return;

    // Resolve and dispatch.
    const resolved = this.resolveRelativeHostPath(currentPath, href);
    if (!resolved) return;

    event.preventDefault();
    event.stopPropagation();
    void this.openHostPath(resolved);
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

  /** Open an arbitrary host file path in the plan-preview pane. Synthesizes
   * a HostFileEntry and reuses `selectPlanFile`'s side effects (content
   * fetch + markdown render + mermaid re-render). Does NOT try to refresh
   * the directory listing — the linked file is usually outside the plan
   * dir (e.g. common/methodology.md). If the target is actually a directory,
   * fall back to opening it in the Plan Files browser instead of surfacing the
   * host "path is not a file" preview error. */
  private async openHostPath(path: string): Promise<void> {
    const dir = path.replace(/\/+[^/]*$/, '') || '/';
    const filename = path.slice(dir.length + 1);
    if (!filename) return;
    // Open the Files modal on the Plan Files tab if it's closed — that's
    // where the preview pane lives.
    if (!this.filesOpen()) {
      this.filesOpen.set(true);
      this.filesTab.set('plan');
    } else if (this.filesTab() !== 'plan') {
      this.filesTab.set('plan');
    }
    await this.selectPlanFile({
      path,
      name: filename,
      kind: 'file',
      status: 'unchanged',
    } as HostFileEntry, { directoryFallbackPath: this.hostAbsolutePath(path) });
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
    return this.fileMode() === 'all' ? file.name : file.path;
  }

  fileStatusLabel(file: HostFileEntry): string {
    return file.status || file.kind;
  }

  fileStatusClass(file: HostFileEntry): string {
    const status = file.status || file.kind;
    if (status.includes('A') || status === 'added') return 'added';
    if (status.includes('D') || status === 'deleted') return 'deleted';
    if (status.includes('M') || status === 'modified') return 'modified';
    return this.statusClass(status);
  }

  fileKind(entry: HostFileEntry): string {
    if (entry.kind === 'directory') return 'dir';
    const ext = entry.name.includes('.') ? entry.name.split('.').pop() : 'file';
    return ext?.toLowerCase() || 'file';
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
  }

  async sendWorkerMessage(): Promise<void> {
    const sessionId = this.currentRun()?.plan.workerSessionId;
    const message = this.workerMessage().trim();
    if (!sessionId || !message) return;
    this.workerMessage.set('');
    await this.relayTerminal.sendInput(sessionId, `${message}\r`);
  }

  async sendReviewerMessage(): Promise<void> {
    const sessionId = this.currentRun()?.plan.reviewerSessionId;
    const message = this.reviewerMessage().trim();
    if (!sessionId || !message) return;
    this.reviewerMessage.set('');
    await this.relayTerminal.sendInput(sessionId, `${message}\r`);
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

  async loadPlannerTerminalHistory(role: 'worker' | 'reviewer', rows: number): Promise<void> {
    const plan = this.currentRun()?.plan;
    const sessionId = role === 'worker' ? plan?.workerSessionId : plan?.reviewerSessionId;
    if (!sessionId) return;
    await this.relayTerminal.loadHistory(sessionId, rows);
  }

  openTerminalMermaid(svg: string): void {
    this.mermaidZoom.open(svg);
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

  eventPayload(event: { payloadJson: string }): string {
    try {
      return JSON.stringify(JSON.parse(event.payloadJson), null, 2);
    } catch {
      return event.payloadJson;
    }
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
        }
        return;
      }
      if (!update.run) return;
      this.currentRun.update((current) => current?.plan.id === update.planId ? update.run! : current);
      this.plans.update((plans) => {
        const plan = update.run!.plan;
        const index = plans.findIndex((item) => item.id === plan.id);
        if (index === -1) return [plan, ...plans];
        const next = [...plans];
        next[index] = plan;
        return next.sort((a, b) => b.updatedAt.localeCompare(a.updatedAt));
      });
      if (this.currentRun()?.plan.id === update.planId) {
        this.ensureActiveMobilePanel(update.run);
        void this.attachPlanTerminals(update.run);
      }
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
      if (run) this.ensureActiveMobilePanel(run);
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

  private async attachPlanTerminals(run: AgentPlanRun): Promise<void> {
    if (document.hidden) return;
    const sessionIds = [run.plan.workerSessionId, run.plan.reviewerSessionId].filter(Boolean) as string[];
    const visibleIds = new Set(sessionIds);

    for (const sessionId of Array.from(this.plannerVisualSubscriptions)) {
      if (!visibleIds.has(sessionId)) {
        await this.unsubscribePlanTerminal(sessionId);
        // Prune the cached screen for the no-longer-visible session so it
        // can't accidentally re-render if currentRun briefly resolves back.
        this.terminalScreens.update((screens) => {
          const next = { ...screens };
          delete next[sessionId];
          return next;
        });
      }
    }

    for (const sessionId of sessionIds) {
      try {
        // refreshVisual = subscribe-if-not-subscribed, otherwise send a fresh
        // `visual_refresh` so the host re-emits the current pane state. This
        // covers tab switches where the subscription already existed but the
        // cached screen was wiped above.
        await this.relayTerminal.refreshVisual(sessionId);
        this.plannerVisualSubscriptions.add(sessionId);
      } catch {
        // The coordinator may still be starting the host/tmux session.
      }
    }
  }

  private async subscribePlanTerminal(sessionId: string): Promise<void> {
    if (this.plannerVisualSubscriptions.has(sessionId)) return;
    await this.relayTerminal.subscribeVisual(sessionId);
    this.plannerVisualSubscriptions.add(sessionId);
  }

  private async unsubscribePlanTerminal(sessionId: string): Promise<void> {
    if (!this.plannerVisualSubscriptions.delete(sessionId)) return;
    await this.relayTerminal.unsubscribeVisual(sessionId);
  }

  private async unsubscribeAllPlanTerminals(): Promise<void> {
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
    if (mode === 'workspace') return this.workspacePath.trim() || '/';
    if (mode === 'plan') return this.mode() === 'planning' ? this.workspacePath.trim() || '/' : this.absolutePlanPath();
    if (mode === 'appScope') return this.absoluteWorkspacePath(this.appScope.trim()) || this.workspacePath.trim() || '/';
    if (mode === 'docsScope') return this.absoluteWorkspacePath(this.docsScope.trim()) || this.workspacePath.trim() || '/';
    return this.workspacePath.trim() || '/';
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
