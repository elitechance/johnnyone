import { Component, OnDestroy, OnInit, computed, effect, inject, signal, untracked } from '@angular/core';
import { FormsModule } from '@angular/forms';
import { ActivatedRoute, Router, RouterLink } from '@angular/router';
import {
  AlertController,
  IonBackButton,
  IonBadge,
  IonButton,
  IonButtons,
  IonCard,
  IonCardContent,
  IonChip,
  IonContent,
  IonFooter,
  IonHeader,
  IonIcon,
  IonLabel,
  IonNote,
  IonSegment,
  IonSegmentButton,
  IonSkeletonText,
  IonSpinner,
  IonTextarea,
  IonTitle,
  IonToolbar,
  ToastController,
} from '@ionic/angular/standalone';
import { addIcons } from 'ionicons';
import {
  addOutline,
  alertCircleOutline,
  arrowDownOutline,
  arrowUndoOutline,
  arrowUpOutline,
  closeOutline,
  createOutline,
  lockClosedOutline,
  searchOutline,
  trashOutline,
} from 'ionicons/icons';
import {
  AiChatService,
  ChatWindowComponent,
  JohnnyApiService,
  StatusPillComponent,
  type AgentPlan,
  type BriefCheckItem,
  type BriefDiff,
  type BubbleDisplay,
  type BriefVersion,
  type BriefVersionMeta,
} from '@johnnyone/ui';
import { Subscription, firstValueFrom } from 'rxjs';
import { RelayTerminalService } from '../../services/relay-terminal.service';
import { BriefFieldComponent } from './components/brief-panel/brief-panel.component';
import { OutlineRowComponent } from './components/outline-panel/outline-panel.component';
import { VersionChipComponent } from './components/version-strip/version-strip.component';
import {
  changedCounts,
  checkRows,
  detailState,
  diffBase,
  diffView,
  fieldRows,
  isFrozen,
  outlineCards,
  startGate,
  summaryLine,
  versionChips,
  whereItems,
  type FieldRow,
  type OutlinePhaseCard,
  type OutlineTaskRow,
} from './plan-brief-logic';
import { AGENT_PILL_LABEL, type CopyState, agentPane, agentState, copyLabel, copyResetMs, composerState, extractChoices, toolSummary } from './plan-chat-logic';
import { POLL_MS, hasNewerVersion, isDeletedUpdate, pollEnabled, shouldRefetch, turnCompleted } from './plan-live-logic';

type Pane = 'chat' | 'brief' | 'outline';

/**
 * `/plans/:id` — one draft plan (Plans tab, initiative 8ad3ae71, phase 03 task 08; screens S5-S11).
 *
 * The 08 layout: the plan agent chat on the left (the shared `johnny-chat-window`, its bubbles
 * showing the agent's MCP tool calls as compact rows), the live structured brief on the right
 * (version strip → diff, six fields with who·version·time, changed highlight, undo, direct edit,
 * brief check, draft outline), a sticky Start / Queue planning bar, and a Chat | Brief | Outline
 * segment under 900px. The header is the title + lifecycle pill only (D16).
 *
 * A container: it owns every API call and every handler; the components under `components/` only
 * render (the container projects their action buttons). Every decision is a pure function in
 * `plan-brief-logic.ts`, `plan-chat-logic.ts` and `plan-live-logic.ts`.
 */
@Component({
  selector: 'app-plan-detail',
  standalone: true,
  imports: [
    FormsModule,
    RouterLink,
    IonHeader,
    IonToolbar,
    IonTitle,
    IonButtons,
    IonBackButton,
    IonButton,
    IonIcon,
    IonContent,
    IonFooter,
    IonCard,
    IonCardContent,
    IonChip,
    IonLabel,
    IonBadge,
    IonSegment,
    IonSegmentButton,
    IonSkeletonText,
    IonSpinner,
    IonTextarea,
    IonNote,
    ChatWindowComponent,
    StatusPillComponent,
    BriefFieldComponent,
    OutlineRowComponent,
    VersionChipComponent,
  ],
  templateUrl: './plan-detail.page.html',
  styleUrl: './plan-detail.page.scss',
})
export class PlanDetailPage implements OnInit, OnDestroy {
  private readonly api = inject(JohnnyApiService);
  private readonly route = inject(ActivatedRoute);
  private readonly router = inject(Router);
  private readonly relay = inject(RelayTerminalService);
  private readonly alertCtrl = inject(AlertController);
  private readonly toastCtrl = inject(ToastController);
  protected readonly chat = inject(AiChatService);

