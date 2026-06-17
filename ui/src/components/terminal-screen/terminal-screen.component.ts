import {
  AfterViewInit,
  ChangeDetectorRef,
  Component,
  ElementRef,
  EventEmitter,
  inject,
  Input,
  OnChanges,
  OnDestroy,
  Output,
  SimpleChanges,
  ViewChild,
} from '@angular/core';
import { CommonModule } from '@angular/common';
import { FormsModule } from '@angular/forms';
import { Terminal } from '@xterm/xterm';
import { FitAddon } from '@xterm/addon-fit';
import { TerminalScreen } from '../../models/terminal.model';
import { normalizeTerminalPlainText } from '../../services/terminal-transcript';

export interface TerminalImageAttachmentPreview {
  id: string;
  previewUrl: string;
  fileName?: string;
}

@Component({
  selector: 'johnny-terminal-screen',
  standalone: true,
  imports: [CommonModule, FormsModule],
  templateUrl: './terminal-screen.component.html',
  styleUrls: ['./terminal-screen.component.scss'],
})
export class TerminalScreenComponent implements AfterViewInit, OnChanges, OnDestroy {
  private readonly changeDetector = inject(ChangeDetectorRef);

  @Input() screen: TerminalScreen | null = null;
  @Input() disabled = false;
  @Input() mobileInputMode = false;
  /** Provider id (e.g. grok, codex) — Grok TUI needs color + layout preserved on mobile. */
  @Input() terminalProvider = '';
  @Input() showInput = true;
  @Input() title = 'Terminal';
  @Input() imageAttachments: TerminalImageAttachmentPreview[] = [];
  @Input() imageSending = false;

  @Output() rawInput = new EventEmitter<string>();
  @Output() imagePasted = new EventEmitter<File[]>();
  @Output() imageRemoved = new EventEmitter<string>();
  @Output() imageMessageSubmitted = new EventEmitter<string>();
  @Output() terminalResize = new EventEmitter<{ cols: number; rows: number }>();

  @ViewChild('terminalShell', { static: true }) private terminalShell!: ElementRef<HTMLElement>;
  @ViewChild('terminalHost', { static: true }) private terminalHost!: ElementRef<HTMLDivElement>;
  @ViewChild('mobileInput') private mobileInput?: ElementRef<HTMLTextAreaElement>;

  private terminal: Terminal | null = null;
  private fitAddon: FitAddon | null = null;
  private resizeObserver: ResizeObserver | null = null;
  private lastCursor: number | null = null;
  private lastContent = '';
  private lastRenderedPlain = '';
  private lastWrittenClipped = '';
  private pendingRender = false;
  private renderScreenTimer: ReturnType<typeof setTimeout> | null = null;
  private writing = false;
  private wheelHandler: ((event: WheelEvent) => void) | null = null;
  /** A live frame arrived while text was selected; repaint once selection clears. */
  private pendingSelectionRender = false;
  private pointerScrollDownHandler: ((event: PointerEvent) => void) | null = null;
  private pointerScrollMoveHandler: ((event: PointerEvent) => void) | null = null;
  private pointerScrollUpHandler: ((event: PointerEvent) => void) | null = null;
  private xtermViewportScrollHandler: (() => void) | null = null;
  private pointerScroll = {
    tracking: false,
    scrolling: false,
    pointerId: -1,
    lastY: 0,
    /** Fractional lines carried between moves so 1:1 tracking stays smooth. */
    accum: 0,
    /** Timestamp of the last move, for flick-velocity estimation. */
    lastT: 0,
    /** Smoothed velocity in lines/ms (sign = drag direction) for momentum. */
    velocity: 0,
  };
  private readonly pointerScrollThresholdPx = 6;
  private momentumRaf: number | null = null;
  private momentumAccum = 0;
  /** When false, mobile mode keeps the user's scroll position instead of following new output. */
  private userPinnedToLatest = true;
  /** Last mobile viewport scrollTop, to detect a *deliberate upward* scroll (vs. a
   * programmatic scroll or simply not being able to reach the exact bottom). */
  private lastMobileScrollTop = 0;
  private idlePromptTimer: ReturnType<typeof setTimeout> | null = null;
  private lastScreenKey = '';
  protected mobileInputBuffer = '';
  protected grokTerminalLayout = false;
  /** Log of what the user actually submitted (preserved even though the relay
   *  hands a rewritten pointer to the CLI), so the question isn't lost in the UI. */
  protected sentMessages: { id: number; text: string; images: string[] }[] = [];
  protected sentLogOpen = false;
  private sentMessageSeq = 0;
  /** Transient "Copied" confirmation after a long-press copy. */
  protected copyToast = '';
  private copyToastTimer: ReturnType<typeof setTimeout> | null = null;
  private longPressTimer: ReturnType<typeof setTimeout> | null = null;
  private longPressFired = false;
  /** Last ANSI-rich scrollback used for mobile live view (survives small TUI redraws). */
  private liveAnsiSnapshot = '';
  private lastSeenScreenLines = 0;
  private visibilityHandler: (() => void) | null = null;
  private pageshowHandler: ((event: PageTransitionEvent) => void) | null = null;

  ngAfterViewInit(): void {
    this.terminal = new Terminal({
      cursorBlink: true,
      convertEol: true,
      disableStdin: true,
      fontFamily: 'Menlo, Monaco, Consolas, "Liberation Mono", "Courier New", monospace',
      fontSize: this.terminalFontSize(),
      lineHeight: this.mobileInputMode ? 1.1 : 1.2,
      minimumContrastRatio: this.mobileInputMode ? 4.5 : 1,
      scrollback: 5000,
      theme: this.terminalTheme(),
    });
    this.fitAddon = new FitAddon();
    this.terminal.loadAddon(this.fitAddon);
    this.terminal.open(this.terminalHost.nativeElement);

    // Desktop copy: mouse-drag selects xterm text, but xterm's selection isn't a
    // DOM selection, so Ctrl/Cmd-C never copies it. Intercept the key here (xterm
    // sees it before the browser) and copy the selection ourselves. Returning
    // false stops xterm from processing the key. (disableStdin is on, so Ctrl-C
    // isn't an interrupt — the toolbar button handles that.)
    this.terminal.attachCustomKeyEventHandler((event) => {
      const isCopy = (event.ctrlKey || event.metaKey)
        && !event.altKey
        && (event.key === 'c' || event.key === 'C');
      if (event.type === 'keydown' && isCopy && this.terminal?.hasSelection()) {
        const selection = this.terminal.getSelection();
        if (selection) {
          void this.writeClipboard(selection, 'Copied selection');
          return false;
        }
      }
      return true;
    });

    // When the selection is cleared, catch up to the latest output we held back
    // while it was active.
    this.terminal.onSelectionChange(() => {
      if (this.terminal && !this.terminal.hasSelection() && this.pendingSelectionRender) {
        this.pendingSelectionRender = false;
        this.scheduleRenderScreen(true);
      }
    });

    // Forward every keystroke that xterm.js captures (Tab, arrow keys, regular
    // characters, Ctrl-*, etc.) up to the host. Without this, the xterm view
    // shows output but typed keys go nowhere — Tab autocomplete, arrow-key
    // history, Ctrl-R etc. all silently fail.
    //
    // xterm.js intercepts Tab when its container has focus, so the browser's
    // default "Tab moves focus" behavior is bypassed automatically.
    this.terminal.onData((data) => {
      if (this.disabled) return;
      this.rawInput.emit(data);
    });

    // Same for paste / structured key events that bubble up via onBinary.
    this.terminal.onBinary((data) => {
      if (this.disabled) return;
      this.rawInput.emit(data);
    });

    this.wheelHandler = (event) => {
      if (!this.terminal) return;
      this.cancelMomentum();
      const lines = this.wheelDeltaToLines(event.deltaY);
      const scrollingUp = event.deltaY < 0;
      this.terminal.scrollLines(event.deltaY > 0 ? lines : -lines);
      this.syncPinnedToLatestAfterUserScroll(scrollingUp);
      event.preventDefault();
    };
    this.terminalHost.nativeElement.addEventListener('wheel', this.wheelHandler, { passive: false });

    this.bindMobileOrPointerScrollHandlers();

    let resizeFitTimer: ReturnType<typeof setTimeout> | null = null;
    this.resizeObserver = new ResizeObserver(() => {
      if (resizeFitTimer) clearTimeout(resizeFitTimer);
      resizeFitTimer = setTimeout(() => {
        resizeFitTimer = null;
        this.fit();
      }, this.mobileInputMode ? 120 : 0);
    });
    this.resizeObserver.observe(this.terminalShell.nativeElement);
    this.resizeObserver.observe(this.terminalHost.nativeElement);
    this.fit();
    this.renderScreen(true);
    this.syncGrokTerminalLayout();

    this.visibilityHandler = () => {
      if (!document.hidden) {
        this.refreshTerminalLayout();
      }
    };
    document.addEventListener('visibilitychange', this.visibilityHandler);

    this.pageshowHandler = (event) => {
      if (event.persisted) {
        this.refreshTerminalLayout();
      }
    };
    window.addEventListener('pageshow', this.pageshowHandler);

    // Mobile flex layout may settle after first paint; refit once dimensions exist.
    setTimeout(() => {
      this.fit();
      this.renderScreen(true);
    }, 120);
  }

