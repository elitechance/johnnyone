import { Component, computed, inject, signal } from '@angular/core';
import { Router } from '@angular/router';
import { IonContent, IonIcon } from '@ionic/angular/standalone';
import { addIcons } from 'ionicons';
import { settingsOutline } from 'ionicons/icons';
import { HostAuthService } from '../../services/host-auth.service';
import { DetectedTool, HostStatusService } from '../../services/host-status.service';
import {
  DEFAULT_HOST_SETTINGS,
  HostSettings,
  HostSettingsService,
} from '../../services/host-settings.service';
import {
  overallHealth,
  providerRows,
  statusRows,
  storePathRows,
} from './host-dashboard-logic';

@Component({
  selector: 'host-status-page',
  standalone: true,
  imports: [IonContent, IonIcon],
  templateUrl: './status.page.html',
  styleUrl: './status.page.scss',
})
export class StatusPage {
  private readonly auth = inject(HostAuthService);
  private readonly settingsService = inject(HostSettingsService);
  private readonly router = inject(Router);
  protected readonly status = inject(HostStatusService);

  readonly user = this.auth.currentUser;
  readonly tools = signal<DetectedTool[]>([]);
  readonly settings = signal<HostSettings>(DEFAULT_HOST_SETTINGS);

  // Every row is shaped by the pure, spec-pinned host-dashboard-logic (D9).
  readonly statusRows = computed(() =>
    statusRows({
      hostUp: this.status.hostUp(),
      workerUp: this.status.workerUp(),
      relayConnected: this.status.relayConnected(),
      workerUrl: this.status.workerUrl(),
      relayEmail: this.user()?.email ?? 'Not signed in',
      relayHeartbeat: this.status.relayHeartbeat(),
      activeSessions: this.status.activeSessions(),
      nowMs: Date.now(),
    }),
  );
  readonly overallHealth = computed(() =>
    overallHealth({
      hostUp: this.status.hostUp(),
      workerUp: this.status.workerUp(),
      relayConnected: this.status.relayConnected(),
    }),
  );
  readonly providerRows = computed(() => providerRows(this.tools()));
  readonly storePathRows = computed(() => storePathRows(this.settings()));

  /** Bare host of the web client URL for the Open-workspace button subtitle. */
  readonly webHost = computed(() => {
    const url = this.status.webClientUrl();
    try {
      return new URL(url).host;
    } catch {
      return url;
    }
  });

  constructor() {
    addIcons({ settingsOutline });
    void this.init();
  }

  private async init(): Promise<void> {
    await this.status.refresh();
    const [tools, settings] = await Promise.all([
      this.status.detectCliTools(),
      this.settingsService.load().catch(() => DEFAULT_HOST_SETTINGS),
    ]);
    this.tools.set(tools);
    this.settings.set(settings);
  }

  openWeb(): void {
    window.open(this.status.webClientUrl(), '_blank', 'noopener,noreferrer');
  }

  openSettings(): void {
    void this.router.navigateByUrl('/settings');
  }

  /** The Store & paths rows' "Edit" affordance deep-links to /settings (D3). */
  editStorePaths(): void {
    this.openSettings();
  }

  signOut(): void {
    this.auth.logout();
    void this.router.navigateByUrl('/login');
  }
}
