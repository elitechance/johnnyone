import { desktopRpc } from '../../lib/runtime/desktop-rpc';
import { authorizeForAltToken } from '../../lib/auth/api-key';

interface ResolverContext { db: D1Database; env: WorkerEnv; auth: { userId: string; tenantId: string }; request?: Request; [key: string]: unknown }
interface WorkerEnv { CHAT_RELAY_DO: DurableObjectNamespace; [key: string]: unknown }

// REPLACE a run's gate result with one measured elsewhere, deliberately (desktop migration 026).
//
// The sibling of `recordWorkflowRunGate`, which refuses a run that already has a score. Two fields
// rather than one field with a `rescore: Boolean`, because the separate NAME is the guard: there is
// no flag to forget and no default to get wrong, and a reader of a call site can see which was
// meant. The sitting champion's promoting run is refused through either field — that rule is in the
// host, not in the choice of door.
export default async function recordWorkflowRunRescore(
  _parent: unknown,
  args: {
    runId: string;
    gateExit: number;
    gateCollected?: number | null;
  },
  ctx: ResolverContext,
) {
  await authorizeForAltToken(ctx, 'plans:write');
  return desktopRpc<unknown>(ctx, 'record_workflow_run_rescore', {
    runId: args.runId,
    gateExit: args.gateExit,
    gateCollected: args.gateCollected ?? null,
  });
}
