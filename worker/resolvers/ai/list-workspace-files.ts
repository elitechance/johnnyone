import { desktopRpc } from '../../lib/runtime/desktop-rpc';

interface ResolverContext { db: D1Database; env: WorkerEnv; auth: { userId: string; tenantId: string } }
interface WorkerEnv { CHAT_RELAY_DO: DurableObjectNamespace; [key: string]: unknown }

export default async function listWorkspaceFiles(
  _parent: unknown,
  args: { planId: string; mode: string },
  ctx: ResolverContext,
) {
  return desktopRpc<unknown[]>(ctx, 'list_workspace_files', { id: args.planId, mode: args.mode });
}
