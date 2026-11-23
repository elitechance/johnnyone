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

// Plans tab (initiative 8ad3ae71, phase 01): The brief check of the latest version (same verdict the agent and Start planning use).
// Thin pass-through to the host `get_brief_check` over the relay, gated by the plans:read scope.
export default async function getBriefCheck(
  _parent: unknown,
  args: { initiativeId: string },
  ctx: ResolverContext,
) {
  await authorizeForAltToken(ctx, 'plans:read');
  return desktopRpc<unknown>(ctx, 'get_brief_check', { initiativeId: args.initiativeId });
}
