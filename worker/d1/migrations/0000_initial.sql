-- 0000_initial.sql
-- Create the tenants table (required by auth module which references it via FK)
-- The auth module creates: users, sessions, role_assignments, oauth_accounts, passkey_credentials
-- but expects tenants to already exist (same as hub's 0000_initial.sql pattern)

CREATE TABLE IF NOT EXISTS tenants (
  tenant_id TEXT PRIMARY KEY,
  preferred_tenant_id TEXT NOT NULL UNIQUE,
  display_name TEXT,
  is_active INTEGER NOT NULL DEFAULT 1,
  created_at TEXT NOT NULL,
  updated_at TEXT NOT NULL,
  updated_by TEXT
);

-- JohnnyOne-specific extensions to tenants
ALTER TABLE tenants ADD COLUMN plan TEXT NOT NULL DEFAULT 'free';
ALTER TABLE tenants ADD COLUMN settings TEXT NOT NULL DEFAULT '{}';
