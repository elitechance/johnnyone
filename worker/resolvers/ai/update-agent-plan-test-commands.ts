import { desktopRpc } from '../../lib/runtime/desktop-rpc';
import { authorizeForAltToken } from '../../lib/auth/api-key';

interface ResolverContext { db: D1Database; env: WorkerEnv; auth: { userId: string; tenantId: string }; request?: Request; [key: string]: unknown }
interface WorkerEnv { CHAT_RELAY_DO: DurableObjectNamespace; [key: string]: unknown }

// The coordinator's test gate (migration 023 plan-level, 025 phase-level). Follows
// updateAgentPlanValidationConfig's auth scope and desktopRpc seam.
//
// `phaseId` picks the level: null targets the PLAN's gate, a phase id targets that phase's. The
// phase-over-plan fallback is NOT implemented here — the host owns it
// (services::agent_plans::effective_gate) and exposes the result as
// AgentPlanPhase.effectiveTestCommands, so no client ever re-derives it.
//
// `commands` is REQUIRED by the schema and removing a gate needs `clear: true`. It did not start
// that way: `commands` was optional and defaulted to [], which meant "clear", so
// `updateAgentPlanTestCommands(id: "x")` — a call that forgot its optional arguments — silently
// wiped the Initiative-wide gate. On a mutation whose name sounds additive, destroying state must
// not be what an incomplete call does. The host rejects the contradictory and the ambiguous
// combinations too, so the RPC is safe on its own rather than only behind this resolver.
export default async function updateAgentPlanTestCommands(
  _parent: unknown,
  args: { id: string; phaseId?: string | null; commands: string[]; clear?: boolean | null },
  ctx: ResolverContext,
) {
  await authorizeForAltToken(ctx, 'plans:write');
  return desktopRpc<unknown>(ctx, 'update_agent_plan_test_commands', {
    id: args.id,
    phaseId: args.phaseId ?? null,
    commands: args.commands ?? [],
    clear: args.clear ?? false,
  });
}
