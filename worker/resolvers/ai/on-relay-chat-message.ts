import { requireIdentity } from '../../lib/auth/require-identity';

interface ResolverContext {
  auth: { userId: string; tenantId: string };
  pubsub: {
    subscribe: (topic: string) => AsyncIterableIterator<unknown>;
  };
}

export default {
  subscribe: async (_parent: unknown, args: { sessionId: string }, ctx: ResolverContext) => {
    const id = await requireIdentity(ctx as any);
    return ctx.pubsub.subscribe(`relay-chat-message:${id.userId}:${args.sessionId}`);
  },
  resolve: (payload: unknown) => payload,
};
