import { desktopRpc } from '../../lib/runtime/desktop-rpc';
import { authorizeForAltToken } from '../../lib/auth/api-key';

interface ResolverContext { db: D1Database; env: WorkerEnv; auth: { userId: string; tenantId: string }; request?: Request; [key: string]: unknown }
interface WorkerEnv { CHAT_RELAY_DO: DurableObjectNamespace; [key: string]: unknown }

// Repair the DEVELOPMENT-stage provider binding on a planning row — the two columns
// `auto_start_development` feeds back into `create_plan` as the live providers. Mirrors
// updateAgentPlanValidationConfig: a null argument CLEARS that side back to SQL NULL, which means
// "inherit the planning row's own provider". The desktop validates both values with the same
// `reject_oneshot_plan_provider` creation uses (and refuses a development row), before writing.
export default async function updateAgentPlanDevProviders(
  _parent: unknown,
  args: { id: string; devWorkerProvider?: string | null; devReviewerProvider?: string | null },
  ctx: ResolverContext,
) {
  await authorizeForAltToken(ctx, 'plans:write');
  return desktopRpc<unknown>(ctx, 'update_agent_plan_dev_providers', {
    id: args.id,
    devWorkerProvider: args.devWorkerProvider ?? null,
    devReviewerProvider: args.devReviewerProvider ?? null,
  });
}