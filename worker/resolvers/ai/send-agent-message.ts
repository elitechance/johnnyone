interface ResolverContext {
  db: D1Database;
  env: WorkerEnv;
  auth: { userId: string; tenantId: string };
}

interface WorkerEnv {
  AGENT_SESSION_DO: DurableObjectNamespace;
  [key: string]: unknown;
}

interface SendAgentMessageInput {
  sessionId: string;
  content: string;
  sourceChannel?: string;
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

export default async function sendAgentMessage(
  parent: unknown,
  args: { input: SendAgentMessageInput },
  ctx: ResolverContext,
) {
  const { sessionId, content, sourceChannel } = args.input;

  // Verify session exists and belongs to tenant
  const session = await ctx.db
    .prepare(
      `SELECT id, status FROM ai_sessions WHERE id = ? AND tenant_id = ? AND is_deleted = 0`,
    )
    .bind(sessionId, ctx.auth.tenantId)
    .first<{ id: string; status: string }>();

  if (!session) {
    throw new Error('Session not found');
  }

  if (session.status !== 'active') {
    throw new Error(`Cannot send message to session with status: ${session.status}`);
  }

  // Create the user message in D1
  const messageId = crypto.randomUUID();

  await ctx.db
    .prepare(
      `INSERT INTO ai_messages (id, tenant_id, session_id, role, content, tool_calls, tool_call_id, source_channel, created_at)
       VALUES (?, ?, ?, 'user', ?, '[]', NULL, ?, datetime('now'))`,
    )
    .bind(messageId, ctx.auth.tenantId, sessionId, content, sourceChannel ?? null)
    .run();

  // Route to AgentSessionDO for async agent processing
  const doId = ctx.env.AGENT_SESSION_DO.idFromName(sessionId);
  const stub = ctx.env.AGENT_SESSION_DO.get(doId);

  const doRequest = new Request('https://do.internal/process-message', {
    method: 'POST',
    headers: { 'Content-Type': 'application/json' },
    body: JSON.stringify({
      sessionId,
      messageId,
      tenantId: ctx.auth.tenantId,
      userId: ctx.auth.userId,
      content,
      sourceChannel: sourceChannel ?? null,
    }),
  });

  // Fire and forget - the DO will process asynchronously and push results via WebSocket/PubSub
  ctx.env.AGENT_SESSION_DO.get(doId).fetch(doRequest).catch((err: Error) => {
    console.error('Failed to route message to AgentSessionDO:', err);
  });

  // Return the user message immediately
  const message = await ctx.db
    .prepare(`SELECT * FROM ai_messages WHERE id = ?`)
    .bind(messageId)
    .first<AiMessageRow>();

  if (!message) {
    throw new Error('Failed to create message');
  }

  return {
    id: message.id,
    tenantId: message.tenant_id,
    sessionId: message.session_id,
    role: message.role,
    content: message.content,
    toolCalls: message.tool_calls,
    toolCallId: message.tool_call_id,
    sourceChannel: message.source_channel,
    finishReason: message.finish_reason,
    inputTokens: message.input_tokens,
    outputTokens: message.output_tokens,
    createdAt: message.created_at,
  };
}
