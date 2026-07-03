import { CommonModule } from '@angular/common';
import { Component, ElementRef, OnInit, ViewChild, computed, inject, signal } from '@angular/core';
import { FormsModule } from '@angular/forms';
import { DomSanitizer, SafeHtml } from '@angular/platform-browser';
import {
  IonHeader,
  IonToolbar,
  IonTitle,
  IonButtons,
  IonMenuButton,
  IonContent,
  IonIcon,
  IonSpinner,
  IonButton,
  IonTextarea,
  AlertController,
} from '@ionic/angular/standalone';
import {
  JohnnyApiService,
  MarkdownViewComponent,
  highlightCode,
  type FileEntry,
  type FileContent,
  type DirListing,
  type FileOpResult,
} from '@johnnyone/ui';
import { Observable, firstValueFrom } from 'rxjs';
import {
  breadcrumbs,
  codeLanguage,
  fileActionIntent,
  formatSize,
  isDirty,
  joinPath,
  normalizeFileError,
  previewMode,
  siblingPath,
  validateNewName,
  type Breadcrumb,
  type FileActionIntent,
  type PreviewMode,
} from './files-page-logic';
import {
  bytesToBase64,
  exceedsTotalCap,
  planChunks,
  uploadCallSequence,
  uploadProgress,
} from './file-chunker';

/** A per-file upload progress chip (mock §05 `.upchip`). */
interface UploadChip {
  name: string;
  progress: number; // 0..1
  done: boolean;
  error?: string;
}

/**
 * The Files manager (mock §05, overhaul P5). Phase 01 shipped the read path (browse + preview); Phase
 * 02 adds the WRITE path: an inline editor (textarea + Save + `● unsaved`) and a CRUD toolbar (New
 * file / New folder / Rename / Delete). Every op reuses an existing P2 `JohnnyApiService` method; the
 * name/dirty/rename math lives in the pure `files-page-logic.ts`; dialogs reuse Ionic `AlertController`
 * (D8). Rooted at `files_root`; the host guards every path — the client only surfaces its errors (D2).
 * Delete is always confirm-gated; file `content` is never logged (D10).
 */
@Component({
  selector: 'app-files',
  standalone: true,
  imports: [
    CommonModule,
    FormsModule,
    IonHeader,
    IonToolbar,
    IonTitle,
    IonButtons,
    IonMenuButton,
    IonContent,
    IonIcon,
    IonSpinner,
    IonButton,
    IonTextarea,
    MarkdownViewComponent,
  ],
  templateUrl: './files.page.html',
  styleUrls: ['./files.page.scss'],
})
export class FilesPage implements OnInit {
  private readonly api = inject(JohnnyApiService);
  private readonly sanitizer = inject(DomSanitizer);
  private readonly alertCtrl = inject(AlertController);

  /** Display label for the root crumb — basename of the configured `files_root` (D2). */
  protected readonly rootLabel = signal('Workspace');
  /** Current directory, `files_root`-relative (`''` = root). */
  protected readonly cwd = signal('');
  protected readonly entries = signal<FileEntry[]>([]);
  protected readonly listing = signal<DirListing | null>(null);
  protected readonly selected = signal<FileEntry | null>(null);
  protected readonly content = signal<FileContent | null>(null);
  protected readonly mode = signal<PreviewMode | null>(null);
  protected readonly loadError = signal<string | null>(null);
  protected readonly busy = signal(false);

  // ── Editor state (Phase 02) ─────────────────────────────────────────────────────────────────────
  /** `true` = editing in a textarea; `false` = rendered/highlighted preview. */
  protected readonly editing = signal(false);
  /** The last-saved content (baseline for dirty). */
  protected readonly original = signal('');
  /** The live editor content (bound to the textarea). */
  protected readonly current = signal('');
  /** Unsaved-edits flag driving the `● unsaved` badge — the pure `isDirty` (D4). */
  protected readonly dirty = computed(() => isDirty(this.original(), this.current()));
  /** Only text/code files are editable; markdown previews rendered, binary is a notice (D3). */
  protected readonly canEdit = computed(() => this.mode() === 'code' || this.mode() === 'text');

  // ── Upload state (Phase 03) ─────────────────────────────────────────────────────────────────────
  /** Per-file progress chips (mock §05 `.uprow`/`.upchip`). */
  protected readonly uploads = signal<UploadChip[]>([]);
  /** True while a file is dragged over the drop zone (for the highlight). */
  protected readonly dragging = signal(false);
  @ViewChild('fileInput') private fileInput?: ElementRef<HTMLInputElement>;

