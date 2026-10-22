import { desktopRpc } from '../../lib/runtime/desktop-rpc';
import { authorizeForAltToken } from '../../lib/auth/api-key';

interface ResolverContext { db: D1Database; env: WorkerEnv; auth: { userId: string; tenantId: string }; request?: Request; [key: string]: unknown }
interface WorkerEnv { CHAT_RELAY_DO: DurableObjectNamespace; [key: string]: unknown }

// PROMOTE (desktop migration 026): make this run's workflow the one champion of its
// (repo, phaseType, family).
//
// Named `update-workflow-champion` rather than `promote-workflow` on purpose: lokal infers the
// GraphQL field type from the filename's leading verb, and `promote-` is not one of its mutation
// prefixes, so a `promote-*` resolver would auto-wire as a QUERY and break schema↔resolver
// validation. The name is also the more honest description of what the row change is.
//
// `confirmed` is relayed as given and the host defaults a missing value to FALSE. Promotion is the
// arena's one irreversible, human-confirmed step, so a call that forgot its arguments must refuse,
// not promote — the same lesson as updateAgentPlanTestCommands' silent clear.
//
// Every refusal (NULL or non-zero gateExit, a non-comparable preference, a preference naming a
// different winner, displacing an incumbent with no comparable preference) lives in the host's
// `promotion_block`. None of it is re-implemented here: one home per rule.
export default async function updateWorkflowChampion(
  _parent: unknown,
  args: { runId: string; prefId?: string | null; confirmed: boolean },
  ctx: ResolverContext,
) {
  await authorizeForAltToken(ctx, 'plans:write');
  return desktopRpc<unknown>(ctx, 'update_workflow_champion', {
    runId: args.runId,
    prefId: args.prefId ?? null,
    confirmed: args.confirmed ?? false,
  });
}
