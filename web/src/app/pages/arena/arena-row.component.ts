import { Component, Input } from '@angular/core';
import { RouterLink } from '@angular/router';
import { arenaEntryRow } from './arena-logic';
import { ARENA_CSS } from './arena-ui.css';

/**
 * Mock step 1: ONE extra row on a phase that already exists, inside the console the operator
 * already uses. No new place to go, and no separate "create a workflow" flow — the entry point is
 * the phase, because the prompt being searched for is the one that phase already runs.
 *
 * When the phase has no gate the control STAYS ON SCREEN, disabled, with its reason in text beside
 * it, and a way out (set a gate). A hidden control teaches nobody why this phase cannot compete.
 */
@Component({
  selector: 'johnny-arena-row',
  standalone: true,
  imports: [RouterLink],
  template: `
    @if (phase) {
      <div class="arena-row">
        @if (entry().enabled) {
          <div class="a-row">
            <a
              class="a-btn"
              data-test="enter-arena"
              [routerLink]="['/initiatives', initiativeId, 'arena']"
              [queryParams]="{ phaseId: phase.phaseId }"
            >⚔ Enter the arena</a>
            <span class="a-muted ar-sub">search for a better prompt for this phase</span>
          </div>
          <div class="mono a-muted ar-cmds">
            @for (c of phase.effectiveTestCommands ?? []; track c) {<div>{{ c }}</div>}
          </div>
        } @else {
          <div class="a-row">
            <button type="button" class="a-btn" data-test="enter-arena-disabled" disabled>⚔ Enter the arena ✕</button>
            <a
              class="ar-set"
              data-test="set-gate"
              [routerLink]="['/initiatives', initiativeId, 'arena']"
              [queryParams]="{ phaseId: phase.phaseId, step: 'gate' }"
            >set one</a>
          </div>
          <p class="a-note" data-test="arena-row-reason">{{ entry().reason }}</p>
        }
      </div>
    }
  `,
  styles: [
    ARENA_CSS,
    `
      .arena-row { padding: 8px 0 2px; }
      .ar-sub { font-size: 12.5px; }
      .ar-cmds { margin-top: 4px; }
      .ar-set { font-size: 13px; }
    `,
  ],
})
export class ArenaRowComponent {
  @Input() initiativeId = '';
  @Input() phase: {
    phaseId: string;
    phaseTitle?: string;
    effectiveTestCommands?: string[] | null;
  } | null = null;

  protected entry() {
    return arenaEntryRow({ effectiveTestCommands: this.phase?.effectiveTestCommands ?? [] });
  }
}
