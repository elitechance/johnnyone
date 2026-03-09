import { hostGraphqlRequest } from '../../lib/runtime/host-graphql';

interface ResolverContext {
  env: WorkerEnv;
  auth: { userId: string; tenantId: string };
}

interface WorkerEnv {
  HOST_GRAPHQL_URL?: string;
  [key: string]: unknown;
}

export default async function listAiMessages(
  _parent: unknown,
  args: { sessionId: string; limit?: number; offset?: number },
  ctx: ResolverContext,
) {
  const result = await hostGraphqlRequest<{ listAiMessages: unknown[] }>(
    ctx.env,
    `query ListAiMessages($sessionId: String!, $limit: Int, $offset: Int) {
      listAiMessages(sessionId: $sessionId, limit: $limit, offset: $offset) {
        id
        sessionId
        role
        content
        toolCalls
        finishReason
        inputTokens
        outputTokens
        costCents
        createdAt
      }
    }`,
    {
      sessionId: args.sessionId,
      limit: args.limit ?? 100,
      offset: args.offset ?? 0,
    },
  );

  return result.listAiMessages;
}
