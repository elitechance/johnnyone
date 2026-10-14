import { Component, Input, inject } from '@angular/core';
import { Router, RouterLink } from '@angular/router';
import { arenaReturnParams } from './arena-nav';
import { ArenaRowComponent } from './arena-row.component';
import { ARENA_CSS } from './arena-ui.css';

/**
 * The body of the console's ADDED `arena` tab. It holds the competition — the way in to the setup
 * for each phase, and the list of workflows — and deliberately holds NO documents: those live in
 * the Plan tab, and a second document browser here would be a second place to read the same files,
 * which is how two surfaces drift apart.
 */
@Component({
  selector: 'app-arena-panel',
  standalone: true,
  imports: [RouterLink, ArenaRowComponent],
  template: `
    <div class="arena-panel">
      <p class="a-note">
        The arena searches for a better prompt for one phase by running two versions of it against
        the same phase and the same test command. The winner is whichever one the gate exits 0 on —
        nothing else promotes.
      </p>
      <p class="a-note">
        The documents each run produced stay in the <b>Plan tab</b>, where every plan document
        already lives. This tab is the competition: setup, the run, and the one promotion decision.
      </p>
      <div class="a-row">
        <a class="a-btn" data-test="all-workflows" [routerLink]="['/workflows']" [queryParams]="onward()">All workflows</a>
      </div>

      @if (phases.length) {
        @for (p of phases; track p.phaseId) {
          <div class="a-card">
            <div class="a-row">
              <b>{{ p.phaseId }}</b>
              <span>{{ p.phaseTitle }}</span>
            </div>
            <johnny-arena-row [initiativeId]="initiativeId" [phase]="p" />
          </div>
        }
      } @else {
        <div class="a-card" data-test="no-phases">
          <p>This initiative has no phases yet, so there is nothing to compete for. The arena starts from a phase that already exists.</p>
        </div>
      }
    </div>
  `,
  styles: [
    ARENA_CSS,
    `
      /* The console's \`.terminal-surface\` is \`height: 100%; overflow: hidden\` — it gives each tab
         a box and expects the TAB to scroll inside it. Measured in Chrome at 400x860: without this
         the third phase card was cut off with no way to reach it, the same clip the routed pages
         had (before3-console-arena-tab-clipped.png). It lives here, in the component's own sheet,
         because terminal.page.scss is ~150 bytes under a budget that FAILS the build. */
      :host {
        display: block;
        height: 100%;
        overflow-y: auto;
        -webkit-overflow-scrolling: touch;
      }
      .arena-panel {
        padding-top: 12px;
        padding-left: max(14px, env(safe-area-inset-left, 0px));
        padding-right: max(14px, env(safe-area-inset-right, 0px));
        padding-bottom: calc(32px + env(safe-area-inset-bottom, 0px));
      }
    `,
  ],
})
export class ArenaPanelComponent {
  private readonly router = inject(Router);

  /**
   * The departure URL for every arena-ward link out of this tab. Without it the arena's back link
   * returned to a bare `/initiatives` — no initiative, no tab — which is not the place the person
   * left (measured: departing from `?initiativeId=p1&tab=arena` landed on `/initiatives`).
   */
  protected onward(): Record<string, string> {
    return arenaReturnParams(this.router.url);
  }

  @Input() initiativeId = '';
  @Input() phases: {
    phaseId: string;
    phaseTitle?: string;
    effectiveTestCommands?: string[] | null;
  }[] = [];
}
