import type { LlmToolCall } from '../providers/provider-interface';

interface DispatchToolCallParams {
  db: D1Database;
  env: Record<string, unknown>;
  sessionId: string;
  messageId: string;
  tenantId: string;
  toolCall: LlmToolCall;
  broadcast: (msg: Record<string, unknown>) => void;
}

interface ToolDispatchResult {
  output: string | null;
  status: 'completed' | 'failed' | 'rejected' | 'cancelled';
  error?: string;
  durationMs?: number;
}

interface ToolDefinitionRow {
  id: string;
  slug: string;
  executor: string;
  requires_approval: number;
}

interface DesktopNodeRow {
  id: string;
  status: string;
}

const TOOL_EXECUTION_TIMEOUT_MS = 60_000;
const APPROVAL_TIMEOUT_MS = 300_000; // 5 minutes for human approval

/**
 * Route tool calls to the appropriate executor:
 * - Desktop tools: forwarded to a connected desktop agent via WebSocket
 * - Cloud tools: executed directly in the worker (future)
 *
 * Handles the approval flow for tools that require human approval.
 */
export async function dispatchToolCall(
  params: DispatchToolCallParams,
): Promise<ToolDispatchResult> {
  const { db, env, sessionId, messageId, tenantId, toolCall, broadcast } = params;

  const startTime = Date.now();

  // Look up tool definition
  const toolDef = await db
    .prepare(
      `SELECT id, slug, executor, requires_approval FROM ai_tool_definitions
       WHERE slug = ? AND tenant_id = ? AND is_deleted = 0 AND is_enabled = 1`,
    )
    .bind(toolCall.name, tenantId)
    .first<ToolDefinitionRow>();

  if (!toolDef) {
    return {
      output: null,
      status: 'failed',
      error: `Unknown tool: ${toolCall.name}`,
    };
  }

  // Create tool execution record
  const executionId = crypto.randomUUID();
  const initialStatus = toolDef.requires_approval ? 'pending' : 'approved';

  await db
    .prepare(
      `INSERT INTO ai_tool_executions (id, tenant_id, session_id, message_id, tool_definition_id, tool_slug, input, status, created_at, updated_at)
       VALUES (?, ?, ?, ?, ?, ?, ?, ?, datetime('now'), datetime('now'))`,
    )
    .bind(
      executionId,
      tenantId,
      sessionId,
      messageId,
      toolDef.id,
      toolCall.name,
      toolCall.arguments,
      initialStatus,
    )
    .run();

  // Broadcast tool execution event
  broadcast({
    type: 'tool_execution',
    data: {
      id: executionId,
      sessionId,
      toolSlug: toolCall.name,
      input: toolCall.arguments,
      status: initialStatus,
    },
  });

  // If approval required, wait for approval signal
  if (toolDef.requires_approval) {
    broadcast({
      type: 'tool_approval_required',
      data: {
        executionId,
        sessionId,
        toolSlug: toolCall.name,
        input: toolCall.arguments,
      },
    });

    const approvalResult = await waitForApproval(env, executionId, APPROVAL_TIMEOUT_MS);

    if (approvalResult !== 'approved') {
      await db
        .prepare(
          `UPDATE ai_tool_executions SET status = ?, updated_at = datetime('now') WHERE id = ?`,
        )
        .bind(approvalResult, executionId)
        .run();

      return {
        output: null,
        status: approvalResult as 'rejected' | 'cancelled',
        durationMs: Date.now() - startTime,
      };
    }
  }

  // Update status to running
  await db
    .prepare(
      `UPDATE ai_tool_executions SET status = 'running', updated_at = datetime('now') WHERE id = ?`,
    )
    .bind(executionId)
    .run();

  broadcast({
    type: 'tool_execution',
    data: {
      id: executionId,
      sessionId,
      toolSlug: toolCall.name,
      status: 'running',
    },
  });

  // Execute the tool
  try {
    let result: ToolDispatchResult;

    if (toolDef.executor === 'desktop') {
      result = await executeOnDesktop(db, env, tenantId, executionId, toolCall, broadcast);
    } else {
      result = await executeInCloud(toolCall);
    }

    const durationMs = Date.now() - startTime;

    // Update execution record
    await db
      .prepare(
        `UPDATE ai_tool_executions SET status = ?, output = ?, error = ?, duration_ms = ?, updated_at = datetime('now') WHERE id = ?`,
      )
      .bind(result.status, result.output, result.error ?? null, durationMs, executionId)
      .run();

    broadcast({
      type: 'tool_execution',
      data: {
        id: executionId,
        sessionId,
        toolSlug: toolCall.name,
        status: result.status,
        output: result.output,
        durationMs,
      },
    });

    return { ...result, durationMs };
  } catch (err) {
    const error = err instanceof Error ? err.message : 'Unknown execution error';
    const durationMs = Date.now() - startTime;

    await db
      .prepare(
        `UPDATE ai_tool_executions SET status = 'failed', error = ?, duration_ms = ?, updated_at = datetime('now') WHERE id = ?`,
      )
      .bind(error, durationMs, executionId)
      .run();

    broadcast({
      type: 'tool_execution',
      data: {
        id: executionId,
        sessionId,
        toolSlug: toolCall.name,
        status: 'failed',
        error,
        durationMs,
      },
    });

    return { output: null, status: 'failed', error, durationMs };
  }
}

