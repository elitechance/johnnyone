-- 0001_johnnyone.sql
-- AI session, messaging, tool execution, provider config, desktop node, and channel tables

CREATE TABLE IF NOT EXISTS ai_sessions (
  id TEXT PRIMARY KEY,
  tenant_id TEXT NOT NULL REFERENCES tenants(id),
  user_id TEXT NOT NULL REFERENCES users(id),
  title TEXT,
  provider TEXT,
  model TEXT,
  status TEXT NOT NULL DEFAULT 'active' CHECK (status IN ('active', 'archived', 'completed')),
  system_prompt TEXT,
  total_input_tokens INTEGER NOT NULL DEFAULT 0,
  total_output_tokens INTEGER NOT NULL DEFAULT 0,
  total_cost REAL NOT NULL DEFAULT 0,
  is_deleted INTEGER NOT NULL DEFAULT 0,
  created_at TEXT NOT NULL DEFAULT (datetime('now')),
  updated_at TEXT NOT NULL DEFAULT (datetime('now'))
);

CREATE TABLE IF NOT EXISTS ai_messages (
  id TEXT PRIMARY KEY,
  tenant_id TEXT NOT NULL REFERENCES tenants(id),
  session_id TEXT NOT NULL REFERENCES ai_sessions(id),
  role TEXT NOT NULL CHECK (role IN ('user', 'assistant', 'system', 'tool')),
  content TEXT,
  tool_calls TEXT NOT NULL DEFAULT '[]',
  tool_call_id TEXT,
  source_channel TEXT,
  finish_reason TEXT,
  input_tokens INTEGER,
  output_tokens INTEGER,
  created_at TEXT NOT NULL DEFAULT (datetime('now'))
);

CREATE TABLE IF NOT EXISTS ai_tool_definitions (
  id TEXT PRIMARY KEY,
  tenant_id TEXT NOT NULL REFERENCES tenants(id),
  slug TEXT NOT NULL UNIQUE,
  name TEXT NOT NULL,
  description TEXT,
  category TEXT NOT NULL CHECK (category IN ('shell', 'filesystem', 'process', 'browser')),
  parameters_schema TEXT NOT NULL DEFAULT '{}',
  executor TEXT NOT NULL CHECK (executor IN ('desktop', 'cloud')),
  requires_approval INTEGER NOT NULL DEFAULT 0,
  is_enabled INTEGER NOT NULL DEFAULT 1,
  is_deleted INTEGER NOT NULL DEFAULT 0,
  created_at TEXT NOT NULL DEFAULT (datetime('now')),
  updated_at TEXT NOT NULL DEFAULT (datetime('now'))
);

CREATE TABLE IF NOT EXISTS ai_tool_executions (
  id TEXT PRIMARY KEY,
  tenant_id TEXT NOT NULL REFERENCES tenants(id),
  session_id TEXT NOT NULL REFERENCES ai_sessions(id),
  message_id TEXT NOT NULL REFERENCES ai_messages(id),
  tool_definition_id TEXT REFERENCES ai_tool_definitions(id),
  tool_slug TEXT NOT NULL,
  input TEXT NOT NULL DEFAULT '{}',
  output TEXT,
  status TEXT NOT NULL DEFAULT 'pending' CHECK (status IN ('pending', 'approved', 'running', 'completed', 'failed', 'rejected', 'cancelled')),
  error TEXT,
  duration_ms INTEGER,
  executor_node_id TEXT,
  created_at TEXT NOT NULL DEFAULT (datetime('now')),
  updated_at TEXT NOT NULL DEFAULT (datetime('now'))
);

CREATE TABLE IF NOT EXISTS ai_provider_configs (
  id TEXT PRIMARY KEY,
  tenant_id TEXT NOT NULL REFERENCES tenants(id),
  user_id TEXT NOT NULL REFERENCES users(id),
  provider TEXT NOT NULL CHECK (provider IN ('claude', 'openai', 'ollama')),
  model TEXT,
  api_key_ref TEXT,
  base_url TEXT,
  is_default INTEGER NOT NULL DEFAULT 0,
  settings TEXT NOT NULL DEFAULT '{}',
  is_deleted INTEGER NOT NULL DEFAULT 0,
  created_at TEXT NOT NULL DEFAULT (datetime('now')),
  updated_at TEXT NOT NULL DEFAULT (datetime('now'))
);

CREATE TABLE IF NOT EXISTS desktop_nodes (
  id TEXT PRIMARY KEY,
  tenant_id TEXT NOT NULL REFERENCES tenants(id),
  user_id TEXT NOT NULL REFERENCES users(id),
  hostname TEXT NOT NULL,
  os TEXT,
  arch TEXT,
  version TEXT,
  status TEXT NOT NULL DEFAULT 'offline' CHECK (status IN ('online', 'offline', 'busy')),
  capabilities TEXT NOT NULL DEFAULT '[]',
  last_heartbeat_at TEXT,
  is_deleted INTEGER NOT NULL DEFAULT 0,
  created_at TEXT NOT NULL DEFAULT (datetime('now')),
  updated_at TEXT NOT NULL DEFAULT (datetime('now'))
);

