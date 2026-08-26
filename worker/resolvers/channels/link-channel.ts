// link-channel.ts — Mutation: linkChannel
// Phase 2: Link an external messaging channel to the user's account

import { requireIdentity } from '../../lib/auth/require-identity';

interface ResolverContext {
  db: D1Database;
  env: WorkerEnv;
  auth: { userId: string; tenantId: string };
}

interface WorkerEnv {
  [key: string]: unknown;
}

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
  const ident = await requireIdentity(ctx as any);
  const { input } = args;
  const id = crypto.randomUUID();
  const now = new Date().toISOString();

  await ctx.db.prepare(
    `INSERT INTO channel_bindings (id, tenant_id, user_id, channel_type, channel_identifier, channel_config, is_active, is_deleted, created_at, updated_at)
     VALUES (?, ?, ?, ?, ?, ?, 1, 0, ?, ?)`,
  )
    .bind(
      id,
      ident.tenantId,
      ident.userId,
      input.channelType,
      input.channelIdentifier,
      input.channelConfig ?? '{}',
      now,
      now,
    )
    .run();

  return {
    id,
    tenantId: ident.tenantId,
    userId: ident.userId,
    channelType: input.channelType,
    channelIdentifier: input.channelIdentifier,
    channelConfig: input.channelConfig ?? '{}',
    isActive: true,
    createdAt: now,
    updatedAt: now,
  };
}
