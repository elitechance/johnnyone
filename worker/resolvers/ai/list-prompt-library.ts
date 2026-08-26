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

export default async function listPromptLibrary(
  _parent: unknown,
  _args: Record<string, never>,
  ctx: ResolverContext,
) {
  return desktopRpc(ctx, 'list_prompt_library');
}
