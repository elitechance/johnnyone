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

export default async function listAiSessions(
  _parent: unknown,
  args: { status?: string },
  ctx: ResolverContext,
) {
  return desktopRpc<unknown[]>(ctx, 'list_sessions', { status: args.status ?? null });
}
