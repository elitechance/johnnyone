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