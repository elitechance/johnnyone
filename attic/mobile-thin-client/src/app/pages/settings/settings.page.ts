import { Component, inject, signal, OnInit } from '@angular/core';
import { CommonModule } from '@angular/common';
import { FormsModule } from '@angular/forms';
import {
  IonHeader,
  IonToolbar,
  IonTitle,
  IonContent,
  IonList,
  IonItem,
  IonLabel,
  IonInput,
  IonSelect,
  IonSelectOption,
  IonToggle,
  IonButton,
  IonNote,
  IonItemGroup,
  IonItemDivider,
} from '@ionic/angular/standalone';

interface ProviderConfig {
  provider: string;
  apiKey: string;
  model: string;
  baseUrl: string;
}

interface ChannelBinding {
  name: string;
  enabled: boolean;
  endpoint: string;
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
    IonList,
    IonItem,
    IonLabel,
    IonInput,
    IonSelect,
    IonSelectOption,
    IonToggle,
    IonButton,
    IonNote,
    IonItemGroup,
    IonItemDivider,
  ],
  templateUrl: './settings.page.html',
  styleUrls: ['./settings.page.scss'],
})
export class SettingsPage implements OnInit {
  readonly providers = signal<string[]>([
    'openai',
    'anthropic',
    'ollama',
    'google',
  ]);

  readonly models = signal<Record<string, string[]>>({
    openai: ['gpt-4o', 'gpt-4o-mini', 'gpt-4-turbo', 'o1', 'o1-mini'],
    anthropic: ['claude-opus-4-20250514', 'claude-sonnet-4-20250514', 'claude-3-5-haiku-20241022'],
    ollama: ['llama3.3', 'codellama', 'mistral', 'deepseek-coder'],
    google: ['gemini-2.0-flash', 'gemini-2.0-pro', 'gemini-1.5-pro'],
  });

  readonly config = signal<ProviderConfig>({
    provider: 'anthropic',
    apiKey: '',
    model: 'claude-sonnet-4-20250514',
    baseUrl: '',
  });

  readonly channels = signal<ChannelBinding[]>([
    { name: 'WebSocket Relay', enabled: true, endpoint: 'ws://localhost:3000/relay' },
    { name: 'Push Notifications', enabled: false, endpoint: '' },
  ]);

  readonly availableModels = signal<string[]>([]);
  readonly isSaving = signal<boolean>(false);

  ngOnInit(): void {
    this.loadSettings();
    this.updateAvailableModels();
  }

  onProviderChange(provider: string): void {
    const current = this.config();
    const providerModels = this.models()[provider] ?? [];
    this.config.set({
      ...current,
      provider,
      model: providerModels[0] ?? '',
    });
    this.updateAvailableModels();
  }

  onModelChange(model: string): void {
    this.config.update((c) => ({ ...c, model }));
  }

  onApiKeyChange(apiKey: string): void {
    this.config.update((c) => ({ ...c, apiKey }));
  }

  onBaseUrlChange(baseUrl: string): void {
    this.config.update((c) => ({ ...c, baseUrl }));
  }

  onChannelToggle(index: number, enabled: boolean): void {
    this.channels.update((channels) =>
      channels.map((ch, i) => (i === index ? { ...ch, enabled } : ch))
    );
  }

  onChannelEndpointChange(index: number, endpoint: string): void {
    this.channels.update((channels) =>
      channels.map((ch, i) => (i === index ? { ...ch, endpoint } : ch))
    );
  }

  async saveSettings(): Promise<void> {
    this.isSaving.set(true);

    try {
      const settings = {
        provider: this.config(),
        channels: this.channels(),
      };
      localStorage.setItem('jno-settings', JSON.stringify(settings));
      console.log('Settings saved:', settings);
    } catch (error) {
      console.error('Failed to save settings:', error);
    } finally {
      this.isSaving.set(false);
    }
  }

  private loadSettings(): void {
    try {
      const raw = localStorage.getItem('jno-settings');
      if (raw) {
        const settings = JSON.parse(raw);
        if (settings.provider) {
          this.config.set(settings.provider);
        }
        if (settings.channels) {
          this.channels.set(settings.channels);
        }
      }
    } catch (error) {
      console.error('Failed to load settings:', error);
    }
  }

  private updateAvailableModels(): void {
    const provider = this.config().provider;
    this.availableModels.set(this.models()[provider] ?? []);
  }
}
