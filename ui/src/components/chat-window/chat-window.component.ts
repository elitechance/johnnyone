import {
  Component,
  Input,
  Output,
  EventEmitter,
  ViewChild,
  ElementRef,
  AfterViewChecked,
  ChangeDetectionStrategy,
} from '@angular/core';
import { CommonModule } from '@angular/common';
import { AiMessage } from '../../models/ai-message.model';
import { MessageBubbleComponent } from '../message-bubble/message-bubble.component';
import { MessageComposerComponent } from '../message-composer/message-composer.component';

@Component({
  selector: 'johnny-chat-window',
  standalone: true,
  imports: [CommonModule, MessageBubbleComponent, MessageComposerComponent],
  templateUrl: './chat-window.component.html',
  styleUrls: ['./chat-window.component.scss'],
  changeDetection: ChangeDetectionStrategy.OnPush,
})
export class ChatWindowComponent implements AfterViewChecked {
  @Input() messages: AiMessage[] = [];
  @Input() isStreaming = false;
  @Input() streamingMessageId: string | null = null;
  @Input() disabled = false;
  @Input() placeholder = 'Type a message...';
  @Input() emptyTitle = 'No messages yet';
  @Input() emptyDescription = 'Start a conversation.';
  @Input() emptyHint = '';
  @Input() showEmpty = true;
  /**
   * Additive (overhaul P4, D7): when false, the built-in `johnny-message-composer` is hidden so a
   * host page can supply its own composer via the `[chat-actions]` slot (e.g. the briefing view).
   * Defaults to true — existing callers are unaffected.
   */
  @Input() showComposer = true;

  @Output() messageSent = new EventEmitter<string>();

  @ViewChild('messagesContainer') private messagesContainerRef?: ElementRef<HTMLDivElement>;

  private shouldAutoScroll = true;
  private lastScrollKey = '';

  ngAfterViewChecked(): void {
    const last = this.messages[this.messages.length - 1];
    const scrollKey = `${this.messages.length}:${last?.id ?? ''}:${last?.content?.length ?? 0}:${this.isStreaming}`;
    if (scrollKey === this.lastScrollKey) return;
    this.lastScrollKey = scrollKey;

    if (this.shouldAutoScroll) {
      this.scrollToBottom();
    }
  }

  onMessageSent(content: string): void {
    this.shouldAutoScroll = true;
    this.messageSent.emit(content);
  }

  onMessagesScroll(): void {
    const el = this.messagesContainerRef?.nativeElement;
    if (!el) return;
    const threshold = 96;
    this.shouldAutoScroll = el.scrollTop + el.clientHeight >= el.scrollHeight - threshold;
  }

  trackByMessageId(_index: number, message: AiMessage): string {
    return message.id;
  }

  isStreamingMessage(message: AiMessage, index: number): boolean {
    if (!this.isStreaming || message.role !== 'assistant') return false;
    if (this.streamingMessageId) return message.id === this.streamingMessageId;
    const isLast = index === this.messages.length - 1;
    return isLast && message.role === 'assistant';
  }

  get showTypingIndicator(): boolean {
    if (!this.isStreaming) return false;
    const last = this.messages[this.messages.length - 1];
    if (!last || last.role !== 'assistant') return true;
    return !last.content;
  }

  private scrollToBottom(): void {
    requestAnimationFrame(() => {
      const el = this.messagesContainerRef?.nativeElement;
      if (!el) return;
      el.scrollTop = el.scrollHeight;
    });
  }
}
