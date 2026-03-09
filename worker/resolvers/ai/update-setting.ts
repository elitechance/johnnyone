import { hostGraphqlRequest } from '../../lib/runtime/host-graphql';

interface ResolverContext {
  env: WorkerEnv;
  auth: { userId: string; tenantId: string };
}

interface WorkerEnv {
  HOST_GRAPHQL_URL?: string;
  [key: string]: unknown;
}

export default async function updateSetting(
  _parent: unknown,
  args: { key: string; value: string },
  ctx: ResolverContext,
) {
  const result = await hostGraphqlRequest<{ setSetting: boolean }>(
    ctx.env,
    `mutation SetSetting($key: String!, $value: String!) {
      setSetting(key: $key, value: $value)
    }`,
    args,
  );

  return result.setSetting;
}
