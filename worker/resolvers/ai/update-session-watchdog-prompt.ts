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

// Watchdog: update the prompt the host sends as the nudge message.
// Thin pass-through to the host `update_session_watchdog_prompt` RPC.
export default async function updateSessionWatchdogPrompt(
  _parent: unknown,
  args: { sessionId: string; prompt: string },
  ctx: ResolverContext,
) {
  await authorizeForAltToken(ctx, 'sessions:write');
  return desktopRpc<boolean>(ctx, 'update_session_watchdog_prompt', {
    sessionId: args.sessionId,
    prompt: args.prompt,
  });
}