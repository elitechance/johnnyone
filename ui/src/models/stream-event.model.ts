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
