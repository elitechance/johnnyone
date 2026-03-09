import {
  Component,
  Input,
  Output,
  EventEmitter,
  ViewChild,
  ChangeDetectionStrategy,
  AfterViewInit,
  OnDestroy,
  ChangeDetectorRef,
} from '@angular/core';
import { CommonModule } from '@angular/common';
import { FormsModule } from '@angular/forms';
import {
  IonTextarea,
  IonItem,
  IonButton,
  IonIcon,
} from '@ionic/angular/standalone';

interface PastedImage {
  id: string;
  name: string;
  dataUrl: string;
}

interface ClipboardImageSource {
  name: string;
  dataUrl: string;
  byteSize: number;
}

@Component({
  selector: 'johnny-message-composer',
  standalone: true,
  imports: [CommonModule, FormsModule, IonTextarea, IonItem, IonButton, IonIcon],
  templateUrl: './message-composer.component.html',
  styleUrls: ['./message-composer.component.scss'],
  changeDetection: ChangeDetectionStrategy.OnPush,
})
export class MessageComposerComponent implements AfterViewInit, OnDestroy {
  @Input() disabled = false;
  @Input() placeholder = 'Type a message...';

  @Output() messageSent = new EventEmitter<string>();

  @ViewChild('messageInput') messageInput?: IonTextarea;

  messageText = '';
  pastedImages: PastedImage[] = [];
  pasteNotice = '';
  isDragOver = false;

  private readonly maxPastedImages = 4;
  private readonly maxImageBytes = 2 * 1024 * 1024;
  private dragDepth = 0;
  private nativeInputElement?: HTMLTextAreaElement;
  private readonly nativePasteHandler = (event: ClipboardEvent): void => {
    void this.onPaste(event);
  };

  constructor(private cdr: ChangeDetectorRef) {}

  async ngAfterViewInit(): Promise<void> {
    try {
      const nativeInput = await this.messageInput?.getInputElement();
      if (!nativeInput) return;

      this.nativeInputElement = nativeInput;
      this.nativeInputElement.addEventListener('paste', this.nativePasteHandler);
    } catch {
      // No-op if native input isn't ready
    }
  }

  ngOnDestroy(): void {
    if (this.nativeInputElement) {
      this.nativeInputElement.removeEventListener('paste', this.nativePasteHandler);
    }
  }

  get canSend(): boolean {
    return !this.disabled && (!!this.messageText.trim() || this.pastedImages.length > 0);
  }

  onKeyDown(event: KeyboardEvent): void {
    if (event.key === 'Enter' && !event.shiftKey) {
      event.preventDefault();
      this.send();
    }
  }

  async onPaste(event: ClipboardEvent): Promise<void> {
    if (this.disabled) return;

    const clipboardItems = Array.from(event.clipboardData?.items ?? []);
    const imageItems = clipboardItems.filter((item) => item.type.startsWith('image/'));

    event.preventDefault();

    if (imageItems.length > 0) {
      const files = imageItems
        .map((item) => item.getAsFile())
        .filter((file): file is File => !!file);
      const result = await this.addImageFiles(files);
      this.setAttachNotice(result.added, result.skipped, 'pasted');
      this.cdr.markForCheck();
      return;
    }

    const fallbackImages = await this.readImagesFromFallbackChain();
    if (fallbackImages.length === 0) {
      this.pasteNotice = 'Clipboard has no image data. Copy an image, then paste here.';
      this.cdr.markForCheck();
      return;
    }

    const result = await this.addClipboardImages(fallbackImages);
    this.setAttachNotice(result.added, result.skipped, 'pasted');
    this.cdr.markForCheck();
  }

  onDragEnter(event: DragEvent): void {
    if (this.disabled || !this.eventHasImage(event)) return;

    event.preventDefault();
    this.dragDepth += 1;
    this.isDragOver = true;
    this.cdr.markForCheck();
  }

  onDragOver(event: DragEvent): void {
    if (this.disabled || !this.eventHasImage(event)) return;

    event.preventDefault();
    event.stopPropagation();

    if (event.dataTransfer) {
      event.dataTransfer.dropEffect = 'copy';
    }

    this.isDragOver = true;
    this.cdr.markForCheck();
  }

