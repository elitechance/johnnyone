import {
  ChangeDetectionStrategy,
  Component,
  ElementRef,
  HostListener,
  ViewChild,
  effect,
  inject,
  signal,
} from '@angular/core';
import { CommonModule } from '@angular/common';
import {
  IonButton,
  IonButtons,
  IonContent,
  IonHeader,
  IonIcon,
  IonModal,
  IonTitle,
  IonToolbar,
} from '@ionic/angular/standalone';
import { DomSanitizer, SafeHtml } from '@angular/platform-browser';
import { addIcons } from 'ionicons';
import {
  addOutline,
  closeOutline,
  expandOutline,
  removeOutline,
} from 'ionicons/icons';
import { MermaidZoomService } from '../../services/mermaid-zoom.service';

addIcons({
  'add-outline': addOutline,
  'remove-outline': removeOutline,
  'expand-outline': expandOutline,
  'close-outline': closeOutline,
});

const MIN_SCALE = 0.25;
const MAX_SCALE = 8;
const ZOOM_STEP = 1.2;
const WHEEL_SENSITIVITY = 0.0015;

/**
 * Full-screen mermaid zoom modal mounted once at the app root. Reads its open
 * state and SVG content from `MermaidZoomService` (any page calls
 * `mermaidZoom.open(svg)` to bring it up).
 *
 * Supports:
 *   - Mouse-wheel / pinch zoom (cursor-anchored)
 *   - Drag to pan
 *   - +/- buttons + Fit button + close
 *   - Escape key closes
 */
@Component({
  selector: 'app-mermaid-zoom-modal',
  standalone: true,
  imports: [
    CommonModule,
    IonModal,
    IonHeader,
    IonToolbar,
    IonTitle,
    IonButtons,
    IonButton,
    IonIcon,
    IonContent,
  ],
  changeDetection: ChangeDetectionStrategy.OnPush,
  templateUrl: './mermaid-zoom-modal.component.html',
  styleUrl: './mermaid-zoom-modal.component.scss',
})
export class MermaidZoomModalComponent {
  protected readonly zoom = inject(MermaidZoomService);
  private readonly sanitizer = inject(DomSanitizer);

  @ViewChild('viewport') viewport?: ElementRef<HTMLElement>;

  protected readonly scale = signal(1);
  protected readonly panX = signal(0);
  protected readonly panY = signal(0);

  // Pointer tracking. Single pointer = drag/pan. Two pointers = pinch zoom.
  // We use the PointerEvent API uniformly so mouse + touch + stylus all work
  // through the same handlers (no separate touchstart/touchmove plumbing).
  private readonly pointers = new Map<number, { x: number; y: number }>();
  private pinchStartDistance = 0;
  private pinchStartScale = 1;
  private pinchStartPan = { x: 0, y: 0 };
  private dragStartX = 0;
  private dragStartY = 0;
  private panStartX = 0;
  private panStartY = 0;
  // Double-tap-to-reset on touch (the wheel-anchored desktop reset feels
  // different from the mobile expectation).
  private lastTapAt = 0;
  private lastTapX = 0;
  private lastTapY = 0;

  protected readonly safeSvg = signal<SafeHtml>('');

  constructor() {
    effect(() => {
      // Whenever the SVG changes, re-sanitize and reset the zoom/pan.
      // The fit-to-viewport pass runs from `onDidPresent` (ion-modal lifecycle)
      // because ion-modal uses <ng-template> — the SVG isn't in the DOM here.
      const raw = this.zoom.svg();
      this.safeSvg.set(raw ? this.sanitizer.bypassSecurityTrustHtml(raw) : '');
      this.reset();
    });
  }

  /**
   * Called by ion-modal's didPresent event — the SVG is now in the DOM and
   * the modal's open animation has finished, so getBoundingClientRect is
   * accurate. Apply a fit-to-viewport scale.
   */
  protected onDidPresent(): void {
    // ion-modal's didPresent fires before the inner ng-template viewchildren
    // resolve on every Ionic build, so we don't rely on @ViewChild here.
    // A short tail of retries handles late layouts (mobile SVG sometimes
    // re-flows after webfonts settle).
    this.scheduleFitRetries();
  }

