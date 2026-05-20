-- 01-default-tenant.sql
-- Seed default tenant and admin user
-- Dev password: johnnyone-dev
-- Auth module creates tenants (tenant_id, preferred_tenant_id, display_name, is_active, ...)
-- and users (id, tenant_id, email, display_name, password_hash, status, ...)
-- Our 0000_initial.sql adds plan + settings columns to tenants

INSERT OR IGNORE INTO tenants (tenant_id, preferred_tenant_id, display_name, is_active, plan, settings, created_at, updated_at)
VALUES (
  '00000000-0000-0000-0000-000000000001',
  'default',
  'Default Tenant',
  1,
  'pro',
  '{"maxSessions": 100, "maxDesktopNodes": 10}',
  datetime('now'),
  datetime('now')
);

INSERT OR IGNORE INTO users (id, tenant_id, email, display_name, password_hash, status, created_at, updated_at)
VALUES (
  '00000000-0000-0000-0000-000000000002',
  '00000000-0000-0000-0000-000000000001',
  'admin@johnnyone.local',
  'Admin',
  '100000:XjiIcbojqYp8Is1OVL4kfA==:koyPbWNdgLUwy1Codv2+ZQbtZ0f7cGTaABDz0hJhL6c=',
  'active',
  datetime('now'),
  datetime('now')
);