  onDragLeave(event: DragEvent): void {
    if (this.disabled) return;

    event.preventDefault();
    this.dragDepth = Math.max(0, this.dragDepth - 1);
    if (this.dragDepth === 0) {
      this.isDragOver = false;
    }
    this.cdr.markForCheck();
  }

  async onDrop(event: DragEvent): Promise<void> {
    if (this.disabled) return;

    event.preventDefault();
    event.stopPropagation();

    this.dragDepth = 0;
    this.isDragOver = false;

    if (!this.eventHasImage(event)) {
      this.pasteNotice = 'Drop an image file (png, jpg, webp, gif, bmp, svg).';
      this.cdr.markForCheck();
      return;
    }

    const files = Array.from(event.dataTransfer?.files ?? []);

    if (files.length === 0) {
      this.setAttachNotice(0, 1, 'dropped');
      return;
    }

    const result = await this.addImageFiles(files);
    this.setAttachNotice(result.added, result.skipped, 'dropped');
    this.cdr.markForCheck();
  }

  removePastedImage(imageId: string): void {
    this.pastedImages = this.pastedImages.filter((img) => img.id !== imageId);
    if (this.pastedImages.length === 0) {
      this.pasteNotice = '';
    }
    this.cdr.markForCheck();
  }

  send(): void {
    const text = this.messageText.trim();
    if ((!text && this.pastedImages.length === 0) || this.disabled) return;

    const imagesMarkdown = this.pastedImages
      .map((img) => `![${img.name}](${img.dataUrl})`)
      .join('\n\n');

    const payload = [text, imagesMarkdown].filter(Boolean).join('\n\n');

    this.messageSent.emit(payload);
    this.messageText = '';
    this.pastedImages = [];
    this.pasteNotice = '';
    this.cdr.markForCheck();

    // Refocus the input after sending
    setTimeout(() => {
      this.messageInput?.setFocus();
    }, 50);
  }

  private readFileAsDataUrl(file: File): Promise<string> {
    return new Promise<string>((resolve, reject) => {
      const reader = new FileReader();
      reader.onload = () => {
        if (typeof reader.result === 'string') {
          resolve(reader.result);
        } else {
          reject(new Error('Failed to read image.'));
        }
      };
      reader.onerror = () => reject(new Error('Failed to read image.'));
      reader.readAsDataURL(file);
    });
  }

  private createImageId(): string {
    if (typeof crypto !== 'undefined' && 'randomUUID' in crypto) {
      return crypto.randomUUID();
    }

    return `${Date.now()}-${Math.random().toString(36).slice(2, 8)}`;
  }

  private async readImagesFromFallbackChain(): Promise<ClipboardImageSource[]> {
    const webClipboardImages = await this.readImagesFromNavigatorClipboard();
    if (webClipboardImages.length > 0) {
      return webClipboardImages;
    }

    const tauriClipboardImages = await this.readImagesFromTauriClipboard();
    if (tauriClipboardImages.length > 0) {
      return tauriClipboardImages;
    }

    return [];
  }

  private async readImagesFromNavigatorClipboard(): Promise<ClipboardImageSource[]> {
    if (!navigator.clipboard || typeof navigator.clipboard.read !== 'function') {
      return [];
    }

    try {
      const items = await navigator.clipboard.read();
      const images: ClipboardImageSource[] = [];

      for (const item of items) {
        const imageType = item.types.find((type) => type.startsWith('image/'));
        if (!imageType) continue;

        try {
          const blob = await item.getType(imageType);
          const dataUrl = await this.readBlobAsDataUrl(blob);
          const ext = this.fileExtensionFromMimeType(imageType);
          images.push({
            name: `clipboard-image-${images.length + 1}.${ext}`,
            dataUrl,
            byteSize: blob.size,
          });
        } catch {
          // Skip unreadable item
        }
      }

      return images;
    } catch {
      return [];
    }
  }

  private async readImagesFromTauriClipboard(): Promise<ClipboardImageSource[]> {
    if (typeof window === 'undefined' || !('__TAURI_INTERNALS__' in window)) {
      return [];
    }

    try {
      const { invoke } = await import('@tauri-apps/api/core');
      const dataUrl = await invoke<string | null>('read_clipboard_image_data_url');
      if (!dataUrl || !dataUrl.startsWith('data:image/')) {
        return [];
      }

      return [
        {
          name: 'clipboard-image.png',
          dataUrl,
          byteSize: this.estimateDataUrlBytes(dataUrl),
        },
      ];
    } catch {
      return [];
    }
  }