  /** Breadcrumb crumbs for the current directory (pure `breadcrumbs`). */
  protected readonly crumbs = computed<Breadcrumb[]>(() =>
    breadcrumbs(this.cwd(), this.rootLabel()),
  );

  /** Highlighted HTML for the `code` preview mode — same trust posture as `johnny-markdown-view` (D10). */
  protected readonly highlighted = computed<SafeHtml>(() => {
    const file = this.content();
    if (!file || this.mode() !== 'code') return '';
    const { html } = highlightCode(file.content, codeLanguage(file.name));
    return this.sanitizer.bypassSecurityTrustHtml(html);
  });

  ngOnInit(): void {
    this.api.getSetting('files_root').subscribe({
      next: (value) => this.rootLabel.set(this.basename(value) || 'Workspace'),
      // A missing/failed setting is non-fatal — keep the default label and still browse.
      error: () => this.rootLabel.set('Workspace'),
    });
    this.openDir('');
  }

  // ── Navigation (guarded against losing unsaved edits) ─────────────────────────────────────────────

  /** Breadcrumb click — confirm discard if dirty, then descend/ascend. */
  protected async onCrumbClick(path: string): Promise<void> {
    if (!(await this.guardDirty())) return;
    this.openDir(path);
  }

  /** Row click — confirm discard if dirty, then descend into a dir or open a file. */
  protected async onEntryClick(entry: FileEntry): Promise<void> {
    if (!(await this.guardDirty())) return;
    if (entry.kind === 'dir') {
      this.openDir(joinPath(this.cwd(), entry.name));
      return;
    }
    this.openFile(entry);
  }

  /** Load a directory listing (no dirty-guard — callers guard; `refresh()` reuses this). */
  private openDir(path: string): void {
    this.busy.set(true);
    this.loadError.set(null);
    this.api.listDir(path).subscribe({
      next: (listing) => {
        this.listing.set(listing);
        this.entries.set(this.sortEntries(listing.entries));
        this.cwd.set(listing.path ?? path);
        // Leaving the directory clears the preview/editor.
        this.clearSelection();
        this.busy.set(false);
      },
      error: (err) => {
        this.busy.set(false);
        this.loadError.set(normalizeFileError(err));
      },
    });
  }

  /** Read a file into the preview/editor pane, routed by `previewMode`. */
  private openFile(entry: FileEntry): void {
    this.busy.set(true);
    this.loadError.set(null);
    this.api.readFile(entry.path).subscribe({
      next: (file) => {
        this.selected.set(entry);
        this.content.set(file);
        this.mode.set(previewMode(entry.name, file.contentType, file.encoding));
        this.original.set(file.content);
        this.current.set(file.content);
        this.editing.set(false);
        this.busy.set(false);
      },
      error: (err) => {
        this.busy.set(false);
        this.loadError.set(normalizeFileError(err));
      },
    });
  }

  // ── Editor ────────────────────────────────────────────────────────────────────────────────────

  protected toggleEdit(): void {
    if (!this.canEdit()) return;
    this.editing.update((v) => !v);
  }

  protected onEditInput(value: string): void {
    this.current.set(value ?? '');
  }

  /** Persist the editor content via the P2 `writeFile`; on success the baseline advances (dirty clears). */
  protected save(): void {
    const sel = this.selected();
    if (!sel || !this.dirty() || this.busy()) return;
    const next = this.current();
    this.busy.set(true);
    this.loadError.set(null);
    this.api.writeFile(sel.path, next).subscribe({
      next: () => {
        this.original.set(next);
        const file = this.content();
        if (file) this.content.set({ ...file, content: next });
        this.busy.set(false);
      },
      error: (err) => {
        this.busy.set(false);
        this.loadError.set(normalizeFileError(err));
      },
    });
  }

  // ── Toolbar CRUD (dispatch the pure `fileActionIntent`) ───────────────────────────────────────────

  protected async newFile(): Promise<void> {
    const name = await this.promptName('New file', '');
    if (name === null) return;
    if (!this.validName(name)) return;
    this.runIntent(fileActionIntent('newFile', { cwd: this.cwd(), name }), { openAfter: name });
  }

