import { CommonModule } from '@angular/common';
import { Component, OnInit, computed, inject, signal } from '@angular/core';
import { Router } from '@angular/router';
import {
  IonHeader,
  IonToolbar,
  IonTitle,
  IonButtons,
  IonButton,
  IonContent,
  IonIcon,
  IonSpinner,
  IonRefresher,
  IonRefresherContent,
  AlertController,
  PopoverController,
} from '@ionic/angular/standalone';
import { addIcons } from 'ionicons';
import { addOutline, terminalOutline, gitNetworkOutline, refreshOutline } from 'ionicons/icons';
import { JohnnyApiService, type AiSession, type TmuxSession } from '@johnnyone/ui';
import { firstValueFrom } from 'rxjs';
import { LauncherMenuComponent } from '../../components/launcher-menu/launcher-menu.component';
import { attachTmuxInput } from '../../components/launcher-menu/launcher-logic';
import {
  partitionShells,
  attachableTmux,
  shellSessionLabel,
  formatRelTime,
  openIntent,
  clearTargetIds,
  clearConfirmCopy,
  clearResultMessage,
  needsClearConfirm,
  fulfilledClearIds,
  sessionsWithoutIds,
  addSuppressed,
  applySuppression,
  pruneSuppressed,
  type SuppressionEntries,
} from './shells-page-logic';

/**
 * The Shells destination (`/shells`, overhaul P6). "See your launched shells in one place" — a LIST of
 * active shell/attached-tmux sessions (`listSessions('active')` → `partitionShells`) plus attachable
 * external tmux panes (`listTmuxSessions()` → `attachableTmux`, de-duped). Opening a row NAVIGATES to the
 * existing terminal surface (`/terminal?sessionId=`, D3) — no second terminal is embedded; an attachable
 * row attaches first (`createSession(attachTmuxInput(name))`) then opens. The header `+ New` reuses the
 * Phase-01 `LauncherMenuComponent`. Every decision lives in the pure `shells-page-logic.ts`; this page is
 * thin wiring + IO.
 */
@Component({
  selector: 'app-shells',
  standalone: true,
  imports: [
    CommonModule,
    IonHeader,
    IonToolbar,
    IonTitle,
    IonButtons,
    IonButton,
    IonContent,
    IonIcon,
    IonSpinner,
    IonRefresher,
    IonRefresherContent,
  ],
  templateUrl: './shells.page.html',
  styleUrls: ['./shells.page.scss'],
})
export class ShellsPage implements OnInit {
  private readonly api = inject(JohnnyApiService);
  private readonly router = inject(Router);
  private readonly popoverCtrl = inject(PopoverController);
  private readonly alertCtrl = inject(AlertController);

  protected readonly sessions = signal<AiSession[]>([]);
  protected readonly tmux = signal<TmuxSession[]>([]);
  protected readonly loadError = signal<string | null>(null);
  protected readonly busy = signal(false);
  /**
   * Ids cleared in the last few seconds, held so a STALE read cannot resurrect them. See the
   * suppression notes in `shells-page-logic.ts`: the worker caches `list_sessions` for 5s per
   * isolate, so without this the rows reappear moments after the user cleared them.
   */
  private readonly suppressedIds = signal<SuppressionEntries>({});
  /** Current time captured once per refresh so `formatRelTime` stays pure/deterministic. */
  protected readonly nowIso = signal(new Date().toISOString());

  protected readonly shells = computed(() => partitionShells(this.sessions()));
  protected readonly attachable = computed(() => attachableTmux(this.tmux(), this.sessions()));
  protected readonly isEmpty = computed(() => this.shells().length === 0 && this.attachable().length === 0);

  // Expose the pure helpers to the template.
  protected readonly shellSessionLabel = shellSessionLabel;
  protected readonly formatRelTime = formatRelTime;

  constructor() {
    // Row/badge glyphs — the popover self-registers the launcher icons, but this page renders its own.
    addIcons({
      'add-outline': addOutline,
      'terminal-outline': terminalOutline,
      'git-network-outline': gitNetworkOutline,
      'refresh-outline': refreshOutline,
    });
  }

  ngOnInit(): void {
    void this.refresh();
  }

