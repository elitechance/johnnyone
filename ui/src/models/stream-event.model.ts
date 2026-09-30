// Structured provider/agent stream event (overhaul P2, decision D6). Mirrors the Rust
// `StreamEvent` (desktop/src-tauri/src/events.rs) and the worker `stream_event` envelope payload —
// camelCase on the wire. The transcript renderer that consumes these is Phase 3; this is the type
// the relay service dispatches, no rendering here.
export interface StreamEvent {
  sessionId: string;
  /** Monotonic per turn/session, for ordering/dedup. */
  seq: number;
  kind: 'text' | 'tool_call' | 'tool_result' | 'code' | 'mermaid' | 'error' | string;
  /** Prose / code body / mermaid source / error message. */
  text?: string;
  /** For kind:"code". */
  language?: string;
  /** For tool_call/tool_result. */
  toolName?: string;
  /** Structured payload (tool args/result); opaque this phase. */
  data?: unknown;
  /** Last event of a turn. */
  final?: boolean;
}

/**
 * A persisted agent report (`reportAgentResult`), read back from the host DB.
 *
 * The live `StreamEvent` lane only delivers what arrives while a client is connected, so a
 * reloaded console used to start blank. These rows are the history it hydrates from.
 */
export interface SessionReport {
  id: string;
  sessionId: string;
  kind: string;
  role?: string | null;
  summary?: string | null;
  markdown?: string | null;
  createdAt: string;
}
