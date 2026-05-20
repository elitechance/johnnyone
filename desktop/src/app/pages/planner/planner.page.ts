import { CommonModule } from '@angular/common';
import { Component, OnDestroy, OnInit, computed, inject, signal } from '@angular/core';
import { FormsModule } from '@angular/forms';
import { Router } from '@angular/router';
import { firstValueFrom, Subscription } from 'rxjs';
import { AuthService } from '../../services/auth.service';
import { RelayTerminalService } from '../../services/relay-terminal.service';
import {
  AgentPlan,
  AgentPlanRun,
  HostFileEntry,
  JohnnyApiService,
  TerminalScreen,
  TerminalScreenComponent,
  WorkspaceValidation,
} from '@johnnyone/ui';

@Component({
  selector: 'app-planner',
  standalone: true,
  imports: [CommonModule, FormsModule, TerminalScreenComponent],
  templateUrl: './planner.page.html',
  styleUrls: ['./planner.page.scss'],
})
export class PlannerPage implements OnInit, OnDestroy {
  private readonly api = inject(JohnnyApiService);
  private readonly auth = inject(AuthService);
  private readonly relayTerminal = inject(RelayTerminalService);
  private readonly router = inject(Router);
  private terminalSubscription: Subscription | null = null;
  private plannerSubscription: Subscription | null = null;
  private validationTimer: ReturnType<typeof setTimeout> | null = null;

  plans = signal<AgentPlan[]>([]);
  currentRun = signal<AgentPlanRun | null>(null);
  terminalScreens = signal<Record<string, TerminalScreen>>({});
  setupOpen = signal(false);
  filesOpen = signal(false);
  fileMode = signal<'changed' | 'all'>('changed');
  workspaceFiles = signal<HostFileEntry[]>([]);
  filesError = signal<string | null>(null);
  browserOpen = signal(false);
  browserMode = signal<'workspace' | 'plan'>('workspace');
  browserEntries = signal<HostFileEntry[]>([]);
  browserError = signal<string | null>(null);
  validation = signal<WorkspaceValidation | null>(null);
  error = signal<string | null>(null);
  isBusy = signal(false);

  setupTitle = '';
  workspacePath = '/home/creepy/documents/workspace';
  planPath = 'personal/docs/johnnyone/plans/agent-validation-loop';
  browsePath = '/home/creepy/documents/workspace';
  workerProvider = 'codex';
  reviewerProvider = 'codex';

  currentPlan = computed(() => this.currentRun()?.plan ?? null);
  currentPhase = computed(() => {
    const run = this.currentRun();
    if (!run) return null;
    return run.phases.find((phase) => phase.phaseId === run.plan.currentPhaseId) ?? run.phases[0] ?? null;
  });
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

  ngOnInit(): void {
    this.subscribeToRelayEvents();
    void this.relayTerminal.connect();
    void this.loadPlans();
    this.scheduleValidation();
  }

  ngOnDestroy(): void {
    if (this.validationTimer) clearTimeout(this.validationTimer);
    this.terminalSubscription?.unsubscribe();
    this.plannerSubscription?.unsubscribe();
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
    this.scheduleValidation();
  }

  closeSetup(): void {
    this.setupOpen.set(false);
  }

  async openPathBrowser(mode: 'workspace' | 'plan'): Promise<void> {
    this.browserMode.set(mode);
    this.browserOpen.set(true);
    const startPath = mode === 'workspace'
      ? this.workspacePath.trim() || '/'
      : this.absolutePlanPath();
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
    await this.loadBrowsePath(this.parentPath(this.browsePath));
  }

  async browseInto(entry: HostFileEntry): Promise<void> {
    if (entry.kind !== 'directory') return;
    await this.loadBrowsePath(entry.path);
  }

  useBrowsePath(path = this.browsePath): void {
    if (this.browserMode() === 'workspace') {
      this.workspacePath = path;
    } else {
      this.planPath = this.planPathFromAbsolute(path);
    }
    this.browserOpen.set(false);
    this.scheduleValidation();
  }

  async loadPlans(selectFirst = true): Promise<void> {
    try {
      const plans = await firstValueFrom(this.api.listAgentPlans());
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
      this.currentRun.set(run);
      await this.attachPlanTerminals(run);
    } catch (err) {
      this.error.set(String(err));
    }
  }

