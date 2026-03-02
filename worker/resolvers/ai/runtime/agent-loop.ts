import { createProvider } from '../providers/provider-factory';
import { assembleContext } from './context-assembler';
import { dispatchToolCall } from './tool-dispatcher';
import type { LlmChatResponse, LlmToolCall, LlmStreamChunk } from '../providers/provider-interface';

interface AgentLoopParams {
  sessionId: string;
  messageId: string;
  tenantId: string;
  userId: string;
  content: string;
  env: Record<string, unknown>;
  broadcast: (msg: Record<string, unknown>) => void;
}

interface AiSessionRow {
  id: string;
  tenant_id: string;
  user_id: string;
  provider: string | null;
  model: string | null;
  system_prompt: string | null;
  total_input_tokens: number;
  total_output_tokens: number;
  total_cost: number;
}

const MAX_ITERATIONS = 10;

/**
 * Core agent loop: receive message -> assemble context -> call LLM -> handle tool_use -> loop -> return final response.
 *
 * The loop continues as long as the LLM responds with tool_use calls.
 * It terminates when:
 * - The LLM responds with a text-only message (end_turn / stop)
 * - The maximum iteration count is reached
 * - A tool execution is rejected or cancelled
 * - An error occurs
 */
export async function runAgentLoop(params: AgentLoopParams): Promise<void> {
  const { sessionId, tenantId, userId, env, broadcast } = params;

  const db = env.JOHNNYONE_DB as D1Database;

  // Fetch session details
  const session = await db
    .prepare(`SELECT * FROM ai_sessions WHERE id = ? AND tenant_id = ?`)
    .bind(sessionId, tenantId)
    .first<AiSessionRow>();

  if (!session) {
    throw new Error(`Session ${sessionId} not found`);
  }

  // Create the LLM provider
  const provider = await createProvider({
    db,
    tenantId,
    userId,
    provider: session.provider ?? undefined,
    model: session.model ?? undefined,
    env,
  });

  let totalInputTokens = session.total_input_tokens;
  let totalOutputTokens = session.total_output_tokens;
  let totalCost = session.total_cost;
  let iteration = 0;

  while (iteration < MAX_ITERATIONS) {
    iteration++;

    // Assemble the full context for the LLM
    const context = await assembleContext({
      db,
      sessionId,
      tenantId,
      systemPrompt: session.system_prompt ?? undefined,
    });

    // Call the LLM
    let response: LlmChatResponse;

    try {
      // Stream the response for real-time updates
      const assistantMessageId = crypto.randomUUID();
      let fullContent = '';
      let finishReason = '';
      let toolCalls: LlmToolCall[] = [];
      let inputTokens = 0;
      let outputTokens = 0;
      let chunkIndex = 0;

      broadcast({
        type: 'agent_message_start',
        data: { sessionId, messageId: assistantMessageId },
      });

      for await (const chunk of provider.chatStream({
        messages: context.messages,
        tools: context.tools,
        systemPrompt: context.systemPrompt,
        maxTokens: 4096,
      })) {
        switch (chunk.type) {
          case 'content_delta':
            if (chunk.delta) {
              fullContent += chunk.delta;
              broadcast({
                type: 'agent_message_delta',
                data: {
                  sessionId,
                  messageId: assistantMessageId,
                  delta: chunk.delta,
                  index: chunkIndex++,
                },
              });
            }
            break;

          case 'tool_call_delta':
            // Accumulate tool calls
            if (chunk.toolCall?.id && chunk.toolCall?.name && chunk.toolCall?.arguments) {
              const existing = toolCalls.find((tc) => tc.id === chunk.toolCall!.id);
              if (!existing) {
                toolCalls.push({
                  id: chunk.toolCall.id,
                  name: chunk.toolCall.name,
                  arguments: chunk.toolCall.arguments,
                });
              } else {
                existing.arguments = chunk.toolCall.arguments;
              }
            }
            break;

          case 'message_stop':
            finishReason = chunk.finishReason ?? 'stop';
            if (chunk.usage) {
              inputTokens = chunk.usage.inputTokens;
              outputTokens = chunk.usage.outputTokens;
            }
            break;

          case 'error':
            throw new Error(`LLM error: ${chunk.error}`);
        }
      }

      response = {
        messageId: assistantMessageId,
        content: fullContent || null,
        toolCalls,
        finishReason,
        usage: { inputTokens, outputTokens },
      };
    } catch (err) {
      const error = err instanceof Error ? err.message : 'Unknown LLM error';
      throw new Error(`LLM call failed on iteration ${iteration}: ${error}`);
    }

    // Track token usage
    totalInputTokens += response.usage.inputTokens;
    totalOutputTokens += response.usage.outputTokens;

    // Estimate cost (rough per-token pricing)
    const costPerInputToken = provider.name === 'claude' ? 0.000003 : provider.name === 'openai' ? 0.000005 : 0;
    const costPerOutputToken = provider.name === 'claude' ? 0.000015 : provider.name === 'openai' ? 0.000015 : 0;
    totalCost += response.usage.inputTokens * costPerInputToken + response.usage.outputTokens * costPerOutputToken;

    // Save the assistant message to D1
    await db
      .prepare(
        `INSERT INTO ai_messages (id, tenant_id, session_id, role, content, tool_calls, finish_reason, input_tokens, output_tokens, created_at)
         VALUES (?, ?, ?, 'assistant', ?, ?, ?, ?, ?, datetime('now'))`,
      )
      .bind(
        response.messageId,
        tenantId,
        sessionId,
        response.content,
        JSON.stringify(response.toolCalls),
        response.finishReason,
        response.usage.inputTokens,
        response.usage.outputTokens,
      )
      .run();

    // Log usage
    await db
      .prepare(
        `INSERT INTO ai_usage_log (id, tenant_id, user_id, session_id, provider, model, input_tokens, output_tokens, total_tokens, cost, created_at)
         VALUES (?, ?, ?, ?, ?, ?, ?, ?, ?, ?, datetime('now'))`,
      )
      .bind(
        crypto.randomUUID(),
        tenantId,
        userId,
        sessionId,
        provider.name,
        provider.model,
        response.usage.inputTokens,
        response.usage.outputTokens,
        response.usage.inputTokens + response.usage.outputTokens,
        response.usage.inputTokens * costPerInputToken + response.usage.outputTokens * costPerOutputToken,
      )
      .run();

    // Broadcast the full assistant message
    broadcast({
      type: 'agent_message',
      data: {
        id: response.messageId,
        sessionId,
        role: 'assistant',
        content: response.content,
        toolCalls: JSON.stringify(response.toolCalls),
        finishReason: response.finishReason,
        inputTokens: response.usage.inputTokens,
        outputTokens: response.usage.outputTokens,
      },
    });

    // If no tool calls, we're done
    if (response.toolCalls.length === 0) {
      break;
    }

    // Handle tool calls
    for (const toolCall of response.toolCalls) {
      const result = await dispatchToolCall({
        db,
        env,
        sessionId,
        messageId: response.messageId,
        tenantId,
        toolCall,
        broadcast,
      });

      // If tool was rejected or cancelled, stop the loop
      if (result.status === 'rejected' || result.status === 'cancelled') {
        // Insert a tool result message indicating rejection
        const toolResultMessageId = crypto.randomUUID();
        await db
          .prepare(
            `INSERT INTO ai_messages (id, tenant_id, session_id, role, content, tool_call_id, created_at)
             VALUES (?, ?, ?, 'tool', ?, ?, datetime('now'))`,
          )
          .bind(
            toolResultMessageId,
            tenantId,
            sessionId,
            `Tool execution ${result.status}: ${toolCall.name}`,
            toolCall.id,
          )
          .run();

        // Update session totals and exit
        await updateSessionTotals(db, sessionId, tenantId, totalInputTokens, totalOutputTokens, totalCost);
        return;
      }

      // Insert tool result as a message
      const toolResultMessageId = crypto.randomUUID();
      await db
        .prepare(
          `INSERT INTO ai_messages (id, tenant_id, session_id, role, content, tool_call_id, created_at)
           VALUES (?, ?, ?, 'tool', ?, ?, datetime('now'))`,
        )
        .bind(
          toolResultMessageId,
          tenantId,
          sessionId,
          result.output ?? `Tool ${toolCall.name} completed with no output`,
          toolCall.id,
        )
        .run();
    }

    // Continue the loop with the new tool results in context
  }

  if (iteration >= MAX_ITERATIONS) {
    broadcast({
      type: 'warning',
      data: {
        sessionId,
        message: `Agent loop reached maximum iterations (${MAX_ITERATIONS}). Stopping.`,
      },
    });
  }

  // Update session totals
  await updateSessionTotals(db, sessionId, tenantId, totalInputTokens, totalOutputTokens, totalCost);
}

async function updateSessionTotals(
  db: D1Database,
  sessionId: string,
  tenantId: string,
  totalInputTokens: number,
  totalOutputTokens: number,
  totalCost: number,
): Promise<void> {
  await db
    .prepare(
      `UPDATE ai_sessions SET total_input_tokens = ?, total_output_tokens = ?, total_cost = ?, updated_at = datetime('now')
       WHERE id = ? AND tenant_id = ?`,
    )
    .bind(totalInputTokens, totalOutputTokens, totalCost, sessionId, tenantId)
    .run();
}
