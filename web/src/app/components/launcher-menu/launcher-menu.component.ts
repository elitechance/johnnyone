import { CommonModule } from '@angular/common';
import { Component, inject } from '@angular/core';
import { Router } from '@angular/router';
import {
  IonList,
  IonItem,
  IonIcon,
  IonLabel,
  AlertController,
  PopoverController,
} from '@ionic/angular/standalone';
import { addIcons } from 'ionicons';
import { createOutline, terminalOutline, gitNetworkOutline, folderOutline } from 'ionicons/icons';
import { JohnnyApiService } from '@johnnyone/ui';
import { firstValueFrom } from 'rxjs';
import {
  LAUNCHER_ENTRIES,
  launcherIntent,
  terminalRoute,
  shellCreateInput,
  attachTmuxInput,
  type LauncherEntry,
} from './launcher-logic';

/**
 * The §06 `+ New` launcher popover (overhaul P6). Renders `LAUNCHER_ENTRIES` and wires each row to the
 * existing P2/P3/P4/P5 surfaces — no new spawn/transport (D1/D3):
 *   New initiative        → navigate /briefing/new (P4)
 *   Raw shell             → createSession({provider:'shell'}) → open /terminal?sessionId= (P3)
 *   Attach to tmux        → listTmuxSessions() picker → createSession({tmuxSessionName}) → open
 *   Open file manager     → navigate /files (P5)
 *
 * The component is THIN wiring — every decision (which entries, what each does, the create inputs, the
 * terminal nav shape) comes from the pure `launcher-logic.ts`. Presented by callers through
 * `PopoverController` (see `app.component.ts openLauncher`).
 */
@Component({
  selector: 'app-launcher-menu',
  standalone: true,
  imports: [CommonModule, IonList, IonItem, IonIcon, IonLabel],
  templateUrl: './launcher-menu.component.html',
  styleUrl: './launcher-menu.component.scss',
})
export class LauncherMenuComponent {
  private readonly api = inject(JohnnyApiService);
  private readonly router = inject(Router);
  private readonly popoverCtrl = inject(PopoverController);
  private readonly alertCtrl = inject(AlertController);

  protected readonly entries = LAUNCHER_ENTRIES;

  constructor() {
    // Self-register every glyph the four §06 rows name. This app has no global ionicons registration
    // (each surface registers its own), and unregistered named ion-icons render blank — so the popover
    // owns all four icons (`create-outline`/`git-network-outline` are otherwise registered nowhere)
    // rather than silently depending on app.component registering a subset.
    addIcons({
      'create-outline': createOutline,
      'terminal-outline': terminalOutline,
      'git-network-outline': gitNetworkOutline,
      'folder-outline': folderOutline,
    });
  }

  /** Dispatch a launcher row to its intent (decision from the pure layer; IO here). */
  async choose(entry: LauncherEntry): Promise<void> {
    const intent = launcherIntent(entry.kind);
    switch (intent.type) {
      case 'navigate':
        await this.popoverCtrl.dismiss();
        await this.router.navigateByUrl(intent.path);
        return;
      case 'createShell':
        await this.launchShell();
        return;
      case 'attachPicker':
        await this.popoverCtrl.dismiss();
        await this.openAttachPicker();
        return;
    }
  }

  /** Spawn a raw `CliProvider::Shell` via the existing create path, then open it on the terminal surface. */
  private async launchShell(): Promise<void> {
    let cwd: string | undefined;
    try {
      cwd = await firstValueFrom(this.api.getSetting('last_working_directory'));
    } catch {
      // Setting may not exist yet — spawn with the host default (no workingDirectory key).
    }
    try {
      const session = await firstValueFrom(this.api.createSession(shellCreateInput(cwd || undefined)));
      await this.popoverCtrl.dismiss();
      await this.openInTerminal(session.id);
    } catch (err) {
      await this.showError('Failed to launch shell', err);
    }
  }

  /** Present a radio picker of attachable external tmux panes, then attach-and-open the chosen one. */
  private async openAttachPicker(): Promise<void> {
    let tmux;
    try {
      tmux = await firstValueFrom(this.api.listTmuxSessions());
    } catch (err) {
      await this.showError('Failed to list tmux sessions', err);
      return;
    }

    if (!tmux || tmux.length === 0) {
      const alert = await this.alertCtrl.create({
        header: 'Attach to tmux session',
        message: 'No attachable tmux sessions.',
        buttons: ['OK'],
      });
      await alert.present();
      return;
    }

    const alert = await this.alertCtrl.create({
      header: 'Attach to tmux session',
      inputs: tmux.map((s, i) => ({
        type: 'radio' as const,
        label: `${s.name} · ${s.windows} window${s.windows === 1 ? '' : 's'}`,
        value: s.name,
        checked: i === 0,
      })),
      buttons: [
        { text: 'Cancel', role: 'cancel' },
        { text: 'Attach', role: 'confirm' },
      ],
    });
    await alert.present();
    const { role, data } = await alert.onDidDismiss();
    if (role !== 'confirm') return;
    const name = String(data?.values ?? '');
    if (!name) return;

    try {
      const session = await firstValueFrom(this.api.createSession(attachTmuxInput(name)));
      await this.openInTerminal(session.id);
    } catch (err) {
      await this.showError('Failed to attach to tmux session', err);
    }
  }

  /** Navigate to the existing terminal surface with the session deep-linked (P3 `?sessionId=`). */
  private async openInTerminal(sessionId: string): Promise<void> {
    const route = terminalRoute(sessionId);
    await this.router.navigate([route.path], { queryParams: route.queryParams });
  }

  /** Surface a create/list failure instead of swallowing it. Never logs session ids as secrets (D10). */
  private async showError(header: string, err: unknown): Promise<void> {
    console.error(header, err);
    const message = err instanceof Error ? err.message : String(err);
    const alert = await this.alertCtrl.create({ header, message, buttons: ['OK'] });
    await alert.present();
  }
}
