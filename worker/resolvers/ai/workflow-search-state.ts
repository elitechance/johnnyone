import { desktopRpc } from '../../lib/runtime/desktop-rpc';
import { authorizeForAltToken } from '../../lib/auth/api-key';

interface ResolverContext { db: D1Database; env: WorkerEnv; auth: { userId: string; tenantId: string }; request?: Request; [key: string]: unknown }
interface WorkerEnv { CHAT_RELAY_DO: DurableObjectNamespace; [key: string]: unknown }

// The halt, as data (desktop migration 026). Three consecutive rejections for one
// (repo, phaseType, family) halt the search; the host both reports that here and enforces it on
// startWorkflowRun, so a client never has to count rejections itself.
export default async function workflowSearchState(
  _parent: unknown,
  args: { repo: string; phaseType: string; family: string },
  ctx: ResolverContext,
) {
  await authorizeForAltToken(ctx, 'plans:read');
  return desktopRpc<unknown>(ctx, 'get_workflow_search_state', {
    repo: args.repo,
    phaseType: args.phaseType,
    family: args.family,
  });
}