  private pushSub?: Subscription;
  private pollTimer: ReturnType<typeof setInterval> | null = null;
  private watchTimer: ReturnType<typeof setTimeout> | null = null;
  private destroyed = false;
  private readonly onVisibility = () => this.syncPoll();

  protected readonly id = signal('');
  protected readonly plan = signal<AgentPlan | null>(null);
  protected readonly latest = signal<BriefVersion | null>(null);
  protected readonly versions = signal<BriefVersionMeta[]>([]);
  protected readonly checks = signal<BriefCheckItem[]>([]);
  protected readonly loading = signal(true);
  protected readonly error = signal<string | null>(null);
  protected readonly busy = signal(false);
  private readonly now = signal(new Date().toISOString());

  protected readonly pane = signal<Pane>('chat');
  protected readonly selectedVersion = signal<number | null>(null);
  protected readonly diff = signal<BriefDiff | null>(null);
  protected readonly diffLoading = signal(false);
  /** The field being edited inline (null = none) and its draft text / new `where` item. */
  protected readonly editing = signal<string | null>(null);
  protected editText = '';
  protected newWhere = '';

  // ── derived view models ──
  protected readonly state = computed(() => detailState(this.loading(), this.error(), this.plan()));
  protected readonly frozen = computed(() => this.state() === 'frozen' || isFrozen(this.latest()));
  protected readonly rows = computed<FieldRow[]>(() => {
    const l = this.latest();
    return l ? fieldRows(l, this.versions(), this.now()) : [];
  });
  protected readonly chips = computed(() => versionChips(this.versions()));
  protected readonly badges = computed(() => changedCounts(this.latest()));
  protected readonly checkList = computed(() => checkRows(this.checks()));
  protected readonly gate = computed(() =>
    startGate(this.checks(), this.plan()?.initiativeStatus, this.frozen()),
  );
  protected readonly summary = computed(() => summaryLine(this.latest()?.version, this.checks()));
  protected readonly outline = computed<OutlinePhaseCard[]>(() =>
    outlineCards(this.latest()?.outline, this.latest()?.changed),
  );
  protected readonly diffModel = computed(() => diffView(this.diff()));
  protected readonly frozenAt = computed(
    () => this.plan()?.acceptedBriefVersion ?? this.latest()?.acceptedBriefVersion ?? this.latest()?.version ?? 0,
  );
  protected readonly queued = computed(() => !!this.plan()?.planningQueuedAfter);

  // ── chat ──
  protected readonly composer = computed(() =>
    composerState(this.frozen() ? 'planning' : this.plan()?.initiativeStatus, this.chat.isStreaming()),
  );
  protected readonly agentLabel = AGENT_PILL_LABEL;
  /** The agent's tmux pane (name + Raw view link), once its first turn created it and while it is live. */
  protected readonly agentPane = computed(() =>
    agentPane(
      this.plan()?.briefingSessionId,
      this.frozen() ? 'planning' : this.plan()?.initiativeStatus,
      this.chat.messages().length,
    ),
  );
  protected readonly paneCopy = signal<CopyState>('idle');
  protected readonly copyLabel = copyLabel;
  private copyResetTimer: ReturnType<typeof setTimeout> | null = null;

  /** Show a Copy result, then go back to `Copy` after [`copyResetMs`]. */
  private setPaneCopy(state: CopyState): void {
    this.paneCopy.set(state);
    if (this.copyResetTimer) clearTimeout(this.copyResetTimer);
    const ms = copyResetMs(state);
    this.copyResetTimer = ms === null ? null : setTimeout(() => this.paneCopy.set('idle'), ms);
  }

  /**
   * Copy `johnnyone_<sid>` for `tmux attach -r -t …`. When the clipboard is unavailable (no secure
   * context, permission denied) the button says so and the name is SELECTED for a manual copy.
   */
  protected async copyPaneName(name: string, el?: HTMLElement): Promise<void> {
    try {
      if (!navigator.clipboard?.writeText) throw new Error('clipboard unavailable');
      await navigator.clipboard.writeText(name);
      this.setPaneCopy('copied');
    } catch {
      this.setPaneCopy('failed');
      if (el) {
        const range = document.createRange();
        range.selectNodeContents(el);
        const sel = window.getSelection();
        sel?.removeAllRanges();
        sel?.addRange(range);
      }
    }
  }
  protected readonly agent = computed(() =>
    agentState(this.frozen() ? 'planning' : this.plan()?.initiativeStatus, this.chat.isStreaming()),
  );
  /** Messages with the trailing `CHOICES:` line hidden (it becomes chips). */
  protected readonly messages = computed(() =>
    this.chat.messages().map((m) => (m.role === 'assistant' ? { ...m, content: extractChoices(m.content).text } : m)),
  );
  /** Chips from the latest assistant message, only while you can still answer. */
  protected readonly choices = computed<string[]>(() => {
    if (this.frozen() || this.chat.isStreaming()) return [];
    const msgs = this.chat.messages();
    const last = msgs[msgs.length - 1];
    return last && last.role === 'assistant' ? extractChoices(last.content).choices : [];
  });
  protected readonly bubbleDisplay: BubbleDisplay = {
    assistantLabel: 'Plan agent',
    toolRow: (call) => toolSummary(call),
  };

