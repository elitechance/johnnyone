import { CommonModule } from '@angular/common';
import { Component, OnInit, computed, inject, signal } from '@angular/core';
import { FormsModule } from '@angular/forms';
import { Router } from '@angular/router';
import {
  IonHeader,
  IonToolbar,
  IonTitle,
  IonContent,
  IonButton,
  IonButtons,
  IonInput,
  IonTextarea,
  IonItem,
  IonLabel,
  IonList,
  IonNote,
  IonSelect,
  IonSelectOption,
  IonToggle,
  IonModal,
  IonCard,
  IonCardHeader,
  IonCardTitle,
  IonCardContent,
  IonSpinner,
} from '@ionic/angular/standalone';
import { JohnnyApiService, DetectedCliTool, DirListing } from '@johnnyone/ui';
import { firstValueFrom } from 'rxjs';
import {
  LensDraft,
  defaultLenses,
  addLens,
  removeLens,
  toConfigJson,
  providerOptions,
} from '../validation-config/validation-config-logic';
import {
  agentChoices,
  createPayload,
  parseDoctorJson,
  parseProbeJson,
  preflightView,
  COMMIT_COPY,
  ESCALATION_COPY,
  FALLBACK_EXECUTOR,
  type DoctorSnapshot,
  type PreflightState,
  type PreflightView,
} from './briefing-logic';

/**
 * New-initiative create form (overhaul P4, briefing step removed). Creating provisions the initiative
 * and immediately accepts it, so the lifecycle starts at planning — there is no interactive briefing
 * conversation. On success it lands on the unified initiative console (`/initiatives?initiativeId=`),
 * where planning → development → review → done are just statuses of the one Initiative.
 */
@Component({
  selector: 'app-briefing',
  standalone: true,
  imports: [
    CommonModule,
    FormsModule,
    IonHeader,
    IonToolbar,
    IonTitle,
    IonContent,
    IonButton,
    IonInput,
    IonTextarea,
    IonItem,
    IonLabel,
    IonList,
    IonNote,
    IonButtons,
    IonSelect,
    IonSelectOption,
    IonToggle,
    IonModal,
    IonCard,
    IonCardHeader,
    IonCardTitle,
    IonCardContent,
    IonSpinner,
  ],
  templateUrl: './briefing.page.html',
  styleUrls: ['./briefing.page.scss'],
})
export class BriefingPage implements OnInit {
  private readonly api = inject(JohnnyApiService);
  private readonly router = inject(Router);

  protected createTitle = '';
  protected createWorkspacePath = '';
  protected createRawAsk = '';
  protected createProvider = 'claude_code';
  protected readonly creating = signal(false);
  protected readonly createError = signal<string | null>(null);

  // Agent (provider) choices come from the host's detected CLIs — a dropdown, not free text.
  protected readonly detectedTools = signal<DetectedCliTool[]>([]);
  protected readonly providerChoices = computed<string[]>(() => {
    const found = this.detectedTools()
      .filter((t) => t.found)
      .map((t) => t.provider)
      .filter((p) => p !== 'shell');
    const known = providerOptions();
    return [...new Set([...found, ...known])];
  });
  protected readonly agentChoiceList = computed(() =>
    agentChoices(this.detectedTools(), providerOptions()),
  );
  protected readonly klooSelected = computed(() => this.createProvider === 'kloo');
  protected readonly doctor = signal<DoctorSnapshot | null>(null);
  protected readonly preflightState = signal<PreflightState>('idle');
  protected readonly preflight = computed<PreflightView>(() => preflightView(this.preflightState()));
  protected readonly escalationCopy = ESCALATION_COPY;
  protected readonly commitCopy = COMMIT_COPY;
  protected readonly executorRows = computed(() => {
    const d = this.doctor();
    return {
      endpoint: d?.endpoint || 'openrouter',
      keySet: d?.api_key && typeof d.api_key === 'object' ? !!d.api_key.set : true,
      model: d?.model || FALLBACK_EXECUTOR.model,
      ctx: typeof d?.ctx === 'number' ? d.ctx : FALLBACK_EXECUTOR.ctx,
    };
  });

  // Per-initiative validation lenses, editable AT creation (seeded from the default triad). Saved to
  // the new initiative right after it is created, so each initiative owns its own config from birth.
  protected readonly createLenses = signal<LensDraft[]>(defaultLenses());

  // Host directory browser (workspace picker) — navigates via listDir instead of free-text entry.
  protected readonly dirBrowserOpen = signal(false);
  protected readonly dirListing = signal<DirListing | null>(null);
  protected readonly dirLoading = signal(false);
  protected readonly dirError = signal<string | null>(null);

  ngOnInit(): void {
    // Populate the agent dropdown from the host's detected CLIs.
    void this.loadDetectedTools();
  }

  private async loadDetectedTools(): Promise<void> {
    try {
      const tools = await firstValueFrom(this.api.detectCliTools());
      this.detectedTools.set(tools ?? []);
      // Default to the first detected agent if the current pick isn't installed.
      if (!tools?.some((t) => t.provider === this.createProvider && t.found)) {
        const first = this.providerChoices()[0];
        if (first) this.createProvider = first;
      }
    } catch {
      // Non-fatal: fall back to the static provider list in `providerChoices`.
    }
  }

