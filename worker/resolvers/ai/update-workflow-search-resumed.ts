import { desktopRpc } from '../../lib/runtime/desktop-rpc';
import { authorizeForAltToken } from '../../lib/auth/api-key';

interface ResolverContext { db: D1Database; env: WorkerEnv; auth: { userId: string; tenantId: string }; request?: Request; [key: string]: unknown }
interface WorkerEnv { CHAT_RELAY_DO: DurableObjectNamespace; [key: string]: unknown }

// RESUME (desktop migration 026): lift a halt on one (repo, phaseType, family). The ONLY thing that
// resets the rejection streak.
//
// It exists because a PROMOTION used to be what cleared a halt, which made the halt escapable with
// no new evidence at all: re-promoting the sitting champion was a row-level no-op that still
// appended a `promote` decision and reset the count, and promoting the very run a human had
// rejected three times overturned the search's own refusal using the operation it was refusing.
// Clearing a halt is now its own explicit, auditable decision, recorded next to the three refusals.
//
// Named `update-workflow-search-resumed` because `resume-` is not one of lokal's mutation filename
// prefixes — a `resume-*` resolver would auto-wire as a Query and break schema↔resolver validation.
//
// `confirmed` is relayed as given and the host defaults a missing value to false; `reason` is
// mandatory and the host refuses a blank one. The host also refuses a resume when the family is not
// halted, because a resume with nothing to resume is a free streak reset.
export default async function updateWorkflowSearchResumed(
  _parent: unknown,
  args: { repo: string; phaseType: string; family: string; reason: string; confirmed: boolean },
  ctx: ResolverContext,
) {
  await authorizeForAltToken(ctx, 'plans:write');
  return desktopRpc<unknown>(ctx, 'update_workflow_search_resumed', {
    repo: args.repo,
    phaseType: args.phaseType,
    family: args.family,
    reason: args.reason,
    confirmed: args.confirmed ?? false,
  });
}
