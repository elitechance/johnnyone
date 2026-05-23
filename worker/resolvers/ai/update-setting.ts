import { relayRpc, type RelayRpcContext } from '../../lib/runtime/relay-rpc';

export default async function updateSetting(
  _parent: unknown,
  args: { key: string; value: string },
  ctx: RelayRpcContext,
) {
  return relayRpc<boolean>(ctx, 'set_setting', { key: args.key, value: args.value });
}
