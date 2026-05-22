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

@Component({
  selector: 'johnny-terminal-screen',
  standalone: true,
  imports: [CommonModule, FormsModule],
  templateUrl: './terminal-screen.component.html',
  styleUrls: ['./terminal-screen.component.scss'],
})
export class TerminalScreenComponent implements AfterViewInit, OnChanges, OnDestroy {
  @Input() screen: TerminalScreen | null = null;
  @Input() disabled = false;
  @Input() mobileInputMode = false;
  @Input() showInput = true;
  @Input() title = 'Terminal';

  @Output() rawInput = new EventEmitter<string>();
  @Output() terminalResize = new EventEmitter<{ cols: number; rows: number }>();

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
      this.renderScreen(false);
    }
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
  }

  @HostListener('pointerdown')
  focus(): void {
    this.terminalHost.nativeElement
      .closest('johnny-terminal-screen')
      ?.querySelector<HTMLTextAreaElement>('textarea[name="terminalInput"]')
      ?.focus();
  }

  protected submitTerminalInput(): void {
    if (this.disabled) return;

    const input = this.mobileInputBuffer;
    this.mobileInputBuffer = '';
    this.rawInput.emit(`${input}\r`);
    this.refocusMobileInput();
  }

  protected sendControlInput(input: string): void {
    if (this.disabled) return;
    this.rawInput.emit(input);
    this.refocusMobileInput();
  }

  protected onMobileInputKeydown(event: KeyboardEvent): void {
    if (event.key !== 'Enter' || event.shiftKey) return;

    event.preventDefault();
    this.submitTerminalInput();
  }

  protected onTerminalInputPaste(event: ClipboardEvent): void {
    if (this.disabled) return;

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

  private refocusMobileInput(): void {
    if (!this.mobileInputMode) return;
    queueMicrotask(() => this.mobileInput?.nativeElement.focus());
  }

  fit(): void {
    if (!this.terminal || !this.fitAddon) return;

    try {
      this.fitAddon.fit();
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
      if (!force && nextContent === this.lastContent) return;

      this.lastContent = nextContent;
      this.writeSnapshot(nextContent);
    });
  }

  private writeSnapshot(content: string): void {
    if (!this.terminal || this.writing || !this.screen) return;

    this.writing = true;
    this.terminal.write(`\x1b[?25l\x1b[3J\x1b[H\x1b[2J${content}\x1b[?25h`, () => {
      this.writing = false;
      if (!this.terminal || !this.screen) return;
      this.terminal.scrollToBottom();

      const latest = this.normalizeSnapshot(this.compactSnapshot(this.screen.content || ''));
      if (latest !== this.lastContent) {
        this.lastContent = latest;
        this.writeSnapshot(latest);
      }
    });
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
