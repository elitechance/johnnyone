import { requireIdentity } from '../../lib/auth/require-identity';

interface ResolverContext {
  auth: { userId: string; tenantId: string };
  pubsub: {
    subscribe: (topic: string) => AsyncIterableIterator<unknown>;
  };
}

export default {
  subscribe: async (_parent: unknown, args: { relayId: string }, ctx: ResolverContext) => {
    await requireIdentity(ctx as any);
    return ctx.pubsub.subscribe(`relay-chat-delta:${args.relayId}`);
  },
  resolve: (payload: unknown) => payload,
};
