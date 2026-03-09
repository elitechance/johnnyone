import {
  hostGraphqlRequest,
  hostGraphqlSubscribe,
  type HostGraphqlSubscription,
} from '../../lib/runtime/host-graphql';

interface ResolverContext {
  env: WorkerEnv;
  auth: { userId: string; tenantId: string };
}

interface WorkerEnv {
  HOST_GRAPHQL_URL?: string;
  PUBSUB_DO?: DurableObjectNamespace;
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

async function publishTopic(
  pubsub: DurableObjectStub,
  topic: string,
  payload: unknown,
): Promise<void> {
  await pubsub.fetch(
    new Request('https://do.internal/publish', {
      method: 'POST',
      headers: { 'Content-Type': 'application/json' },
      body: JSON.stringify({ topic, payload }),
    }),
  );
}

function wait(ms: number): Promise<void> {
  return new Promise((resolve) => setTimeout(resolve, ms));
}

export default async function sendAiChatMessage(
  _parent: unknown,
  args: { input: SendAiChatMessageInput },
  ctx: ResolverContext,
) {
  const pubsubNamespace = ctx.env.PUBSUB_DO as DurableObjectNamespace | undefined;
  const pubsub = pubsubNamespace
    ? pubsubNamespace.get(pubsubNamespace.idFromName('global'))
    : null;
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
            void publishTopic(
              pubsub,
              `ai-chat-delta:${payload.onAiChatDelta.sessionId}`,
              payload.onAiChatDelta,
            ).catch((err: Error) => {
              console.error('Failed to publish streamed ai chat delta:', err);
            });
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
            void publishTopic(
              pubsub,
              `ai-chat-complete:${payload.onAiChatComplete.sessionId}`,
              payload.onAiChatComplete,
            ).catch((err: Error) => {
              console.error('Failed to publish streamed ai chat completion:', err);
            });
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
        await publishTopic(pubsub, `ai-chat-delta:${payload.assistantMessage.sessionId}`, {
          sessionId: payload.assistantMessage.sessionId,
          messageId: payload.assistantMessage.id,
          delta: payload.assistantMessage.content,
          chunkType: 'text',
          isFinal: true,
        }).catch((err: Error) => {
          console.error('Failed to publish fallback ai chat delta:', err);
        });
      }

      if (!sawComplete) {
        await publishTopic(pubsub, `ai-chat-complete:${payload.assistantMessage.sessionId}`, {
          sessionId: payload.assistantMessage.sessionId,
          messageId: payload.assistantMessage.id,
        }).catch((err: Error) => {
          console.error('Failed to publish fallback ai chat completion:', err);
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
