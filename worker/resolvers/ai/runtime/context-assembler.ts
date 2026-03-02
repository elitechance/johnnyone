import type { LlmMessage, LlmToolDefinition } from '../providers/provider-interface';

interface AssembleContextParams {
  db: D1Database;
  sessionId: string;
  tenantId: string;
  systemPrompt?: string;
}

interface AssembledContext {
  systemPrompt: string;
  messages: LlmMessage[];
  tools: LlmToolDefinition[];
}

interface AiMessageRow {
  id: string;
  role: string;
  content: string | null;
  tool_calls: string;
  tool_call_id: string | null;
  created_at: string;
}

interface ToolDefinitionRow {
  slug: string;
  name: string;
  description: string | null;
  parameters_schema: string;
  is_enabled: number;
}

const DEFAULT_SYSTEM_PROMPT = `You are JohnnyOne, an AI assistant with access to desktop tools. You can execute shell commands, read and write files, search file contents, list processes, and retrieve system information through the connected desktop agent.

Guidelines:
- Be helpful, accurate, and concise.
- When using tools, explain what you're about to do before executing.
- If a tool execution fails, analyze the error and suggest alternatives.
- For destructive operations (file writes, shell commands), confirm intent when appropriate.
- Format responses using markdown for readability.
- When listing files or showing code, use code blocks with appropriate language tags.`;

/**
 * Build the full context for an LLM request:
 * - System prompt (custom or default)
 * - Conversation history from D1
 * - Available tool definitions
 */
export async function assembleContext(
  params: AssembleContextParams,
): Promise<AssembledContext> {
  const { db, sessionId, tenantId, systemPrompt } = params;

  // Fetch conversation history
  const messagesResult = await db
    .prepare(
      `SELECT id, role, content, tool_calls, tool_call_id, created_at
       FROM ai_messages
       WHERE session_id = ? AND tenant_id = ?
       ORDER BY created_at ASC`,
    )
    .bind(sessionId, tenantId)
    .all<AiMessageRow>();

  // Fetch enabled tool definitions
  const toolsResult = await db
    .prepare(
      `SELECT slug, name, description, parameters_schema, is_enabled
       FROM ai_tool_definitions
       WHERE tenant_id = ? AND is_deleted = 0 AND is_enabled = 1
       ORDER BY category, name`,
    )
    .bind(tenantId)
    .all<ToolDefinitionRow>();

  // Convert message rows to LlmMessage format
  const messages: LlmMessage[] = messagesResult.results.map((row) => {
    const msg: LlmMessage = {
      role: row.role as LlmMessage['role'],
      content: row.content,
    };

    // Parse tool calls for assistant messages
    if (row.role === 'assistant' && row.tool_calls) {
      try {
        const toolCalls = JSON.parse(row.tool_calls);
        if (Array.isArray(toolCalls) && toolCalls.length > 0) {
          msg.toolCalls = toolCalls;
        }
      } catch {
        // Ignore malformed tool_calls JSON
      }
    }

    // Set tool_call_id for tool result messages
    if (row.role === 'tool' && row.tool_call_id) {
      msg.toolCallId = row.tool_call_id;
    }

    return msg;
  });

  // Convert tool definitions to LlmToolDefinition format
  const tools: LlmToolDefinition[] = toolsResult.results.map((row) => {
    let parametersSchema: Record<string, unknown> = {};
    try {
      parametersSchema = JSON.parse(row.parameters_schema);
    } catch {
      // Use empty schema
    }

    return {
      name: row.slug,
      description: row.description ?? row.name,
      parametersSchema,
    };
  });

  // Apply context window limits
  // Keep the most recent messages if conversation is very long
  const MAX_MESSAGES = 100;
  const trimmedMessages =
    messages.length > MAX_MESSAGES
      ? messages.slice(messages.length - MAX_MESSAGES)
      : messages;

  return {
    systemPrompt: systemPrompt ?? DEFAULT_SYSTEM_PROMPT,
    messages: trimmedMessages,
    tools,
  };
}