  ngOnChanges(changes: SimpleChanges): void {
    if (changes['disabled'] && this.terminal) {
      this.terminal.options.disableStdin = true;
    }

    if (changes['mobileInputMode'] && this.terminal) {
      this.terminal.options.disableStdin = true;
      this.applyTerminalPresentation();
      if (this.mobileInputMode) {
        this.userPinnedToLatest = true;
      }
      this.lastContent = '';
      this.lastRenderedPlain = '';
      this.lastWrittenClipped = '';
      queueMicrotask(() => {
        this.fit();
        this.renderScreen(true);
        this.bindMobileOrPointerScrollHandlers();
      });
    }

    if (changes['showInput']) {
      queueMicrotask(() => this.fit());
    }

    if (changes['screen'] || changes['terminalProvider']) {
      this.syncGrokTerminalLayout();
    }

    if (changes['screen']) {
      this.resetIdlePromptTimer();
      const nextScreenKey = this.screen ? `${this.screen.sessionId}:${this.screen.paneId}` : '';
      if (nextScreenKey !== this.lastScreenKey) {
        this.lastScreenKey = nextScreenKey;
        this.userPinnedToLatest = true;
        this.liveAnsiSnapshot = '';
        this.lastSeenScreenLines = 0;
        this.lastWrittenClipped = '';
        this.lastRenderedPlain = '';
        // The sent-message log is per session — clear it when the pane switches.
        this.sentMessages = [];
        this.sentLogOpen = false;
      }
      if (this.screen?.content) {
        this.updateLiveAnsiSnapshot(this.screen.content);
        this.lastSeenScreenLines = this.snapshotLineCount(this.screen.content);
      }
      // When the parent swaps to a different session (e.g. user switches plan
      // tabs), `screen` flips to null while the new screen loads. Clear xterm
      // immediately so the previous session's content doesn't stay on screen.
      if (!this.screen && this.terminal) {
        this.terminal.reset();
        this.lastContent = '';
        this.lastRenderedPlain = '';
        this.lastWrittenClipped = '';
        this.lastCursor = -1;
      } else if (this.userPinnedToLatest) {
        // While unpinned, freeze the frame so live snapshots don't wipe scrollback.
        this.scheduleRenderScreen(false);
      }
    }
  }

  /** Re-fit + repaint after the tab becomes visible again (pageshow/visibility). */
  private refreshTerminalLayout(): void {
    if (!this.terminal) return;
    this.userPinnedToLatest = true;
    this.lastWrittenClipped = '';
    this.lastRenderedPlain = '';
    queueMicrotask(() => {
      this.fit();
      this.renderScreen(true);
    });
  }

  ngOnDestroy(): void {
    if (this.visibilityHandler) {
      document.removeEventListener('visibilitychange', this.visibilityHandler);
      this.visibilityHandler = null;
    }
    if (this.pageshowHandler) {
      window.removeEventListener('pageshow', this.pageshowHandler);
      this.pageshowHandler = null;
    }
    this.resizeObserver?.disconnect();
    if (this.wheelHandler) {
      this.terminalHost.nativeElement.removeEventListener('wheel', this.wheelHandler);
    }
    this.cancelMomentum();
    this.unbindPointerScrollHandlers();
    this.unbindMobileViewportScroll();
    this.terminal?.dispose();
    this.resizeObserver = null;
    this.terminal = null;
    this.fitAddon = null;
    this.wheelHandler = null;
    if (this.idlePromptTimer) {
      clearTimeout(this.idlePromptTimer);
      this.idlePromptTimer = null;
    }
    this.clearLongPressTimer();
    if (this.copyToastTimer) {
      clearTimeout(this.copyToastTimer);
      this.copyToastTimer = null;
    }
    if (this.renderScreenTimer) {
      clearTimeout(this.renderScreenTimer);
      this.renderScreenTimer = null;
    }
  }

  protected submitTerminalInput(): void {
    if (this.disabled || this.imageSending) return;

    const input = this.mobileInput?.nativeElement.value ?? this.mobileInputBuffer;
    this.mobileInputBuffer = '';
    const images = this.imageAttachments.map((a) => a.fileName || 'image');
    this.recordSentMessage(input, images);
    if (this.imageAttachments.length > 0) {
      this.imageMessageSubmitted.emit(input);
      this.refocusMobileInput();
      return;
    }
    this.rawInput.emit(`${input}\r`);
    this.refocusMobileInput();
  }

  /** Keep the user's original submission visible in JohnnyOne (the relay sends a
   *  rewritten file-handoff pointer to the CLI, which otherwise hides the question). */
  private recordSentMessage(text: string, images: string[]): void {
    const trimmed = text.trim();
    if (!trimmed && images.length === 0) return;
    this.sentMessages = [...this.sentMessages, { id: ++this.sentMessageSeq, text: trimmed, images }];
    if (this.sentMessages.length > 50) {
      this.sentMessages = this.sentMessages.slice(-50);
    }
  }

  protected toggleSentLog(event?: Event): void {
    event?.preventDefault();
    event?.stopPropagation();
    this.sentLogOpen = !this.sentLogOpen;
  }

  protected async copySentMessage(message: { text: string }, event?: Event): Promise<void> {
    event?.preventDefault();
    event?.stopPropagation();
    await this.writeClipboard(message.text, 'Copied your message');
  }

  private async copyTerminalText(): Promise<void> {
    if (!this.terminal) return;
    const buffer = this.terminal.buffer.active;
    const start = buffer.viewportY;
    const lines: string[] = [];
    for (let i = 0; i < this.terminal.rows; i += 1) {
      lines.push(buffer.getLine(start + i)?.translateToString(true) ?? '');
    }
    const text = lines.join('\n').replace(/[ \t]+$/gm, '').replace(/\n+$/, '');
    await this.writeClipboard(text, 'Copied screen text');
  }

