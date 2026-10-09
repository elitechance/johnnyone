import { readCachedRpc, writeCachedRpc, invalidateCachedRpc } from './desktop-rpc-cache';
import { resolveOnlineNode } from '../auth/resolve-online-node';
import { requireIdentity } from '../auth/require-identity';

interface DesktopRpcEnv {
  CHAT_RELAY_DO: DurableObjectNamespace;
  [key: string]: unknown;
}

interface DesktopRpcContext {
  db: D1Database;
  env: DesktopRpcEnv;
  auth: { userId: string; tenantId: string };
}

interface DesktopRpcResult<T> {
  success?: boolean;
  data?: T;
  error?: string;
  timedOut?: boolean;
}

export async function desktopRpc<T>(
  ctx: DesktopRpcContext,
  method: string,
  params: Record<string, unknown> = {},
): Promise<T> {
  const identity = await requireIdentity(ctx as any);

  const cached = readCachedRpc<T>(identity, method, params);
  if (cached !== null) {
    return cached;
  }

  const node = await resolveOnlineNode(ctx.db, identity);

  if (!node) {
    throw new Error('No online backend app found. Start the backend app and connect it to the Worker.');
  }

  const doId = ctx.env.CHAT_RELAY_DO.idFromName(node.id);
  const doStub = ctx.env.CHAT_RELAY_DO.get(doId);
  const response = await doStub.fetch('https://internal/relay-rpc', {
    method: 'POST',
    headers: { 'Content-Type': 'application/json' },
    body: JSON.stringify({ method, params }),
  });

  const result = (await response.json()) as DesktopRpcResult<T>;
  if (!response.ok || result.timedOut || result.success === false) {
    throw new Error(result.error || `Backend RPC failed: ${method}`);
  }

  const data = result.data as T;
  // A successful WRITE makes the related cached reads stale (e.g. `archive_session` → `list_sessions`).
  // Done here, in the one seam every session resolver already calls, rather than repeated in each
  // resolver — the same reason the `AiSession` selection set got hoisted: duplicated knowledge drifts.
  invalidateCachedRpc(identity, method);
  writeCachedRpc(identity, method, params, data);
  return data;
}
