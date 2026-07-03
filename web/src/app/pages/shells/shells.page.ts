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
  PopoverController,
} from '@ionic/angular/standalone';
import { addIcons } from 'ionicons';
import { addOutline, terminalOutline, gitNetworkOutline } from 'ionicons/icons';
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
  ],
  templateUrl: './shells.page.html',
  styleUrls: ['./shells.page.scss'],
})
export class ShellsPage implements OnInit {
  private readonly api = inject(JohnnyApiService);
  private readonly router = inject(Router);
  private readonly popoverCtrl = inject(PopoverController);

  protected readonly sessions = signal<AiSession[]>([]);
  protected readonly tmux = signal<TmuxSession[]>([]);
  protected readonly loadError = signal<string | null>(null);
  protected readonly busy = signal(false);
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
      this.sessions.set(sessions ?? []);
      this.tmux.set(tmux ?? []);
      this.loadError.set(null);
    } catch (err) {
      this.loadError.set(this.normalizeError(err));
    } finally {
      this.busy.set(false);
    }
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
