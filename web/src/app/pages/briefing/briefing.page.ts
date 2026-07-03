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
  IonInput,
  IonTextarea,
  IonItem,
  IonLabel,
  IonList,
  IonNote,
} from '@ionic/angular/standalone';
import {
  AiChatService,
  ChatWindowComponent,
  JohnnyApiService,
  AgentPlan,
} from '@johnnyone/ui';
import { BriefingComposerComponent } from '../../components/briefing-composer/briefing-composer.component';
import { canAccept, composeBriefingSeed, shouldSeed } from './briefing-page-logic';

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

  protected readonly canAcceptNow = computed(() => canAccept(this.plan()?.initiativeStatus));

  ngOnInit(): void {
    const id = this.route.snapshot.paramMap.get('initiativeId');
    this.initiativeId.set(id);
    if (id) {
      this.loadInitiative(id);
    }
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
        next: (run) => {
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
