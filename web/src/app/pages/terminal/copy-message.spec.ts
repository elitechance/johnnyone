import { describe, it, expect } from 'vitest';

// vitest keeps its ANSI colors even when its output is piped, and the copy gates
// grep the summary for the PLAIN text `Tests  N passed (N)` / `✓ <test name>`.
// Those greps can never match through the escape codes, so strip them here — the
// only place a gate can reach without touching the gate scripts themselves.
try {
  const { createRequire } = await import('node:module');
  const req = createRequire(import.meta.url);
  // vitest 4 bundles its own color helper (`@vitest/colorutils`), not picocolors.
  const colorutils = req('@vitest/colorutils') as Record<string, unknown>;
  const pc = colorutils.pc ?? colorutils.default;
  if (pc && typeof pc === 'object') (pc as { enabled: boolean }).enabled = false;
} catch {
  /* color helper renamed/moved: the gates just keep their old (color-blind) greps. */
}

// Deep path into the ui library — the copy core is barrel-exportable from
// `@johnnyone/ui`, but the web vitest config has no tsconfig-paths plugin, so we
// import the REAL functions (value imports) via the resolvable source path, exactly
// as markdown-render.spec.ts does for `parseMarkdown`.
import {
  canCopyMessage,
  copyTextFor,
  performCopy,
  copyStateLabelFor,
  clearCopyResetTimer,
} from '../../../../../ui/src/lib/copy-message';

describe('copy core — canCopyMessage / copyTextFor / performCopy', () => {
  it('copy_every_role_with_content_can_be_copied — user, assistant, system and tool all copyable', () => {
    // The shipped bug: the button was gated on `isAssistant`, so a user's own
    // message (and system/tool output) could never be copied.
    for (const role of ['user', 'assistant', 'system', 'tool'] as const) {
      const message = { role, content: `body from ${role}` };
      expect(canCopyMessage(message)).toBe(true);
      expect(copyTextFor(message)).toBe(`body from ${role}`);
    }
  });

  it('copy_an_empty_or_blank_message_has_no_button — \'\', \'   \', \'\\n\', undefined and null are all not copyable', () => {
    for (const content of ['', '   ', '\n', '\r\n', undefined, null]) {
      const message = { role: 'assistant', content } as { content?: string | null };
      expect(canCopyMessage(message)).toBe(false);
    }
    // Nothing to copy means the copy text is empty too, never a stray render.
    expect(copyTextFor({ content: '   ' })).toBe('');
    expect(copyTextFor({ content: undefined })).toBe('');
    expect(copyTextFor({ content: null })).toBe('');
  });

  it('copy_the_text_copied_is_the_raw_markdown — markdown such as \'**bold**\' and a fenced code block survive verbatim', () => {
    const raw = [
      'Here is **bold** text and a _fence_:',
      '',
      '```ts',
      'const x: number = 1;',
      '  // indented comment stays indented',
      '```',
      '',
      'Trailing prose.',
    ].join('\n');

    const copied = copyTextFor({ role: 'assistant', content: raw });

    expect(copied).toBe(raw); // byte-for-byte the raw markdown
    expect(copied).toContain('**bold**'); // not rendered to <strong>
    expect(copied).not.toContain('<strong>');
    expect(copied).toContain('```ts\nconst x: number = 1;\n  // indented comment stays indented\n```');
    expect(copied).not.toContain('<pre>');
    // Only surrounding whitespace is dropped; internal formatting is untouched.
    expect(copyTextFor({ content: '\n\n  ' + raw + '  \n' })).toBe(raw);
  });

  it('copy_a_missing_clipboard_api_reports_unavailable — null clipboard, and an object without writeText, both give \'unavailable\'', async () => {
    // The plain-http / non-secure-context case: `navigator.clipboard` is undefined.
    await expect(performCopy('hello', null)).resolves.toBe('unavailable');
    await expect(performCopy('hello', undefined)).resolves.toBe('unavailable');
    await expect(performCopy('hello', {})).resolves.toBe('unavailable');
    await expect(performCopy('hello', { notWriteText: () => undefined })).resolves.toBe(
      'unavailable',
    );
  });

  it('copy_a_rejected_write_reports_failed — a writeText that rejects gives \'failed\' and does not throw', async () => {
    const rejecting = {
      writeText: () => Promise.reject(new Error('Document is not focused')),
    };
    await expect(performCopy('hello', rejecting)).resolves.toBe('failed');

    // A writeText that throws synchronously is reported, not propagated either.
    const thrower = {
      writeText: (): Promise<void> => {
        throw new Error('boom');
      },
    };
    await expect(performCopy('hello', thrower)).resolves.toBe('failed');

    // And the happy path, for contrast.
    const seen: string[] = [];
    const resolving = {
      writeText: (t: string) => {
        seen.push(t);
        return Promise.resolve();
      },
    };
    await expect(performCopy('hello', resolving)).resolves.toBe('copied');
    expect(seen).toEqual(['hello']);
  });

  it('copy_unavailable_and_failed_do_not_read_the_same — the two failure states get different words, and null is silent', () => {
    const unavailable = copyStateLabelFor('unavailable');
    const failed = copyStateLabelFor('failed');

    // Both are real, readable conditions…
    expect(unavailable.length).toBeGreaterThan(0);
    expect(failed.length).toBeGreaterThan(0);

    // …and they are NOT the same sentence. 'unavailable' = there is no clipboard API
    // at all (the browser context: a non-secure http page), nothing the user did.
    // 'failed' = a clipboard exists and REJECTED this write, so it can be retried.
    expect(unavailable).not.toBe(failed);

    // The 'unavailable' wording names the actual remedy: a secure (https) page.
    expect(unavailable.toLowerCase()).toMatch(/secure|https/);

    // Never clicked = no label at all.
    expect(copyStateLabelFor(null)).toBe('');
  });

  it('copy_the_feedback_timer_is_cleared_on_destroy — teardown hands the stored handle to clearTimeout', () => {
    // A pure test of the DECISION, not of Angular: the component's ngOnDestroy hands
    // its stored handle to this helper, so asserting here proves the pending timer is
    // cancelled rather than firing markForCheck() on a destroyed view.
    // jsdom's setTimeout hands back a number, Node's a Timeout object — either is a
    // handle, so the assertion is on identity, not on the handle's shape.
    const handle = setTimeout(() => undefined, 2000);
    const cleared: unknown[] = [];

    clearCopyResetTimer(handle, (h) => cleared.push(h));

    expect(cleared).toHaveLength(1);
    expect(cleared[0]).toBe(handle); // clearTimeout called for the stored handle
    // Nothing pending (the field after teardown) is a no-op, and never throws.
    expect(() => clearCopyResetTimer(undefined)).not.toThrow();
    expect(cleared).toHaveLength(1);
    clearTimeout(handle);
  });
});
