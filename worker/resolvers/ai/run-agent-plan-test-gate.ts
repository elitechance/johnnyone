import { desktopRpc } from '../../lib/runtime/desktop-rpc';
import { authorizeForAltToken } from '../../lib/auth/api-key';

interface ResolverContext { db: D1Database; env: WorkerEnv; auth: { userId: string; tenantId: string }; request?: Request; [key: string]: unknown }
interface WorkerEnv { CHAT_RELAY_DO: DurableObjectNamespace; [key: string]: unknown }

// Run a phase's (or the plan's) effective test gate ONCE and report the outcome. This EXECUTES an
// arbitrary shell command on the host, so it carries plans:write — the same authority as setting
// the gate — not a read scope.
//
// `exitCode` is authoritative. `collected` is advisory and nullable: a gate that prints nothing a
// parser recognises is a legitimate gate with a known exit code, so null is a normal result and
// must never be treated as an error or gated on.
export default async function runAgentPlanTestGate(
  _parent: unknown,
  args: { id: string; phaseId?: string | null },
  ctx: ResolverContext,
) {
  await authorizeForAltToken(ctx, 'plans:write');
  return desktopRpc<unknown>(ctx, 'run_agent_plan_test_gate', {
    id: args.id,
    phaseId: args.phaseId ?? null,
  });
}
