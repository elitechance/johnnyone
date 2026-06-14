import { Component, inject, signal } from '@angular/core';
import { FormsModule } from '@angular/forms';
import { Router } from '@angular/router';
import {
  IonButton,
  IonCard,
  IonCardContent,
  IonCardHeader,
  IonCardTitle,
  IonContent,
  IonHeader,
  IonInput,
  IonItem,
  IonList,
  IonText,
  IonTitle,
  IonToolbar,
} from '@ionic/angular/standalone';
import { HostAuthService } from '../../services/host-auth.service';
import { HostSettingsService } from '../../services/host-settings.service';

@Component({
  selector: 'host-login-page',
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
    IonCardContent,
    IonList,
    IonItem,
    IonInput,
    IonButton,
    IonText,
  ],
  templateUrl: './login.page.html',
  styleUrl: './login.page.scss',
})
export class LoginPage {
  private readonly auth = inject(HostAuthService);
  private readonly settings = inject(HostSettingsService);
  private readonly router = inject(Router);

  email = signal('');
  password = signal('');
  tenantId = signal('00000000-0000-0000-0000-000000000001');
  workerUrl = signal('');
  loading = signal(false);
  error = signal('');

  constructor() {
    void this.loadDefaults();
  }

  private async loadDefaults(): Promise<void> {
    try {
      const hostSettings = await this.settings.load();
      this.tenantId.set(hostSettings.tenantId);
      this.workerUrl.set(hostSettings.workerUrl);
    } catch {
      // Host GraphQL may still be starting; login will surface errors if needed.
    }
  }

  async login(): Promise<void> {
    if (this.loading()) return;
    this.error.set('');
    this.loading.set(true);
    try {
      await this.auth.login(
        this.email().trim(),
        this.password(),
        this.tenantId().trim(),
        this.workerUrl().trim(),
      );
      await this.router.navigateByUrl('/status');
    } catch (err) {
      this.error.set(err instanceof Error ? err.message : String(err));
    } finally {
      this.loading.set(false);
    }
  }
}