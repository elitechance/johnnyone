/** Built-in tool slug constants */
export const TOOL_SLUGS = {
  SHELL_EXECUTE: 'shell_execute',
  FILE_READ: 'file_read',
  FILE_WRITE: 'file_write',
  FILE_LIST: 'file_list',
  FILE_SEARCH: 'file_search',
  PROCESS_LIST: 'process_list',
  SYSTEM_INFO: 'system_info',
} as const;

export type ToolSlug = typeof TOOL_SLUGS[keyof typeof TOOL_SLUGS];

/** Tool categories */
export const TOOL_CATEGORIES = {
  SHELL: 'shell',
  FILESYSTEM: 'filesystem',
  PROCESS: 'process',
  BROWSER: 'browser',
} as const;

export type ToolCategory = typeof TOOL_CATEGORIES[keyof typeof TOOL_CATEGORIES];

/** JSON Schema parameter definitions for built-in tools */
export const TOOL_PARAMETER_SCHEMAS: Record<ToolSlug, object> = {
  shell_execute: {
    type: 'object',
    properties: {
      command: { type: 'string', description: 'The shell command to execute' },
      working_directory: { type: 'string', description: 'Working directory for the command' },
      timeout_ms: { type: 'number', description: 'Timeout in milliseconds', default: 30000 },
    },
    required: ['command'],
  },
  file_read: {
    type: 'object',
    properties: {
      path: { type: 'string', description: 'Absolute file path to read' },
      encoding: { type: 'string', description: 'File encoding', default: 'utf-8' },
      line_start: { type: 'number', description: 'Start reading from this line (1-based)' },
      line_end: { type: 'number', description: 'Stop reading at this line (inclusive)' },
    },
    required: ['path'],
  },
  file_write: {
    type: 'object',
    properties: {
      path: { type: 'string', description: 'Absolute file path to write' },
      content: { type: 'string', description: 'Content to write' },
      create_dirs: { type: 'boolean', description: 'Create parent directories', default: false },
      append: { type: 'boolean', description: 'Append to file instead of overwriting', default: false },
    },
    required: ['path', 'content'],
  },
  file_list: {
    type: 'object',
    properties: {
      path: { type: 'string', description: 'Directory path to list' },
      recursive: { type: 'boolean', description: 'List recursively', default: false },
      pattern: { type: 'string', description: 'Glob pattern to filter files' },
    },
    required: ['path'],
  },
  file_search: {
    type: 'object',
    properties: {
      path: { type: 'string', description: 'Directory to search in' },
      query: { type: 'string', description: 'Search query (regex supported)' },
      file_pattern: { type: 'string', description: 'Glob pattern to filter files' },
      max_results: { type: 'number', description: 'Maximum results to return', default: 50 },
    },
    required: ['path', 'query'],
  },
  process_list: {
    type: 'object',
    properties: {
      filter: { type: 'string', description: 'Filter processes by name' },
      sort_by: { type: 'string', enum: ['cpu', 'memory', 'name', 'pid'], default: 'cpu' },
      limit: { type: 'number', description: 'Max processes to return', default: 20 },
    },
  },
  system_info: {
    type: 'object',
    properties: {
      sections: {
        type: 'array',
        items: { type: 'string', enum: ['cpu', 'memory', 'disk', 'network', 'os'] },
        description: 'Which system info sections to include',
        default: ['cpu', 'memory', 'disk', 'os'],
      },
    },
  },
};

/** Blocked shell commands that should never be executed */
export const BLOCKED_COMMANDS = [
  'rm -rf /',
  'mkfs',
  'dd if=/dev/zero',
  ':(){:|:&};:',
  'chmod -R 777 /',
  'shutdown',
  'reboot',
  'halt',
  'poweroff',
  'init 0',
  'init 6',
] as const;