  // Refresh when the user returns from the terminal surface so the list is current (D8).
  ionViewWillEnter(): void {
    void this.refresh();
  }

  async refresh(): Promise<void> {
    this.busy.set(true);
    try {
      const [sessions, tmux] = await Promise.all([
        firstValueFrom(this.api.listSessions('active')),
        firstValueFrom(this.api.listTmuxSessions()),
      ]);
      this.nowIso.set(new Date().toISOString());
      // Suppression is applied to EVERY read — the automatic reconcile and the manual
      // pull-to-refresh alike. Prune against the RAW read first (an id the host no longer reports
      // has nothing left to suppress), then subtract whatever is still inside its window.
      const raw = sessions ?? [];
      const now = Date.now();
      const entries = pruneSuppressed(this.suppressedIds(), raw, now);
      this.suppressedIds.set(entries);
      this.sessions.set(applySuppression(raw, entries, now));
      this.tmux.set(tmux ?? []);
      this.loadError.set(null);
    } catch (err) {
      this.loadError.set(this.normalizeError(err));
    } finally {
      this.busy.set(false);
    }
  }

  /**
   * Pull-to-refresh (phone surface). Completes the refresher in a `finally` so a failed load can
   * never leave the spinner stuck open — `refresh()` already swallows its own error into
   * `loadError`, but the `finally` makes that independent of it.
   */
  async handleRefresh(ev: Event): Promise<void> {
    try {
      await this.refresh();
    } finally {
      const target = ev?.target as { complete?: () => void } | null;
      target?.complete?.();
    }
  }

  /**
   * Clear the whole "Active shells" list: ARCHIVE every listed session, then refresh.
   *
   * ARCHIVE, NEVER DELETE — and do not "upgrade" this to `deleteSession`/`deleteAiSession`, not even
   * behind a second confirm. `reportAgentResult` is the WHOLE reason, and the only one. The host's
   * `record_agent_report` (`agent_plans.rs:3625`) accepts a narration report from a non-plan session
   * only when `session_exists` says the ROW is still there, and `session_exists`
   * (`agent_plans.rs:4160`) is `SELECT 1 FROM sessions WHERE id = ?1` with NO status filter.
   * Archiving keeps the row, so the reporter keeps working; deleting it would break
   * `reportAgentResult` permanently for that session id — including the live shell this console
   * reports through.
   *
   * NOT a reason, despite being the obvious guess: protecting the user's own tmux. Archive does skip
   * `kill_terminal_session` for an attached row (`sessions.rs:198`) while `delete_session`
   * (`sessions.rs:239-266`) does not — but neither can actually kill an external session, because
   * `tmux_session_name` (`terminal.rs:1388-1400`) is unconditionally `johnnyone_<id>` and never reads
   * the stored external name. So tmux safety is not what archiving buys; the reporter is.
   */
  async clearAll(): Promise<void> {
    if (this.busy()) return;
    const rows = this.shells();
    const ids = clearTargetIds(rows);
    if (ids.length === 0) return;

    // Liveness comes from the RAW `tmux()` signal, not `attachable()` — the latter has already
    // filtered out the very panes these rows are attached to (F1).
    const copy = clearConfirmCopy(rows, this.tmux().map((t) => t.name));
    const alert = await this.alertCtrl.create({
      header: copy.header,
      message: copy.message,
      buttons: [
        { text: 'Cancel', role: 'cancel' },
        { text: copy.confirmText, role: 'confirm' },
      ],
    });
    await alert.present();
    const { role } = await alert.onDidDismiss();
    if (role !== 'confirm') return;

    this.busy.set(true);
    let summary: string | null = null;
    try {
      // Concurrent, but partial failure is tolerated: one rejected archive must not abandon the rest.
      const results = await Promise.allSettled(
        ids.map((id) => firstValueFrom(this.api.archiveSession(id))),
      );
      const failed = results.filter((r) => r.status === 'rejected').length;
      summary = clearResultMessage(ids.length, failed);
      // Correct the list LOCALLY before the read, because the read may legitimately be stale: the
      // worker caches `list_sessions` for 5s per isolate with no cross-isolate invalidation, and on a
      // phone the read goes through the worker (`mutate` always does; `queryPreferLocalHost` only
      // prefers the host on localhost). Without this the user taps Clear and sees the same rows —
      // "the button did nothing". Only FULFILLED ids are dropped; a rejected row must stay visible
      // because it still needs clearing. And it is a FILTER BY ID, never a patch from the mutation
      // result — patching is what loses `attachedTmux` and silently drops the row.
      const cleared = fulfilledClearIds(
        ids,
        results.map((r) => r.status),
      );
      this.sessions.update((rows) => sessionsWithoutIds(rows, cleared));
      // …and suppress them, so the `refresh()` below cannot undo the drop with a cached read.
      this.suppressedIds.update((entries) => addSuppressed(entries, cleared, Date.now()));
    } catch (err) {
      summary = this.normalizeError(err);
    } finally {
      this.busy.set(false);
    }
    // Always refresh so the list shows reality either way, THEN report — `refresh()` clears
    // `loadError` on success, so setting the summary afterwards is what keeps it visible.
    await this.refresh();
    if (summary) this.loadError.set(summary);
  }

