import type {
  LlmProvider,
  LlmChatRequest,
  LlmChatResponse,
  LlmStreamChunk,
  LlmMessage,
  LlmToolDefinition,
  ProviderConfig,
} from './provider-interface';

interface OllamaMessage {
  role: 'user' | 'assistant' | 'system' | 'tool';
  content: string;
  tool_calls?: OllamaToolCall[];
}

interface OllamaToolCall {
  function: {
    name: string;
    arguments: Record<string, unknown>;
  };
}

interface OllamaTool {
  type: 'function';
  function: {
    name: string;
    description: string;
    parameters: Record<string, unknown>;
  };
}

interface OllamaChatResponse {
  model: string;
  message: {
    role: string;
    content: string;
    tool_calls?: OllamaToolCall[];
  };
  done: boolean;
  done_reason?: string;
  total_duration?: number;
  prompt_eval_count?: number;
  eval_count?: number;
}

export class OllamaProvider implements LlmProvider {
  readonly name = 'ollama';
  readonly model: string;
  private baseUrl: string;

  constructor(config: ProviderConfig) {
    this.model = config.model || 'llama3.1';
    this.baseUrl = config.baseUrl || 'http://localhost:11434';
  }

  private convertMessages(messages: LlmMessage[], systemPrompt?: string): OllamaMessage[] {
    const ollamaMessages: OllamaMessage[] = [];

    if (systemPrompt) {
      ollamaMessages.push({ role: 'system', content: systemPrompt });
    }

    for (const msg of messages) {
      if (msg.role === 'tool') {
        ollamaMessages.push({
          role: 'tool',
          content: msg.content ?? '',
        });
        continue;
      }

      if (msg.role === 'assistant' && msg.toolCalls && msg.toolCalls.length > 0) {
        ollamaMessages.push({
          role: 'assistant',
          content: msg.content ?? '',
          tool_calls: msg.toolCalls.map((tc) => ({
            function: {
              name: tc.name,
              arguments: JSON.parse(tc.arguments),
            },
          })),
        });
        continue;
      }

      ollamaMessages.push({
        role: msg.role as 'user' | 'assistant' | 'system',
        content: msg.content ?? '',
      });
    }

    return ollamaMessages;
  }

  private convertTools(tools: LlmToolDefinition[]): OllamaTool[] {
    return tools.map((tool) => ({
      type: 'function' as const,
      function: {
        name: tool.name,
        description: tool.description,
        parameters: tool.parametersSchema,
      },
    }));
  }

  async chat(request: LlmChatRequest): Promise<LlmChatResponse> {
    const body: Record<string, unknown> = {
      model: this.model,
      messages: this.convertMessages(request.messages, request.systemPrompt),
      stream: false,
      options: {} as Record<string, unknown>,
    };

    if (request.tools && request.tools.length > 0) {
      body.tools = this.convertTools(request.tools);
    }

    if (request.temperature !== undefined) {
      (body.options as Record<string, unknown>).temperature = request.temperature;
    }

    if (request.maxTokens) {
      (body.options as Record<string, unknown>).num_predict = request.maxTokens;
    }

    const response = await fetch(`${this.baseUrl}/api/chat`, {
      method: 'POST',
      headers: { 'Content-Type': 'application/json' },
      body: JSON.stringify(body),
    });

    if (!response.ok) {
      const errorBody = await response.text();
      throw new Error(`Ollama API error (${response.status}): ${errorBody}`);
    }

    const data = (await response.json()) as OllamaChatResponse;

    const toolCalls = (data.message.tool_calls ?? []).map((tc, index) => ({
      id: `ollama-tc-${crypto.randomUUID().slice(0, 8)}-${index}`,
      name: tc.function.name,
      arguments: JSON.stringify(tc.function.arguments),
    }));

    return {
      messageId: `ollama-${crypto.randomUUID().slice(0, 12)}`,
      content: data.message.content || null,
      toolCalls,
      finishReason: data.done_reason ?? (data.done ? 'stop' : 'unknown'),
      usage: {
        inputTokens: data.prompt_eval_count ?? 0,
        outputTokens: data.eval_count ?? 0,
      },
    };
  }

  async *chatStream(request: LlmChatRequest): AsyncIterable<LlmStreamChunk> {
    const body: Record<string, unknown> = {
      model: this.model,
      messages: this.convertMessages(request.messages, request.systemPrompt),
      stream: true,
      options: {} as Record<string, unknown>,
    };

    if (request.tools && request.tools.length > 0) {
      body.tools = this.convertTools(request.tools);
    }

    if (request.temperature !== undefined) {
      (body.options as Record<string, unknown>).temperature = request.temperature;
    }

    if (request.maxTokens) {
      (body.options as Record<string, unknown>).num_predict = request.maxTokens;
    }

    const response = await fetch(`${this.baseUrl}/api/chat`, {
      method: 'POST',
      headers: { 'Content-Type': 'application/json' },
      body: JSON.stringify(body),
    });

    if (!response.ok) {
      const errorBody = await response.text();
      throw new Error(`Ollama API streaming error (${response.status}): ${errorBody}`);
    }

    const reader = response.body?.getReader();
    if (!reader) {
      throw new Error('No response body for streaming');
    }

    const decoder = new TextDecoder();
    let buffer = '';
    let messageStarted = false;

    try {
      while (true) {
        const { done, value } = await reader.read();
        if (done) break;

        buffer += decoder.decode(value, { stream: true });
        const lines = buffer.split('\n');
        buffer = lines.pop() ?? '';

        for (const line of lines) {
          if (!line.trim()) continue;

          let chunk: OllamaChatResponse;
          try {
            chunk = JSON.parse(line);
          } catch {
            continue;
          }

          if (!messageStarted) {
            messageStarted = true;
            yield {
              type: 'message_start',
              messageId: `ollama-${crypto.randomUUID().slice(0, 12)}`,
            };
          }

          if (chunk.message?.content) {
            yield {
              type: 'content_delta',
              delta: chunk.message.content,
            };
          }

          if (chunk.message?.tool_calls) {
            for (const tc of chunk.message.tool_calls) {
              yield {
                type: 'tool_call_delta',
                toolCall: {
                  id: `ollama-tc-${crypto.randomUUID().slice(0, 8)}`,
                  name: tc.function.name,
                  arguments: JSON.stringify(tc.function.arguments),
                },
              };
            }
          }

          if (chunk.done) {
            yield {
              type: 'message_stop',
              finishReason: chunk.done_reason ?? 'stop',
              usage: {
                inputTokens: chunk.prompt_eval_count ?? 0,
                outputTokens: chunk.eval_count ?? 0,
              },
            };
          }
        }
      }
    } finally {
      reader.releaseLock();
    }
  }
}
