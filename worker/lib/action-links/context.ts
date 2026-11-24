/**
 * context.ts — wire the action-link core (action-links.ts) to a resolver's ctx.
 *
 * Why the relay here is NOT `desktopRpc`: desktopRpc takes its identity from requireIdentity(ctx),
 * i.e. from the request's session credential. A redeem has none, by design (a phone tap with no
 * login); its identity is the minter's {tenant, sub} from the VERIFIED token. Memoising that onto
 * ctx so desktopRpc would accept it is exactly what this module must never do: GraphQL executes
 * every field of one document against the same ctx, so `mutation { runActionLink(...)
 * deleteAgentPlan(...) }` would then run the second field as the minter. The identity is instead
 * passed explicitly, to this one relay call, and ctx.auth is never read or written.
 */
import { resolveOnlineNode } from '../auth/resolve-online-node';
import { DEFAULT_ACTION_LINK_BASE_URL, type ActionLinkDeps, type ActionRelay } from './action-links';

export interface ActionLinkEnv {
  CHAT_RELAY_DO: DurableObjectNamespace;
  /** HS256 key for action links. A wrangler secret, never in git (lokal.yaml `secrets:`). */
  ACTION_LINK_SECRET?: string;
  /** Web origin that serves /act. Defaults to the production Pages URL. */
  ACTION_LINK_BASE_URL?: string;
  [key: string]: unknown;
}

export interface ActionLinkCtx {
  db: D1Database;
  env: ActionLinkEnv;
  request?: Request;
  waitUntil?: (p: Promise<unknown>) => void;
  [key: string]: unknown;
}

export function relayForIdentity(db: D1Database, env: ActionLinkEnv): ActionRelay {
  return async (identity, method, params) => {
    const node = await resolveOnlineNode(db, identity);
    if (!node) throw new Error('No online backend app found');
    const stub = env.CHAT_RELAY_DO.get(env.CHAT_RELAY_DO.idFromName(node.id));
    const response = await stub.fetch('https://internal/relay-rpc', {
      method: 'POST',
      headers: { 'Content-Type': 'application/json' },
      body: JSON.stringify({ method, params }),
    });
    const result = (await response.json()) as { success?: boolean; data?: unknown; error?: string; timedOut?: boolean };
    if (!response.ok || result.timedOut || result.success === false) {
      throw new Error(result.timedOut ? 'timed out' : result.error || `relay failed: ${method}`);
    }
    return result.data;
  };
}

export function depsFromCtx(ctx: ActionLinkCtx): ActionLinkDeps {
  const waitUntil = typeof ctx.waitUntil === 'function' ? ctx.waitUntil : undefined;
  return {
    db: ctx.db,
    secret: ctx.env?.ACTION_LINK_SECRET,
    now: () => Math.floor(Date.now() / 1000),
    relay: relayForIdentity(ctx.db, ctx.env),
    baseUrl: ctx.env?.ACTION_LINK_BASE_URL || DEFAULT_ACTION_LINK_BASE_URL,
    defer: waitUntil ? (p) => waitUntil(p) : undefined,
  };
}

export function requestTenant(ctx: ActionLinkCtx): string | null {
  const v = ctx.request?.headers?.get?.('x-tenant-id');
  return v && v.trim() ? v.trim() : null;
}

export function requestMethod(ctx: ActionLinkCtx): string | null {
  const m = ctx.request?.method;
  return typeof m === 'string' ? m.toUpperCase() : null;
}