  protected async newFolder(): Promise<void> {
    const name = await this.promptName('New folder', '');
    if (name === null) return;
    if (!this.validName(name)) return;
    this.runIntent(fileActionIntent('newFolder', { cwd: this.cwd(), name }));
  }

  protected async rename(): Promise<void> {
    const sel = this.selected();
    if (!sel) return;
    const name = await this.promptName('Rename', sel.name);
    if (name === null) return;
    if (!this.validName(name, sel.name)) return;
    this.runIntent(
      fileActionIntent('rename', { cwd: this.cwd(), name, selectedPath: sel.path }),
      { clearSelection: true },
    );
  }

  protected async remove(): Promise<void> {
    const sel = this.selected();
    if (!sel) return;
    const ok = await this.confirm(
      'Delete ' + sel.name + '?',
      'This permanently deletes it from the host filesystem.',
      'Delete',
    );
    if (!ok) return;
    this.runIntent(
      fileActionIntent('delete', { cwd: this.cwd(), selectedPath: sel.path }),
      { clearSelection: true },
    );
  }

  // ── Upload (drag-drop + button) — Phase 03 ────────────────────────────────────────────────────

  /** Toolbar ⤒ Upload → open the native file picker. */
  protected triggerUpload(): void {
    this.fileInput?.nativeElement.click();
  }

  protected onFilePicked(event: Event): void {
    const input = event.target as HTMLInputElement;
    const files = Array.from(input.files ?? []);
    input.value = ''; // allow re-picking the same file
    void this.uploadFiles(files);
  }

  protected onDragOver(event: DragEvent): void {
    event.preventDefault();
    this.dragging.set(true);
  }

  protected onDragLeave(event: DragEvent): void {
    event.preventDefault();
    this.dragging.set(false);
  }

  protected onDrop(event: DragEvent): void {
    event.preventDefault();
    this.dragging.set(false);
    const files = Array.from(event.dataTransfer?.files ?? []);
    void this.uploadFiles(files);
  }

  /**
   * Upload each file into the current directory: per file, pre-check the 50 MiB cap, then slice per
   * `planChunks` and stream ≤1 MiB base64 chunks via the P2 `uploadChunk` following the pure
   * `uploadCallSequence` (monotonic index, `done` only on the last). The only runtime step is the
   * `slice`/`arrayBuffer` read; the encode is the pinned `bytesToBase64`. `dataBase64` is never
   * logged (D10). On the final `complete`, refresh the listing so the new file appears.
   */
  protected async uploadFiles(files: File[]): Promise<void> {
    if (files.length === 0) return;
    this.loadError.set(null);
    let anyCompleted = false;

    for (const file of files) {
      const chipIndex = this.pushChip({ name: file.name, progress: 0, done: false });

      if (exceedsTotalCap(file.size)) {
        this.patchChip(chipIndex, { error: 'Exceeds the 50 MiB upload cap' });
        continue;
      }

      const chunks = planChunks(file.size);
      const sequence = uploadCallSequence(chunks.length);
      const dest = siblingPath(this.cwd(), file.name);

      try {
        for (let i = 0; i < chunks.length; i += 1) {
          const bytes = new Uint8Array(
            await file.slice(chunks[i].start, chunks[i].end).arrayBuffer(),
          );
          const dataBase64 = bytesToBase64(bytes); // payload — never logged (D10)
          const result = await firstValueFrom(
            this.api.uploadChunk(
              dest,
              sequence[i].chunkIndex,
              sequence[i].totalChunks,
              dataBase64,
              sequence[i].done,
            ),
          );
          this.patchChip(chipIndex, {
            progress: uploadProgress(result?.received ?? i + 1, chunks.length),
          });
          if (result?.complete) {
            this.patchChip(chipIndex, { done: true, progress: 1 });
            anyCompleted = true;
          }
        }
      } catch (err) {
        this.patchChip(chipIndex, { error: normalizeFileError(err) });
      }
    }

    if (anyCompleted) this.refresh();
  }

  protected progressPercent(chip: UploadChip): number {
    return Math.round(chip.progress * 100);
  }

  private pushChip(chip: UploadChip): number {
    let index = 0;
    this.uploads.update((list) => {
      index = list.length;
      return [...list, chip];
    });
    return index;
  }

  private patchChip(index: number, patch: Partial<UploadChip>): void {
    this.uploads.update((list) => list.map((c, i) => (i === index ? { ...c, ...patch } : c)));
  }

