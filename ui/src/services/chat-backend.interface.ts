import { Observable } from 'rxjs';

/** Unified message type used by shared UI components. */
export interface ChatMessage {
  id: string;
  sessionId: string;
  role: 'user' | 'assistant' | 'system' | 'tool';
  content: string;
  toolCalls?: unknown[];
  finishReason?: string;
  inputTokens?: number;
  outputTokens?: number;
  createdAt: string;
}

/** Session summary type. */
export interface ChatSession {
  id: string;
  title: string;
  provider: string;
  model: string;
  status: 'active' | 'archived';
  createdAt: string;
  updatedAt: string;
}

/** Streaming delta event. */
export interface ChatDeltaEvent {
  sessionId: string;
  messageId: string;
  delta: string;
  chunkType: string;
  isFinal: boolean;
}

/**
 * Abstract chat backend interface.
 * Desktop implements via TauriBridgeService.
 * Mobile implements via GraphQL relay.
 */
export interface ChatBackend {
  /** Send a message and get back the saved user message. */
  sendMessage(sessionId: string, content: string): Promise<ChatMessage>;

  /** List messages for a session. */
  listMessages(sessionId: string): Promise<ChatMessage[]>;

  /** Create a new session. */
  createSession(provider?: string): Promise<ChatSession>;

  /** List sessions. */
  listSessions(): Promise<ChatSession[]>;

  /** Observable of streaming deltas. */
  onDelta(): Observable<ChatDeltaEvent>;

  /** Stop a running generation. */
  stopGeneration?(sessionId: string): Promise<void>;
}
