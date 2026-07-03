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

// Briefing loop (overhaul P4, D6): thin pass-through to the host `add_initiative_reference_path`,
// which records a ▤ Reference host path on the briefing Initiative. Gated by the plans:write scope.
export default async function addInitiativeReferencePath(
  _parent: unknown,
  args: { initiativeId: string; path: string },
  ctx: ResolverContext,
) {
  await authorizeForAltToken(ctx, 'plans:write');
  return desktopRpc<unknown>(ctx, 'add_initiative_reference_path', {
    initiativeId: args.initiativeId,
    path: args.path,
  });
}
