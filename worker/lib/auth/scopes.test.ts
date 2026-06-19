import { describe, it, expect } from 'vitest';
import { requireScope, API_SCOPES, ForbiddenScopeError, isForbiddenScopeError } from './scopes';

describe('scopes guard', () => {
  it('JWT (no apiKey) is unrestricted for all scopes', () => {
    const ctx = { auth: { tenantId: 't', userId: 'u' } };
    for (const s of API_SCOPES) {
      expect(() => requireScope(ctx as any, s)).not.toThrow();
    }
  });

  it('key allow when scope present', () => {
    const ctx = { apiKey: { id: 'k', scopes: ['terminal:read', 'sessions:write'] } };
    expect(() => requireScope(ctx as any, 'terminal:read')).not.toThrow();
    expect(() => requireScope(ctx as any, 'sessions:write')).not.toThrow();
  });

  it('key deny throws FORBIDDEN_SCOPE naming the scope', () => {
    const ctx = { apiKey: { id: 'k', scopes: ['terminal:read'] } };
    try {
      requireScope(ctx as any, 'terminal:write');
      throw new Error('should have thrown');
    } catch (e) {
      expect(isForbiddenScopeError(e)).toBe(true);
      expect((e as ForbiddenScopeError).missing).toBe('terminal:write');
      expect((e as any).code).toBe('FORBIDDEN_SCOPE');
    }
  });

  it('empty key scopes deny everything', () => {
    const ctx = { apiKey: { id: 'k', scopes: [] } };
    expect(() => requireScope(ctx as any, 'plans:read')).toThrow(ForbiddenScopeError);
  });

  it('unknown scope is programming error', () => {
    const ctx = { auth: { tenantId: 't' } };
    expect(() => requireScope(ctx as any, 'foo:bar' as any)).toThrow(/unknown scope/);
  });
});
