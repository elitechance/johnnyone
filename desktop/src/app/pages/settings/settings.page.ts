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
  IonSelect,
  IonSelectOption,
  IonToggle,
  IonItemDivider,
  IonCard,
  IonCardHeader,
  IonCardTitle,
  IonCardContent,
  IonToast,
} from '@ionic/angular/standalone';
import { Router } from '@angular/router';

export interface ProviderConfig {
  name: string;
  enabled: boolean;
  apiKey: string;
  baseUrl: string;
  defaultModel: string;
  availableModels: string[];
}

export interface NodeConfig {
  nodeId: string;
  nodeName: string;
  registrationUrl: string;
  autoConnect: boolean;
  heartbeatIntervalMs: number;
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
    IonSelect,
    IonSelectOption,
    IonToggle,
    IonItemDivider,
    IonCard,
    IonCardHeader,
    IonCardTitle,
    IonCardContent,
    IonToast,
  ],
  templateUrl: './settings.page.html',
  styleUrls: ['./settings.page.scss'],
})
export class SettingsPage implements OnInit {
  private readonly router = inject(Router);

  providers: ProviderConfig[] = [
    {
      name: 'Ollama',
      enabled: true,
      apiKey: '',
      baseUrl: 'http://localhost:11434',
      defaultModel: 'llama3',
      availableModels: ['llama3', 'llama3:70b', 'codellama', 'mistral', 'mixtral'],
    },
    {
      name: 'OpenAI',
      enabled: false,
      apiKey: '',
      baseUrl: 'https://api.openai.com/v1',
      defaultModel: 'gpt-4o',
      availableModels: ['gpt-4o', 'gpt-4o-mini', 'gpt-4-turbo', 'o1-preview'],
    },
    {
      name: 'Anthropic',
      enabled: false,
      apiKey: '',
      baseUrl: 'https://api.anthropic.com',
      defaultModel: 'claude-sonnet-4-20250514',
      availableModels: ['claude-sonnet-4-20250514', 'claude-opus-4-20250514', 'claude-3-5-haiku-20241022'],
    },
  ];

  nodeConfig: NodeConfig = {
    nodeId: '',
    nodeName: '',
    registrationUrl: '',
    autoConnect: true,
    heartbeatIntervalMs: 30000,
  };

  showSaveToast = false;
  saveToastMessage = '';

  ngOnInit(): void {
    this.loadSettings();
  }

  private loadSettings(): void {
    // Load settings from Tauri store / local storage
    try {
      const savedProviders = localStorage.getItem('jno_providers');
      if (savedProviders) {
        this.providers = JSON.parse(savedProviders);
      }

      const savedNode = localStorage.getItem('jno_node_config');
      if (savedNode) {
        this.nodeConfig = JSON.parse(savedNode);
      }

      if (!this.nodeConfig.nodeId) {
        this.nodeConfig.nodeId = crypto.randomUUID();
      }
      if (!this.nodeConfig.nodeName) {
        this.nodeConfig.nodeName = `node-${this.nodeConfig.nodeId.substring(0, 8)}`;
      }
    } catch (err) {
      console.error('Failed to load settings:', err);
    }
  }

  saveProviderSettings(): void {
    try {
      localStorage.setItem('jno_providers', JSON.stringify(this.providers));
      this.showToast('Provider settings saved');
    } catch (err) {
      console.error('Failed to save provider settings:', err);
      this.showToast('Failed to save settings');
    }
  }

  saveNodeSettings(): void {
    try {
      localStorage.setItem('jno_node_config', JSON.stringify(this.nodeConfig));
      this.showToast('Node settings saved');
    } catch (err) {
      console.error('Failed to save node settings:', err);
      this.showToast('Failed to save settings');
    }
  }

  async registerNode(): Promise<void> {
    try {
      // In production, POST to registration endpoint via Tauri HTTP plugin
      this.showToast(`Node ${this.nodeConfig.nodeName} registration requested`);
    } catch (err) {
      console.error('Failed to register node:', err);
      this.showToast('Node registration failed');
    }
  }

  regenerateNodeId(): void {
    this.nodeConfig.nodeId = crypto.randomUUID();
    this.nodeConfig.nodeName = `node-${this.nodeConfig.nodeId.substring(0, 8)}`;
  }

  private showToast(message: string): void {
    this.saveToastMessage = message;
    this.showSaveToast = true;
    setTimeout(() => {
      this.showSaveToast = false;
    }, 2000);
  }

  goBack(): void {
    this.router.navigate(['/chat']);
  }
}
