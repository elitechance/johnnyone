import { desktopRpc } from '../../lib/runtime/desktop-rpc';

interface ResolverContext { db: D1Database; env: WorkerEnv; auth: { userId: string; tenantId: string } }
interface WorkerEnv { CHAT_RELAY_DO: DurableObjectNamespace; [key: string]: unknown }

export default async function gitFileView(
  _parent: unknown,
  args: { planId: string; path?: string | null },
  ctx: ResolverContext,
) {
  return desktopRpc<unknown>(ctx, 'git_file_view', { id: args.planId, path: args.path ?? null });
}
