import type { RelayRpcAuth } from './relay-rpc';

interface CacheEntry {
  expiresAt: number;
  data: unknown;
}

const cache = new Map<string, CacheEntry>();

const CACHE_TTL_MS: Record<string, number> = {
  list_sessions: 5_000,
  get_setting: 5_000,
  detect_cli_tools: 30_000,
  get_agent_plan: 3_000,
  list_agent_plans: 5_000,
};

/**
 * Which cached READ methods a WRITE method makes stale. Without this, archiving a session left the
 * `list_sessions` entry in place for up to its 5s TTL, so the very next read returned the rows that
 * were just archived — on a phone that is the whole round trip, because `GraphQLClient.mutate`
 * always goes to the worker while `listSessions` only prefers the local host on localhost. The user
 * taps Clear, the same rows come back, and it reads as "the button did nothing".
 *
 * IMPORTANT, and the reason the UI does not rely on this alone: `cache` is a module-level `Map`, so
 * it is PER-ISOLATE. A Cloudflare worker runs many isolates, and invalidating in the isolate that
 * handled the write does nothing for a read served by another one. This fixes the common
 * same-isolate case and is strictly correct; the `/shells` page additionally drops cleared ids from
 * its local list so the tap is right whatever the worker serves.
 *
 * Keyed by the host RPC method name (`archive_session`, …), not the GraphQL field.
 */
const INVALIDATED_READS: Record<string, string[]> = {
  archive_session: ['list_sessions'],
  delete_session: ['list_sessions'],
  create_session: ['list_sessions'],
  update_session_title: ['list_sessions'],
  update_session_working_directory: ['list_sessions'],
  update_session_provider: ['list_sessions'],
};

/** The cached read methods `method` invalidates — `[]` for a read or an unrelated write. */
export function invalidatedReadsFor(method: string): string[] {
  return INVALIDATED_READS[method] ?? [];
}

/**
 * Drop every cached entry for the reads `method` makes stale, for THIS identity only. All parameter
 * variants of each read go (the key embeds the params, so `status:'active'` and `status:null` are
 * separate entries and a write staled both) — invalidating the whole method beats guessing the one
 * key the caller happened to populate.
 */
export function invalidateCachedRpc(auth: RelayRpcAuth, method: string): void {
  const reads = invalidatedReadsFor(method);
  if (reads.length === 0) return;
  for (const read of reads) {
    const prefix = `${auth.tenantId}:${auth.userId}:${read}:`;
    for (const key of cache.keys()) {
      if (key.startsWith(prefix)) cache.delete(key);
    }
  }
}

export function isReadRpcCacheable(method: string): boolean {
  return Object.prototype.hasOwnProperty.call(CACHE_TTL_MS, method);
}

function cacheKey(
  auth: RelayRpcAuth,
  method: string,
  params: Record<string, unknown>,
): string {
  return `${auth.tenantId}:${auth.userId}:${method}:${stableParams(params)}`;
}

function stableParams(params: Record<string, unknown>): string {
  const keys = Object.keys(params).sort();
  if (keys.length === 0) return '';
  return JSON.stringify(keys.map((key) => [key, params[key]]));
}

export function readCachedRpc<T>(
  auth: RelayRpcAuth,
  method: string,
  params: Record<string, unknown>,
): T | null {
  const ttl = CACHE_TTL_MS[method];
  if (!ttl) return null;

  const entry = cache.get(cacheKey(auth, method, params));
  if (!entry || entry.expiresAt <= Date.now()) {
    if (entry) cache.delete(cacheKey(auth, method, params));
    return null;
  }

  return entry.data as T;
}

export function writeCachedRpc(
  auth: RelayRpcAuth,
  method: string,
  params: Record<string, unknown>,
  data: unknown,
): void {
  const ttl = CACHE_TTL_MS[method];
  if (!ttl) return;

  cache.set(cacheKey(auth, method, params), {
    data,
    expiresAt: Date.now() + ttl,
  });
}