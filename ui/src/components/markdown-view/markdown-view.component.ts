import {
  Component,
  Input,
  ElementRef,
  AfterViewChecked,
  ChangeDetectionStrategy,
  inject,
} from '@angular/core';
import { CommonModule } from '@angular/common';
import { DomSanitizer, SafeHtml } from '@angular/platform-browser';
import { parseMarkdown, hydrateMermaid } from '../../lib/markdown-render';

/**
 * Thin, document-oriented markdown renderer over the Phase-01 render core.
 * The single reusable renderer the brief names — reused by the transcript view,
 * the Phase 5 file viewer, and the Plan tab. Same sanitization posture as
 * `message-bubble` (`bypassSecurityTrustHtml`, decision D9 / D9-followup).
 */
@Component({
  selector: 'johnny-markdown-view',
  standalone: true,
  imports: [CommonModule],
  template: `<div class="markdown-body" [innerHTML]="html"></div>`,
  changeDetection: ChangeDetectionStrategy.OnPush,
})
export class MarkdownViewComponent implements AfterViewChecked {
  private readonly sanitizer = inject(DomSanitizer);
  private readonly host = inject(ElementRef) as ElementRef<HTMLElement>;
  private _md = '';
  html: SafeHtml = '';

  @Input() set markdown(value: string) {
    this._md = value ?? '';
    this.html = this.sanitizer.bypassSecurityTrustHtml(parseMarkdown(this._md));
  }
  get markdown(): string {
    return this._md;
  }

  ngAfterViewChecked(): void {
    hydrateMermaid(this.host.nativeElement);
  }
}
