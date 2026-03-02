export interface AiSession {
  id: string;
  userId: string;
  title: string;
  provider: string;
  model: string;
  status: 'active' | 'archived' | 'completed';
  systemPrompt?: string;
  totalInputTokens: number;
  totalOutputTokens: number;
  totalCost: number;
  createdAt: string;
  updatedAt: string;
}

export interface CreateAiSessionInput {
  title?: string;
  provider?: string;
  model?: string;
  systemPrompt?: string;
}
