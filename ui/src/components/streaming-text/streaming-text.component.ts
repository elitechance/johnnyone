import { Component, Input, ChangeDetectionStrategy } from '@angular/core';
import { CommonModule } from '@angular/common';

@Component({
  selector: 'johnny-streaming-text',
  standalone: true,
  imports: [CommonModule],
  templateUrl: './streaming-text.component.html',
  styleUrls: ['./streaming-text.component.scss'],
  changeDetection: ChangeDetectionStrategy.OnPush,
})
export class StreamingTextComponent {
  @Input() text = '';
  @Input() isStreaming = false;

  get hasText(): boolean {
    return this.text.length > 0;
  }

  get textSegments(): string[] {
    // Split text into paragraphs for rendering
    return this.text.split('\n');
  }
}
