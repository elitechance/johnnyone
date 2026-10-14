import { desktopRpc } from '../../lib/runtime/desktop-rpc';
import { authorizeForAltToken } from '../../lib/auth/api-key';

interface ResolverContext { db: D1Database; env: WorkerEnv; auth: { userId: string; tenantId: string }; request?: Request; [key: string]: unknown }
interface WorkerEnv { CHAT_RELAY_DO: DurableObjectNamespace; [key: string]: unknown }

// EXECUTE an arena run's gate and record the result (desktop migration 026) — the step that closes
// the arena's loop.
//
// `recordWorkflowRunGate` takes a score someone else measured; this one MEASURES it, by running the
// plan's effective gate in the plan's workspace via the same resolution rule and the same shell the
// coordinator's own ready-check uses. Without it every run the UI creates keeps a null `gateExit`
// forever, promotion is refused for having no score, comparison is impossible, and the one human
// decision the arena exists for is unreachable.
//
// This EXECUTES an arbitrary shell command on the host, so it carries plans:write — the same
// authority as `runAgentPlanTestGate`, and deliberately not a read scope.
//
// Executes the gate AGAIN for a run that already has a score, and replaces it. Its own mutation
// rather than a flag on `runWorkflowRunGate`, for the same reason as recordWorkflowRunRescore: the
// separate name is the guard. A sitting champion's promoting run is refused here too.
//
// Named `run-workflow-run-rescore` and not `rescore-workflow-run-gate`: lokal infers the GraphQL
// field TYPE from the filename's leading verb, and `rescore-` is not one of its mutation prefixes,
// so the latter auto-wired as a Query and broke schema-resolver validation. Measured, not guessed.
export default async function runWorkflowRunRescore(
  _parent: unknown,
  args: { runId: string; phaseId?: string | null },
  ctx: ResolverContext,
) {
  await authorizeForAltToken(ctx, 'plans:write');
  return desktopRpc<unknown>(ctx, 'rescore_workflow_run_gate', {
    runId: args.runId,
    phaseId: args.phaseId ?? null,
  });
}
