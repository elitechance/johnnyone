// Pure, framework-free decision logic for the terminal pane's Transcript/Raw tab
// shell and its structured-stream lane. Kept Angular/Ionic-free so it can be unit
// -tested directly in plain vitest+jsdom (decision D8) — the page (which pulls in
// Ionic/xterm and can't be imported by the scaffold vitest) delegates to these.
//
// `import type` is erased at build time, so this module has NO runtime dependency
// on `@johnnyone/ui`.
import type { StreamEvent } from '@johnnyone/ui';

/** Per-session terminal pane tab. `plan`/`diff` are reserved structurally (D7). */
export type PaneTab = 'transcript' | 'raw' | 'plan' | 'diff';

export const DEFAULT_PANE_TAB: PaneTab = 'transcript';

/** Cap per-session transcript growth in long sessions. */
export const MAX_TRANSCRIPT_EVENTS = 2000;

/** Active tab for a pane, defaulting to the Transcript view (D6). */
export function paneTabOf(tabs: Record<string, PaneTab>, id: string): PaneTab {
  return tabs[id] ?? DEFAULT_PANE_TAB;
}

/** Live until the last event of the turn (`final`) — drives the tab's `.live` dot. */
export function eventsAreStreaming(events: StreamEvent[]): boolean {
  return events.length > 0 && !events[events.length - 1].final;
}

/** Bounded per-session append; returns a new map (signal-friendly, no mutation of input). */
export function appendTranscriptEvent(
  bySession: Record<string, StreamEvent[]>,
  event: StreamEvent,
  max: number = MAX_TRANSCRIPT_EVENTS,
): Record<string, StreamEvent[]> {
  const prev = bySession[event.sessionId] ?? [];
  const next = [...prev, event];
  if (next.length > max) {
    next.splice(0, next.length - max);
  }
  return { ...bySession, [event.sessionId]: next };
}

/**
 * The subscribe/unsubscribe decision for the stream lane, mirroring the visual
 * -subscription lane (D6): subscribe sessions newly visible, unsubscribe ones no
 * longer visible.
 */
export function diffStreamSubscriptions(
  current: Set<string>,
  visible: Set<string>,
): { toSubscribe: string[]; toUnsubscribe: string[] } {
  const toSubscribe = [...visible].filter((id) => !current.has(id));
  const toUnsubscribe = [...current].filter((id) => !visible.has(id));
  return { toSubscribe, toUnsubscribe };
}
