interface DesktopRpcEnv {
  CHAT_RELAY_DO: DurableObjectNamespace;
  [key: string]: unknown;
}

interface DesktopRpcContext {
  db: D1Database;
  env: DesktopRpcEnv;
  auth: { userId: string; tenantId: string };
}

interface DesktopRpcResult<T> {
  success?: boolean;
  data?: T;
  error?: string;
  timedOut?: boolean;
}

export async function desktopRpc<T>(
  ctx: DesktopRpcContext,
  method: string,
  params: Record<string, unknown> = {},
): Promise<T> {
  const node = await ctx.db
    .prepare(
      `SELECT id FROM desktop_nodes
       WHERE tenant_id = ? AND user_id = ? AND status = 'online' AND is_deleted = 0
       ORDER BY last_heartbeat_at DESC LIMIT 1`,
    )
    .bind(ctx.auth.tenantId, ctx.auth.userId)
    .first<{ id: string }>();

  if (!node) {
    throw new Error('No online backend app found. Start the backend app and connect it to the Worker.');
  }

  const doId = ctx.env.CHAT_RELAY_DO.idFromName(node.id);
  const doStub = ctx.env.CHAT_RELAY_DO.get(doId);
  const response = await doStub.fetch('https://internal/relay-rpc', {
    method: 'POST',
    headers: { 'Content-Type': 'application/json' },
    body: JSON.stringify({ method, params }),
  });

  const result = (await response.json()) as DesktopRpcResult<T>;
  if (!response.ok || result.timedOut || result.success === false) {
    throw new Error(result.error || `Backend RPC failed: ${method}`);
  }

  return result.data as T;
}
