import { Injectable, signal } from '@angular/core';

/**
 * Global state for the mermaid zoom modal. One instance lives at the app root
 * and any page can `open(svg)` it; the modal renders the SVG full-screen with
 * wheel-zoom / drag-pan.
 *
 * The SVG is passed as raw markup (the string emitted by `mermaid.render`'s
 * `.svg` property, or the `outerHTML` of a rendered `<svg>` node).
 */
@Injectable({ providedIn: 'root' })
export class MermaidZoomService {
  /** Whether the modal is currently shown. */
  readonly isOpen = signal(false);
  /** SVG markup currently being displayed. */
  readonly svg = signal<string>('');

  open(svgMarkup: string): void {
    if (!svgMarkup) return;
    this.svg.set(svgMarkup);
    this.isOpen.set(true);
  }

  close(): void {
    this.isOpen.set(false);
    // Clear after the modal animates out so the next open doesn't briefly
    // flash the previous content.
    setTimeout(() => this.svg.set(''), 300);
  }
}
