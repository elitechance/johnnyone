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

export default async function createAgentPlan(
  _parent: unknown,
  args: { input: unknown },
  ctx: ResolverContext,
) {
  return desktopRpc<unknown>(ctx, 'create_agent_plan', { input: args.input });
}
