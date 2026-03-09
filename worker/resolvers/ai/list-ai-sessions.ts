import { hostGraphqlRequest } from '../../lib/runtime/host-graphql';

interface ResolverContext {
  env: WorkerEnv;
  auth: { userId: string; tenantId: string };
}

interface WorkerEnv {
  HOST_GRAPHQL_URL?: string;
  [key: string]: unknown;
}

export default async function listAiSessions(
  _parent: unknown,
  args: { status?: string },
  ctx: ResolverContext,
) {
  const result = await hostGraphqlRequest<{ listAiSessions: unknown[] }>(
    ctx.env,
    `query ListAiSessions($status: String) {
      listAiSessions(status: $status) {
        id
        title
        provider
        model
        workingDirectory
        status
        totalInputTokens
        totalOutputTokens
        totalCostCents
        createdAt
        updatedAt
      }
    }`,
    { status: args.status ?? null },
  );

  return result.listAiSessions;
}
