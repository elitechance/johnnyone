/**
 * api-key.ts — pure primitives for jk_ M2M tokens (Phase 05).
 * No D1, no side effects. Used by create resolver and alt-token validators.
 *
 * Token format: jk_<keyId>_<secret>
 *   - keyId: url-safe id (16+ chars)
 *   - secret: high-entropy (>=128 bit)
 * Only the SHA-256 of secret is ever stored.
 */

import type { ApiScope } from './scopes';
import { requireScope } from './scopes';

const KEY_PREFIX = 'jk_';
const MIN_SECRET_BYTES = 16; // >=128 bits

export interface GeneratedKey {
  keyId: string;
  secret: string;
  token: string;
  keyPrefix: string;
}

export function isTokenLike(token: string): boolean {
  return typeof token === 'string' && token.startsWith(KEY_PREFIX);
}

export interface ValidatedKey {
  tenantId: string;
  userId: string;
  apiKey: { id: string; scopes: string[] };
}

/**
 * Generate a new API key.
 * secret from crypto.getRandomValues (Web Crypto, available in Workers).
 */
export function generateApiKey(): GeneratedKey {
  const keyId = generateId(16);
  const secretBytes = new Uint8Array(MIN_SECRET_BYTES);
  crypto.getRandomValues(secretBytes);
  const secret = bytesToHex(secretBytes); // hex is fine, 32 chars = 128 bits
  const token = `${KEY_PREFIX}${keyId}_${secret}`;
  const keyPrefix = `${KEY_PREFIX}${keyId}`;
  return { keyId, secret, token, keyPrefix };
}

/**
 * Parse a presented jk_ token. Returns parts or null on malformation.
 * Never throws.
 */
export function parseApiKey(token: string): { keyId: string; secret: string } | null {
  if (!token || typeof token !== 'string') return null;
  if (!token.startsWith(KEY_PREFIX)) return null;
  const rest = token.slice(KEY_PREFIX.length);
  const lastUnderscore = rest.lastIndexOf('_');
  if (lastUnderscore <= 0) return null;
  const keyId = rest.slice(0, lastUnderscore);
  const secret = rest.slice(lastUnderscore + 1);
  if (!keyId || !secret) return null;
  return { keyId, secret };
}

/** SHA-256 hex via Web Crypto (async for subtle). */
export async function sha256Hex(secret: string): Promise<string> {
  const data = new TextEncoder().encode(secret);
  const digest = await crypto.subtle.digest('SHA-256', data);
  return bytesToHex(new Uint8Array(digest));
}

/**
 * Constant-time compare of two hex strings.
 * - Normalizes and requires even-length valid hex; otherwise unequal.
 * - Compares full length (padded if needed).
 * - Accumulates mismatches; no early return on first diff.
 */
export function timingSafeEqualHex(a: string, b: string): boolean {
  if (typeof a !== 'string' || typeof b !== 'string') return false;
  const aBuf = hexToBytes(a);
  const bBuf = hexToBytes(b);
  if (aBuf.length === 0 && a.length > 0) return false; // invalid hex input
  if (bBuf.length === 0 && b.length > 0) return false;
  const len = Math.max(aBuf.length, bBuf.length);
  let result = 0;
  for (let i = 0; i < len; i++) {
    const va = i < aBuf.length ? aBuf[i] : 0;
    const vb = i < bBuf.length ? bBuf[i] : 0;
    result |= va ^ vb;
  }
  result |= (aBuf.length ^ bBuf.length);
  return result === 0;
}

function generateId(len: number): string {
  // Use hex only (0-9a-f) — underscore-free, safe delimiter.
  const bytes = new Uint8Array(Math.ceil(len / 2));
  crypto.getRandomValues(bytes);
  let s = bytesToHex(bytes);
  if (s.length > len) s = s.slice(0, len);
  return s;
}

function bytesToHex(bytes: Uint8Array): string {
  return Array.from(bytes, b => b.toString(16).padStart(2, '0')).join('');
}

function hexToBytes(hex: string): Uint8Array {
  const clean = hex.replace(/\s+/g, '');
  if (clean.length % 2 !== 0) return new Uint8Array(0);
  const out = new Uint8Array(clean.length / 2);
  for (let i = 0; i < out.length; i++) {
    out[i] = parseInt(clean.substr(i * 2, 2), 16);
  }
  return out;
}

/**
 * Validate a jk_ token against D1 api_keys row.
 * Returns tenant/user + apiKey info or null.
 * Caller must have already done isTokenLike check if desired.
 * Never logs secret.
 */
export async function validateApiKeyToken(
  token: string,
  db: D1Database
): Promise<ValidatedKey | null> {
  const parsed = parseApiKey(token);
  if (!parsed) return null;
  const { keyId, secret } = parsed;

  const row = await db
    .prepare(
      `SELECT id, tenant_id, user_id, key_hash, scopes, revoked_at, expires_at, is_deleted
       FROM api_keys
       WHERE id = ? AND is_deleted = 0`
    )
    .bind(keyId)
    .first<any>();

  if (!row) return null;
  if (row.revoked_at) return null;
  if (row.expires_at) {
    const exp = Date.parse(row.expires_at);
    if (isNaN(exp) || exp < Date.now()) return null;
  }

  const candidateHash = await sha256Hex(secret);
  if (!timingSafeEqualHex(candidateHash, row.key_hash || '')) return null;

  const scopes: string[] = (() => { try { return JSON.parse(row.scopes || '[]'); } catch { return []; } })();

  return {
    tenantId: row.tenant_id,
    userId: row.user_id,
    apiKey: { id: row.id, scopes },
  };
}

/**
 * Populate ctx.auth + ctx.apiKey from a jk_ Bearer token if present in ctx.request.
 * Safe no-op if already authenticated or not a jk_ token.
 * Used to extend the GraphQL auth path for alt tokens (resolvers see the populated ctx).
 */
export async function applyAltTokenToContext(ctx: any): Promise<void> {
  try {
    const auth = ctx && ctx.auth;
    if (auth && auth.userId) return; // already set by platform JWT
    const req = ctx && ctx.request;
    if (!req || !req.headers) return;
    let token = '';
    if (typeof req.headers.get === 'function') {
      token = req.headers.get('Authorization') || '';
    } else if (req.headers.Authorization) {
      token = String(req.headers.Authorization);
    }
    token = token.replace(/^Bearer\s+/i, '').trim();
    if (!isTokenLike(token)) return;
    const db = ctx.db;
    if (!db) return;
    const v = await validateApiKeyToken(token, db);
    if (v) {
      ctx.auth = { userId: v.userId, tenantId: v.tenantId, isAuthenticated: true };
      (ctx as any).apiKey = v.apiKey;
      updateLastUsed(db, v.apiKey.id); // best-effort
    }
  } catch {
    // never throw from auth fallback
  }
}

/**
 * Central helper for alt-token (jk_) auth + optional scope enforcement.
 * Call at start of resolvers that support API key access.
 * For key-management resolvers, use apply + manual reject instead.
 */
export async function authorizeForAltToken(ctx: any, scope?: ApiScope): Promise<void> {
  await applyAltTokenToContext(ctx);
  if (scope) {
    requireScope(ctx as any, scope);
  }
}

/** Best-effort last_used_at update (shared by GraphQL and WSS paths). */
export function updateLastUsed(db: D1Database, keyId: string): void {
  try {
    db.prepare('UPDATE api_keys SET last_used_at = ? WHERE id = ?')
      .bind(new Date().toISOString(), keyId)
      .run().catch(() => {});
  } catch {}
}
