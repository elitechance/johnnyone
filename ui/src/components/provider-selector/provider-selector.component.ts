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

  selectedProvider: ProviderConfig['provider'] = 'claude_code';
  selectedModel = '';

  readonly providers: ProviderOption[] = [
    {
      value: 'claude_code',
      label: 'Claude Code',
      models: [
        { value: 'sonnet', label: 'Claude Sonnet' },
        { value: 'opus', label: 'Claude Opus' },
      ],
    },
    {
      value: 'codex',
      label: 'Codex',
      models: [
        { value: 'gpt-4o', label: 'GPT-4o' },
        { value: 'gpt-4o-mini', label: 'GPT-4o Mini' },
        { value: 'o1-preview', label: 'o1 Preview' },
        { value: 'o1-mini', label: 'o1 Mini' },
      ],
    },
    {
      value: 'cline',
      label: 'Cline',
      models: [
        { value: 'anthropic/claude-sonnet-4', label: 'Claude Sonnet 4' },
        { value: 'openai/gpt-4o', label: 'GPT-4o' },
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
      const preferredModel = this.currentConfig.model ?? this.currentConfig.defaultModel;
      this.selectedModel = preferredModel ?? '';
    } else {
      this.selectedProvider = 'claude_code';
    }

    if (!this.selectedModel) {
      this.selectedModel = this.availableModels[0]?.value ?? '';
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
      defaultModel: this.selectedModel,
      apiKeyRef: this.currentConfig?.apiKeyRef,
      baseUrl: this.currentConfig?.baseUrl,
      isDefault: this.currentConfig?.isDefault ?? true,
      settings: this.currentConfig?.settings ?? {},
    };
    this.configChanged.emit(config);
  }
}