  private readBlobAsDataUrl(blob: Blob): Promise<string> {
    return new Promise<string>((resolve, reject) => {
      const reader = new FileReader();
      reader.onload = () => {
        if (typeof reader.result === 'string') {
          resolve(reader.result);
        } else {
          reject(new Error('Failed to read image blob.'));
        }
      };
      reader.onerror = () => reject(new Error('Failed to read image blob.'));
      reader.readAsDataURL(blob);
    });
  }

  private fileExtensionFromMimeType(mime: string): string {
    const normalized = mime.toLowerCase();
    if (normalized === 'image/jpeg') return 'jpg';
    if (normalized === 'image/svg+xml') return 'svg';
    return normalized.replace('image/', '') || 'png';
  }

  private estimateDataUrlBytes(dataUrl: string): number {
    const [, base64 = ''] = dataUrl.split(',', 2);
    if (!base64) return 0;
    const padding = base64.endsWith('==') ? 2 : base64.endsWith('=') ? 1 : 0;
    return Math.max(0, Math.floor((base64.length * 3) / 4) - padding);
  }

  private eventHasImage(event: DragEvent): boolean {
    const types = Array.from(event.dataTransfer?.types ?? []);
    if (types.includes('Files')) {
      return true;
    }

    const items = Array.from(event.dataTransfer?.items ?? []);
    if (items.some((item) => item.kind === 'file')) {
      return true;
    }

    const files = Array.from(event.dataTransfer?.files ?? []);
    return files.some((file) => this.isImageFile(file));
  }

  private isImageFile(file: File): boolean {
    if (file.type.startsWith('image/')) {
      return true;
    }

    const fileName = file.name.toLowerCase();
    return ['.png', '.jpg', '.jpeg', '.gif', '.webp', '.bmp', '.svg'].some((ext) =>
      fileName.endsWith(ext)
    );
  }

  private async addImageFiles(files: File[]): Promise<{ added: number; skipped: number }> {
    let added = 0;
    let skipped = 0;

    for (const file of files) {
      if (this.pastedImages.length >= this.maxPastedImages) {
        skipped++;
        continue;
      }

      if (!this.isImageFile(file) || file.size > this.maxImageBytes) {
        skipped++;
        continue;
      }

      try {
        const dataUrl = await this.readFileAsDataUrl(file);
        this.pastedImages = [
          ...this.pastedImages,
          {
            id: this.createImageId(),
            name: file.name || `pasted-image-${this.pastedImages.length + 1}.png`,
            dataUrl,
          },
        ];
        added++;
      } catch {
        skipped++;
      }
    }

    return { added, skipped };
  }

  private async addClipboardImages(
    images: ClipboardImageSource[]
  ): Promise<{ added: number; skipped: number }> {
    let added = 0;
    let skipped = 0;

    for (const image of images) {
      if (this.pastedImages.length >= this.maxPastedImages) {
        skipped++;
        continue;
      }

      if (!image.dataUrl.startsWith('data:image/') || image.byteSize > this.maxImageBytes) {
        skipped++;
        continue;
      }

      this.pastedImages = [
        ...this.pastedImages,
        {
          id: this.createImageId(),
          name: image.name || `pasted-image-${this.pastedImages.length + 1}.png`,
          dataUrl: image.dataUrl,
        },
      ];
      added++;
    }

    return { added, skipped };
  }

  private setAttachNotice(added: number, skipped: number, source: 'pasted' | 'dropped'): void {
    const action = source === 'pasted' ? 'pasted' : 'dropped';

    if (added > 0 && skipped === 0) {
      this.pasteNotice = `${added} image${added > 1 ? 's' : ''} ${action}.`;
      return;
    }

    if (added > 0 && skipped > 0) {
      this.pasteNotice = `${added} image${added > 1 ? 's' : ''} ${action}, ${skipped} skipped.`;
      return;
    }

    this.pasteNotice = `Image ${action} failed. Max ${this.maxPastedImages} images, ${Math.floor(
      this.maxImageBytes / (1024 * 1024)
    )}MB each.`;
    this.cdr.markForCheck();
  }
}
