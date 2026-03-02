export interface AiMessage {
  id: string;
  sessionId: string;
  role: 'user' | 'assistant' | 'system' | 'tool';
  content: string;
  toolCalls: ToolCallRef[];
  toolCallId?: string;
  sourceChannel?: string;
  finishReason?: string;
  inputTokens?: number;
  outputTokens?: number;
  createdAt: string;
}

export interface ToolCallRef {
  id: string;
  name: string;
  input: Record<string, unknown>;
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
