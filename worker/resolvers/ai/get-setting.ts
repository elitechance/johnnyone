import { relayRpc, type RelayRpcContext } from '../../lib/runtime/relay-rpc';

export default async function getSetting(
  _parent: unknown,
  args: { key: string },
  ctx: RelayRpcContext,
) {
  return relayRpc<string>(ctx, 'get_setting', { key: args.key });
}
