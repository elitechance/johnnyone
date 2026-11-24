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

  it('strips raw prose <script> (sanitized since U-1-1; was the pinned unsanitized posture)', () => {
    // The follow-up this test used to point at is done: parseMarkdown now sanitizes its output
    // (DOMPurify + URL allowlist). Full coverage: web/src/app/components/markdown-sanitize.spec.ts.
    const html = parseMarkdown('<script>alert(1)</script> hi');
    expect(html).not.toContain('<script');
    expect(html).toContain('hi');
  });

  it('escapes html *inside* a code fence (highlightCode path)', () => {
    // The one place the pipeline does escape: fenced code content runs through
    // escapeHtml/highlight, so a `<script>` in a code block is rendered inert.
    const html = parseMarkdown('```text\n<script>alert(1)</script>\n```');
    expect(html).toContain('&lt;script&gt;');
    expect(html).not.toContain('<script>alert(1)</script>');
  });
});
