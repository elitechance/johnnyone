import { Component, OnInit, inject, signal } from '@angular/core';
import { FormsModule } from '@angular/forms';
import {
  IonButton,
  IonCard,
  IonCardContent,
  IonCardHeader,
  IonCardSubtitle,
  IonCardTitle,
  IonContent,
  IonHeader,
  IonInput,
  IonItem,
  IonLabel,
  IonList,
  IonText,
  IonTitle,
  IonToolbar,
} from '@ionic/angular/standalone';
import { firstValueFrom } from 'rxjs';
import { GRAPHQL_API_URL, JohnnyApiService } from '@johnnyone/ui';
import { AuthService } from '../../services/auth.service';

const DISCORD_WEBHOOK_SETTING = 'discord_webhook_url';

@Component({
  selector: 'app-settings-page',
  standalone: true,
  imports: [
    FormsModule,
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
    IonInput,
    IonButton,
    IonText,
  ],
  template: `
    <ion-header>
      <ion-toolbar>
        <ion-title>Settings</ion-title>
      </ion-toolbar>
    </ion-header>
    <ion-content class="ion-padding">
      <ion-card>
        <ion-card-header>
          <ion-card-title>Connection</ion-card-title>
          <ion-card-subtitle>Where this client talks to the worker</ion-card-subtitle>
        </ion-card-header>
        <ion-card-content>
          <ion-list lines="none">
            <ion-item>
              <ion-label>
                <h3>Worker GraphQL URL</h3>
                <p>{{ apiUrl }}</p>
              </ion-label>
            </ion-item>
            <ion-item>
              <ion-label>
                <h3>Tenant ID</h3>
                <p>{{ tenantId() || '—' }}</p>
              </ion-label>
            </ion-item>
          </ion-list>
          <ion-text color="medium">
            <p>
              <em>
                Worker URL is fixed at build time per Master plan §Decisions #3. Per-user host
                registration UI ships in Phase 3 (installer control panel).
              </em>
            </p>
          </ion-text>
        </ion-card-content>
      </ion-card>

      <ion-card>
        <ion-card-header>
          <ion-card-title>Alerts</ion-card-title>
          <ion-card-subtitle>Ping a Discord channel when a run needs your attention</ion-card-subtitle>
        </ion-card-header>
        <ion-card-content>
          <ion-list lines="none">
            <ion-item>
              <ion-input
                label="Discord webhook URL"
                labelPlacement="stacked"
                type="url"
                autocomplete="off"
                placeholder="https://discord.com/api/webhooks/…"
                [(ngModel)]="discordWebhook"
              ></ion-input>
            </ion-item>
          </ion-list>
          <ion-button expand="block" (click)="saveDiscordWebhook()" [disabled]="savingDiscord()">
            {{ savingDiscord() ? 'Saving…' : 'Save webhook' }}
          </ion-button>
          @if (discordSaved()) {
            <ion-text color="success"><p>Saved.</p></ion-text>
          }
          <ion-text color="medium">
            <p>
              <em>
                Fires only on attention (a run is blocked / needs a human decision) — never on
                routine review changes. Leave blank to disable. Stored on the host.
              </em>
            </p>
          </ion-text>
        </ion-card-content>
      </ion-card>

      <ion-card>
        <ion-card-header>
          <ion-card-title>Account</ion-card-title>
        </ion-card-header>
        <ion-card-content>
          <ion-button expand="block" color="medium" (click)="logout()">Sign out</ion-button>
        </ion-card-content>
      </ion-card>

      <ion-card>
        <ion-card-header>
          <ion-card-title>Provider configs</ion-card-title>
          <ion-card-subtitle>Coming after Phase 2</ion-card-subtitle>
        </ion-card-header>
        <ion-card-content>
          <p>
            Provider CRUD (Claude Code, Codex, Cline, Ollama) routes through the host via
            relay-RPC — full wiring lands once Phase 2 task 08 migrates
            <code>listProviderConfigs</code>, <code>upsertProviderConfig</code>, and
            <code>deleteProviderConfig</code> off the legacy forward path.
          </p>
        </ion-card-content>
      </ion-card>
    </ion-content>
  `,
})
export class SettingsPage implements OnInit {
  readonly apiUrl = inject(GRAPHQL_API_URL);
  private readonly auth = inject(AuthService);
  private readonly api = inject(JohnnyApiService);

  tenantId = (() => {
    const id = this.auth.getTenantId();
    return () => id;
  })();

  discordWebhook = '';
  savingDiscord = signal(false);
  discordSaved = signal(false);

  ngOnInit(): void {
    firstValueFrom(this.api.getSetting(DISCORD_WEBHOOK_SETTING))
      .then((value) => {
        this.discordWebhook = value ?? '';
      })
      .catch(() => {
        /* setting not set yet / host unreachable — leave blank */
      });
  }

  async saveDiscordWebhook(): Promise<void> {
    this.savingDiscord.set(true);
    this.discordSaved.set(false);
    try {
      await firstValueFrom(
        this.api.setSetting(DISCORD_WEBHOOK_SETTING, this.discordWebhook.trim()),
      );
      this.discordSaved.set(true);
    } finally {
      this.savingDiscord.set(false);
    }
  }

  logout(): void {
    this.auth.logout();
  }
}
