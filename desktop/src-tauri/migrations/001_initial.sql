-- JohnnyOne Desktop Local Schema
-- All AI data lives locally on the desktop

CREATE TABLE IF NOT EXISTS sessions (
    id TEXT PRIMARY KEY,
    title TEXT NOT NULL DEFAULT 'New Session',
    provider TEXT NOT NULL DEFAULT 'claude_code',  -- claude_code, codex, cline, ollama
    model TEXT NOT NULL DEFAULT '',
    working_directory TEXT NOT NULL DEFAULT '',
    status TEXT NOT NULL DEFAULT 'active',          -- active, archived
    total_input_tokens INTEGER NOT NULL DEFAULT 0,
    total_output_tokens INTEGER NOT NULL DEFAULT 0,
    total_cost_cents INTEGER NOT NULL DEFAULT 0,    -- stored as cents to avoid float
    created_at TEXT NOT NULL DEFAULT (datetime('now')),
    updated_at TEXT NOT NULL DEFAULT (datetime('now'))
);

CREATE TABLE IF NOT EXISTS messages (
    id TEXT PRIMARY KEY,
    session_id TEXT NOT NULL REFERENCES sessions(id) ON DELETE CASCADE,
    role TEXT NOT NULL,                              -- user, assistant, system, tool
    content TEXT NOT NULL DEFAULT '',
    tool_calls TEXT,                                  -- JSON array of tool call refs
    tool_call_id TEXT,
    finish_reason TEXT,
    input_tokens INTEGER NOT NULL DEFAULT 0,
    output_tokens INTEGER NOT NULL DEFAULT 0,
    cost_cents INTEGER NOT NULL DEFAULT 0,
    created_at TEXT NOT NULL DEFAULT (datetime('now'))
);

CREATE INDEX IF NOT EXISTS idx_messages_session ON messages(session_id, created_at);

CREATE TABLE IF NOT EXISTS provider_configs (
    id TEXT PRIMARY KEY,
    provider TEXT NOT NULL UNIQUE,                   -- claude_code, codex, cline, ollama
    cli_path TEXT NOT NULL DEFAULT '',                -- path to CLI binary
    api_key TEXT NOT NULL DEFAULT '',                 -- stored locally (encrypted in future)
    default_model TEXT NOT NULL DEFAULT '',
    settings TEXT NOT NULL DEFAULT '{}',              -- JSON blob for provider-specific settings
    is_available INTEGER NOT NULL DEFAULT 0,          -- detected on system?
    updated_at TEXT NOT NULL DEFAULT (datetime('now'))
);

CREATE TABLE IF NOT EXISTS usage_log (
    id INTEGER PRIMARY KEY AUTOINCREMENT,
    session_id TEXT NOT NULL REFERENCES sessions(id) ON DELETE CASCADE,
    message_id TEXT,
    provider TEXT NOT NULL,
    model TEXT NOT NULL DEFAULT '',
    input_tokens INTEGER NOT NULL DEFAULT 0,
    output_tokens INTEGER NOT NULL DEFAULT 0,
    cost_cents INTEGER NOT NULL DEFAULT 0,
    created_at TEXT NOT NULL DEFAULT (datetime('now'))
);

CREATE TABLE IF NOT EXISTS settings (
    key TEXT PRIMARY KEY,
    value TEXT NOT NULL DEFAULT ''
);

-- Default settings
INSERT OR IGNORE INTO settings (key, value) VALUES ('last_working_directory', '');
INSERT OR IGNORE INTO settings (key, value) VALUES ('default_provider', 'claude_code');
INSERT OR IGNORE INTO settings (key, value) VALUES ('theme', 'dark');
