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

// Plans tab (initiative 8ad3ae71, phase 01): Rename a draft-outline phase or task.
// Thin pass-through to the host `update_outline_item` over the relay, gated by the plans:write scope.
export default async function updateOutlineItem(
  _parent: unknown,
  args: { initiativeId: string; id: string; title: string },
  ctx: ResolverContext,
) {
  await authorizeForAltToken(ctx, 'plans:write');
  return desktopRpc<unknown>(ctx, 'update_outline_item', { initiativeId: args.initiativeId, id: args.id, title: args.title });
}