  constructor() {
    addIcons({
      'add-outline': addOutline,
      'alert-circle-outline': alertCircleOutline,
      'arrow-down-outline': arrowDownOutline,
      'arrow-undo-outline': arrowUndoOutline,
      'arrow-up-outline': arrowUpOutline,
      'close-outline': closeOutline,
      'create-outline': createOutline,
      'lock-closed-outline': lockClosedOutline,
      'search-outline': searchOutline,
      'trash-outline': trashOutline,
    });
    // A finished agent turn: re-read the messages (the stored rows carry the MCP tool calls) and the
    // brief (the push normally beats us to it; this covers a dropped socket).
    let wasStreaming = false;
    effect(() => {
      const streaming = this.chat.isStreaming();
      if (turnCompleted(wasStreaming, streaming)) {
        untracked(() => {
          this.chat.refreshMessages();
          void this.loadBrief();
        });
      }
      wasStreaming = streaming;
    });
  }

  ngOnInit(): void {
    this.id.set(this.route.snapshot.paramMap.get('id') ?? '');
    void this.load();
    this.pushSub = this.relay.agentPlanRuns().subscribe((u) => {
      const id = this.id();
      if (isDeletedUpdate(u, id)) {
        void this.leaveDeleted();
        return;
      }
      const p = this.plan();
      if (shouldRefetch(u, id, { brief: p?.brief, initiativeStatus: p?.initiativeStatus })) void this.loadBrief();
    });
    void this.relay.connect().catch(() => undefined);
    document.addEventListener('visibilitychange', this.onVisibility);
    this.watchConnection();
  }

  ngOnDestroy(): void {
    this.destroyed = true;
    this.pushSub?.unsubscribe();
    this.stopPoll();
    if (this.watchTimer) clearTimeout(this.watchTimer);
    if (this.copyResetTimer) clearTimeout(this.copyResetTimer);
    document.removeEventListener('visibilitychange', this.onVisibility);
    this.chat.teardownSubscriptions();
  }

  // ── loading ──

  /** The full load (Retry is bound to it): plan row, brief, versions, check, then the chat session. */
  protected async load(): Promise<void> {
    this.loading.set(true);
    this.error.set(null);
    try {
      const run = await firstValueFrom(this.api.getAgentPlan(this.id()));
      this.plan.set(run.plan);
      if (run.plan.status !== 'closed') {
        await this.loadBrief(false);
        const sid = run.plan.briefingSessionId;
        if (sid && this.chat.currentSession()?.id !== sid) this.chat.loadSession(sid);
        else if (sid) this.chat.refreshMessages();
      }
    } catch (err) {
      this.error.set(`Couldn't load this plan: ${message(err)}`);
    } finally {
      this.loading.set(false);
      this.syncPoll();
    }
  }

  /** Refetch the brief side (version, versions, check) and the plan row. */
  private async loadBrief(withPlan = true): Promise<void> {
    const id = this.id();
    if (!id) return;
    try {
      const [plan, latest, versions, checks] = await Promise.all([
        withPlan ? firstValueFrom(this.api.getAgentPlan(id)).then((r) => r.plan) : Promise.resolve(this.plan()),
        firstValueFrom(this.api.getBriefVersion(id)).catch(() => null),
        firstValueFrom(this.api.listBriefVersions(id)).catch(() => [] as BriefVersionMeta[]),
        firstValueFrom(this.api.getBriefCheck(id)).catch(() => [] as BriefCheckItem[]),
      ]);
      if (plan) this.plan.set(plan);
      this.latest.set(latest);
      this.versions.set(versions ?? []);
      this.checks.set(checks ?? []);
      this.now.set(new Date().toISOString());
    } catch (err) {
      // A background refresh failing keeps the page; a toast says why.
      if (!this.loading()) await this.toast(`Couldn't refresh: ${message(err)}`, 'danger');
    }
  }

