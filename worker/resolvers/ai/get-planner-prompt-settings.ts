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

export default async function getPlannerPromptSettings(
  _parent: unknown,
  _args: Record<string, never>,
  ctx: ResolverContext,
) {
  return desktopRpc<unknown>(ctx, 'get_planner_prompt_settings');
}
