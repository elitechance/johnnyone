/**
 * Tool definition interfaces and built-in tool constants for JohnnyOne.
 */

export interface ToolParameterSchema {
  type: string;
  properties: Record<string, ToolParameterProperty>;
  required: string[];
}

export interface ToolParameterProperty {
  type: string;
  description: string;
  default?: unknown;
  enum?: string[];
  items?: ToolParameterProperty;
}

export interface ToolDefinition {
  slug: string;
  name: string;
  description: string;
  category: 'shell' | 'filesystem' | 'process' | 'browser';
  parametersSchema: ToolParameterSchema;
  executor: 'desktop' | 'cloud';
  requiresApproval: boolean;
}

// Built-in tool slug constants
export const TOOL_SHELL_EXECUTE = 'shell_execute';
export const TOOL_FILE_READ = 'file_read';
export const TOOL_FILE_WRITE = 'file_write';
export const TOOL_FILE_LIST = 'file_list';
export const TOOL_FILE_SEARCH = 'file_search';
export const TOOL_PROCESS_LIST = 'process_list';
export const TOOL_SYSTEM_INFO = 'system_info';

export const BUILT_IN_TOOL_SLUGS = [
  TOOL_SHELL_EXECUTE,
  TOOL_FILE_READ,
  TOOL_FILE_WRITE,
  TOOL_FILE_LIST,
  TOOL_FILE_SEARCH,
  TOOL_PROCESS_LIST,
  TOOL_SYSTEM_INFO,
] as const;

export type BuiltInToolSlug = (typeof BUILT_IN_TOOL_SLUGS)[number];

// Tools that require human approval before execution
export const APPROVAL_REQUIRED_TOOLS: BuiltInToolSlug[] = [
  TOOL_SHELL_EXECUTE,
  TOOL_FILE_WRITE,
];

// Parameter schema definitions for all built-in tools
export const BUILT_IN_TOOL_SCHEMAS: Record<BuiltInToolSlug, ToolParameterSchema> = {
  [TOOL_SHELL_EXECUTE]: {
    type: 'object',
    properties: {
      command: {
        type: 'string',
        description: 'The shell command to execute',
      },
      working_directory: {
        type: 'string',
        description: 'Working directory for the command (optional)',
      },
      timeout_ms: {
        type: 'integer',
        description: 'Timeout in milliseconds (default 30000)',
        default: 30000,
      },
    },
    required: ['command'],
  },

  [TOOL_FILE_READ]: {
    type: 'object',
    properties: {
      path: {
        type: 'string',
        description: 'Absolute or relative path to the file to read',
      },
      encoding: {
        type: 'string',
        description: 'File encoding (default utf-8)',
        default: 'utf-8',
      },
      max_bytes: {
        type: 'integer',
        description: 'Maximum bytes to read (default 1048576)',
        default: 1048576,
      },
    },
    required: ['path'],
  },

  [TOOL_FILE_WRITE]: {
    type: 'object',
    properties: {
      path: {
        type: 'string',
        description: 'Absolute or relative path to the file to write',
      },
      content: {
        type: 'string',
        description: 'Content to write to the file',
      },
      create_dirs: {
        type: 'boolean',
        description: 'Create parent directories if they do not exist (default true)',
        default: true,
      },
      append: {
        type: 'boolean',
        description: 'Append to existing file instead of overwriting (default false)',
        default: false,
      },
    },
    required: ['path', 'content'],
  },

  [TOOL_FILE_LIST]: {
    type: 'object',
    properties: {
      path: {
        type: 'string',
        description: 'Directory path to list',
      },
      recursive: {
        type: 'boolean',
        description: 'List files recursively (default false)',
        default: false,
      },
      pattern: {
        type: 'string',
        description: 'Glob pattern to filter results (optional)',
      },
      max_depth: {
        type: 'integer',
        description: 'Maximum depth for recursive listing (default 3)',
        default: 3,
      },
    },
    required: ['path'],
  },

  [TOOL_FILE_SEARCH]: {
    type: 'object',
    properties: {
      path: {
        type: 'string',
        description: 'Root directory to search from',
      },
      pattern: {
        type: 'string',
        description: 'Search pattern (regex supported)',
      },
      file_glob: {
        type: 'string',
        description: 'Glob pattern to filter which files to search (e.g. *.ts)',
      },
      case_sensitive: {
        type: 'boolean',
        description: 'Case sensitive search (default false)',
        default: false,
      },
      max_results: {
        type: 'integer',
        description: 'Maximum number of results to return (default 50)',
        default: 50,
      },
    },
    required: ['path', 'pattern'],
  },

  [TOOL_PROCESS_LIST]: {
    type: 'object',
    properties: {
      filter: {
        type: 'string',
        description: 'Filter process names by substring (optional)',
      },
      sort_by: {
        type: 'string',
        description: 'Sort by field: cpu, memory, pid, name (default cpu)',
        enum: ['cpu', 'memory', 'pid', 'name'],
        default: 'cpu',
      },
      limit: {
        type: 'integer',
        description: 'Maximum number of processes to return (default 25)',
        default: 25,
      },
    },
    required: [],
  },

  [TOOL_SYSTEM_INFO]: {
    type: 'object',
    properties: {
      sections: {
        type: 'array',
        description: 'Which info sections to include (default all)',
        items: {
          type: 'string',
          enum: ['os', 'cpu', 'memory', 'disk', 'network'],
          description: 'System info section',
        },
        default: ['os', 'cpu', 'memory', 'disk', 'network'],
      },
    },
    required: [],
  },
};

/**
 * Check if a tool slug is a built-in tool.
 */
export function isBuiltInTool(slug: string): slug is BuiltInToolSlug {
  return BUILT_IN_TOOL_SLUGS.includes(slug as BuiltInToolSlug);
}

/**
 * Check if a built-in tool requires human approval.
 */
export function requiresApproval(slug: string): boolean {
  return APPROVAL_REQUIRED_TOOLS.includes(slug as BuiltInToolSlug);
}

/**
 * Get the parameter schema for a built-in tool.
 */
export function getToolSchema(slug: string): ToolParameterSchema | null {
  if (isBuiltInTool(slug)) {
    return BUILT_IN_TOOL_SCHEMAS[slug];
  }
  return null;
}
