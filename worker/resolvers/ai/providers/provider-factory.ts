import type { LlmProvider, ProviderConfig } from './provider-interface';
import { ClaudeProvider } from './claude-provider';
import { OpenAiProvider } from './openai-provider';
import { OllamaProvider } from './ollama-provider';

interface ProviderConfigRow {
  id: string;
  tenant_id: string;
  user_id: string;
  provider: string;
  model: string | null;
  api_key_ref: string | null;
  base_url: string | null;
  is_default: number;
  settings: string;
  is_deleted: number;
  created_at: string;
  updated_at: string;
}

interface ResolveOptions {
  db: D1Database;
  tenantId: string;
  userId: string;
  /** Override provider name (e.g., 'claude', 'openai', 'ollama') */
  provider?: string;
  /** Override model name */
  model?: string;
  /** Environment bindings for resolving API key secrets */
  env: Record<string, unknown>;
}

/**
 * Resolve a provider configuration from D1 and return a configured LlmProvider instance.
 *
 * Resolution order:
 * 1. If provider/model are specified, look for matching config
 * 2. Otherwise, use the user's default config
 * 3. Fall back to tenant-level default
 */
export async function createProvider(options: ResolveOptions): Promise<LlmProvider> {
  const { db, tenantId, userId, provider, model, env } = options;

  let configRow: ProviderConfigRow | null = null;

  if (provider) {
    // Look for specific provider config
    let sql = `SELECT * FROM ai_provider_configs WHERE tenant_id = ? AND user_id = ? AND provider = ? AND is_deleted = 0`;
    const bindings: (string | number)[] = [tenantId, userId, provider];

    if (model) {
      sql += ` AND model = ?`;
      bindings.push(model);
    }

    sql += ` ORDER BY is_default DESC LIMIT 1`;

    configRow = await db
      .prepare(sql)
      .bind(...bindings)
      .first<ProviderConfigRow>();
  }

  if (!configRow) {
    // Fall back to user's default
    configRow = await db
      .prepare(
        `SELECT * FROM ai_provider_configs WHERE tenant_id = ? AND user_id = ? AND is_default = 1 AND is_deleted = 0 LIMIT 1`,
      )
      .bind(tenantId, userId)
      .first<ProviderConfigRow>();
  }

  if (!configRow) {
    // Fall back to any available config for the user
    configRow = await db
      .prepare(
        `SELECT * FROM ai_provider_configs WHERE tenant_id = ? AND user_id = ? AND is_deleted = 0 ORDER BY created_at ASC LIMIT 1`,
      )
      .bind(tenantId, userId)
      .first<ProviderConfigRow>();
  }

  if (!configRow) {
    throw new Error(
      'No provider configuration found. Please configure an AI provider in settings.',
    );
  }

  const config: ProviderConfig = {
    id: configRow.id,
    provider: configRow.provider as 'claude' | 'openai' | 'ollama',
    model: model ?? configRow.model ?? '',
    apiKeyRef: configRow.api_key_ref,
    baseUrl: configRow.base_url,
    settings: JSON.parse(configRow.settings || '{}'),
  };

  // Resolve the API key from the environment or secret store
  const apiKey = resolveApiKey(config.apiKeyRef, env);

  switch (config.provider) {
    case 'claude':
      if (!apiKey) {
        throw new Error('Claude API key not configured. Set the api_key_ref in provider config.');
      }
      return new ClaudeProvider(config, apiKey);

    case 'openai':
      if (!apiKey) {
        throw new Error('OpenAI API key not configured. Set the api_key_ref in provider config.');
      }
      return new OpenAiProvider(config, apiKey);

    case 'ollama':
      // Ollama does not require an API key
      return new OllamaProvider(config);

    default:
      throw new Error(`Unsupported provider: ${config.provider}`);
  }
}

/**
 * Resolve an API key reference to an actual key value.
 * Supports environment variable references (e.g., "env:ANTHROPIC_API_KEY")
 * and direct values for development.
 */
function resolveApiKey(
  apiKeyRef: string | null,
  env: Record<string, unknown>,
): string | null {
  if (!apiKeyRef) return null;

  // If it starts with "env:", resolve from environment
  if (apiKeyRef.startsWith('env:')) {
    const envVar = apiKeyRef.slice(4);
    const value = env[envVar];
    if (typeof value === 'string') {
      return value;
    }
    throw new Error(`Environment variable ${envVar} not found for API key reference`);
  }

  // If it starts with "secret:", this would be resolved from a secrets manager
  if (apiKeyRef.startsWith('secret:')) {
    const secretName = apiKeyRef.slice(7);
    const value = env[secretName];
    if (typeof value === 'string') {
      return value;
    }
    throw new Error(`Secret ${secretName} not found for API key reference`);
  }

  // Direct value (for development only)
  return apiKeyRef;
}
