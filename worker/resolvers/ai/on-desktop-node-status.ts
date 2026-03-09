interface ResolverContext {
  auth: { userId: string; tenantId: string };
  pubsub: {
    subscribe: (topic: string) => AsyncIterableIterator<unknown>;
  };
}

export default {
  subscribe: (_parent: unknown, _args: Record<string, never>, ctx: ResolverContext) =>
    ctx.pubsub.subscribe(`desktop-node-status:${ctx.auth.tenantId}`),
  resolve: (payload: unknown) => payload,
};
