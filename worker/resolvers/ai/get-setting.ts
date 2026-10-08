import { relayRpc, type RelayRpcContext } from '../../lib/runtime/relay-rpc';
import { authorizeForAltToken } from '../../lib/auth/api-key';

// Host settings surface: thin pass-through to the host `get_setting` RPC. Gated by the
// settings:read scope rather than left on identity alone — settings values can carry
// webhook URLs and provider keys.
export default async function getSetting(
  _parent: unknown,
  args: { key: string },
  ctx: RelayRpcContext,
) {
  await authorizeForAltToken(ctx, 'settings:read');
  return relayRpc<string>(ctx, 'get_setting', { key: args.key });
}
