/**
 * LLM Provider interfaces for the JohnnyOne agent system.
 * All providers implement the LlmProvider interface for uniform access.
 */

export interface LlmMessage {
  role: 'user' | 'assistant' | 'system' | 'tool';
  content: string | null;
  toolCalls?: LlmToolCall[];
  toolCallId?: string;
  name?: string;
}

export interface LlmToolDefinition {
  name: string;
  description: string;
  parametersSchema: Record<string, unknown>;
}

export interface LlmToolCall {
  id: string;
  name: string;
  arguments: string;
}

export interface LlmStreamChunk {
  type: 'content_delta' | 'tool_call_delta' | 'message_start' | 'message_stop' | 'error';
  delta?: string;
  toolCall?: Partial<LlmToolCall>;
  finishReason?: string;
  messageId?: string;
  error?: string;
  usage?: {
    inputTokens: number;
    outputTokens: number;
  };
}

export interface LlmChatRequest {
  messages: LlmMessage[];
  tools?: LlmToolDefinition[];
  systemPrompt?: string;
  maxTokens?: number;
  temperature?: number;
  stream?: boolean;
}

export interface LlmChatResponse {
  messageId: string;
  content: string | null;
  toolCalls: LlmToolCall[];
  finishReason: string;
  usage: {
    inputTokens: number;
    outputTokens: number;
  };
}

export interface LlmProvider {
  readonly name: string;
  readonly model: string;

  /**
   * Send a chat completion request and return the full response.
   */
  chat(request: LlmChatRequest): Promise<LlmChatResponse>;

  /**
   * Send a streaming chat completion request.
   * Yields chunks as they arrive from the provider.
   */
  chatStream(request: LlmChatRequest): AsyncIterable<LlmStreamChunk>;
}

export interface ProviderConfig {
  id: string;
  provider: 'claude' | 'openai' | 'ollama';
  model: string;
  apiKeyRef: string | null;
  baseUrl: string | null;
  settings: Record<string, unknown>;
}
