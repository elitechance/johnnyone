import { hostGraphqlRequest } from '../../lib/runtime/host-graphql';

interface ResolverContext {
  env: WorkerEnv;
  auth: { userId: string; tenantId: string };
}

interface WorkerEnv {
  HOST_GRAPHQL_URL?: string;
  [key: string]: unknown;
}

export default async function updateAiSessionWorkingDirectory(
  _parent: unknown,
  args: { id: string; workingDirectory: string },
  ctx: ResolverContext,
) {
  const result = await hostGraphqlRequest<{ updateAiSessionWorkingDirectory: unknown }>(
    ctx.env,
    `mutation UpdateAiSessionWorkingDirectory($id: String!, $workingDirectory: String!) {
      updateAiSessionWorkingDirectory(id: $id, workingDirectory: $workingDirectory) {
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
    { id: args.id, workingDirectory: args.workingDirectory },
  );

  return result.updateAiSessionWorkingDirectory;
}
