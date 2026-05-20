import { desktopRpc } from '../../lib/runtime/desktop-rpc';

interface ResolverContext { db: D1Database; env: WorkerEnv; auth: { userId: string; tenantId: string } }
interface WorkerEnv { CHAT_RELAY_DO: DurableObjectNamespace; [key: string]: unknown }

export default async function updateAgentPlanStopped(_parent: unknown, args: { id: string }, ctx: ResolverContext) {
  return desktopRpc<unknown>(ctx, 'stop_agent_plan', { id: args.id });
}
