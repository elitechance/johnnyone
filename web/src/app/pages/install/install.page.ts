import { Component, signal } from '@angular/core';
import {
  IonButton,
  IonCard,
  IonCardContent,
  IonCardHeader,
  IonCardSubtitle,
  IonCardTitle,
  IonContent,
  IonHeader,
  IonIcon,
  IonItem,
  IonLabel,
  IonList,
  IonTitle,
  IonToolbar,
} from '@ionic/angular/standalone';
import { addIcons } from 'ionicons';
import { downloadOutline, openOutline, terminalOutline } from 'ionicons/icons';

@Component({
  selector: 'app-install-page',
  standalone: true,
  imports: [
    IonHeader,
    IonToolbar,
    IonTitle,
    IonContent,
    IonCard,
    IonCardHeader,
    IonCardTitle,
    IonCardSubtitle,
    IonCardContent,
    IonList,
    IonItem,
    IonLabel,
    IonButton,
    IonIcon,
  ],
  template: `
    <ion-header>
      <ion-toolbar>
        <ion-title>Install your JohnnyOne host</ion-title>
      </ion-toolbar>
    </ion-header>
    <ion-content class="ion-padding">
      <ion-card class="install-card">
        <ion-card-header>
          <ion-card-title>One last step</ion-card-title>
          <ion-card-subtitle>
            JohnnyOne runs your agent on your own machine. Install the host app to get started.
          </ion-card-subtitle>
        </ion-card-header>
        <ion-card-content>
          <p>
            The host is a small background process that runs Claude Code, Codex, Cline, or Ollama
            inside tmux on your machine. The cloud (this site) routes your requests to it.
          </p>

          <ion-list lines="full">
            <ion-item button (click)="download('mac')" [disabled]="downloading()">
              <ion-icon name="download-outline" slot="start"></ion-icon>
              <ion-label>
                <h2>macOS (Apple Silicon / Intel)</h2>
                <p>Unsigned binary · ~30 MB · v0.1.0</p>
              </ion-label>
            </ion-item>
            <ion-item lines="none">
              <ion-icon name="terminal-outline" slot="start"></ion-icon>
              <ion-label>
                <h2>Windows / Linux</h2>
                <p>Coming in v1.1 — build from source for now</p>
              </ion-label>
            </ion-item>
          </ion-list>

          <h3>Run it</h3>
          <ol>
            <li>Open Terminal in the folder where you downloaded the binary.</li>
            <li>Make it executable: <code>chmod +x johnnyone-host</code></li>
            <li>On first run macOS will block it. Run <code>xattr -d com.apple.quarantine johnnyone-host</code> (or right-click → Open in Finder).</li>
            <li>Run it: <code>./johnnyone-host</code></li>
            <li>The control panel opens. Sign in with the same account you used here.</li>
          </ol>

          <ion-button expand="block" fill="outline" (click)="openRunbook()">
            <ion-icon name="open-outline" slot="start"></ion-icon>
            Detailed install guide
          </ion-button>
        </ion-card-content>
      </ion-card>
    </ion-content>
  `,
  styles: [
    `
      .install-card {
        max-width: 640px;
        margin: 16px auto;
      }
      ol {
        padding-left: 1.25rem;
        line-height: 1.7;
      }
      code {
        font-family: ui-monospace, SFMono-Regular, Menlo, monospace;
        background: var(--ion-color-step-100);
        padding: 0.1em 0.4em;
        border-radius: 4px;
      }
    `,
  ],
})
export class InstallPage {
  readonly downloading = signal(false);

  // TODO Phase 4 task 06: replace with the real CF R2 download URL.
  readonly downloadUrl = signal<Record<'mac', string>>({
    mac: 'https://johnnyone-binaries.example/johnnyone-host-mac-arm64',
  });

  constructor() {
    addIcons({ downloadOutline, openOutline, terminalOutline });
  }

  download(target: 'mac'): void {
    this.downloading.set(true);
    window.location.href = this.downloadUrl()[target];
    setTimeout(() => this.downloading.set(false), 2000);
  }

  openRunbook(): void {
    window.open(
      'https://github.com/elitechance/johnnyone/blob/main/personal/docs/johnnyone/runbooks/installing.md',
      '_blank',
      'noopener,noreferrer',
    );
  }
}
