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

// Plans tab (initiative 8ad3ae71, phase 01): Add a task under a draft-outline phase (new id NN/MM).
// Thin pass-through to the host `add_outline_task` over the relay, gated by the plans:write scope.
export default async function addOutlineTask(
  _parent: unknown,
  args: { initiativeId: string; phaseId: string; title: string },
  ctx: ResolverContext,
) {
  await authorizeForAltToken(ctx, 'plans:write');
  return desktopRpc<unknown>(ctx, 'add_outline_task', { initiativeId: args.initiativeId, phaseId: args.phaseId, title: args.title });
}
