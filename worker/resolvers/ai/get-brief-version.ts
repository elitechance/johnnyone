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

// Plans tab (initiative 8ad3ae71, phase 01): One brief version (latest when `version` is omitted).
// Thin pass-through to the host `get_brief_version` over the relay, gated by the plans:read scope.
export default async function getBriefVersion(
  _parent: unknown,
  args: { initiativeId: string; version?: number | null },
  ctx: ResolverContext,
) {
  await authorizeForAltToken(ctx, 'plans:read');
  return desktopRpc<unknown>(ctx, 'get_brief_version', { initiativeId: args.initiativeId, version: args.version });
}
