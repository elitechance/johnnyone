import {
  AfterViewChecked,
  Component,
  ElementRef,
  Input,
  OnDestroy,
  ChangeDetectionStrategy,
  ChangeDetectorRef,
  inject,
} from '@angular/core';
import { CommonModule } from '@angular/common';
import { IonicModule } from '@ionic/angular';
import { addIcons } from 'ionicons';
import {
  personOutline,
  sparklesOutline,
  settingsOutline,
  constructOutline,
  copyOutline,
  checkmarkOutline,
  closeOutline,
  alertCircleOutline,
} from 'ionicons/icons';
import { DomSanitizer, SafeHtml } from '@angular/platform-browser';
import { AiMessage } from '../../models/ai-message.model';
import { parseMarkdown, hydrateMermaid } from '../../lib/markdown-render';
import {
  canCopyMessage,
  clearCopyResetTimer,
  copyTextFor,
  copyStateLabelFor,
  copyIconFor,
  performCopy,
  type CopyOutcome,
} from '../../lib/copy-message';

/** How long the confirmed "Copied" state stays up before it clears itself. */
const COPY_FEEDBACK_MS = 2000;

/** What the copy button shows right now — nothing until it is first clicked. */
export type CopyState = CopyOutcome | null;

@Component({
  selector: 'johnny-message-bubble',
  standalone: true,
  imports: [CommonModule, IonicModule],
  templateUrl: './message-bubble.component.html',
  styleUrls: ['./message-bubble.component.scss'],
  changeDetection: ChangeDetectionStrategy.OnPush,
})
export class MessageBubbleComponent implements AfterViewChecked, OnDestroy {
  @Input({ required: true }) message!: AiMessage;
  @Input() isStreamingMessage = false;
  private readonly host: ElementRef<HTMLElement> = inject(ElementRef);
  private readonly cdr: ChangeDetectorRef = inject(ChangeDetectorRef);

  /**
   * Last copy outcome for THIS bubble, so the button can actually tell the user what
   * happened. `null` = never clicked; 'copied' shows a checkmark that clears itself;
   * 'unavailable'/'failed' stay up (they are a real condition, not a no-op to hide).
   */
  copyState: CopyState = null;
  private copyResetTimer?: ReturnType<typeof setTimeout>;

  constructor(private sanitizer: DomSanitizer) {
    // Register the ion-icons this bubble uses — without this, Ionicons standalone tries to fetch the
    // SVG by URL and throws "Failed to construct 'URL': Invalid base URL" for each icon.
    addIcons({
      'person-outline': personOutline,
      'sparkles-outline': sparklesOutline,
      'settings-outline': settingsOutline,
      'construct-outline': constructOutline,
      'copy-outline': copyOutline,
      'checkmark-outline': checkmarkOutline,
      'close-outline': closeOutline,
      'alert-circle-outline': alertCircleOutline,
    });
  }

  /** Every message with non-whitespace content is copyable, whatever its role. */
  get canCopy(): boolean {
    return canCopyMessage(this.message);
  }

  /** The icon decision lives in the shared seam, so the transcript row shows the same one. */
  get copyIcon(): string {
    return copyIconFor(this.copyState);
  }

  /**
   * The wording lives in the pure `copyStateLabelFor` seam (ui/src/lib/copy-message)
   * so it can be tested without Angular/Ionic — and so 'unavailable' (no clipboard API
   * at all, i.e. a non-secure http page) reads differently from 'failed' (a clipboard
   * that refused THIS write, which the user can retry).
   */
  get copyStateLabel(): string {
    return copyStateLabelFor(this.copyState);
  }

  get copyButtonTitle(): string {
    switch (this.copyState) {
      case 'copied':
        return 'Copied';
      case 'unavailable':
      case 'failed':
        return 'Copying is not available here';
      default:
        return 'Copy message';
    }
  }

  /**
   * Hydrate any `<div class="mermaid-svg">` placeholders the markdown parser
   * emitted with actual rendered SVGs. mermaid.render is async, so we can't
   * do this inside marked's synchronous renderer — we do it here after each
   * view check, marking nodes as processed to avoid re-rendering.
   */
  ngAfterViewChecked(): void {
    hydrateMermaid(this.host.nativeElement);
  }

