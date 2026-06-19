import type { ResolverContext } from 'lokal-cf-worker';
import { generateApiKey, sha256Hex, authorizeForAltToken } from '../lib/auth/api-key';
import { API_SCOPES } from '../lib/auth/scopes';

interface CreateApiKeyInput {
  name: string;
  scopes: string[];
  expiresAt?: string | null;
}

export default async function createApiKey(
  _parent: unknown,
  args: { input: CreateApiKeyInput },
  ctx: ResolverContext & { apiKey?: { id: string; scopes: string[] } }
): Promise<{ apiKey: any; secret: string }> {
  await authorizeForAltToken(ctx);
  const auth = (ctx as any).auth || {};
  const apiKey = (ctx as any).apiKey;

  // JWT-only: API keys cannot mint keys (no escalation)
  if (apiKey) {
    throw new Error('API keys cannot manage API keys');
  }
  if (!auth.userId || !auth.tenantId) {
    throw new Error('Not authenticated');
  }

  const { name, scopes, expiresAt } = args.input || ({} as any);
  if (!name || typeof name !== 'string' || !name.trim()) {
    throw new Error('name is required');
  }
  if (!Array.isArray(scopes) || scopes.length === 0) {
    throw new Error('at least one scope is required');
  }
  const invalid = scopes.filter((s) => !API_SCOPES.includes(s as any));
  if (invalid.length) {
    throw new Error(`invalid scope(s): ${invalid.join(', ')}`);
  }

  let normalizedExpires: string | null = null;
  if (expiresAt != null && expiresAt !== '') {
    const expDate = new Date(expiresAt);
    if (isNaN(expDate.getTime())) {
      throw new Error('expiresAt must be a valid ISO-8601 date string');
    }
    if (expDate.getTime() <= Date.now()) {
      throw new Error('expiresAt must be a future date');
    }
    normalizedExpires = expDate.toISOString();
  }

  const g = generateApiKey();
  const keyHash = await sha256Hex(g.secret);

  const now = new Date().toISOString();
  const id = g.keyId;

  await (ctx.db as any)
    .prepare(
      `INSERT INTO api_keys (id, tenant_id, user_id, name, key_hash, key_prefix, scopes, expires_at, created_at, updated_at, is_deleted)
       VALUES (?, ?, ?, ?, ?, ?, ?, ?, ?, ?, 0)`
    )
    .bind(
      id,
      auth.tenantId,
      auth.userId,
      name.trim(),
      keyHash,
      g.keyPrefix,
      JSON.stringify(scopes),
      normalizedExpires,
      now,
      now
    )
    .run();

  const apiKeyOut = {
    id,
    name: name.trim(),
    keyPrefix: g.keyPrefix,
    scopes,
    lastUsedAt: null,
    expiresAt: normalizedExpires,
    revokedAt: null,
    createdAt: now,
  };

  return { apiKey: apiKeyOut, secret: g.token };
}
