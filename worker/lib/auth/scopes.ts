/**
 * scopes.ts — requireScope guard + canonical scope list (Phase 05).
 * JWT callers: unrestricted.
 * API-key callers: must have the requested scope; else FORBIDDEN_SCOPE.
 */

export const API_SCOPES = [
  'terminal:read',
  'terminal:write',
  'plans:read',
  'plans:write',
  'sessions:read',
  'sessions:write',
] as const;

export type ApiScope = (typeof API_SCOPES)[number];

export interface ApiKeyCtx {
  id: string;
  scopes: string[];
}

export interface ResolverCtxWithAuth {
  auth?: { userId?: string | null; tenantId?: string | null };
  apiKey?: ApiKeyCtx;
  [k: string]: any;
}

export class ForbiddenScopeError extends Error {
  code = 'FORBIDDEN_SCOPE';
  constructor(public readonly missing: string) {
    super(`missing required scope: ${missing}`);
  }
}

export function isForbiddenScopeError(e: unknown): e is ForbiddenScopeError {
  return !!e && typeof e === 'object' && (e as any).code === 'FORBIDDEN_SCOPE';
}

/**
 * requireScope(ctx, scope)
 * - If no apiKey in ctx → JWT path → allow (unrestricted)
 * - If apiKey present → allow only if scope in apiKey.scopes
 * Throws ForbiddenScopeError (with .code) on deny.
 */
export function requireScope(ctx: ResolverCtxWithAuth, scope: ApiScope): void {
  if (!API_SCOPES.includes(scope as any)) {
    // programming error surfaced clearly
    throw new Error(`unknown scope: ${scope}`);
  }
  const apiKey = ctx && (ctx as any).apiKey;
  if (!apiKey) {
    // JWT or unauth (unauth will be caught earlier by other guards)
    return;
  }
  const have = Array.isArray(apiKey.scopes) ? apiKey.scopes : [];
  if (!have.includes(scope)) {
    throw new ForbiddenScopeError(scope);
  }
}
