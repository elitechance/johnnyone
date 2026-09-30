import { Injectable, signal } from '@angular/core';

/** What the viewer is showing. `html` renders sandboxed; `image` and `text` render directly. */
export type FileViewKind = 'html' | 'image' | 'text';

export interface FileView {
  /** The path as the report wrote it — shown as the modal title. */
  path: string;
  kind: FileViewKind;
  /** Object URL for `html`, data URI for `image`, raw content for `text`. */
  source: string;
}

/**
 * Global state for the file viewer modal.
 *
 * Files a report mentions used to open in a new tab, which on a phone means leaving the
 * conversation and finding your way back. One modal lives at the app root and any surface can
 * `open(...)` it, so the file shows over the transcript and closes back to exactly where you were.
 */
@Injectable({ providedIn: 'root' })
export class FileViewerService {
  readonly isOpen = signal(false);
  readonly view = signal<FileView | null>(null);
  /** Set while a file is being fetched, so the caller can show progress on the button. */
  readonly loadingPath = signal<string | null>(null);

  open(view: FileView): void {
    // Release any previous blob before replacing it — these are not garbage collected on their own.
    this.revoke();
    this.view.set(view);
    this.isOpen.set(true);
  }

  close(): void {
    this.isOpen.set(false);
    // Clear after the dismiss animation so the next open does not flash the previous file.
    setTimeout(() => {
      this.revoke();
      this.view.set(null);
    }, 300);
  }

  private revoke(): void {
    const current = this.view();
    if (current?.kind === 'html' && current.source.startsWith('blob:')) {
      URL.revokeObjectURL(current.source);
    }
  }
}
