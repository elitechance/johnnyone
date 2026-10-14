import { Component } from '@angular/core';
import { IonContent } from '@ionic/angular/standalone';
import { WorkspaceStatusbarComponent } from '../../components/workspace-statusbar/workspace-statusbar.component';
import { ArenaApiService } from './arena-api.service';
import { WorkflowsPage } from './workflows.page';

/**
 * The routed shell: an `ion-content` wrapper around the page body.
 *
 * MEASURED, in real Chrome at 400x860 (screenshots and metrics under
 * `.johnnyone/reports/arena-screens/`): without this, every arena route was CLIPPED and
 * UNSCROLLABLE. Ionic's router outlet puts `.ion-page` on the routed component — `position:
 * absolute; inset: 0; contain: size layout style` — and supplies the scroller through
 * `ion-content`, which these pages did not have. `document.scrollHeight` was pinned at the
 * viewport's 860px no matter how tall the content was, `scrollTop = 99999` moved nothing, and on
 * `/workflows/:id/arena` the content was 1437px tall: the promote and reject controls sat at
 * y=1324, 464px past a hard clip, with no way to reach them. That is the one place in this feature
 * a human is required, and it was unreachable on a phone. All 11 other routed pages in web/src use
 * `IonContent`; these four were the exception, and that was the defect.
 *
 * It is a SEPARATE FILE from the page body on purpose. `@ionic/angular/standalone` cannot be
 * imported under `web/vitest.config.ts` — it reaches `@ionic/core/components`, a directory
 * import Node's ESM loader refuses, so any spec that touches it collects zero tests (measured
 * twice: once with a resolve alias, which then died inside Ionic's own runtime at
 * `@ionic/core/components/p-DWoUQeZ3.js:244`). Keeping the import out of `workflows.page.ts`
 * is what lets the body stay mounted and clicked in jsdom specs while the route still gets the
 * Ionic scroller it needs.
 *
 * It also renders the console's own status strip, OUTSIDE the `ion-content`. Ionic's `.ion-page` is
 * a flex column and `ion-content` takes the free space, so a sibling after it is pinned to the
 * bottom rather than scrolling away with the content. MEASURED at 1200x860: the strip painted on
 * `/initiatives` and on no arena route, so the chrome disappeared as you crossed into the arena.
 * At phone width it costs nothing — the component is `display: none` under 900px, exactly as the
 * console's copy already was, so the 400x860 reachability numbers are unchanged.
 */
@Component({
  selector: 'app-workflows-shell',
  standalone: true,
  // The arena's wire is provided by the SHELL, which is lazy, so the service and its ten GraphQL
  // documents are constructed in — and bundled with — the arena chunk. On a route it would have to
  // be imported by the eagerly-loaded route table instead.
  providers: [ArenaApiService],
  imports: [IonContent, WorkflowsPage, WorkspaceStatusbarComponent],
  template: `<ion-content class="arena-scroll"><app-workflows /></ion-content><johnny-workspace-statusbar />`,
  styles: [
    `
      ion-content {
        --background: var(--ion-background-color, #000);
      }
    `,
  ],
})
export class WorkflowsShell {}
