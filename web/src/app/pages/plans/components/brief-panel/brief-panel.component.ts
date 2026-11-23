import { Component, input } from '@angular/core';
import type { FieldRow } from '../../plan-brief-logic';

/**
 * One field of the structured brief (presentational): label, provenance (`agent · v4 · just now`),
 * the `changed` highlight, and the value (lines or `where` chips). The container projects the action
 * buttons (`[field-actions]`: undo / edit) and the inline editor (`[field-editor]`), so every handler
 * — and every API call — stays in `plan-detail.page`.
 */
@Component({
  selector: 'app-brief-field',
  standalone: true,
  host: { class: 'brief-field', '[class.changed]': 'row().changed', '[class.editing]': 'editing()' },
  template: `
    <div class="fl">
      <span class="name">{{ row().label }}</span>
      <span class="who">
        <span class="author" [class.agent]="row().who === 'agent'">{{ row().who }}</span>
        · v{{ row().version }}@if (row().ago) {<span class="ago"> · {{ row().ago }}</span>}
      </span>
      <span class="actions"><ng-content select="[field-actions]"></ng-content></span>
    </div>
    @if (editing()) {
      <ng-content select="[field-editor]"></ng-content>
    } @else if (row().empty) {
      <div class="fv empty">—</div>
    } @else if (row().isList) {
      <div class="chips">
        @for (c of row().chips; track c) {<code class="chip">{{ c }}</code>}
      </div>
    } @else {
      <div class="fv" [class.mono]="row().key === 'done_when'">
        @for (l of row().lines; track $index) {<div class="ln">{{ l }}</div>}
      </div>
    }
  `,
  styles: [
    `
      :host {
        display: block;
        border: 1px solid var(--jo-border, var(--ion-color-step-150));
        border-radius: 10px;
        padding: 8px 10px;
        margin-bottom: 8px;
        background: var(--jo-bg-elevated, var(--ion-item-background));
      }
      :host(.changed) {
        border-color: color-mix(in srgb, var(--jo-accent, var(--ion-color-warning)) 55%, transparent);
        background: color-mix(in srgb, var(--jo-accent, var(--ion-color-warning)) 7%, var(--jo-bg-elevated, transparent));
      }
      .fl {
        display: flex;
        flex-wrap: wrap;
        gap: 6px;
        align-items: center;
        font: 600 10.5px var(--ion-font-family-monospace, monospace);
        letter-spacing: 0.07em;
        text-transform: uppercase;
        color: var(--jo-fg-muted, var(--ion-color-medium));
      }
      .who {
        font-weight: 400;
        letter-spacing: 0;
        text-transform: none;
      }
      .author.agent {
        color: var(--jo-st-briefing, var(--ion-color-tertiary));
      }
      .actions {
        margin-left: auto;
        display: inline-flex;
        gap: 2px;
        text-transform: none;
        letter-spacing: 0;
      }
      .fv {
        margin-top: 4px;
        font-size: 13px;
        color: var(--jo-fg, var(--ion-text-color));
        white-space: pre-wrap;
        overflow-wrap: anywhere;
      }
      .fv.mono {
        font-family: var(--ion-font-family-monospace, monospace);
        font-size: 12px;
      }
      .fv.empty {
        color: var(--jo-fg-faint, var(--ion-color-medium));
      }
      .chips {
        display: flex;
        flex-wrap: wrap;
        gap: 6px;
        margin-top: 6px;
      }
      .chip {
        font-size: 11.5px;
        padding: 1px 6px;
        border-radius: 4px;
        border: 1px solid var(--jo-border, var(--ion-color-step-150));
        background: var(--jo-bg-raised, var(--ion-color-step-50));
        overflow-wrap: anywhere;
      }
    `,
  ],
})
export class BriefFieldComponent {
  readonly row = input.required<FieldRow>();
  readonly editing = input(false);
}
