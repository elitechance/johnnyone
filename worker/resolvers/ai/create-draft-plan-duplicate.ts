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

// Plans tab (initiative 8ad3ae71, phase 01): Copy a draft plan; v1 of the copy = the source latest fields + outline.
// Thin pass-through to the host `duplicate_draft_plan` over the relay, gated by the plans:write scope.
export default async function createDraftPlanDuplicate(
  _parent: unknown,
  args: { initiativeId: string },
  ctx: ResolverContext,
) {
  await authorizeForAltToken(ctx, 'plans:write');
  return desktopRpc<unknown>(ctx, 'duplicate_draft_plan', { initiativeId: args.initiativeId });
}
