interface ResolverContext {
  auth: { userId: string; tenantId: string };
  pubsub: {
    subscribe: (topic: string) => AsyncIterableIterator<unknown>;
  };
}

export default {
  subscribe: (_parent: unknown, args: { relayId: string }, ctx: ResolverContext) =>
    ctx.pubsub.subscribe(`relay-chat-delta:${args.relayId}`),
  resolve: (payload: unknown) => payload,
};
