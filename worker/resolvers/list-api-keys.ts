import type { ResolverContext } from 'lokal-cf-worker';
import { authorizeForAltToken } from '../lib/auth/api-key';

export default async function listApiKeys(
  _parent: unknown,
  _args: unknown,
  ctx: ResolverContext & { apiKey?: { id: string; scopes: string[] } }
): Promise<any[]> {
  await authorizeForAltToken(ctx);
  const auth = (ctx as any).auth || {};
  const apiKey = (ctx as any).apiKey;

  if (apiKey) {
    throw new Error('API keys cannot manage API keys');
  }
  if (!auth.userId || !auth.tenantId) {
    throw new Error('Not authenticated');
  }

  const rows = await (ctx.db as any)
    .prepare(
      `SELECT id, name, key_prefix, scopes, last_used_at, expires_at, revoked_at, created_at
       FROM api_keys
       WHERE tenant_id = ? AND user_id = ? AND is_deleted = 0
       ORDER BY created_at DESC`
    )
    .bind(auth.tenantId, auth.userId)
    .all();

  return (rows.results || []).map((r: any) => ({
    id: r.id,
    name: r.name,
    keyPrefix: r.key_prefix,
    scopes: JSON.parse(r.scopes || '[]'),
    lastUsedAt: r.last_used_at || null,
    expiresAt: r.expires_at || null,
    revokedAt: r.revoked_at || null,
    createdAt: r.created_at,
  }));
}
