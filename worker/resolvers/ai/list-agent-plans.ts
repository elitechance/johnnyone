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

export default async function listAgentPlans(
  _parent: unknown,
  args: { status?: string; runType?: string; onlyExisting?: boolean },
  ctx: ResolverContext,
) {
  await authorizeForAltToken(ctx, 'plans:read');
  return desktopRpc<unknown[]>(ctx, 'list_agent_plans', {
    status: args.status ?? null,
    runType: args.runType ?? null,
    onlyExisting: args.onlyExisting ?? false,
  });
}
