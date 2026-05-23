-- Temporary chat attachments stored in R2 until the backend host saves them
-- into the selected workspace.

CREATE TABLE IF NOT EXISTS chat_attachments (
  id TEXT PRIMARY KEY,
  tenant_id TEXT NOT NULL REFERENCES tenants(tenant_id),
  user_id TEXT NOT NULL REFERENCES users(id),
  session_id TEXT,
  original_name TEXT NOT NULL,
  content_type TEXT NOT NULL,
  size INTEGER NOT NULL,
  r2_key TEXT NOT NULL,
  status TEXT NOT NULL DEFAULT 'uploaded' CHECK (status IN ('uploaded', 'local_saved', 'deleted')),
  local_path TEXT,
  uploaded_at TEXT NOT NULL DEFAULT (datetime('now')),
  local_saved_at TEXT,
  deleted_at TEXT
);

CREATE INDEX IF NOT EXISTS idx_chat_attachments_tenant_user ON chat_attachments(tenant_id, user_id);
CREATE INDEX IF NOT EXISTS idx_chat_attachments_session ON chat_attachments(session_id);
CREATE INDEX IF NOT EXISTS idx_chat_attachments_status ON chat_attachments(status);
