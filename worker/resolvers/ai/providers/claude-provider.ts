import type {
  LlmProvider,
  LlmChatRequest,
  LlmChatResponse,
  LlmStreamChunk,
  LlmMessage,
  LlmToolDefinition,
  ProviderConfig,
} from './provider-interface';

interface AnthropicMessage {
  role: 'user' | 'assistant';
  content: string | AnthropicContentBlock[];
}

interface AnthropicContentBlock {
  type: 'text' | 'tool_use' | 'tool_result';
  text?: string;
  id?: string;
  name?: string;
  input?: Record<string, unknown>;
  tool_use_id?: string;
  content?: string;
}

interface AnthropicTool {
  name: string;
  description: string;
  input_schema: Record<string, unknown>;
}

interface AnthropicResponse {
  id: string;
  type: string;
  role: string;
  content: AnthropicContentBlock[];
  stop_reason: string;
  usage: {
    input_tokens: number;
    output_tokens: number;
  };
}

interface AnthropicStreamEvent {
  type: string;
  index?: number;
  message?: AnthropicResponse;
  delta?: {
    type?: string;
    text?: string;
    partial_json?: string;
    stop_reason?: string;
  };
  content_block?: AnthropicContentBlock;
  usage?: {
    input_tokens?: number;
    output_tokens?: number;
  };
}

export class ClaudeProvider implements LlmProvider {
  readonly name = 'claude';
  readonly model: string;
  private apiKey: string;
  private baseUrl: string;

  constructor(config: ProviderConfig, apiKey: string) {
    this.model = config.model || 'claude-sonnet-4-20250514';
    this.apiKey = apiKey;
    this.baseUrl = config.baseUrl || 'https://api.anthropic.com';
  }

  private convertMessages(messages: LlmMessage[]): AnthropicMessage[] {
    const anthropicMessages: AnthropicMessage[] = [];

    for (const msg of messages) {
      if (msg.role === 'system') {
        // System messages are handled via the system parameter
        continue;
      }

      if (msg.role === 'tool') {
        // Tool results become user messages with tool_result content blocks
        anthropicMessages.push({
          role: 'user',
          content: [
            {
              type: 'tool_result',
              tool_use_id: msg.toolCallId,
              content: msg.content ?? '',
            },
          ],
        });
        continue;
      }

      if (msg.role === 'assistant' && msg.toolCalls && msg.toolCalls.length > 0) {
        const contentBlocks: AnthropicContentBlock[] = [];
        if (msg.content) {
          contentBlocks.push({ type: 'text', text: msg.content });
        }
        for (const tc of msg.toolCalls) {
          contentBlocks.push({
            type: 'tool_use',
            id: tc.id,
            name: tc.name,
            input: JSON.parse(tc.arguments),
          });
        }
        anthropicMessages.push({ role: 'assistant', content: contentBlocks });
        continue;
      }

      anthropicMessages.push({
        role: msg.role as 'user' | 'assistant',
        content: msg.content ?? '',
      });
    }

    return anthropicMessages;
  }

  private convertTools(tools: LlmToolDefinition[]): AnthropicTool[] {
    return tools.map((tool) => ({
      name: tool.name,
      description: tool.description,
      input_schema: tool.parametersSchema,
    }));
  }

