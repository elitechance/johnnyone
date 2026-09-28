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

/**
 * CSI/OSC escape sequences. Screen content arrives as the RAW pane — xterm interprets these, but
 * we are extracting plain text, so they must come off first or a colourful status line reads as
 * `[38;5;174m✽[39m Mustering…`. Real captured rows carry a colour change per word.
 */
// eslint-disable-next-line no-control-regex
const ANSI = /(?:\[[0-9;?]*[ -/]*[@-~]|\][^]*(?:|\\)|[@-Z\\-_])/g;

function stripAnsi(text: string): string {
  return text.replace(ANSI, '');
}

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
    // Strip escapes BEFORE any matching: the glyph, the ellipsis and the timer are each wrapped
    // in their own colour codes on a real pane.
    const raw = stripAnsi(lines[i] ?? '');
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
  // Colour codes can land mid-phrase, so match on the stripped text.
  return /esc to interrupt/i.test(stripAnsi(content));
}


/**
 * A comparison key for "has this pane changed?".
 *
 * Busy detection must not depend on one CLI's wording. Claude says `esc to interrupt`, Grok shows
 * a boxed composer and `Worked for 13s`, Codex differs again — matching any of those strings only
 * ever works for the CLI it was written against. What every agent has in common is that its pane
 * CHANGES while it works, so churn is the portable signal. This mirrors the host coordinator's
 * `normalize_terminal_snapshot_for_idle`, which settled the same question server-side.
 *
 * Escapes and the blinking cursor block are dropped so a colour tick or a blink is not mistaken
 * for progress.
 */
export function screenIdleKey(content: string | null | undefined): string {
  if (!content) return '';
  return stripAnsi(content)
    .split('\n')
    .map((line) => line.replace(/\u2588+/g, '').trimEnd())
    .filter((line) => line.trim().length > 0)
    .join('\n');
}


/** Default depth of the live pane preview shown while an agent works. */
export const PANE_TAIL_LINES = 20;

/**
 * The last meaningful lines of the pane, for a live preview while waiting.
 *
 * A spinner throws away information we already have. We poll the pane anyway, so show the work:
 * the tool calls and output scrolling past are far more reassuring than "Slithering… (4m 17s)",
 * and it costs nothing extra. Replaced by the agent's reported answer once it lands.
 *
 * Only unambiguous chrome is dropped — separator rules and box edges, which carry no information
 * once stripped of their layout. Anything that could be content is kept, because deciding what is
 * "chrome" per CLI is exactly the provider-specific guessing that broke busy-detection.
 */
export function paneTail(content: string | null | undefined, maxLines = PANE_TAIL_LINES): string[] {
  if (!content) return [];
  const lines = stripAnsi(content)
    .split('\n')
    .map((line) => line.replace(/\u2588+/g, '').trimEnd())
    .filter((line) => {
      const t = line.trim();
      if (!t) return false;
      // Pure rules / box edges: nothing but line-drawing, dashes or box corners.
      return !/^[\u2500-\u257F\-_=\s]+$/.test(t);
    });
  return lines.slice(-Math.max(1, maxLines));
}
