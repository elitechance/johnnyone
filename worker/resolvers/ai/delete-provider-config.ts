interface ResolverContext {
  db: D1Database;
  env: WorkerEnv;
  auth: { userId: string; tenantId: string };
}

interface WorkerEnv {
  [key: string]: unknown;
}

interface ProviderConfigRow {
  id: string;
  tenant_id: string;
  user_id: string;
  provider: string;
  model: string | null;
  api_key_ref: string | null;
  base_url: string | null;
  is_default: number;
  settings: string;
  is_deleted: number;
  created_at: string;
  updated_at: string;
}

export default async function deleteProviderConfig(
  parent: unknown,
  args: { id: string },
  ctx: ResolverContext,
) {
  const existing = await ctx.db
    .prepare(
      `SELECT id FROM ai_provider_configs WHERE id = ? AND tenant_id = ? AND user_id = ? AND is_deleted = 0`,
    )
    .bind(args.id, ctx.auth.tenantId, ctx.auth.userId)
    .first();

  if (!existing) {
    throw new Error('Provider config not found');
  }

  await ctx.db
    .prepare(
      `UPDATE ai_provider_configs SET is_deleted = 1, is_default = 0, updated_at = datetime('now') WHERE id = ? AND tenant_id = ?`,
    )
    .bind(args.id, ctx.auth.tenantId)
    .run();

  const config = await ctx.db
    .prepare(`SELECT * FROM ai_provider_configs WHERE id = ?`)
    .bind(args.id)
    .first<ProviderConfigRow>();

  if (!config) {
    throw new Error('Failed to delete provider config');
  }

  return {
    id: config.id,
    tenantId: config.tenant_id,
    userId: config.user_id,
    provider: config.provider,
    model: config.model,
    apiKeyRef: config.api_key_ref,
    baseUrl: config.base_url,
    isDefault: config.is_default === 1,
    settings: config.settings,
    createdAt: config.created_at,
    updatedAt: config.updated_at,
  };
}
