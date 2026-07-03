import { desktopRpc } from '../../lib/runtime/desktop-rpc';
import { authorizeForAltToken } from '../../lib/auth/api-key';

interface ResolverContext {
  db: D1Database;
  env: WorkerEnv;
  auth: { userId: string; tenantId: string };
  request?: Request;
  [key: string]: unknown;
}

interface WorkerEnv {
  CHAT_RELAY_DO: DurableObjectNamespace;
  [key: string]: unknown;
}

// Briefing loop (overhaul P4, D1): thin pass-through to the host `create_briefing_run`. No FS/DB
// logic here — the host owns behavior; the worker relays, gated by the plans:write scope.
export default async function createBriefingInitiative(
  _parent: unknown,
  args: { input: unknown },
  ctx: ResolverContext,
) {
  await authorizeForAltToken(ctx, 'plans:write');
  return desktopRpc<unknown>(ctx, 'create_briefing_run', { input: args.input });
}
