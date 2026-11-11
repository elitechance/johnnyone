/**
 * Watchdog logic — a pure seam for the per-shell watchdog chip state and
 * transcript row-type mapping. DOM-free so it is loadable under vitest's
 * `node` environment with no Angular plugin.
 */

export type WatchdogChipState = 'off' | 'holding' | 'quiet' | 'nudged';

/**
 * Parse a host timestamp into epoch milliseconds.
 *
 * The engine stores `datetime('now')` in SQLite, which is **UTC** formatted
 * `YYYY-MM-DD HH:MM:SS` — a space separator and NO timezone designator. JS resolves
 * that string as *local* time (`new Date('2026-11-02 20:33:38')`), so on a UTC-4 box
 * every host timestamp lands four hours in the future and every elapsed calculation
 * computed from it goes negative — the watchdog chip reported "armed -17320s ago" and
 * its `nudgeSecsAgo < 60` gate was permanently true, so the chip sat stuck on
 * "Blocked on a question · nudged Ns ago" for a nudge that never happened.
 *
 * Rules:
 * - `null` / `undefined` / empty → `null`.
 * - A string already carrying a zone (`Z`, or `+HH:MM` / `-HH:MM` after the time) is
 *   parsed as-is — the producer already said what zone it means.
 * - Otherwise it is a bare host timestamp: normalise the space separator to `T`,
 *   append `Z`, and parse as UTC.
 *
 * Returns `null` when nothing was actually parsed (`NaN`), never a bogus number —
 * a caller's `!== null` guard must not be fooled into computing elapsed time off 0.
 */
export function parseHostTimestamp(ts: string | null | undefined): number | null {
  if (!ts) return null;
  // Already zoned: a trailing `Z`, or an offset after the time component.
  const ZONED = /\d{2}:\d{2}(?::\d{2}(?:\.\d+)?)?(?:Z|[+-]\d{2}:?\d{2})$/;
  if (ZONED.test(ts)) {
    const zoned = Date.parse(ts);
    return Number.isNaN(zoned) ? null : zoned;
  }
  const utc = Date.parse(ts.replace(' ', 'T') + 'Z');
  return Number.isNaN(utc) ? null : utc;
}

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

/** The watchdog signals, hydrated from one session's watchdog fields. */
export interface WatchdogSettings {
  enabled: boolean;
  intervalSecs: number;
  prompt: string;
  /** "Also nudge when: the pane is blocked on a question" (migration 029). */
  onBlocked: boolean;
  /** "...nothing has been reported for twice the interval" (migration 029). */
  onStaleReport: boolean;
}

/** What the signals hold before any session has been seen. */
export const WATCHDOG_SIGNAL_DEFAULTS: WatchdogSettings = {
  enabled: false,
  intervalSecs: 300,
  prompt: '',
  onBlocked: false,
  onStaleReport: false,
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
  watchdogOnBlocked?: boolean | null;
  watchdogOnStaleReport?: boolean | null;
} | null | undefined): WatchdogSettings {
  if (!session) return { ...WATCHDOG_SIGNAL_DEFAULTS };
  return {
    enabled: session.watchdogEnabled === true,
    intervalSecs: session.watchdogIntervalSecs ?? WATCHDOG_SIGNAL_DEFAULTS.intervalSecs,
    prompt: session.watchdogPrompt ?? WATCHDOG_SIGNAL_DEFAULTS.prompt,
    // Strict `=== true` for the same reason as `enabled`: absent (deploy skew, or a row predating
    // migration 029) means unchecked, and `undefined` must never become a truthy checkbox.
    onBlocked: session.watchdogOnBlocked === true,
    onStaleReport: session.watchdogOnStaleReport === true,
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
  } catch (err) {
    // The API client throws `GraphQLRequestError`, an `Error` whose `.message` IS the server's
    // text ("Watchdog prompt contains control characters"). Without it the strip could only say
    // "the server refused" and the user had to guess which of a dozen refusals it was.
    const reason = err instanceof Error && err.message.trim() ? err.message.trim() : '';
    if (!reason) return failure;
    // Only a verdict the SERVER actually issued may be quoted back. `GraphQLRequestError` is the
    // one type whose `.message` is the engine/worker's own text — its constructor sets `name` and
    // it carries `errors[]`. A bare `new Error('boom')` from a transport failure or a bug in the
    // handler is not the server's verdict, and quoting it would put words in the host's mouth, so
    // the ":" form is reserved for a real GraphQL error and anything else reads as plain `failure`.
    if (!isServerRefusal(err)) return failure;
    // Drop the trailing "." off the canned failure and lead in with ":" so the two read as one
    // sentence: "…the server refused: Watchdog prompt contains control characters". A non-Error
    // throw (a string, a rejection payload) has no usable message and falls through above.
    return `${failure.replace(/\.\s*$/, '')}: ${reason}`;
  }
}

