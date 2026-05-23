import { Component, inject } from '@angular/core';
import {
  IonButton,
  IonCard,
  IonCardContent,
  IonCardHeader,
  IonCardSubtitle,
  IonCardTitle,
  IonContent,
  IonHeader,
  IonItem,
  IonLabel,
  IonList,
  IonText,
  IonTitle,
  IonToolbar,
} from '@ionic/angular/standalone';
import { GRAPHQL_API_URL } from '@johnnyone/ui';
import { AuthService } from '../../services/auth.service';

@Component({
  selector: 'app-settings-page',
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
export class SettingsPage {
  readonly apiUrl = inject(GRAPHQL_API_URL);
  private readonly auth = inject(AuthService);

  tenantId = (() => {
    const id = this.auth.getTenantId();
    return () => id;
  })();

  logout(): void {
    this.auth.logout();
  }
}