  private async writeClipboard(text: string, toast: string): Promise<void> {
    if (!text) return;
    // execCommand runs synchronously so it keeps the pointerup user-activation;
    // the async Clipboard API can lose activation by the time its promise settles
    // on mobile Safari. Try the sync path first, fall back to the async API.
    let ok = this.execCopyFallback(text);
    if (!ok) {
      try {
        await navigator.clipboard.writeText(text);
        ok = true;
      } catch {
        ok = false;
      }
    }
    this.showCopyToast(ok ? toast : 'Copy failed');
  }

  private execCopyFallback(text: string): boolean {
    try {
      const ta = document.createElement('textarea');
      ta.value = text;
      ta.setAttribute('readonly', '');
      ta.style.position = 'fixed';
      ta.style.top = '0';
      ta.style.left = '0';
      ta.style.opacity = '0';
      document.body.appendChild(ta);
      ta.focus();
      ta.select();
      const ok = document.execCommand('copy');
      document.body.removeChild(ta);
      return ok;
    } catch {
      return false;
    }
  }

  private showCopyToast(message: string): void {
    this.copyToast = message;
    this.changeDetector.detectChanges();
    if (this.copyToastTimer) clearTimeout(this.copyToastTimer);
    this.copyToastTimer = setTimeout(() => {
      this.copyToast = '';
      this.copyToastTimer = null;
      this.changeDetector.detectChanges();
    }, 1600);
  }

  protected sendEnterInput(): void {
    if (this.disabled || this.imageSending) return;
    const input = this.mobileInput?.nativeElement.value ?? this.mobileInputBuffer;
    if (this.imageAttachments.length > 0 || input.length > 0) {
      this.submitTerminalInput();
      return;
    }
    this.sendControlInput('\r');
  }

  protected sendControlInput(input: string): void {
    if (this.disabled) return;
    this.rawInput.emit(input);
    this.refocusMobileInput();
  }

  protected onMobileInputKeydown(event: KeyboardEvent): void {
    // Tab: don't let the browser move focus. Flush whatever's in the textarea
    // to the shell + a literal Tab so readline triggers completion. Then
    // clear the textarea — those bytes now live on the shell's line, the
    // completion (if any) will appear in the xterm pane.
    if (event.key === 'Tab' && !event.shiftKey) {
      event.preventDefault();
      if (this.disabled) return;
      const buffered = this.mobileInputBuffer ?? '';
      this.rawInput.emit(`${buffered}\t`);
      this.mobileInputBuffer = '';
      return;
    }

    if (event.key !== 'Enter' || event.shiftKey) return;

    event.preventDefault();
    this.submitTerminalInput();
  }

  protected onTerminalInputPaste(event: ClipboardEvent): void {
    if (this.disabled) return;

    const files = this.dedupeImageFiles(
      Array.from(event.clipboardData?.files ?? []).filter((file) =>
        file.type.startsWith('image/')
      )
    );
    if (files.length > 0) {
      event.preventDefault();
      // Stop the paste from also bubbling to a page-level paste handler
      // (e.g. the terminal page's onWorkspacePaste), which would add the same
      // image a second time.
      event.stopPropagation();
      this.imagePasted.emit(files);
      return;
    }

    const text = event.clipboardData?.getData('text/plain');
    if (!text) return;

    event.preventDefault();
    const textarea = event.target as HTMLTextAreaElement;
    const start = textarea.selectionStart ?? this.mobileInputBuffer.length;
    const end = textarea.selectionEnd ?? start;
    this.mobileInputBuffer = `${this.mobileInputBuffer.slice(0, start)}${text}${this.mobileInputBuffer.slice(end)}`;

    queueMicrotask(() => {
      const cursor = start + text.length;
      textarea.selectionStart = cursor;
      textarea.selectionEnd = cursor;
    });
  }

  protected onTerminalImageBrowse(event: Event): void {
    if (this.disabled) return;

    const input = event.target as HTMLInputElement;
    const files = this.dedupeImageFiles(
      Array.from(input.files ?? []).filter((file) => file.type.startsWith('image/'))
    );
    if (files.length > 0) {
      this.imagePasted.emit(files);
    }
    input.value = '';
    this.refocusMobileInput();
  }

  /**
   * Drop duplicate image files within a single paste/drop/browse. Some clipboard
   * sources place the same screenshot into the clipboard more than once, which
   * otherwise shows up as two identical attachments the user has to remove.
   */
  private dedupeImageFiles(files: File[]): File[] {
    const seen = new Set<string>();
    return files.filter((file) => {
      const key = `${file.name}|${file.size}|${file.type}|${file.lastModified}`;
      if (seen.has(key)) return false;
      seen.add(key);
      return true;
    });
  }

  private refocusMobileInput(): void {
    if (!this.mobileInputMode) return;
    queueMicrotask(() => this.mobileInput?.nativeElement.focus());
  }

  fit(): void {
    if (!this.terminal || !this.fitAddon) return;

    try {
      const colsBefore = this.terminal.cols;
      // FitAddon occasionally rounds up by one row when the host's height is
      // fractionally larger than `cols * cellHeight`. Use `proposeDimensions`
      // and apply the proposal manually so we can clamp instead of trusting
      // its built-in `fit()` rounding.
      const proposed = this.fitAddon.proposeDimensions();
      if (proposed && proposed.cols > 0 && proposed.rows > 0) {
        const safeRows = Math.max(1, proposed.rows);
        const safeCols = this.colsWithScrollbarReserve(proposed.cols);
        if (this.terminal.cols !== safeCols || this.terminal.rows !== safeRows) {
          this.terminal.resize(safeCols, safeRows);
        }
        this.applyColumnFit();
      } else {
        this.fitAddon.fit();
        this.applyColumnFit();
      }
      this.terminalResize.emit({
        cols: this.terminal.cols,
        rows: this.terminal.rows,
      });
      if (
        this.mobileInputMode
        && this.terminal.cols !== colsBefore
        && this.screen?.content
      ) {
        this.scheduleSnapshotReflow();
      }
      if (this.mobileInputMode && this.userPinnedToLatest && this.screen?.content) {
        queueMicrotask(() => {
          this.scrollToLatestOutput();
        });
      }
    } catch {
      // The terminal can be hidden during route transitions; next resize will refit.
    }
  }

  /**
   * Reserve room for the vertical scrollbar in the proposed column count.
   *
   * FitAddon only subtracts a fixed 14px for the scrollbar, which can be ~1px short
   * of the real scrollbar — pushing the last column under it. Mobile corrects this
   * after the fact with `clampColsToViewportWidth` because it *reflows* the snapshot
   * to the clamped width. Desktop has no reflow, so a post-hoc clamp would create a
   * snapshot/cols mismatch and oscillate (cols flip C↔C-1 as Grok redraws), wrapping
   * the last character of full-width lines. Instead, reserve one extra column up
   * front: a stable value that matches the width we send to tmux, so Grok draws to
   * fit and nothing wraps. (~1 char of empty space on desktop is imperceptible.)
   */
  private colsWithScrollbarReserve(proposedCols: number): number {
    if (this.mobileInputMode) return Math.max(1, proposedCols);
    return Math.max(2, proposedCols - 1);
  }

  private applyColumnFit(): void {
    if (!this.terminal || !this.mobileInputMode || this.isGrokTerminal()) return;
    this.clampColsToViewportWidth(this.terminal.cols);
  }

  /** Shrink cols until rendered rows fit inside the viewport. */
  private clampColsToViewportWidth(cols: number): number {
    if (!this.terminal) return cols;

    let nextCols = Math.max(1, cols);
    for (let attempt = 0; attempt < 16 && nextCols > 1; attempt++) {
      if (!this.terminalRowsOverflowViewport()) break;
      nextCols -= 1;
      this.terminal.resize(nextCols, this.terminal.rows);
    }
    return nextCols;
  }

