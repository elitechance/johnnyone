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

// Plans tab (initiative 8ad3ae71, phase 01): Undo one field / revert the whole brief to a version — always a NEW version.
// Thin pass-through to the host `reset_brief_to_version` over the relay, gated by the plans:write scope.
export default async function resetBriefToVersion(
  _parent: unknown,
  args: { initiativeId: string; version: number; field?: string | null },
  ctx: ResolverContext,
) {
  await authorizeForAltToken(ctx, 'plans:write');
  return desktopRpc<unknown>(ctx, 'reset_brief_to_version', { initiativeId: args.initiativeId, version: args.version, field: args.field });
}
