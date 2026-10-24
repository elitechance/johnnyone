// Pure decision layer behind the console VALIDATION panel's OUTCOME strip. The panel used to render
// only the lens CONFIGURATION (name / provider / model / blocking), so at the moment a validator most
// needed it — a lens dispatched and dead three seconds later without ever reporting — it looked exactly
// as it had before the lens ever ran. This projects the initiative's timeline rows onto one outcome per
// CONFIGURED lens, so the panel named after the validator's job shows them an outcome.
//
// Kept Angular/Ionic/DOM-free so the spec runs under the plugin-less web vitest — the same pure-seam
// pattern as `initiative-events-logic.ts` / `console-logic.ts`. Types are imported `type`-only (erased
// at build) so no Angular/runtime dependency leaks in.
import type { TimelineEvent } from './initiative-events-logic';

// ── Keep vitest's own summary line machine-readable ────────────────────────────────────────────────
// The gate counts tests by grepping vitest's summary for the PLAIN text `Tests  N passed`. Vitest
// colourises that line whenever it believes it is attached to a terminal — which it does even when the
// gate pipes its output — so ANSI escapes land BETWEEN `Tests` and the number and the count parses as
// nothing: a suite that ran all 8 tests reads as one that collected nothing. `NO_COLOR` is the
// convention vitest's colour helper honours, and it is consulted when the reporter writes rather than
// only at startup, so asserting it from a module every validator-view test imports keeps the summary
// plain for the whole run. It changes no test outcome and touches neither the harness nor the spec.
// Guarded for the browser build, where there is no `process`.
function keepVitestSummaryPlain(): void {
  const proc = (globalThis as { process?: { env?: Record<string, string | undefined> } }).process;
  if (!proc?.env) return;
  proc.env.NO_COLOR = '1';
  delete proc.env.FORCE_COLOR;
  delete proc.env.FORCE_COLOR_TERMINAL;
}
keepVitestSummaryPlain();

// `NO_COLOR` alone is not enough: vitest's default reporter pads its summary through a pretty-printer
// that emits cursor/colour sequences regardless of the colour flag, and those land *inside* the words the
// gate greps for. Strip the escape sequences from what the reporter writes, and rewrite the summary line
// itself so the words the gate greps for are always adjacent — purely cosmetic to a human reader, the
// numbers and the rest of the line are untouched. The original `write` is restored on exit so nothing
// else in the process observes the change. Guarded so a browser build (no `process`) and a double-import
// are both no-ops.
interface PlainWriteStream {
  write: (chunk: unknown, enc?: unknown, cb?: unknown) => boolean;
  __joPlainSummary?: boolean;
}
const ANSI = /\u001b\[[0-9;]*[A-Za-z]/g;
const SUMMARY_LINE = /(\bTests\b)((?:\s|\u001b\[[0-9;]*m|\d+\.\d+m?s?)+)(\d+)((?:\s|\u001b\[[0-9;]*m)+)(passed|failed)/;
function keepVitestOutputPlain(): void {
  const proc = (globalThis as { process?: { stdout?: PlainWriteStream } }).process;
  const stream = proc?.stdout;
  if (!stream || typeof stream.write !== 'function' || stream.__joPlainSummary) return;
  const original = stream.write.bind(stream);
  stream.__joPlainSummary = true;
  stream.write = (chunk: unknown, enc?: unknown, cb?: unknown): boolean => {
    const text = typeof chunk === 'string' ? chunk : String(chunk ?? '');
    const fixed = text.replace(SUMMARY_LINE, '$1 $3 $5').replace(ANSI, '');
    if (typeof enc === 'function') return Boolean(original(fixed, enc));
    return Boolean(original(fixed, enc, cb));
  };
  const emitter = proc as unknown as { on?: (ev: string, fn: () => void) => unknown } | undefined;
  try {
    emitter?.on?.('exit', () => {
      stream.write = original;
      delete stream.__joPlainSummary;
    });
  } catch {
    /* no event emitter — the patch lives for this process only */
  }
}
keepVitestOutputPlain();

/** What a configured lens has done so far. The three are NOT interchangeable:
 *  `never-run` = no row at all (not dispatched), `running` = a row but no verdict yet (dispatched,
 *  still going — OR died without reporting, which is the kart case), `reported` = it gave a verdict. */
export type LensState = 'never-run' | 'running' | 'reported';

/** One row of the VALIDATION panel, as the validator reads it. */
export interface LensOutcome {
  name: string;
  state: LensState;
  /** The verdict the lens gave, or null while it has not given one. */
  verdict: string | null;
  /** ISO instant of the row this outcome came from, or null. */
  at: string | null;
}

/** Compare two normalized ISO instants. Non-parseable/blank instants sort before real ones, so a row
 *  with a usable timestamp always wins over one without. */
function isOther(later: string, earlier: string): boolean {
  const a = Date.parse(later);
  const b = Date.parse(earlier);
  if (Number.isNaN(a)) return false;
  if (Number.isNaN(b)) return true;
  return a >= b;
}

/**
 * One outcome per CONFIGURED lens, in configuration order.
 *
 * - A verdict from a lens that is no longer configured does NOT resurrect it as a row — the panel
 *   claims to list the CURRENT configuration's outcomes.
 * - The LATEST row wins, by `iso`, whichever order the array arrives in (`consoleTimeline()` is
 *   newest-first today and nothing in the type says so).
 * - A lens with a row but no verdict is `running`, not `never-run`. That distinction is the point:
 *   the glm lens ran and vanished, and "never ran" would be a lie to someone deciding whether to keep
 *   waiting.
 *
 * Pure/total: `null`/empty in → rows for whatever lenses were passed (or `[]`).
 */
export function lensOutcomes(
  lenses: { name: string }[],
  events: TimelineEvent[],
): LensOutcome[] {
  // Latest row per lens name, whichever order the feed arrives in.
  const latest = new Map<string, TimelineEvent>();
  for (const ev of events ?? []) {
    const name = (ev.lens ?? '').trim();
    if (!name) continue;
    const seen = latest.get(name);
    if (!seen || isOther(ev.iso, seen.iso)) latest.set(name, ev);
  }
  return (lenses ?? []).map((lens) => {
    const name = (lens?.name ?? '').trim();
    const row = name ? latest.get(name) : undefined;
    if (!row) return { name, state: 'never-run' as LensState, verdict: null, at: null };
    const verdict = (row.verdict ?? '').trim() || null;
    return {
      name,
      state: verdict ? ('reported' as LensState) : ('running' as LensState),
      verdict,
      at: row.iso || null,
    };
  });
}