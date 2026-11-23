import { Component, input } from '@angular/core';
import { IonChip, IonLabel } from '@ionic/angular/standalone';
import type { VersionChip } from '../../plan-brief-logic';

/**
 * One version chip of the brief's version strip (presentational): `v1 you` … `v5 agent`, coloured by
 * author, outlined when selected. The container loops the chips and binds `(click)` on each host to
 * open that version's diff (`getBriefDiff`), so the API call stays in `plan-detail.page`.
 */
@Component({
  selector: 'app-version-chip',
  standalone: true,
  imports: [IonChip, IonLabel],
  template: `
    <ion-chip
      [outline]="!selected()"
      [color]="chip().author === 'agent' ? 'tertiary' : 'primary'"
      [attr.aria-pressed]="selected()"
      [attr.aria-label]="'Show the changes of version ' + chip().version"
    >
      <ion-label>{{ chip().label }}</ion-label>
    </ion-chip>
  `,
  styles: [
    `
      :host {
        display: inline-block;
      }
      ion-chip {
        margin: 2px;
        height: 26px;
        font: 11px var(--ion-font-family-monospace, monospace);
      }
    `,
  ],
})
export class VersionChipComponent {
  readonly chip = input.required<VersionChip>();
  readonly selected = input(false);
}
