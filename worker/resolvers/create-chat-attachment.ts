interface ResolverContext {
  db: D1Database;
  env: WorkerEnv;
  auth: { userId: string; tenantId: string };
}

interface WorkerEnv {
  R2_ASSETS: R2Bucket;
  [key: string]: unknown;
}

interface ChatAttachmentInput {
  sessionId?: string;
  originalName: string;
  contentType: string;
  dataBase64: string;
}

const MAX_ATTACHMENT_BYTES = 10 * 1024 * 1024;
const ALLOWED_IMAGE_TYPES = new Set(['image/png', 'image/jpeg', 'image/webp', 'image/gif']);

export default async function createChatAttachment(
  _parent: unknown,
  args: { input: ChatAttachmentInput },
  ctx: ResolverContext,
) {
  const originalName = sanitizeOriginalName(args.input.originalName);
  const contentType = args.input.contentType.trim().toLowerCase();
  if (!ALLOWED_IMAGE_TYPES.has(contentType)) {
    throw new Error(`Unsupported image type: ${contentType}`);
  }

  const bytes = decodeBase64(args.input.dataBase64);
  if (bytes.byteLength === 0) {
    throw new Error('Attachment is empty');
  }
  if (bytes.byteLength > MAX_ATTACHMENT_BYTES) {
    throw new Error('Attachment exceeds the 10MB limit');
  }

  const id = crypto.randomUUID();
  const ext = extensionFor(contentType, originalName);
  const r2Key = `tmp/johnnyone-attachments/${ctx.auth.tenantId}/${ctx.auth.userId}/${id}/${originalName || `image${ext}`}`;

  await ctx.env.R2_ASSETS.put(r2Key, bytes, {
    httpMetadata: { contentType },
    customMetadata: {
      attachmentId: id,
      tenantId: ctx.auth.tenantId,
      userId: ctx.auth.userId,
      originalName,
    },
  });

  await ctx.db
    .prepare(
      `INSERT INTO chat_attachments
       (id, tenant_id, user_id, session_id, original_name, content_type, size, r2_key, status)
       VALUES (?, ?, ?, ?, ?, ?, ?, ?, 'uploaded')`,
    )
    .bind(
      id,
      ctx.auth.tenantId,
      ctx.auth.userId,
      args.input.sessionId ?? null,
      originalName,
      contentType,
      bytes.byteLength,
      r2Key,
    )
    .run();

  return {
    id,
    sessionId: args.input.sessionId ?? null,
    originalName,
    contentType,
    size: bytes.byteLength,
    status: 'uploaded',
    uploadedAt: new Date().toISOString(),
    localPath: null,
  };
}

function decodeBase64(value: string): Uint8Array {
  const clean = value.includes(',') ? value.split(',').pop() ?? '' : value;
  const binary = atob(clean);
  const bytes = new Uint8Array(binary.length);
  for (let i = 0; i < binary.length; i += 1) {
    bytes[i] = binary.charCodeAt(i);
  }
  return bytes;
}

function sanitizeOriginalName(value: string): string {
  const name = value.trim().split(/[\\/]/).pop() || 'image';
  return name.replace(/[^a-zA-Z0-9._-]+/g, '-').replace(/^-+|-+$/g, '').slice(0, 120) || 'image';
}

function extensionFor(contentType: string, originalName: string): string {
  if (/\.[a-zA-Z0-9]{1,8}$/.test(originalName)) return '';
  if (contentType === 'image/jpeg') return '.jpg';
  if (contentType === 'image/webp') return '.webp';
  if (contentType === 'image/gif') return '.gif';
  return '.png';
}
