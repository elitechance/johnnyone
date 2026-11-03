import { desktopRpc } from '../../lib/runtime/desktop-rpc';
import { authorizeForAltToken } from '../../lib/auth/api-key';

interface ResolverContext {
  db: D1Database;
  env: WorkerEnv;
  auth: { userId: string; tenantId: string };
  request?: Request;
  [key: string]: unknown;
}

interface WorkerEnv {
  CHAT_RELAY_DO: DurableObjectNamespace;
  [key: string]: unknown;
}

// Watchdog: persist the two "Also nudge when" checkboxes (blocked-on-a-question, stale reports).
// Thin pass-through to the host `update_session_watchdog_triggers` RPC.
//
// NOTE the filename: the lokal CLI infers Query-vs-Mutation from the leading verb, and an
// unrecognised verb silently becomes a Query — which the deploy then refuses. `update-` is a
// prefix it knows; this file MUST keep it, exactly like `update-session-watchdog.ts` beside it.
export default async function updateSessionWatchdogTriggers(
  _parent: unknown,
  args: { sessionId: string; onBlocked: boolean; onStaleReport: boolean },
  ctx: ResolverContext,
) {
  await authorizeForAltToken(ctx, 'sessions:write');
  return desktopRpc<boolean>(ctx, 'update_session_watchdog_triggers', {
    sessionId: args.sessionId,
    onBlocked: args.onBlocked,
    onStaleReport: args.onStaleReport,
  });
}