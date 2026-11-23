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

// Plans tab (initiative 8ad3ae71, phase 01): Reorder an outline item within its parent (ids never change).
// Thin pass-through to the host `update_outline_item_position` over the relay, gated by the plans:write scope.
export default async function updateOutlineItemPosition(
  _parent: unknown,
  args: { initiativeId: string; id: string; toIndex: number },
  ctx: ResolverContext,
) {
  await authorizeForAltToken(ctx, 'plans:write');
  return desktopRpc<unknown>(ctx, 'update_outline_item_position', { initiativeId: args.initiativeId, id: args.id, toIndex: args.toIndex });
}
