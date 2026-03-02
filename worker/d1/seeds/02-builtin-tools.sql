-- 02-builtin-tools.sql
-- Seed built-in tool definitions

INSERT OR IGNORE INTO ai_tool_definitions (id, tenant_id, slug, name, description, category, parameters_schema, executor, requires_approval, is_enabled, is_deleted, created_at, updated_at)
VALUES (
  '00000000-0000-0000-0001-000000000001',
  '00000000-0000-0000-0000-000000000001',
  'shell_execute',
  'Shell Execute',
  'Execute a shell command on the desktop agent and return stdout/stderr.',
  'shell',
  '{
    "type": "object",
    "properties": {
      "command": {
        "type": "string",
        "description": "The shell command to execute"
      },
      "working_directory": {
        "type": "string",
        "description": "Working directory for the command (optional)"
      },
      "timeout_ms": {
        "type": "integer",
        "description": "Timeout in milliseconds (default 30000)",
        "default": 30000
      }
    },
    "required": ["command"]
  }',
  'desktop',
  1,
  1,
  0,
  datetime('now'),
  datetime('now')
);

INSERT OR IGNORE INTO ai_tool_definitions (id, tenant_id, slug, name, description, category, parameters_schema, executor, requires_approval, is_enabled, is_deleted, created_at, updated_at)
VALUES (
  '00000000-0000-0000-0001-000000000002',
  '00000000-0000-0000-0000-000000000001',
  'file_read',
  'File Read',
  'Read the contents of a file from the desktop agent filesystem.',
  'filesystem',
  '{
    "type": "object",
    "properties": {
      "path": {
        "type": "string",
        "description": "Absolute or relative path to the file to read"
      },
      "encoding": {
        "type": "string",
        "description": "File encoding (default utf-8)",
        "default": "utf-8"
      },
      "max_bytes": {
        "type": "integer",
        "description": "Maximum bytes to read (default 1048576)",
        "default": 1048576
      }
    },
    "required": ["path"]
  }',
  'desktop',
  0,
  1,
  0,
  datetime('now'),
  datetime('now')
);

INSERT OR IGNORE INTO ai_tool_definitions (id, tenant_id, slug, name, description, category, parameters_schema, executor, requires_approval, is_enabled, is_deleted, created_at, updated_at)
VALUES (
  '00000000-0000-0000-0001-000000000003',
  '00000000-0000-0000-0000-000000000001',
  'file_write',
  'File Write',
  'Write content to a file on the desktop agent filesystem.',
  'filesystem',
  '{
    "type": "object",
    "properties": {
      "path": {
        "type": "string",
        "description": "Absolute or relative path to the file to write"
      },
      "content": {
        "type": "string",
        "description": "Content to write to the file"
      },
      "create_dirs": {
        "type": "boolean",
        "description": "Create parent directories if they do not exist (default true)",
        "default": true
      },
      "append": {
        "type": "boolean",
        "description": "Append to existing file instead of overwriting (default false)",
        "default": false
      }
    },
    "required": ["path", "content"]
  }',
  'desktop',
  1,
  1,
  0,
  datetime('now'),
  datetime('now')
);

INSERT OR IGNORE INTO ai_tool_definitions (id, tenant_id, slug, name, description, category, parameters_schema, executor, requires_approval, is_enabled, is_deleted, created_at, updated_at)
VALUES (
  '00000000-0000-0000-0001-000000000004',
  '00000000-0000-0000-0000-000000000001',
  'file_list',
  'File List',
  'List files and directories at a given path on the desktop agent.',
  'filesystem',
  '{
    "type": "object",
    "properties": {
      "path": {
        "type": "string",
        "description": "Directory path to list"
      },
      "recursive": {
        "type": "boolean",
        "description": "List files recursively (default false)",
        "default": false
      },
      "pattern": {
        "type": "string",
        "description": "Glob pattern to filter results (optional)"
      },
      "max_depth": {
        "type": "integer",
        "description": "Maximum depth for recursive listing (default 3)",
        "default": 3
      }
    },
    "required": ["path"]
  }',
  'desktop',
  0,
  1,
  0,
  datetime('now'),
  datetime('now')
);

INSERT OR IGNORE INTO ai_tool_definitions (id, tenant_id, slug, name, description, category, parameters_schema, executor, requires_approval, is_enabled, is_deleted, created_at, updated_at)
VALUES (
  '00000000-0000-0000-0001-000000000005',
  '00000000-0000-0000-0000-000000000001',
  'file_search',
  'File Search',
  'Search file contents using regex or text patterns across the desktop agent filesystem.',
  'filesystem',
  '{
    "type": "object",
    "properties": {
      "path": {
        "type": "string",
        "description": "Root directory to search from"
      },
      "pattern": {
        "type": "string",
        "description": "Search pattern (regex supported)"
      },
      "file_glob": {
        "type": "string",
        "description": "Glob pattern to filter which files to search (e.g. *.ts)"
      },
      "case_sensitive": {
        "type": "boolean",
        "description": "Case sensitive search (default false)",
        "default": false
      },
      "max_results": {
        "type": "integer",
        "description": "Maximum number of results to return (default 50)",
        "default": 50
      }
    },
    "required": ["path", "pattern"]
  }',
  'desktop',
  0,
  1,
  0,
  datetime('now'),
  datetime('now')
);

INSERT OR IGNORE INTO ai_tool_definitions (id, tenant_id, slug, name, description, category, parameters_schema, executor, requires_approval, is_enabled, is_deleted, created_at, updated_at)
VALUES (
  '00000000-0000-0000-0001-000000000006',
  '00000000-0000-0000-0000-000000000001',
  'process_list',
  'Process List',
  'List running processes on the desktop agent machine.',
  'process',
  '{
    "type": "object",
    "properties": {
      "filter": {
        "type": "string",
        "description": "Filter process names by substring (optional)"
      },
      "sort_by": {
        "type": "string",
        "description": "Sort by field: cpu, memory, pid, name (default cpu)",
        "enum": ["cpu", "memory", "pid", "name"],
        "default": "cpu"
      },
      "limit": {
        "type": "integer",
        "description": "Maximum number of processes to return (default 25)",
        "default": 25
      }
    },
    "required": []
  }',
  'desktop',
  0,
  1,
  0,
  datetime('now'),
  datetime('now')
);

INSERT OR IGNORE INTO ai_tool_definitions (id, tenant_id, slug, name, description, category, parameters_schema, executor, requires_approval, is_enabled, is_deleted, created_at, updated_at)
VALUES (
  '00000000-0000-0000-0001-000000000007',
  '00000000-0000-0000-0000-000000000001',
  'system_info',
  'System Info',
  'Retrieve system information from the desktop agent: OS, CPU, memory, disk usage.',
  'process',
  '{
    "type": "object",
    "properties": {
      "sections": {
        "type": "array",
        "items": {
          "type": "string",
          "enum": ["os", "cpu", "memory", "disk", "network"]
        },
        "description": "Which info sections to include (default all)",
        "default": ["os", "cpu", "memory", "disk", "network"]
      }
    },
    "required": []
  }',
  'desktop',
  0,
  1,
  0,
  datetime('now'),
  datetime('now')
);
