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

export default async function deleteAiSession(
  _parent: unknown,
  args: { id: string },
  ctx: ResolverContext,
) {
  await authorizeForAltToken(ctx, 'sessions:write');
  await desktopRpc<{ deleted: boolean }>(ctx, 'delete_session', { id: args.id });
  return true;
}
