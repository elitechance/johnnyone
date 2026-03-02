interface ResolverContext {
  db: D1Database;
  env: WorkerEnv;
  auth: { userId: string; tenantId: string };
}

interface WorkerEnv {
  [key: string]: unknown;
}

interface CreateAiSessionInput {
  title?: string;
  provider?: string;
  model?: string;
  systemPrompt?: string;
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

export default async function createAiSession(
  parent: unknown,
  args: { input: CreateAiSessionInput },
  ctx: ResolverContext,
) {
  const id = crypto.randomUUID();
  const { title, provider, model, systemPrompt } = args.input;

  await ctx.db
    .prepare(
      `INSERT INTO ai_sessions (id, tenant_id, user_id, title, provider, model, status, system_prompt, total_input_tokens, total_output_tokens, total_cost, is_deleted, created_at, updated_at)
       VALUES (?, ?, ?, ?, ?, ?, 'active', ?, 0, 0, 0, 0, datetime('now'), datetime('now'))`,
    )
    .bind(
      id,
      ctx.auth.tenantId,
      ctx.auth.userId,
      title ?? null,
      provider ?? null,
      model ?? null,
      systemPrompt ?? null,
    )
    .run();

  const session = await ctx.db
    .prepare(`SELECT * FROM ai_sessions WHERE id = ?`)
    .bind(id)
    .first<AiSessionRow>();

  if (!session) {
    throw new Error('Failed to create session');
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
