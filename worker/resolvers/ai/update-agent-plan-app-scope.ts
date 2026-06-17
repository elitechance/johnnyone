import { desktopRpc } from '../../lib/runtime/desktop-rpc';

interface ResolverContext { db: D1Database; env: WorkerEnv; auth: { userId: string; tenantId: string } }
interface WorkerEnv { CHAT_RELAY_DO: DurableObjectNamespace; [key: string]: unknown }

export default async function updateAgentPlanAppScope(
  _parent: unknown,
  args: { id: string; appScope?: string | null },
  ctx: ResolverContext,
) {
  return desktopRpc<unknown>(ctx, 'update_agent_plan_app_scope', {
    id: args.id,
    appScope: args.appScope ?? '',
  });
}
