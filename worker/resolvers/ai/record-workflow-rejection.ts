import { desktopRpc } from '../../lib/runtime/desktop-rpc';
import { authorizeForAltToken } from '../../lib/auth/api-key';

interface ResolverContext { db: D1Database; env: WorkerEnv; auth: { userId: string; tenantId: string }; request?: Request; [key: string]: unknown }
interface WorkerEnv { CHAT_RELAY_DO: DurableObjectNamespace; [key: string]: unknown }

// REJECT (desktop migration 026): saying no to a run, with a reason.
//
// Named `record-workflow-rejection` because `reject-` is not one of lokal's mutation filename
// prefixes — a `reject-*` resolver would auto-wire as a Query.
//
// The reason is mandatory in the schema and the host refuses a blank one too: three consecutive
// rejections in a (repo, phaseType, family) HALT the search, and the reasons are the only thing a
// halted search has to show a human. Returns the resulting WorkflowSearchState so the caller holds
// the halt as data rather than having to ask again.
export default async function recordWorkflowRejection(
  _parent: unknown,
  args: { runId: string; reason: string },
  ctx: ResolverContext,
) {
  await authorizeForAltToken(ctx, 'plans:write');
  return desktopRpc<unknown>(ctx, 'record_workflow_rejection', {
    runId: args.runId,
    reason: args.reason,
  });
}
