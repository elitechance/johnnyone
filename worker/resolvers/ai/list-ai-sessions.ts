interface ResolverContext {
  db: D1Database;
  env: WorkerEnv;
  auth: { userId: string; tenantId: string };
}

interface WorkerEnv {
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
  created_at: string;
  updated_at: string;
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

export default async function listAiSessions(
  parent: unknown,
  args: { status?: string; limit?: number; offset?: number },
  ctx: ResolverContext,
) {
  const limit = args.limit ?? 20;
  const offset = args.offset ?? 0;

  let sql = `SELECT * FROM ai_sessions WHERE tenant_id = ? AND user_id = ? AND is_deleted = 0`;
  const bindings: (string | number)[] = [ctx.auth.tenantId, ctx.auth.userId];

  if (args.status) {
    sql += ` AND status = ?`;
    bindings.push(args.status);
  }

  sql += ` ORDER BY updated_at DESC LIMIT ? OFFSET ?`;
  bindings.push(limit, offset);

  const result = await ctx.db
    .prepare(sql)
    .bind(...bindings)
    .all<AiSessionRow>();

  return result.results.map(mapSessionRow);
}