CREATE TABLE IF NOT EXISTS channel_bindings (
  id TEXT PRIMARY KEY,
  tenant_id TEXT NOT NULL REFERENCES tenants(id),
  user_id TEXT NOT NULL REFERENCES users(id),
  channel_type TEXT NOT NULL CHECK (channel_type IN ('telegram', 'discord', 'whatsapp')),
  channel_identifier TEXT NOT NULL,
  channel_config TEXT NOT NULL DEFAULT '{}',
  is_active INTEGER NOT NULL DEFAULT 1,
  is_deleted INTEGER NOT NULL DEFAULT 0,
  created_at TEXT NOT NULL DEFAULT (datetime('now')),
  updated_at TEXT NOT NULL DEFAULT (datetime('now'))
);

CREATE TABLE IF NOT EXISTS ai_usage_log (
  id TEXT PRIMARY KEY,
  tenant_id TEXT NOT NULL REFERENCES tenants(id),
  user_id TEXT NOT NULL REFERENCES users(id),
  session_id TEXT REFERENCES ai_sessions(id),
  provider TEXT NOT NULL,
  model TEXT NOT NULL,
  input_tokens INTEGER NOT NULL DEFAULT 0,
  output_tokens INTEGER NOT NULL DEFAULT 0,
  total_tokens INTEGER NOT NULL DEFAULT 0,
  cost REAL NOT NULL DEFAULT 0,
  created_at TEXT NOT NULL DEFAULT (datetime('now'))
);

-- Indexes for ai_sessions
CREATE INDEX IF NOT EXISTS idx_ai_sessions_tenant_id ON ai_sessions(tenant_id);
CREATE INDEX IF NOT EXISTS idx_ai_sessions_user_id ON ai_sessions(user_id);
CREATE INDEX IF NOT EXISTS idx_ai_sessions_tenant_user ON ai_sessions(tenant_id, user_id);
CREATE INDEX IF NOT EXISTS idx_ai_sessions_status ON ai_sessions(status);

-- Indexes for ai_messages
CREATE INDEX IF NOT EXISTS idx_ai_messages_tenant_id ON ai_messages(tenant_id);
CREATE INDEX IF NOT EXISTS idx_ai_messages_session_id ON ai_messages(session_id);
CREATE INDEX IF NOT EXISTS idx_ai_messages_tenant_session ON ai_messages(tenant_id, session_id);

-- Indexes for ai_tool_definitions
CREATE INDEX IF NOT EXISTS idx_ai_tool_definitions_tenant_id ON ai_tool_definitions(tenant_id);
CREATE INDEX IF NOT EXISTS idx_ai_tool_definitions_slug ON ai_tool_definitions(slug);
CREATE INDEX IF NOT EXISTS idx_ai_tool_definitions_category ON ai_tool_definitions(category);

-- Indexes for ai_tool_executions
CREATE INDEX IF NOT EXISTS idx_ai_tool_executions_tenant_id ON ai_tool_executions(tenant_id);
CREATE INDEX IF NOT EXISTS idx_ai_tool_executions_session_id ON ai_tool_executions(session_id);
CREATE INDEX IF NOT EXISTS idx_ai_tool_executions_message_id ON ai_tool_executions(message_id);
CREATE INDEX IF NOT EXISTS idx_ai_tool_executions_tool_definition_id ON ai_tool_executions(tool_definition_id);
CREATE INDEX IF NOT EXISTS idx_ai_tool_executions_status ON ai_tool_executions(status);

-- Indexes for ai_provider_configs
CREATE INDEX IF NOT EXISTS idx_ai_provider_configs_tenant_id ON ai_provider_configs(tenant_id);
CREATE INDEX IF NOT EXISTS idx_ai_provider_configs_user_id ON ai_provider_configs(user_id);
CREATE INDEX IF NOT EXISTS idx_ai_provider_configs_tenant_user ON ai_provider_configs(tenant_id, user_id);

-- Indexes for desktop_nodes
CREATE INDEX IF NOT EXISTS idx_desktop_nodes_tenant_id ON desktop_nodes(tenant_id);
CREATE INDEX IF NOT EXISTS idx_desktop_nodes_user_id ON desktop_nodes(user_id);
CREATE INDEX IF NOT EXISTS idx_desktop_nodes_status ON desktop_nodes(status);

-- Indexes for channel_bindings
CREATE INDEX IF NOT EXISTS idx_channel_bindings_tenant_id ON channel_bindings(tenant_id);
CREATE INDEX IF NOT EXISTS idx_channel_bindings_user_id ON channel_bindings(user_id);
CREATE INDEX IF NOT EXISTS idx_channel_bindings_channel_type ON channel_bindings(channel_type);

-- Indexes for ai_usage_log
CREATE INDEX IF NOT EXISTS idx_ai_usage_log_tenant_id ON ai_usage_log(tenant_id);
CREATE INDEX IF NOT EXISTS idx_ai_usage_log_user_id ON ai_usage_log(user_id);
CREATE INDEX IF NOT EXISTS idx_ai_usage_log_session_id ON ai_usage_log(session_id);
CREATE INDEX IF NOT EXISTS idx_ai_usage_log_created_at ON ai_usage_log(created_at);
CREATE INDEX IF NOT EXISTS idx_ai_usage_log_tenant_user ON ai_usage_log(tenant_id, user_id);
