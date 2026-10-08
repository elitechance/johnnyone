import { desktopRpc } from '../../lib/runtime/desktop-rpc';
import { authorizeForAltToken } from '../../lib/auth/api-key';

interface ResolverContext { db: D1Database; env: WorkerEnv; auth: { userId: string; tenantId: string } }
interface WorkerEnv { CHAT_RELAY_DO: DurableObjectNamespace; [key: string]: unknown }

// Plan-workspace file read: thin pass-through to the host read_host_file surface. No FS logic
// here — the host owns behavior (it path-confines the read to the plan's workspace_path); the
// worker only relays, gated by the files:read scope.
export default async function readHostFile(
  _parent: unknown,
  args: { planId: string; path: string },
  ctx: ResolverContext,
) {
  await authorizeForAltToken(ctx, 'files:read');
  return desktopRpc<unknown>(ctx, 'read_host_file', { id: args.planId, path: args.path });
}
