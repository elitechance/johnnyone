import { Component, OnInit, inject } from '@angular/core';
import { CommonModule } from '@angular/common';
import { FormsModule } from '@angular/forms';
import {
  IonHeader,
  IonToolbar,
  IonTitle,
  IonContent,
  IonButtons,
  IonBackButton,
  IonButton,
  IonList,
  IonItem,
  IonLabel,
  IonInput,
  IonItemDivider,
  IonCard,
  IonCardHeader,
  IonCardTitle,
  IonCardContent,
  IonToast,
  IonBadge,
  IonNote,
} from '@ionic/angular/standalone';
import { Router } from '@angular/router';
import {
  DetectedCliTool,
  JohnnyApiService,
  ProviderConfig,
} from '@johnnyone/ui';
import { firstValueFrom } from 'rxjs';

interface EdgeConfig {
  workerUrl: string;
  tenantId: string;
  userId: string;
}

interface ProviderRuntimeRow {
  provider: string;
  config: ProviderConfig | null;
  tool: DetectedCliTool | null;
}

@Component({
  selector: 'app-settings',
  standalone: true,
  imports: [
    CommonModule,
    FormsModule,
    IonHeader,
    IonToolbar,
    IonTitle,
    IonContent,
    IonButtons,
    IonBackButton,
    IonButton,
    IonList,
    IonItem,
    IonLabel,
    IonInput,
    IonItemDivider,
    IonCard,
    IonCardHeader,
    IonCardTitle,
    IonCardContent,
    IonToast,
    IonBadge,
    IonNote,
  ],
  templateUrl: './settings.page.html',
  styleUrls: ['./settings.page.scss'],
})
export class SettingsPage implements OnInit {
  private static readonly DEFAULT_WORKER_URL = 'http://127.0.0.1:7714';
  private static readonly DEFAULT_TENANT_ID = '00000000-0000-0000-0000-000000000001';
  private static readonly DEFAULT_USER_ID = '00000000-0000-0000-0000-000000000002';

  private readonly api = inject(JohnnyApiService);
  private readonly router = inject(Router);

  edgeConfig: EdgeConfig = {
    workerUrl: SettingsPage.DEFAULT_WORKER_URL,
    tenantId: SettingsPage.DEFAULT_TENANT_ID,
    userId: SettingsPage.DEFAULT_USER_ID,
  };

  providerConfigs: ProviderConfig[] = [];
  detectedTools: DetectedCliTool[] = [];
  lastWorkingDirectory = '';
  loadingHostState = false;
  showSaveToast = false;
  saveToastMessage = '';
  saveToastColor: 'success' | 'warning' | 'danger' = 'success';

  get providerRuntimeRows(): ProviderRuntimeRow[] {
    const providers = new Set<string>();

    for (const config of this.providerConfigs) {
      providers.add(config.provider);
    }

    for (const tool of this.detectedTools) {
      providers.add(tool.provider);
    }

    return Array.from(providers)
      .sort((a, b) => a.localeCompare(b))
      .map((provider) => ({
        provider,
        config: this.providerConfigs.find((item) => item.provider === provider) ?? null,
        tool: this.detectedTools.find((item) => item.provider === provider) ?? null,
      }));
  }

  ngOnInit(): void {
    this.loadEdgeSettings();
    void this.loadHostState();
  }

  private loadEdgeSettings(): void {
    try {
      this.edgeConfig = {
        workerUrl: localStorage.getItem('johnnyone_worker_url')?.trim() || SettingsPage.DEFAULT_WORKER_URL,
        tenantId: localStorage.getItem('johnnyone_tenant_id')?.trim() || SettingsPage.DEFAULT_TENANT_ID,
        userId: localStorage.getItem('johnnyone_user_id')?.trim() || SettingsPage.DEFAULT_USER_ID,
      };
    } catch (err) {
      console.error('Failed to load edge settings:', err);
    }
  }

  async loadHostState(): Promise<void> {
    this.loadingHostState = true;

    try {
      const [providerConfigs, detectedTools, lastWorkingDirectory] = await Promise.all([
        firstValueFrom(this.api.listProviderConfigs()),
        firstValueFrom(this.api.detectCliTools()),
        firstValueFrom(this.api.getSetting('last_working_directory')).catch(() => ''),
      ]);

      this.providerConfigs = providerConfigs;
      this.detectedTools = detectedTools;
      this.lastWorkingDirectory = lastWorkingDirectory;
    } catch (err) {
      console.error('Failed to load host state:', err);
      this.showToast('Failed to load host settings', 'danger');
    } finally {
      this.loadingHostState = false;
    }
  }

  saveConnectionSettings(): void {
    try {
      localStorage.setItem('johnnyone_worker_url', this.edgeConfig.workerUrl.trim() || SettingsPage.DEFAULT_WORKER_URL);
      localStorage.setItem('johnnyone_tenant_id', this.edgeConfig.tenantId.trim() || SettingsPage.DEFAULT_TENANT_ID);
      localStorage.setItem('johnnyone_user_id', this.edgeConfig.userId.trim() || SettingsPage.DEFAULT_USER_ID);
      this.showToast('Edge settings saved. Reload desktop to apply.', 'success');
    } catch (err) {
      console.error('Failed to save edge settings:', err);
      this.showToast('Failed to save edge settings', 'danger');
    }
  }

  async saveHostSettings(): Promise<void> {
    try {
      await firstValueFrom(
        this.api.setSetting('last_working_directory', this.lastWorkingDirectory.trim())
      );
      this.showToast('Host settings saved', 'success');
    } catch (err) {
      console.error('Failed to save host settings:', err);
      this.showToast('Failed to save host settings', 'danger');
    }
  }

  reloadDesktop(): void {
    window.location.reload();
  }

  providerLabel(provider: string): string {
    switch (provider) {
      case 'claude_code':
        return 'Claude Code';
      case 'codex':
        return 'Codex';
      case 'cline':
        return 'Cline';
      case 'ollama':
        return 'Ollama';
      default:
        return provider;
    }
  }

  providerStatusColor(row: ProviderRuntimeRow): 'success' | 'warning' | 'medium' {
    if (row.tool?.found && row.config?.isAvailable) {
      return 'success';
    }

    if (row.tool?.found || row.config) {
      return 'warning';
    }

    return 'medium';
  }

  providerStatusLabel(row: ProviderRuntimeRow): string {
    if (row.tool?.found && row.config?.isAvailable) {
      return 'ready';
    }

    if (row.tool?.found) {
      return 'tool found';
    }

    if (row.config) {
      return 'configured';
    }

    return 'not configured';
  }

  maskedApiKeyStatus(config: ProviderConfig | null): string {
    if (!config?.apiKey) {
      return 'Not configured';
    }

    return `Configured (${Math.max(config.apiKey.length - 4, 0)} hidden chars)`;
  }

  showToast(message: string, color: 'success' | 'warning' | 'danger'): void {
    this.saveToastMessage = message;
    this.saveToastColor = color;
    this.showSaveToast = true;
  }

  goBack(): void {
    this.router.navigate(['/chat']);
  }
}
