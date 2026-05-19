import { desktopRpc } from '../../lib/runtime/desktop-rpc';

interface ResolverContext {
  db: D1Database;
  env: WorkerEnv;
  auth: { userId: string; tenantId: string };
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
  await desktopRpc<{ deleted: boolean }>(ctx, 'delete_session', { id: args.id });
  return true;
}
