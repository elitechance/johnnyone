import { desktopRpc } from '../../lib/runtime/desktop-rpc';
import { authorizeForAltToken } from '../../lib/auth/api-key';

interface ResolverContext { db: D1Database; env: WorkerEnv; auth: { userId: string; tenantId: string }; request?: Request; [key: string]: unknown }
interface WorkerEnv { CHAT_RELAY_DO: DurableObjectNamespace; [key: string]: unknown }

// Create a workflow version from the four prompt parts (desktop migration 026).
//
// `version` is NOT an argument: the host assigns it inside the same transaction as the insert, one
// past the highest in (repo, phaseType, family). A client-supplied version would let two concurrent
// creates claim the same number, and the schema's only uniqueness claim is one champion per family.
//
// Nor is `isChampion`: a new version is never born champion. That is updateWorkflowChampion's job
// and it needs a green gate and a human.
//
// And nor is `repo`: the host derives the repo identity from `workspacePath`
// (`services::workflows::repo_identity`). The client used to send `basename(workspace_path)`, so two
// unrelated checkouts called johnnyone shared one champion — a client-supplied identity is a
// client-supplied champion key. `phaseType` is validated against a closed set for the same reason.
export default async function createWorkflowVersion(
  _parent: unknown,
  args: {
    workspacePath: string;
    phaseType: string;
    family: string;
    parentId?: string | null;
    role: string;
    instruction: string;
    contract: string;
    hop: string;
    fanOut: number;
  },
  ctx: ResolverContext,
) {
  await authorizeForAltToken(ctx, 'plans:write');
  return desktopRpc<unknown>(ctx, 'create_workflow_version', {
    workspacePath: args.workspacePath,
    phaseType: args.phaseType,
    family: args.family,
    parentId: args.parentId ?? null,
    role: args.role,
    instruction: args.instruction,
    contract: args.contract,
    hop: args.hop,
    fanOut: args.fanOut,
  });
}
