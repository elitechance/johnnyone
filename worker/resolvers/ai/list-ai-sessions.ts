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

export default async function listAiSessions(
  _parent: unknown,
  args: { status?: string },
  ctx: ResolverContext,
) {
  await authorizeForAltToken(ctx, 'sessions:read');
  return desktopRpc<unknown[]>(ctx, 'list_sessions', { status: args.status ?? null });
}
