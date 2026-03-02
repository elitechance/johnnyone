interface ResolverContext {
  db: D1Database;
  env: WorkerEnv;
  auth: { userId: string; tenantId: string };
}

interface WorkerEnv {
  [key: string]: unknown;
}

interface UpsertProviderConfigInput {
  id?: string;
  provider: string;
  model?: string;
  apiKeyRef?: string;
  baseUrl?: string;
  isDefault?: boolean;
  settings?: string;
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

function mapProviderConfigRow(row: ProviderConfigRow) {
  return {
    id: row.id,
    tenantId: row.tenant_id,
    userId: row.user_id,
    provider: row.provider,
    model: row.model,
    apiKeyRef: row.api_key_ref,
    baseUrl: row.base_url,
    isDefault: row.is_default === 1,
    settings: row.settings,
    createdAt: row.created_at,
    updatedAt: row.updated_at,
  };
}

export default async function upsertProviderConfig(
  parent: unknown,
  args: { input: UpsertProviderConfigInput },
  ctx: ResolverContext,
) {
  const { id, provider, model, apiKeyRef, baseUrl, isDefault, settings } = args.input;

  // If marking as default, unset all other defaults for this user
  if (isDefault) {
    await ctx.db
      .prepare(
        `UPDATE ai_provider_configs SET is_default = 0, updated_at = datetime('now')
         WHERE tenant_id = ? AND user_id = ? AND is_deleted = 0`,
      )
      .bind(ctx.auth.tenantId, ctx.auth.userId)
      .run();
  }

  if (id) {
    // Update existing
    const existing = await ctx.db
      .prepare(
        `SELECT id FROM ai_provider_configs WHERE id = ? AND tenant_id = ? AND user_id = ? AND is_deleted = 0`,
      )
      .bind(id, ctx.auth.tenantId, ctx.auth.userId)
      .first();

    if (!existing) {
      throw new Error('Provider config not found');
    }

    await ctx.db
      .prepare(
        `UPDATE ai_provider_configs
         SET provider = ?, model = ?, api_key_ref = ?, base_url = ?, is_default = ?, settings = ?, updated_at = datetime('now')
         WHERE id = ? AND tenant_id = ?`,
      )
      .bind(
        provider,
        model ?? null,
        apiKeyRef ?? null,
        baseUrl ?? null,
        isDefault ? 1 : 0,
        settings ?? '{}',
        id,
        ctx.auth.tenantId,
      )
      .run();

    const config = await ctx.db
      .prepare(`SELECT * FROM ai_provider_configs WHERE id = ?`)
      .bind(id)
      .first<ProviderConfigRow>();

    if (!config) {
      throw new Error('Failed to update provider config');
    }

    return mapProviderConfigRow(config);
  } else {
    // Insert new
    const newId = crypto.randomUUID();

    await ctx.db
      .prepare(
        `INSERT INTO ai_provider_configs (id, tenant_id, user_id, provider, model, api_key_ref, base_url, is_default, settings, is_deleted, created_at, updated_at)
         VALUES (?, ?, ?, ?, ?, ?, ?, ?, ?, 0, datetime('now'), datetime('now'))`,
      )
      .bind(
        newId,
        ctx.auth.tenantId,
        ctx.auth.userId,
        provider,
        model ?? null,
        apiKeyRef ?? null,
        baseUrl ?? null,
        isDefault ? 1 : 0,
        settings ?? '{}',
      )
      .run();

    const config = await ctx.db
      .prepare(`SELECT * FROM ai_provider_configs WHERE id = ?`)
      .bind(newId)
      .first<ProviderConfigRow>();

    if (!config) {
      throw new Error('Failed to create provider config');
    }

    return mapProviderConfigRow(config);
  }
}
