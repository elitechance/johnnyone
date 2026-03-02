import { normalizeMessage, type NormalizedMessage } from '../../lib/channels/normalize';

interface ResolverContext {
  db: D1Database;
  env: WorkerEnv;
  auth: { userId: string; tenantId: string };
}

interface WorkerEnv {
  AGENT_SESSION_DO: DurableObjectNamespace;
  [key: string]: unknown;
}

interface ProcessChannelWebhookInput {
  channelType: string;
  payload: string;
}

interface ChannelBindingRow {
  id: string;
  tenant_id: string;
  user_id: string;
  channel_type: string;
  channel_identifier: string;
  channel_config: string;
  is_active: number;
}

/**
 * Process an incoming webhook from a channel adapter (Telegram, Discord, WhatsApp).
 *
 * Flow:
 * 1. Parse and normalize the incoming message
 * 2. Look up the channel binding to find the user and tenant
 * 3. Find or create an AI session for this channel conversation
 * 4. Route the message to the agent via sendAgentMessage
 *
 * Phase 2: This is a placeholder that establishes the pattern.
 */
export default async function createChannelWebhook(
  parent: unknown,
  args: { channelType: string; payload: string },
  ctx: ResolverContext,
) {
  const { channelType, payload } = args;

  // Normalize the incoming message based on channel type
  let normalized: NormalizedMessage;
  try {
    normalized = normalizeMessage(channelType, payload);
  } catch (err) {
    const error = err instanceof Error ? err.message : 'Unknown normalization error';
    throw new Error(`Failed to normalize ${channelType} message: ${error}`);
  }

  // Look up channel binding
  const binding = await ctx.db
    .prepare(
      `SELECT * FROM channel_bindings
       WHERE channel_type = ? AND channel_identifier = ? AND is_active = 1 AND is_deleted = 0`,
    )
    .bind(channelType, normalized.channelIdentifier)
    .first<ChannelBindingRow>();

  if (!binding) {
    throw new Error(
      `No active channel binding found for ${channelType}:${normalized.channelIdentifier}`,
    );
  }

  // Find or create a session for this channel conversation
  let sessionId: string;

  const existingSession = await ctx.db
    .prepare(
      `SELECT id FROM ai_sessions
       WHERE tenant_id = ? AND user_id = ? AND status = 'active' AND is_deleted = 0
       ORDER BY updated_at DESC LIMIT 1`,
    )
    .bind(binding.tenant_id, binding.user_id)
    .first<{ id: string }>();

  if (existingSession) {
    sessionId = existingSession.id;
  } else {
    sessionId = crypto.randomUUID();
    await ctx.db
      .prepare(
        `INSERT INTO ai_sessions (id, tenant_id, user_id, title, status, is_deleted, created_at, updated_at)
         VALUES (?, ?, ?, ?, 'active', 0, datetime('now'), datetime('now'))`,
      )
      .bind(sessionId, binding.tenant_id, binding.user_id, `${channelType} conversation`)
      .run();
  }

  // Create the user message
  const messageId = crypto.randomUUID();
  await ctx.db
    .prepare(
      `INSERT INTO ai_messages (id, tenant_id, session_id, role, content, source_channel, created_at)
       VALUES (?, ?, ?, 'user', ?, ?, datetime('now'))`,
    )
    .bind(messageId, binding.tenant_id, sessionId, normalized.text, channelType)
    .run();

  // Route to AgentSessionDO
  const doId = ctx.env.AGENT_SESSION_DO.idFromName(sessionId);
  const stub = ctx.env.AGENT_SESSION_DO.get(doId);

  stub
    .fetch(
      new Request('https://do.internal/process-message', {
        method: 'POST',
        headers: { 'Content-Type': 'application/json' },
        body: JSON.stringify({
          sessionId,
          messageId,
          tenantId: binding.tenant_id,
          userId: binding.user_id,
          content: normalized.text,
          sourceChannel: channelType,
        }),
      }),
    )
    .catch((err: Error) => {
      console.error(`Failed to route ${channelType} message to AgentSessionDO:`, err);
    });

  return {
    success: true,
    sessionId,
    messageId,
  };
}
