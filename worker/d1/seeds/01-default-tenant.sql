-- 01-default-tenant.sql
-- Seed default tenant and admin user

INSERT OR IGNORE INTO tenants (id, name, slug, plan, settings, is_deleted, created_at, updated_at)
VALUES (
  '00000000-0000-0000-0000-000000000001',
  'Default Tenant',
  'default',
  'pro',
  '{"maxSessions": 100, "maxDesktopNodes": 10}',
  0,
  datetime('now'),
  datetime('now')
);

INSERT OR IGNORE INTO users (id, tenant_id, email, password_hash, display_name, avatar_url, role, is_deleted, created_at, updated_at)
VALUES (
  '00000000-0000-0000-0000-000000000002',
  '00000000-0000-0000-0000-000000000001',
  'admin@johnnyone.local',
  '$argon2id$v=19$m=65536,t=3,p=4$c2VlZHBhc3N3b3Jk$placeholder_hash_replace_in_production',
  'Admin',
  NULL,
  'admin',
  0,
  datetime('now'),
  datetime('now')
);
