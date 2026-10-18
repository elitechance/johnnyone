import { desktopRpc } from '../../lib/runtime/desktop-rpc';
import { authorizeForAltToken } from '../../lib/auth/api-key';

interface ResolverContext { db: D1Database; env: WorkerEnv; auth: { userId: string; tenantId: string }; request?: Request; [key: string]: unknown }
interface WorkerEnv { CHAT_RELAY_DO: DurableObjectNamespace; [key: string]: unknown }

// A workflow's runs, newest first (desktop migration 026). A missing workflow is an error from the
// host, not an empty array: "no runs yet" and "no such workflow" are different answers.
export default async function workflowRuns(
  _parent: unknown,
  args: { workflowId: string },
  ctx: ResolverContext,
) {
  await authorizeForAltToken(ctx, 'plans:read');
  return desktopRpc<unknown[]>(ctx, 'list_workflow_runs', { workflowId: args.workflowId });
}
