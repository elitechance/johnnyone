import {
  hostGraphqlRequest,
  hostGraphqlSubscribe,
  type HostGraphqlSubscription,
} from '../../lib/runtime/host-graphql';

interface ResolverContext {
  env: WorkerEnv;
  auth: { userId: string; tenantId: string };
  pubsub?: {
    publish: (topic: string, data: unknown) => void;
  };
}

interface WorkerEnv {
  HOST_GRAPHQL_URL?: string;
  [key: string]: unknown;
}

interface SendAiChatMessageInput {
  sessionId: string;
  content: string;
}

interface AiChatDeltaPayload {
  sessionId: string;
  messageId: string;
  delta: string;
  chunkType: string;
  isFinal: boolean;
}

interface AiChatCompletePayload {
  sessionId: string;
  messageId: string;
}

function wait(ms: number): Promise<void> {
  return new Promise((resolve) => setTimeout(resolve, ms));
}

export default async function sendAiChatMessage(
  _parent: unknown,
  args: { input: SendAiChatMessageInput },
  ctx: ResolverContext,
) {
  const pubsub = ctx.pubsub;
  const sessionId = args.input.sessionId;
  let sawDelta = false;
  let sawComplete = false;
  let resolveCompletionEvent: (() => void) | null = null;
  const completionEvent = new Promise<void>((resolve) => {
    resolveCompletionEvent = resolve;
  });
  const subscriptions: HostGraphqlSubscription<unknown>[] = [];

  if (pubsub) {
    try {
      const [deltaSubscription, completeSubscription] = await Promise.all([
        hostGraphqlSubscribe<{ onAiChatDelta: AiChatDeltaPayload }>(
          ctx.env,
          `subscription OnAiChatDelta($sessionId: String!) {
            onAiChatDelta(sessionId: $sessionId) {
              sessionId
              messageId
              delta
              chunkType
              isFinal
            }
          }`,
          { sessionId },
          (payload) => {
            sawDelta = true;
            pubsub.publish(
              `ai-chat-delta:${payload.onAiChatDelta.sessionId}`,
              payload.onAiChatDelta,
            );
          },
        ),
        hostGraphqlSubscribe<{ onAiChatComplete: AiChatCompletePayload }>(
          ctx.env,
          `subscription OnAiChatComplete($sessionId: String!) {
            onAiChatComplete(sessionId: $sessionId) {
              sessionId
              messageId
            }
          }`,
          { sessionId },
          (payload) => {
            sawComplete = true;
            resolveCompletionEvent?.();
            resolveCompletionEvent = null;
            pubsub.publish(
              `ai-chat-complete:${payload.onAiChatComplete.sessionId}`,
              payload.onAiChatComplete,
            );
          },
        ),
      ]);

      subscriptions.push(deltaSubscription, completeSubscription);
    } catch (err) {
      console.error('Failed to establish host chat subscriptions:', err);
    }
  }

  try {
    const result = await hostGraphqlRequest<{
      sendAiChatMessage: {
        userMessage: {
          id: string;
          sessionId: string;
          role: string;
          content: string;
          toolCalls?: string | null;
          finishReason?: string | null;
          inputTokens: number;
          outputTokens: number;
          costCents: number;
          createdAt: string;
        };
        assistantMessage: {
          id: string;
          sessionId: string;
          role: string;
          content: string;
          toolCalls?: string | null;
          finishReason?: string | null;
          inputTokens: number;
          outputTokens: number;
          costCents: number;
          createdAt: string;
        };
      };
    }>(
      ctx.env,
      `mutation SendAiChatMessage($input: SendAiChatMessageInput!) {
        sendAiChatMessage(input: $input) {
          userMessage {
            id
            sessionId
            role
            content
            toolCalls
            finishReason
            inputTokens
            outputTokens
            costCents
            createdAt
          }
          assistantMessage {
            id
            sessionId
            role
            content
            toolCalls
            finishReason
            inputTokens
            outputTokens
            costCents
            createdAt
          }
        }
      }`,
      {
        input: {
          sessionId: args.input.sessionId,
          content: args.input.content,
        },
      },
    );

    if (pubsub) {
      const payload = result.sendAiChatMessage;

      await Promise.race([completionEvent, wait(250)]);

      if (!sawDelta) {
        pubsub.publish(`ai-chat-delta:${payload.assistantMessage.sessionId}`, {
          sessionId: payload.assistantMessage.sessionId,
          messageId: payload.assistantMessage.id,
          delta: payload.assistantMessage.content,
          chunkType: 'text',
          isFinal: true,
        });
      }

      if (!sawComplete) {
        pubsub.publish(`ai-chat-complete:${payload.assistantMessage.sessionId}`, {
          sessionId: payload.assistantMessage.sessionId,
          messageId: payload.assistantMessage.id,
        });
      }
    }

    return result.sendAiChatMessage;
  } finally {
    for (const subscription of subscriptions) {
      subscription.unsubscribe();
    }
  }
}
