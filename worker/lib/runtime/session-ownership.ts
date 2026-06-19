import { relayRpc, RelayRpcContext } from './relay-rpc';

/**
 * verifySessionOwnership
 * Returns true if `sessionId` belongs to the authenticated `{tenantId, userId}`.
 * Resolution is performed by relay-rpc to the desktop (authoritative owner).
 * "Not owned" and "does not exist" both return false (no existence leak).
 * Transport failures (no node, timeout) are re-thrown so the caller can fail-closed.
 *
 * Fronted by short per-(tenant,user,session) cache (Task 05) to honor DO throttle.
 */

const ownershipCache = new Map<string, { expiresAt: number; owned: boolean }>();
const OWNERSHIP_CACHE_TTL_MS = 5_000;
const NEGATIVE_TTL_MS = 500; // short/zero-ish per D-cache-negative to not deny fresh sessions long, while bounding probes

function ownCacheKey(tenantId: string, userId: string, sessionId: string): string {
  return `${tenantId}:${userId}:${sessionId}`;
}

export function invalidateSessionOwnership(sessionId: string): void {
  for (const k of Array.from(ownershipCache.keys())) {
    if (k.endsWith(':' + sessionId)) ownershipCache.delete(k);
  }
}

export async function verifySessionOwnership(
  ctx: RelayRpcContext,
  sessionId: string
): Promise<boolean> {
  if (!sessionId || typeof sessionId !== 'string' || sessionId.trim() === '') {
    return false;
  }

  const key = ownCacheKey(ctx.auth.tenantId, ctx.auth.userId, sessionId);
  const entry = ownershipCache.get(key);
  if (entry && entry.expiresAt > Date.now()) {
    return entry.owned;
  }

  let owned = false;
  let isTransport = false;
  try {
    const data = await relayRpc<any>(ctx, 'get_session', { id: sessionId });
    owned = !!data && !!data.id; // structured success from scoped desktop = owned
  } catch (err: any) {
    const msg = err && err.message ? String(err.message) : String(err);
    if (
      msg.includes('No online desktop node') ||
      msg.includes('Desktop RPC timed out') ||
      msg.includes('Desktop not connected')
    ) {
      isTransport = true;
      throw err;
    }
    owned = false;
  }

  if (!isTransport) {
    const ttl = owned ? OWNERSHIP_CACHE_TTL_MS : NEGATIVE_TTL_MS;
    ownershipCache.set(key, { owned, expiresAt: Date.now() + ttl });
  }
  return owned;
}