/**
 * True when the thrown value carries the engine/worker's own verdict — the `GraphQLRequestError`
 * the API client raises when the response comes back with `errors[]` (its `.message` is exactly the
 * server's text, joined with "; "). Anything else is a local failure and reads as plain `failure`.
 */
function isServerRefusal(err: unknown): boolean {
  if (!err || typeof err !== 'object') return false;
  const name = (err as { name?: unknown }).name;
  return typeof name === 'string' && name.includes('GraphQL');
}

/**
 * Normalise a watchdog prompt for the wire.
 *
 * The host rejects ANY control character except tab, but the prompt is a textarea — so pressing
 * Enter, or pasting a multi-line block, made every subsequent Save fail with a refusal the user
 * could not edit their way out of. Newlines carry no meaning in a nudge that gets typed into a
 * pane, so a run of control characters (other than `\t`) collapses to a single space, runs of
 * spaces collapse to one, and the ends are trimmed. A single-line prompt is untouched.
 */
export function normalizeWatchdogPrompt(s: string): string {
  return s
    .replace(/[\u0000-\u0008\u000A-\u001F\u007F]+/g, ' ')
    .replace(/ {2,}/g, ' ')
    .trim();
}

/**
 * The footnote text for the watchdog's last-nudge line.
 *
 * It used to read "armed {{ wdArmedAgo() }} ago" off a fallback to the session's `created_at`, so
 * a shell created 33 days ago claimed the watchdog had been armed for 792h. No armed-at timestamp
 * is stored anywhere, so the only honest clock is the last nudge — and when there hasn't been one,
 * say so instead of inventing a duration.
 */
export function watchdogLastNudgeText(
  enabled: boolean,
  lastNudgeAt: number | null,
  now: number,
): string {
  if (!enabled) return 'off';
  if (lastNudgeAt === null) return 'no nudge yet';
  const secs = Math.round((now - lastNudgeAt) / 1000);
  if (secs < 60) return `last nudge ${secs}s ago`;
  if (secs < 3600) return `last nudge ${Math.floor(secs / 60)}m ago`;
  return `last nudge ${Math.floor(secs / 3600)}h ago`;
}

/**
 * The session as it is AFTER a watchdog enable/disable succeeded.
 *
 * `toggleWatchdogEnabled` set the `wdEnabled`/`wdPrompt` signals but left `currentSession()`
 * holding the session as loaded, and the status banner (`watchdogChipData()`) plus the footnote
 * (`wdArmedAgo`) read THAT — so the strip kept saying "Watchdog off. This shell is not being
 * watched." under a switch that had just flipped to On, until a page reload.
 *
 * Returns a NEW object (never mutates the caller's session — the same shape also sits in the
 * `sessions()` list, and mutating an object a signal holds would not notify): `watchdogEnabled`
 * is the value that was just saved, and arming also records the prompt that was just seeded
 * (`watchdogPromptAfterEnable`) so the banner and the textarea agree. Disarming keeps the
 * session's existing `watchdogPrompt` — the host leaves it stored, and the next arm offers the
 * user's own words back. Every other field is copied unchanged.
 */
export function sessionAfterWatchdogToggle<T extends { watchdogEnabled?: boolean | null; watchdogPrompt?: string | null }>(
  session: T,
  enabled: boolean,
  prompt: string,
): T {
  return {
    ...session,
    watchdogEnabled: enabled,
    ...(enabled ? { watchdogPrompt: prompt } : {}),
  };
}

export function transcriptRowType(ev: TranscriptRowEvent): 'you' | 'agent' | 'error' | 'markdown' | 'watchdog' {
  if (ev.role === 'watchdog') return 'watchdog';
  if (ev.kind === 'code' && ev.language === 'markdown') return 'markdown';
  if (ev.kind === 'prompt') return 'you';
  if (ev.kind === 'error') return 'error';
  return 'agent';
}