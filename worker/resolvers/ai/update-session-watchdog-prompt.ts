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
  // The host answers `{"ok": true}` while the schema declares this field `Boolean!`, so the
  // resolver converts the payload to a boolean (the convention in `delete-ai-session.ts`).
  // Returning the object verbatim makes graphql-js reject it AFTER the write has landed, so the
  // client reports failure for a successful write. Keep the `await` — dropping it would swallow a
  // host-side rejection and report success for a failed write, which is this bug mirrored.
  // NOTE: the host handler `rpc_update_session_watchdog_prompt` reads `id`, NOT `sessionId` like
  // the other watchdog RPCs — the host is the odd one out, so the Worker maps it here until the
  // engine is rebuilt to standardise on `sessionId`.
  await desktopRpc<{ ok: boolean }>(ctx, 'update_session_watchdog_prompt', {
    id: args.sessionId,
    prompt: args.prompt,
  });
  return true;
}