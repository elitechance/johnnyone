/**
 * Shared fixture helper for authenticated worker tests after requireIdentity.
 * Presents a Bearer token and a verifyJwt mock that returns the intended
 * {sub, tenantId} so identity is verified, not spoofed via ctx.auth ids.
 */
export function authedCtx(opts: {
  tenantId: string;
  userId: string;
  token?: string;
  db?: any;
  env?: any;
  apiKey?: any;
  extra?: Record<string, unknown>;
}): any {
  const token = opts.token ?? 'dummy-jwt';
  const verifyJwt = async () => ({ sub: opts.userId, tenantId: opts.tenantId });
  const ctx: any = {
    db: opts.db ?? ({} as any),
    env: opts.env ?? { CHAT_RELAY_DO: {} as any },
    request: {
      headers: {
        get(k: string) {
          return String(k).toLowerCase() === 'authorization' ? `Bearer ${token}` : null;
        },
      },
    },
    auth: {
      tenantId: opts.tenantId,
      userId: opts.userId,
      isAuthenticated: true,
      verifyJwt,
    },
  };
  if (opts.apiKey !== undefined) ctx.apiKey = opts.apiKey;
  return Object.assign(ctx, opts.extra || {});
}
