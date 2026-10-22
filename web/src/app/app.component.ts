import { Component, HostListener, computed, inject, signal } from '@angular/core';
import { NavigationEnd, Router, RouterLink } from '@angular/router';
import {
  IonApp,
  IonIcon,
  IonRouterOutlet,
  IonSplitPane,
  PopoverController,
} from '@ionic/angular/standalone';
import { addIcons } from 'ionicons';
import {
  addOutline,
  appsOutline,
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
  trophyOutline,
} from 'ionicons/icons';
import { AuthService } from './services/auth.service';
import { MermaidZoomService } from './services/mermaid-zoom.service';
import { MermaidZoomModalComponent } from './components/mermaid-zoom-modal/mermaid-zoom-modal.component';
import { FileViewerModalComponent } from './components/file-viewer-modal/file-viewer-modal.component';
import { LauncherMenuComponent } from './components/launcher-menu/launcher-menu.component';
import { NAV_ITEMS, navItemIsActive, navLinkQueryParams } from './nav-items';

@Component({
  imports: [
    RouterLink,
    IonApp,
    IonIcon,
    IonRouterOutlet,
    IonSplitPane,
    MermaidZoomModalComponent,
    FileViewerModalComponent,
  ],
  selector: 'app-root',
  templateUrl: './app.component.html',
  styleUrl: './app.component.scss',
})
export class AppComponent {
  protected readonly auth = inject(AuthService);
  private readonly router = inject(Router);
  private readonly mermaidZoom = inject(MermaidZoomService);
  private readonly popoverCtrl = inject(PopoverController);

  /** The global destinations shared by the rail + bottom nav (single source). */
  protected readonly navItems = NAV_ITEMS;

  /**
   * The URL the nav lights from. `routerLinkActive` is NOT used for this any more, and that is the
   * whole fix: it lights an item only when the URL starts with that item's own `routerLink`, so the
   * arena's `/workflows` tree lit nothing — measured in Chrome at 400x860, `nav.botnav a.b.on` was
   * the empty set on `/workflows`, `/workflows/:id` and `/workflows/:id/arena`. Each item now
   * declares the area it OWNS (`NavItem.owns`) and the lit state is a prefix test against that
   * (`navItemIsActive`, pinned by `nav-active.spec.ts`).
   *
   * Seeded from `router.url` rather than only from the event stream: the first paint happens before
   * any `NavigationEnd` on a cold load, and an unlit bar on first paint is the same defect.
   */
  private readonly currentUrl = signal(this.router.url);
  protected readonly navActive = computed(() => {
    const url = this.currentUrl();
    return Object.fromEntries(NAV_ITEMS.map((item) => [item.id, navItemIsActive(item, url)]));
  });

  /**
   * Per-item link query. Only the arena entry gets one, and only from the console — see
   * `navLinkQueryParams`. It is what stops the global way in from being a way to LOSE the
   * initiative you were on.
   */
  protected readonly navQuery = computed(() => {
    const url = this.currentUrl();
    return Object.fromEntries(NAV_ITEMS.map((item) => [item.id, navLinkQueryParams(item, url)]));
  });

  constructor() {
    this.router.events.subscribe((event) => {
      if (event instanceof NavigationEnd) this.currentUrl.set(event.urlAfterRedirects);
    });
    // The rail uses add/apps/folder/code-working/settings; the remaining glyphs
    // stay registered so other pages that reference them by name keep resolving.
    addIcons({
      'add-outline': addOutline,
      'apps-outline': appsOutline,
      'terminal-outline': terminalOutline,
      'chevron-back-outline': chevronBackOutline,
      'chevron-forward-outline': chevronForwardOutline,
      'document-text-outline': documentTextOutline,
      'folder-outline': folderOutline,
      'hammer-outline': hammerOutline,
      'code-slash-outline': codeSlashOutline,
      'code-working-outline': codeWorkingOutline,
      'settings-outline': settingsOutline,
      'trophy-outline': trophyOutline,
      'log-out-outline': logOutOutline,
    });
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

  /** Open the §06 `+ New` launcher popover, anchored at the trigger (P6, reused verbatim). */
  async openLauncher(ev: Event): Promise<void> {
    const popover = await this.popoverCtrl.create({
      component: LauncherMenuComponent,
      event: ev,
      cssClass: 'launcher-popover',
    });
    await popover.present();
  }
}
