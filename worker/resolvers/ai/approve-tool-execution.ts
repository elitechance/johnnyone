interface ResolverContext {
  db: D1Database;
  env: WorkerEnv;
  auth: { userId: string; tenantId: string };
}

interface WorkerEnv {
  AGENT_SESSION_DO: DurableObjectNamespace;
  [key: string]: unknown;
}

interface ToolExecutionRow {
  id: string;
  tenant_id: string;
  session_id: string;
  message_id: string;
  tool_definition_id: string | null;
  tool_slug: string;
  input: string;
  output: string | null;
  status: string;
  error: string | null;
  duration_ms: number | null;
  executor_node_id: string | null;
  created_at: string;
  updated_at: string;
}

function mapToolExecutionRow(row: ToolExecutionRow) {
  return {
    id: row.id,
    tenantId: row.tenant_id,
    sessionId: row.session_id,
    messageId: row.message_id,
    toolDefinitionId: row.tool_definition_id,
    toolSlug: row.tool_slug,
    input: row.input,
    output: row.output,
    status: row.status,
    error: row.error,
    durationMs: row.duration_ms,
    executorNodeId: row.executor_node_id,
    createdAt: row.created_at,
    updatedAt: row.updated_at,
  };
}

export default async function approveToolExecution(
  parent: unknown,
  args: { id: string },
  ctx: ResolverContext,
) {
  const existing = await ctx.db
    .prepare(
      `SELECT id, session_id, status FROM ai_tool_executions WHERE id = ? AND tenant_id = ?`,
    )
    .bind(args.id, ctx.auth.tenantId)
    .first<{ id: string; session_id: string; status: string }>();

  if (!existing) {
    throw new Error('Tool execution not found');
  }

  if (existing.status !== 'pending') {
    throw new Error(`Cannot approve tool execution with status: ${existing.status}`);
  }

  await ctx.db
    .prepare(
      `UPDATE ai_tool_executions SET status = 'approved', updated_at = datetime('now') WHERE id = ? AND tenant_id = ?`,
    )
    .bind(args.id, ctx.auth.tenantId)
    .run();

  // Notify the AgentSessionDO that the tool was approved so it can proceed
  const doId = ctx.env.AGENT_SESSION_DO.idFromName(existing.session_id);
  const stub = ctx.env.AGENT_SESSION_DO.get(doId);

  stub
    .fetch(
      new Request('https://do.internal/tool-approved', {
        method: 'POST',
        headers: { 'Content-Type': 'application/json' },
        body: JSON.stringify({
          executionId: args.id,
          sessionId: existing.session_id,
        }),
      }),
    )
    .catch((err: Error) => {
      console.error('Failed to notify DO of tool approval:', err);
    });

  const execution = await ctx.db
    .prepare(`SELECT * FROM ai_tool_executions WHERE id = ?`)
    .bind(args.id)
    .first<ToolExecutionRow>();

  if (!execution) {
    throw new Error('Failed to approve tool execution');
  }

  return mapToolExecutionRow(execution);
}
