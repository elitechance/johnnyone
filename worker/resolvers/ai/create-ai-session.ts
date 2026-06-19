import { desktopRpc } from '../../lib/runtime/desktop-rpc';
import { authorizeForAltToken } from '../../lib/auth/api-key';

interface ResolverContext {
  db: D1Database;
  env: WorkerEnv;
  auth: { userId: string; tenantId: string };
  request?: Request;
  [key: string]: unknown;
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
  await authorizeForAltToken(ctx, 'sessions:write');
  return desktopRpc<unknown>(ctx, 'create_session', { input: args.input });
}
