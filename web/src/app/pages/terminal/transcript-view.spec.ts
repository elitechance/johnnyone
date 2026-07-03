import { describe, it, expect } from 'vitest';

// Deep path into the ui library — the mapper is barrel-exported from
// `@johnnyone/ui`, but the web vitest config has no tsconfig-paths plugin. The
// deep path also keeps the barrel's heavier components (xterm, etc.) out of jsdom.
import { eventsToBlocks } from '../../../../../ui/src/components/transcript-view/transcript-blocks';
import type { StreamEvent } from '../../../../../ui/src/models/stream-event.model';

const ev = (p: Partial<StreamEvent>): StreamEvent => ({ sessionId: 's', seq: 0, kind: 'text', ...p });

describe('eventsToBlocks', () => {
  it('coalesces consecutive text events into one prose block', () => {
    const blocks = eventsToBlocks([
      ev({ seq: 0, kind: 'text', text: 'Hello ' }),
      ev({ seq: 1, kind: 'text', text: 'world' }),
    ]);
    expect(blocks).toHaveLength(1);
    expect(blocks[0].kind).toBe('prose');
    expect(blocks[0].markdown).toBe('Hello world');
  });

  it('maps a code event to a fenced code block by language', () => {
    const [b] = eventsToBlocks([ev({ seq: 0, kind: 'code', language: 'ts', text: 'const x = 1' })]);
    expect(b.kind).toBe('code');
    expect(b.markdown).toContain('```ts');
    expect(b.markdown).toContain('const x = 1');
  });

  it('maps a mermaid event to a mermaid fence', () => {
    const [b] = eventsToBlocks([ev({ seq: 0, kind: 'mermaid', text: 'graph TD; A-->B' })]);
    expect(b.kind).toBe('mermaid');
    expect(b.markdown).toContain('```mermaid');
  });

  it('maps tool_call/tool_result to a tool block carrying toolName', () => {
    const [b] = eventsToBlocks([
      ev({ seq: 0, kind: 'tool_call', toolName: 'read_file', data: { path: 'a.ts' } }),
    ]);
    expect(b.kind).toBe('tool');
    expect(b.toolName).toBe('read_file');
  });

  it('maps an error event to an error block', () => {
    const [b] = eventsToBlocks([ev({ seq: 0, kind: 'error', text: 'boom' })]);
    expect(b.kind).toBe('error');
    expect(b.markdown).toBe('boom');
  });

  it('orders blocks by seq and flushes prose around non-text events', () => {
    const blocks = eventsToBlocks([
      ev({ seq: 2, kind: 'text', text: 'after' }),
      ev({ seq: 0, kind: 'text', text: 'before' }),
      ev({ seq: 1, kind: 'error', text: 'mid' }),
    ]);
    expect(blocks.map((b) => b.kind)).toEqual(['prose', 'error', 'prose']);
    expect(blocks.map((b) => b.seq)).toEqual([0, 1, 2]);
  });
});
