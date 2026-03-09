import { hostGraphqlRequest } from '../../lib/runtime/host-graphql';

interface ResolverContext {
  env: WorkerEnv;
  auth: { userId: string; tenantId: string };
}

interface WorkerEnv {
  HOST_GRAPHQL_URL?: string;
  [key: string]: unknown;
}

export default async function stopAiGeneration(
  _parent: unknown,
  args: { sessionId: string },
  ctx: ResolverContext,
) {
  const result = await hostGraphqlRequest<{ stopAiGeneration: boolean }>(
    ctx.env,
    `mutation StopAiGeneration($sessionId: String!) {
      stopAiGeneration(sessionId: $sessionId)
    }`,
    { sessionId: args.sessionId },
  );

  return result.stopAiGeneration;
}
