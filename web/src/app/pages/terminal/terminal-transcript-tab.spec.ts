import { describe, expect, it } from 'vitest';
import { StreamEvent } from '@johnnyone/ui';
import { appendTranscriptEvent } from './terminal-transcript-tab';

describe('appendTranscriptEvent — repeat collapsing', () => {
  const ev = (seq: number, text: string, kind = 'text') =>
    ({ sessionId: 's1', seq, kind, text }) as StreamEvent;

  it('collapses an identical repeat of the newest row', () => {
    let by: Record<string, StreamEvent[]> = {};
    by = appendTranscriptEvent(by, ev(1, 'Committed to master'));
    by = appendTranscriptEvent(by, ev(2, 'Committed to master'));
    by = appendTranscriptEvent(by, ev(3, 'Committed to master'));
    expect(by['s1'].length).toBe(1);
  });

  it('keeps different text', () => {
    let by: Record<string, StreamEvent[]> = {};
    by = appendTranscriptEvent(by, ev(1, 'one'));
    by = appendTranscriptEvent(by, ev(2, 'two'));
    expect(by['s1'].map((e) => e.text)).toEqual(['one', 'two']);
  });

  it('keeps the same text when it recurs later, after something else', () => {
    let by: Record<string, StreamEvent[]> = {};
    by = appendTranscriptEvent(by, ev(1, 'build'));
    by = appendTranscriptEvent(by, ev(2, 'test'));
    by = appendTranscriptEvent(by, ev(3, 'build'));
    expect(by['s1'].map((e) => e.text)).toEqual(['build', 'test', 'build']);
  });

  it('does not collapse across kinds — the markdown report follows its own summary', () => {
    let by: Record<string, StreamEvent[]> = {};
    by = appendTranscriptEvent(by, ev(1, 'same', 'text'));
    by = appendTranscriptEvent(by, ev(2, 'same', 'code'));
    expect(by['s1'].length).toBe(2);
  });

  it('does not collapse across sessions', () => {
    let by: Record<string, StreamEvent[]> = {};
    by = appendTranscriptEvent(by, ev(1, 'x'));
    by = appendTranscriptEvent(by, { ...ev(1, 'x'), sessionId: 's2' } as StreamEvent);
    expect(by['s1'].length).toBe(1);
    expect(by['s2'].length).toBe(1);
  });
});