  private async leaveDeleted(): Promise<void> {
    await this.toast('This plan was deleted.', 'danger');
    void this.router.navigate(['/plans']);
  }

  // ── live: poll fallback (D7) ──

  /** Poll `listBriefVersions` every 5 s only while the relay is down and the tab is visible. */
  private syncPoll(): void {
    if (this.destroyed) return;
    const visible = typeof document === 'undefined' || document.visibilityState !== 'hidden';
    if (pollEnabled(this.relay.isConnected(), visible)) {
      if (!this.pollTimer) this.pollTimer = setInterval(() => void this.pollOnce(), POLL_MS);
    } else {
      this.stopPoll();
    }
  }

  private stopPoll(): void {
    if (this.pollTimer) clearInterval(this.pollTimer);
    this.pollTimer = null;
  }

  private async pollOnce(): Promise<void> {
    try {
      const versions = await firstValueFrom(this.api.listBriefVersions(this.id()));
      const newest = versions.reduce((m, v) => Math.max(m, v.version), 0);
      if (hasNewerVersion(newest, this.latest()?.version)) await this.loadBrief();
    } catch {
      // Still offline: the next tick tries again.
    }
  }

  /** Cheap local check (no network) that re-evaluates the poll gate and retries the socket. */
  private watchConnection(): void {
    this.watchTimer = setTimeout(() => {
      if (this.destroyed) return;
      if (!this.relay.isConnected() && document.visibilityState !== 'hidden') {
        void this.relay.connect().catch(() => undefined);
      }
      this.syncPoll();
      this.watchConnection();
    }, POLL_MS);
  }

  // ── chat ──

  protected send(text: string): void {
    const t = (text ?? '').trim();
    if (!t || this.composer().disabled) return;
    this.chat.sendMessage(t);
  }

  // ── phone segment ──

  protected setPane(value: unknown): void {
    if (value === 'chat' || value === 'brief' || value === 'outline') this.pane.set(value);
  }

  // ── version strip + diff ──

  protected async showDiff(version: number): Promise<void> {
    if (this.selectedVersion() === version) {
      this.clearDiff();
      return;
    }
    this.selectedVersion.set(version);
    const base = diffBase(version);
    if (base === null) {
      this.diff.set(null);
      return;
    }
    this.diffLoading.set(true);
    try {
      this.diff.set(await firstValueFrom(this.api.getBriefDiff(this.id(), base, version)));
    } catch (err) {
      await this.toast(message(err), 'danger');
      this.clearDiff();
    } finally {
      this.diffLoading.set(false);
    }
  }

  protected clearDiff(): void {
    this.selectedVersion.set(null);
    this.diff.set(null);
  }

  // ── fields: edit + undo ──

  protected startEdit(row: FieldRow): void {
    this.editing.set(row.key);
    this.editText = row.text;
    this.newWhere = '';
  }

  protected cancelEdit(): void {
    this.editing.set(null);
  }

  /** Save a text field (author you → a new version). */
  protected async saveField(row: FieldRow): Promise<void> {
    await this.write(async () => {
      await firstValueFrom(this.api.updateBriefField(this.id(), row.key, 'value', this.editText));
      this.editing.set(null);
    }, 'Saved');
  }

  /** Add the typed `where` items (one per line). */
  protected async addWhere(): Promise<void> {
    const items = whereItems(this.newWhere);
    if (!items.length) return;
    const next = [...(this.rows().find((r) => r.key === 'where')?.chips ?? []), ...items];
    await this.write(async () => {
      await firstValueFrom(this.api.updateBriefField(this.id(), 'where', 'value', null, next));
      this.newWhere = '';
    }, 'Saved');
  }

  protected async removeWhere(item: string): Promise<void> {
    await this.write(() => firstValueFrom(this.api.updateBriefField(this.id(), 'where', 'remove', item)), 'Removed');
  }

  /** Per-field undo: a NEW version copying that field from the version before its latest change. */
  protected async undo(row: FieldRow): Promise<void> {
    if (!row.canUndo || row.undoTo == null) return;
    const to = row.undoTo;
    await this.write(() => firstValueFrom(this.api.resetBriefToVersion(this.id(), to, row.key)), `Undone to v${to}`);
  }

  // ── outline ──

  protected async addPhase(): Promise<void> {
    const title = await this.prompt('Add phase', '');
    if (!title) return;
    await this.write(() => firstValueFrom(this.api.addOutlinePhase(this.id(), title)), 'Phase added');
  }

