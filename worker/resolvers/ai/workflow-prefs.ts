import { desktopRpc } from '../../lib/runtime/desktop-rpc';
import { authorizeForAltToken } from '../../lib/auth/api-key';

interface ResolverContext { db: D1Database; env: WorkerEnv; auth: { userId: string; tenantId: string }; request?: Request; [key: string]: unknown }
interface WorkerEnv { CHAT_RELAY_DO: DurableObjectNamespace; [key: string]: unknown }

// Every preference involving this workflow's runs (desktop migration 026).
//
// Added because nothing could read a preference back: a client that recorded one and then reloaded
// could not recover the `prefId` that `updateWorkflowChampion` requires, so the only route forward
// was to record it again — duplicate rows for one head-to-head, in the table the design calls the
// project-level measurement. `recordWorkflowPref` is idempotent per ordered pair now; this is the
// read half, so discovering an existing preference does not require issuing a write.
//
// A missing workflow is an error from the host, not an empty array: "no preferences yet" and "no
// such workflow" are different answers.
export default async function workflowPrefs(
  _parent: unknown,
  args: { workflowId: string },
  ctx: ResolverContext,
) {
  await authorizeForAltToken(ctx, 'plans:read');
  return desktopRpc<unknown[]>(ctx, 'list_workflow_prefs', { workflowId: args.workflowId });
}
