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

// Plans tab (initiative 8ad3ae71, phase 01): Server-computed diff between two versions.
// Thin pass-through to the host `get_brief_diff` over the relay, gated by the plans:read scope.
export default async function getBriefDiff(
  _parent: unknown,
  args: { initiativeId: string; fromVersion: number; toVersion: number },
  ctx: ResolverContext,
) {
  await authorizeForAltToken(ctx, 'plans:read');
  return desktopRpc<unknown>(ctx, 'get_brief_diff', { initiativeId: args.initiativeId, fromVersion: args.fromVersion, toVersion: args.toVersion });
}
