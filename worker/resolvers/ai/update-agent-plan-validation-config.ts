import { desktopRpc } from '../../lib/runtime/desktop-rpc';
import { authorizeForAltToken } from '../../lib/auth/api-key';

interface ResolverContext { db: D1Database; env: WorkerEnv; auth: { userId: string; tenantId: string }; request?: Request; [key: string]: unknown }
interface WorkerEnv { CHAT_RELAY_DO: DurableObjectNamespace; [key: string]: unknown }

// Overhaul phase 7 (D1). Persist the Initiative's ordered validation-lens array. Mirrors
// updateAgentPlanAppScope: a null/empty `config` clears it (→ desktop resolves the default
// product/qa/lead template). The desktop validates the JSON (parse + provider, D13) before writing.
export default async function updateAgentPlanValidationConfig(
  _parent: unknown,
  args: { id: string; config?: string | null },
  ctx: ResolverContext,
) {
  await authorizeForAltToken(ctx, 'plans:write');
  return desktopRpc<unknown>(ctx, 'update_agent_plan_validation_config', {
    id: args.id,
    config: args.config ?? null,
  });
}
