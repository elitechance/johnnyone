export interface ProviderConfig {
  id: string;
  provider: string;
  model?: string;
  cliPath?: string;
  apiKey?: string;
  apiKeyRef?: string;
  baseUrl?: string;
  defaultModel?: string;
  isDefault?: boolean;
  isAvailable?: boolean;
  settings: Record<string, unknown> | string;
  updatedAt?: string;
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
