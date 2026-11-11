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

// Watchdog: arm/disarm per-shell periodic nudges with a configurable interval.
// Thin pass-through to the host `update_session_watchdog` RPC.
export default async function updateSessionWatchdog(
  _parent: unknown,
  args: { sessionId: string; enabled: boolean; intervalSecs: number },
  ctx: ResolverContext,
) {
  await authorizeForAltToken(ctx, 'sessions:write');
  // The host answers `{"ok": true}` while the schema declares this field `Boolean!`, so the
  // resolver converts the payload to a boolean (the convention in `delete-ai-session.ts`).
  // Returning the object verbatim makes graphql-js reject it AFTER the write has landed, so the
  // client reports failure for a successful write. Keep the `await` — dropping it would swallow a
  // host-side rejection and report success for a failed write, which is this bug mirrored.
  await desktopRpc<{ ok: boolean }>(ctx, 'update_session_watchdog', {
    sessionId: args.sessionId,
    enabled: args.enabled,
    intervalSecs: args.intervalSecs,
  });
  return true;
}