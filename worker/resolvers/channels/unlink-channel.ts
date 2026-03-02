// unlink-channel.ts — Mutation: unlinkChannel
// Phase 2: Unlink an external messaging channel from the user's account

import type { ResolverContext } from '@lokal/worker';

export default async function unlinkChannel(
  _parent: unknown,
  args: { id: string },
  ctx: ResolverContext,
) {
  const now = new Date().toISOString();

  const binding = await ctx.env.DB.prepare(
    `SELECT * FROM channel_bindings WHERE id = ? AND tenant_id = ? AND user_id = ? AND is_deleted = 0`,
  )
    .bind(args.id, ctx.tenantId, ctx.userId)
    .first();

  if (!binding) {
    throw new Error('Channel binding not found');
  }

  await ctx.env.DB.prepare(
    `UPDATE channel_bindings SET is_deleted = 1, is_active = 0, updated_at = ? WHERE id = ?`,
  )
    .bind(now, args.id)
    .run();

  return {
    id: binding.id,
    tenantId: binding.tenant_id,
    userId: binding.user_id,
    channelType: binding.channel_type,
    channelIdentifier: binding.channel_identifier,
    channelConfig: binding.channel_config,
    isActive: false,
    createdAt: binding.created_at,
    updatedAt: now,
  };
}
