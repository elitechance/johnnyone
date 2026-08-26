import { requireIdentity } from '../../lib/auth/require-identity';

interface ResolverContext {
  auth: { userId: string; tenantId: string };
  pubsub: {
    subscribe: (topic: string) => AsyncIterableIterator<unknown>;
  };
}

export default {
  subscribe: async (_parent: unknown, _args: Record<string, never>, ctx: ResolverContext) => {
    const id = await requireIdentity(ctx as any);
    return ctx.pubsub.subscribe(`desktop-node-status:${id.tenantId}`);
  },
  resolve: (payload: unknown) => payload,
};
