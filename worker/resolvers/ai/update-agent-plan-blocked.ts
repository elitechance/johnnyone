import { desktopRpc } from '../../lib/runtime/desktop-rpc';

interface ResolverContext { db: D1Database; env: WorkerEnv; auth: { userId: string; tenantId: string } }
interface WorkerEnv { CHAT_RELAY_DO: DurableObjectNamespace; [key: string]: unknown }

export default async function updateAgentPlanBlocked(
  _parent: unknown,
  args: { id: string; reason: string },
  ctx: ResolverContext,
) {
  return desktopRpc<unknown>(ctx, 'block_agent_plan', { id: args.id, reason: args.reason });
}
