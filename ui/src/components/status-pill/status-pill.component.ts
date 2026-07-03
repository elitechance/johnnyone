import { Component, Input, ChangeDetectionStrategy } from '@angular/core';
import { CommonModule } from '@angular/common';
import { statusMeta, healthMeta, StatusMeta } from '../../lib/lifecycle-status';

/**
 * Presentational status/health pill (Overhaul P8 / phase 02, mock `.pill` lines
 * 94-103). Colors itself from the pure `lifecycle-status` map (D5) via the P01
 * `--jo-*` tokens — no inline color logic, no data fetching. `kind` selects the
 * status vs. health map; `value` is the `initiativeStatus` / `health` string.
 */
@Component({
  selector: 'johnny-status-pill',
  standalone: true,
  imports: [CommonModule],
  templateUrl: './status-pill.component.html',
  styleUrls: ['./status-pill.component.scss'],
  changeDetection: ChangeDetectionStrategy.OnPush,
})
export class StatusPillComponent {
  @Input() kind: 'status' | 'health' = 'status';
  @Input() value = '';

  get meta(): StatusMeta {
    return this.kind === 'health' ? healthMeta(this.value) : statusMeta(this.value);
  }

  /** The color the mock's `.pill` reads from `--sc`, resolved to the mapped P01 token. */
  get sc(): string {
    return `var(${this.meta.cssVar})`;
  }
}
