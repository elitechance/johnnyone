import { desktopRpc } from '../../lib/runtime/desktop-rpc';
import { authorizeForAltToken } from '../../lib/auth/api-key';

interface ResolverContext { db: D1Database; env: WorkerEnv; auth: { userId: string; tenantId: string }; request?: Request; [key: string]: unknown }
interface WorkerEnv { CHAT_RELAY_DO: DurableObjectNamespace; [key: string]: unknown }

// Record that one run beat another (desktop migration 026).
//
// There is deliberately NO `comparable` argument. The host computes it from the two runs' own
// `seatsHeld`, so a client cannot assert a comparability it does not have — which matters because
// `comparable: false` is what stops a promotion that really only measured fleet contention.
export default async function recordWorkflowPref(
  _parent: unknown,
  args: { winnerRunId: string; loserRunId: string; reason?: string | null },
  ctx: ResolverContext,
) {
  await authorizeForAltToken(ctx, 'plans:write');
  return desktopRpc<unknown>(ctx, 'record_workflow_pref', {
    winnerRunId: args.winnerRunId,
    loserRunId: args.loserRunId,
    reason: args.reason ?? null,
  });
}
