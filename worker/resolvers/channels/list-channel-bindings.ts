import { requireIdentity } from '../../lib/auth/require-identity';

interface ResolverContext {
  db: D1Database;
  env: WorkerEnv;
  auth: { userId: string; tenantId: string };
}

interface WorkerEnv {
  [key: string]: unknown;
}

interface ChannelBindingRow {
  id: string;
  tenant_id: string;
  user_id: string;
  channel_type: string;
  channel_identifier: string;
  channel_config: string;
  is_active: number;
  is_deleted: number;
  created_at: string;
  updated_at: string;
}

function mapChannelBindingRow(row: ChannelBindingRow) {
  return {
    id: row.id,
    tenantId: row.tenant_id,
    userId: row.user_id,
    channelType: row.channel_type,
    channelIdentifier: row.channel_identifier,
    channelConfig: row.channel_config,
    isActive: row.is_active === 1,
    createdAt: row.created_at,
    updatedAt: row.updated_at,
  };
}

/**
 * List channel bindings for the authenticated user.
 * Optionally filter by channel type.
 */
export default async function listChannelBindings(
  parent: unknown,
  args: { channelType?: string },
  ctx: ResolverContext,
) {
  const ident = await requireIdentity(ctx as any);
  let sql = `SELECT * FROM channel_bindings WHERE tenant_id = ? AND user_id = ? AND is_deleted = 0`;
  const bindings: string[] = [ident.tenantId, ident.userId];

  if (args.channelType) {
    sql += ` AND channel_type = ?`;
    bindings.push(args.channelType);
  }

  sql += ` ORDER BY created_at DESC`;

  const result = await ctx.db
    .prepare(sql)
    .bind(...bindings)
    .all<ChannelBindingRow>();

  return result.results.map(mapChannelBindingRow);
}
