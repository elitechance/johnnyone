import { Injectable } from '@angular/core';
import { invoke } from '@tauri-apps/api/core';

export interface ConnectionStatus {
  connected: boolean;
  session_id: string | null;
  last_heartbeat: string | null;
}

export interface ToolExecution {
  call_id: string;
  tool_name: string;
  status: 'pending' | 'running' | 'completed' | 'failed' | 'cancelled';
  input: Record<string, unknown>;
  output?: Record<string, unknown>;
  error?: string;
  started_at: string;
  completed_at?: string;
}

export interface PendingApproval {
  call_id: string;
  tool_name: string;
  description: string;
  input: Record<string, unknown>;
  requested_at: string;
}

/**
 * Angular service that bridges to Tauri backend commands.
 * All methods invoke Tauri IPC commands defined in the Rust backend.
 */
@Injectable({
  providedIn: 'root',
})
export class TauriBridgeService {
  /**
   * Start a WebSocket connection to the agent session.
   * This initiates the connection to the Cloudflare Worker AgentSessionDO.
   */
  async connectAgent(sessionId: string): Promise<void> {
    await invoke('start_agent', { sessionId });
  }

  /**
   * Disconnect the current agent WebSocket connection.
   */
  async disconnectAgent(): Promise<void> {
    await invoke('stop_agent');
  }

  /**
   * Get the current connection status of the agent.
   */
  async getConnectionStatus(): Promise<ConnectionStatus> {
    return await invoke<ConnectionStatus>('get_connection_status');
  }

  /**
   * Get the list of tool executions (pending, running, completed, failed).
   */
  async getToolExecutions(): Promise<ToolExecution[]> {
    return await invoke<ToolExecution[]>('get_tool_executions');
  }

  /**
   * Get the list of tool calls pending user approval.
   */
  async getPendingApprovals(): Promise<PendingApproval[]> {
    return await invoke<PendingApproval[]>('get_pending_approvals');
  }

  /**
   * Approve a pending tool execution.
   */
  async approveTool(callId: string): Promise<void> {
    await invoke('approve_tool', { callId });
  }

  /**
   * Deny a pending tool execution.
   */
  async denyTool(callId: string): Promise<void> {
    await invoke('deny_tool', { callId });
  }
}
