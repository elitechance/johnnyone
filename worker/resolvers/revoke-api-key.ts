import type { ResolverContext } from 'lokal-cf-worker';
import { authorizeForAltToken } from '../lib/auth/api-key';

export default async function revokeApiKey(
  _parent: unknown,
  args: { id: string },
  ctx: ResolverContext & { apiKey?: { id: string; scopes: string[] } }
): Promise<any> {
  await authorizeForAltToken(ctx);
  const auth = (ctx as any).auth || {};
  const apiKey = (ctx as any).apiKey;

  if (apiKey) {
    throw new Error('API keys cannot manage API keys');
  }
  if (!auth.userId || !auth.tenantId) {
    throw new Error('Not authenticated');
  }
  const id = args && args.id;
  if (!id) {
    throw new Error('id is required');
  }

  const now = new Date().toISOString();

  // Scoped update: only affect caller's tenant+user key
  const res = await (ctx.db as any)
    .prepare(
      `UPDATE api_keys
       SET revoked_at = ?, updated_at = ?
       WHERE id = ? AND tenant_id = ? AND user_id = ? AND is_deleted = 0`
    )
    .bind(now, now, id, auth.tenantId, auth.userId)
    .run();

  // Idempotent: even if no row changed, fetch and return current state
  const row = await (ctx.db as any)
    .prepare(
      `SELECT id, name, key_prefix, scopes, last_used_at, expires_at, revoked_at, created_at
       FROM api_keys
       WHERE id = ? AND tenant_id = ? AND user_id = ? AND is_deleted = 0`
    )
    .bind(id, auth.tenantId, auth.userId)
    .first();

  if (!row) {
    throw new Error('not found');
  }

  return {
    id: row.id,
    name: row.name,
    keyPrefix: row.key_prefix,
    scopes: JSON.parse(row.scopes || '[]'),
    lastUsedAt: row.last_used_at || null,
    expiresAt: row.expires_at || null,
    revokedAt: row.revoked_at || null,
    createdAt: row.created_at,
  };
}
