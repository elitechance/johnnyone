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

interface CreateAiSessionInput {
  title?: string;
  provider?: string;
  model?: string;
  workingDirectory?: string;
}

export default async function createAiSession(
  _parent: unknown,
  args: { input: CreateAiSessionInput },
  ctx: ResolverContext,
) {
  return desktopRpc<unknown>(ctx, 'create_session', { input: args.input });
}
