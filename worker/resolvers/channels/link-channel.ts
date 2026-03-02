// link-channel.ts — Mutation: linkChannel
// Phase 2: Link an external messaging channel to the user's account

import type { ResolverContext } from '@lokal/worker';

interface LinkChannelInput {
  channelType: 'telegram' | 'discord' | 'whatsapp';
  channelIdentifier: string;
  channelConfig?: string;
}

export default async function linkChannel(
  _parent: unknown,
  args: { input: LinkChannelInput },
  ctx: ResolverContext,
) {
  const { input } = args;
  const id = crypto.randomUUID();
  const now = new Date().toISOString();

  await ctx.env.DB.prepare(
    `INSERT INTO channel_bindings (id, tenant_id, user_id, channel_type, channel_identifier, channel_config, is_active, is_deleted, created_at, updated_at)
     VALUES (?, ?, ?, ?, ?, ?, 1, 0, ?, ?)`,
  )
    .bind(
      id,
      ctx.tenantId,
      ctx.userId,
      input.channelType,
      input.channelIdentifier,
      input.channelConfig ?? '{}',
      now,
      now,
    )
    .run();

  return {
    id,
    tenantId: ctx.tenantId,
    userId: ctx.userId,
    channelType: input.channelType,
    channelIdentifier: input.channelIdentifier,
    channelConfig: input.channelConfig ?? '{}',
    isActive: true,
    createdAt: now,
    updatedAt: now,
  };
}
