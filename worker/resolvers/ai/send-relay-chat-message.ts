import { requireIdentity } from '../../lib/auth/require-identity';

interface ResolverContext {
  db: D1Database;
  env: WorkerEnv;
  auth: { userId: string; tenantId: string };
}

interface WorkerEnv {
  CHAT_RELAY_DO: DurableObjectNamespace;
  [key: string]: unknown;
}

interface RelayChatMessageInput {
  sessionId: string;
  content: string;
  provider?: string;
  model?: string;
  workingDirectory?: string;
}

/**
 * Relay a chat message from mobile to the desktop via ChatRelayDO.
 *
 * 1. Find an online desktop node for this user
 * 2. Route the request to the ChatRelayDO for that node
 * 3. The DO forwards to the desktop via WebSocket
 */
export default async function sendRelayChatMessage(
  _parent: unknown,
  args: { input: RelayChatMessageInput },
  ctx: ResolverContext,
) {
  const id = await requireIdentity(ctx as any);

  const { sessionId, content, provider, model, workingDirectory } = args.input;

  // Find an online desktop node for this user
  const node = await ctx.db
    .prepare(
      `SELECT id FROM desktop_nodes
       WHERE tenant_id = ? AND user_id = ? AND status = 'online' AND is_deleted = 0
       ORDER BY last_heartbeat_at DESC LIMIT 1`,
    )
    .bind(id.tenantId, id.userId)
    .first<{ id: string }>();

  if (!node) {
    throw new Error('No online desktop node found. Please ensure your desktop app is running and connected.');
  }

  // Get or create a ChatRelayDO instance for this desktop node
  const doId = (ctx.env.CHAT_RELAY_DO as DurableObjectNamespace).idFromName(node.id);
  const doStub = (ctx.env.CHAT_RELAY_DO as DurableObjectNamespace).get(doId);

  const relayId = crypto.randomUUID();
  const relayData: Record<string, unknown> = {
    relayId,
    sessionId,
    content,
    userId: id.userId,
    tenantId: id.tenantId,
  };

  if (provider?.trim()) relayData.provider = provider.trim();
  if (model?.trim()) relayData.model = model.trim();
  if (workingDirectory?.trim()) relayData.workingDirectory = workingDirectory.trim();

  // Send the relay request to the DO
  const response = await doStub.fetch('https://internal/relay', {
    method: 'POST',
    headers: { 'Content-Type': 'application/json' },
    body: JSON.stringify({
      type: 'chat_request',
      data: relayData,
    }),
  });

  if (!response.ok) {
    throw new Error('Failed to relay message to desktop node');
  }

  return {
    success: true,
    relayId,
    desktopNodeId: node.id,
  };
}
