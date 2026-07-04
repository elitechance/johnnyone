import { CommonModule } from '@angular/common';
import { Component, OnDestroy, OnInit, computed, inject, signal } from '@angular/core';
import { FormsModule } from '@angular/forms';
import { ActivatedRoute, Router } from '@angular/router';
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
} from '@ionic/angular/standalone';
import {
  AiChatService,
  ChatWindowComponent,
  JohnnyApiService,
  AgentPlan,
  DetectedCliTool,
  DirListing,
} from '@johnnyone/ui';
import { firstValueFrom } from 'rxjs';
import { BriefingComposerComponent } from '../../components/briefing-composer/briefing-composer.component';
import { canAccept, composeBriefingSeed, shouldSeed } from './briefing-page-logic';
import {
  LensDraft,
  defaultLenses,
  addLens,
  removeLens,
  toConfigJson,
  providerOptions,
} from '../validation-config/validation-config-logic';

/**
 * The briefing view (mock §07, overhaul P4). Reuses `johnny-chat-window` for the conversation
 * (`message-bubble` renders via the P3 markdown core), supplies the new `app-briefing-composer` and
 * the Accept bar through the window's `[chat-actions]` slot, and drives the multi-turn conversation
 * with the existing `AiChatService` on the initiative's `briefingSessionId` (D2). Accept flips the
 * same initiative briefing→planning (D1/D4) and navigates to its planning run. `briefing/new` shows a
 * minimal create form so the flow is reachable (the full rail launcher is Phase 6).
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
    ChatWindowComponent,
    BriefingComposerComponent,
  ],
  templateUrl: './briefing.page.html',
  styleUrls: ['./briefing.page.scss'],
})
export class BriefingPage implements OnInit, OnDestroy {
  private readonly api = inject(JohnnyApiService);
  private readonly chat = inject(AiChatService);
  private readonly router = inject(Router);
  private readonly route = inject(ActivatedRoute);

  /** Conversation state comes straight from the reused AiChatService signals. */
  protected readonly messages = this.chat.messages;
  protected readonly streaming = this.chat.isStreaming;

  protected readonly initiativeId = signal<string | null>(null);
  protected readonly plan = signal<AgentPlan | null>(null);
  protected readonly loadError = signal<string | null>(null);
  protected readonly accepting = signal(false);

  /** Pending attachment paths surfaced by the composer (folded into a re-seed if needed). */
  protected readonly attachmentPaths = signal<string[]>([]);
  private seeded = false;

  // ── Create form (briefing/new) ──────────────────────────────────────────────────────────────────
  protected readonly isCreate = computed(() => this.initiativeId() === null);
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
    // Union the detected providers with the known reviewer-capable set, de-duped, detected first.
    const known = providerOptions();
    return [...new Set([...found, ...known])];
  });

  // Per-initiative validation lenses, editable AT creation (seeded from the default triad). Saved to
  // the new initiative right after it is created, so each initiative owns its own config from birth.
  protected readonly createLenses = signal<LensDraft[]>(defaultLenses());

  // Host directory browser (workspace picker) — navigates via listDir instead of free-text entry.
  protected readonly dirBrowserOpen = signal(false);
  protected readonly dirListing = signal<DirListing | null>(null);
  protected readonly dirLoading = signal(false);
  protected readonly dirError = signal<string | null>(null);

  protected readonly canAcceptNow = computed(() => canAccept(this.plan()?.initiativeStatus));

  ngOnInit(): void {
    const id = this.route.snapshot.paramMap.get('initiativeId');
    this.initiativeId.set(id);
    if (id) {
      this.loadInitiative(id);
    } else {
      // Create form: populate the agent dropdown from the host's detected CLIs.
      void this.loadDetectedTools();
    }
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

  ngOnDestroy(): void {
    this.chat.teardownSubscriptions();
  }

  private loadInitiative(id: string): void {
    this.api.getAgentPlan(id).subscribe({
      next: (run) => {
        const plan = run.plan;
        this.plan.set(plan);
        const sessionId = plan.briefingSessionId;
        if (!sessionId) {
          this.loadError.set('This initiative has no briefing conversation.');
          return;
        }
        this.chat.loadSession(sessionId);
        // Seed the first turn once, only if the conversation is still empty (D2).
        this.api.listMessages(sessionId).subscribe((rows) => {
          if (!this.seeded && shouldSeed(rows)) {
            this.seeded = true;
            const refs = (plan.referencePaths ?? '')
              .split('\n')
              .map((line) => line.trim())
              .filter(Boolean);
            this.chat.sendMessage(
              composeBriefingSeed(plan.brief ?? '', this.attachmentPaths(), refs),
            );
          }
        });
      },
      error: (err) => this.loadError.set(String(err)),
    });
  }

  // ── Conversation ────────────────────────────────────────────────────────────────────────────────
  protected onSend(text: string): void {
    this.chat.sendMessage(text);
  }

  protected onAttachmentsChanged(paths: string[]): void {
    this.attachmentPaths.set(paths);
  }

  protected onReferencePathAdded(_path: string): void {
    // The host already recorded it (addInitiativeReferencePath); nothing else to do here.
  }

  // ── Accept → planning (same initiative) ─────────────────────────────────────────────────────────
  protected onAccept(): void {
    const id = this.initiativeId();
    if (!id || !this.canAcceptNow() || this.accepting()) return;
    this.accepting.set(true);
    this.api.acceptInitiativeBrief({ initiativeId: id }).subscribe({
      next: () => {
        this.accepting.set(false);
        void this.router.navigateByUrl('/planning/' + id);
      },
      error: (err) => {
        this.accepting.set(false);
        this.loadError.set(String(err));
      },
    });
  }

  protected onKeepRefining(): void {
    // No-op: stay in the briefing conversation (mock §07 "Keep refining").
  }

  // ── Create (briefing/new) ───────────────────────────────────────────────────────────────────────
  protected createBriefing(): void {
    if (this.creating()) return;
    const workspacePath = this.createWorkspacePath.trim();
    const brief = this.createRawAsk.trim();
    if (!workspacePath || !brief) {
      this.createError.set('Workspace path and initial ask are required.');
      return;
    }
    this.creating.set(true);
    this.createError.set(null);
    this.api
      .createBriefingInitiative({
        title: this.createTitle.trim() || undefined,
        workspacePath,
        brief,
        workerProvider: this.createProvider,
        reviewerProvider: this.createProvider,
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
          this.creating.set(false);
          void this.router.navigateByUrl('/briefing/' + run.plan.id);
        },
        error: (err) => {
          this.creating.set(false);
          this.createError.set(String(err));
        },
      });
  }
}
