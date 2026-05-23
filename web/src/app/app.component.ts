import { Component, ElementRef, HostListener, inject, signal } from '@angular/core';
import { CommonModule } from '@angular/common';
import { Router, RouterLink, RouterLinkActive, RouterModule } from '@angular/router';
import {
  IonApp,
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
} from '@ionic/angular/standalone';
import { addIcons } from 'ionicons';
import {
  documentTextOutline,
  hammerOutline,
  logOutOutline,
  settingsOutline,
  terminalOutline,
} from 'ionicons/icons';
import { AuthService } from './services/auth.service';

const MIN_WIDTH = 180;
const MAX_WIDTH = 480;
const STORAGE_KEY = 'jo_side_menu_width';

@Component({
  imports: [
    CommonModule,
    RouterModule,
    RouterLink,
    RouterLinkActive,
    IonApp,
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
  ],
  selector: 'app-root',
  templateUrl: './app.component.html',
  styleUrl: './app.component.scss',
})
export class AppComponent {
  protected readonly auth = inject(AuthService);
  private readonly router = inject(Router);
  private readonly host = inject(ElementRef<HTMLElement>);

  /** True while the user is dragging the side-menu resize bar. */
  protected readonly isResizing = signal(false);

  constructor() {
    addIcons({
      'terminal-outline': terminalOutline,
      'document-text-outline': documentTextOutline,
      'hammer-outline': hammerOutline,
      'settings-outline': settingsOutline,
      'log-out-outline': logOutOutline,
    });
    this.applyWidth(this.loadWidth());
  }

  logout(): void {
    this.auth.logout();
    void this.router.navigate(['/login']);
  }

  startResize(event: MouseEvent): void {
    // Only react to primary-button drags; ignore on phone-narrow viewports
    // where the menu is an overlay drawer (resizer is hidden via CSS too).
    if (event.button !== 0 || window.matchMedia('(max-width: 767px)').matches) return;
    event.preventDefault();
    this.isResizing.set(true);
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
}
