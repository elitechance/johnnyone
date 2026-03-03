import { Component, Input, ChangeDetectionStrategy } from '@angular/core';
import { CommonModule } from '@angular/common';
import { IonicModule } from '@ionic/angular';
import { DomSanitizer, SafeHtml } from '@angular/platform-browser';
import { AiMessage, ToolCallRef } from '../../models/ai-message.model';
import { marked } from 'marked';

@Component({
  selector: 'johnny-message-bubble',
  standalone: true,
  imports: [CommonModule, IonicModule],
  templateUrl: './message-bubble.component.html',
  styleUrls: ['./message-bubble.component.scss'],
  changeDetection: ChangeDetectionStrategy.OnPush,
})
export class MessageBubbleComponent {
  @Input({ required: true }) message!: AiMessage;
  @Input() isStreamingMessage = false;

  constructor(private sanitizer: DomSanitizer) {
    // Configure marked for safe rendering
    marked.setOptions({
      gfm: true,
      breaks: true,
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
    return this.message.toolCalls?.length > 0;
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

  /** Render message content as markdown (for assistant) or plain text (for user). */
  get renderedContent(): SafeHtml {
    if (!this.message.content) {
      return '';
    }

    // Only render markdown for assistant messages
    if (this.isAssistant) {
      const html = marked.parse(this.message.content) as string;
      return this.sanitizer.bypassSecurityTrustHtml(html);
    }

    return this.message.content;
  }

  get useHtml(): boolean {
    return this.isAssistant;
  }

  formatToolCallInput(toolCall: ToolCallRef): string {
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
