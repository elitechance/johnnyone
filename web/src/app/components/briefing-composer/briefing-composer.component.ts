import { CommonModule } from '@angular/common';
import { Component, EventEmitter, Input, Output, inject, signal } from '@angular/core';
import { FormsModule } from '@angular/forms';
import { firstValueFrom } from 'rxjs';
import { JohnnyApiService, HostFileEntry } from '@johnnyone/ui';
import {
  pendingAttachmentsReducer,
  referencePathsReducer,
} from '../../pages/briefing/briefing-page-logic';

/**
 * The mock §07 briefing composer (overhaul P4): message input + 📎 Attach / ⤒ Upload / ▤ Reference
 * path + Send. Attach/Upload land device bytes in the initiative attachments store via the Phase-03
 * `initiativeUploadChunk` relay (D5); ▤ records a host path via `addInitiativeReferencePath` (D6),
 * reusing the planner's `browseHostDirectory` flow. All non-trivial chip state lives in the pure
 * `briefing-page-logic` reducers (D8) so this component stays thin.
 */
@Component({
  selector: 'app-briefing-composer',
  standalone: true,
  imports: [CommonModule, FormsModule],
  templateUrl: './briefing-composer.component.html',
  styleUrls: ['./briefing-composer.component.scss'],
})
export class BriefingComposerComponent {
  private readonly api = inject(JohnnyApiService);

  /** The Initiative whose attachments store / reference paths this composer writes to. */
  @Input() initiativeId!: string;
  @Input() disabled = false;

  @Output() messageSent = new EventEmitter<string>();
  @Output() attachmentsChanged = new EventEmitter<string[]>();
  @Output() referencePathAdded = new EventEmitter<string>();

  /** ≤1 MiB decoded per chunk — matches the host cap (`MAX_UPLOAD_CHUNK_BYTES`). */
  private static readonly CHUNK_BYTES = 1024 * 1024;

  protected message = '';
  protected readonly pendingAttachments = signal<string[]>([]);
  protected readonly referencePaths = signal<string[]>([]);
  protected readonly uploading = signal(false);
  protected readonly uploadError = signal<string | null>(null);

  // ── Host path browser (reuses api.browseHostDirectory — the planner flow) ──────────────────────
  protected readonly browserOpen = signal(false);
  protected readonly browsePath = signal('/');
  protected readonly browserEntries = signal<HostFileEntry[]>([]);
  protected readonly browserError = signal<string | null>(null);

  // ── Send ───────────────────────────────────────────────────────────────────────────────────────
  protected send(): void {
    const text = this.message.trim();
    if (!text || this.disabled) return;
    this.messageSent.emit(text);
    this.message = '';
  }

  protected onInputKeydown(event: KeyboardEvent): void {
    if (event.key === 'Enter' && !event.shiftKey) {
      event.preventDefault();
      this.send();
    }
  }

  // ── Attach / Upload (both affordances → same initiative attachments store) ─────────────────────
  protected async onFilesSelected(event: Event): Promise<void> {
    const input = event.target as HTMLInputElement;
    const files = Array.from(input.files ?? []);
    input.value = '';
    for (const file of files) {
      await this.uploadFile(file);
    }
  }

  private async uploadFile(file: File): Promise<void> {
    if (!this.initiativeId) return;
    this.uploading.set(true);
    this.uploadError.set(null);
    try {
      const bytes = new Uint8Array(await file.arrayBuffer());
      const total = Math.max(1, Math.ceil(bytes.length / BriefingComposerComponent.CHUNK_BYTES));
      let finalPath = file.name;
      for (let i = 0; i < total; i++) {
        const slice = bytes.subarray(
          i * BriefingComposerComponent.CHUNK_BYTES,
          (i + 1) * BriefingComposerComponent.CHUNK_BYTES,
        );
        const result = await firstValueFrom(
          this.api.initiativeUploadChunk(
            this.initiativeId,
            file.name,
            i,
            total,
            this.bytesToBase64(slice),
            i === total - 1,
          ),
        );
        finalPath = result.path;
      }
      this.pendingAttachments.update((state) =>
        pendingAttachmentsReducer(state, { type: 'add', value: finalPath }),
      );
      this.attachmentsChanged.emit(this.pendingAttachments());
    } catch (err) {
      this.uploadError.set(String(err));
    } finally {
      this.uploading.set(false);
    }
  }

  private bytesToBase64(bytes: Uint8Array): string {
    let binary = '';
    const stride = 0x8000; // avoid String.fromCharCode arg-count limits on large chunks
    for (let i = 0; i < bytes.length; i += stride) {
      binary += String.fromCharCode(...bytes.subarray(i, i + stride));
    }
    return btoa(binary);
  }

  protected removeAttachment(path: string): void {
    this.pendingAttachments.update((state) =>
      pendingAttachmentsReducer(state, { type: 'remove', value: path }),
    );
    this.attachmentsChanged.emit(this.pendingAttachments());
  }

  // ── ▤ Reference path (browse host dir → addInitiativeReferencePath) ────────────────────────────
  protected async openPathBrowser(): Promise<void> {
    this.browserOpen.set(true);
    await this.loadBrowsePath(this.browsePath() || '/');
  }

  protected closePathBrowser(): void {
    this.browserOpen.set(false);
  }

  protected async loadBrowsePath(path: string): Promise<void> {
    this.browsePath.set(path || '/');
    this.browserError.set(null);
    try {
      this.browserEntries.set(await firstValueFrom(this.api.browseHostDirectory(this.browsePath())));
    } catch (err) {
      this.browserEntries.set([]);
      this.browserError.set(String(err));
    }
  }

  protected async browseInto(entry: HostFileEntry): Promise<void> {
    if (entry.kind !== 'directory' && entry.kind !== 'dir') return;
    await this.loadBrowsePath(entry.path);
  }

  protected async browseParent(): Promise<void> {
    const current = this.browsePath().replace(/\/+$/, '');
    const parent = current.slice(0, current.lastIndexOf('/')) || '/';
    await this.loadBrowsePath(parent);
  }

  protected async useReferencePath(path = this.browsePath()): Promise<void> {
    if (!this.initiativeId || !path) return;
    try {
      await firstValueFrom(this.api.addInitiativeReferencePath(this.initiativeId, path));
      this.referencePaths.update((state) =>
        referencePathsReducer(state, { type: 'add', value: path }),
      );
      this.referencePathAdded.emit(path);
      this.browserOpen.set(false);
    } catch (err) {
      this.browserError.set(String(err));
    }
  }

  protected removeReferencePath(path: string): void {
    this.referencePaths.update((state) =>
      referencePathsReducer(state, { type: 'remove', value: path }),
    );
  }
}
