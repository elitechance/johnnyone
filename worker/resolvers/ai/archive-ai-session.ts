import { hostGraphqlRequest } from '../../lib/runtime/host-graphql';

interface ResolverContext {
  env: WorkerEnv;
  auth: { userId: string; tenantId: string };
}

interface WorkerEnv {
  HOST_GRAPHQL_URL?: string;
  [key: string]: unknown;
}

export default async function archiveAiSession(
  _parent: unknown,
  args: { id: string },
  ctx: ResolverContext,
) {
  const result = await hostGraphqlRequest<{ archiveAiSession: unknown }>(
    ctx.env,
    `mutation ArchiveAiSession($id: String!) {
      archiveAiSession(id: $id) {
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
    { id: args.id },
  );

  return result.archiveAiSession;
}
