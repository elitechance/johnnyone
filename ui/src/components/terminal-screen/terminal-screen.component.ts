import {
  AfterViewInit,
  Component,
  ElementRef,
  EventEmitter,
  HostListener,
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
      fontFamily: 'ui-monospace, SFMono-Regular, Menlo, Monaco, Consolas, monospace',
      fontSize: 14,
      lineHeight: 1.2,
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
      const lines = Math.max(1, Math.ceil(Math.abs(event.deltaY) / 40));
      this.terminal.scrollLines(event.deltaY > 0 ? lines : -lines);
      event.preventDefault();
    };
    this.terminalHost.nativeElement.addEventListener('wheel', this.wheelHandler, { passive: false });

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
      }
      this.renderScreen(true);
    }
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
    this.requestHistoryRows(this.loadedHistoryRows() + TerminalScreenComponent.HISTORY_CHUNK_ROWS);
  }

  ngOnDestroy(): void {
    this.resizeObserver?.disconnect();
    if (this.wheelHandler) {
      this.terminalHost.nativeElement.removeEventListener('wheel', this.wheelHandler);
    }
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

  @HostListener('pointerdown')
  focus(): void {
    this.terminalHost.nativeElement
      .closest('johnny-terminal-screen')
      ?.querySelector<HTMLTextAreaElement>('textarea[name="terminalInput"]')
      ?.focus();
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
      } else {
        this.fitAddon.fit();
      }
      this.terminalResize.emit({
        cols: this.terminal.cols,
        rows: this.terminal.rows,
      });
    } catch {
      // The terminal can be hidden during route transitions; next resize will refit.
    }
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
      if (!force && nextContent === this.lastContent) return;

      this.lastContent = nextContent;
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

  private writeSnapshot(content: string): void {
    if (!this.terminal || this.writing || !this.screen) return;

    this.writing = true;
    this.terminal.write(`\x1b[?25l\x1b[3J\x1b[H\x1b[2J${content}\x1b[?25h`, () => {
      this.writing = false;
      if (!this.terminal || !this.screen) return;
      if (this.revealHistoryOnNextRender) {
        this.revealHistoryOnNextRender = false;
        this.terminal.scrollToTop();
      } else {
        this.terminal.scrollToBottom();
      }

      const latest = this.normalizeSnapshot(this.compactSnapshot(this.screen.content || ''));
      if (this.shouldPreserveLoadedHistory(latest)) {
        this.refreshLoadedHistory();
        return;
      }
      if (latest !== this.lastContent) {
        this.lastContent = latest;
        this.updateMermaidBlocks(latest);
        this.writeSnapshot(latest);
      }
    });
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

  private resetIdlePromptTimer(): void {
    if (this.idlePromptTimer) clearTimeout(this.idlePromptTimer);
  }
}
