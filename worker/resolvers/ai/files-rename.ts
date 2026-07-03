import { desktopRpc } from '../../lib/runtime/desktop-rpc';
import { authorizeForAltToken } from '../../lib/auth/api-key';

interface ResolverContext { db: D1Database; env: WorkerEnv; auth: { userId: string; tenantId: string } }
interface WorkerEnv { CHAT_RELAY_DO: DurableObjectNamespace; [key: string]: unknown }

// File manager (overhaul P2): thin pass-through to the host files_root surface. No FS logic here —
// the host owns behavior; the worker only relays, gated by the files:write scope.
export default async function filesRename(
  _parent: unknown,
  args: { from: string; to: string },
  ctx: ResolverContext,
) {
  await authorizeForAltToken(ctx, 'files:write');
  return desktopRpc<unknown>(ctx, 'files_rename', { from: args.from, to: args.to });
}
