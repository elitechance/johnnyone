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

export default async function updateAiSessionWorkingDirectory(
  _parent: unknown,
  args: { id: string; workingDirectory: string },
  ctx: ResolverContext,
) {
  await authorizeForAltToken(ctx, 'sessions:write');
  return desktopRpc<unknown>(ctx, 'update_session_working_directory', {
    id: args.id,
    workingDirectory: args.workingDirectory,
  });
}
