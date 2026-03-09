interface ResolverContext {
  auth: { userId: string; tenantId: string };
  pubsub: {
    subscribe: (topic: string) => AsyncIterableIterator<unknown>;
  };
}

export default {
  subscribe: (_parent: unknown, args: { sessionId: string }, ctx: ResolverContext) =>
    ctx.pubsub.subscribe(`ai-chat-delta:${args.sessionId}`),
  resolve: (payload: unknown) => payload,
};
