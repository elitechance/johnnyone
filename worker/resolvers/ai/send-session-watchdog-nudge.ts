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
  // The host answers `{"ok": true}` while the schema declares this field `Boolean!`, so the
  // resolver converts the payload to a boolean (the convention in `delete-ai-session.ts`).
  // Returning the object verbatim makes graphql-js reject it AFTER the write has landed, so the
  // client reports failure for a successful write. Keep the `await` — dropping it would swallow a
  // host-side rejection and report success for a failed write, which is this bug mirrored.
  await desktopRpc<{ ok: boolean }>(ctx, 'nudge_session_watchdog', {
    sessionId: args.sessionId,
  });
  return true;
}