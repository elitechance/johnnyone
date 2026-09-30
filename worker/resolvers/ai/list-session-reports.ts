import { desktopRpc } from '../../lib/runtime/desktop-rpc';
import { authorizeForAltToken } from '../../lib/auth/api-key';

interface ResolverContext {
  db: D1Database;
  env: { CHAT_RELAY_DO: DurableObjectNamespace; [key: string]: unknown };
  auth: { userId: string; tenantId: string };
  request?: Request;
  [key: string]: unknown;
}

// A session's persisted reports. Reports used to live only in memory and on the live stream, so a
// reload showed an empty transcript; this is what lets the console hydrate its history.
export default async function listSessionReports(
  _parent: unknown,
  args: { sessionId: string; limit?: number },
  ctx: ResolverContext,
) {
  await authorizeForAltToken(ctx, 'sessions:read');
  return desktopRpc<unknown[]>(ctx, 'list_session_reports', {
    sessionId: args.sessionId,
    limit: args.limit ?? 100,
  });
}
