-- 0004_api_keys.sql
-- M2M scoped API keys (P2/optional, Phase 05).
-- Only SHA-256 hash of the secret is stored. Secret is returned once at create time.
-- Keys are tenant+user scoped and can be revoked/expired.
-- See Phase 05 docs and master plan §6.

CREATE TABLE api_keys (
  id TEXT PRIMARY KEY,
  tenant_id TEXT NOT NULL REFERENCES tenants(tenant_id),
  user_id TEXT NOT NULL REFERENCES users(id),
  name TEXT NOT NULL,
  key_hash TEXT NOT NULL,     -- SHA-256 hex of secret only (never plaintext)
  key_prefix TEXT NOT NULL,   -- non-secret display prefix e.g. jk_<keyId>
  scopes TEXT NOT NULL DEFAULT '[]',  -- JSON array of scope strings
  last_used_at TEXT,
  expires_at TEXT,
  revoked_at TEXT,
  is_deleted INTEGER NOT NULL DEFAULT 0,
  created_at TEXT NOT NULL DEFAULT (datetime('now')),
  updated_at TEXT NOT NULL DEFAULT (datetime('now'))
);

-- Tenant+user listing (leading tenant_id for isolation)
CREATE INDEX idx_api_keys_tenant_user ON api_keys(tenant_id, user_id);

-- Note: no partial unique on id (id is already PRIMARY KEY; the prior index was redundant)