  /**
   * A bubble scrolled out of a long transcript (the list is capped at
   * MAX_TRANSCRIPT_EVENTS, so this happens constantly) is destroyed with the
   * "Copied" reset timer still pending, and that callback would then touch a
   * destroyed view. Cancel it on teardown.
   */
  ngOnDestroy(): void {
    clearCopyResetTimer(this.copyResetTimer);
    this.copyResetTimer = undefined;
  }

  get isUser(): boolean {
    return this.message.role === 'user';
  }

  get isAssistant(): boolean {
    return this.message.role === 'assistant';
  }

  get isSystem(): boolean {
    return this.message.role === 'system';
  }

  get isTool(): boolean {
    return this.message.role === 'tool';
  }

  get hasToolCalls(): boolean {
    return !!this.message.toolCalls && this.message.toolCalls.length > 0;
  }

  get parsedToolCalls(): Array<{ id: string; name: string; input: unknown }> {
    if (!this.message.toolCalls) return [];
    try {
      return JSON.parse(this.message.toolCalls);
    } catch (err) {
      // Malformed tool_calls JSON from the backend: show no tool calls rather than
      // blanking the whole bubble, but say so in the console instead of hiding it.
      console.warn('[message-bubble] unparseable tool_calls payload', this.message.id, err);
      return [];
    }
  }

  get roleLabel(): string {
    switch (this.message.role) {
      case 'user':
        return 'You';
      case 'assistant':
        return 'Johnny';
      case 'system':
        return 'System';
      case 'tool':
        return 'Tool';
      default:
        return this.message.role;
    }
  }

  get roleIcon(): string {
    switch (this.message.role) {
      case 'user':
        return 'person-outline';
      case 'assistant':
        return 'sparkles-outline';
      case 'system':
        return 'settings-outline';
      case 'tool':
        return 'construct-outline';
      default:
        return 'chatbubble-outline';
    }
  }

  get formattedTime(): string {
    if (!this.message.createdAt) return '';
    const date = new Date(this.message.createdAt);
    return date.toLocaleTimeString([], { hour: '2-digit', minute: '2-digit' });
  }

  /** Render message content as markdown (for assistant/user) or plain text (for others). */
  get renderedContent(): SafeHtml {
    if (!this.message.content) {
      return '';
    }

    if (this.isAssistant || this.isUser) {
      const html = parseMarkdown(this.message.content);
      return this.sanitizer.bypassSecurityTrustHtml(html);
    }

    return this.message.content;
  }

  get useHtml(): boolean {
    return this.isAssistant || this.isUser;
  }

  formatToolCallInput(toolCall: { id: string; name: string; input: unknown }): string {
    try {
      return JSON.stringify(toolCall.input, null, 2);
    } catch (err) {
      // Circular / non-serializable input: fall back to the string form, and log why.
      console.warn('[message-bubble] unserializable tool_call input', toolCall?.name, err);
      return String(toolCall.input);
    }
  }

  /**
   * Copy THIS message's raw markdown to the clipboard and show the result on the
   * button. `performCopy` never throws, so there is nothing to swallow here; the
   * 'unavailable' case (no `navigator.clipboard`, i.e. a non-secure context such as
   * plain http on a phone) is surfaced instead of silently doing nothing.
   */
  async copyContent(): Promise<void> {
    const text = copyTextFor(this.message);
    if (!text) return;

    // `navigator.clipboard` is undefined in a non-secure context — pass it through
    // as-is so performCopy can classify that as 'unavailable'.
    const clipboard = typeof navigator === 'undefined' ? null : navigator.clipboard ?? null;
    const outcome = await performCopy(text, clipboard);

    this.copyState = outcome;
    clearCopyResetTimer(this.copyResetTimer);
    if (outcome === 'copied') {
      // The confirmation is transient; a failure stays up so it can be read.
      this.copyResetTimer = setTimeout(() => {
        this.copyState = null;
        this.cdr.markForCheck();
      }, COPY_FEEDBACK_MS);
    }
    this.cdr.markForCheck();
  }
}
