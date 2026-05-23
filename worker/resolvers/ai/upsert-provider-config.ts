import { relayRpc, type RelayRpcContext } from '../../lib/runtime/relay-rpc';

interface UpsertProviderConfigInput {
  provider: string;
  cliPath?: string;
  apiKey?: string;
  defaultModel?: string;
  settings?: string;
}

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

export default async function upsertProviderConfig(
  _parent: unknown,
  args: { input: UpsertProviderConfigInput },
  ctx: RelayRpcContext,
) {
  // camelCase → snake_case for the host's serde-deserialized input
  const input = {
    provider: args.input.provider,
    cli_path: args.input.cliPath ?? null,
    api_key: args.input.apiKey ?? null,
    default_model: args.input.defaultModel ?? null,
    settings: args.input.settings ?? null,
  };

  // Host returns snake_case; GraphQL schema expects camelCase — flip back.
  const r = await relayRpc<RustProviderConfig>(ctx, 'upsert_provider_config', { input });
  return {
    id: r.id,
    provider: r.provider,
    cliPath: r.cli_path,
    apiKey: r.api_key,
    defaultModel: r.default_model,
    settings: r.settings,
    isAvailable: r.is_available,
    updatedAt: r.updated_at,
  };
}
