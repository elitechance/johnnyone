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

export default async function listAiMessages(
  _parent: unknown,
  args: { sessionId: string; limit?: number; offset?: number },
  ctx: ResolverContext,
) {
  return desktopRpc<unknown[]>(ctx, 'list_messages', {
    sessionId: args.sessionId,
    limit: args.limit ?? 100,
    offset: args.offset ?? 0,
  });
}
