import { relayRpc, type RelayRpcContext } from '../../lib/runtime/relay-rpc';

export default async function cancelAiGeneration(
  _parent: unknown,
  args: { sessionId: string },
  ctx: RelayRpcContext,
) {
  return relayRpc<boolean>(ctx, 'stop_ai_generation', { sessionId: args.sessionId });
}
