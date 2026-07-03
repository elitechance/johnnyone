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

// Briefing loop (overhaul P4, D1/D4): thin pass-through to the host `accept_brief`, which flips the
// same Initiative briefing→planning and starts the planner. Gated by the plans:write scope.
export default async function acceptInitiativeBrief(
  _parent: unknown,
  args: { input: { initiativeId: string; finalBrief?: string | null } },
  ctx: ResolverContext,
) {
  await authorizeForAltToken(ctx, 'plans:write');
  return desktopRpc<unknown>(ctx, 'accept_brief', {
    initiativeId: args.input.initiativeId,
    finalBrief: args.input.finalBrief,
  });
}
