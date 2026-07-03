import { desktopRpc } from '../../lib/runtime/desktop-rpc';
import { authorizeForAltToken } from '../../lib/auth/api-key';

interface ResolverContext { db: D1Database; env: WorkerEnv; auth: { userId: string; tenantId: string } }
interface WorkerEnv { CHAT_RELAY_DO: DurableObjectNamespace; [key: string]: unknown }

// Whole-tree working-tree diff (overhaul P7, D7/D10/D13). A path-keyed op — same posture as
// filesListDir/filesRead: gate on the files:read scope FIRST, then relay to the host, which
// path-guards under files_root and only ever reads git. The host owns behavior; the worker relays.
export default async function gitDiff(
  _parent: unknown,
  args: { path: string },
  ctx: ResolverContext,
) {
  await authorizeForAltToken(ctx, 'files:read');
  return desktopRpc<unknown>(ctx, 'git_diff', { path: args.path });
}