  /** Re-list the current directory, optionally opening a just-created file, preserving selection by path. */
  protected refresh(opts?: { openAfter?: string }): void {
    const keepPath = this.selected()?.path;
    this.api.listDir(this.cwd()).subscribe({
      next: (listing) => {
        this.listing.set(listing);
        const sorted = this.sortEntries(listing.entries);
        this.entries.set(sorted);
        if (opts?.openAfter) {
          const created = sorted.find((e) => e.name === opts.openAfter && e.kind !== 'dir');
          if (created) this.openFile(created);
        } else if (keepPath && !sorted.some((e) => e.path === keepPath)) {
          // Selected entry vanished (renamed/deleted elsewhere) — drop the stale preview.
          this.clearSelection();
        }
      },
      error: (err) => this.loadError.set(normalizeFileError(err)),
    });
  }

  protected sizeLabel(entry: FileEntry): string {
    return formatSize(entry.size);
  }

  // ── Internals ────────────────────────────────────────────────────────────────────────────────────

  /** Dispatch a `{op,args}` intent to the matching P2 op, then refresh the list. */
  private runIntent(
    intent: FileActionIntent,
    opts?: { openAfter?: string; clearSelection?: boolean },
  ): void {
    this.busy.set(true);
    this.loadError.set(null);
    this.dispatch(intent).subscribe({
      next: () => {
        this.busy.set(false);
        if (opts?.clearSelection) this.clearSelection();
        this.refresh({ openAfter: opts?.openAfter });
      },
      error: (err) => {
        this.busy.set(false);
        this.loadError.set(normalizeFileError(err));
      },
    });
  }

  /** Map a pure intent to the concrete P2 call — the op+args mapping itself is unit-tested. */
  private dispatch(intent: FileActionIntent): Observable<FileOpResult> {
    const [a, b] = intent.args as [string, string?];
    switch (intent.op) {
      case 'writeFile':
        return this.api.writeFile(a, b ?? '');
      case 'mkdir':
        return this.api.mkdir(a);
      case 'rename':
        return this.api.rename(a, b ?? '');
      case 'deleteFile':
        return this.api.deleteFile(a);
    }
  }

  /** Validate a proposed name against current siblings (optionally excluding the entry being renamed). */
  private validName(name: string, exclude?: string): boolean {
    const siblings = this.entries()
      .map((e) => e.name)
      .filter((n) => n !== exclude);
    const result = validateNewName(name, siblings);
    if (!result.ok) {
      this.loadError.set(result.error ?? 'Invalid name');
      return false;
    }
    this.loadError.set(null);
    return true;
  }

  /** If there are unsaved edits, confirm discarding them before navigating away (D4). */
  private async guardDirty(): Promise<boolean> {
    if (!this.dirty()) return true;
    return this.confirm(
      'Discard unsaved changes?',
      'Your edits to this file have not been saved.',
      'Discard',
    );
  }

  private clearSelection(): void {
    this.selected.set(null);
    this.content.set(null);
    this.mode.set(null);
    this.original.set('');
    this.current.set('');
    this.editing.set(false);
  }

  private async confirm(header: string, message: string, confirmText: string): Promise<boolean> {
    const alert = await this.alertCtrl.create({
      header,
      message,
      buttons: [
        { text: 'Cancel', role: 'cancel' },
        { text: confirmText, role: 'confirm' },
      ],
    });
    await alert.present();
    const { role } = await alert.onDidDismiss();
    return role === 'confirm';
  }

  private async promptName(header: string, value: string): Promise<string | null> {
    const alert = await this.alertCtrl.create({
      header,
      inputs: [{ name: 'name', type: 'text', value, placeholder: 'name' }],
      buttons: [
        { text: 'Cancel', role: 'cancel' },
        { text: 'OK', role: 'confirm' },
      ],
    });
    await alert.present();
    const { role, data } = await alert.onDidDismiss();
    if (role !== 'confirm') return null;
    return String(data?.values?.name ?? '').trim();
  }

  private sortEntries(entries: FileEntry[]): FileEntry[] {
    return [...entries].sort((a, b) => {
      const aDir = a.kind === 'dir' ? 0 : 1;
      const bDir = b.kind === 'dir' ? 0 : 1;
      if (aDir !== bDir) return aDir - bDir;
      return a.name.localeCompare(b.name, undefined, { sensitivity: 'base' });
    });
  }

  private basename(value: string): string {
    return (value ?? '').replace(/\/+$/, '').split('/').pop() ?? '';
  }
}
