import { Component } from '@angular/core';
import { IonContent } from '@ionic/angular/standalone';
import { ArenaApiService } from './arena-api.service';
import { ArenaSetupPage } from './arena-setup.page';

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
 * `@ionic/core/components/p-DWoUQeZ3.js:244`). Keeping the import out of `arena-setup.page.ts`
 * is what lets the body stay mounted and clicked in jsdom specs while the route still gets the
 * Ionic scroller it needs.
 */
@Component({
  selector: 'app-arena-setup-shell',
  standalone: true,
  // The arena's wire is provided by the SHELL, which is lazy, so the service and its ten GraphQL
  // documents are constructed in — and bundled with — the arena chunk. On a route it would have to
  // be imported by the eagerly-loaded route table instead.
  providers: [ArenaApiService],
  imports: [IonContent, ArenaSetupPage],
  template: `<ion-content class="arena-scroll"><app-arena-setup /></ion-content>`,
  styles: [
    `
      ion-content {
        --background: var(--ion-background-color, #000);
      }
    `,
  ],
})
export class ArenaSetupShell {}
