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
  /** True when this session attaches to an external tmux session (UI shows Detach). */
  attachedTmux?: boolean;
}

export interface CreateAiSessionInput {
  title?: string;
  provider?: string;
  model?: string;
  systemPrompt?: string;
}
