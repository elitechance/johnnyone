export interface SocketAuth {
  tenantId: string;
  userId: string;
  clientType: 'api' | 'mobile' | 'desktop';
  apiKey?: { id: string; scopes: string[] };
}

/**
 * getSocketAuth
 * Reconstructs the authenticated identity attached at upgrade time.
 * Prefers structured attachment when present, falls back to tags.
 * Used by webSocketMessage and (in Phase 01) the ownership gate.
 */
export function getSocketAuth(ws: WebSocket, state: DurableObjectState): SocketAuth | null {
  // Try attachment (set via serializeAttachment on the server side during accept)
  // Prefer standard deserializeAttachment; avoid non-standard .attachment
  try {
    const direct = (ws as any).deserializeAttachment?.();
    if (direct && direct.tenantId && direct.userId) {
      return {
        tenantId: direct.tenantId,
        userId: direct.userId,
        clientType: (direct.clientType as any) || 'api',
        apiKey: direct.apiKey,
      };
    }
  } catch {}

  // Fallback to tags (always present for api + desktop + mobile)
  const tags = state.getTags(ws) || [];
  let tenantId = '';
  let userId = '';
  let clientType: SocketAuth['clientType'] = 'mobile';

  for (const t of tags) {
    if (t === 'desktop') clientType = 'desktop';
    if (t === 'api') clientType = 'api';
    if (t.startsWith('tenant:')) tenantId = t.slice(7);
    if (t.startsWith('user:')) userId = t.slice(5);
  }

  if (tenantId && userId) {
    return { tenantId, userId, clientType };
  }
  return null;
}
