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

export default async function updateAiSessionTitle(
  parent: unknown,
  args: { id: string; title: string },
  ctx: ResolverContext,
) {
  const existing = await ctx.db
    .prepare(
      `SELECT id FROM ai_sessions WHERE id = ? AND tenant_id = ? AND is_deleted = 0`,
    )
    .bind(args.id, ctx.auth.tenantId)
    .first();

  if (!existing) {
    throw new Error('Session not found');
  }

  await ctx.db
    .prepare(
      `UPDATE ai_sessions SET title = ?, updated_at = datetime('now') WHERE id = ? AND tenant_id = ?`,
    )
    .bind(args.title, args.id, ctx.auth.tenantId)
    .run();

  const session = await ctx.db
    .prepare(`SELECT * FROM ai_sessions WHERE id = ?`)
    .bind(args.id)
    .first<AiSessionRow>();

  if (!session) {
    throw new Error('Failed to update session title');
  }

  return {
    id: session.id,
    tenantId: session.tenant_id,
    userId: session.user_id,
    title: session.title,
    provider: session.provider,
    model: session.model,
    status: session.status,
    systemPrompt: session.system_prompt,
    totalInputTokens: session.total_input_tokens,
    totalOutputTokens: session.total_output_tokens,
    totalCost: session.total_cost,
    createdAt: session.created_at,
    updatedAt: session.updated_at,
  };
}
