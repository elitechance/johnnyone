import { desktopRpc } from '../../lib/runtime/desktop-rpc';
import { authorizeForAltToken } from '../../lib/auth/api-key';

interface ResolverContext { db: D1Database; env: WorkerEnv; auth: { userId: string; tenantId: string } }
interface WorkerEnv { CHAT_RELAY_DO: DurableObjectNamespace; [key: string]: unknown }

// File manager (overhaul P2): thin pass-through to the host files_root surface. No FS logic here —
// the host owns behavior; the worker only relays, gated by the files:read scope.
export default async function filesRead(
  _parent: unknown,
  args: { path: string },
  ctx: ResolverContext,
) {
  await authorizeForAltToken(ctx, 'files:read');
  return desktopRpc<unknown>(ctx, 'files_read', { path: args.path });
}
