export interface AiMessage {
  id: string;
  sessionId: string;
  role: 'user' | 'assistant' | 'system' | 'tool';
  content: string;
  toolCalls?: string;
  finishReason?: string;
  inputTokens: number;
  outputTokens: number;
  costCents: number;
  createdAt: string;
}

export interface AiMessageDelta {
  sessionId: string;
  messageId: string;
  delta: string;
  finishReason?: string;
}

export interface SendAgentMessageInput {
  sessionId: string;
  content: string;
}
