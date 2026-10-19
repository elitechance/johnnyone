// The DECISION behind the transcript's per-row copy button.
//
// The console's transcript is `terminal.page.html`'s `st-row` loop over `shellRows()`,
// whose rows are `{ type: 'you' | 'agent' | …, text }` — NOT an `AiMessage`. So the
// page maps a row through `canCopyMessage({ content: row.text })` /
// `copyTextFor({ content: row.text })` and the seam decides.
//
// Deliberately no Angular and no page-component import: `web/vitest.config.ts` has no
// tsconfig-paths plugin, so '@johnnyone/ui' does not resolve here. Deep relative path,
// exactly as `copy-message.spec.ts` does it.

import { describe, expect, it } from 'vitest';
import {
  canCopyMessage,
  copyTextFor,
} from '../../../../../ui/src/lib/copy-message';

/** The row shape the transcript loop hands the seam. */
type TranscriptRow = { type: 'you' | 'agent'; text: string };

/** What the page asks the seam for a row: is a button warranted? */
const copyable = (row: TranscriptRow): boolean => canCopyMessage({ content: row.text });

/** What the page asks the seam for a row: what goes on the clipboard? */
const copiedText = (row: TranscriptRow): string => copyTextFor({ content: row.text });

describe('transcript row copy decisions', () => {
  it('transcript_copy_a_you_row_and_an_agent_row_are_both_copyable', () => {
    const you: TranscriptRow = { type: 'you', text: 'ship the copy button' };
    const agent: TranscriptRow = {
      type: 'agent',
      text: '## Phase 3 done\n\nAll 11 tasks pass their gates.',
    };

    // The role never gates the button — a prompt you sent is as worth re-using as the
    // agent's report, and on a phone you cannot select the pane text any other way.
    expect(copyable(you)).toBe(true);
    expect(copyable(agent)).toBe(true);
  });

  it('transcript_copy_a_blank_row_gets_no_button', () => {
    // A heartbeat/progress row can arrive with nothing in it. Offering "Copy" for it
    // promises a clipboard write that would put an empty string there.
    expect(copyable({ type: 'agent', text: '' })).toBe(false);
    expect(copyable({ type: 'agent', text: '   \n\t  ' })).toBe(false);
    expect(copyable({ type: 'you', text: '' })).toBe(false);
    // …while a row that merely has whitespace around real content is still copyable.
    expect(copyable({ type: 'agent', text: '  report  ' })).toBe(true);
  });

  it('transcript_copy_the_text_is_the_rows_raw_text', () => {
    const raw = '  ## Report\n\n```mermaid\ngraph TD; A-->B;\n```\n\n  ';
    const row: TranscriptRow = { type: 'agent', text: raw };

    // The RAW text, not the rendered markdown: fenced code, emphasis and inner line
    // breaks survive verbatim; only the meaningless outer padding is dropped.
    expect(copiedText(row)).toBe('## Report\n\n```mermaid\ngraph TD; A-->B;\n```');
    expect(copiedText({ type: 'you', text: '  do it  ' })).toBe('do it');
    expect(copiedText({ type: 'agent', text: '   \n ' })).toBe('');
  });
});