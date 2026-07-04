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

/**
 * Full SDLC event timeline for one Initiative. Events are stored per plan-run, but an Initiative spans
 * two runs (planning + development) sharing one `initiativeId`; the desktop `list_initiative_events`
 * UNIONs both runs' events (oldest-first) so the console renders planning → approval → development →
 * review loop → done in one stream.
 */
export default async function listInitiativeEvents(
  _parent: unknown,
  args: { initiativeId: string },
  ctx: ResolverContext,
) {
  await authorizeForAltToken(ctx, 'plans:read');
  return desktopRpc<unknown>(ctx, 'list_initiative_events', {
    initiativeId: args.initiativeId,
  });
}
