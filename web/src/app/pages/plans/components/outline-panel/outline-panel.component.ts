import { Component, input } from '@angular/core';

/**
 * One row of the draft outline (presentational): a phase header (`00 Retire on job end`) or a task
 * row (`01 retire helper`), with the `agent v4` tag and the "added in the latest version" highlight.
 * The container projects the row's actions (rename / move / delete / add task) via `[row-actions]`.
 */
@Component({
  selector: 'app-outline-row',
  standalone: true,
  host: { '[class.phase]': "kind() === 'phase'", '[class.task]': "kind() === 'task'", '[class.added]': 'added()' },
  template: `
    <span class="num">{{ num() }}</span>
    <span class="title">{{ title() }}</span>
    @if (tag()) {<span class="tag">{{ tag() }}</span>}
    <span class="actions"><ng-content select="[row-actions]"></ng-content></span>
  `,
  styles: [
    `
      :host {
        display: flex;
        align-items: center;
        gap: 8px;
        padding: 5px 10px;
        font-size: 13px;
        min-height: 34px;
      }
      :host(.phase) {
        font-weight: 600;
        color: var(--jo-fg-strong, var(--ion-text-color));
        border-bottom: 1px solid var(--jo-border, var(--ion-color-step-150));
      }
      :host(.task) {
        padding-left: 18px;
      }
      :host(.added) {
        background: color-mix(in srgb, var(--jo-accent, var(--ion-color-warning)) 10%, transparent);
      }
      .num {
        font-family: var(--ion-font-family-monospace, monospace);
        font-size: 11px;
        color: var(--jo-fg-muted, var(--ion-color-medium));
        min-width: 20px;
      }
      .title {
        flex: 1;
        overflow-wrap: anywhere;
      }
      .tag {
        font: 10.5px var(--ion-font-family-monospace, monospace);
        color: var(--jo-st-briefing, var(--ion-color-tertiary));
        white-space: nowrap;
      }
      .actions {
        display: inline-flex;
        gap: 0;
        margin-left: 2px;
      }
    `,
  ],
})
export class OutlineRowComponent {
  readonly kind = input<'phase' | 'task'>('task');
  readonly num = input('');
  readonly title = input('');
  readonly tag = input<string | null>(null);
  readonly added = input(false);
}
