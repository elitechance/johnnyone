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

export default async function listAgentPlans(
  _parent: unknown,
  args: { status?: string },
  ctx: ResolverContext,
) {
  return desktopRpc<unknown[]>(ctx, 'list_agent_plans', { status: args.status ?? null });
}
