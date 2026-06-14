import {
  AfterViewInit,
  Component,
  ElementRef,
  EventEmitter,
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
import mermaid from 'mermaid';

mermaid.initialize({ startOnLoad: false, securityLevel: 'strict', theme: 'dark' });

interface TerminalMermaidBlock {
  id: string;
  label: string;
  source: string;
}

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
  private static readonly HISTORY_CHUNK_ROWS = 200;

  @Input() screen: TerminalScreen | null = null;
  @Input() disabled = false;
  @Input() mobileInputMode = false;
  @Input() showInput = true;
  @Input() title = 'Terminal';
  @Input() imageAttachments: TerminalImageAttachmentPreview[] = [];
  @Input() imageSending = false;

  @Output() rawInput = new EventEmitter<string>();
  @Output() imagePasted = new EventEmitter<File[]>();
  @Output() imageRemoved = new EventEmitter<string>();
  @Output() imageMessageSubmitted = new EventEmitter<string>();
  @Output() terminalResize = new EventEmitter<{ cols: number; rows: number }>();
  @Output() historyRequested = new EventEmitter<number>();
  @Output() mermaidRequested = new EventEmitter<string>();

  @ViewChild('terminalShell', { static: true }) private terminalShell!: ElementRef<HTMLElement>;
  @ViewChild('terminalHost', { static: true }) private terminalHost!: ElementRef<HTMLDivElement>;
  @ViewChild('mobileInput') private mobileInput?: ElementRef<HTMLTextAreaElement>;

  private terminal: Terminal | null = null;
  private fitAddon: FitAddon | null = null;
  private resizeObserver: ResizeObserver | null = null;
  private lastCursor: number | null = null;
  private lastContent = '';
  private pendingRender = false;
  private writing = false;
  private wheelHandler: ((event: WheelEvent) => void) | null = null;
  private pointerScrollDownHandler: ((event: PointerEvent) => void) | null = null;
  private pointerScrollMoveHandler: ((event: PointerEvent) => void) | null = null;
  private pointerScrollUpHandler: ((event: PointerEvent) => void) | null = null;
  private pointerScroll = {
    tracking: false,
    scrolling: false,
    pointerId: -1,
    lastY: 0,
  };
  private readonly pointerScrollThresholdPx = 6;
  /** When false, mobile mode keeps the user's scroll position instead of following new output. */
  private userPinnedToLatest = true;
  private idlePromptTimer: ReturnType<typeof setTimeout> | null = null;
  private requestedHistoryRows = 0;
  private historyRequestInFlight = false;
  private historyRequestTimer: ReturnType<typeof setTimeout> | null = null;
  private historyRefreshTimer: ReturnType<typeof setTimeout> | null = null;
  private revealHistoryOnNextRender = false;
  private lastScreenKey = '';
  protected mermaidBlocks: TerminalMermaidBlock[] = [];
  protected mermaidRenderError = '';
  protected mobileInputBuffer = '';

  ngAfterViewInit(): void {
    this.terminal = new Terminal({
      cursorBlink: true,
      convertEol: true,
      disableStdin: true,
      fontFamily: 'Menlo, Monaco, Consolas, "Liberation Mono", "Courier New", monospace',
      fontSize: this.terminalFontSize(),
      lineHeight: this.mobileInputMode ? 1.1 : 1.2,
      scrollback: 5000,
      theme: {
        background: '#05070b',
        foreground: '#d7e1ee',
        cursor: '#7dd3fc',
        selectionBackground: '#1e3a5f',
      },
    });
    this.fitAddon = new FitAddon();
    this.terminal.loadAddon(this.fitAddon);
    this.terminal.open(this.terminalHost.nativeElement);

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
      const lines = this.wheelDeltaToLines(event.deltaY);
      const scrollingUp = event.deltaY < 0;
      this.terminal.scrollLines(event.deltaY > 0 ? lines : -lines);
      this.syncPinnedToLatestAfterUserScroll(scrollingUp);
      event.preventDefault();
    };
    this.terminalHost.nativeElement.addEventListener('wheel', this.wheelHandler, { passive: false });
    this.bindPointerScrollHandlers();

    this.resizeObserver = new ResizeObserver(() => this.fit());
    this.resizeObserver.observe(this.terminalShell.nativeElement);
    this.resizeObserver.observe(this.terminalHost.nativeElement);
    this.fit();
    this.renderScreen(true);
  }

  ngOnChanges(changes: SimpleChanges): void {
    if (changes['disabled'] && this.terminal) {
      this.terminal.options.disableStdin = true;
    }

    if (changes['mobileInputMode'] && this.terminal) {
      this.terminal.options.disableStdin = true;
      this.terminal.options.fontSize = this.terminalFontSize();
      this.terminal.options.lineHeight = this.mobileInputMode ? 1.1 : 1.2;
      if (this.mobileInputMode) {
        this.userPinnedToLatest = true;
      }
      this.lastContent = '';
      queueMicrotask(() => {
        this.fit();
        this.renderScreen(true);
      });
    }

    if (changes['showInput']) {
      queueMicrotask(() => this.fit());
    }

    if (changes['screen']) {
      this.resetIdlePromptTimer();
      const nextScreenKey = this.screen ? `${this.screen.sessionId}:${this.screen.paneId}` : '';
      if (nextScreenKey !== this.lastScreenKey) {
        this.lastScreenKey = nextScreenKey;
        this.requestedHistoryRows = 0;
        this.historyRequestInFlight = false;
        this.revealHistoryOnNextRender = false;
        this.userPinnedToLatest = true;
      }
      // When the parent swaps to a different session (e.g. user switches plan
      // tabs), `screen` flips to null while the new screen loads. Clear xterm
      // immediately so the previous session's content doesn't stay on screen.
      if (!this.screen && this.terminal) {
        this.terminal.reset();
        this.lastContent = '';
        this.lastCursor = -1;
        this.mermaidBlocks = [];
        this.mermaidRenderError = '';
      } else if (this.userPinnedToLatest || this.revealHistoryOnNextRender) {
        // While unpinned, freeze the frame so live snapshots don't wipe scrollback.
        this.renderScreen(true);
      }
    }
  }

  protected isFollowingLive(): boolean {
    return this.userPinnedToLatest;
  }

  protected canLoadHistory(): boolean {
    return !!this.screen
      && this.screen.status === 'attached'
      && this.availableHistoryRows() > 0
      && this.loadedHistoryRows() < this.availableHistoryRows()
      && !this.historyRequestInFlight;
  }

  protected loadedHistoryRows(): number {
    return Math.min(this.requestedHistoryRows, this.availableHistoryRows());
  }

  protected availableHistoryRows(): number {
    return Math.max(0, this.screen?.historyLines ?? 0);
  }

  protected historyProgressText(): string {
    const available = this.availableHistoryRows();
    if (available <= 0) return 'no history';
    return `${this.loadedHistoryRows()} / ${available}`;
  }

  protected loadPreviousHistory(event?: Event): void {
    event?.preventDefault();
    event?.stopPropagation();
    this.userPinnedToLatest = false;
    this.requestHistoryRows(this.loadedHistoryRows() + TerminalScreenComponent.HISTORY_CHUNK_ROWS);
  }

  protected scrollViewportUp(event?: Event): void {
    this.userPinnedToLatest = false;
    this.scrollViewportBy(-this.viewportScrollStep(), event);
  }

  protected scrollViewportDown(event?: Event): void {
    this.scrollViewportBy(this.viewportScrollStep(), event);
    this.syncPinnedToLatestAfterUserScroll(false);
  }

  protected scrollViewportToLatest(event?: Event): void {
    event?.preventDefault();
    event?.stopPropagation();
    this.userPinnedToLatest = true;
    this.renderScreen(true);
    this.scrollToLatestOutput(this.compactSnapshot(this.screen?.content || ''));
  }

  ngOnDestroy(): void {
    this.resizeObserver?.disconnect();
    if (this.wheelHandler) {
      this.terminalHost.nativeElement.removeEventListener('wheel', this.wheelHandler);
    }
    this.unbindPointerScrollHandlers();
    this.terminal?.dispose();
    this.resizeObserver = null;
    this.terminal = null;
    this.fitAddon = null;
    this.wheelHandler = null;
    if (this.idlePromptTimer) {
      clearTimeout(this.idlePromptTimer);
      this.idlePromptTimer = null;
    }
    this.clearHistoryRequestTimer();
    this.clearHistoryRefreshTimer();
  }

  protected submitTerminalInput(): void {
    if (this.disabled || this.imageSending) return;

    const input = this.mobileInput?.nativeElement.value ?? this.mobileInputBuffer;
    this.mobileInputBuffer = '';
    if (this.imageAttachments.length > 0) {
      this.imageMessageSubmitted.emit(input);
      this.refocusMobileInput();
      return;
    }
    this.rawInput.emit(`${input}\r`);
    this.refocusMobileInput();
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

    const files = Array.from(event.clipboardData?.files ?? []).filter((file) =>
      file.type.startsWith('image/')
    );
    if (files.length > 0) {
      event.preventDefault();
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
    const files = Array.from(input.files ?? []).filter((file) => file.type.startsWith('image/'));
    if (files.length > 0) {
      this.imagePasted.emit(files);
    }
    input.value = '';
    this.refocusMobileInput();
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
        const safeCols = Math.max(1, proposed.cols);
        if (this.terminal.cols !== safeCols || this.terminal.rows !== safeRows) {
          this.terminal.resize(safeCols, safeRows);
        }
        this.applyColumnFitIfMobile();
      } else {
        this.fitAddon.fit();
        this.applyColumnFitIfMobile();
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
          this.scrollToLatestOutput(this.compactSnapshot(this.screen?.content || ''));
        });
      }
    } catch {
      // The terminal can be hidden during route transitions; next resize will refit.
    }
  }

  private applyColumnFitIfMobile(): void {
    if (!this.terminal || !this.mobileInputMode) return;
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
    this.lastContent = '';
    queueMicrotask(() => this.renderScreen(true));
  }

  private renderScreen(force: boolean): void {
    if (!this.terminal || !this.screen) return;
    if (!force && this.lastCursor === this.screen.cursor) return;

    this.lastCursor = this.screen.cursor;
    if (this.pendingRender) return;

    this.pendingRender = true;
    requestAnimationFrame(() => {
      this.pendingRender = false;
      if (!this.terminal || !this.screen) return;

      const nextContent = this.normalizeSnapshot(this.compactSnapshot(this.screen.content || ''));
      if (this.shouldPreserveLoadedHistory(nextContent)) {
        this.refreshLoadedHistory();
        return;
      }
      const preparedContent = this.prepareSnapshotContent(nextContent);
      if (!force && preparedContent === this.lastContent) return;

      this.lastContent = preparedContent;
      this.updateMermaidBlocks(nextContent);
      this.historyRequestInFlight = false;
      this.clearHistoryRequestTimer();
      this.writeSnapshot(nextContent);
    });
  }

  private requestHistoryRows(rows: number): void {
    if (!this.screen || this.historyRequestInFlight) return;

    const nextRows = Math.min(this.availableHistoryRows(), Math.max(1, Math.floor(rows)));
    if (nextRows <= this.loadedHistoryRows()) return;

    this.requestedHistoryRows = nextRows;
    this.historyRequestInFlight = true;
    this.revealHistoryOnNextRender = true;
    this.clearHistoryRequestTimer();
    this.historyRequestTimer = setTimeout(() => {
      this.historyRequestInFlight = false;
      this.historyRequestTimer = null;
    }, 4_000);
    this.historyRequested.emit(nextRows);
  }

  private refreshLoadedHistory(): void {
    if (!this.screen || this.historyRequestInFlight || this.historyRefreshTimer) return;
    const rows = this.loadedHistoryRows();
    if (rows <= 0) return;

    this.historyRefreshTimer = setTimeout(() => {
      this.historyRefreshTimer = null;
      if (!this.screen || this.historyRequestInFlight) return;
      this.historyRequestInFlight = true;
      this.revealHistoryOnNextRender = false;
      this.clearHistoryRequestTimer();
      this.historyRequestTimer = setTimeout(() => {
        this.historyRequestInFlight = false;
        this.historyRequestTimer = null;
      }, 4_000);
      this.historyRequested.emit(rows);
    }, 250);
  }

  private clearHistoryRequestTimer(): void {
    if (!this.historyRequestTimer) return;
    clearTimeout(this.historyRequestTimer);
    this.historyRequestTimer = null;
  }

  private clearHistoryRefreshTimer(): void {
    if (!this.historyRefreshTimer) return;
    clearTimeout(this.historyRefreshTimer);
    this.historyRefreshTimer = null;
  }

  private writeSnapshot(content: string, reflowAttempt = 0): void {
    if (!this.terminal || this.writing || !this.screen) return;
    if (reflowAttempt > 5) return;

    this.fit();
    this.applyColumnFitIfMobile();
    const displayContent = this.prepareSnapshotContent(content);
    const clipped = this.clipSnapshotToTerminalWidth(displayContent);
    const preserveReadingPosition = !this.userPinnedToLatest;
    const linesFromBottom = preserveReadingPosition
      ? this.captureViewportAnchorFromBottom()
      : 0;

    this.writing = true;
    this.terminal.write(`\x1b[?25l\x1b[3J\x1b[H\x1b[2J${clipped}\x1b[?25h`, () => {
      this.writing = false;
      if (!this.terminal || !this.screen) return;

      const finalize = () => {
        if (!this.terminal || !this.screen) return;

        const colsBefore = this.terminal.cols;
        this.fit();
        this.applyColumnFitIfMobile();
        if (
          this.mobileInputMode
          && !preserveReadingPosition
          && (this.terminal.cols < colsBefore || this.terminalRowsOverflowViewport())
          && reflowAttempt < 5
        ) {
          this.writeSnapshot(content, reflowAttempt + 1);
          return;
        }

        this.finishSnapshotWrite(clipped, preserveReadingPosition, linesFromBottom, content);
      };

      requestAnimationFrame(() => requestAnimationFrame(finalize));
      return;
    });
  }

  private finishSnapshotWrite(
    clipped: string,
    preserveReadingPosition: boolean,
    linesFromBottom: number,
    sourceContent: string,
  ): void {
    if (!this.terminal || !this.screen) return;

    if (this.revealHistoryOnNextRender) {

      this.revealHistoryOnNextRender = false;
      this.terminal.scrollToTop();
    } else if (preserveReadingPosition) {
      this.restoreViewportAnchorFromBottom(linesFromBottom);
    } else {
      this.scrollToLatestOutput(clipped);
    }

    const latest = this.normalizeSnapshot(this.compactSnapshot(this.screen.content || ''));
    if (this.shouldPreserveLoadedHistory(latest)) {
      this.refreshLoadedHistory();
      return;
    }
    const preparedLatest = this.prepareSnapshotContent(latest);
    if (preparedLatest !== this.lastContent) {
      this.lastContent = preparedLatest;
      this.updateMermaidBlocks(latest);
      this.writeSnapshot(sourceContent);
    }
  }

  private prepareSnapshotContent(content: string): string {
    const repaired = this.repairAnsiSequences(content);
    if (this.mobileInputMode) {
      return this.sanitizeMobileSnapshot(repaired);
    }
    return this.sanitizeDesktopSnapshot(repaired);
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

  protected async openMermaidBlock(block: TerminalMermaidBlock, event?: Event): Promise<void> {
    event?.preventDefault();
    event?.stopPropagation();
    this.mermaidRenderError = '';
    try {
      const id = `terminal-mermaid-${block.id}-${Math.random().toString(36).slice(2, 10)}`;
      const rendered = await mermaid.render(id, block.source);
      this.mermaidRequested.emit(rendered.svg);
    } catch (err) {
      this.mermaidRenderError = `Mermaid render failed: ${String(err)}`;
    }
  }

  private updateMermaidBlocks(content: string): void {
    const plain = this.stripAnsi(content).replace(/\r\n/g, '\n').replace(/\r/g, '\n');
    const blocks = this.extractMermaidBlocks(plain);
    this.mermaidBlocks = blocks.map((source, index) => ({
      id: `${index + 1}`,
      label: blocks.length === 1 ? 'Mermaid' : `Mermaid ${index + 1}`,
      source,
    }));
    if (this.mermaidBlocks.length === 0) {
      this.mermaidRenderError = '';
    }
  }

  private extractMermaidBlocks(content: string): string[] {
    const blocks: string[] = [];
    const fencePattern = /(?:^|\n)[ \t]*(`{3,}|~{3,})[ \t]*mermaid[^\n]*\n([\s\S]*?)(?=\n[ \t]*\1[ \t]*(?:\n|$)|$)/gi;
    let match: RegExpExecArray | null;
    while ((match = fencePattern.exec(content)) !== null) {
      const source = match[2]?.trim();
      if (source) blocks.push(source);
      if (blocks.length >= 8) break;
    }
    return blocks;
  }

  private stripAnsi(content: string): string {
    return content
      .replace(/\x1b\[[0-?]*[ -/]*[@-~]/g, '')
      .replace(/\x1b\][^\x07]*(?:\x07|\x1b\\)/g, '')
      .replace(/\x1b[@-_][0-?]*[ -/]*[@-~]/g, '');
  }

  private shouldPreserveLoadedHistory(nextContent: string): boolean {
    if (!this.screen || this.requestedHistoryRows <= 0 || !this.lastContent) return false;
    if (!this.isExpandedHistorySnapshot(this.lastContent)) return false;
    return !this.isExpandedHistorySnapshot(nextContent);
  }

  private isExpandedHistorySnapshot(content: string): boolean {
    if (!this.screen) return false;
    return this.snapshotLineCount(content) > Math.max(this.screen.rows + 2, this.screen.rows + Math.min(5, this.loadedHistoryRows()));
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
    // Reflowing ANSI lines drops inline SGR codes (e.g. blue "Deployed"). Keep the
    // line intact and rely on column clamp + xterm clipping instead.
    if (/\x1b\[[0-?]*[ -/]*[@-~]/.test(line)) {
      return [line];
    }
    return this.splitLineToDisplayCols(line, cols);
  }

  private splitLineToDisplayCols(line: string, cols: number): string[] {
    if (!line) return [''];
    const chunks: string[] = [];
    let current = '';
    let width = 0;
    for (const char of line) {
      const charWidth = this.terminalDisplayWidth(char);
      if (width + charWidth > cols && current.length > 0) {
        chunks.push(current);
        current = char;
        width = charWidth;
        continue;
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
    return this.mobileInputMode ? 9 : 14;
  }

  private bindPointerScrollHandlers(): void {
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

    this.resetPointerScroll();
    this.pointerScroll = {
      tracking: true,
      scrolling: false,
      pointerId: event.pointerId,
      lastY: event.clientY,
    };
    try {
      this.terminalHost.nativeElement.setPointerCapture(event.pointerId);
    } catch {
      // Pointer capture can fail on some mobile browsers for touch.
    }
    this.mobileInput?.nativeElement.blur();
  }

  private onTerminalPointerMove(event: PointerEvent): void {
    if (!this.pointerScroll.tracking || event.pointerId !== this.pointerScroll.pointerId || !this.terminal) {
      return;
    }

    const deltaY = event.clientY - this.pointerScroll.lastY;
    if (!this.pointerScroll.scrolling) {
      if (Math.abs(deltaY) < this.pointerScrollThresholdPx) return;
      this.pointerScroll.scrolling = true;
      this.terminalHost.nativeElement.classList.add('terminal-host-scrolling');
    }

    const lines = this.touchDeltaToLines(deltaY);
    if (lines !== 0) {
      // Match the wheel handler: drag down reveals earlier output; drag up reveals newer.
      const towardNewer = deltaY < 0;
      this.terminal.scrollLines(deltaY > 0 ? lines : -lines);
      this.syncPinnedToLatestAfterUserScroll(towardNewer);
      this.pointerScroll.lastY = event.clientY;
      event.preventDefault();
    }
  }

  private onTerminalPointerUp(event: PointerEvent): void {
    if (!this.pointerScroll.tracking || event.pointerId !== this.pointerScroll.pointerId) return;

    try {
      this.terminalHost.nativeElement.releasePointerCapture(event.pointerId);
    } catch {
      // Ignore release failures after implicit pointer cancel.
    }
    this.resetPointerScroll();
  }

  private resetPointerScroll(): void {
    this.terminalHost?.nativeElement.classList.remove('terminal-host-scrolling');
    this.pointerScroll = {
      tracking: false,
      scrolling: false,
      pointerId: -1,
      lastY: 0,
    };
  }

  private scrollViewportBy(lines: number, event?: Event): void {
    event?.preventDefault();
    event?.stopPropagation();
    if (!this.terminal || lines === 0) return;
    this.terminal.scrollLines(-lines);
  }

  private viewportScrollStep(): number {
    return Math.max(3, Math.floor((this.terminal?.rows ?? 12) / 2));
  }

  private wheelDeltaToLines(deltaY: number): number {
    return Math.max(1, Math.ceil(Math.abs(deltaY) / 40));
  }

  private touchDeltaToLines(deltaY: number): number {
    const cellHeight = Math.max(
      12,
      (this.terminal?.options.lineHeight ?? 1.1) * (this.terminal?.options.fontSize ?? 9),
    );
    return Math.max(1, Math.round(Math.abs(deltaY) / (cellHeight * 0.65)));
  }

  private isViewportAtBottom(): boolean {
    if (!this.terminal) return true;
    const buffer = this.terminal.buffer.active;
    return buffer.viewportY + buffer.baseY >= buffer.length - this.terminal.rows;
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
  private scrollToLatestOutput(_content: string): void {
    if (!this.terminal) return;
    if (!this.userPinnedToLatest) return;

    // Always follow the bottom of the snapshot. tmux cursorY is pane-local and
    // sends the viewport back to the top when the cursor sits on an upper row.
    this.terminal.scrollToBottom();
  }
}
