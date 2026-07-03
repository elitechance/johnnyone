import { describe, it, expect } from 'vitest';

// Deep path into the ui library — the render core is barrel-exported from
// `@johnnyone/ui`, but the web vitest config has no tsconfig-paths plugin, so we
// import the REAL `parseMarkdown` (value import) via the resolvable source path.
import { parseMarkdown } from '../../../../../ui/src/lib/markdown-render';

describe('render core — parseMarkdown', () => {
  it('renders prose as reflowing markdown', () => {
    const html = parseMarkdown('hello **world**');
    expect(html).toContain('<p>');
    expect(html).toContain('<strong>world</strong>');
  });

  it('highlights a fenced code block by language', () => {
    const html = parseMarkdown('```ts\nconst x: number = 1\n```');
    expect(html).toContain('hljs');
    expect(html).toMatch(/language-ts|code-block/); // highlighted, not raw text
  });

  it('emits a placeholder (not inline svg) for a mermaid fence', () => {
    const html = parseMarkdown('```mermaid\ngraph TD; A-->B\n```');
    expect(html).toContain('mermaid-svg');
    expect(html).toContain('data-mermaid-src'); // async hydration contract
    expect(html).not.toContain('<svg'); // rendered later by hydrateMermaid, not here
  });

  it('passes raw prose HTML through unsanitized (current shipped posture)', () => {
    // ACTUAL behavior, pinned as-is: marked v14 does NOT escape raw block/inline
    // HTML, and message-bubble wraps parseMarkdown's output in
    // DomSanitizer.bypassSecurityTrustHtml(...). So a prose `<script>` survives
    // verbatim. This is a strict extract/dedupe phase (D2/D8/D9) — behavior is
    // preserved, not changed. Hardening this into real sanitization is a tracked
    // follow-up (see decisions.md "D-followup" / BLOCKER-task03-sanitization.md).
    const html = parseMarkdown('<script>alert(1)</script> hi');
    expect(html).toContain('<script>alert(1)</script>');
    expect(html).not.toContain('&lt;script&gt;');
  });

  it('escapes html *inside* a code fence (highlightCode path)', () => {
    // The one place the pipeline does escape: fenced code content runs through
    // escapeHtml/highlight, so a `<script>` in a code block is rendered inert.
    const html = parseMarkdown('```text\n<script>alert(1)</script>\n```');
    expect(html).toContain('&lt;script&gt;');
    expect(html).not.toContain('<script>alert(1)</script>');
  });
});
