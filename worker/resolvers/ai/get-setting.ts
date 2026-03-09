import { hostGraphqlRequest } from '../../lib/runtime/host-graphql';

interface ResolverContext {
  env: WorkerEnv;
  auth: { userId: string; tenantId: string };
}

interface WorkerEnv {
  HOST_GRAPHQL_URL?: string;
  [key: string]: unknown;
}

export default async function getSetting(
  _parent: unknown,
  args: { key: string },
  ctx: ResolverContext,
) {
  const result = await hostGraphqlRequest<{ getSetting: string }>(
    ctx.env,
    `query GetSetting($key: String!) {
      getSetting(key: $key)
    }`,
    { key: args.key },
  );

  return result.getSetting;
}