  /**
   * Clear one row (same archive-not-delete contract as `clearAll` — see its comment).
   *
   * DELIBERATELY ASYMMETRIC, and the asymmetry is not a bug: an ATTACHED row's ✕ acts silently,
   * a PLAIN row's ✕ confirms first. The two rows look identical in the list, so the reason is
   * written down here. Archiving an attached row leaves the external tmux alive
   * (`sessions.rs:198`) and the pane reappears under "Attachable tmux sessions" on the very next
   * refresh — one tap to undo, so a confirm would be friction for nothing. A plain shell has no
   * external tmux, so `archive_session` takes the `else` branch and really does
   * `kill_terminal_session` (`sessions.rs:221-225`): the terminal closes and there is nothing to
   * re-attach. On a phone list whose whole row is a navigation target, that is one mis-tap from an
   * unrecoverable close, so it asks. The copy is the single-row branch of `clearConfirmCopy` —
   * the same string the bulk clear uses, not a second one that can drift.
   */
  async clearOne(s: AiSession): Promise<void> {
    if (this.busy()) return;

    if (needsClearConfirm(s)) {
      const copy = clearConfirmCopy([s], this.tmux().map((t) => t.name));
      const alert = await this.alertCtrl.create({
        header: copy.header,
        message: copy.message,
        buttons: [
          { text: 'Cancel', role: 'cancel' },
          { text: copy.confirmText, role: 'confirm' },
        ],
      });
      await alert.present();
      const { role } = await alert.onDidDismiss();
      if (role !== 'confirm') return;
    }

    this.busy.set(true);
    let summary: string | null = null;
    try {
      await firstValueFrom(this.api.archiveSession(s.id));
      // Same optimistic drop + suppression as `clearAll`, same reason — and only on success.
      this.sessions.update((rows) => sessionsWithoutIds(rows, [s.id]));
      this.suppressedIds.update((entries) => addSuppressed(entries, [s.id], Date.now()));
    } catch (err) {
      summary = this.normalizeError(err);
    } finally {
      this.busy.set(false);
    }
    await this.refresh();
    if (summary) this.loadError.set(summary);
  }

  /** Open a listed shell on the existing terminal surface (navigation only — no embedded terminal). */
  async open(s: AiSession): Promise<void> {
    const route = openIntent(s.id);
    await this.router.navigate([route.path], { queryParams: route.queryParams });
  }

  /** Attach an external tmux pane via the existing create path, then open it. */
  async attach(name: string): Promise<void> {
    this.busy.set(true);
    try {
      const session = await firstValueFrom(this.api.createSession(attachTmuxInput(name)));
      await this.open(session);
    } catch (err) {
      this.loadError.set(this.normalizeError(err));
    } finally {
      this.busy.set(false);
    }
  }

  /** Open the Phase-01 `+ New` launcher popover from the header (same as the app-nav trigger). */
  async openLauncher(ev: Event): Promise<void> {
    const popover = await this.popoverCtrl.create({
      component: LauncherMenuComponent,
      event: ev,
      cssClass: 'launcher-popover',
    });
    await popover.present();
  }

  private normalizeError(err: unknown): string {
    if (err instanceof Error) return err.message;
    if (typeof err === 'string') return err;
    return 'Failed to load shells.';
  }
}
