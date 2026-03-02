interface ResolverContext {
  db: D1Database;
  env: WorkerEnv;
  auth: { userId: string; tenantId: string };
}

interface WorkerEnv {
  PUBSUB_DO: DurableObjectNamespace;
  [key: string]: unknown;
}

/**
 * Subscription resolver for full agent messages on a session.
 * In the lokal framework, subscriptions return a topic identifier
 * that the PubSubDO uses to route WebSocket messages to subscribers.
 *
 * The actual WebSocket upgrade and message delivery is handled by the
 * PubSubDO durable object. This resolver validates access and returns
 * the subscription configuration.
 */
export default async function onAgentMessage(
  parent: unknown,
  args: { sessionId: string },
  ctx: ResolverContext,
) {
  // Verify session belongs to tenant
  const session = await ctx.db
    .prepare(
      `SELECT id FROM ai_sessions WHERE id = ? AND tenant_id = ? AND is_deleted = 0`,
    )
    .bind(args.sessionId, ctx.auth.tenantId)
    .first();

  if (!session) {
    throw new Error('Session not found');
  }

  // Return subscription topic for the PubSub system
  const topic = `agent-message:${args.sessionId}`;

  // Register subscription in PubSubDO
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
        sessionId: args.sessionId,
        type: 'agent-message',
      }),
    }),
  );

  if (!response.ok) {
    throw new Error('Failed to establish subscription');
  }

  const subscriptionInfo = (await response.json()) as { subscriptionId: string; wsUrl: string };

  return {
    __typename: '_Subscription',
    topic,
    subscriptionId: subscriptionInfo.subscriptionId,
    wsUrl: subscriptionInfo.wsUrl,
  };
}
