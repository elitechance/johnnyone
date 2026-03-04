import { Injectable } from '@angular/core';
import { invoke } from '@tauri-apps/api/core';
import { listen, UnlistenFn } from '@tauri-apps/api/event';
import { Observable } from 'rxjs';

// ── Agent Connection ─────────────────────────────────────────────────────────

export interface ConnectionStatus {
  connected: boolean;
  session_id: string | null;
  last_heartbeat: string | null;
}

// ── Sessions ─────────────────────────────────────────────────────────────────

export interface Session {
  id: string;
  title: string;
  provider: string;
  model: string;
  working_directory: string;
  status: 'active' | 'archived';
  total_input_tokens: number;
  total_output_tokens: number;
  total_cost_cents: number;
  created_at: string;
  updated_at: string;
}

export interface CreateSessionInput {
  provider?: string;
  model?: string;
  working_directory?: string;
  title?: string;
}

// ── Messages ─────────────────────────────────────────────────────────────────

export interface Message {
  id: string;
  session_id: string;
  role: 'user' | 'assistant' | 'system' | 'tool';
  content: string;
  tool_calls?: string;
  tool_call_id?: string;
  finish_reason?: string;
  input_tokens: number;
  output_tokens: number;
  cost_cents: number;
  created_at: string;
}

// ── Chat Delta (Streaming) ───────────────────────────────────────────────────

export interface StreamChunk {
  chunk_type: 'text' | 'tool_use' | 'tool_result' | 'result' | 'error' | 'stderr';
  content: string;
  tool_name?: string;
  tool_input?: Record<string, unknown>;
  tool_output?: string;
  cost_usd?: number;
  input_tokens?: number;
  output_tokens?: number;
  is_final: boolean;
}

export interface ChatDelta {
  session_id: string;
  message_id: string;
  chunk: StreamChunk;
}

export interface ChatComplete {
  session_id: string;
  message_id: string;
}

// ── Provider Config ──────────────────────────────────────────────────────────

export interface ProviderConfig {
  id: string;
  provider: string;
  cli_path: string;
  api_key: string;
  default_model: string;
  settings: string;
  is_available: boolean;
  updated_at: string;
}

export interface UpsertProviderConfigInput {
  provider: string;
  cli_path?: string;
  api_key?: string;
  default_model?: string;
  settings?: string;
}

export interface DetectedTool {
  provider: string;
  command: string;
  found: boolean;
  path?: string;
}

// ── Tool Executions ──────────────────────────────────────────────────────────

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
  // ── Agent Connection ─────────────────────────────────────────────────

  async connectAgent(): Promise<void> {
    await invoke('start_agent');
  }

  async disconnectAgent(): Promise<void> {
    await invoke('stop_agent');
  }

  async getConnectionStatus(): Promise<ConnectionStatus> {
    return await invoke<ConnectionStatus>('get_connection_status');
  }

  // ── Sessions ─────────────────────────────────────────────────────────

  async createSession(input: CreateSessionInput): Promise<Session> {
    return await invoke<Session>('create_session', { input });
  }

  async listSessions(status?: string): Promise<Session[]> {
    return await invoke<Session[]>('list_sessions', { status: status ?? null });
  }

  async getSession(id: string): Promise<Session> {
    return await invoke<Session>('get_session', { id });
  }

  async updateSessionTitle(id: string, title: string): Promise<Session> {
    return await invoke<Session>('update_session_title', { id, title });
  }

  async updateSessionWorkingDirectory(id: string, workingDirectory: string): Promise<Session> {
    return await invoke<Session>('update_session_working_directory', { id, workingDirectory });
  }

  async updateSessionProvider(id: string, provider: string): Promise<Session> {
    return await invoke<Session>('update_session_provider', { id, provider });
  }

  async archiveSession(id: string): Promise<Session> {
    return await invoke<Session>('archive_session', { id });
  }

  async deleteSession(id: string): Promise<boolean> {
    return await invoke<boolean>('delete_session', { id });
  }

  // ── Chat Messages ────────────────────────────────────────────────────

  async sendChatMessage(sessionId: string, content: string): Promise<Message> {
    return await invoke<Message>('send_chat_message', { sessionId, content });
  }

  async stopGeneration(sessionId: string): Promise<void> {
    await invoke('stop_generation', { sessionId });
  }

  async listMessages(sessionId: string, limit?: number, offset?: number): Promise<Message[]> {
    return await invoke<Message[]>('list_messages', {
      sessionId,
      limit: limit ?? null,
      offset: offset ?? null,
    });
  }

  /**
   * Subscribe to streaming chat deltas via Tauri events.
   * Returns an Observable that emits ChatDelta events.
   */
  onChatDelta(): Observable<ChatDelta> {
    return new Observable<ChatDelta>((subscriber) => {
      let unlisten: UnlistenFn | undefined;

      listen<ChatDelta>('chat:delta', (event) => {
        subscriber.next(event.payload);
      }).then((fn) => {
        unlisten = fn;
      });

      return () => {
        unlisten?.();
      };
    });
  }

  /**
   * Subscribe to chat completion events.
   */
  onChatComplete(): Observable<ChatComplete> {
    return new Observable<ChatComplete>((subscriber) => {
      let unlisten: UnlistenFn | undefined;

      listen<ChatComplete>('chat:complete', (event) => {
        subscriber.next(event.payload);
      }).then((fn) => {
        unlisten = fn;
      });

      return () => {
        unlisten?.();
      };
    });
  }

  // ── Providers ────────────────────────────────────────────────────────

  async listProviderConfigs(): Promise<ProviderConfig[]> {
    return await invoke<ProviderConfig[]>('list_provider_configs');
  }

  async upsertProviderConfig(input: UpsertProviderConfigInput): Promise<ProviderConfig> {
    return await invoke<ProviderConfig>('upsert_provider_config', { input });
  }

  async deleteProviderConfig(provider: string): Promise<void> {
    await invoke('delete_provider_config', { provider });
  }

  async detectCliTools(): Promise<DetectedTool[]> {
    return await invoke<DetectedTool[]>('detect_cli_tools');
  }

  // ── Settings ─────────────────────────────────────────────────────────

  async getSetting(key: string): Promise<string> {
    return await invoke<string>('get_setting', { key });
  }

  async setSetting(key: string, value: string): Promise<void> {
    await invoke('set_setting', { key, value });
  }

  // ── Dialog ────────────────────────────────────────────────────────────

  async pickDirectory(): Promise<string | null> {
    try {
      // Dynamic import to avoid issues when not in Tauri context
      const { open } = await import('@tauri-apps/plugin-dialog');
      const selected = await open({
        directory: true,
        multiple: false,
        title: 'Select Working Directory',
      });
      return typeof selected === 'string' ? selected : null;
    } catch {
      return null;
    }
  }

  // ── Tool Executions ──────────────────────────────────────────────────

  async getToolExecutions(): Promise<ToolExecution[]> {
    return await invoke<ToolExecution[]>('get_tool_executions');
  }

  async getPendingApprovals(): Promise<PendingApproval[]> {
    return await invoke<PendingApproval[]>('get_pending_approvals');
  }

  async approveTool(callId: string): Promise<void> {
    await invoke('approve_tool', { callId });
  }

  async denyTool(callId: string): Promise<void> {
    await invoke('deny_tool', { callId });
  }
}
