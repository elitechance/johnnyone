import { relayRpc, type RelayRpcContext } from '../../lib/runtime/relay-rpc';

interface RustProviderConfig {
  id: string;
  provider: string;
  cli_path: string;
  api_key: string;
  default_model: string;
  settings: string;
  is_available: boolean;
  updated_at: string;
}

export default async function listProviderConfigs(
  _parent: unknown,
  _args: Record<string, never>,
  ctx: RelayRpcContext,
) {
  // Host returns serde-default snake_case keys; GraphQL schema expects camelCase.
  // (The host's own GraphQL handler converts via `rename_fields = "camelCase"`,
  // but relay-RPC delivers the raw service struct — convert here.)
  const rows = await relayRpc<RustProviderConfig[]>(ctx, 'list_provider_configs', {});
  return rows.map((r) => ({
    id: r.id,
    provider: r.provider,
    cliPath: r.cli_path,
    apiKey: r.api_key,
    defaultModel: r.default_model,
    settings: r.settings,
    isAvailable: r.is_available,
    updatedAt: r.updated_at,
  }));
}
