import { describe, it, expect } from 'vitest';

// The tab-shell/stream-lane DECISION logic is extracted into an Angular/Ionic-free
// module (decision D8 / D-tab-testability) so it can be unit-tested directly — the
// real `TerminalPage` pulls in Ionic + xterm, which the scaffold vitest (no Angular
// plugin) cannot import. The page delegates to exactly these functions.
import {
  paneTabOf,
  eventsAreStreaming,
  appendTranscriptEvent,
  diffStreamSubscriptions,
  DEFAULT_PANE_TAB,
  MAX_TRANSCRIPT_EVENTS,
} from './terminal-transcript-tab';
import type { StreamEvent } from '../../../../../ui/src/models/stream-event.model';

const ev = (p: Partial<StreamEvent>): StreamEvent => ({ sessionId: 's1', seq: 0, kind: 'text', ...p });

describe('terminal page — transcript tab shell', () => {
  it('defaults each pane to the Transcript tab', () => {
    expect(paneTabOf({}, 's1')).toBe('transcript');
    expect(DEFAULT_PANE_TAB).toBe('transcript');
  });

  it('reads a set tab, other panes stay on the default (setPaneTab isolation)', () => {
    const tabs = { s1: 'raw' as const }; // shape produced by setPaneTab(id,'raw')
    expect(paneTabOf(tabs, 's1')).toBe('raw');
    expect(paneTabOf(tabs, 's2')).toBe('transcript');
  });

  it('subscribes visible sessions and unsubscribes removed ones', () => {
    // s1 visible, nothing subscribed yet → subscribe s1.
    let d = diffStreamSubscriptions(new Set<string>(), new Set(['s1']));
    expect(d.toSubscribe).toEqual(['s1']);
    expect(d.toUnsubscribe).toEqual([]);
    // s1 subscribed, now nothing visible → unsubscribe s1.
    d = diffStreamSubscriptions(new Set(['s1']), new Set<string>());
    expect(d.toSubscribe).toEqual([]);
    expect(d.toUnsubscribe).toEqual(['s1']);
    // steady state (visible == subscribed) → no churn.
    d = diffStreamSubscriptions(new Set(['s1']), new Set(['s1']));
    expect(d.toSubscribe).toEqual([]);
    expect(d.toUnsubscribe).toEqual([]);
  });

  it('is streaming until the turn\'s final event', () => {
    expect(eventsAreStreaming([])).toBe(false);
    expect(eventsAreStreaming([ev({ text: 'a' })])).toBe(true);
    expect(eventsAreStreaming([ev({ seq: 1, text: 'b', final: true })])).toBe(false);
  });

  it('accumulates stream events per session without cross-leak', () => {
    let map: Record<string, StreamEvent[]> = {};
    map = appendTranscriptEvent(map, ev({ sessionId: 's1', text: 'a' }));
    map = appendTranscriptEvent(map, ev({ sessionId: 's2', text: 'b' }));
    expect(map['s1'].map((e) => e.text)).toEqual(['a']);
    expect(map['s2'].map((e) => e.text)).toEqual(['b']);
  });

  it('bounds per-session accumulation to the cap', () => {
    let map: Record<string, StreamEvent[]> = {};
    for (let i = 0; i < MAX_TRANSCRIPT_EVENTS + 5; i++) {
      map = appendTranscriptEvent(map, ev({ sessionId: 's1', seq: i, text: String(i) }));
    }
    expect(map['s1']).toHaveLength(MAX_TRANSCRIPT_EVENTS);
    // Oldest dropped, newest kept.
    expect(map['s1'][0].text).toBe('5');
    expect(map['s1'][MAX_TRANSCRIPT_EVENTS - 1].text).toBe(String(MAX_TRANSCRIPT_EVENTS + 4));
  });
});
