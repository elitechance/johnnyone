/**
 * U-1-1 (security): `parseMarkdown` output is bound with `bypassSecurityTrustHtml`
 * (`markdown-view`, `message-bubble`), and the markdown is agent-written. Before the fix an
 * `<img onerror>` and a `javascript:` link passed straight through. Now every render is sanitized
 * (a private DOMPurify instance + an http/https/mailto/relative URL allowlist), while tables,
 * highlighted code and the mermaid placeholder still render (O3).
 */
import { describe, expect, it } from 'vitest';
import { isSafeUrl, parseMarkdown, safeTextAlign, sanitizeHtml } from '../../../../ui/src/lib/markdown-render';

/** Parse the rendered HTML so assertions look at the DOM, not at substrings. */
function dom(html: string): HTMLElement {
  const host = document.createElement('div');
  host.innerHTML = html;
  return host;
}

function noScriptVectors(host: HTMLElement): void {
  expect(host.querySelector('script')).toBeNull();
  for (const el of Array.from(host.querySelectorAll('*'))) {
    for (const a of Array.from(el.attributes)) {
      expect(a.name.startsWith('on'), `${el.tagName} ${a.name}`).toBe(false);
      if (['href', 'src', 'xlink:href'].includes(a.name)) {
        expect(a.value, `${el.tagName} ${a.name}`).not.toMatch(/^\s*(javascript|data|vbscript):/i);
      }
    }
  }
}

