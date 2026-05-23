import { relayRpc, type RelayRpcContext } from '../../lib/runtime/relay-rpc';

export default async function deleteProviderConfig(
  _parent: unknown,
  args: { provider: string },
  ctx: RelayRpcContext,
) {
  return relayRpc<boolean>(ctx, 'delete_provider_config', { provider: args.provider });
}
