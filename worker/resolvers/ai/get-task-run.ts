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

export default async function getTaskRun(
  _parent: unknown,
  args: { planId: string; phaseId: string },
  ctx: ResolverContext,
) {
  await authorizeForAltToken(ctx, 'plans:read');
  return desktopRpc<unknown>(ctx, 'get_task_run', {
    planId: args.planId,
    phaseId: args.phaseId,
  });
}
