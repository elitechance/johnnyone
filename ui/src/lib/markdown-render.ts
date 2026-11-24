// Shared, framework-agnostic markdown/code/mermaid render core.
//
// Extracted verbatim from `components/message-bubble/message-bubble.component.ts`
// so the chat bubble, the Phase 02 transcript/markdown-view, and the Phase 5 file
// viewer / Plan tab all render through ONE pipeline (decision D2). No Angular
// import — pure `marked` + `highlight.js` + `mermaid` plus DOM.

import { Marked, marked } from 'marked';
import hljs from 'highlight.js/lib/common';
import mermaid from 'mermaid';
import DOMPurify, { type DOMPurify as DOMPurifyInstance } from 'dompurify';

// One-time mermaid bootstrap (idempotent — mermaid guards reinit internally).
mermaid.initialize({ startOnLoad: false, securityLevel: 'strict', theme: 'dark' });

export function escapeHtml(raw: string): string {
  return raw
    .replace(/&/g, '&amp;')
    .replace(/</g, '&lt;')
    .replace(/>/g, '&gt;')
    .replace(/"/g, '&quot;')
    .replace(/'/g, '&#39;');
}

function normalizeLanguage(lang?: string): string | undefined {
  if (!lang) return undefined;

  const base = lang
    .trim()
    .toLowerCase()
    .replace(/^language-/, '')
    .replace(/^\{/, '')
    .replace(/\}$/, '')
    .split(/\s+/)[0];
  if (!base) return undefined;

  const safe = base.match(/^[a-z0-9#+.-]+$/i)?.[0];
  return safe ? safe.toLowerCase() : undefined;
}

export function highlightCode(
  text: string,
  lang?: string,
): { html: string; label: string; className: string } {
  const language = normalizeLanguage(lang);

  if (language && hljs.getLanguage(language)) {
    const highlighted = hljs.highlight(text, {
      language,
      ignoreIllegals: true,
    });

    return {
      html: highlighted.value,
      label: language,
      className: `language-${language}`,
    };
  }

  if (language) {
    return {
      html: escapeHtml(text),
      label: language,
      className: 'language-plaintext',
    };
  }

  const auto = hljs.highlightAuto(text);
  const autoLanguage = normalizeLanguage(auto.language);

  if (autoLanguage) {
    return {
      html: auto.value,
      label: autoLanguage,
      className: `language-${autoLanguage}`,
    };
  }

  return {
    html: escapeHtml(text),
    label: 'text',
    className: 'language-plaintext',
  };
}

function createMarkdownParser(): Marked {
  const renderer = new marked.Renderer();

  renderer.code = ({ text, lang }) => {
    // Mermaid fences get emitted as a placeholder that `hydrateMermaid`
    // hydrates with a real SVG via `mermaid.render`. Inline rendering inside
    // marked's renderer would need to be async, which marked doesn't support.
    if ((lang || '').trim().toLowerCase() === 'mermaid') {
      const source = encodeURIComponent(text);
      return `<div class="mermaid-svg" data-mermaid-src="${source}" role="img" aria-label="Mermaid diagram"><div class="mermaid-pending">Rendering diagram…</div></div>`;
    }

    const highlighted = highlightCode(text, lang);

    return (
      `<div class="code-block">` +
      `<div class="code-header"><span class="code-lang">${escapeHtml(highlighted.label)}</span></div>` +
      `<pre><code class="hljs ${highlighted.className}">${highlighted.html}</code></pre>` +
      `</div>`
    );
  };

  renderer.table = (token) => {
    const renderCell = (cell: (typeof token.header)[number], tag: 'th' | 'td'): string => {
      const align = cell.align ? ` style="text-align: ${cell.align}"` : '';
      const content = marked.parseInline(cell.text, { gfm: true, breaks: true }) as string;
      return `<${tag}${align}>${content}</${tag}>`;
    };
    const header = token.header.map((cell) => renderCell(cell, 'th')).join('');
    const rows = token.rows
      .map((row) => `<tr>${row.map((cell) => renderCell(cell, 'td')).join('')}</tr>`)
      .join('');

    return (
      `<div class="markdown-table-wrap">` +
      `<table><thead><tr>${header}</tr></thead><tbody>${rows}</tbody></table>` +
      `</div>`
    );
  };

  return new Marked({
    gfm: true,
    breaks: true,
    renderer,
  });
}

// Module singleton — one parser instance for the whole app.
const markdownParser = createMarkdownParser();

/**
 * URLs a rendered link or image may carry: http(s), mailto, and relative / fragment URLs (no
 * scheme). Everything else (`javascript:`, `data:`, `vbscript:`, `file:`, …) is removed (U-1-1).
 */
const SAFE_URL = /^(?:(?:https?|mailto):|[^a-z]|[a-z0-9+.-]*(?:[^a-z0-9+.:-]|$))/i;
const URL_ATTRS = ['href', 'src', 'xlink:href', 'action', 'formaction', 'srcset', 'poster', 'background'];

/** Is `url` allowed in a rendered href/src? Leading whitespace/controls are ignored, as browsers do. */
export function isSafeUrl(url: string | null | undefined): boolean {
  // eslint-disable-next-line no-control-regex
  const u = (url ?? '').replace(/[\u0000-\u0020\u007f-\u009f]/g, '');
  return SAFE_URL.test(u);
}

let purifier: DOMPurifyInstance | null = null;

/**
 * A PRIVATE DOMPurify instance (mermaid imports the same module and sanitizes its SVG with the
 * default one; our hooks must not change that). `null` where there is no DOM.
 */
function getPurifier(): DOMPurifyInstance | null {
  if (purifier) return purifier;
  if (typeof window === 'undefined') return null;
  const p = DOMPurify(window);
  p.addHook('afterSanitizeAttributes', (node) => {
    for (const name of URL_ATTRS) {
      if (node.hasAttribute?.(name) && !isSafeUrl(node.getAttribute(name))) node.removeAttribute(name);
    }
  });
  purifier = p;
  return p;
}

/**
 * Sanitize rendered markdown (U-1-1). The markdown is agent-written (briefs, plan docs, chat,
 * transcripts) and the result is bound with `bypassSecurityTrustHtml`, so without this an
 * `<img onerror>` or a `javascript:` link ran in the app's origin, next to the access token.
 * Keeps what the renderer emits: tables (with `style="text-align"`), highlighted code, and the
 * mermaid placeholder (`data-mermaid-src`, `role`, `aria-label`) that `hydrateMermaid` fills.
 */
export function sanitizeHtml(html: string): string {
  const p = getPurifier();
  if (!p) return escapeHtml(html);
  return p.sanitize(html, {
    USE_PROFILES: { html: true },
    ALLOW_DATA_ATTR: true,
    ALLOWED_URI_REGEXP: SAFE_URL,
    FORBID_TAGS: ['style', 'form', 'input', 'button', 'textarea', 'select', 'iframe', 'object', 'embed'],
  }) as string;
}

/** Parse markdown → SANITIZED HTML string (code highlighted, mermaid fences as placeholders). */
export function parseMarkdown(md: string): string {
  return sanitizeHtml(markdownParser.parse(md) as string);
}

/**
 * Hydrate any `<div class="mermaid-svg" data-mermaid-src>` placeholders under
 * `host` with actual rendered SVGs. `mermaid.render` is async, so this can't run
 * inside marked's synchronous renderer — callers run it after the placeholder
 * markup is in the DOM, marking nodes as processed to avoid re-rendering.
 * Framework-agnostic (pure DOM).
 */
export function hydrateMermaid(host: HTMLElement): void {
  const pending = host.querySelectorAll<HTMLElement>('.mermaid-svg[data-mermaid-src]');
  if (pending.length === 0) return;
  pending.forEach((node) => {
    const raw = node.getAttribute('data-mermaid-src');
    if (!raw) return;
    // Mark as processing so a follow-up tick doesn't re-render the same node.
    node.removeAttribute('data-mermaid-src');
    const source = decodeURIComponent(raw);
    const id = `mb-mermaid-${Math.random().toString(36).slice(2, 10)}`;
    mermaid
      .render(id, source)
      .then((rendered) => {
        node.innerHTML = rendered.svg;
        rendered.bindFunctions?.(node);
      })
      .catch((err) => {
        node.innerHTML = `<div class="mermaid-error">Failed to render mermaid diagram: ${escapeHtml(String(err))}</div>`;
      });
  });
}
