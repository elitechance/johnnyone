import { Component, ElementRef, HostListener, inject, signal } from '@angular/core';
import { CommonModule } from '@angular/common';
import { Router, RouterLink, RouterLinkActive, RouterModule } from '@angular/router';
import {
  IonApp,
  IonButton,
  IonContent,
  IonIcon,
  IonItem,
  IonLabel,
  IonList,
  IonListHeader,
  IonMenu,
  IonMenuToggle,
  IonRouterOutlet,
  IonSplitPane,
  PopoverController,
} from '@ionic/angular/standalone';
import { addIcons } from 'ionicons';
import {
  addOutline,
  chevronBackOutline,
  chevronForwardOutline,
  codeSlashOutline,
  codeWorkingOutline,
  documentTextOutline,
  folderOutline,
  hammerOutline,
  logOutOutline,
  settingsOutline,
  terminalOutline,
} from 'ionicons/icons';
import { AuthService } from './services/auth.service';
import { MermaidZoomService } from './services/mermaid-zoom.service';
import { MermaidZoomModalComponent } from './components/mermaid-zoom-modal/mermaid-zoom-modal.component';
import { LauncherMenuComponent } from './components/launcher-menu/launcher-menu.component';

const MIN_WIDTH = 180;
const MAX_WIDTH = 480;
const STORAGE_KEY = 'jo_side_menu_width';
const COLLAPSED_STORAGE_KEY = 'jo_side_menu_collapsed';

@Component({
  imports: [
    CommonModule,
    RouterModule,
    RouterLink,
    RouterLinkActive,
    IonApp,
    IonButton,
    IonContent,
    IonIcon,
    IonItem,
    IonLabel,
    IonList,
    IonListHeader,
    IonMenu,
    IonMenuToggle,
    IonRouterOutlet,
    IonSplitPane,
    MermaidZoomModalComponent,
  ],
  selector: 'app-root',
  templateUrl: './app.component.html',
  styleUrl: './app.component.scss',
})
export class AppComponent {
  protected readonly auth = inject(AuthService);
  private readonly router = inject(Router);
  private readonly host = inject(ElementRef<HTMLElement>);
  private readonly mermaidZoom = inject(MermaidZoomService);
  private readonly popoverCtrl = inject(PopoverController);

  /** True while the user is dragging the side-menu resize bar. */
  protected readonly isResizing = signal(false);
  protected readonly isMenuCollapsed = signal(false);

  constructor() {
    addIcons({
      'add-outline': addOutline,
      'terminal-outline': terminalOutline,
      'chevron-back-outline': chevronBackOutline,
      'chevron-forward-outline': chevronForwardOutline,
      'document-text-outline': documentTextOutline,
      'folder-outline': folderOutline,
      'hammer-outline': hammerOutline,
      'code-slash-outline': codeSlashOutline,
      'code-working-outline': codeWorkingOutline,
      'settings-outline': settingsOutline,
      'log-out-outline': logOutOutline,
    });
    this.isMenuCollapsed.set(this.loadCollapsed());
    this.applyWidth(this.loadWidth());
  }

  /**
   * Global click delegation for mermaid diagrams. Any element on any page
   * with class `mermaid-rendered` (planner's renderer output) or `mermaid-svg`
   * (message-bubble's inline render — see message-bubble.component.ts) opens
   * the zoom modal with its SVG content.
   */
  @HostListener('document:click', ['$event'])
  onDocClick(event: MouseEvent): void {
    const target = event.target as HTMLElement | null;
    if (!target) return;
    const container =
      target.closest('.mermaid-rendered') ?? target.closest('.mermaid-svg');
    if (!container) return;
    const svg = container.querySelector('svg');
    if (!svg) return;
    event.preventDefault();
    this.mermaidZoom.open(svg.outerHTML);
  }

  /** Open the §06 `+ New` launcher popover, anchored at the trigger (P6). */
  async openLauncher(ev: Event): Promise<void> {
    const popover = await this.popoverCtrl.create({
      component: LauncherMenuComponent,
      event: ev,
      cssClass: 'launcher-popover',
    });
    await popover.present();
  }

  logout(): void {
    this.auth.logout();
    void this.router.navigate(['/login']);
  }

  startResize(event: MouseEvent): void {
    // Only react to primary-button drags; ignore on phone-narrow viewports
    // where the menu is an overlay drawer (resizer is hidden via CSS too).
    if (this.isMenuCollapsed() || event.button !== 0 || window.matchMedia('(max-width: 767px)').matches) return;
    event.preventDefault();
    this.isResizing.set(true);
  }

  toggleSideMenu(): void {
    this.isMenuCollapsed.update((collapsed) => {
      const next = !collapsed;
      try {
        localStorage.setItem(COLLAPSED_STORAGE_KEY, next ? '1' : '0');
      } catch {
        // ignore — quota / private browsing
      }
      return next;
    });
  }

  @HostListener('document:mousemove', ['$event'])
  onDocMouseMove(event: MouseEvent): void {
    if (!this.isResizing()) return;
    const next = Math.max(MIN_WIDTH, Math.min(MAX_WIDTH, event.clientX));
    this.applyWidth(next);
  }

  @HostListener('document:mouseup')
  onDocMouseUp(): void {
    if (!this.isResizing()) return;
    this.isResizing.set(false);
    const px = this.host.nativeElement.style.getPropertyValue('--jo-side-menu-width').trim();
    if (px) {
      try {
        localStorage.setItem(STORAGE_KEY, px);
      } catch {
        // ignore — quota / private browsing
      }
    }
  }

  private applyWidth(px: number): void {
    this.host.nativeElement.style.setProperty('--jo-side-menu-width', `${px}px`);
  }

  private loadWidth(): number {
    try {
      const raw = localStorage.getItem(STORAGE_KEY);
      if (raw) {
        const n = parseInt(raw.replace('px', ''), 10);
        if (Number.isFinite(n) && n >= MIN_WIDTH && n <= MAX_WIDTH) return n;
      }
    } catch {
      // ignore
    }
    return 240; // default — matches CSS :host fallback
  }

  private loadCollapsed(): boolean {
    try {
      return localStorage.getItem(COLLAPSED_STORAGE_KEY) === '1';
    } catch {
      return false;
    }
  }
}
