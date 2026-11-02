/**
 * Watchdog logic — a pure seam for the per-shell watchdog chip state and
 * transcript row-type mapping. DOM-free so it is loadable under vitest's
 * `node` environment with no Angular plugin.
 */

export type WatchdogChipState = 'off' | 'holding' | 'quiet' | 'nudged';

/**
 * Format a countdown in seconds to "M:SS", clamped at 0.
 */
export function formatCountdown(secs: number): string {
  const clamped = Math.max(0, Math.round(secs));
  const m = Math.floor(clamped / 60);
  const s = clamped % 60;
  return `${m}:${s.toString().padStart(2, '0')}`;
}

/**
 * Compute the countdown seconds for the watchdog ETA display.
 * Moves the Math.max arithmetic out of the template so the component
 * does not need a `Math` alias.
 */
export function watchdogEtaSeconds(
  state: WatchdogChipState,
  intervalSecs: number,
  paneSecsAgo: number,
  quietSecs: number,
): number {
  if (state === 'holding') {
    return Math.max(0, intervalSecs - paneSecsAgo);
  }
  return Math.max(0, intervalSecs - quietSecs);
}

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

/**
 * The nudge prompt the host backfills when a session is armed with an empty one.
 *
 * The sweep in `services/session_watchdog.rs` only considers sessions whose
 * `watchdog_prompt != ''`, and migration 027 defaults that column to `''` — so
 * arming a session that has never had a prompt typed into it used to flip
 * `watchdog_enabled = 1` while the sweep skipped it forever: "On", but inert.
 * The host now writes `DEFAULT_WATCHDOG_PROMPT` in the same statement that
 * enables the session; this string is that constant, kept byte-identical so the
 * textarea shows the text that will actually be typed instead of an empty box
 * that reads armed-but-does-nothing. `{quiet}`, `{name}` and `{nudges}` are
 * substituted by the sweep.
 */
export const DEFAULT_WATCHDOG_PROMPT =
  'You have been quiet for {quiet}. Report where {name} stands right now: ' +
  'what you just finished, what you are working on, and anything blocking you. ' +
  'This is nudge {nudges}.';

/**
 * The prompt to show (and send) for a session after arming it.
 *
 * An empty stored prompt means the host is about to backfill `DEFAULT_WATCHDOG_PROMPT`,
 * so the UI seeds the same text rather than leaving the box blank. A prompt the user
 * has already written is returned untouched — never overwrite their words.
 */
export function watchdogPromptAfterEnable(storedPrompt: string | null | undefined): string {
  return storedPrompt && storedPrompt.trim() !== '' ? storedPrompt : DEFAULT_WATCHDOG_PROMPT;
}

/** The three watchdog signals, hydrated from one session's watchdog fields. */
export interface WatchdogSettings {
  enabled: boolean;
  intervalSecs: number;
  prompt: string;
}

/** What the signals hold before any session has been seen. */
export const WATCHDOG_SIGNAL_DEFAULTS: WatchdogSettings = {
  enabled: false,
  intervalSecs: 300,
  prompt: '',
};

/**
 * Seed the watchdog signals from a session.
 *
 * Nullable on the wire (deploy skew) and on rows written before migration 027, so
 * every field falls back to the signal's initial value rather than to `undefined` —
 * an `undefined` interval renders "NaN min" in the interval picker.
 *
 * `prompt` is taken verbatim from the session: hydration must not invent a prompt the
 * server does not have, or the textarea would read 190/600 chars for a nudge that was
 * never stored. Only arming (`watchdogPromptAfterEnable`) backfills the default.
 */
export function watchdogSettingsFromSession(session: {
  watchdogEnabled?: boolean | null;
  watchdogIntervalSecs?: number | null;
  watchdogPrompt?: string | null;
} | null | undefined): WatchdogSettings {
  if (!session) return { ...WATCHDOG_SIGNAL_DEFAULTS };
  return {
    enabled: session.watchdogEnabled === true,
    intervalSecs: session.watchdogIntervalSecs ?? WATCHDOG_SIGNAL_DEFAULTS.intervalSecs,
    prompt: session.watchdogPrompt ?? WATCHDOG_SIGNAL_DEFAULTS.prompt,
  };
}

/**
 * Whether the watchdog signals should be re-seeded. True only when the session id
 * CHANGED — the effect that hydrates the toggle must not re-run on every signal read,
 * or it would overwrite the keystrokes of someone typing in the prompt textarea.
 */
export function shouldHydrateWatchdogSignals(
  hydratedFor: string | null,
  sessionId: string | null | undefined,
): boolean {
  return (sessionId ?? null) !== hydratedFor;
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
/**
 * Run one watchdog save and turn a rejection into a message the view can show.
 *
 * The three handlers used to end in `catch { /* nop *\/ }`, so a refused save left the toggle
 * where it was and the strip said nothing — the user could click "Save" forever and the only
 * evidence was a console line. This keeps the try/catch out of the component and returns the
 * string that belongs in `wdError`: `null` on success, `failure` on rejection.
 */
export async function runWatchdogAction(
  action: () => Promise<unknown>,
  failure: string,
): Promise<string | null> {
  try {
    await action();
    return null;
  } catch {
    return failure;
  }
}

export function transcriptRowType(ev: TranscriptRowEvent): 'you' | 'agent' | 'error' | 'markdown' | 'watchdog' {
  if (ev.role === 'watchdog') return 'watchdog';
  if (ev.kind === 'code' && ev.language === 'markdown') return 'markdown';
  if (ev.kind === 'prompt') return 'you';
  if (ev.kind === 'error') return 'error';
  return 'agent';
}