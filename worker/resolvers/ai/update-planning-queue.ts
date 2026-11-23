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

// Plans tab (initiative 8ad3ae71, phase 01): Queue Start planning after the conflicting initiatives (null clears the queue).
// Thin pass-through to the host `update_planning_queue` over the relay, gated by the plans:write scope.
export default async function updatePlanningQueue(
  _parent: unknown,
  args: { initiativeId: string; afterInitiativeIds?: string[] | null },
  ctx: ResolverContext,
) {
  await authorizeForAltToken(ctx, 'plans:write');
  return desktopRpc<unknown>(ctx, 'update_planning_queue', { initiativeId: args.initiativeId, afterInitiativeIds: args.afterInitiativeIds ?? null });
}
