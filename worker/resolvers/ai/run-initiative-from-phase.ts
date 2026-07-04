import { desktopRpc } from '../../lib/runtime/desktop-rpc';
import { authorizeForAltToken } from '../../lib/auth/api-key';

interface ResolverContext { db: D1Database; env: WorkerEnv; auth: { userId: string; tenantId: string }; request?: Request; [key: string]: unknown }
interface WorkerEnv { CHAT_RELAY_DO: DurableObjectNamespace; [key: string]: unknown }

export default async function runInitiativeFromPhase(
  _parent: unknown,
  args: { id: string; phaseId?: string | null; phaseRunMode?: string | null; comment?: string | null },
  ctx: ResolverContext,
) {
  await authorizeForAltToken(ctx, 'plans:write');
  return desktopRpc<unknown>(ctx, 'run_initiative_from_phase', {
    id: args.id,
    phaseId: args.phaseId ?? null,
    phaseRunMode: args.phaseRunMode ?? null,
    comment: args.comment ?? null,
  });
}
