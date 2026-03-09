import { hostGraphqlRequest } from '../../lib/runtime/host-graphql';

interface ResolverContext {
  env: WorkerEnv;
  auth: { userId: string; tenantId: string };
}

interface WorkerEnv {
  HOST_GRAPHQL_URL?: string;
  [key: string]: unknown;
}

interface UpsertProviderConfigInput {
  provider: string;
  cliPath?: string;
  apiKey?: string;
  defaultModel?: string;
  settings?: string;
}

export default async function upsertProviderConfig(
  _parent: unknown,
  args: { input: UpsertProviderConfigInput },
  ctx: ResolverContext,
) {
  const result = await hostGraphqlRequest<{ upsertProviderConfig: unknown }>(
    ctx.env,
    `mutation UpsertProviderConfig($input: UpsertProviderConfigInput!) {
      upsertProviderConfig(input: $input) {
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
    { input: args.input },
  );

  return result.upsertProviderConfig;
}
