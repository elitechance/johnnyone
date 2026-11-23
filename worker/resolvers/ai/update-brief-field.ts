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

// Plans tab (initiative 8ad3ae71, phase 01): Human edit of one brief field (author you) -> a new version.
// Thin pass-through to the host `update_brief_field` over the relay, gated by the plans:write scope.
export default async function updateBriefField(
  _parent: unknown,
  args: { initiativeId: string; field: string; op: string; value?: string | null; values?: string[] | null },
  ctx: ResolverContext,
) {
  await authorizeForAltToken(ctx, 'plans:write');
  return desktopRpc<unknown>(ctx, 'update_brief_field', { initiativeId: args.initiativeId, field: args.field, op: args.op, value: args.value, values: args.values });
}
