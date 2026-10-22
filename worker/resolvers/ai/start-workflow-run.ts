import { desktopRpc } from '../../lib/runtime/desktop-rpc';
import { authorizeForAltToken } from '../../lib/auth/api-key';

interface ResolverContext { db: D1Database; env: WorkerEnv; auth: { userId: string; tenantId: string }; request?: Request; [key: string]: unknown }
interface WorkerEnv { CHAT_RELAY_DO: DurableObjectNamespace; [key: string]: unknown }

// Start a run of a workflow (desktop migration 026).
//
// `seatsHeld` and `fleetBusy` are NOT relayed, and used to be. The caller here is a browser, which
// can measure neither — the UI was sending the declared budget and `{}` for contention, honestly
// labelled as a declaration in its own code, which reduced `comparable` to "did two clients type
// the same numbers". That is the exact failure `schema.md` says those two columns exist to prevent:
// "the first thing a busy fleet does is promote a champion because the other box was quiet."
//
// The HOST measures both (`services::workflows::measure_run_budget`, whose intended source is
// `services::leases::LeasePool`). `seats` optionally overrides the workflow's own `fanOut`; the
// model family is never an argument, because it is the workflow's and the workflow row holds it.
//
// REFUSED by the host while the family's search is halted. That refusal is the whole meaning of
// "three consecutive rejections halt the search" — the client does not have to check first.
export default async function startWorkflowRun(
  _parent: unknown,
  args: {
    workflowId: string;
    planId?: string | null;
    seed?: number | null;
    seats?: number | null;
  },
  ctx: ResolverContext,
) {
  await authorizeForAltToken(ctx, 'plans:write');
  return desktopRpc<unknown>(ctx, 'start_workflow_run', {
    workflowId: args.workflowId,
    planId: args.planId ?? null,
    seed: args.seed ?? 0,
    seats: args.seats ?? null,
  });
}
