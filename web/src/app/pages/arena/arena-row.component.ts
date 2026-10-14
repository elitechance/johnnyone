import { Component, Input, inject } from '@angular/core';
import { Router, RouterLink } from '@angular/router';
import { arenaReturnParams } from './arena-nav';
import { arenaEntryRow } from './arena-logic';
import { ARENA_CSS } from './arena-ui.css';

/**
 * Mock step 1: ONE extra row on a phase that already exists, inside the console the operator
 * already uses. No new place to go, and no separate "create a workflow" flow — the entry point is
 * the phase, because the prompt being searched for is the one that phase already runs.
 *
 * When the phase has no gate the control STAYS ON SCREEN, disabled, with its reason in text beside
 * it, and a way out (set a gate). A hidden control teaches nobody why this phase cannot compete.
 *
 * The mock draws a crossed-swords glyph on this control. It is not here: in the console's font
 * stack it falls back to a mark that renders as ✕ (measured —
 * `.johnnyone/reports/arena-screens/before-console-arena-tab.png`), so the ENABLED button read as a
 * close button and the disabled one read as "✕ Enter the arena ✕". The words carry it.
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
              [queryParams]="onward({ phaseId: phase.phaseId })"
            >Enter the arena</a>
            <span class="a-muted ar-sub">search for a better prompt for this phase</span>
          </div>
          <div class="mono a-muted ar-cmds">
            @for (c of phase.effectiveTestCommands ?? []; track c) {<div>{{ c }}</div>}
          </div>
        } @else {
          <div class="a-row">
            <button type="button" class="a-btn" data-test="enter-arena-disabled" disabled>Enter the arena · no gate</button>
            <a
              class="ar-set"
              data-test="set-gate"
              [routerLink]="['/initiatives', initiativeId, 'arena']"
              [queryParams]="onward({ phaseId: phase.phaseId, step: 'gate' })"
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
  private readonly router = inject(Router);

  /**
   * This row's own query params PLUS the departure URL, so the setup wizard's back link returns to
   * the console tab the row was clicked from. Merged in one binding: two `[queryParams]` on one
   * anchor is a silent template collision — the second wins and the first is simply dropped.
   */
  protected onward(params: Record<string, string>): Record<string, string> {
    return { ...params, ...arenaReturnParams(this.router.url) };
  }

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
