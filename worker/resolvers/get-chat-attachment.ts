import { requireIdentity } from '../lib/auth/require-identity';

interface ResolverContext {
  db: D1Database;
  env: WorkerEnv;
  auth: { userId: string; tenantId: string };
}

interface WorkerEnv {
  R2_ASSETS: R2Bucket;
  [key: string]: unknown;
}

interface ChatAttachmentRow {
  id: string;
  original_name: string;
  content_type: string;
  size: number;
  r2_key: string;
  status: string;
}

export default async function getChatAttachment(
  _parent: unknown,
  args: { id: string },
  ctx: ResolverContext,
) {
  const idn = await requireIdentity(ctx as any);
  const row = await ctx.db
    .prepare(
      `SELECT id, original_name, content_type, size, r2_key, status
       FROM chat_attachments
       WHERE id = ? AND tenant_id = ? AND user_id = ?`,
    )
    .bind(args.id, idn.tenantId, idn.userId)
    .first<ChatAttachmentRow>();

  if (!row) {
    throw new Error('Attachment not found');
  }
  if (row.status !== 'uploaded') {
    throw new Error(`Attachment is not available for download: ${row.status}`);
  }

  const object = await ctx.env.R2_ASSETS.get(row.r2_key);
  if (!object) {
    throw new Error('Attachment object is missing from R2');
  }

  return {
    id: row.id,
    originalName: row.original_name,
    contentType: row.content_type,
    size: row.size,
    dataBase64: encodeBase64(new Uint8Array(await object.arrayBuffer())),
  };
}

function encodeBase64(bytes: Uint8Array): string {
  let binary = '';
  const chunkSize = 0x8000;
  for (let i = 0; i < bytes.length; i += chunkSize) {
    const chunk = bytes.subarray(i, i + chunkSize);
    binary += String.fromCharCode(...chunk);
  }
  return btoa(binary);
}
