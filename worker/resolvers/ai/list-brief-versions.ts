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

// Plans tab (initiative 8ad3ae71, phase 01): Version metadata (the version strip), oldest first.
// Thin pass-through to the host `list_brief_versions` over the relay, gated by the plans:read scope.
export default async function listBriefVersions(
  _parent: unknown,
  args: { initiativeId: string },
  ctx: ResolverContext,
) {
  await authorizeForAltToken(ctx, 'plans:read');
  return desktopRpc<unknown>(ctx, 'list_brief_versions', { initiativeId: args.initiativeId });
}
