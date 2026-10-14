import { Component, OnInit, computed, inject, signal } from '@angular/core';
import { ActivatedRoute, Router, RouterLink } from '@angular/router';
import { WORKFLOW_NOT_FOUND, arenaBackTarget, arenaReturnParams, isMissingWorkflow } from './arena-nav';
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
        <a class="a-back" [routerLink]="back().link" [queryParams]="back().queryParams">{{ back().label }}</a>
        <span class="eyebrow">{{ wf()?.repo }} · {{ wf()?.phaseType }} · {{ wf()?.family }}</span>
        <h1>v{{ wf()?.version }} @if (wf()?.isChampion) { <span class="a-ok">· champion</span> }</h1>
      </div>

      @if (loadError()) { <div class="a-block" data-test="load-error">{{ loadError() }}</div> }

      @if (notFound()) {
        <div class="a-card" data-test="not-found">
          <p>{{ notFoundCopy }}</p>
          <div class="a-row">
            <a class="a-btn" data-test="not-found-out" [routerLink]="['/workflows']">All workflows</a>
          </div>
        </div>
      } @else {

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
                <a [routerLink]="['/workflows', l.id]" [queryParams]="onward()">open</a>
              </li>
            } @empty {
              <li class="a-muted">No lineage recorded for this version.</li>
            }
          </ul>
          <a class="a-btn" [routerLink]="['/workflows', w.id, 'arena']" [queryParams]="onward()">Open the arena</a>
        </div>
      }
      }
    </div>
  `,
  styles: [ARENA_CSS, `.slot { white-space: pre-wrap; overflow-wrap: anywhere; font-size: 13.5px; } a { margin-left: 6px; }`],
})
export class WorkflowDetailPage implements OnInit {
  private readonly api = inject(ArenaApiService);
  private readonly route = inject(ActivatedRoute);
  private readonly router = inject(Router);

  /**
   * Where "back" goes. `?from=` is the URL the person DEPARTED from — every arena-ward link carries
   * it — and the static link below is only the fallback for a pasted URL or a bookmark, which has
   * no history to honour. See `arena-nav.ts` for the four different hard-coded labels this replaces.
   */
  protected back() {
    return arenaBackTarget(this.route.snapshot.queryParamMap.get('from'), {
      link: ['/workflows'],
      label: 'workflows',
    });
  }

  /** The `?from=` an onward arena link must carry, so the page it opens can come back here. */
  protected onward(): Record<string, string> {
    return arenaReturnParams(this.router.url);
  }
  protected readonly wf = signal<Workflow | null>(null);
  protected readonly siblings = signal<Workflow[]>([]);
  protected readonly loadError = signal<string | null>(null);

  /**
   * The id in the URL is not a workflow. Separate from `loadError` on purpose: an absence has a
   * way out (the list), a failure does not, and conflating them is what put a raw TypeError on the
   * screen. `WORKFLOW_NOT_FOUND` carries the words.
   */
  protected readonly notFound = signal(false);
  protected readonly notFoundCopy = WORKFLOW_NOT_FOUND;

  protected readonly lineage = computed(() => {
    const w = this.wf();
    return w ? lineageOf(this.siblings(), w.id) : [];
  });

  async ngOnInit(): Promise<void> {
    const id = this.route.snapshot.paramMap.get('id') ?? '';
    try {
      const w = await firstValueFrom(this.api.workflow(id));
      // AN ABSENCE IS NOT A FAILURE. The host answers `null` for an id that is not a workflow — a
      // stale link, a bookmark, a removed challenger — and the old code let that `null` run on into
      // `w.repo`, threw inside this try, and printed the TypeError as body copy (measured on
      // /workflows/w9: "Could not read this workflow: TypeError: Cannot read properties of null").
      if (isMissingWorkflow(w)) {
        this.notFound.set(true);
        return;
      }
      this.wf.set(w);
      this.siblings.set(
        await firstValueFrom(this.api.workflows({ repo: w.repo, phaseType: w.phaseType, family: w.family })),
      );
    } catch (e) {
      this.loadError.set(`Could not read this workflow: ${String(e)}`);
    }
  }
}
