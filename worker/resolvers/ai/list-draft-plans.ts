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

// Plans tab (initiative 8ad3ae71, phase 01): Every draft plan (briefing Initiative, not closed), newest latest-version first.
// Thin pass-through to the host `list_draft_plans` over the relay, gated by the plans:read scope.
export default async function listDraftPlans(
  _parent: unknown,
  args: Record<string, never>,
  ctx: ResolverContext,
) {
  await authorizeForAltToken(ctx, 'plans:read');
  return desktopRpc<unknown>(ctx, 'list_draft_plans', {});
}
