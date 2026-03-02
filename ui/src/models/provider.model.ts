export interface ProviderConfig {
  id: string;
  provider: 'claude' | 'openai' | 'ollama';
  model: string;
  apiKeyRef?: string;
  baseUrl?: string;
  isDefault: boolean;
  settings: Record<string, unknown>;
}

export interface LlmModel {
  id: string;
  name: string;
  provider: string;
  contextWindow: number;
  maxOutput: number;
}

export interface AiUsageSummary {
  totalInputTokens: number;
  totalOutputTokens: number;
  totalCost: number;
  sessionCount: number;
  messageCount: number;
}
