import type { ResolverContext } from 'lokal-cf-worker';
import { authorizeForAltToken } from '../lib/auth/api-key';

/**
 * deleteApiKey(id) — hard-remove a key from the management surface.
 *
 * Sets is_deleted = 1 (every read path already filters is_deleted = 0), so the
 * key disappears from listApiKeys and can never authenticate again. JWT-only,
 * scoped to the caller's own tenant+user. Idempotent: returns true even if the
 * key was already gone.
 */
export default async function deleteApiKey(
  _parent: unknown,
  args: { id: string },
  ctx: ResolverContext & { apiKey?: { id: string; scopes: string[] } }
): Promise<boolean> {
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

  // Scoped soft-delete: only the caller's own tenant+user key.
  await (ctx.db as any)
    .prepare(
      `UPDATE api_keys
       SET is_deleted = 1, updated_at = ?
       WHERE id = ? AND tenant_id = ? AND user_id = ?`
    )
    .bind(now, id, auth.tenantId, auth.userId)
    .run();

  return true;
}