  // ── Validation lens editor (per-initiative, at creation) ────────────────────────────────────────
  /** The built-in lenses that have a named checklist in review-lenses.md (no prompt required). */
  private static readonly NAMED_LENSES = new Set(['product', 'qa', 'lead', 'reviewer']);
  protected isNamedLens(name: string): boolean {
    return BriefingPage.NAMED_LENSES.has((name ?? '').trim().toLowerCase());
  }
  protected addValidationLens(): void {
    this.createLenses.update((list) => addLens(list));
  }
  protected removeValidationLens(index: number): void {
    this.createLenses.update((list) => removeLens(list, index));
  }
  protected setLensField(index: number, field: keyof LensDraft, value: string | boolean): void {
    this.createLenses.update((list) =>
      list.map((lens, i) => (i === index ? { ...lens, [field]: value } : lens)),
    );
  }

  // ── Workspace directory browser ─────────────────────────────────────────────────────────────────
  protected async openDirBrowser(): Promise<void> {
    this.dirBrowserOpen.set(true);
    await this.browseTo('');
  }
  protected closeDirBrowser(): void {
    this.dirBrowserOpen.set(false);
  }
  protected async browseTo(relPath: string): Promise<void> {
    this.dirLoading.set(true);
    this.dirError.set(null);
    try {
      const listing = await firstValueFrom(this.api.listDir(relPath));
      this.dirListing.set(listing);
    } catch (err) {
      this.dirError.set(err instanceof Error ? err.message : String(err));
    } finally {
      this.dirLoading.set(false);
    }
  }
  /** Directories in the current listing (files are hidden — we pick a folder). */
  protected browserDirs = computed(() =>
    (this.dirListing()?.entries ?? []).filter((e) => e.kind === 'dir'),
  );
  /** Go up one level from the current relative path (no-op at the root). */
  protected async dirUp(): Promise<void> {
    const cur = this.dirListing()?.path ?? '';
    if (!cur) return;
    const parent = cur.includes('/') ? cur.slice(0, cur.lastIndexOf('/')) : '';
    await this.browseTo(parent);
  }
  /** Absolute path of the currently-browsed directory (root + relative). */
  private currentAbsoluteDir(): string {
    const l = this.dirListing();
    if (!l) return '';
    const root = (l.root || '').replace(/\/+$/, '');
    return l.path ? `${root}/${l.path}` : root;
  }
  /** Choose the currently-open directory as the workspace path. */
  protected useCurrentDir(): void {
    const abs = this.currentAbsoluteDir();
    if (abs) this.createWorkspacePath = abs;
    this.dirBrowserOpen.set(false);
  }

  protected async onAgentChange(value: string): Promise<void> {
    this.createProvider = value;
    if (value === 'kloo') {
      await this.refreshDoctor();
    }
  }

  private async refreshDoctor(): Promise<void> {
    try {
      const raw = await firstValueFrom(this.api.getKlooDoctor());
      this.doctor.set(parseDoctorJson(raw));
    } catch {
      this.doctor.set(null);
    }
  }

  protected async runPreflight(): Promise<void> {
    this.preflightState.set('running');
    try {
      const doctorRaw = await firstValueFrom(this.api.getKlooDoctor());
      const doctor = parseDoctorJson(doctorRaw);
      this.doctor.set(doctor);
      let probe = null;
      try {
        const probeRaw = await firstValueFrom(this.api.getKlooProbe());
        probe = parseProbeJson(probeRaw);
      } catch (err) {
        this.preflightState.set({
          error: err instanceof Error ? err.message : String(err),
        });
        return;
      }
      if (!doctor) {
        this.preflightState.set({ error: 'kloo not found' });
        return;
      }
      this.preflightState.set({ doctor, probe });
    } catch (err) {
      this.preflightState.set({
        error: err instanceof Error ? err.message : String(err),
      });
    }
  }

  // ── Create → planning ─────────────────────────────────────────────────────────────────────────
  protected createBriefing(): void {
    const workspacePath = this.createWorkspacePath.trim();
    const brief = this.createRawAsk.trim();
    if (!workspacePath || !brief) {
      this.createError.set('A workspace directory and a brief are both required.');
      return;
    }
    this.creating.set(true);
    this.createError.set(null);
    const commercial = this.providerChoices().find((p) => p !== 'kloo') || 'claude_code';
    const payload = createPayload(
      this.createProvider,
      commercial,
      { title: this.createTitle.trim() || undefined, workspacePath, brief },
      this.doctor(),
    );
    this.api
      .createBriefingInitiative({
        title: payload.title,
        workspacePath: payload.workspacePath,
        brief: payload.brief,
        workerProvider: payload.workerProvider,
        reviewerProvider: payload.reviewerProvider,
        executorConfig: payload.executorConfig,
      })
      .subscribe({
        next: async (run) => {
          // Persist THIS initiative's own validation lenses right after creation, so the config is
          // per-initiative from the start (not the shared default template). Best-effort: a failed
          // save still leaves the initiative on the default template, so it must not block the flow.
          try {
            await firstValueFrom(
              this.api.updateAgentPlanValidationConfig(run.plan.id, toConfigJson(this.createLenses())),
            );
          } catch {
            // Non-fatal — the initiative keeps the default template; user can Configure later.
          }
          // Briefing step removed: go STRAIGHT to planning. Accept immediately (the brief IS the ask)
          // — this flips the initiative to planning and starts the T1 planner — then land on the
          // UNIFIED initiative console (planning/development/review/done are just statuses of the one
          // Initiative; the legacy /planning + /development split-pages are not the target).
          try {
            await firstValueFrom(this.api.acceptInitiativeBrief({ initiativeId: run.plan.id }));
            this.creating.set(false);
            void this.router.navigate(['/initiatives'], {
              queryParams: { initiativeId: run.plan.id },
            });
          } catch (err) {
            this.creating.set(false);
            this.createError.set('Created, but failed to start planning: ' + String(err));
          }
        },
        error: (err) => {
          this.creating.set(false);
          this.createError.set(String(err));
        },
      });
  }
}