  private scheduleFitRetries(): void {
    const delays = [0, 50, 150, 400];
    for (const ms of delays) {
      setTimeout(() => this.fitToViewport(), ms);
    }
  }

  /**
   * Compute the scale needed so the SVG fills ~95% of the viewport's smaller
   * dimension, then apply it. Called from didPresent so a freshly-loaded
   * diagram looks usable instead of microscopic on phones.
   *
   * Uses `document.querySelector` rather than @ViewChild because the modal
   * is mounted globally and ion-modal's ng-template viewchildren are
   * resolved unreliably across versions.
   */
  private fitToViewport(): void {
    // The mermaid modal is mounted once globally — there's only ever one
    // `.mermaid-zoom-viewport` and one `.mermaid-zoom-stage` in the DOM.
    const viewport = document.querySelector<HTMLElement>('.mermaid-zoom-viewport');
    const svg = document.querySelector<SVGSVGElement>('.mermaid-zoom-stage svg');
    if (!viewport || !svg) return;

    const svgRect = svg.getBoundingClientRect();
    const vpRect = viewport.getBoundingClientRect();
    if (svgRect.width <= 0 || svgRect.height <= 0 || vpRect.width <= 0 || vpRect.height <= 0) {
      return;
    }

    // Pick the scale that fills more of the viewport. Mermaid SVGs are often
    // wide-aspect (graphs and flowcharts), so on a portrait phone, simple
    // fit-to-both leaves the diagram microscopic with huge empty bands above
    // and below. We bias toward filling: take the bigger of (width-fit) and
    // (height-fit-at-60%-vh), capped at 3x so we never go crazy. If the
    // diagram overflows the viewport in one dimension, the user can pan.
    const fitW = vpRect.width * 0.95 / svgRect.width;
    const fitH60 = vpRect.height * 0.6 / svgRect.height;
    const fit = Math.max(fitW, fitH60);
    const clamped = Math.max(MIN_SCALE, Math.min(3, Math.min(MAX_SCALE, fit)));

    // Only override the initial scale if the user hasn't started zooming yet
    // (scale === 1 means we're still at the post-reset baseline).
    if (this.scale() <= 1.01 && clamped > 1) {
      this.scale.set(clamped);
    }
  }

  protected onWheel(event: WheelEvent): void {
    event.preventDefault();
    const factor = Math.exp(-event.deltaY * WHEEL_SENSITIVITY);
    this.applyZoomAt(factor, event.clientX, event.clientY);
  }

  protected onPointerDown(event: PointerEvent): void {
    // Accept primary mouse + touch + stylus. Ignore secondary mouse buttons.
    if (event.pointerType === 'mouse' && event.button !== 0) return;

    (event.target as HTMLElement).setPointerCapture?.(event.pointerId);
    this.pointers.set(event.pointerId, { x: event.clientX, y: event.clientY });

    if (this.pointers.size === 1) {
      // Single pointer down: start a drag.
      this.dragStartX = event.clientX;
      this.dragStartY = event.clientY;
      this.panStartX = this.panX();
      this.panStartY = this.panY();

      // Detect double-tap (within 300ms, within 30px) to reset zoom.
      const now = Date.now();
      const dt = now - this.lastTapAt;
      const dx = event.clientX - this.lastTapX;
      const dy = event.clientY - this.lastTapY;
      if (dt > 0 && dt < 300 && Math.hypot(dx, dy) < 30) {
        this.reset();
        this.lastTapAt = 0;
      } else {
        this.lastTapAt = now;
        this.lastTapX = event.clientX;
        this.lastTapY = event.clientY;
      }
    } else if (this.pointers.size === 2) {
      // Second pointer down: start a pinch. Cache the current distance,
      // scale, and pan so the move handler can compute relative changes.
      const [a, b] = Array.from(this.pointers.values());
      this.pinchStartDistance = Math.hypot(b.x - a.x, b.y - a.y) || 1;
      this.pinchStartScale = this.scale();
      this.pinchStartPan = { x: this.panX(), y: this.panY() };
    }
  }

