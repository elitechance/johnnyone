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

// Lists the external tmux sessions a new terminal can attach to (the desktop
// excludes the johnnyone_<id> panes it already manages).
export default async function listTmuxSessions(
  _parent: unknown,
  _args: Record<string, never>,
  ctx: ResolverContext,
) {
  await authorizeForAltToken(ctx, 'sessions:read');
  return desktopRpc<unknown[]>(ctx, 'list_tmux_sessions', {});
}
