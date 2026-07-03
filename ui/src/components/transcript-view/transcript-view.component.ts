import {
  Component,
  Input,
  Output,
  EventEmitter,
  ElementRef,
  ChangeDetectionStrategy,
  OnChanges,
  inject,
} from '@angular/core';
import { CommonModule } from '@angular/common';
import { StreamEvent } from '../../models/stream-event.model';
import { MarkdownViewComponent } from '../markdown-view/markdown-view.component';
import { eventsToBlocks, TranscriptBlock } from './transcript-blocks';

// Re-export the pure mapper + types so consumers/tests can reach them via the
// component path or the `@johnnyone/ui` barrel (the pure module carries no Angular
// import, so specs import it directly — decision D8).
export { eventsToBlocks } from './transcript-blocks';
export type { TranscriptBlock, TranscriptBlockKind } from './transcript-blocks';

/**
 * Event-oriented transcript renderer. Maps `events` → blocks via `eventsToBlocks`
 * and renders each through the shared render core (`johnny-markdown-view` for
 * prose/code/mermaid, a compact card for tools, an error block for errors).
 *
 * SVG-zoom is surfaced as an `@Output` — the `ui` library must NOT import the
 * `web` `MermaidZoomService` (decision D4); the web host wires `svgZoom` to it.
 */
@Component({
  selector: 'johnny-transcript-view',
  standalone: true,
  imports: [CommonModule, MarkdownViewComponent],
  templateUrl: './transcript-view.component.html',
  styleUrls: ['./transcript-view.component.scss'],
  changeDetection: ChangeDetectionStrategy.OnPush,
})
export class TranscriptViewComponent implements OnChanges {
  @Input() events: StreamEvent[] = [];
  @Input() streaming = false;
  @Output() svgZoom = new EventEmitter<string>();

  blocks: TranscriptBlock[] = [];
  private readonly host = inject(ElementRef) as ElementRef<HTMLElement>;

  ngOnChanges(): void {
    this.blocks = eventsToBlocks(this.events);
  }

  /** Delegated click: if the click landed inside a rendered mermaid SVG, zoom it. */
  onClick(event: MouseEvent): void {
    const target = event.target as HTMLElement | null;
    const svg = target?.closest('svg') ?? null;
    if (svg) this.svgZoom.emit(svg.outerHTML);
  }

  toolPreview(data: unknown): string {
    if (data == null) return '';
    try {
      return JSON.stringify(data, null, 2);
    } catch {
      return String(data);
    }
  }
}
