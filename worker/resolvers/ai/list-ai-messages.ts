interface ResolverContext {
  db: D1Database;
  env: WorkerEnv;
  auth: { userId: string; tenantId: string };
}

interface WorkerEnv {
  [key: string]: unknown;
}

interface AiMessageRow {
  id: string;
  tenant_id: string;
  session_id: string;
  role: string;
  content: string | null;
  tool_calls: string;
  tool_call_id: string | null;
  source_channel: string | null;
  finish_reason: string | null;
  input_tokens: number | null;
  output_tokens: number | null;
  created_at: string;
}

function mapMessageRow(row: AiMessageRow) {
  return {
    id: row.id,
    tenantId: row.tenant_id,
    sessionId: row.session_id,
    role: row.role,
    content: row.content,
    toolCalls: row.tool_calls,
    toolCallId: row.tool_call_id,
    sourceChannel: row.source_channel,
    finishReason: row.finish_reason,
    inputTokens: row.input_tokens,
    outputTokens: row.output_tokens,
    createdAt: row.created_at,
  };
}

export default async function listAiMessages(
  parent: unknown,
  args: { sessionId: string; limit?: number; offset?: number },
  ctx: ResolverContext,
) {
  const limit = args.limit ?? 50;
  const offset = args.offset ?? 0;

  // Verify session belongs to tenant
  const session = await ctx.db
    .prepare(
      `SELECT id FROM ai_sessions WHERE id = ? AND tenant_id = ? AND is_deleted = 0`,
    )
    .bind(args.sessionId, ctx.auth.tenantId)
    .first();

  if (!session) {
    throw new Error('Session not found');
  }

  const result = await ctx.db
    .prepare(
      `SELECT * FROM ai_messages WHERE session_id = ? AND tenant_id = ? ORDER BY created_at ASC LIMIT ? OFFSET ?`,
    )
    .bind(args.sessionId, ctx.auth.tenantId, limit, offset)
    .all<AiMessageRow>();

  return result.results.map(mapMessageRow);
}
