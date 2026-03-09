import { hostGraphqlRequest } from '../../lib/runtime/host-graphql';

interface ResolverContext {
  env: WorkerEnv;
  auth: { userId: string; tenantId: string };
}

interface WorkerEnv {
  HOST_GRAPHQL_URL?: string;
  [key: string]: unknown;
}

interface CreateAiSessionInput {
  title?: string;
  provider?: string;
  model?: string;
  workingDirectory?: string;
}

export default async function createAiSession(
  _parent: unknown,
  args: { input: CreateAiSessionInput },
  ctx: ResolverContext,
) {
  const result = await hostGraphqlRequest<{ createAiSession: unknown }>(
    ctx.env,
    `mutation CreateAiSession($input: CreateAiSessionInput!) {
      createAiSession(input: $input) {
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
    { input: args.input },
  );

  return result.createAiSession;
}
