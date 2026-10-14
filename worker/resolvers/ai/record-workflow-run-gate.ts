import { desktopRpc } from '../../lib/runtime/desktop-rpc';
import { authorizeForAltToken } from '../../lib/auth/api-key';

interface ResolverContext { db: D1Database; env: WorkerEnv; auth: { userId: string; tenantId: string }; request?: Request; [key: string]: unknown }
interface WorkerEnv { CHAT_RELAY_DO: DurableObjectNamespace; [key: string]: unknown }

// Record a run's gate result (desktop migration 026).
//
// `gateExit` is authoritative and non-null: it is the only score this feature has. `gateCollected`
// is ADVISORY and nullable, and omitting it is a NORMAL result — a gate that prints nothing a
// parser recognises still has a perfectly good exit code. It is relayed as `null` rather than 0,
// because 0 collected means "the gate discriminates nothing", which is a real and different claim.
// `rescore` defaults to FALSE and is relayed explicitly. A second result for a run used to be an
// unconditional overwrite: it replaced a green score with a red one and erased the advisory count,
// so a promoted champion's own run could be re-scored after the fact. The default here is the one
// that refuses, not the one that overwrites the only score the arena has.
export default async function recordWorkflowRunGate(
  _parent: unknown,
  args: {
    runId: string;
    gateExit: number;
    gateCollected?: number | null;
    rescore?: boolean | null;
  },
  ctx: ResolverContext,
) {
  await authorizeForAltToken(ctx, 'plans:write');
  return desktopRpc<unknown>(ctx, 'record_workflow_run_gate', {
    runId: args.runId,
    gateExit: args.gateExit,
    gateCollected: args.gateCollected ?? null,
    rescore: args.rescore ?? false,
  });
}
