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

// Watchdog: "Nudge now" — deliver this session's watchdog prompt immediately, skipping the
// sweep's due check and idle probe. Thin pass-through to the host `nudge_session_watchdog` RPC.
export default async function sendSessionWatchdogNudge(
  _parent: unknown,
  args: { sessionId: string },
  ctx: ResolverContext,
) {
  await authorizeForAltToken(ctx, 'sessions:write');
  return desktopRpc<boolean>(ctx, 'nudge_session_watchdog', {
    sessionId: args.sessionId,
  });
}