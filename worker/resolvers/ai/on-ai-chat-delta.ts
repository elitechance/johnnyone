interface ResolverContext {
  env: WorkerEnv;
  auth: { userId: string; tenantId: string };
}

interface WorkerEnv {
  PUBSUB_DO: DurableObjectNamespace;
  [key: string]: unknown;
}

export default async function onAiChatDelta(
  _parent: unknown,
  args: { sessionId: string },
  ctx: ResolverContext,
) {
  const topic = `ai-chat-delta:${args.sessionId}`;

  const pubsubId = ctx.env.PUBSUB_DO.idFromName('global');
  const pubsub = ctx.env.PUBSUB_DO.get(pubsubId);

  const response = await pubsub.fetch(
    new Request('https://do.internal/subscribe', {
      method: 'POST',
      headers: { 'Content-Type': 'application/json' },
      body: JSON.stringify({
        topic,
        userId: ctx.auth.userId,
        tenantId: ctx.auth.tenantId,
        type: 'ai-chat-delta',
      }),
    }),
  );

  if (!response.ok) {
    throw new Error('Failed to establish ai chat delta subscription');
  }

  const subscriptionInfo = (await response.json()) as { subscriptionId: string; wsUrl: string };

  return {
    __typename: '_Subscription',
    topic,
    subscriptionId: subscriptionInfo.subscriptionId,
    wsUrl: subscriptionInfo.wsUrl,
  };
}
