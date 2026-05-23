interface ResolverContext {
  db: D1Database;
  env: WorkerEnv;
  auth: { userId: string; tenantId: string };
}

interface WorkerEnv {
  R2_ASSETS: R2Bucket;
  [key: string]: unknown;
}

interface MarkChatAttachmentDeliveredInput {
  id: string;
  localPath: string;
}

interface ChatAttachmentRow {
  id: string;
  session_id: string | null;
  original_name: string;
  content_type: string;
  size: number;
  r2_key: string;
  status: string;
  uploaded_at: string;
  local_path: string | null;
}

export default async function deleteChatAttachment(
  _parent: unknown,
  args: { input: MarkChatAttachmentDeliveredInput },
  ctx: ResolverContext,
) {
  const row = await ctx.db
    .prepare(
      `SELECT id, session_id, original_name, content_type, size, r2_key, status, uploaded_at, local_path
       FROM chat_attachments
       WHERE id = ? AND tenant_id = ? AND user_id = ?`,
    )
    .bind(args.input.id, ctx.auth.tenantId, ctx.auth.userId)
    .first<ChatAttachmentRow>();

  if (!row) {
    throw new Error('Attachment not found');
  }

  if (row.status === 'uploaded') {
    await ctx.env.R2_ASSETS.delete(row.r2_key);
  }

  await ctx.db
    .prepare(
      `UPDATE chat_attachments
       SET status = 'deleted', local_path = ?, local_saved_at = datetime('now'), deleted_at = datetime('now')
       WHERE id = ? AND tenant_id = ? AND user_id = ?`,
    )
    .bind(args.input.localPath, args.input.id, ctx.auth.tenantId, ctx.auth.userId)
    .run();

  return {
    id: row.id,
    sessionId: row.session_id,
    originalName: row.original_name,
    contentType: row.content_type,
    size: row.size,
    status: 'deleted',
    uploadedAt: row.uploaded_at,
    localPath: args.input.localPath,
  };
}
