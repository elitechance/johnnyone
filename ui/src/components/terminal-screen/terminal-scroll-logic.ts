// Pure decision/math layer behind the terminal's scroll-position preservation (console-features P02,
// C2). Extracted from the inline arithmetic + pin toggles in `terminal-screen.component.ts` so the
// "keep the reading position across a snapshot refresh" behavior is unit-provable without a browser
// (functional-validation override). Framework/DOM-free — no xterm/Angular imports.
//
// Co-located in the shared `ui` lib (NOT the web page) so the widget can import it WITHOUT a `ui → web`
// dependency (which would be a circular lib boundary). The web spec imports it via the normal
// `web → ui` direction. See `phases/02-.../tasks/02-*/decisions.md`.

/**
 * How many lines the current viewport top sits ABOVE the bottom of the buffer — the stable anchor to
 * capture before a repaint (from `captureViewportAnchorFromBottom`). `viewportY` is the buffer row at
 * the top of the viewport. Clamped to ≥0.
 */
export function viewportAnchorFromBottom(bufferLength: number, viewportY: number, rows: number): number {
  return Math.max(0, bufferLength - viewportY - rows);
}

/**
 * The buffer row to scroll the viewport top to so the SAME `linesFromBottom` anchor is restored after
 * the buffer was repainted to a (possibly different) length (from `restoreViewportAnchorFromBottom`).
 * Clamped to ≥0 so a shorter buffer can't scroll to a negative line.
 */
export function restoreTargetTop(bufferLength: number, rows: number, linesFromBottom: number): number {
  return Math.max(0, bufferLength - rows - linesFromBottom);
}

/** Preserve the reading position (freeze/anchor instead of snapping to bottom) exactly when the user
 *  has scrolled up — i.e. is NOT pinned to the latest output. */
export function shouldPreserveReadingPosition(userPinnedToLatest: boolean): boolean {
  return !userPinnedToLatest;
}

/**
 * Next follow-live pin state after a user scroll. Re-pin when the viewport is at the bottom; unpin on a
 * deliberate upward scroll; otherwise keep the current state. `atBottom` wins over `deliberateUpwardScroll`
 * (they can't both truly hold). Encodes the `bindMobileViewportScroll` / `syncPinnedToLatestAfterUserScroll`
 * decision in one testable place.
 */
export function nextPinState(
  atBottom: boolean,
  deliberateUpwardScroll: boolean,
  current: boolean,
): boolean {
  if (atBottom) return true;
  if (deliberateUpwardScroll) return false;
  return current;
}

/**
 * C2b — capture depth for the console's PRIMARY (Raw) pane. Raised from the old 200-line snapshot so the
 * home-anchored repaint carries real scrollback that xterm (scrollback: 5000) holds and the user can
 * scroll within. The content-change gate in `writeSnapshot` keeps a deep repaint from running while idle,
 * and the `\x1b[3J`+home repaint (never `\x1b[2J`) keeps the pane from going black.
 */
export const CONSOLE_CAPTURE_LINES = 1500;

/** The primary-pane capture depth (function form for callers/tests). */
export function consoleCaptureLines(): number {
  return CONSOLE_CAPTURE_LINES;
}
