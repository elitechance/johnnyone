import type { LlmStreamChunk } from '../providers/provider-interface';

interface StreamRelayOptions {
  sessionId: string;
  messageId: string;
  broadcast: (msg: Record<string, unknown>) => void;
}

interface ConnectedClient {
  ws: WebSocket;
  userId: string;
  tenantId: string;
  sessionFilter?: string;
}

/**
 * StreamRelay bridges LLM streaming responses to connected WebSocket clients.
 *
 * It receives LlmStreamChunk events from the provider and converts them
 * into the GraphQL subscription format (AiMessageDelta) for delivery
 * to subscribed clients.
 */
export class StreamRelay {
  private sessionId: string;
  private messageId: string;
  private broadcast: (msg: Record<string, unknown>) => void;
  private chunkIndex: number = 0;
  private accumulatedContent: string = '';
  private accumulatedToolCalls: Array<{
    id: string;
    name: string;
    arguments: string;
  }> = [];

  constructor(options: StreamRelayOptions) {
    this.sessionId = options.sessionId;
    this.messageId = options.messageId;
    this.broadcast = options.broadcast;
  }

  /**
   * Process a stream of LLM chunks and relay them to connected clients.
   */
  async relayStream(stream: AsyncIterable<LlmStreamChunk>): Promise<{
    content: string;
    toolCalls: Array<{ id: string; name: string; arguments: string }>;
    finishReason: string;
    usage: { inputTokens: number; outputTokens: number };
  }> {
    let finishReason = 'stop';
    let usage = { inputTokens: 0, outputTokens: 0 };

    for await (const chunk of stream) {
      switch (chunk.type) {
        case 'message_start':
          this.broadcast({
            type: 'subscription:agent-message-delta',
            data: {
              sessionId: this.sessionId,
              messageId: chunk.messageId ?? this.messageId,
              delta: '',
              finishReason: null,
              index: this.chunkIndex++,
              event: 'start',
            },
          });
          break;

        case 'content_delta':
          if (chunk.delta) {
            this.accumulatedContent += chunk.delta;

            this.broadcast({
              type: 'subscription:agent-message-delta',
              data: {
                sessionId: this.sessionId,
                messageId: this.messageId,
                delta: chunk.delta,
                finishReason: null,
                index: this.chunkIndex++,
              },
            });
          }
          break;

        case 'tool_call_delta':
          if (chunk.toolCall) {
            // Update or add tool call
            if (chunk.toolCall.id) {
              const existing = this.accumulatedToolCalls.find(
                (tc) => tc.id === chunk.toolCall!.id,
              );
              if (existing) {
                if (chunk.toolCall.arguments) {
                  existing.arguments = chunk.toolCall.arguments;
                }
              } else {
                this.accumulatedToolCalls.push({
                  id: chunk.toolCall.id!,
                  name: chunk.toolCall.name ?? '',
                  arguments: chunk.toolCall.arguments ?? '',
                });
              }
            }

            this.broadcast({
              type: 'subscription:tool-call-delta',
              data: {
                sessionId: this.sessionId,
                messageId: this.messageId,
                toolCall: chunk.toolCall,
                index: this.chunkIndex++,
              },
            });
          }
          break;

        case 'message_stop':
          finishReason = chunk.finishReason ?? 'stop';
          if (chunk.usage) {
            usage = chunk.usage;
          }

          this.broadcast({
            type: 'subscription:agent-message-delta',
            data: {
              sessionId: this.sessionId,
              messageId: this.messageId,
              delta: '',
              finishReason,
              index: this.chunkIndex++,
              event: 'stop',
            },
          });
          break;

        case 'error':
          this.broadcast({
            type: 'subscription:agent-message-delta',
            data: {
              sessionId: this.sessionId,
              messageId: this.messageId,
              delta: '',
              finishReason: 'error',
              index: this.chunkIndex++,
              event: 'error',
              error: chunk.error,
            },
          });
          throw new Error(chunk.error ?? 'Stream error');
      }
    }

    return {
      content: this.accumulatedContent,
      toolCalls: this.accumulatedToolCalls,
      finishReason,
      usage,
    };
  }

  /**
   * Send a final complete message event to all subscribers.
   */
  sendComplete(message: {
    id: string;
    content: string | null;
    toolCalls: string;
    finishReason: string;
    inputTokens: number;
    outputTokens: number;
  }): void {
    this.broadcast({
      type: 'subscription:agent-message',
      data: {
        id: message.id,
        sessionId: this.sessionId,
        role: 'assistant',
        content: message.content,
        toolCalls: message.toolCalls,
        finishReason: message.finishReason,
        inputTokens: message.inputTokens,
        outputTokens: message.outputTokens,
      },
    });
  }
}