  private terminalRowsOverflowViewport(): boolean {
    if (!this.terminal) return false;

    const viewport = this.terminal.element?.querySelector('.xterm-viewport') as HTMLElement | null;
    if (!viewport) return false;

    const viewportRight = viewport.getBoundingClientRect().right - (this.mobileInputMode ? 3 : 2);
    const rows = this.terminal.element?.querySelectorAll('.xterm-rows > div');
    if (rows && rows.length > 0) {
      for (const row of Array.from(rows)) {
        const rowRect = row.getBoundingClientRect();
        if (rowRect.right > viewportRight) return true;
        const spans = row.querySelectorAll('span');
        const lastSpan = spans[spans.length - 1];
        if (lastSpan && lastSpan.getBoundingClientRect().right > viewportRight) return true;
      }
      return false;
    }

    const screen = this.terminal.element?.querySelector('.xterm-screen') as HTMLElement | null;
    return !!screen && screen.clientWidth > viewport.clientWidth + 1;
  }

  private measureRenderedCellWidth(): number {
    if (!this.terminal) return 0;

    const core = (this.terminal as unknown as {
      _core?: { _renderService?: { dimensions?: { css?: { cell?: { width?: number } } } } };
    })._core;
    const fromRenderer = core?._renderService?.dimensions?.css?.cell?.width ?? 0;
    if (fromRenderer > 0) return fromRenderer;

    const row = this.terminal.element?.querySelector('.xterm-rows > div');
    if (row && row.children.length >= 2) {
      const first = row.children[0].getBoundingClientRect();
      const second = row.children[1].getBoundingClientRect();
      const width = second.left - first.left;
      if (width > 0) return width;
    }

    const canvas = document.createElement('canvas');
    const ctx = canvas.getContext('2d');
    if (!ctx) return 0;
    const fontSize = this.terminal.options.fontSize ?? 14;
    const fontFamily = this.terminal.options.fontFamily ?? 'monospace';
    ctx.font = `${fontSize}px ${fontFamily}`;
    const samples = this.mobileInputMode ? ['W', 'M', '@', 'm', '0'] : ['W'];
    let widest = 0;
    for (const sample of samples) {
      widest = Math.max(widest, ctx.measureText(sample).width || 0);
    }
    return widest;
  }

  private safeColsForReflow(): number {
    if (!this.terminal) return 80;

    const viewport = this.terminal.element?.querySelector('.xterm-viewport') as HTMLElement | null;
    const viewportWidth = viewport?.clientWidth
      ?? Math.max(1, this.terminalHost.nativeElement.clientWidth);
    const cellWidth = this.measureRenderedCellWidth();
    if (cellWidth <= 0) {
      return Math.max(1, this.terminal.cols - (this.mobileInputMode ? 6 : 4));
    }

    const effectiveCellWidth = cellWidth * (this.mobileInputMode ? 1.06 : 1.04);
    const margin = this.mobileInputMode ? 1 : 0;
    return Math.max(1, Math.floor(viewportWidth / effectiveCellWidth) - margin);
  }

  private scheduleSnapshotReflow(): void {
    if (!this.screen?.content) return;
    this.scheduleRenderScreen(true);
  }

  private scheduleRenderScreen(force: boolean): void {
    if (!this.terminal || !this.screen) return;

    const delay = this.mobileInputMode && !force ? 220 : 0;
    if (delay === 0) {
      this.renderScreen(force);
      return;
    }

    if (this.renderScreenTimer) clearTimeout(this.renderScreenTimer);
    this.renderScreenTimer = setTimeout(() => {
      this.renderScreenTimer = null;
      this.renderScreen(force);
    }, delay);
  }

  private renderScreen(force: boolean): void {
    if (!this.terminal || !this.screen) return;

    // Never repaint while the user has text selected: repainting rewrites the
    // xterm buffer, and the selection is anchored to buffer cells, so the
    // highlight would end up over the wrong text. Defer and catch up once the
    // selection clears (see onSelectionChange).
    if (this.terminal.hasSelection()) {
      this.pendingSelectionRender = true;
      return;
    }

    const nextContent = this.normalizeSnapshot(this.compactSnapshot(this.liveDisplaySnapshot()));
    const preparedContent = this.prepareSnapshotContent(nextContent);
    const preparedPlain = normalizeTerminalPlainText(preparedContent);
    // Skip redraw when tmux sends cursor-only updates, TUI chrome redraws, or duplicate frames.
    if (preparedPlain === this.lastRenderedPlain) {
      this.lastCursor = this.screen.cursor;
      return;
    }
    if (preparedContent === this.lastContent) {
      this.lastCursor = this.screen.cursor;
      return;
    }
    if (!force && this.lastCursor === this.screen.cursor && preparedContent === this.lastContent) {
      return;
    }

    this.lastCursor = this.screen.cursor;
    if (this.pendingRender) return;

    this.pendingRender = true;
    requestAnimationFrame(() => {
      this.pendingRender = false;
      if (!this.terminal || !this.screen) return;
      // A selection may have started between scheduling and this frame — don't
      // rewrite the buffer out from under it.
      if (this.terminal.hasSelection()) {
        this.pendingSelectionRender = true;
        return;
      }

      const frameContent = this.normalizeSnapshot(this.compactSnapshot(this.liveDisplaySnapshot()));
      const framePrepared = this.prepareSnapshotContent(frameContent);
      const framePlain = normalizeTerminalPlainText(framePrepared);
      if (framePlain === this.lastRenderedPlain || framePrepared === this.lastContent) return;

      this.lastContent = framePrepared;
      this.lastRenderedPlain = framePlain;
      if (framePlain.length >= normalizeTerminalPlainText(this.liveAnsiSnapshot).length) {
        this.liveAnsiSnapshot = frameContent;
      }
      this.writeSnapshot(frameContent);
      if (this.mobileInputMode) {
        queueMicrotask(() => this.bindMobileViewportScroll());
      }
    });
  }

  private writeSnapshot(content: string, reflowAttempt = 0): void {
    if (!this.terminal || this.writing || !this.screen) return;
    if (reflowAttempt > 5) return;
    // Don't rewrite the buffer while text is selected — it would move the
    // selection highlight off the text. Resume when the selection clears.
    if (this.terminal.hasSelection()) {
      this.pendingSelectionRender = true;
      return;
    }

    if (reflowAttempt === 0) {
      this.fitIfDimensionsChanged();
    } else {
      this.fit();
      this.applyColumnFit();
    }
    const displayContent = this.prepareSnapshotContent(content);
    const clipped = this.clipSnapshotToTerminalWidth(displayContent);
    const bufferEmpty = this.terminal.buffer.active.length === 0;
    const contentChanged = clipped !== this.lastWrittenClipped || bufferEmpty;
    if (!contentChanged && reflowAttempt === 0) {
      return;
    }
    const preserveReadingPosition = !this.userPinnedToLatest;
    const linesFromBottom = preserveReadingPosition
      ? this.captureViewportAnchorFromBottom()
      : 0;

    this.writing = true;
    // Repaint in place instead of clearing the whole screen first. The old
    // `\x1b[2J` blanked every cell before the new content was parsed; on the 2s
    // capture cadence xterm could paint that empty frame, producing a visible
    // blink. Here we clear only the scrollback (`\x1b[3J`, which leaves the
    // visible grid untouched), home the cursor, then overwrite each row and
    // erase its tail (`\x1b[K`). A trailing `\x1b[J` drops any rows left over
    // from a taller previous frame. No cell is ever blank between frames, so
    // steady output stops flickering while taller snapshots still scroll in.
    const repainted = clipped.split(/\r?\n/).map((line) => `${line}\x1b[K`).join('\r\n');
    this.terminal.write(`\x1b[?25l\x1b[3J\x1b[H${repainted}\x1b[0m\x1b[J\x1b[?25h`, () => {
      this.writing = false;
      if (!this.terminal || !this.screen) return;

      // Land on the latest line in the SAME frame the content was written. The
      // scroll in finishSnapshotWrite runs two rAFs later, leaving a window where
      // a refresh paints mid-content before snapping to the bottom — which reads
      // as a blink. Scrolling here pins the first painted frame to the bottom.
      if (!preserveReadingPosition) {
        this.scrollToLatestOutput();
      }

      const finalize = () => {
        if (!this.terminal || !this.screen) return;

        const colsBefore = this.terminal.cols;
        this.fitIfDimensionsChanged();
        if (
          this.mobileInputMode
          && !this.isGrokTerminal()
          && !preserveReadingPosition
          && (this.terminal.cols < colsBefore || this.terminalRowsOverflowViewport())
          && reflowAttempt < 5
        ) {
          this.writeSnapshot(content, reflowAttempt + 1);
          return;
        }

        this.lastWrittenClipped = clipped;
        this.finishSnapshotWrite(clipped, preserveReadingPosition, linesFromBottom, contentChanged);
      };

      requestAnimationFrame(() => requestAnimationFrame(finalize));
      return;
    });
  }

