interface ResolverContext {
  db: D1Database;
  env: WorkerEnv;
  auth: { userId: string; tenantId: string };
}

interface WorkerEnv {
  CHAT_RELAY_DO: DurableObjectNamespace;
  [key: string]: unknown;
}

/**
 * Query messages for a session from the desktop's local SQLite via RPC-over-relay.
 */
export default async function listAiMessages(
  _parent: unknown,
  args: { sessionId: string; limit?: number; offset?: number },
  ctx: ResolverContext,
) {
  const node = await ctx.db
    .prepare(
      `SELECT id FROM desktop_nodes
       WHERE tenant_id = ? AND user_id = ? AND status = 'online' AND is_deleted = 0
       ORDER BY last_heartbeat_at DESC LIMIT 1`,
    )
    .bind(ctx.auth.tenantId, ctx.auth.userId)
    .first<{ id: string }>();

  if (!node) {
    throw new Error('No online desktop node found. Please ensure your desktop app is running and connected.');
  }

  const doId = (ctx.env.CHAT_RELAY_DO as DurableObjectNamespace).idFromName(node.id);
  const doStub = (ctx.env.CHAT_RELAY_DO as DurableObjectNamespace).get(doId);

  const response = await doStub.fetch('https://internal/relay-rpc', {
    method: 'POST',
    headers: { 'Content-Type': 'application/json' },
    body: JSON.stringify({
      method: 'list_messages',
      params: {
        sessionId: args.sessionId,
        limit: args.limit ?? 100,
        offset: args.offset ?? 0,
      },
    }),
  });

  if (!response.ok) {
    const body = await response.text();
    throw new Error(`RPC failed (${response.status}): ${body}`);
  }

  const result = (await response.json()) as { success: boolean; data?: unknown; error?: string };
  if (!result.success) {
    throw new Error(result.error ?? 'RPC request failed');
  }

  return result.data ?? [];
}
