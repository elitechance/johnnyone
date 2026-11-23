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

// Plans tab (initiative 8ad3ae71, phase 01): New draft plan: briefing Initiative + plan-agent session + v1 seeded from the ask. Spawns nothing.
// Thin pass-through to the host `create_draft_plan` over the relay, gated by the plans:write scope.
export default async function createDraftPlan(
  _parent: unknown,
  args: { input: { title: string; workspacePath: string; ask?: string | null } },
  ctx: ResolverContext,
) {
  await authorizeForAltToken(ctx, 'plans:write');
  return desktopRpc<unknown>(ctx, 'create_draft_plan', { title: args.input.title, workspacePath: args.input.workspacePath, ask: args.input.ask });
}
