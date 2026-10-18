import { Component, EventEmitter, Input, Output } from '@angular/core';
// Imported from the service module rather than the `@johnnyone/ui` barrel on purpose: the
// barrel pulls in @ionic/angular, whose `@ionic/core/loader` directory import cannot be
// resolved by vitest's ESM loader, which would make every spec in this folder uncollectable
// (measured). `import type` is erased at build time either way.
import type { TestGateRun } from '../../../../../ui/src/services/johnny-api.service';
import { gateCollectedLabel, gateExitLabel, NO_GATE_REASON } from './arena-logic';
import { ARENA_CSS } from './arena-ui.css';

/**
 * The gate's result, as the Checks tab and the arena screens both show it (mock step 5): the exit
 * code — the ONLY score — and the collected count, which is advisory and nullable and is rendered
 * through `gateCollectedLabel` so a null never appears as `0`.
 *
 * `gateDeclared` is read FIRST: a phase with no gate also exits 0, so "exit 0" alone is not a pass
 * and this says "No gate" instead of claiming one.
 */
@Component({
  selector: 'johnny-gate-strip',
  standalone: true,
  template: `
    <div class="a-card gate-strip">
      <h2>The gate</h2>
      @if (commands.length) {
        <div class="mono" data-test="gate-commands">
          @for (c of commands; track c) {<div>{{ c }}</div>}
        </div>
      }
      @if (gate && !gate.gateDeclared) {
        <p class="a-note" data-test="gate-undeclared">No gate is declared for this phase, at either level. {{ noGateReason }}</p>
      } @else if (gate) {
        <div class="a-row gate-nums">
          <span data-test="gate-exit" [class.a-ok]="gate.exitCode === 0" [class.a-bad]="gate.exitCode !== 0">{{ exitLabel() }}</span>
          <span class="a-muted">·</span>
          <span data-test="gate-collected">{{ collectedLabel() }}</span>
          @if (gate.command) { <span class="a-muted">·</span><span class="mono">{{ gate.command }}</span> }
        </div>
        @if (gate.collected === 0) {
          <p class="a-note" data-test="gate-zero-collected">
            This suite collects nothing. It exits 0 and scores every workflow identically.
          </p>
        }
        @if (gate.output) {
          <pre class="a-out" data-test="gate-output">{{ gate.output }}</pre>
          @if (gate.truncated) { <p class="a-note">Output is a tail window — the host's cap discarded earlier lines.</p> }
        }
      } @else {
        <p class="a-note" data-test="gate-never-run">The gate has not been run from here yet.</p>
      }
      <div class="a-row">
        <button
          type="button"
          class="a-btn"
          data-test="run-gate"
          [disabled]="pending"
          (click)="run.emit()"
        >{{ pending ? 'Running…' : 'Run the gate once' }}</button>
      </div>
    </div>
  `,
  styles: [ARENA_CSS, `.gate-nums { font-size: 14px; } .gate-strip { margin-top: 10px; }`],
})
export class GateStripComponent {
  @Input() gate: TestGateRun | null = null;
  @Input() pending = false;
  @Input() commands: readonly string[] = [];
  @Output() run = new EventEmitter<void>();

  protected readonly noGateReason = NO_GATE_REASON;

  protected exitLabel(): string {
    return gateExitLabel(this.gate?.exitCode);
  }
  protected collectedLabel(): string {
    return gateCollectedLabel(this.gate?.collected);
  }
}
