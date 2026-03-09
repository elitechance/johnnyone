import { hostGraphqlRequest } from '../../lib/runtime/host-graphql';

interface ResolverContext {
  env: WorkerEnv;
  auth: { userId: string; tenantId: string };
}

interface WorkerEnv {
  HOST_GRAPHQL_URL?: string;
  [key: string]: unknown;
}

export default async function updateAiSessionTitle(
  _parent: unknown,
  args: { id: string; title: string },
  ctx: ResolverContext,
) {
  const result = await hostGraphqlRequest<{ updateAiSessionTitle: unknown }>(
    ctx.env,
    `mutation UpdateAiSessionTitle($id: String!, $title: String!) {
      updateAiSessionTitle(id: $id, title: $title) {
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
    { id: args.id, title: args.title },
  );

  return result.updateAiSessionTitle;
}
