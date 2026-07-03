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
import { parseMarkdown, hydrateMermaid } from '../../lib/markdown-render';

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
  private readonly host: ElementRef<HTMLElement> = inject(ElementRef);

  constructor(private sanitizer: DomSanitizer) {}

  /**
   * Hydrate any `<div class="mermaid-svg">` placeholders the markdown parser
   * emitted with actual rendered SVGs. mermaid.render is async, so we can't
   * do this inside marked's synchronous renderer — we do it here after each
   * view check, marking nodes as processed to avoid re-rendering.
   */
  ngAfterViewChecked(): void {
    hydrateMermaid(this.host.nativeElement);
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
      const html = parseMarkdown(this.message.content);
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
