import { CommonModule } from '@angular/common';
import { Component, EventEmitter, Input, Output } from '@angular/core';
import { FormsModule } from '@angular/forms';

export interface PendingImageAttachment {
  id: string;
  file: File;
  previewUrl: string;
}

@Component({
  selector: 'app-image-attachment-composer',
  standalone: true,
  imports: [CommonModule, FormsModule],
  templateUrl: './image-attachment-composer.component.html',
  styleUrl: './image-attachment-composer.component.scss',
})
export class ImageAttachmentComposerComponent {
  @Input() attachments: PendingImageAttachment[] = [];
  @Input() message = '';
  @Input() placeholder = 'Message with image';
  @Input() browseLabel = 'Image';
  @Input() sendLabel = 'Send image';
  @Input() sendingLabel = 'Sending...';
  @Input() idleLabel = 'paste, drop, or attach images';
  @Input() disabled = false;
  @Input() sending = false;

  @Output() messageChange = new EventEmitter<string>();
  @Output() send = new EventEmitter<void>();
  @Output() removeAttachment = new EventEmitter<string>();
  @Output() filesSelected = new EventEmitter<File[]>();
  @Output() imagePasted = new EventEmitter<File[]>();
  @Output() imageDropped = new EventEmitter<File[]>();

  protected onPaste(event: ClipboardEvent): void {
    const files = Array.from(event.clipboardData?.files ?? []).filter((file) => file.type.startsWith('image/'));
    if (files.length === 0) return;
    event.preventDefault();
    this.imagePasted.emit(files);
  }

  protected onDragOver(event: DragEvent): void {
    if (!this.hasImageFile(event)) return;
    event.preventDefault();
  }

  protected onDrop(event: DragEvent): void {
    const files = Array.from(event.dataTransfer?.files ?? []).filter((file) => file.type.startsWith('image/'));
    if (files.length === 0) return;
    event.preventDefault();
    this.imageDropped.emit(files);
  }

  protected onBrowse(event: Event): void {
    const input = event.target as HTMLInputElement;
    const files = Array.from(input.files ?? []).filter((file) => file.type.startsWith('image/'));
    if (files.length > 0) {
      this.filesSelected.emit(files);
    }
    input.value = '';
  }

  private hasImageFile(event: DragEvent): boolean {
    return Array.from(event.dataTransfer?.items ?? []).some((item) =>
      item.kind === 'file' && item.type.startsWith('image/')
    );
  }
}
