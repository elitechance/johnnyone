import { requireIdentity } from '../../lib/auth/require-identity';

interface ResolverContext {
  db: D1Database;
  env: WorkerEnv;
  auth: { userId: string; tenantId: string };
}

interface WorkerEnv {
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

function mapDesktopNodeRow(row: DesktopNodeRow) {
  return {
    id: row.id,
    tenantId: row.tenant_id,
    userId: row.user_id,
    hostname: row.hostname,
    os: row.os,
    arch: row.arch,
    version: row.version,
    status: row.status,
    capabilities: row.capabilities,
    lastHeartbeatAt: row.last_heartbeat_at,
    createdAt: row.created_at,
    updatedAt: row.updated_at,
  };
}

export default async function listDesktopNodes(
  parent: unknown,
  args: Record<string, never>,
  ctx: ResolverContext,
) {
  const id = await requireIdentity(ctx as any);

  const result = await ctx.db
    .prepare(
      `SELECT * FROM desktop_nodes WHERE tenant_id = ? AND user_id = ? AND is_deleted = 0 ORDER BY last_heartbeat_at DESC`,
    )
    .bind(id.tenantId, id.userId)
    .all<DesktopNodeRow>();

  return result.results.map(mapDesktopNodeRow);
}
