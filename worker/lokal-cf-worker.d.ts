// Ambient type shim for the `lokal-cf-worker` virtual module.
//
// The lokal builtin modules (e.g. modules/auth/auth-middleware) import their
// framework types from the bare specifier `lokal-cf-worker`, which only exists
// inside lokal's GENERATED worker build dir (it is injected there per
// cli/cloudflare/worker.ts) — it is NOT installed in this source tree's
// node_modules. The real lokal build resolves it; the standalone
// `tsc -p worker/tsconfig.json --noEmit` typecheck (lokal.yaml builds.edge)
// runs against the source tree and therefore cannot. This shim provides the
// minimal type surface the imported builtin source needs so the standalone
// typecheck passes without pulling lokal's full generated context.
declare module 'lokal-cf-worker' {
  export interface JwtPayload {
    sub: string;
    tenantId: string;
    userId?: string;
    uid?: string;
    tid?: string;
    tenant_id?: string;
    iat?: number;
    exp?: number;
    [claim: string]: unknown;
  }

  export interface AuthContext {
    userId: string | null;
    tenantId: string | null;
    verifyJwt: (token: string) => Promise<JwtPayload>;
    signJwt: (payload: JwtPayload) => Promise<string>;
    [key: string]: unknown;
  }

  // The real D1Database comes from @cloudflare/workers-types at build time;
  // a structural alias is sufficient for the standalone typecheck.
  export type D1Database = import('@cloudflare/workers-types').D1Database;

  export interface ResolverContext {
    db: D1Database;
    auth: AuthContext;
    [key: string]: unknown;
  }
}
