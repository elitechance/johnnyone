import { desktopRpc } from '../lib/runtime/desktop-rpc';

interface ResolverContext { db: D1Database; env: WorkerEnv; auth: { userId: string; tenantId: string } }
interface WorkerEnv { CHAT_RELAY_DO: DurableObjectNamespace; [key: string]: unknown }

export default async function updateGitAction(
  _parent: unknown,
  args: { planId: string; path?: string | null; action: string; message?: string | null },
  ctx: ResolverContext,
) {
  return desktopRpc<unknown>(ctx, 'run_git_action', {
    id: args.planId,
    path: args.path ?? null,
    action: args.action,
    message: args.message ?? null,
  });
}
