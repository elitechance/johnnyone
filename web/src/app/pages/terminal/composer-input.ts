/**
 * Composer key handling for the transcript.
 *
 * Extracted as pure functions so the Enter/newline split is testable without a TestBed — the
 * decision is the part that is easy to get subtly wrong, not the DOM plumbing around it.
 */

/** Tallest the composer grows before it scrolls, so a long message cannot eat the transcript. */
export const COMPOSER_MAX_PX = 140;

/**
 * Whether Enter should SEND, given the pointer type.
 *
 * Keyed off pointer type rather than screen width, because the thing that actually matters is
 * whether a Shift key exists: a soft keyboard has no Shift+Enter, so on touch Enter must be the
 * newline and Send does the sending. A tablet with a keyboard attached reports a fine pointer and
 * behaves like a desktop — width would get that case backwards.
 */
export function enterSendsForPointer(pointerIsCoarse: boolean): boolean {
  return !pointerIsCoarse;
}

/**
 * What an Enter keypress should do.
 *
 * Shift+Enter is a newline on every surface, including touch, so an attached keyboard behaves the
 * way its user expects regardless of what the pointer heuristic decided.
 */
export function enterAction(
  options: { shiftKey: boolean; enterSends: boolean },
): 'send' | 'newline' {
  if (options.shiftKey) return 'newline';
  return options.enterSends ? 'send' : 'newline';
}

/** Height the composer should take for its content, capped. */
export function composerHeightPx(scrollHeight: number, maxPx: number = COMPOSER_MAX_PX): number {
  return Math.min(scrollHeight, maxPx);
}
