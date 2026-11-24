import { requireIdentity } from '../../lib/auth/require-identity';

interface ResolverContext {
  auth: { userId: string; tenantId: string };
  pubsub: {
    subscribe: (topic: string) => AsyncIterableIterator<unknown>;
  };
}

export default {
  subscribe: async (_parent: unknown, args: { relayId: string }, ctx: ResolverContext) => {
    // Owner-scoped (L-1-1): the relay publishes a turn's deltas under the desktop owner's id, so a
    // relayId alone never lets another user read them. Room stays the relayId (last segment).
    const id = await requireIdentity(ctx as any);
    return ctx.pubsub.subscribe(`relay-chat-delta:${id.userId}:${args.relayId}`);
  },
  resolve: (payload: unknown) => payload,
};
