export interface ToolDefinition {
  id: string;
  slug: string;
  name: string;
  description: string;
  category: 'shell' | 'filesystem' | 'process' | 'browser';
  parametersSchema: Record<string, unknown>;
  executor: 'desktop' | 'cloud';
  requiresApproval: boolean;
  isEnabled: boolean;
}

export interface ToolExecution {
  id: string;
  sessionId: string;
  messageId: string;
  toolSlug: string;
  input: Record<string, unknown>;
  output?: string;
  status: 'pending' | 'approved' | 'running' | 'completed' | 'failed' | 'rejected' | 'cancelled';
  error?: string;
  durationMs?: number;
  executorNodeId?: string;
  createdAt: string;
  updatedAt: string;
}

export interface ToolCall {
  id: string;
  name: string;
  input: Record<string, unknown>;
}
