import { desktopRpc } from '../../lib/runtime/desktop-rpc';
import { authorizeForAltToken } from '../../lib/auth/api-key';

interface ResolverContext { db: D1Database; env: WorkerEnv; auth: { userId: string; tenantId: string }; request?: Request; [key: string]: unknown }
interface WorkerEnv { CHAT_RELAY_DO: DurableObjectNamespace; [key: string]: unknown }

// Start a run of a workflow (desktop migration 026).
//
// `seatsHeld` and `fleetBusy` are both required JSON objects and both are validated by the host
// before the insert. An unreadable seatsHeld is not a cosmetic problem: it makes EVERY preference
// involving this run non-comparable, which is a silent dead end rather than an error.
//
// REFUSED by the host while the family's search is halted. That refusal is the whole meaning of
// "three consecutive rejections halt the search" — the client does not have to check first.
export default async function startWorkflowRun(
  _parent: unknown,
  args: {
    workflowId: string;
    planId?: string | null;
    seed?: number | null;
    seatsHeld: string;
    fleetBusy: string;
  },
  ctx: ResolverContext,
) {
  await authorizeForAltToken(ctx, 'plans:write');
  return desktopRpc<unknown>(ctx, 'start_workflow_run', {
    workflowId: args.workflowId,
    planId: args.planId ?? null,
    seed: args.seed ?? 0,
    seatsHeld: args.seatsHeld,
    fleetBusy: args.fleetBusy,
  });
}
