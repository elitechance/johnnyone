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

export default async function updateAiSessionTitle(
  _parent: unknown,
  args: { id: string; title: string },
  ctx: ResolverContext,
) {
  return desktopRpc<unknown>(ctx, 'update_session_title', { id: args.id, title: args.title });
}
