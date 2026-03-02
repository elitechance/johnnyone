interface ResolverContext {
  db: D1Database;
  env: WorkerEnv;
  auth: { userId: string; tenantId: string };
}

interface WorkerEnv {
  [key: string]: unknown;
}

interface UsageTotalRow {
  total_input_tokens: number;
  total_output_tokens: number;
  total_tokens: number;
  total_cost: number;
  session_count: number;
  message_count: number;
}

interface ProviderUsageRow {
  provider: string;
  model: string;
  input_tokens: number;
  output_tokens: number;
  total_tokens: number;
  cost: number;
}

export default async function getAiUsageSummary(
  parent: unknown,
  args: { from?: string; to?: string },
  ctx: ResolverContext,
) {
  const fromDate = args.from ?? '1970-01-01T00:00:00Z';
  const toDate = args.to ?? '9999-12-31T23:59:59Z';

  const totals = await ctx.db
    .prepare(
      `SELECT
        COALESCE(SUM(input_tokens), 0) as total_input_tokens,
        COALESCE(SUM(output_tokens), 0) as total_output_tokens,
        COALESCE(SUM(total_tokens), 0) as total_tokens,
        COALESCE(SUM(cost), 0) as total_cost,
        COUNT(DISTINCT session_id) as session_count,
        COUNT(*) as message_count
      FROM ai_usage_log
      WHERE tenant_id = ? AND user_id = ? AND created_at >= ? AND created_at <= ?`,
    )
    .bind(ctx.auth.tenantId, ctx.auth.userId, fromDate, toDate)
    .first<UsageTotalRow>();

  const byProviderResult = await ctx.db
    .prepare(
      `SELECT
        provider,
        model,
        COALESCE(SUM(input_tokens), 0) as input_tokens,
        COALESCE(SUM(output_tokens), 0) as output_tokens,
        COALESCE(SUM(total_tokens), 0) as total_tokens,
        COALESCE(SUM(cost), 0) as cost
      FROM ai_usage_log
      WHERE tenant_id = ? AND user_id = ? AND created_at >= ? AND created_at <= ?
      GROUP BY provider, model
      ORDER BY cost DESC`,
    )
    .bind(ctx.auth.tenantId, ctx.auth.userId, fromDate, toDate)
    .all<ProviderUsageRow>();

  return {
    totalInputTokens: totals?.total_input_tokens ?? 0,
    totalOutputTokens: totals?.total_output_tokens ?? 0,
    totalTokens: totals?.total_tokens ?? 0,
    totalCost: totals?.total_cost ?? 0,
    sessionCount: totals?.session_count ?? 0,
    messageCount: totals?.message_count ?? 0,
    byProvider: byProviderResult.results.map((row) => ({
      provider: row.provider,
      model: row.model,
      inputTokens: row.input_tokens,
      outputTokens: row.output_tokens,
      totalTokens: row.total_tokens,
      cost: row.cost,
    })),
  };
}
