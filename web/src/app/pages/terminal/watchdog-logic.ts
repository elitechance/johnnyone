/**
 * Watchdog logic — a pure seam for the per-shell watchdog chip state and
 * transcript row-type mapping. DOM-free so it is loadable under vitest's
 * `node` environment with no Angular plugin.
 */

export type WatchdogChipState = 'off' | 'holding' | 'quiet' | 'nudged';

export interface WatchdogChipInput {
  enabled: boolean;
  lastNudgeAt: string | null;
  paneLastActiveAt: number;
  now: number;
  intervalSecs: number;
}

/**
 * Decide the chip state from the session's watchdog fields and the pane's activity.
 *
 * - off: not enabled.
 * - holding: enabled, pane last active < 20 s ago.
 * - quiet: enabled and pane last active >= 20 s ago, not nudged in the last 60 s.
 * - nudged: enabled and nudge within the last 60 s.
 */
export function watchdogChipState(input: WatchdogChipInput): WatchdogChipState {
  if (!input.enabled) return 'off';

  const nudgeAt = input.lastNudgeAt ? new Date(input.lastNudgeAt).getTime() : null;
  const nudgeSecsAgo = nudgeAt !== null ? (input.now - nudgeAt) / 1000 : null;
  const nudgedRecently = nudgeSecsAgo !== null && nudgeSecsAgo < 60;

  if (nudgedRecently) return 'nudged';

  const paneSecsAgo = (input.now - input.paneLastActiveAt) / 1000;
  if (paneSecsAgo < 20) return 'holding';

  return 'quiet';
}

export interface TranscriptRowEvent {
  role?: string;
  kind: string;
  language?: string | null;
}

/**
 * Map a session_reports row to a transcript row type.
 * When the role is 'watchdog', returns 'watchdog'.
 * Otherwise applies the existing mapping rules.
 */
export function transcriptRowType(ev: TranscriptRowEvent): 'you' | 'agent' | 'error' | 'markdown' | 'watchdog' {
  if (ev.role === 'watchdog') return 'watchdog';
  if (ev.kind === 'code' && ev.language === 'markdown') return 'markdown';
  if (ev.kind === 'prompt') return 'you';
  if (ev.kind === 'error') return 'error';
  return 'agent';
}