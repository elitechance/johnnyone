import type {
  LlmProvider,
  LlmChatRequest,
  LlmChatResponse,
  LlmStreamChunk,
  LlmMessage,
  LlmToolDefinition,
  ProviderConfig,
} from './provider-interface';

interface OpenAiMessage {
  role: 'user' | 'assistant' | 'system' | 'tool';
  content: string | null;
  tool_calls?: OpenAiToolCall[];
  tool_call_id?: string;
  name?: string;
}

interface OpenAiToolCall {
  id: string;
  type: 'function';
  function: {
    name: string;
    arguments: string;
  };
}

interface OpenAiTool {
  type: 'function';
  function: {
    name: string;
    description: string;
    parameters: Record<string, unknown>;
  };
}

interface OpenAiChoice {
  index: number;
  message: {
    role: string;
    content: string | null;
    tool_calls?: OpenAiToolCall[];
  };
  finish_reason: string;
}

interface OpenAiResponse {
  id: string;
  choices: OpenAiChoice[];
  usage: {
    prompt_tokens: number;
    completion_tokens: number;
    total_tokens: number;
  };
}

interface OpenAiStreamDelta {
  role?: string;
  content?: string | null;
  tool_calls?: Array<{
    index: number;
    id?: string;
    type?: string;
    function?: {
      name?: string;
      arguments?: string;
    };
  }>;
}

interface OpenAiStreamChoice {
  index: number;
  delta: OpenAiStreamDelta;
  finish_reason: string | null;
}

interface OpenAiStreamEvent {
  id: string;
  choices: OpenAiStreamChoice[];
  usage?: {
    prompt_tokens: number;
    completion_tokens: number;
  };
}

export class OpenAiProvider implements LlmProvider {
  readonly name = 'openai';
  readonly model: string;
  private apiKey: string;
  private baseUrl: string;

  constructor(config: ProviderConfig, apiKey: string) {
    this.model = config.model || 'gpt-4o';
    this.apiKey = apiKey;
    this.baseUrl = config.baseUrl || 'https://api.openai.com';
  }

  private convertMessages(messages: LlmMessage[], systemPrompt?: string): OpenAiMessage[] {
    const openaiMessages: OpenAiMessage[] = [];

    if (systemPrompt) {
      openaiMessages.push({ role: 'system', content: systemPrompt });
    }

    for (const msg of messages) {
      if (msg.role === 'system') {
        openaiMessages.push({ role: 'system', content: msg.content });
        continue;
      }

      if (msg.role === 'tool') {
        openaiMessages.push({
          role: 'tool',
          content: msg.content,
          tool_call_id: msg.toolCallId,
        });
        continue;
      }

      if (msg.role === 'assistant' && msg.toolCalls && msg.toolCalls.length > 0) {
        openaiMessages.push({
          role: 'assistant',
          content: msg.content,
          tool_calls: msg.toolCalls.map((tc) => ({
            id: tc.id,
            type: 'function' as const,
            function: {
              name: tc.name,
              arguments: tc.arguments,
            },
          })),
        });
        continue;
      }

      openaiMessages.push({
        role: msg.role as 'user' | 'assistant',
        content: msg.content,
      });
    }

    return openaiMessages;
  }

  private convertTools(tools: LlmToolDefinition[]): OpenAiTool[] {
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
      max_tokens: request.maxTokens ?? 4096,
    };

    if (request.tools && request.tools.length > 0) {
      body.tools = this.convertTools(request.tools);
    }

    if (request.temperature !== undefined) {
      body.temperature = request.temperature;
    }

    const response = await fetch(`${this.baseUrl}/v1/chat/completions`, {
      method: 'POST',
      headers: {
        'Content-Type': 'application/json',
        Authorization: `Bearer ${this.apiKey}`,
      },
      body: JSON.stringify(body),
    });

