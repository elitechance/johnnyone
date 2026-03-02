interface ResolverContext {
  db: D1Database;
  env: WorkerEnv;
  auth: { userId: string; tenantId: string };
}

interface WorkerEnv {
  AGENT_SESSION_DO: DurableObjectNamespace;
  PUBSUB_DO: DurableObjectNamespace;
  [key: string]: unknown;
}

interface AiSessionRow {
  id: string;
  tenant_id: string;
  user_id: string;
  title: string | null;
  provider: string | null;
  model: string | null;
  status: string;
  system_prompt: string | null;
  total_input_tokens: number;
  total_output_tokens: number;
  total_cost: number;
  is_deleted: number;
  created_at: string;
  updated_at: string;
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

function mapSessionRow(row: AiSessionRow) {
  return {
    id: row.id,
    tenantId: row.tenant_id,
    userId: row.user_id,
    title: row.title,
    provider: row.provider,
    model: row.model,
    status: row.status,
    systemPrompt: row.system_prompt,
    totalInputTokens: row.total_input_tokens,
    totalOutputTokens: row.total_output_tokens,
    totalCost: row.total_cost,
    createdAt: row.created_at,
    updatedAt: row.updated_at,
  };
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

export default async function getAiSession(
  parent: unknown,
  args: { id: string },
  ctx: ResolverContext,
) {
  const session = await ctx.db
    .prepare(
      `SELECT * FROM ai_sessions WHERE id = ? AND tenant_id = ? AND is_deleted = 0`,
    )
    .bind(args.id, ctx.auth.tenantId)
    .first<AiSessionRow>();

  if (!session) {
    return null;
  }

  const messagesResult = await ctx.db
    .prepare(
      `SELECT * FROM ai_messages WHERE session_id = ? AND tenant_id = ? ORDER BY created_at ASC`,
    )
    .bind(args.id, ctx.auth.tenantId)
    .all<AiMessageRow>();

  const mapped = mapSessionRow(session);

  return {
    ...mapped,
    messages: messagesResult.results.map(mapMessageRow),
  };
}