  private finishSnapshotWrite(
    clipped: string,
    preserveReadingPosition: boolean,
    linesFromBottom: number,
    contentChanged: boolean,
  ): void {
    if (!this.terminal || !this.screen) return;

    if (preserveReadingPosition) {
      this.restoreViewportAnchorFromBottom(linesFromBottom);
    } else if (contentChanged) {
      this.scrollToLatestOutput();
    }

    const latest = this.normalizeSnapshot(this.compactSnapshot(this.liveDisplaySnapshot()));
    const preparedLatest = this.prepareSnapshotContent(latest);
    if (preparedLatest !== this.lastContent) {
      this.scheduleRenderScreen(false);
    }
  }

  private prepareSnapshotContent(content: string): string {
    const repaired = this.repairAnsiSequences(content);
    if (this.mobileInputMode) {
      if (this.isGrokTerminal()) {
        return this.sanitizeGrokMobileSnapshot(repaired);
      }
      return this.sanitizeMobileSnapshot(repaired);
    }
    return this.sanitizeDesktopSnapshot(repaired);
  }

  private syncGrokTerminalLayout(): void {
    const next = this.isGrokTerminal();
    if (next !== this.grokTerminalLayout) {
      this.grokTerminalLayout = next;
      if (next && this.mobileInputMode && this.screen?.content) {
        this.lastWrittenClipped = '';
        this.scheduleRenderScreen(true);
      }
      return;
    }
    this.grokTerminalLayout = next;
  }

