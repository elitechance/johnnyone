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
 * Subscription resolver for desktop node status changes.
 * Delivers DesktopNode events when nodes come online, go offline,
 * or change to busy status. Scoped to the tenant.
 */
export default async function onDesktopNodeStatus(
  parent: unknown,
  args: Record<string, never>,
  ctx: ResolverContext,
) {
  const topic = `desktop-node-status:${ctx.auth.tenantId}`;

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
        type: 'desktop-node-status',
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
