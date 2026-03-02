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
 * Subscription resolver for streaming token deltas on a session.
 * Delivers AiMessageDelta events as the LLM generates tokens.
 * Each delta contains a partial content string, the message ID,
 * and an index for ordering.
 */
export default async function onAgentMessageDelta(
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

  const topic = `agent-message-delta:${args.sessionId}`;

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
        type: 'agent-message-delta',
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
