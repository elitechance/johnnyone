import { hostGraphqlRequest } from '../../lib/runtime/host-graphql';

interface ResolverContext {
  env: WorkerEnv;
  auth: { userId: string; tenantId: string };
}

interface WorkerEnv {
  HOST_GRAPHQL_URL?: string;
  [key: string]: unknown;
}

export default async function deleteAiSession(
  _parent: unknown,
  args: { id: string },
  ctx: ResolverContext,
) {
  const result = await hostGraphqlRequest<{ deleteAiSession: boolean }>(
    ctx.env,
    `mutation DeleteAiSession($id: String!) {
      deleteAiSession(id: $id)
    }`,
    { id: args.id },
  );

  return result.deleteAiSession;
}
