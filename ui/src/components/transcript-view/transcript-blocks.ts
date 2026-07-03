// Pure, framework-agnostic transcript mapping — NO Angular import, so it can be
// unit-tested directly in plain vitest+jsdom (decision D8, "pure functions tested
// directly, no TestBed"). The component re-exports these; the spec imports here.

import { StreamEvent } from '../../models/stream-event.model';

export type TranscriptBlockKind = 'prose' | 'code' | 'mermaid' | 'tool' | 'error';

export interface TranscriptBlock {
  kind: TranscriptBlockKind;
  /** prose (coalesced text), or a synthesized ```lang fence for code/mermaid. */
  markdown?: string;
  toolName?: string;
  data?: unknown;
  /** first event's seq (ordering). */
  seq: number;
}

/**
 * Pure map of a session's `StreamEvent[]` → ordered `TranscriptBlock[]`.
 * Consecutive `text` events coalesce into one prose block; every other kind
 * flushes the current prose and emits its own block. Unknown kinds are treated
 * as prose (forward-compatible — `StreamEvent.kind` is `… | string`). Routing
 * code/mermaid through a synthesized fence keeps ONE render path (decision D2).
 */
export function eventsToBlocks(events: StreamEvent[]): TranscriptBlock[] {
  const sorted = [...events].sort((a, b) => a.seq - b.seq);
  const blocks: TranscriptBlock[] = [];
  let prose: { parts: string[]; seq: number } | null = null;
  const flush = () => {
    if (prose) {
      blocks.push({ kind: 'prose', markdown: prose.parts.join(''), seq: prose.seq });
      prose = null;
    }
  };
  for (const e of sorted) {
    switch (e.kind) {
      case 'code':
        flush();
        blocks.push({
          kind: 'code',
          markdown: '```' + (e.language || '') + '\n' + (e.text || '') + '\n```',
          seq: e.seq,
        });
        break;
      case 'mermaid':
        flush();
        blocks.push({ kind: 'mermaid', markdown: '```mermaid\n' + (e.text || '') + '\n```', seq: e.seq });
        break;
      case 'tool_call':
      case 'tool_result':
        flush();
        blocks.push({ kind: 'tool', toolName: e.toolName, data: e.data, seq: e.seq });
        break;
      case 'error':
        flush();
        blocks.push({ kind: 'error', markdown: e.text || '', seq: e.seq });
        break;
      case 'text':
      default:
        if (!prose) prose = { parts: [], seq: e.seq };
        prose.parts.push(e.text || '');
    }
  }
  flush();
  return blocks;
}
