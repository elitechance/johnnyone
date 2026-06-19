import { readCachedRpc, writeCachedRpc } from './desktop-rpc-cache';
import { resolveOnlineNode } from '../auth/resolve-online-node';

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
  const cached = readCachedRpc<T>(ctx.auth, method, params);
  if (cached !== null) {
    return cached;
  }

  const node = await resolveOnlineNode(ctx.db, ctx.auth);

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
  writeCachedRpc(ctx.auth, method, params, data);
  return data;
}
