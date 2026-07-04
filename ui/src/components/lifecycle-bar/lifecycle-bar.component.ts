import { Component, Input, ChangeDetectionStrategy } from '@angular/core';
import { CommonModule } from '@angular/common';
import { StatusPillComponent } from '../status-pill/status-pill.component';
import {
  LIFECYCLE_STAGES,
  statusMeta,
  stageIndex,
} from '../../lib/lifecycle-status';

/** One lifecycle-bar cell, precomputed for the template. */
interface StageCell {
  key: string;
  num: string;
  label: string;
  className: string;
  sc: string;
  description: string;
  on: boolean;
  /** Progress-bar fill width: complete stages `100%`, the active stage a partial, later stages `0%`. */
  fill: string;
}

/** Static per-stage copy mirroring mock §01 (`.stage .d`). */
const STAGE_COPY: Record<string, string> = {
  briefing: 'Clarify scope with a back-and-forth until the brief is accepted.',
  planning: 'Turn the accepted brief into a task tree.',
  development: 'T1 workers build against the plan.',
  review: 'Configurable validation lenses run in parallel.',
  // "Done" = work complete (all phases validated). J1 does not auto-merge/deploy,
  // so the copy reflects completion, not promotion.
  done: 'Work complete.',
};

/**
 * Presentational initiative lifecycle bar (Overhaul P8 / phase 02, mock §01 lines
 * 488-522). Renders the five `LIFECYCLE_STAGES` in order with the active stage
 * `.on` in its semantic color, earlier stages complete and later ones inert, plus
 * a `health` pill on the axis below. All color/label decisions come from the pure
 * `lifecycle-status` map (D5) via the P01 tokens — no inline color logic.
 */
@Component({
  selector: 'johnny-lifecycle-bar',
  standalone: true,
  imports: [CommonModule, StatusPillComponent],
  templateUrl: './lifecycle-bar.component.html',
  styleUrls: ['./lifecycle-bar.component.scss'],
  changeDetection: ChangeDetectionStrategy.OnPush,
})
export class LifecycleBarComponent {
  @Input() status = '';
  @Input() health = '';

  get stages(): StageCell[] {
    const active = stageIndex(this.status);
    return LIFECYCLE_STAGES.map((stage, idx) => {
      const meta = statusMeta(stage);
      const fill = active < 0 || idx > active ? '0%' : idx < active ? '100%' : '60%';
      return {
        key: stage,
        num: String(idx).padStart(2, '0'),
        label: meta.label,
        className: meta.className,
        sc: `var(${meta.cssVar})`,
        description: STAGE_COPY[stage] ?? '',
        on: idx === active,
        fill,
      };
    });
  }
}
