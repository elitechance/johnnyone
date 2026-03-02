interface ResolverContext {
  db: D1Database;
  env: WorkerEnv;
  auth: { userId: string; tenantId: string };
}

interface WorkerEnv {
  PUBSUB_DO: DurableObjectNamespace;
  [key: string]: unknown;
}

interface DesktopNodeRow {
  id: string;
  tenant_id: string;
  user_id: string;
  hostname: string;
  os: string | null;
  arch: string | null;
  version: string | null;
  status: string;
  capabilities: string;
  last_heartbeat_at: string | null;
  is_deleted: number;
  created_at: string;
  updated_at: string;
}

export default async function updateDesktopNodeStatus(
  parent: unknown,
  args: { id: string; status: string },
  ctx: ResolverContext,
) {
  const validStatuses = ['online', 'offline', 'busy'];
  if (!validStatuses.includes(args.status)) {
    throw new Error(`Invalid status: ${args.status}. Must be one of: ${validStatuses.join(', ')}`);
  }

  const existing = await ctx.db
    .prepare(
      `SELECT id FROM desktop_nodes WHERE id = ? AND tenant_id = ? AND is_deleted = 0`,
    )
    .bind(args.id, ctx.auth.tenantId)
    .first();

  if (!existing) {
    throw new Error('Desktop node not found');
  }

  await ctx.db
    .prepare(
      `UPDATE desktop_nodes SET status = ?, last_heartbeat_at = datetime('now'), updated_at = datetime('now') WHERE id = ? AND tenant_id = ?`,
    )
    .bind(args.status, args.id, ctx.auth.tenantId)
    .run();

  const node = await ctx.db
    .prepare(`SELECT * FROM desktop_nodes WHERE id = ?`)
    .bind(args.id)
    .first<DesktopNodeRow>();

  if (!node) {
    throw new Error('Failed to update desktop node status');
  }

  // Publish node status change via PubSub DO
  const pubsubId = ctx.env.PUBSUB_DO.idFromName('global');
  const pubsub = ctx.env.PUBSUB_DO.get(pubsubId);

  pubsub
    .fetch(
      new Request('https://do.internal/publish', {
        method: 'POST',
        headers: { 'Content-Type': 'application/json' },
        body: JSON.stringify({
          topic: `desktop-node-status:${ctx.auth.tenantId}`,
          payload: {
            id: node.id,
            tenantId: node.tenant_id,
            userId: node.user_id,
            hostname: node.hostname,
            os: node.os,
            arch: node.arch,
            version: node.version,
            status: node.status,
            capabilities: node.capabilities,
            lastHeartbeatAt: node.last_heartbeat_at,
            createdAt: node.created_at,
            updatedAt: node.updated_at,
          },
        }),
      }),
    )
    .catch((err: Error) => {
      console.error('Failed to publish node status change:', err);
    });

  return {
    id: node.id,
    tenantId: node.tenant_id,
    userId: node.user_id,
    hostname: node.hostname,
    os: node.os,
    arch: node.arch,
    version: node.version,
    status: node.status,
    capabilities: node.capabilities,
    lastHeartbeatAt: node.last_heartbeat_at,
    createdAt: node.created_at,
    updatedAt: node.updated_at,
  };
}
