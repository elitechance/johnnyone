/**
 * token.ts — sign / verify the action-link JWT (HS256). Pure apart from Web Crypto.
 *
 * The token is a bearer credential for ONE allowlisted action, valid for 120 s and redeemable
 * once (the once-ness lives in D1, see action-links.ts). This module only answers "is this a
 * token we signed, for us, still in its window, naming an allowlisted action?".
 *
 * Compactness: a Discord link button's URL is capped at 512 characters, and with `args` (two
 * UUID-sized ids or a 200-character reply) plus a 120-character label inside it a token does not
 * fit. So the token carries the identity of the link — iss, aud, iat, exp, jti, tenant, sub, act —
 * and the worker stores the link's `args` and `label` in D1 (`action_link_mints`) under the jti at
 * mint time. The signature binds the jti; the mint row is written only by the worker; preview and
 * redeem require the row to match the token's tenant, user, act and exp. A link is ~405
 * characters.
 *
 * Hardening, each pinned by a test in token.test.ts:
 *  - the header must be exactly alg=HS256 / typ=JWT: no `none`, no algorithm confusion;
 *  - the MAC is checked with crypto.subtle.verify (constant time) BEFORE the payload is parsed;
 *  - aud and iss must match; exp is enforced server-side with a ±5 s skew; iat may not be in
 *    the future beyond the skew; and exp - iat may not exceed the 120 s TTL, so even a token
 *    signed with the right key cannot be given a longer life;
 *  - act is re-validated against the allowlist (actions.ts); args are re-validated from the mint
 *    row by action-links.ts;
 *  - the secret must be at least 32 characters, otherwise signing AND verifying fail closed.
 */
import { isActionKind, type ActionKind } from './actions';

export const ACTION_LINK_AUD = 'j1-action';
export const ACTION_LINK_ISS = 'j1';
export const ACTION_LINK_TTL_S = 120;
export const ACTION_LINK_SKEW_S = 5;
/** A token longer than this is refused before any crypto (it rides in a URL). */
export const ACTION_LINK_MAX_LEN = 2048;
export const MIN_SECRET_LEN = 32;

export interface ActionClaims {
  iss: string;
  aud: string;
  iat: number;
  exp: number;
  jti: string;
  tenant: string;
  /** The minting user: the redeem relays to THIS user's desktop node. */
  sub: string;
  act: ActionKind;
}

export type VerifyCode =
  | 'malformed'
  | 'bad_header'
  | 'bad_signature'
  | 'wrong_aud'
  | 'wrong_iss'
  | 'bad_claims'
  | 'not_allowlisted'
  | 'expired'
  | 'not_yet_valid';

export type VerifyResult =
  | { ok: true; claims: ActionClaims }
  /** `claims` is set only when the signature verified (expired / not_yet_valid). */
  | { ok: false; code: VerifyCode; claims?: ActionClaims };

const UUID_RE = /^[0-9a-f]{8}-[0-9a-f]{4}-[0-9a-f]{4}-[0-9a-f]{4}-[0-9a-f]{12}$/;
const HEADER = { alg: 'HS256', typ: 'JWT' };

