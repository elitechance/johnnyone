import { hostGraphqlRequest } from '../../lib/runtime/host-graphql';

interface ResolverContext {
  env: WorkerEnv;
  auth: { userId: string; tenantId: string };
}

interface WorkerEnv {
  HOST_GRAPHQL_URL?: string;
  [key: string]: unknown;
}

export default async function listDetectedCliTools(
  _parent: unknown,
  _args: Record<string, never>,
  ctx: ResolverContext,
) {
  const result = await hostGraphqlRequest<{ detectCliTools: unknown[] }>(
    ctx.env,
    `mutation DetectCliTools {
      detectCliTools {
        provider
        command
        found
        path
      }
    }`,
  );

  return result.detectCliTools;
}
