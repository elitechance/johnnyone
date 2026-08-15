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