export function base64UrlEncode(bytes: Uint8Array): string {
  let bin = '';
  for (let i = 0; i < bytes.length; i++) bin += String.fromCharCode(bytes[i]);
  return btoa(bin).replace(/\+/g, '-').replace(/\//g, '_').replace(/=+$/, '');
}

export function base64UrlDecode(s: string): Uint8Array | null {
  if (!/^[A-Za-z0-9_-]*$/.test(s)) return null;
  const pad = s.length % 4 === 0 ? '' : '='.repeat(4 - (s.length % 4));
  try {
    const bin = atob(s.replace(/-/g, '+').replace(/_/g, '/') + pad);
    const out = new Uint8Array(bin.length);
    for (let i = 0; i < bin.length; i++) out[i] = bin.charCodeAt(i);
    return out;
  } catch {
    return null;
  }
}

function utf8(s: string): Uint8Array {
  return new TextEncoder().encode(s);
}

function jsonB64(v: unknown): string {
  return base64UrlEncode(utf8(JSON.stringify(v)));
}

function parseJsonB64(s: string): unknown {
  const bytes = base64UrlDecode(s);
  if (!bytes) return undefined;
  try {
    return JSON.parse(new TextDecoder('utf-8', { fatal: true, ignoreBOM: false }).decode(bytes));
  } catch {
    return undefined;
  }
}

export class ActionLinkConfigError extends Error {
  constructor() {
    super('Action links are not configured on this worker');
  }
}

async function hmacKey(secret: string | undefined, usage: 'sign' | 'verify'): Promise<CryptoKey> {
  if (typeof secret !== 'string' || secret.length < MIN_SECRET_LEN) throw new ActionLinkConfigError();
  return crypto.subtle.importKey('raw', utf8(secret), { name: 'HMAC', hash: 'SHA-256' }, false, [usage]);
}

export async function signActionToken(claims: ActionClaims, secret: string | undefined): Promise<string> {
  const key = await hmacKey(secret, 'sign');
  const signingInput = `${jsonB64(HEADER)}.${jsonB64(claims)}`;
  const sig = new Uint8Array(await crypto.subtle.sign('HMAC', key, utf8(signingInput)));
  return `${signingInput}.${base64UrlEncode(sig)}`;
}

/** Shape-check a decoded payload. Returns typed claims or null. */
function asClaims(p: unknown): ActionClaims | null {
  if (!p || typeof p !== 'object' || Array.isArray(p)) return null;
  const c = p as Record<string, unknown>;
  const str = (k: string) => typeof c[k] === 'string' && (c[k] as string).length > 0 && (c[k] as string).length <= 256;
  const int = (k: string) => typeof c[k] === 'number' && Number.isSafeInteger(c[k]);
  if (!str('iss') || !str('aud') || !str('jti') || !str('tenant') || !str('sub') || !str('act')) return null;
  if (!int('iat') || !int('exp')) return null;
  return c as unknown as ActionClaims;
}

/**
 * Verify a presented token. Never throws for a bad token (only for a missing secret, which is a
 * deploy error and must not look like "your link is invalid").
 */
export async function verifyActionToken(
  token: unknown,
  secret: string | undefined,
  nowS: number,
): Promise<VerifyResult> {
  const key = await hmacKey(secret, 'verify');
  if (typeof token !== 'string' || token.length === 0 || token.length > ACTION_LINK_MAX_LEN) {
    return { ok: false, code: 'malformed' };
  }
  const parts = token.split('.');
  if (parts.length !== 3) return { ok: false, code: 'malformed' };
  const [h, p, s] = parts;

  const header = parseJsonB64(h) as Record<string, unknown> | undefined;
  if (
    !header ||
    typeof header !== 'object' ||
    header.alg !== HEADER.alg ||
    header.typ !== HEADER.typ ||
    Object.keys(header).length !== 2
  ) {
    return { ok: false, code: 'bad_header' };
  }

  const sig = base64UrlDecode(s);
  if (!sig || sig.length !== 32) return { ok: false, code: 'bad_signature' };
  const valid = await crypto.subtle.verify('HMAC', key, sig, utf8(`${h}.${p}`));
  if (!valid) return { ok: false, code: 'bad_signature' };

  const claims = asClaims(parseJsonB64(p));
  if (!claims) return { ok: false, code: 'bad_claims' };
  if (claims.aud !== ACTION_LINK_AUD) return { ok: false, code: 'wrong_aud' };
  if (claims.iss !== ACTION_LINK_ISS) return { ok: false, code: 'wrong_iss' };
  if (!UUID_RE.test(claims.jti)) return { ok: false, code: 'bad_claims' };
  if (claims.exp <= claims.iat || claims.exp - claims.iat > ACTION_LINK_TTL_S) {
    return { ok: false, code: 'bad_claims' };
  }
  if (!isActionKind(claims.act)) return { ok: false, code: 'not_allowlisted' };

  if (claims.iat > nowS + ACTION_LINK_SKEW_S) return { ok: false, code: 'not_yet_valid', claims };
  if (nowS > claims.exp + ACTION_LINK_SKEW_S) return { ok: false, code: 'expired', claims };
  return { ok: true, claims };
}

/** The `jti` only, for logs and audit rows. Never log the token itself. */
export function jtiForLog(claims: { jti?: string } | undefined): string | null {
  return claims?.jti && UUID_RE.test(claims.jti) ? claims.jti : null;
}
