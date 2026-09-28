/**
 * Pull the agent's CURRENT activity line out of a raw terminal screen.
 *
 * The transcript is fed by agent reports, which arrive in bursts — a long think can run minutes
 * with nothing to show. A bare spinner in that gap tells the user nothing, while the pane itself
 * is carrying exactly the answer: `✽ Pontificating… (9m 12s · ↓ 19.8k tokens)`.
 *
 * This is the one job the screen mirror is genuinely good at. Measured on a live pane, the status
 * row changes on ~93% of ticks while the body is static — it is the volatile band, and volatile
 * is precisely what a "still working" indicator wants. Committed content still comes from reports;
 * this only fills the wait.
 */

/** Spinner glyphs the agent CLIs prefix their status line with. */
const GLYPHS = ['✻', '✽', '✶', '✳', '✢', '·', '●', '◐', '◓', '◑', '◒', '⏺', '*', '→'];

/** `(12s`, `(9m 12s`, `(1h 2m` — a parenthesised elapsed time. */
const ELAPSED = /\((?:\d+h\s*)?(?:\d+m\s*)?\d+s\b/;

function stripGlyph(line: string): string {
  let out = line.trim();
  // A status line can stack a glyph and a bullet; peel any leading run of them.
  for (let guard = 0; guard < 4; guard++) {
    const first = out[0];
    if (first && GLYPHS.includes(first)) {
      out = out.slice(1).trim();
      continue;
    }
    break;
  }
  return out;
}

/**
 * The newest line that reads as "working on it": an ellipsis plus an elapsed timer.
 * Returns null when the pane shows no such line — the caller then falls back to a plain
 * indicator rather than inventing a status.
 */
export function agentActivityLine(content: string | null | undefined): string | null {
  if (!content) return null;
  const lines = content.split('\n');
  for (let i = lines.length - 1; i >= 0; i--) {
    const raw = lines[i];
    if (!raw || !raw.trim()) continue;
    // "Cogitated for 6s · done 1:15 PM" is a FINISHED turn, not current activity.
    if (/\bdone\b\s+\d{1,2}:\d{2}/.test(raw)) continue;
    const hasEllipsis = raw.includes('…') || raw.includes('...');
    if (!hasEllipsis || !ELAPSED.test(raw)) continue;
    const cleaned = stripGlyph(raw);
    if (cleaned.length < 3) continue;
    // Collapse the runs of spaces a TUI pads its columns with.
    return cleaned.replace(/\s{2,}/g, ' ').trim();
  }
  return null;
}

/**
 * Is the pane showing an agent mid-turn at all? Used to keep the waiting row honest when there is
 * no timer line yet — the CLIs all offer an interrupt affordance while a turn is running.
 */
export function agentIsBusy(content: string | null | undefined): boolean {
  if (!content) return false;
  return /esc to interrupt/i.test(content);
}
