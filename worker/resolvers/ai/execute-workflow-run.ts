import { desktopRpc } from '../../lib/runtime/desktop-rpc';
import { authorizeForAltToken } from '../../lib/auth/api-key';

interface ResolverContext { db: D1Database; env: WorkerEnv; auth: { userId: string; tenantId: string }; request?: Request; [key: string]: unknown }
interface WorkerEnv { CHAT_RELAY_DO: DurableObjectNamespace; [key: string]: unknown }

// Run ONE PHASE of a workflow run on the desktop host (the `execute_workflow_run` RPC).
//
// `verify` and `binary` are relayed ONLY when the caller actually supplied a non-empty string.
// They are deliberately NOT defaulted here: the host owns the resolution rule for which verify
// command and which binary a run executes with, and a second copy of that default in the worker
// would be a second home for it — the two would drift, and the worker's stale copy would silently
// win because it is the one on the wire. An omitted, empty or whitespace-only value therefore sends
// NOTHING for that key (absent, not null), so the host applies its own default.
//
// This EXECUTES work on the host, so it carries plans:write — the same authority as
// `startWorkflowRun` and `runWorkflowRunGate`, and deliberately not a read scope.
export default async function executeWorkflowRun(
  _parent: unknown,
  args: { runId: string; verify?: string | null; binary?: string | null },
  ctx: ResolverContext,
) {
  await authorizeForAltToken(ctx, 'plans:write');
  const params: Record<string, unknown> = { runId: args.runId };
  // Non-empty after trimming: a whitespace-only string is a caller that meant "unset", and
  // relaying it would hand the host a command that is not its default and is not runnable.
  if (typeof args.verify === 'string' && args.verify.trim() !== '') {
    params.verify = args.verify;
  }
  if (typeof args.binary === 'string' && args.binary.trim() !== '') {
    params.binary = args.binary;
  }
  return desktopRpc<unknown>(ctx, 'execute_workflow_run', params);
}