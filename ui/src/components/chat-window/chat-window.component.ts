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
import { IonicModule, IonContent } from '@ionic/angular';
import { AiMessage } from '../../models/ai-message.model';
import { MessageBubbleComponent } from '../message-bubble/message-bubble.component';
import { MessageComposerComponent } from '../message-composer/message-composer.component';

@Component({
  selector: 'johnny-chat-window',
  standalone: true,
  imports: [CommonModule, IonicModule, MessageBubbleComponent, MessageComposerComponent],
  templateUrl: './chat-window.component.html',
  styleUrls: ['./chat-window.component.scss'],
  changeDetection: ChangeDetectionStrategy.OnPush,
})
export class ChatWindowComponent implements AfterViewChecked {
  @Input() messages: AiMessage[] = [];
  @Input() isStreaming = false;

  @Output() messageSent = new EventEmitter<string>();

  @ViewChild(IonContent) private content?: IonContent;

  private shouldAutoScroll = true;
  private lastMessageCount = 0;

  ngAfterViewChecked(): void {
    if (this.messages.length !== this.lastMessageCount) {
      this.lastMessageCount = this.messages.length;
      if (this.shouldAutoScroll) {
        this.scrollToBottom();
      }
    }
  }

  onMessageSent(content: string): void {
    this.shouldAutoScroll = true;
    this.messageSent.emit(content);
  }

  onContentScroll(event: CustomEvent): void {
    const detail = event.detail;
    if (detail && detail.scrollTop !== undefined) {
      const threshold = 100;
      const atBottom =
        detail.scrollTop + detail.contentHeight >= detail.scrollHeight - threshold;
      this.shouldAutoScroll = atBottom;
    }
  }

  trackByMessageId(_index: number, message: AiMessage): string {
    return message.id;
  }

  private scrollToBottom(): void {
    setTimeout(() => {
      this.content?.scrollToBottom(200);
    }, 50);
  }
}
