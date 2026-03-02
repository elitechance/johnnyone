import { Component, Input, Output, EventEmitter, ChangeDetectionStrategy, OnInit } from '@angular/core';
import { CommonModule } from '@angular/common';
import { FormsModule } from '@angular/forms';
import { IonicModule } from '@ionic/angular';
import { ProviderConfig } from '../../models/provider.model';

interface ProviderOption {
  value: ProviderConfig['provider'];
  label: string;
  models: ModelOption[];
}

interface ModelOption {
  value: string;
  label: string;
}

@Component({
  selector: 'johnny-provider-selector',
  standalone: true,
  imports: [CommonModule, FormsModule, IonicModule],
  templateUrl: './provider-selector.component.html',
  styleUrls: ['./provider-selector.component.scss'],
  changeDetection: ChangeDetectionStrategy.OnPush,
})
export class ProviderSelectorComponent implements OnInit {
  @Input() currentConfig: ProviderConfig | null = null;

  @Output() configChanged = new EventEmitter<ProviderConfig>();

  selectedProvider: ProviderConfig['provider'] = 'claude';
  selectedModel = '';

  readonly providers: ProviderOption[] = [
    {
      value: 'claude',
      label: 'Anthropic Claude',
      models: [
        { value: 'claude-opus-4-0', label: 'Claude Opus 4' },
        { value: 'claude-sonnet-4-0', label: 'Claude Sonnet 4' },
        { value: 'claude-3-5-haiku-latest', label: 'Claude 3.5 Haiku' },
      ],
    },
    {
      value: 'openai',
      label: 'OpenAI',
      models: [
        { value: 'gpt-4o', label: 'GPT-4o' },
        { value: 'gpt-4o-mini', label: 'GPT-4o Mini' },
        { value: 'o1-preview', label: 'o1 Preview' },
        { value: 'o1-mini', label: 'o1 Mini' },
      ],
    },
    {
      value: 'ollama',
      label: 'Ollama (Local)',
      models: [
        { value: 'llama3.1:70b', label: 'Llama 3.1 70B' },
        { value: 'llama3.1:8b', label: 'Llama 3.1 8B' },
        { value: 'codellama:34b', label: 'Code Llama 34B' },
        { value: 'mistral:7b', label: 'Mistral 7B' },
      ],
    },
  ];

  get availableModels(): ModelOption[] {
    const provider = this.providers.find((p) => p.value === this.selectedProvider);
    return provider?.models ?? [];
  }

  get currentProviderLabel(): string {
    return this.providers.find((p) => p.value === this.selectedProvider)?.label ?? '';
  }

  ngOnInit(): void {
    if (this.currentConfig) {
      this.selectedProvider = this.currentConfig.provider;
      this.selectedModel = this.currentConfig.model;
    } else {
      this.selectedProvider = 'claude';
      this.selectedModel = 'claude-opus-4-0';
    }
  }

  onProviderChange(event: CustomEvent): void {
    this.selectedProvider = event.detail.value;
    // Reset model to first available when provider changes
    const models = this.availableModels;
    this.selectedModel = models.length > 0 ? models[0].value : '';
    this.emitChange();
  }

  onModelChange(event: CustomEvent): void {
    this.selectedModel = event.detail.value;
    this.emitChange();
  }

  private emitChange(): void {
    const config: ProviderConfig = {
      id: this.currentConfig?.id ?? '',
      provider: this.selectedProvider,
      model: this.selectedModel,
      apiKeyRef: this.currentConfig?.apiKeyRef,
      baseUrl: this.currentConfig?.baseUrl,
      isDefault: this.currentConfig?.isDefault ?? true,
      settings: this.currentConfig?.settings ?? {},
    };
    this.configChanged.emit(config);
  }
}
