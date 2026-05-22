import { desktopRpc } from '../../lib/runtime/desktop-rpc';

interface ResolverContext { db: D1Database; env: WorkerEnv; auth: { userId: string; tenantId: string } }
interface WorkerEnv { CHAT_RELAY_DO: DurableObjectNamespace; [key: string]: unknown }

export default async function getWorkspaceFileDiff(
  _parent: unknown,
  args: { planId: string; path: string },
  ctx: ResolverContext,
) {
  return desktopRpc<unknown>(ctx, 'get_workspace_file_diff', { id: args.planId, path: args.path });
}
