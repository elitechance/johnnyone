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

// Plans tab (initiative 8ad3ae71, phase 01): Delete an outline item (a phase takes its tasks with it).
// Thin pass-through to the host `delete_outline_item` over the relay, gated by the plans:write scope.
export default async function deleteOutlineItem(
  _parent: unknown,
  args: { initiativeId: string; id: string },
  ctx: ResolverContext,
) {
  await authorizeForAltToken(ctx, 'plans:write');
  return desktopRpc<unknown>(ctx, 'delete_outline_item', { initiativeId: args.initiativeId, id: args.id });
}
