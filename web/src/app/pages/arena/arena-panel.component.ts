import { Component, Input } from '@angular/core';
import { RouterLink } from '@angular/router';
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
        <a class="a-btn" data-test="all-workflows" [routerLink]="['/workflows']">All workflows</a>
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
  styles: [ARENA_CSS, `.arena-panel { padding: 12px 14px 32px; }`],
})
export class ArenaPanelComponent {
  @Input() initiativeId = '';
  @Input() phases: {
    phaseId: string;
    phaseTitle?: string;
    effectiveTestCommands?: string[] | null;
  }[] = [];
}
