interface ResolverContext {
  db: D1Database;
  env: WorkerEnv;
  auth: { userId: string; tenantId: string };
}

interface WorkerEnv {
  [key: string]: unknown;
}

interface RegisterDesktopNodeInput {
  hostname: string;
  os?: string;
  arch?: string;
  version?: string;
  capabilities?: string;
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

export default async function registerDesktopNode(
  parent: unknown,
  args: { input: RegisterDesktopNodeInput },
  ctx: ResolverContext,
) {
  const { hostname, os, arch, version, capabilities } = args.input;

  // Check if node with same hostname already exists for this user
  const existing = await ctx.db
    .prepare(
      `SELECT id FROM desktop_nodes WHERE tenant_id = ? AND user_id = ? AND hostname = ? AND is_deleted = 0`,
    )
    .bind(ctx.auth.tenantId, ctx.auth.userId, hostname)
    .first<{ id: string }>();

  let nodeId: string;

  if (existing) {
    // Update existing node
    nodeId = existing.id;
    await ctx.db
      .prepare(
        `UPDATE desktop_nodes
         SET os = ?, arch = ?, version = ?, capabilities = ?, status = 'online', last_heartbeat_at = datetime('now'), updated_at = datetime('now')
         WHERE id = ? AND tenant_id = ?`,
      )
      .bind(
        os ?? null,
        arch ?? null,
        version ?? null,
        capabilities ?? '[]',
        nodeId,
        ctx.auth.tenantId,
      )
      .run();
  } else {
    // Insert new node
    nodeId = crypto.randomUUID();
    await ctx.db
      .prepare(
        `INSERT INTO desktop_nodes (id, tenant_id, user_id, hostname, os, arch, version, status, capabilities, last_heartbeat_at, is_deleted, created_at, updated_at)
         VALUES (?, ?, ?, ?, ?, ?, ?, 'online', ?, datetime('now'), 0, datetime('now'), datetime('now'))`,
      )
      .bind(
        nodeId,
        ctx.auth.tenantId,
        ctx.auth.userId,
        hostname,
        os ?? null,
        arch ?? null,
        version ?? null,
        capabilities ?? '[]',
      )
      .run();
  }

  const node = await ctx.db
    .prepare(`SELECT * FROM desktop_nodes WHERE id = ?`)
    .bind(nodeId)
    .first<DesktopNodeRow>();

  if (!node) {
    throw new Error('Failed to register desktop node');
  }

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
