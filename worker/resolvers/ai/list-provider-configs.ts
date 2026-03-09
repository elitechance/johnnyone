import { hostGraphqlRequest } from '../../lib/runtime/host-graphql';

interface ResolverContext {
  env: WorkerEnv;
  auth: { userId: string; tenantId: string };
}

interface WorkerEnv {
  HOST_GRAPHQL_URL?: string;
  [key: string]: unknown;
}

export default async function listProviderConfigs(
  _parent: unknown,
  _args: Record<string, never>,
  ctx: ResolverContext,
) {
  const result = await hostGraphqlRequest<{ listProviderConfigs: unknown[] }>(
    ctx.env,
    `query ListProviderConfigs {
      listProviderConfigs {
        id
        provider
        cliPath
        apiKey
        defaultModel
        settings
        isAvailable
        updatedAt
      }
    }`,
  );

  return result.listProviderConfigs;
}