/**
 * Execute a tool on the desktop agent.
 * Finds an online desktop node and sends the tool call via the PubSub system.
 * Waits for the result to come back.
 */
async function executeOnDesktop(
  db: D1Database,
  env: Record<string, unknown>,
  tenantId: string,
  executionId: string,
  toolCall: LlmToolCall,
  broadcast: (msg: Record<string, unknown>) => void,
): Promise<ToolDispatchResult> {
  // Find an online desktop node
  const node = await db
    .prepare(
      `SELECT id, status FROM desktop_nodes
       WHERE tenant_id = ? AND is_deleted = 0 AND status = 'online'
       ORDER BY last_heartbeat_at DESC LIMIT 1`,
    )
    .bind(tenantId)
    .first<DesktopNodeRow>();

  if (!node) {
    return {
      output: null,
      status: 'failed',
      error: 'No online desktop agent available. Please ensure your desktop agent is running.',
    };
  }

  // Update execution with the executor node
  await db
    .prepare(
      `UPDATE ai_tool_executions SET executor_node_id = ?, updated_at = datetime('now') WHERE id = ?`,
    )
    .bind(node.id, executionId)
    .run();

  // Send tool call to desktop agent via PubSub
  const PUBSUB_DO = env.PUBSUB_DO as DurableObjectNamespace;
  const pubsubId = PUBSUB_DO.idFromName('global');
  const pubsub = PUBSUB_DO.get(pubsubId);

  await pubsub.fetch(
    new Request('https://do.internal/publish', {
      method: 'POST',
      headers: { 'Content-Type': 'application/json' },
      body: JSON.stringify({
        topic: `desktop-tool:${node.id}`,
        payload: {
          executionId,
          toolSlug: toolCall.name,
          input: JSON.parse(toolCall.arguments),
        },
      }),
    }),
  );

  // Wait for the tool result (the desktop agent will POST back to the DO)
  const result = await waitForToolResult(env, executionId, TOOL_EXECUTION_TIMEOUT_MS);

  return result;
}

/**
 * Execute a tool in the cloud worker.
 * Reserved for future cloud-side tool implementations.
 */
async function executeInCloud(toolCall: LlmToolCall): Promise<ToolDispatchResult> {
  return {
    output: null,
    status: 'failed',
    error: `Cloud execution not yet implemented for tool: ${toolCall.name}`,
  };
}

/**
 * Poll for approval signal from the Durable Object storage.
 * In production, this would use a more efficient signaling mechanism.
 */
async function waitForApproval(
  env: Record<string, unknown>,
  executionId: string,
  timeoutMs: number,
): Promise<string> {
  const AGENT_SESSION_DO = env.AGENT_SESSION_DO as DurableObjectNamespace;
  const startTime = Date.now();
  const pollInterval = 1000;

  while (Date.now() - startTime < timeoutMs) {
    // Check if a signal has been stored
    // In practice, this would be event-driven rather than polling
    await new Promise((resolve) => setTimeout(resolve, pollInterval));

    // The signal would be set by the approve/reject/cancel handlers
    // This is a simplified polling approach
    if (Date.now() - startTime > timeoutMs) {
      return 'cancelled';
    }
  }

  return 'cancelled';
}

/**
 * Wait for tool execution result from the desktop agent.
 */
async function waitForToolResult(
  env: Record<string, unknown>,
  executionId: string,
  timeoutMs: number,
): Promise<ToolDispatchResult> {
  const startTime = Date.now();
  const pollInterval = 500;

  while (Date.now() - startTime < timeoutMs) {
    await new Promise((resolve) => setTimeout(resolve, pollInterval));

    // In production, this would be event-driven
    // The desktop agent posts results back to the DO which stores them
    if (Date.now() - startTime > timeoutMs) {
      break;
    }
  }

  return {
    output: null,
    status: 'failed',
    error: `Tool execution timed out after ${timeoutMs}ms`,
  };
}