    if (!response.ok) {
      const errorBody = await response.text();
      throw new Error(`OpenAI API error (${response.status}): ${errorBody}`);
    }

    const data = (await response.json()) as OpenAiResponse;
    const choice = data.choices[0];

    const toolCalls = (choice.message.tool_calls ?? []).map((tc) => ({
      id: tc.id,
      name: tc.function.name,
      arguments: tc.function.arguments,
    }));

    return {
      messageId: data.id,
      content: choice.message.content,
      toolCalls,
      finishReason: choice.finish_reason,
      usage: {
        inputTokens: data.usage.prompt_tokens,
        outputTokens: data.usage.completion_tokens,
      },
    };
  }

  async *chatStream(request: LlmChatRequest): AsyncIterable<LlmStreamChunk> {
    const body: Record<string, unknown> = {
      model: this.model,
      messages: this.convertMessages(request.messages, request.systemPrompt),
      max_tokens: request.maxTokens ?? 4096,
      stream: true,
      stream_options: { include_usage: true },
    };

    if (request.tools && request.tools.length > 0) {
      body.tools = this.convertTools(request.tools);
    }

    if (request.temperature !== undefined) {
      body.temperature = request.temperature;
    }

    const response = await fetch(`${this.baseUrl}/v1/chat/completions`, {
      method: 'POST',
      headers: {
        'Content-Type': 'application/json',
        Authorization: `Bearer ${this.apiKey}`,
      },
      body: JSON.stringify(body),
    });

    if (!response.ok) {
      const errorBody = await response.text();
      throw new Error(`OpenAI API streaming error (${response.status}): ${errorBody}`);
    }

    const reader = response.body?.getReader();
    if (!reader) {
      throw new Error('No response body for streaming');
    }

    const decoder = new TextDecoder();
    let buffer = '';
    const activeToolCalls = new Map<number, { id: string; name: string; arguments: string }>();

    try {
      while (true) {
        const { done, value } = await reader.read();
        if (done) break;

        buffer += decoder.decode(value, { stream: true });
        const lines = buffer.split('\n');
        buffer = lines.pop() ?? '';

        for (const line of lines) {
          if (!line.startsWith('data: ')) continue;
          const data = line.slice(6).trim();
          if (data === '[DONE]') continue;

          let event: OpenAiStreamEvent;
          try {
            event = JSON.parse(data);
          } catch {
            continue;
          }

          const choice = event.choices?.[0];

          if (!choice) {
            // Usage-only event at the end
            if (event.usage) {
              yield {
                type: 'message_stop',
                usage: {
                  inputTokens: event.usage.prompt_tokens,
                  outputTokens: event.usage.completion_tokens,
                },
              };
            }
            continue;
          }

          const delta = choice.delta;

          // Role indicates message start
          if (delta.role) {
            yield { type: 'message_start', messageId: event.id };
          }

          // Content delta
          if (delta.content) {
            yield { type: 'content_delta', delta: delta.content };
          }

          // Tool call deltas
          if (delta.tool_calls) {
            for (const tc of delta.tool_calls) {
              if (tc.id) {
                activeToolCalls.set(tc.index, {
                  id: tc.id,
                  name: tc.function?.name ?? '',
                  arguments: tc.function?.arguments ?? '',
                });
              } else if (tc.function?.arguments) {
                const existing = activeToolCalls.get(tc.index);
                if (existing) {
                  existing.arguments += tc.function.arguments;
                  if (tc.function.name) {
                    existing.name = tc.function.name;
                  }
                }
              }

              const current = activeToolCalls.get(tc.index);
              if (current) {
                yield {
                  type: 'tool_call_delta',
                  toolCall: { ...current },
                };
              }
            }
          }

          // Finish reason
          if (choice.finish_reason) {
            yield {
              type: 'message_stop',
              finishReason: choice.finish_reason,
            };
          }
        }
      }
    } finally {
      reader.releaseLock();
    }
  }
}