  async createPlanner(): Promise<void> {
    const validation = this.validation();
    if (!validation?.valid || this.isBusy()) return;
    this.isBusy.set(true);
    this.error.set(null);
    try {
      const run = await firstValueFrom(this.api.createAgentPlan({
        title: this.setupTitle.trim() || validation.title || undefined,
        workspacePath: this.workspacePath.trim(),
        planPath: this.planPath.trim(),
        workerProvider: this.workerProvider,
        reviewerProvider: this.reviewerProvider,
      }));
      this.currentRun.set(run);
      this.setupOpen.set(false);
      await this.loadPlans(false);
      await this.startPlan(run.plan.id);
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
      const run = await firstValueFrom(this.api.startAgentPlan(id));
      this.currentRun.set(run);
      await this.attachPlanTerminals(run);
      await this.loadPlans(false);
    } catch (err) {
      this.error.set(String(err));
    } finally {
      this.isBusy.set(false);
    }
  }

  async stopPlan(): Promise<void> {
    const id = this.currentRun()?.plan.id;
    if (!id || this.isBusy()) return;
    this.isBusy.set(true);
    try {
      const run = await firstValueFrom(this.api.stopAgentPlan(id));
      this.currentRun.set(run);
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
    await this.loadPlans(false);
  }

  async sendBackToWorker(): Promise<void> {
    const id = this.currentRun()?.plan.id;
    if (!id) return;
    this.currentRun.set(await firstValueFrom(this.api.sendAgentFeedbackToWorker(id)));
  }

  async rerunReviewer(): Promise<void> {
    const id = this.currentRun()?.plan.id;
    if (!id) return;
    this.currentRun.set(await firstValueFrom(this.api.rerunAgentReviewer(id)));
  }

  async manualPass(): Promise<void> {
    const id = this.currentRun()?.plan.id;
    const phaseId = this.currentPhase()?.phaseId;
    if (!id || !phaseId || !this.canManualPass()) return;
    this.currentRun.set(await firstValueFrom(this.api.manualPassAgentPhase(id, phaseId)));
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

  async openFiles(mode: 'changed' | 'all' = this.fileMode()): Promise<void> {
    const planId = this.currentRun()?.plan.id;
    if (!planId) return;
    this.fileMode.set(mode);
    this.filesOpen.set(true);
    this.filesError.set(null);
    try {
      this.workspaceFiles.set(await firstValueFrom(this.api.listWorkspaceFiles(planId, mode)));
    } catch (err) {
      this.filesError.set(String(err));
      this.workspaceFiles.set([]);
    }
  }

  closeFiles(): void {
    this.filesOpen.set(false);
  }

  eventPayload(event: { payloadJson: string }): string {
    try {
      return JSON.stringify(JSON.parse(event.payloadJson), null, 2);
    } catch {
      return event.payloadJson;
    }
  }

  private subscribeToRelayEvents(): void {
    this.terminalSubscription = this.relayTerminal.screens().subscribe((screen) => {
      this.terminalScreens.update((screens) => ({ ...screens, [screen.sessionId]: screen }));
    });
    this.plannerSubscription = this.relayTerminal.agentPlanRuns().subscribe((update) => {
      if (update.deleted) {
        this.plans.update((plans) => plans.filter((plan) => plan.id !== update.planId));
        if (this.currentRun()?.plan.id === update.planId) {
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
        void this.attachPlanTerminals(update.run);
      }
    });
  }

  private async attachPlanTerminals(run: AgentPlanRun): Promise<void> {
    const sessionIds = [run.plan.workerSessionId, run.plan.reviewerSessionId].filter(Boolean) as string[];
    for (const sessionId of sessionIds) {
      try {
        await this.relayTerminal.attach(sessionId);
      } catch {
        // The coordinator may still be starting the host/tmux session.
      }
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

  private parentPath(path: string): string {
    const normalized = path.replace(/\/+$/, '') || '/';
    if (normalized === '/') return '/';
    const index = normalized.lastIndexOf('/');
    return index <= 0 ? '/' : normalized.slice(0, index);
  }
}
