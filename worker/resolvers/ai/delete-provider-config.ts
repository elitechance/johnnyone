import { hostGraphqlRequest } from '../../lib/runtime/host-graphql';

interface ResolverContext {
  env: WorkerEnv;
  auth: { userId: string; tenantId: string };
}

interface WorkerEnv {
  HOST_GRAPHQL_URL?: string;
  [key: string]: unknown;
}

export default async function deleteProviderConfig(
  _parent: unknown,
  args: { provider: string },
  ctx: ResolverContext,
) {
  const result = await hostGraphqlRequest<{ deleteProviderConfig: boolean }>(
    ctx.env,
    `mutation DeleteProviderConfig($provider: String!) {
      deleteProviderConfig(provider: $provider)
    }`,
    { provider: args.provider },
  );

  return result.deleteProviderConfig;
}
