export interface AiSession {
  id: string;
  title: string;
  provider: string;
  model: string;
  workingDirectory?: string;
  status: 'active' | 'archived' | 'completed';
  totalInputTokens: number;
  totalOutputTokens: number;
  totalCostCents: number;
  createdAt: string;
  updatedAt: string;
}

export interface CreateAiSessionInput {
  title?: string;
  provider?: string;
  model?: string;
  systemPrompt?: string;
}
