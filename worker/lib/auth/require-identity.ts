/**
 * require-identity.ts — the single JohnnyOne choke point that may return
 * a non-nullable {tenantId, userId}. Present-but-invalid and absent
 * credentials throw GraphQLError UNAUTHENTICATED. Never an empty identity.
 *
 * JWT ids come from the verified payload (D3/D7), never ctx.auth.tenantId
 * on HTTP. graphql-ws (empty Request, already-verified auth) is D13.
 */
import { GraphQLError } from 'graphql';
import { isTokenLike, updateLastUsed, validateApiKeyToken } from './api-key';

export interface Identity {
  tenantId: string;
  userId: string;
}

export class UnauthenticatedError extends GraphQLError {
  constructor() {
    super('UNAUTHENTICATED', { extensions: { code: 'UNAUTHENTICATED' } });
  }
}

const IDENTITY_MEMO = Symbol.for('johnnyone.requireIdentity');

function unauthenticated(): UnauthenticatedError {
  return new UnauthenticatedError();
}

function nonempty(v: unknown): v is string {
  return typeof v === 'string' && v.length > 0;
}

function headerValue(
  headers: { get?(k: string): string | null } | Record<string, unknown> | undefined,
  name: string,
): string | null {
  if (!headers) return null;
  if (typeof headers.get === 'function') {
    const viaGet = headers.get(name) ?? headers.get(name.toLowerCase());
    if (viaGet != null) return viaGet;
  }
  const rec = headers as Record<string, unknown>;
  const raw = rec[name] ?? rec[name.toLowerCase()];
  return raw == null ? null : String(raw);
}

/** True when the request carries any Authorization header (D13 harden). */
function requestHasAuthorization(ctx: {
  request?: { headers?: { get?(k: string): string | null } };
}): boolean {
  const v = headerValue(ctx?.request?.headers, 'Authorization');
  return v != null && String(v).length > 0;
}

/**
 * Authorization header only (D8). Strips an optional leading `Bearer `
 * case-insensitively — same parse as buildAuthContext / api-key.ts:174.
 * Does not read `?token=` or x-user-id / x-tenant-id.
 */
export function extractPresentedToken(ctx: {
  request?: { headers?: { get?(k: string): string | null } };
}): string {
  const raw = headerValue(ctx?.request?.headers, 'Authorization') || '';
  return raw.replace(/^Bearer\s+/i, '').trim();
}

export async function requireIdentity(ctx: any): Promise<Identity> {
  if (ctx && ctx[IDENTITY_MEMO]) return ctx[IDENTITY_MEMO];

  const token = extractPresentedToken(ctx);

  if (token && isTokenLike(token)) {
    if (!ctx?.db) throw unauthenticated();
    const v = await validateApiKeyToken(token, ctx.db);
    if (!v) throw unauthenticated();
    ctx.auth = {
      ...(ctx.auth || {}),
      userId: v.userId,
      tenantId: v.tenantId,
      isAuthenticated: true,
    };
    ctx.apiKey = v.apiKey;
    updateLastUsed(ctx.db, v.apiKey.id);
    const id: Identity = { tenantId: v.tenantId, userId: v.userId };
    ctx[IDENTITY_MEMO] = id;
    return id;
  }

  if (token) {
    const verify = ctx?.auth?.verifyJwt;
    if (typeof verify !== 'function') throw unauthenticated();
    let payload: any;
    try {
      payload = await verify(token);
    } catch {
      throw unauthenticated();
    }
    const userId = payload.sub || payload.userId || payload.uid || '';
    const tenantId = payload.tenantId || payload.tid || payload.tenant_id || '';
    if (!userId || !tenantId) throw unauthenticated();
    ctx.auth = { ...(ctx.auth || {}), userId, tenantId, isAuthenticated: true };
    const id: Identity = { tenantId, userId };
    ctx[IDENTITY_MEMO] = id;
    return id;
  }

  // D13 — graphql-ws only. Any Authorization header means this branch is
  // forbidden (do not trust ctx.auth ids after a parse miss).
  if (requestHasAuthorization(ctx)) throw unauthenticated();
  if (
    ctx?.auth?.isAuthenticated === true &&
    nonempty(ctx.auth.tenantId) &&
    nonempty(ctx.auth.userId)
  ) {
    const id: Identity = { tenantId: ctx.auth.tenantId, userId: ctx.auth.userId };
    ctx[IDENTITY_MEMO] = id;
    return id;
  }

  throw unauthenticated();
}
