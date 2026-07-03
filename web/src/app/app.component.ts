import { Component, HostListener, inject } from '@angular/core';
import { RouterLink, RouterLinkActive } from '@angular/router';
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
} from 'ionicons/icons';
import { AuthService } from './services/auth.service';
import { MermaidZoomService } from './services/mermaid-zoom.service';
import { MermaidZoomModalComponent } from './components/mermaid-zoom-modal/mermaid-zoom-modal.component';
import { LauncherMenuComponent } from './components/launcher-menu/launcher-menu.component';
import { NAV_ITEMS } from './nav-items';

@Component({
  imports: [
    RouterLink,
    RouterLinkActive,
    IonApp,
    IonIcon,
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
  private readonly mermaidZoom = inject(MermaidZoomService);
  private readonly popoverCtrl = inject(PopoverController);

  /** The four global destinations shared by the rail + bottom nav (single source). */
  protected readonly navItems = NAV_ITEMS;

  constructor() {
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
