/**
 * resolveOnlineNode
 * Given verified {tenantId, userId}, return the caller's current online desktop node id
 * or null. Query shape exactly mirrors relay-rpc.ts:58-66.
 */
export async function resolveOnlineNode(
  db: D1Database,
  auth: { tenantId: string; userId: string }
): Promise<{ id: string } | null> {
  const row = await db
    .prepare(
      `SELECT id FROM desktop_nodes
       WHERE tenant_id = ? AND user_id = ? AND status = 'online' AND is_deleted = 0
       ORDER BY last_heartbeat_at DESC LIMIT 1`
    )
    .bind(auth.tenantId, auth.userId)
    .first<{ id: string }>();

  return row ?? null;
}
