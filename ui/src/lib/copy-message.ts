// Shared, framework-agnostic "copy this message" core.
//
// Extracted from `components/message-bubble/message-bubble.component.ts` the same
// way `markdown-render.ts` was, so the rule "what is copyable, what text goes on
// the clipboard, and what happened when we tried" is ONE pure seam that the web
// vitest can drive directly (no Angular, no Ionic, no DOM).
//
// The shipped bug this seam exists to fix: `navigator.clipboard` is UNDEFINED in a
// non-secure context (plain http on a phone reading the console), so the old
// `try { await navigator.clipboard.writeText(...) } catch {}` silently did nothing
// and the user could not tell a copy from a no-op. `performCopy` therefore reports
// the three distinct outcomes instead of swallowing two of them.

/** Anything with an optional markdown `content` — an `AiMessage`, a transcript block, … */
export interface CopyableMessage {
  content?: string | null;
}

/** The clipboard surface `performCopy` needs — `navigator.clipboard` satisfies it. */
export interface ClipboardLike {
  writeText(text: string): Promise<void>;
}

/** Outcome of a copy attempt: succeeded, no clipboard to write to, or the write failed. */
export type CopyOutcome = 'copied' | 'unavailable' | 'failed';

/** What the copy button shows right now — `null` until it is first clicked. */
export type CopyStateOrNull = CopyOutcome | null;

/**
 * The words the copy button shows for each state. The two failure states are
 * DIFFERENT situations and must not read the same:
 *  - 'unavailable' is the browser context — there is no clipboard API at all
 *    (a non-secure http page), nothing the user did, and retrying will not help;
 *  - 'failed' is a real clipboard refusing THIS write (permission, the page not
 *    focused, a transient error) — the copy can simply be retried.
 * `null` (never clicked) and an unknown state render no label at all.
 */
export function copyStateLabelFor(state: CopyStateOrNull): string {
  switch (state) {
    case 'copied':
      return 'Copied';
    case 'unavailable':
      return 'Copying needs a secure (https) page';
    case 'failed':
      return 'Copy refused — try again';
    default:
      return '';
  }
}

/**
 * The pure decision behind "clear the pending copy feedback". A bubble scrolled out
 * of a long transcript is destroyed with its reset timer still pending, so the
 * teardown must cancel it; this helper makes that decision testable without Angular.
 * It always clears (a pending handle must never survive teardown) and reports whether
 * there was one, returning the handle it cancelled.
 */
export function clearCopyResetTimer(
  handle: ReturnType<typeof setTimeout> | undefined,
  clear: (h: ReturnType<typeof setTimeout>) => void = (h) => clearTimeout(h),
): void {
  if (handle === undefined) return;
  clear(handle);
}


/**
 * Is there anything worth copying? True for ANY role (user, assistant, system, tool)
 * as long as the message has non-whitespace content — a whitespace-only or missing
 * body has nothing to copy, so the button should not be offered at all.
 */
export function canCopyMessage(message: CopyableMessage | null | undefined): boolean {
  const content = message?.content;
  return typeof content === 'string' && content.trim().length > 0;
}

/**
 * The exact text to put on the clipboard: the message's RAW markdown content, never
 * the rendered HTML, and not trimmed of internal formatting (leading/trailing
 * whitespace only carries no information, so it is dropped; everything inside the
 * body — markdown emphasis, fenced code blocks, indentation, inner newlines — is
 * preserved verbatim).
 */
export function copyTextFor(message: CopyableMessage | null | undefined): string {
  const content = message?.content;
  if (typeof content !== 'string') return '';
  return content.replace(/^\s+/, '').replace(/\s+$/, '');
}

/**
 * Write `text` to `clipboard` and report what happened. Never throws:
 *  - 'unavailable' when there is no clipboard object / no `writeText` on it — the
 *    non-secure-context case (plain http), which is a real condition to surface,
 *    not a failure to hide;
 *  - 'failed' when `writeText` rejects (permission denied, page not focused, …);
 *  - 'copied' when it resolves.
 */
export async function performCopy(
  text: string,
  clipboard?: ClipboardLike | null,
): Promise<CopyOutcome> {
  if (!clipboard || typeof clipboard.writeText !== 'function') {
    return 'unavailable';
  }

  try {
    await clipboard.writeText(text);
    return 'copied';
  } catch {
    // The write itself was rejected (permissions / focus / transient browser error).
    // Reported as 'failed' rather than rethrown so the caller can always show a state.
    return 'failed';
  }
}