  /** Grok TUI uses 256-color foreground/background and box-drawing layout chrome. */
  private isGrokTerminal(): boolean {
    const provider = (this.terminalProvider || '').trim().toLowerCase();
    if (provider === 'grok') return true;

    const content = this.screen?.content || '';
    if (!content) return false;

    return (
      /Grok Composer/i.test(content)
      || /(?:\x1b\[[0-9;]*48;5;|\x1b\[[0-9;]*38;5;)/.test(content)
      || /[╭╰│▾⸬]/.test(content)
    );
  }

  /**
   * Grok mobile: keep ANSI colors and unicode layout, but tame harsh reds that
   * blow out on small screens (full-width red bars, saturated error text).
   */
  private sanitizeGrokMobileSnapshot(content: string): string {
    let next = this.stripAnsiBackgrounds(content)
      .replace(/\r(?!\n)/g, '\n')
      .replace(/[▀-▟█]/g, ' ')
      .replace(/[─-╿]/g, ' ')
      .replace(/[\u0000-\u0008\u000b\u000c\u000e-\u001a\u001c-\u001f\u007f]/g, '')
      .replace(/\ufffd/g, '')
      .replace(/█$/gm, ' ');

    next = this.softenGrokMobileRedForeground(next);
    next = next
      .replace(/\x1b\[1;31m/g, '\x1b[38;5;214m')
      .replace(/\x1b\[91m/g, '\x1b[38;5;214m')
      .replace(/\x1b\[31m/g, '\x1b[38;5;210m');

    return next
      .split('\n')
      .map((line) => {
        const trimmed = this.stripAnsi(line).trim();
        // Drop lines that are now only border/blank remnants of Grok's boxes.
        if (/^[-# ]+$/.test(trimmed)) return '';
        return line.replace(/[ \t]+$/g, '');
      })
      .join('\n');
  }

  /** Remap saturated 256-color reds to softer coral tones for mobile readability. */
  private softenGrokMobileRedForeground(content: string): string {
    const harshRed = new Set([1, 9, 160, 196, 202, 203]);
    const softRed = '210';
    const softBrightRed = '214';

    return content.replace(/\x1b\[([0-9;]*)m/g, (match, params: string) => {
      const codes = params.split(';').filter((part) => part !== '');
      let changed = false;

      for (let index = 0; index < codes.length; index++) {
        const code = Number(codes[index]);
        if (Number.isNaN(code)) continue;

        if ((code === 38 || code === 58) && codes[index + 1] === '5' && codes[index + 2] !== undefined) {
          const palette = Number(codes[index + 2]);
          if (harshRed.has(palette)) {
            codes[index + 2] = palette === 9 || palette === 203 ? softBrightRed : softRed;
            changed = true;
          }
          index += 2;
        }
      }

      if (!changed) return match;
      return `\x1b[${codes.join(';')}m`;
    });
  }

  /** Re-attach ESC when CSI color codes arrive without the leading byte. */
  private repairAnsiSequences(content: string): string {
    return content.replace(/(^|[^\x1b])\[([0-9;]*[ -/]*[@-~])/g, (_match, prefix: string, csi: string) => (
      `${prefix}\x1b[${csi}`
    ));
  }

  /** Desktop: keep ANSI foreground; only strip backgrounds that render as gray bars. */
  private sanitizeDesktopSnapshot(content: string): string {
    return this.stripAnsiBackgrounds(content).replace(/\r(?!\n)/g, '\n');
  }

  private stripAnsiBackgrounds(content: string): string {
    return content.replace(/\x1b\[([0-9;]*)m/g, (_match, params: string) => {
      const codes = params.split(';').filter((part) => part !== '');
      const kept: string[] = [];

      for (let index = 0; index < codes.length; index++) {
        const code = Number(codes[index]);
        if (Number.isNaN(code)) {
          kept.push(codes[index]);
          continue;
        }

        if (code === 0) {
          kept.push(codes[index]);
          continue;
        }

        if ((code >= 40 && code <= 47) || (code >= 100 && code <= 107) || code === 49) {
          continue;
        }

        if ((code === 48 || code === 58) && codes[index + 1] === '5' && codes[index + 2] !== undefined) {
          index += 2;
          continue;
        }

        if ((code === 48 || code === 58) && codes[index + 1] === '2' && codes[index + 4] !== undefined) {
          index += 4;
          continue;
        }

        kept.push(codes[index]);
      }

      if (kept.length === 0) return '';
      return `\x1b[${kept.join(';')}m`;
    });
  }

  private sanitizeMobileSnapshot(content: string): string {
    return this.stripAnsiBackgrounds(content)
      .replace(/\r(?!\n)/g, '\n')
      // Keep ESC (U+001B) — xterm needs it for CSI color sequences. Stripping it
      // leaves visible "[38;5;141m" junk instead of rendered foreground color.
      .replace(/[\u0000-\u0008\u000b\u000c\u000e-\u001a\u001c-\u001f\u007f]/g, '')
      .replace(/[\u2580-\u259f\u2588]/g, ' ')
      .replace(/[\u2500-\u257f]/g, '-')
      .replace(/[\ufffd]/g, '')
      .split('\n')
      .map((line) => {
        const trimmed = this.stripAnsi(line).trim();
        if (/^[-# ]+$/.test(trimmed)) return '';
        return line.replace(/[ \t]+$/g, '');
      })
      .join('\n');
  }

  private stripAnsi(content: string): string {
    return content
      .replace(/\x1b\[[0-?]*[ -/]*[@-~]/g, '')
      .replace(/\x1b\][^\x07]*(?:\x07|\x1b\\)/g, '')
      .replace(/\x1b[@-_][0-?]*[ -/]*[@-~]/g, '');
  }

  /** Keep the richest ANSI scrollback for mobile live view across small TUI redraws. */
  private updateLiveAnsiSnapshot(content: string): void {
    const compact = this.compactSnapshot(content);
    if (!compact) return;

    const lines = this.snapshotLineCount(compact);
    const richCaptureThreshold = Math.max((this.screen?.rows ?? 12) + 8, 28);

    // Live captures already carry recent scrollback; keep the richest one so a
    // small in-place TUI redraw doesn't momentarily shrink the mobile view.
    if (this.mobileInputMode && lines >= richCaptureThreshold) {
      this.liveAnsiSnapshot = compact;
      return;
    }

    const compactPlain = normalizeTerminalPlainText(compact);
    if (!this.liveAnsiSnapshot) {
      this.liveAnsiSnapshot = compact;
      return;
    }

    const existingPlain = normalizeTerminalPlainText(this.liveAnsiSnapshot);
    if (compactPlain.length > existingPlain.length + 8) {
      this.liveAnsiSnapshot = compact;
    }
  }

  /** Refit only when the host size would change terminal dimensions. */
  private fitIfDimensionsChanged(): void {
    if (!this.terminal || !this.fitAddon) return;

    try {
      const proposed = this.fitAddon.proposeDimensions();
      if (!proposed || proposed.cols <= 0 || proposed.rows <= 0) {
        this.fit();
        return;
      }
      const safeRows = Math.max(1, proposed.rows);
      const safeCols = this.colsWithScrollbarReserve(proposed.cols);
      if (this.terminal.cols !== safeCols || this.terminal.rows !== safeRows) {
        this.fit();
        return;
      }
      this.applyColumnFit();
    } catch {
      // The terminal can be hidden during route transitions; next resize will refit.
    }
  }

  private snapshotLineCount(content: string): number {
    if (!content) return 0;
    return content.replace(/\r\n/g, '\n').replace(/\r/g, '\n').split('\n').length;
  }

  private normalizeSnapshot(content: string): string {
    return content
      .replace(/\r\n/g, '\n')
      .replace(/\r/g, '\n')
      .replace(/\n/g, '\r\n');
  }

  private compactSnapshot(content: string): string {
    const lines = content.replace(/\r\n/g, '\n').replace(/\r/g, '\n').split('\n');
    while (lines.length > 0 && lines[lines.length - 1].trim() === '') {
      lines.pop();
    }
    return lines.join('\n');
  }

  /** On mobile, prefer cached ANSI scrollback so Grok responses stay visible live. */
  private liveDisplaySnapshot(): string {
    const viewport = this.screen?.content || '';
    if (!this.mobileInputMode || !this.screen || !this.userPinnedToLatest) {
      return viewport;
    }

    const rowBudget = Math.max((this.terminal?.rows ?? 12) * 8, 120);
    const compactViewport = this.compactSnapshot(viewport);
    const viewportPlain = normalizeTerminalPlainText(compactViewport);

    if (this.liveAnsiSnapshot) {
      const ansiPlain = normalizeTerminalPlainText(this.liveAnsiSnapshot);
      const preferCached = this.isGrokTerminal()
        ? ansiPlain.length >= viewportPlain.length
        : ansiPlain.length >= viewportPlain.length + 20;
      if (preferCached) {
        return this.tailSnapshotLines(this.liveAnsiSnapshot, rowBudget);
      }
    }

    if (this.snapshotLineCount(compactViewport) > (this.screen.rows ?? 12) + 2) {
      return this.tailSnapshotLines(compactViewport, rowBudget);
    }

    return viewport;
  }

  private tailSnapshotLines(content: string, maxLines: number): string {
    const lines = content.replace(/\r\n/g, '\n').replace(/\r/g, '\n').split('\n');
    if (lines.length <= maxLines) return lines.join('\n');
    return lines.slice(-maxLines).join('\n');
  }

  /** Reflow long tmux rows to terminal cols so xterm never soft-wraps mid-line. */
  private clipSnapshotToTerminalWidth(content: string): string {
    if (!this.terminal) return content;
    const cols = Math.max(1, this.terminal.cols);
    if (!this.mobileInputMode) {
      return content
        .replace(/\r\n/g, '\n')
        .replace(/\r/g, '\n')
        .replace(/\n/g, '\r\n');
    }
    return content
      .replace(/\r\n/g, '\n')
      .replace(/\r/g, '\n')
      .split('\n')
      .flatMap((line) => this.splitSnapshotLineToTerminalCols(line, cols))
      .join('\r\n');
  }

  private splitSnapshotLineToTerminalCols(line: string, cols: number): string[] {
    if (!line) return [''];
    if (/\x1b\[[0-?]*[ -/]*[@-~]/.test(line)) {
      if (this.isGrokTerminal()) {
        return this.splitAnsiLineToTerminalCols(line, cols);
      }
      // Non-Grok ANSI: keep line intact; column clamp + xterm clipping handle overflow.
      return [line];
    }
    return this.splitLineToDisplayCols(line, cols);
  }

  /** Wrap Grok colored rows to mobile cols while carrying open SGR state across wraps. */
  private splitAnsiLineToTerminalCols(line: string, cols: number): string[] {
    if (!line) return [''];
    const chunks: string[] = [];
    let current = '';
    let width = 0;
    let openSgr = '';
    let lastSpace = -1;       // string index in `current` of the last visible space
    let widthBeforeSpace = 0; // visible width up to (not incl.) that space

    for (let index = 0; index < line.length;) {
      if (line.charCodeAt(index) === 0x1b && line[index + 1] === '[') {
        let end = index + 2;
        while (end < line.length) {
          const code = line.charCodeAt(end);
          if (code >= 0x40 && code <= 0x7e) break;
          end += 1;
        }
        if (end < line.length) {
          const seq = line.slice(index, end + 1);
          current += seq;
          if (seq.endsWith('m')) {
            if (seq === '\x1b[0m' || seq === '\x1b[m') {
              openSgr = '';
            } else {
              openSgr += seq;
            }
          }
          index = end + 1;
          continue;
        }
      }

      const codePoint = line.codePointAt(index) ?? 0;
      const char = String.fromCodePoint(codePoint);
      const charWidth = this.terminalDisplayWidth(char);
      if (width + charWidth > cols && width > 0) {
        if (lastSpace > 0) {
          // Word wrap: break at the last space; carry SGR state onto the new line.
          chunks.push(current.slice(0, lastSpace));
          current = openSgr + current.slice(lastSpace + 1);
          width = width - widthBeforeSpace - 1;
        } else {
          chunks.push(current);
          current = openSgr;
          width = 0;
        }
        lastSpace = -1;
      }
      if (char === ' ') {
        lastSpace = current.length;
        widthBeforeSpace = width;
      }
      current += char;
      width += charWidth;
      index += codePoint > 0xffff ? 2 : 1;
    }

    if (current.length > 0) chunks.push(current);
    return chunks.length > 0 ? chunks : [''];
  }

  private splitLineToDisplayCols(line: string, cols: number): string[] {
    if (!line) return [''];
    const chunks: string[] = [];
    let current = '';
    let width = 0;
    let lastSpace = -1;       // index in `current` of the last breakable space
    let widthBeforeSpace = 0; // visible width up to (not incl.) that space
    for (const char of line) {
      const charWidth = this.terminalDisplayWidth(char);
      if (width + charWidth > cols && current.length > 0) {
        if (lastSpace > 0) {
          // Word wrap: break at the last space instead of mid-word.
          chunks.push(current.slice(0, lastSpace));
          current = current.slice(lastSpace + 1);
          width = width - widthBeforeSpace - 1;
        } else {
          chunks.push(current);
          current = '';
          width = 0;
        }
        lastSpace = -1;
      }
      if (char === ' ') {
        lastSpace = current.length;
        widthBeforeSpace = width;
      }
      current += char;
      width += charWidth;
    }
    if (current.length > 0) chunks.push(current);
    return chunks.length > 0 ? chunks : [''];
  }

  private terminalDisplayWidth(char: string): number {
    const code = char.codePointAt(0) ?? 0;
    if (code <= 0x1f || code === 0x7f) return 0;
    if (
      (code >= 0x1100 && code <= 0x115f)
      || (code >= 0x2e80 && code <= 0xa4cf)
      || (code >= 0xac00 && code <= 0xd7af)
      || (code >= 0xf900 && code <= 0xfaff)
      || (code >= 0xfe10 && code <= 0xfe6f)
      || (code >= 0xff00 && code <= 0xff60)
      || (code >= 0xffe0 && code <= 0xffe6)
      || (code >= 0x1f300 && code <= 0x1faff)
    ) {
      return 2;
    }
    return 1;
  }

  private captureViewportAnchorFromBottom(): number {
    if (!this.terminal) return 0;
    const buffer = this.terminal.buffer.active;
    return Math.max(0, buffer.length - buffer.viewportY - this.terminal.rows);
  }

  private restoreViewportAnchorFromBottom(linesFromBottom: number): void {
    if (!this.terminal) return;
    const buffer = this.terminal.buffer.active;
    const targetTop = Math.max(0, buffer.length - this.terminal.rows - linesFromBottom);
    this.terminal.scrollToLine(targetTop);
  }

  private resetIdlePromptTimer(): void {
    if (this.idlePromptTimer) clearTimeout(this.idlePromptTimer);
  }

  private terminalFontSize(): number {
    return this.mobileInputMode ? 11 : 14;
  }

  private terminalTheme(): {
    background: string;
    foreground: string;
    cursor: string;
    selectionBackground: string;
    red?: string;
    brightRed?: string;
  } {
    const base = {
      background: '#05070b',
      foreground: '#d7e1ee',
      cursor: '#7dd3fc',
      selectionBackground: '#1e3a5f',
    };
    if (!this.mobileInputMode) return base;
    return {
      ...base,
      red: '#fb7185',
      brightRed: '#fda4af',
    };
  }

  private applyTerminalPresentation(): void {
    if (!this.terminal) return;
    this.terminal.options.fontSize = this.terminalFontSize();
    this.terminal.options.lineHeight = this.mobileInputMode ? 1.1 : 1.2;
    this.terminal.options.minimumContrastRatio = this.mobileInputMode ? 4.5 : 1;
    this.terminal.options.theme = this.terminalTheme();
  }

  private bindMobileOrPointerScrollHandlers(): void {
    if (this.mobileInputMode) {
      this.bindPointerScrollHandlers(true);
      this.bindMobileViewportScroll();
      return;
    }
    this.unbindMobileViewportScroll();
    this.bindPointerScrollHandlers();
  }

  private bindMobileViewportScroll(): void {
    if (!this.terminal || !this.mobileInputMode) return;

    const viewport = this.terminal.element?.querySelector('.xterm-viewport') as HTMLElement | null;
    if (!viewport) return;

    this.unbindMobileViewportScroll();
    this.lastMobileScrollTop = viewport.scrollTop;
    this.xtermViewportScrollHandler = () => {
      if (!this.terminal) return;
      queueMicrotask(() => {
        if (!this.terminal) return;
        const top = viewport.scrollTop;
        // A real upward gesture (top decreased meaningfully) means the user wants to
        // read scrollback — only then freeze follow-live. Otherwise (scrolled toward
        // the bottom, a programmatic scroll, or simply can't reach the exact bottom on
        // a phone) keep following live, re-pinning once near the bottom.
        const scrolledUp = top < this.lastMobileScrollTop - 4;
        this.lastMobileScrollTop = top;
        if (this.isViewportAtBottom()) {
          this.userPinnedToLatest = true;
        } else if (scrolledUp) {
          this.userPinnedToLatest = false;
        }
      });
    };
    viewport.addEventListener('scroll', this.xtermViewportScrollHandler, { passive: true });
  }

  private unbindMobileViewportScroll(): void {
    if (!this.terminal || !this.xtermViewportScrollHandler) {
      this.xtermViewportScrollHandler = null;
      return;
    }

    const viewport = this.terminal.element?.querySelector('.xterm-viewport') as HTMLElement | null;
    if (viewport) {
      viewport.removeEventListener('scroll', this.xtermViewportScrollHandler);
    }
    this.xtermViewportScrollHandler = null;
  }

  private bindPointerScrollHandlers(allowMobile = false): void {
    if (this.mobileInputMode && !allowMobile) return;

    this.unbindPointerScrollHandlers();
    const host = this.terminalHost.nativeElement;
    this.pointerScrollDownHandler = (event) => this.onTerminalPointerDown(event);
    this.pointerScrollMoveHandler = (event) => this.onTerminalPointerMove(event);
    this.pointerScrollUpHandler = (event) => this.onTerminalPointerUp(event);
    host.addEventListener('pointerdown', this.pointerScrollDownHandler);
    host.addEventListener('pointermove', this.pointerScrollMoveHandler);
    host.addEventListener('pointerup', this.pointerScrollUpHandler);
    host.addEventListener('pointercancel', this.pointerScrollUpHandler);
  }

  private unbindPointerScrollHandlers(): void {
    const host = this.terminalHost.nativeElement;
    if (this.pointerScrollDownHandler) {
      host.removeEventListener('pointerdown', this.pointerScrollDownHandler);
    }
    if (this.pointerScrollMoveHandler) {
      host.removeEventListener('pointermove', this.pointerScrollMoveHandler);
    }
    if (this.pointerScrollUpHandler) {
      host.removeEventListener('pointerup', this.pointerScrollUpHandler);
      host.removeEventListener('pointercancel', this.pointerScrollUpHandler);
    }
    this.pointerScrollDownHandler = null;
    this.pointerScrollMoveHandler = null;
    this.pointerScrollUpHandler = null;
    this.resetPointerScroll();
  }

  private onTerminalPointerDown(event: PointerEvent): void {
    if (!this.mobileInputMode || !this.terminal) return;
    if (event.pointerType === 'mouse' && event.button !== 0) return;

    this.cancelMomentum();
    this.resetPointerScroll();
    this.pointerScroll = {
      tracking: true,
      scrolling: false,
      pointerId: event.pointerId,
      lastY: event.clientY,
      accum: 0,
      lastT: this.now(),
      velocity: 0,
    };
    try {
      this.terminalHost.nativeElement.setPointerCapture(event.pointerId);
    } catch {
      // Pointer capture can fail on some mobile browsers for touch.
    }
    this.mobileInput?.nativeElement.blur();

    // Long-press = "grab the text": a finger held still (no scroll) copies the
    // visible screen to the clipboard. A quick drag scrolls instead, so the two
    // gestures don't collide. (Native touch selection is blocked by our scroll
    // hijack + touch-action:none, so this is the reliable way to copy on mobile.)
    // The timer only *flags* the long-press; the clipboard write happens on
    // pointerup, because clipboard access requires live user activation that a
    // setTimeout callback no longer has (esp. iOS Safari).
    this.longPressFired = false;
    this.clearLongPressTimer();
    this.longPressTimer = setTimeout(() => {
      this.longPressTimer = null;
      if (this.pointerScroll.scrolling) return;
      this.longPressFired = true;
      this.showCopyToast('Release to copy');
    }, 450);
  }

  private onTerminalPointerMove(event: PointerEvent): void {
    if (!this.pointerScroll.tracking || event.pointerId !== this.pointerScroll.pointerId || !this.terminal) {
      return;
    }

    const deltaY = event.clientY - this.pointerScroll.lastY;
    if (!this.pointerScroll.scrolling) {
      if (Math.abs(deltaY) < this.pointerScrollThresholdPx) return;
      this.pointerScroll.scrolling = true;
      this.clearLongPressTimer(); // movement ⇒ scroll, not a long-press copy
      this.longPressFired = false;
      this.terminalHost.nativeElement.classList.add('terminal-host-scrolling');
    }

    // Direct-manipulation: dragging the finger DOWN pulls the content down so
    // earlier output comes into view (scroll up) — the inverse of a mouse wheel.
    // Track 1:1 against the cell height, carrying the fractional remainder so the
    // content follows the finger smoothly instead of snapping a whole row at a time.
    const cell = this.cellHeightPx();
    this.pointerScroll.accum += deltaY / cell;
    const lines = Math.trunc(this.pointerScroll.accum);
    if (lines !== 0) {
      this.pointerScroll.accum -= lines;
      this.terminal.scrollLines(-lines);
      // Dragging DOWN (deltaY > 0) scrolls toward older output — that's "scrolled
      // up", which unpins follow-live. Dragging UP heads back toward the latest,
      // which re-pins once the viewport reaches the bottom. (Was inverted, which
      // left the view unpinned after a scroll so live output stopped refreshing.)
      this.syncPinnedToLatestAfterUserScroll(deltaY > 0);
    }

    const now = this.now();
    const dt = now - this.pointerScroll.lastT;
    if (dt > 0) {
      // Exponentially smooth velocity (lines/ms) so a flick reads cleanly.
      const instant = (deltaY / cell) / dt;
      this.pointerScroll.velocity = this.pointerScroll.velocity * 0.7 + instant * 0.3;
    }
    this.pointerScroll.lastY = event.clientY;
    this.pointerScroll.lastT = now;
    event.preventDefault();
  }

  private onTerminalPointerUp(event: PointerEvent): void {
    if (!this.pointerScroll.tracking || event.pointerId !== this.pointerScroll.pointerId) return;

    try {
      this.terminalHost.nativeElement.releasePointerCapture(event.pointerId);
    } catch {
      // Ignore release failures after implicit pointer cancel.
    }

    this.clearLongPressTimer();
    const longPress = this.longPressFired && event.type !== 'pointercancel';
    this.longPressFired = false;
    const flung = this.pointerScroll.scrolling && event.type !== 'pointercancel' && !longPress;
    const velocity = this.pointerScroll.velocity;
    this.resetPointerScroll();
    // Copy here (not in the timer): pointerup still carries user activation,
    // which the clipboard API requires.
    if (longPress) {
      void this.copyTerminalText();
      return;
    }
    // Carry a flick into inertial scrolling for a native feel.
    if (flung && Math.abs(velocity) > 0.01) {
      this.startMomentum(velocity);
    }
  }

  private clearLongPressTimer(): void {
    if (this.longPressTimer) {
      clearTimeout(this.longPressTimer);
      this.longPressTimer = null;
    }
  }

  private resetPointerScroll(): void {
    this.terminalHost?.nativeElement.classList.remove('terminal-host-scrolling');
    this.pointerScroll = {
      tracking: false,
      scrolling: false,
      pointerId: -1,
      lastY: 0,
      accum: 0,
      lastT: 0,
      velocity: 0,
    };
  }

  /** Pixel height of one terminal row, used to map finger travel to line scroll. */
  private cellHeightPx(): number {
    const fontSize = this.terminal?.options.fontSize ?? 11;
    const lineHeight = this.terminal?.options.lineHeight ?? 1.1;
    return Math.max(10, fontSize * lineHeight);
  }

  private now(): number {
    return typeof performance !== 'undefined' ? performance.now() : Date.now();
  }

  /** Inertial scroll after a flick: decay the release velocity frame by frame. */
  private startMomentum(velocity: number): void {
    this.cancelMomentum();
    if (!this.terminal) return;

    let v = velocity;
    let last = this.now();
    this.momentumAccum = 0;

    const step = (): void => {
      this.momentumRaf = null;
      if (!this.terminal || this.pointerScroll.tracking) return;

      const now = this.now();
      const dt = Math.min(48, now - last);
      last = now;

      this.momentumAccum += v * dt;
      const lines = Math.trunc(this.momentumAccum);
      if (lines !== 0) {
        this.momentumAccum -= lines;
        this.terminal.scrollLines(-lines);
        // Same sign convention as the drag: v > 0 = toward older = unpin.
        this.syncPinnedToLatestAfterUserScroll(v > 0);
      }

      // ~0.94 per 16ms frame; framerate-independent decay.
      v *= Math.pow(0.94, dt / 16);
      if (Math.abs(v) < 0.0006 || this.isMomentumAtEdge(v)) {
        this.cancelMomentum();
        return;
      }
      this.momentumRaf = requestAnimationFrame(step);
    };
    this.momentumRaf = requestAnimationFrame(step);
  }

  private isMomentumAtEdge(velocity: number): boolean {
    if (!this.terminal) return true;
    const buffer = this.terminal.buffer.active;
    // velocity > 0 means dragging down → scrolling up → stop at the top.
    if (velocity > 0) return buffer.viewportY <= 0;
    return this.isViewportAtBottom();
  }

  private cancelMomentum(): void {
    if (this.momentumRaf !== null) {
      cancelAnimationFrame(this.momentumRaf);
      this.momentumRaf = null;
    }
    this.momentumAccum = 0;
  }

  private wheelDeltaToLines(deltaY: number): number {
    return Math.max(1, Math.ceil(Math.abs(deltaY) / 40));
  }

  private isViewportAtBottom(): boolean {
    if (!this.terminal) return true;
    const buffer = this.terminal.buffer.active;
    // Forgiving threshold: with no "Live" button to resume, scrolling back to
    // *near* the bottom must re-pin follow-live, otherwise the view could get
    // stuck paused a couple of rows short of the end.
    const slackRows = 2;
    return buffer.viewportY + buffer.baseY >= buffer.length - this.terminal.rows - slackRows;
  }

  private syncPinnedToLatestAfterUserScroll(scrolledUp: boolean): void {
    if (scrolledUp) {
      this.userPinnedToLatest = false;
      return;
    }
    queueMicrotask(() => {
      if (this.isViewportAtBottom()) {
        this.userPinnedToLatest = true;
      }
    });
  }

  /** Keep the latest output in view after each snapshot refresh. */
  private scrollToLatestOutput(): void {
    if (!this.terminal || !this.userPinnedToLatest) return;
    // Always scrollToBottom — it lands on the true last buffer row regardless of
    // soft-wrapping. The old desktop path computed scrollToLine(logicalLines -
    // rows), but xterm wraps long lines into extra buffer rows, so that math
    // undershot the bottom and parked the view mid-content on refresh.
    this.terminal.scrollToBottom();
  }
}
