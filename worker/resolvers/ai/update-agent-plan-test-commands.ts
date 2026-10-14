import { desktopRpc } from '../../lib/runtime/desktop-rpc';
import { authorizeForAltToken } from '../../lib/auth/api-key';

interface ResolverContext { db: D1Database; env: WorkerEnv; auth: { userId: string; tenantId: string }; request?: Request; [key: string]: unknown }
interface WorkerEnv { CHAT_RELAY_DO: DurableObjectNamespace; [key: string]: unknown }

// The coordinator's test gate (migration 023 plan-level, 025 phase-level). Mirrors
// updateAgentPlanValidationConfig: same auth scope, same desktopRpc seam, and an absent/empty
// value CLEARS the gate at the chosen level.
//
// `phaseId` picks the level: null targets the PLAN's gate, a phase id targets that phase's. The
// phase-over-plan fallback is NOT implemented here — the host owns it
// (services::agent_plans::effective_test_commands) and exposes the result as
// AgentPlanPhase.effectiveTestCommands, so no client ever re-derives it.
export default async function updateAgentPlanTestCommands(
  _parent: unknown,
  args: { id: string; phaseId?: string | null; commands?: string[] | null },
  ctx: ResolverContext,
) {
  await authorizeForAltToken(ctx, 'plans:write');
  return desktopRpc<unknown>(ctx, 'update_agent_plan_test_commands', {
    id: args.id,
    phaseId: args.phaseId ?? null,
    commands: args.commands ?? [],
  });
}