describe('markdown output is sanitized (U-1-1)', () => {
  it('strips an <img onerror>', () => {
    const host = dom(parseMarkdown('hi <img src=x onerror="alert(1)">'));
    noScriptVectors(host);
    expect(host.querySelector('img')?.getAttribute('onerror')).toBeFalsy();
  });

  it('strips a javascript: link (markdown and raw HTML forms, obfuscated too)', () => {
    for (const md of ['[x](javascript:alert(1))', '<a href="javascript:alert(1)">x</a>', '<a href=" JaVaScRiPt:alert(1)">x</a>', '<a href="vbscript:msgbox(1)">x</a>']) {
      const host = dom(parseMarkdown(md));
      noScriptVectors(host);
      expect(host.querySelector('a')?.getAttribute('href') ?? null).toBeNull();
    }
  });

  it('strips a data: link and a data: image', () => {
    const host = dom(parseMarkdown('[x](data:text/html,<script>alert(1)</script>) ![i](data:image/svg+xml,<svg onload=alert(1)>)'));
    noScriptVectors(host);
    // DOMPurify alone lets `data:` through on <img>; the allowlist does not.
    const img = dom(parseMarkdown('<img src="data:image/svg+xml;base64,PHN2ZyBvbmxvYWQ9YWxlcnQoMSk+" alt="i">')).querySelector('img');
    expect(img?.getAttribute('src') ?? null).toBeNull();
    const md = dom(parseMarkdown('![i](data:image/png;base64,iVBORw0KGgo=)')).querySelector('img');
    expect(md?.getAttribute('src') ?? null).toBeNull();
  });

  it('only http(s), mailto and relative URLs survive (no tel:, ftp:, file:)', () => {
    const host = dom(parseMarkdown('<a href="ftp://x/y">f</a> <a href="tel:123">t</a> <a href="file:///etc/passwd">p</a>'));
    expect(Array.from(host.querySelectorAll('a')).map((a) => a.getAttribute('href'))).toEqual([null, null, null]);
  });

  it('strips a <script> tag', () => {
    const host = dom(parseMarkdown('before\n\n<script>alert(1)</script>\n\nafter'));
    noScriptVectors(host);
    expect(host.textContent).toContain('before');
    expect(host.textContent).toContain('after');
  });

  it('strips an inline <svg onload>', () => {
    const host = dom(parseMarkdown('<svg onload="alert(1)"><circle r="2"></circle></svg>'));
    noScriptVectors(host);
  });

  it('keeps ordinary markdown, safe links and images', () => {
    const host = dom(parseMarkdown('# Title\n\n**bold** [site](https://example.com) [mail](mailto:a@b.c) [rel](./docs/a.md) [frag](#top) ![pic](https://example.com/p.png)'));
    expect(host.querySelector('h1')?.textContent).toBe('Title');
    expect(host.querySelector('strong')?.textContent).toBe('bold');
    const hrefs = Array.from(host.querySelectorAll('a')).map((a) => a.getAttribute('href'));
    expect(hrefs).toEqual(['https://example.com', 'mailto:a@b.c', './docs/a.md', '#top']);
    expect(host.querySelector('img')?.getAttribute('src')).toBe('https://example.com/p.png');
  });

  it('keeps tables, including column alignment', () => {
    const host = dom(parseMarkdown('| a | b |\n|:--|--:|\n| 1 | 2 |'));
    expect(host.querySelector('.markdown-table-wrap table')).not.toBeNull();
    expect(host.querySelectorAll('td').length).toBe(2);
    expect(host.querySelector('th')?.getAttribute('style')).toContain('text-align: left');
  });

  it('keeps highlighted code blocks', () => {
    const host = dom(parseMarkdown('```ts\nconst x = 1;\n```'));
    expect(host.querySelector('.code-block pre code.hljs')).not.toBeNull();
    expect(host.textContent).toContain('const x = 1;');
  });

  it('keeps the mermaid placeholder that hydrateMermaid fills (O3)', () => {
    const host = dom(parseMarkdown('```mermaid\ngraph TD; A-->B\n```'));
    const node = host.querySelector('.mermaid-svg') as HTMLElement | null;
    expect(node).not.toBeNull();
    expect(decodeURIComponent(node!.getAttribute('data-mermaid-src') ?? '')).toBe('graph TD; A-->B');
    expect(node!.getAttribute('role')).toBe('img');
  });

  it('the URL allowlist', () => {
    for (const ok of ['https://a.b', 'http://a', 'mailto:x@y', '/abs', './rel', 'rel/x', '#frag', '?q=1']) expect(isSafeUrl(ok), ok).toBe(true);
    for (const bad of ['javascript:alert(1)', ' javascript:x', 'java\tscript:x', 'data:text/html,x', 'vbscript:x', 'file:///etc/passwd']) expect(isSafeUrl(bad), bad).toBe(false);
  });

  it('sanitizeHtml is what parseMarkdown returns through', () => {
    expect(sanitizeHtml('<p onclick="x()">t</p>')).toBe('<p>t</p>');
  });

  it('U-2-19: style keeps only table text-align; an overlay loses its style', () => {
    const div = dom(parseMarkdown('<div style="position:fixed;inset:0;z-index:9999">pwn</div>')).querySelector('div');
    expect(div?.getAttribute('style') ?? null).toBeNull();
    const mixed = dom(parseMarkdown('<p style="text-align: center; position: fixed">x</p>')).querySelector('p');
    expect(mixed?.getAttribute('style') ?? null).toBeNull();
    const host = dom(parseMarkdown('| a | b | c |\n|:--|:-:|--:|\n| 1 | 2 | 3 |'));
    expect(Array.from(host.querySelectorAll('th')).map((th) => th.getAttribute('style'))).toEqual([
      'text-align: left',
      'text-align: center',
      'text-align: right',
    ]);
    expect(safeTextAlign('text-align:LEFT;')).toBe('left');
    expect(safeTextAlign('text-align: justify')).toBeNull();
    expect(safeTextAlign('color: red')).toBeNull();
  });

  it('U-2-20: GFM task lists keep a visible box (as text, no <input>)', () => {
    const host = dom(parseMarkdown('- [x] done\n- [ ] todo'));
    const items = Array.from(host.querySelectorAll('li')).map((li) => li.textContent?.trim());
    expect(items).toEqual(['☑ done', '☐ todo']);
    expect(host.querySelector('input')).toBeNull();
  });

  it('form controls are stripped (form, input, button)', () => {
    const host = dom(parseMarkdown('<form action="https://evil.example"><input name="pw" type="password"><button>Sign in</button></form>'));
    expect(host.querySelector('form')).toBeNull();
    expect(host.querySelector('input')).toBeNull();
    expect(host.querySelector('button')).toBeNull();
  });
});