  async chat(request: LlmChatRequest): Promise<LlmChatResponse> {
    const body: Record<string, unknown> = {
      model: this.model,
      messages: this.convertMessages(request.messages),
      max_tokens: request.maxTokens ?? 4096,
    };

    if (request.systemPrompt) {
      body.system = request.systemPrompt;
    }

    if (request.tools && request.tools.length > 0) {
      body.tools = this.convertTools(request.tools);
    }

    if (request.temperature !== undefined) {
      body.temperature = request.temperature;
    }

    const response = await fetch(`${this.baseUrl}/v1/messages`, {
      method: 'POST',
      headers: {
        'Content-Type': 'application/json',
        'x-api-key': this.apiKey,
        'anthropic-version': '2023-06-01',
      },
      body: JSON.stringify(body),
    });

    if (!response.ok) {
      const errorBody = await response.text();
      throw new Error(`Claude API error (${response.status}): ${errorBody}`);
    }

    const data = (await response.json()) as AnthropicResponse;

    let content: string | null = null;
    const toolCalls: { id: string; name: string; arguments: string }[] = [];

    for (const block of data.content) {
      if (block.type === 'text' && block.text) {
        content = (content ?? '') + block.text;
      } else if (block.type === 'tool_use' && block.id && block.name) {
        toolCalls.push({
          id: block.id,
          name: block.name,
          arguments: JSON.stringify(block.input ?? {}),
        });
      }
    }

    return {
      messageId: data.id,
      content,
      toolCalls,
      finishReason: data.stop_reason,
      usage: {
        inputTokens: data.usage.input_tokens,
        outputTokens: data.usage.output_tokens,
      },
    };
  }

  async *chatStream(request: LlmChatRequest): AsyncIterable<LlmStreamChunk> {
    const body: Record<string, unknown> = {
      model: this.model,
      messages: this.convertMessages(request.messages),
      max_tokens: request.maxTokens ?? 4096,
      stream: true,
    };

    if (request.systemPrompt) {
      body.system = request.systemPrompt;
    }

    if (request.tools && request.tools.length > 0) {
      body.tools = this.convertTools(request.tools);
    }

    if (request.temperature !== undefined) {
      body.temperature = request.temperature;
    }

    const response = await fetch(`${this.baseUrl}/v1/messages`, {
      method: 'POST',
      headers: {
        'Content-Type': 'application/json',
        'x-api-key': this.apiKey,
        'anthropic-version': '2023-06-01',
      },
      body: JSON.stringify(body),
    });

    if (!response.ok) {
      const errorBody = await response.text();
      throw new Error(`Claude API streaming error (${response.status}): ${errorBody}`);
    }

    const reader = response.body?.getReader();
    if (!reader) {
      throw new Error('No response body for streaming');
    }

    const decoder = new TextDecoder();
    let buffer = '';
    let currentToolCall: Partial<{ id: string; name: string; arguments: string }> | null = null;

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

          let event: AnthropicStreamEvent;
          try {
            event = JSON.parse(data);
          } catch {
            continue;
          }

          switch (event.type) {
            case 'message_start':
              yield {
                type: 'message_start',
                messageId: event.message?.id,
              };
              break;

            case 'content_block_start':
              if (event.content_block?.type === 'tool_use') {
                currentToolCall = {
                  id: event.content_block.id,
                  name: event.content_block.name,
                  arguments: '',
                };
              }
              break;

            case 'content_block_delta':
              if (event.delta?.type === 'text_delta' && event.delta.text) {
                yield {
                  type: 'content_delta',
                  delta: event.delta.text,
                };
              } else if (event.delta?.type === 'input_json_delta' && event.delta.partial_json) {
                if (currentToolCall) {
                  currentToolCall.arguments =
                    (currentToolCall.arguments ?? '') + event.delta.partial_json;
                }
                yield {
                  type: 'tool_call_delta',
                  toolCall: currentToolCall ? { ...currentToolCall } : undefined,
                };
              }
              break;

            case 'content_block_stop':
              if (currentToolCall) {
                yield {
                  type: 'tool_call_delta',
                  toolCall: {
                    id: currentToolCall.id,
                    name: currentToolCall.name,
                    arguments: currentToolCall.arguments,
                  },
                };
                currentToolCall = null;
              }
              break;

            case 'message_delta':
              if (event.delta?.stop_reason) {
                yield {
                  type: 'message_stop',
                  finishReason: event.delta.stop_reason,
                  usage: event.usage
                    ? {
                        inputTokens: event.usage.input_tokens ?? 0,
                        outputTokens: event.usage.output_tokens ?? 0,
                      }
                    : undefined,
                };
              }
              break;
          }
        }
      }
    } finally {
      reader.releaseLock();
    }
  }
}
