import { readCachedRpc, writeCachedRpc } from './desktop-rpc-cache';
import { resolveOnlineNode } from '../auth/resolve-online-node';

/**
 * Relay-RPC helper — replaces the legacy `hostGraphqlRequest` forward path.
 *
 * Flow:
 *   1. Resolve the authenticated user's online desktop_node from D1.
 *   2. Get the ChatRelayDO instance for that node.
 *   3. POST `{method, params}` to the DO's `/relay-rpc` endpoint.
 *   4. The DO forwards the RPC over WebSocket to the desktop host, waits up to 15s
 *      for the host's `rpc_response` envelope, and returns the result.
 *
 * Caller-side errors:
 *   - No online node → "No online desktop node found"
 *   - Desktop RPC failed/timed out → message from the host (or "Desktop RPC timed out")
 *
 * This file replaces `worker/lib/runtime/host-graphql.ts` — single-tenant friendly
 * but doesn't scale to multi-user. See Phase 2 of the multi-user-saas plan.
 */

export interface RelayRpcEnv {
  CHAT_RELAY_DO: DurableObjectNamespace;
  [key: string]: unknown;
}

export interface RelayRpcAuth {
  userId: string;
  tenantId: string;
}

export interface RelayRpcContext {
  db: D1Database;
  env: RelayRpcEnv;
  auth: RelayRpcAuth;
}

interface RelayRpcResponse<T> {
  success?: boolean;
  data?: T;
  error?: string;
  timedOut?: boolean;
}

/**
 * Invoke `method` on the authenticated user's host via ChatRelayDO `/relay-rpc`.
 * Returns the host's response data, throws on missing node / RPC error / timeout.
 */
export async function relayRpc<T>(
  ctx: RelayRpcContext,
  method: string,
  params: Record<string, unknown> = {},
): Promise<T> {
  const cached = readCachedRpc<T>(ctx.auth, method, params);
  if (cached !== null) {
    return cached;
  }

  const node = await resolveOnlineNode(ctx.db, ctx.auth);

  if (!node) {
    throw new Error(
      'No online desktop node found. Please ensure your desktop app is running and connected.',
    );
  }

  const doId = ctx.env.CHAT_RELAY_DO.idFromName(node.id);
  const doStub = ctx.env.CHAT_RELAY_DO.get(doId);

  const response = await doStub.fetch('https://internal/relay-rpc', {
    method: 'POST',
    headers: { 'Content-Type': 'application/json' },
    body: JSON.stringify({ method, params }),
  });

  if (!response.ok) {
    if (response.status === 503) {
      throw new Error('Desktop not connected. Please ensure your desktop app is running.');
    }
    if (response.status === 504) {
      throw new Error('Desktop RPC timed out');
    }
    throw new Error(`Relay-RPC request failed: ${response.status} ${response.statusText}`);
  }

  const body = (await response.json()) as RelayRpcResponse<T>;

  if (body.timedOut) {
    throw new Error('Desktop RPC timed out');
  }
  if (body.success === false) {
    throw new Error(body.error ?? 'Desktop RPC failed');
  }

  const data = body.data as T;
  writeCachedRpc(ctx.auth, method, params, data);
  return data;
}
