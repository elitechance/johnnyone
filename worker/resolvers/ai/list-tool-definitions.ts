interface ResolverContext {
  db: D1Database;
  env: WorkerEnv;
  auth: { userId: string; tenantId: string };
}

interface WorkerEnv {
  [key: string]: unknown;
}

interface ToolDefinitionRow {
  id: string;
  tenant_id: string;
  slug: string;
  name: string;
  description: string | null;
  category: string;
  parameters_schema: string;
  executor: string;
  requires_approval: number;
  is_enabled: number;
  is_deleted: number;
  created_at: string;
  updated_at: string;
}

function mapToolDefinitionRow(row: ToolDefinitionRow) {
  return {
    id: row.id,
    tenantId: row.tenant_id,
    slug: row.slug,
    name: row.name,
    description: row.description,
    category: row.category,
    parametersSchema: row.parameters_schema,
    executor: row.executor,
    requiresApproval: row.requires_approval === 1,
    isEnabled: row.is_enabled === 1,
    createdAt: row.created_at,
    updatedAt: row.updated_at,
  };
}

export default async function listToolDefinitions(
  parent: unknown,
  args: { category?: string },
  ctx: ResolverContext,
) {
  let sql = `SELECT * FROM ai_tool_definitions WHERE tenant_id = ? AND is_deleted = 0 AND is_enabled = 1`;
  const bindings: string[] = [ctx.auth.tenantId];

  if (args.category) {
    sql += ` AND category = ?`;
    bindings.push(args.category);
  }

  sql += ` ORDER BY category, name ASC`;

  const result = await ctx.db
    .prepare(sql)
    .bind(...bindings)
    .all<ToolDefinitionRow>();

  return result.results.map(mapToolDefinitionRow);
}
