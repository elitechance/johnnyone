import {
  Component,
  Input,
  Output,
  EventEmitter,
  ViewChild,
  ChangeDetectionStrategy,
} from '@angular/core';
import { CommonModule } from '@angular/common';
import { FormsModule } from '@angular/forms';
import {
  IonTextarea,
  IonItem,
  IonButton,
  IonIcon,
} from '@ionic/angular/standalone';

@Component({
  selector: 'johnny-message-composer',
  standalone: true,
  imports: [CommonModule, FormsModule, IonTextarea, IonItem, IonButton, IonIcon],
  templateUrl: './message-composer.component.html',
  styleUrls: ['./message-composer.component.scss'],
  changeDetection: ChangeDetectionStrategy.OnPush,
})
export class MessageComposerComponent {
  @Input() disabled = false;
  @Input() placeholder = 'Type a message...';

  @Output() messageSent = new EventEmitter<string>();

  @ViewChild('messageInput') messageInput?: IonTextarea;

  messageText = '';

  onKeyDown(event: KeyboardEvent): void {
    if (event.key === 'Enter' && !event.shiftKey) {
      event.preventDefault();
      this.send();
    }
  }

  send(): void {
    const text = this.messageText.trim();
    if (!text || this.disabled) return;

    this.messageSent.emit(text);
    this.messageText = '';

    // Refocus the input after sending
    setTimeout(() => {
      this.messageInput?.setFocus();
    }, 50);
  }
}
