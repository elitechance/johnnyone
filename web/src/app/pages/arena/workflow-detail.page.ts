import { Component, OnInit, computed, inject, signal } from '@angular/core';
import { ActivatedRoute, RouterLink } from '@angular/router';
import { firstValueFrom } from 'rxjs';
import { ArenaApiService, type Workflow } from './arena-api.service';
import { ARENA_CSS } from './arena-ui.css';
import { lineageOf } from './arena-logic';

/**
 * `/workflows/:id` — the four prompt slots and the lineage (mock step 4, read-only).
 *
 * Read-only by design: a version is immutable once it exists, because a run's result is only
 * meaningful against the exact text that produced it. Editing happens by proposing a NEW version
 * from the arena setup, which is what `parentId` records.
 */
@Component({
  selector: 'app-workflow-detail',
  standalone: true,
  imports: [RouterLink],
  template: `
    <div class="aw">
      <div class="a-head">
        <a class="a-back" [routerLink]="['/workflows']">‹ workflows</a>
        <span class="eyebrow">{{ wf()?.repo }} · {{ wf()?.phaseType }} · {{ wf()?.family }}</span>
        <h1>v{{ wf()?.version }} @if (wf()?.isChampion) { <span class="a-ok">· champion</span> }</h1>
      </div>

      @if (loadError()) { <div class="a-block" data-test="load-error">{{ loadError() }}</div> }

      @if (wf(); as w) {
        <div class="a-card">
          <h2>role</h2>
          <div data-test="slot-role" class="slot">{{ w.role }}</div>
        </div>
        <div class="a-card">
          <h2>instruction</h2>
          <div data-test="slot-instruction" class="slot">{{ w.instruction }}</div>
        </div>
        <div class="a-card">
          <h2>contract</h2>
          <div data-test="slot-contract" class="slot">{{ w.contract }}</div>
        </div>
        <div class="a-card">
          <h2>hop</h2>
          <pre class="a-out mono" data-test="slot-hop">{{ w.hop }}</pre>
          <p class="a-note">width {{ w.fanOut }} — {{ w.fanOut }} seats. A prompt must be correct at width 1; parallelism is an optimisation.</p>
        </div>

        <div class="a-card">
          <h2>lineage</h2>
          <p class="a-note">Every version stays here, including one that was rejected — that is what stops the same challenger being proposed again.</p>
          <ul class="a-list">
            @for (l of lineage(); track l.id) {
              <li data-test="lineage-item">
                <b>v{{ l.version }}</b>
                @if (l.isChampion) { <span class="a-ok">champion</span> }
                @if (l.id === w.id) { <span class="a-muted">· this one</span> }
                <a [routerLink]="['/workflows', l.id]">open</a>
              </li>
            } @empty {
              <li class="a-muted">No lineage recorded for this version.</li>
            }
          </ul>
          <a class="a-btn" [routerLink]="['/workflows', w.id, 'arena']">Open the arena</a>
        </div>
      }
    </div>
  `,
  styles: [ARENA_CSS, `.slot { white-space: pre-wrap; overflow-wrap: anywhere; font-size: 13.5px; } a { margin-left: 6px; }`],
})
export class WorkflowDetailPage implements OnInit {
  private readonly api = inject(ArenaApiService);
  private readonly route = inject(ActivatedRoute);
  protected readonly wf = signal<Workflow | null>(null);
  protected readonly siblings = signal<Workflow[]>([]);
  protected readonly loadError = signal<string | null>(null);

  protected readonly lineage = computed(() => {
    const w = this.wf();
    return w ? lineageOf(this.siblings(), w.id) : [];
  });

  async ngOnInit(): Promise<void> {
    const id = this.route.snapshot.paramMap.get('id') ?? '';
    try {
      const w = await firstValueFrom(this.api.workflow(id));
      this.wf.set(w);
      this.siblings.set(
        await firstValueFrom(this.api.workflows({ repo: w.repo, phaseType: w.phaseType, family: w.family })),
      );
    } catch (e) {
      this.loadError.set(`Could not read this workflow: ${String(e)}`);
    }
  }
}
