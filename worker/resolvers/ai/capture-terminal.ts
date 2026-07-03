import { desktopRpc } from '../../lib/runtime/desktop-rpc';
import { authorizeForAltToken } from '../../lib/auth/api-key';

interface ResolverContext { db: D1Database; env: WorkerEnv; auth: { userId: string; tenantId: string } }
interface WorkerEnv { CHAT_RELAY_DO: DurableObjectNamespace; [key: string]: unknown }

// Deterministic one-shot terminal capture (overhaul P2, decision D7): thin pass-through to the host
// `capture_terminal` relay-RPC. No new shell transport — input still rides the terminal_command
// envelope; this only reads the current pane snapshot. Gated by the terminal:read scope.
export default async function captureTerminal(
  _parent: unknown,
  args: { sessionId: string; historyLines?: number | null },
  ctx: ResolverContext,
) {
  await authorizeForAltToken(ctx, 'terminal:read');
  return desktopRpc<unknown>(ctx, 'capture_terminal', {
    sessionId: args.sessionId,
    historyLines: args.historyLines,
  });
}
