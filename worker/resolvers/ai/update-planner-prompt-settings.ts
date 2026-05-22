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

export default async function updatePlannerPromptSettings(
  _parent: unknown,
  args: { input: unknown },
  ctx: ResolverContext,
) {
  return desktopRpc<unknown>(ctx, 'update_planner_prompt_settings', { input: args.input });
}
