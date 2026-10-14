import { Component } from '@angular/core';

/**
 * The thin chrome strip along the bottom of a workspace surface.
 *
 * It was markup inside `terminal.page.html` and nowhere else, which made it one of the seams
 * between the console and the arena: MEASURED in Chrome against the built bundle, at 1200x860, a
 * `footer.workspace-statusbar` painted on `/initiatives` and on NO arena route — the chrome
 * disappeared as you crossed. Extracting it is what lets all five surfaces render the same strip.
 *
 * AT PHONE WIDTH THERE WAS NOTHING TO FIX, and the media query below is why. The console's own copy
 * was already `display: none` under 900px: at 400x860 the footer was in the DOM with
 * `getComputedStyle(...).display === 'none'` and `checkVisibility()` false, so a phone saw no strip
 * on EITHER side of the switch. The brief that asked for this listed the status bar as a phone-width
 * seam on DOM presence; it is not one. The media query is kept verbatim so that stays true, and the
 * fix lands at the width where the seam is real.
 *
 * The two lines are static text, carried over unchanged — and the first one is a claim this
 * component cannot support: it says `relay connected` whatever the relay is doing. Wiring it to the
 * real connection state is a change to what the UI ASSERTS, not to its layout, so it is reported
 * rather than smuggled into a navigation fix.
 */
@Component({
  selector: 'johnny-workspace-statusbar',
  standalone: true,
  template: `
    <footer class="workspace-statusbar">
      <span>relay <strong>connected</strong></span>
      <span>raw input batched at <strong>200ms</strong></span>
    </footer>
  `,
  styles: [
    `
      :host {
        display: block;
      }

      .workspace-statusbar {
        display: flex;
        align-items: center;
        justify-content: space-between;
        gap: 12px;
        padding: 0 10px;
        border-top: 1px solid #263241;
        background: #0a0d12;
        color: #8fa0b3;
        font: 11px/1 ui-monospace, SFMono-Regular, Menlo, Monaco, Consolas, monospace;
        min-height: 22px;
      }

      .workspace-statusbar strong {
        color: #dbe7f3;
        font-weight: 500;
      }

      /* Verbatim from terminal.page.scss. See the class comment: at phone width the console never
         painted this strip either, so hiding it here is what keeps the two sides identical. */
      @media (max-width: 900px) {
        :host {
          display: none;
        }
      }
    `,
  ],
})
export class WorkspaceStatusbarComponent {}
