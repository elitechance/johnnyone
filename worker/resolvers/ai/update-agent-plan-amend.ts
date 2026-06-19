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
 * Amend an existing approved planning run.
 *
 * The user clicks "Amend" on an approved plan and types a brief describing
 * what they want to add or change. The host stashes the brief on the
 * agent_plans row + switches T1 into "edit mode" — instead of recreating the
 * plan from scratch, T1 reads the existing plan + the per-plan git history
 * and edits files in place. T2 then reviews the resulting `git diff HEAD`.
 *
 * On T2 PASS, the host commits the amended state as a new commit on the
 * plan's `main` branch (message: `amend: <brief excerpt>`) and clears the
 * amend_brief field.
 */
export default async function amendAgentPlan(
  _parent: unknown,
  args: { id: string; brief: string },
  ctx: ResolverContext,
) {
  await authorizeForAltToken(ctx, 'plans:write');
  return desktopRpc<unknown>(ctx, 'amend_agent_plan', {
    id: args.id,
    brief: args.brief,
  });
}
