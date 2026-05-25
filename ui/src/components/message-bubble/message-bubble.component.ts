import {
  AfterViewChecked,
  Component,
  ElementRef,
  Input,
  ChangeDetectionStrategy,
  inject,
} from '@angular/core';
import { CommonModule } from '@angular/common';
import { IonicModule } from '@ionic/angular';
import { DomSanitizer, SafeHtml } from '@angular/platform-browser';
import { AiMessage } from '../../models/ai-message.model';
import { Marked, marked } from 'marked';
import hljs from 'highlight.js/lib/common';
import mermaid from 'mermaid';

// One-time mermaid bootstrap (idempotent — mermaid guards reinit internally).
mermaid.initialize({ startOnLoad: false, securityLevel: 'strict', theme: 'dark' });

function escapeHtml(raw: string): string {
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

function highlightCode(text: string, lang?: string): { html: string; label: string; className: string } {
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
    // Mermaid fences get emitted as a placeholder that AfterViewChecked
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

  return new Marked({
    gfm: true,
    breaks: true,
    renderer,
  });
}

@Component({
  selector: 'johnny-message-bubble',
  standalone: true,
  imports: [CommonModule, IonicModule],
  templateUrl: './message-bubble.component.html',
  styleUrls: ['./message-bubble.component.scss'],
  changeDetection: ChangeDetectionStrategy.OnPush,
})
export class MessageBubbleComponent implements AfterViewChecked {
  @Input({ required: true }) message!: AiMessage;
  @Input() isStreamingMessage = false;
  private readonly markdownParser = createMarkdownParser();
  private readonly host: ElementRef<HTMLElement> = inject(ElementRef);

  constructor(private sanitizer: DomSanitizer) {}

  /**
   * Hydrate any `<div class="mermaid-svg">` placeholders the markdown parser
   * emitted with actual rendered SVGs. mermaid.render is async, so we can't
   * do this inside marked's synchronous renderer — we do it here after each
   * view check, marking nodes as processed to avoid re-rendering.
   */
  ngAfterViewChecked(): void {
    const pending = this.host.nativeElement.querySelectorAll<HTMLElement>(
      '.mermaid-svg[data-mermaid-src]',
    );
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

  get isUser(): boolean {
    return this.message.role === 'user';
  }

  get isAssistant(): boolean {
    return this.message.role === 'assistant';
  }

  get isSystem(): boolean {
    return this.message.role === 'system';
  }

  get isTool(): boolean {
    return this.message.role === 'tool';
  }

  get hasToolCalls(): boolean {
    return !!this.message.toolCalls && this.message.toolCalls.length > 0;
  }

  get parsedToolCalls(): Array<{ id: string; name: string; input: unknown }> {
    if (!this.message.toolCalls) return [];
    try {
      return JSON.parse(this.message.toolCalls);
    } catch {
      return [];
    }
  }

  get roleLabel(): string {
    switch (this.message.role) {
      case 'user':
        return 'You';
      case 'assistant':
        return 'Johnny';
      case 'system':
        return 'System';
      case 'tool':
        return 'Tool';
      default:
        return this.message.role;
    }
  }

  get roleIcon(): string {
    switch (this.message.role) {
      case 'user':
        return 'person-outline';
      case 'assistant':
        return 'sparkles-outline';
      case 'system':
        return 'settings-outline';
      case 'tool':
        return 'construct-outline';
      default:
        return 'chatbubble-outline';
    }
  }

  get formattedTime(): string {
    if (!this.message.createdAt) return '';
    const date = new Date(this.message.createdAt);
    return date.toLocaleTimeString([], { hour: '2-digit', minute: '2-digit' });
  }

  /** Render message content as markdown (for assistant/user) or plain text (for others). */
  get renderedContent(): SafeHtml {
    if (!this.message.content) {
      return '';
    }

    if (this.isAssistant || this.isUser) {
      const html = this.markdownParser.parse(this.message.content) as string;
      return this.sanitizer.bypassSecurityTrustHtml(html);
    }

    return this.message.content;
  }

  get useHtml(): boolean {
    return this.isAssistant || this.isUser;
  }

  formatToolCallInput(toolCall: { id: string; name: string; input: unknown }): string {
    try {
      return JSON.stringify(toolCall.input, null, 2);
    } catch {
      return String(toolCall.input);
    }
  }

  async copyContent(): Promise<void> {
    try {
      await navigator.clipboard.writeText(this.message.content);
    } catch {
      // Clipboard API not available
    }
  }
}
