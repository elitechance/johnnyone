import { hostGraphqlRequest } from '../../lib/runtime/host-graphql';

interface ResolverContext {
  env: WorkerEnv;
  auth: { userId: string; tenantId: string };
}

interface WorkerEnv {
  HOST_GRAPHQL_URL?: string;
  [key: string]: unknown;
}

export default async function updateAiSessionProvider(
  _parent: unknown,
  args: { id: string; provider: string },
  ctx: ResolverContext,
) {
  const result = await hostGraphqlRequest<{ updateAiSessionProvider: unknown }>(
    ctx.env,
    `mutation UpdateAiSessionProvider($id: String!, $provider: String!) {
      updateAiSessionProvider(id: $id, provider: $provider) {
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
    { id: args.id, provider: args.provider },
  );

  return result.updateAiSessionProvider;
}