  protected async addTask(phase: OutlinePhaseCard): Promise<void> {
    const title = await this.prompt(`Add task to ${phase.id}`, '');
    if (!title) return;
    await this.write(() => firstValueFrom(this.api.addOutlineTask(this.id(), phase.id, title)), 'Task added');
  }

  protected async renameItem(item: OutlinePhaseCard | OutlineTaskRow): Promise<void> {
    const title = await this.prompt(`Rename ${item.id}`, item.title);
    if (!title || title === item.title) return;
    await this.write(() => firstValueFrom(this.api.updateOutlineItem(this.id(), item.id, title)), 'Renamed');
  }

  /** Move within the same parent; `index` is the item's current index, `delta` ±1. */
  protected async moveItem(item: OutlinePhaseCard | OutlineTaskRow, index: number, delta: number): Promise<void> {
    const to = index + delta;
    if (to < 0) return;
    await this.write(() => firstValueFrom(this.api.updateOutlineItemPosition(this.id(), item.id, to)), 'Moved');
  }

  protected async deleteItem(item: OutlinePhaseCard | OutlineTaskRow): Promise<void> {
    const alert = await this.alertCtrl.create({
      header: `Delete ${item.id}?`,
      message: `“${item.title}” is removed from the draft outline (undoable from the version strip).`,
      buttons: [
        { text: 'Cancel', role: 'cancel' },
        { text: 'Delete', role: 'destructive' },
      ],
    });
    await alert.present();
    const { role } = await alert.onDidDismiss();
    if (role !== 'destructive') return;
    await this.write(() => firstValueFrom(this.api.deleteOutlineItem(this.id(), item.id)), 'Deleted');
  }

  // ── sticky bar ──

  /** Queue planning behind the conflicting initiatives (or clear the queue when already queued). */
  protected async queuePlanning(): Promise<void> {
    const clear = this.queued();
    const after = clear ? null : this.gate().queueAfter;
    await this.write(async () => {
      const run = await firstValueFrom(this.api.updatePlanningQueue(this.id(), after));
      this.plan.set(run.plan);
    }, clear ? 'Queue cleared' : 'Queued: planning starts when the conflicting initiative finishes');
  }

  /** Start planning. Failed checks ask first (D11) — they never block. */
  protected async startPlanning(): Promise<void> {
    const gate = this.gate();
    if (!gate.canStart || this.busy()) return;
    if (gate.needsConfirm) {
      const alert = await this.alertCtrl.create({
        header: 'Brief check has failures',
        message: gate.confirmLines.map((l) => `• ${l}`).join('\n'),
        cssClass: 'plan-start-confirm',
        buttons: [
          { text: 'Keep editing', role: 'cancel' },
          { text: 'Start anyway', role: 'confirm' },
        ],
      });
      await alert.present();
      const { role } = await alert.onDidDismiss();
      if (role !== 'confirm') return;
    }
    this.busy.set(true);
    try {
      await firstValueFrom(this.api.acceptInitiativeBrief({ initiativeId: this.id() }));
      void this.router.navigate(['/initiatives'], { queryParams: { initiativeId: this.id() } });
    } catch (err) {
      await this.toast(message(err), 'danger');
    } finally {
      this.busy.set(false);
    }
  }

  // ── helpers ──

  /** Run one write with the controls disabled; then refetch the brief. */
  private async write(fn: () => Promise<unknown>, done: string): Promise<void> {
    if (this.busy() || this.frozen()) return;
    this.busy.set(true);
    try {
      await fn();
      await this.loadBrief();
      await this.toast(done);
    } catch (err) {
      await this.toast(message(err), 'danger');
    } finally {
      this.busy.set(false);
    }
  }

  private async prompt(header: string, value: string): Promise<string | null> {
    const alert = await this.alertCtrl.create({
      header,
      inputs: [{ name: 'title', type: 'text', value, placeholder: 'One line', attributes: { maxlength: 200 } }],
      buttons: [
        { text: 'Cancel', role: 'cancel' },
        { text: 'Save', role: 'confirm' },
      ],
    });
    await alert.present();
    const { role, data } = await alert.onDidDismiss();
    const t = String(data?.values?.title ?? '').trim();
    return role === 'confirm' && t ? t : null;
  }

  private async toast(text: string, color: 'success' | 'danger' = 'success'): Promise<void> {
    const t = await this.toastCtrl.create({ message: text, duration: 2200, color, position: 'bottom' });
    await t.present();
  }
}

function message(err: unknown): string {
  return err instanceof Error ? err.message : String(err);
}
