import { desktopRpc } from '../../lib/runtime/desktop-rpc';

interface ResolverContext { db: D1Database; env: WorkerEnv; auth: { userId: string; tenantId: string } }
interface WorkerEnv { CHAT_RELAY_DO: DurableObjectNamespace; [key: string]: unknown }

export default async function updateAgentPhaseManualPass(
  _parent: unknown,
  args: { id: string; phaseId: string },
  ctx: ResolverContext,
) {
  return desktopRpc<unknown>(ctx, 'manual_pass_agent_phase', { id: args.id, phaseId: args.phaseId });
}
