import { desktopRpc } from '../../lib/runtime/desktop-rpc';
import { authorizeForAltToken } from '../../lib/auth/api-key';

interface ResolverContext { db: D1Database; env: WorkerEnv; auth: { userId: string; tenantId: string }; request?: Request; [key: string]: unknown }
interface WorkerEnv { CHAT_RELAY_DO: DurableObjectNamespace; [key: string]: unknown }

// The repo identity the HOST derives for a workspace path (desktop migration 026).
//
// `workflows.repo` used to be sent by the client as `basename(workspace_path)`, so `/a/johnnyone`
// and `/b/johnnyone` — two unrelated checkouts — shared one champion. The host derives it now
// (`services::workflows::repo_identity`: the git remote, normalised, else the absolute path), and
// this read is the one way a client learns the value so it can key `workflows` and
// `workflowSearchState` on it without re-implementing the derivation in a second place.
export default async function repoIdentity(
  _parent: unknown,
  args: { workspacePath: string },
  ctx: ResolverContext,
) {
  await authorizeForAltToken(ctx, 'plans:read');
  return desktopRpc<string>(ctx, 'get_repo_identity', { workspacePath: args.workspacePath });
}
