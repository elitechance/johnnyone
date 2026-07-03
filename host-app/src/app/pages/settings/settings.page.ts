import { Component, inject, signal } from '@angular/core';
import { FormsModule } from '@angular/forms';
import { Router } from '@angular/router';
import {
  IonBackButton,
  IonButton,
  IonButtons,
  IonCard,
  IonCardContent,
  IonCardHeader,
  IonCardSubtitle,
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
import {
  DEFAULT_HOST_SETTINGS,
  HostSettings,
  HostSettingsService,
} from '../../services/host-settings.service';
import { HostRelayService } from '../../services/host-relay.service';
import { trimStorePaths } from './host-settings-logic';

@Component({
  selector: 'host-settings-page',
  standalone: true,
  imports: [
    FormsModule,
    IonHeader,
    IonToolbar,
    IonButtons,
    IonBackButton,
    IonTitle,
    IonContent,
    IonCard,
    IonCardHeader,
    IonCardTitle,
    IonCardSubtitle,
    IonCardContent,
    IonList,
    IonItem,
    IonInput,
    IonButton,
    IonText,
  ],
  templateUrl: './settings.page.html',
  styleUrl: './settings.page.scss',
})
export class SettingsPage {
  private readonly settingsService = inject(HostSettingsService);
  private readonly relayService = inject(HostRelayService);
  private readonly router = inject(Router);

  readonly loading = signal(true);
  readonly saving = signal(false);
  readonly error = signal('');
  readonly notice = signal('');

  workerUrl = DEFAULT_HOST_SETTINGS.workerUrl;
  tenantId = DEFAULT_HOST_SETTINGS.tenantId;
  userId = DEFAULT_HOST_SETTINGS.userId;
  plannerMethodologyPath = DEFAULT_HOST_SETTINGS.plannerMethodologyPath;
  plannerConventionsPath = DEFAULT_HOST_SETTINGS.plannerConventionsPath;
  webClientUrl = DEFAULT_HOST_SETTINGS.webClientUrl;
  discordWebhookUrl = DEFAULT_HOST_SETTINGS.discordWebhookUrl;
  initiativesDir = DEFAULT_HOST_SETTINGS.initiativesDir;
  filesRoot = DEFAULT_HOST_SETTINGS.filesRoot;

  constructor() {
    void this.load();
  }

  async load(): Promise<void> {
    this.loading.set(true);
    this.error.set('');
    try {
      const settings = await this.settingsService.load();
      this.apply(settings);
    } catch (err) {
      this.error.set(err instanceof Error ? err.message : String(err));
    } finally {
      this.loading.set(false);
    }
  }

  async save(): Promise<void> {
    if (this.saving()) return;
    this.saving.set(true);
    this.error.set('');
    this.notice.set('');
    try {
      const settings = this.snapshot();
      await this.settingsService.save(settings);
      this.notice.set('Settings saved.');
      if (settings.userId.trim()) {
        await this.relayService.connect();
        this.notice.set('Settings saved and relay reconnect requested.');
      }
      await this.router.navigateByUrl('/status');
    } catch (err) {
      this.error.set(err instanceof Error ? err.message : String(err));
    } finally {
      this.saving.set(false);
    }
  }

  private apply(settings: HostSettings): void {
    this.workerUrl = settings.workerUrl;
    this.tenantId = settings.tenantId;
    this.userId = settings.userId;
    this.plannerMethodologyPath = settings.plannerMethodologyPath;
    this.plannerConventionsPath = settings.plannerConventionsPath;
    this.webClientUrl = settings.webClientUrl;
    this.discordWebhookUrl = settings.discordWebhookUrl;
    this.initiativesDir = settings.initiativesDir;
    this.filesRoot = settings.filesRoot;
  }

  private snapshot(): HostSettings {
    const storePaths = trimStorePaths({
      initiativesDir: this.initiativesDir,
      filesRoot: this.filesRoot,
    });
    return {
      workerUrl: this.workerUrl.trim(),
      tenantId: this.tenantId.trim(),
      userId: this.userId.trim(),
      plannerMethodologyPath: this.plannerMethodologyPath.trim(),
      plannerConventionsPath: this.plannerConventionsPath.trim(),
      webClientUrl: this.webClientUrl.trim(),
      discordWebhookUrl: this.discordWebhookUrl.trim(),
      initiativesDir: storePaths.initiativesDir,
      filesRoot: storePaths.filesRoot,
    };
  }
}