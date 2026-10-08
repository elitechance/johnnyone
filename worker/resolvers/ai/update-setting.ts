import { relayRpc, type RelayRpcContext } from '../../lib/runtime/relay-rpc';
import { authorizeForAltToken } from '../../lib/auth/api-key';

// Host settings surface: thin pass-through to the host `set_setting` RPC. Gated by the
// settings:write scope — settings include `files_root`, the root the host measures its own
// path confinement against, so this is strictly more privileged than files:write and must
// not be reachable by a files:write-only key (nor by an empty-scope jk_ key, which passes
// identity alone).
export default async function updateSetting(
  _parent: unknown,
  args: { key: string; value: string },
  ctx: RelayRpcContext,
) {
  await authorizeForAltToken(ctx, 'settings:write');
  return relayRpc<boolean>(ctx, 'set_setting', { key: args.key, value: args.value });
}
