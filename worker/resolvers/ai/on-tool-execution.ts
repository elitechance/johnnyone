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
 * Subscription resolver for tool execution status changes on a session.
 * Delivers ToolExecution events when tools transition through statuses:
 * pending -> approved/rejected -> running -> completed/failed/cancelled
 */
export default async function onToolExecution(
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

  const topic = `tool-execution:${args.sessionId}`;

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
        type: 'tool-execution',
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