  protected onPointerMove(event: PointerEvent): void {
    if (!this.pointers.has(event.pointerId)) return;
    this.pointers.set(event.pointerId, { x: event.clientX, y: event.clientY });

    if (this.pointers.size === 2) {
      // Pinch: scale anchored at the midpoint of the two pointers.
      const [a, b] = Array.from(this.pointers.values());
      const dist = Math.hypot(b.x - a.x, b.y - a.y);
      const targetScale = Math.max(
        MIN_SCALE,
        Math.min(MAX_SCALE, this.pinchStartScale * (dist / this.pinchStartDistance)),
      );
      const midX = (a.x + b.x) / 2;
      const midY = (a.y + b.y) / 2;
      this.setZoomAnchored(targetScale, midX, midY, this.pinchStartScale, this.pinchStartPan);
    } else if (this.pointers.size === 1) {
      // Pan.
      const dx = event.clientX - this.dragStartX;
      const dy = event.clientY - this.dragStartY;
      this.panX.set(this.panStartX + dx);
      this.panY.set(this.panStartY + dy);
    }
  }

  protected onPointerUp(event: PointerEvent): void {
    (event.target as HTMLElement).releasePointerCapture?.(event.pointerId);
    this.pointers.delete(event.pointerId);

    // If we drop from 2 → 1 pointer, the remaining pointer becomes a fresh
    // drag-anchor so the user can pan without lifting and re-touching.
    if (this.pointers.size === 1) {
      const [remaining] = Array.from(this.pointers.values());
      this.dragStartX = remaining.x;
      this.dragStartY = remaining.y;
      this.panStartX = this.panX();
      this.panStartY = this.panY();
    }
  }

  protected zoomIn(): void {
    this.applyZoomAt(ZOOM_STEP, undefined, undefined);
  }

  protected zoomOut(): void {
    this.applyZoomAt(1 / ZOOM_STEP, undefined, undefined);
  }

  protected reset(): void {
    this.scale.set(1);
    this.panX.set(0);
    this.panY.set(0);
  }

  protected close(): void {
    this.zoom.close();
  }

  @HostListener('document:keydown.escape')
  onEscape(): void {
    if (this.zoom.isOpen()) this.close();
  }

  /**
   * Zoom by `factor`, keeping the point under (clientX,clientY) — or the
   * viewport center if no anchor — visually fixed.
   */
  private applyZoomAt(factor: number, clientX?: number, clientY?: number): void {
    const oldScale = this.scale();
    const newScale = Math.max(MIN_SCALE, Math.min(MAX_SCALE, oldScale * factor));
    if (newScale === oldScale) return;

    const viewport = this.viewport?.nativeElement;
    if (viewport && clientX !== undefined && clientY !== undefined) {
      const rect = viewport.getBoundingClientRect();
      const cx = rect.width / 2;
      const cy = rect.height / 2;
      // (x,y) under cursor in pre-zoom local coords:
      const localX = clientX - rect.left - cx - this.panX();
      const localY = clientY - rect.top - cy - this.panY();
      const ratio = newScale / oldScale;
      this.panX.set(this.panX() - localX * (ratio - 1));
      this.panY.set(this.panY() - localY * (ratio - 1));
    }

    this.scale.set(newScale);
  }

  /**
   * Set absolute scale (not relative) keeping the point (clientX,clientY)
   * anchored. Used by pinch where we know the target scale directly.
   * `pinchStartScale` + `pinchStartPan` are the values at pinch start so
   * the math doesn't drift across many small move events.
   */
  private setZoomAnchored(
    targetScale: number,
    clientX: number,
    clientY: number,
    startScale: number,
    startPan: { x: number; y: number },
  ): void {
    const viewport = this.viewport?.nativeElement;
    if (!viewport) {
      this.scale.set(targetScale);
      return;
    }
    const rect = viewport.getBoundingClientRect();
    const cx = rect.width / 2;
    const cy = rect.height / 2;
    const localX = clientX - rect.left - cx - startPan.x;
    const localY = clientY - rect.top - cy - startPan.y;
    const ratio = targetScale / startScale;
    this.panX.set(startPan.x - localX * (ratio - 1));
    this.panY.set(startPan.y - localY * (ratio - 1));
    this.scale.set(targetScale);
  }
}
