import { Component, OnInit, computed, inject, signal } from '@angular/core';
import { ActivatedRoute, Router, RouterLink } from '@angular/router';
import { arenaBackTarget, arenaReturnParams } from './arena-nav';
import { firstValueFrom } from 'rxjs';
import { ArenaApiService, type Workflow } from './arena-api.service';
import { ARENA_CSS } from './arena-ui.css';

/** Same three families as the setup screen, with the caps MEASURED on 2026-10-12. */
const FAMILIES = ['glm-5.3-flash', 'deepseek-v4.1-flash', 'qwen3.8-flash-next'];

/**
 * `/workflows` — champions and challengers, grouped by the key that actually identifies a champion:
 * (repo, phaseType, family). This route had no mock drawn for it; it is the list the design says
 * must exist because champions have to be listable somewhere, so it shows exactly what the data
 * says and invents nothing.
 */
@Component({
  selector: 'app-workflows',
  standalone: true,
  imports: [RouterLink],
  template: `
    <div class="aw">
      <div class="a-head">
        <a class="a-back" [routerLink]="back().link" [queryParams]="back().queryParams">{{ back().label }}</a>
        <span class="eyebrow">Workflows</span>
        <h1>Champions and challengers</h1>
      </div>

      <div class="a-card" data-test="fleet-strip">
        <h2>The fleet</h2>
        <p class="a-note">
          Seats in use are <b>not measured</b>: J1 has no lease pool, and the fleet API exposes no
          provider count, no queue depth and no active-user field. Over-running a cap raises no
          error either — requests are batched, not rejected — so there is no number to show and no
          meter is drawn here.
        </p>
        <dl class="a-kv">
          @for (fam of families; track fam) {
            <dt>{{ fam }}</dt><dd class="a-muted">leases held: not measured</dd>
          }
        </dl>
      </div>

      @if (loadError()) { <div class="a-block" data-test="load-error">{{ loadError() }}</div> }

      @if (!loading() && groups().length === 0) {
        <div class="a-card" data-test="empty"><p>No workflows yet. A workflow is created by entering the arena from a phase that has a gate.</p></div>
      }

      @for (g of groups(); track g.key) {
        <div class="a-card">
          <h2>{{ g.repo }} · {{ g.phaseType }} · {{ g.family }}</h2>
          <ul class="a-list">
            @for (w of g.items; track w.id) {
              <li [attr.data-test]="'wf-row-' + w.id">
                <div class="a-row">
                  <b>v{{ w.version }}</b>
                  @if (w.isChampion) { <span class="a-ok" data-test="champion-badge">champion</span> }
                  <span class="a-muted">width {{ w.fanOut }}</span>
                  <a [attr.data-test]="'wf-link-' + w.id" [routerLink]="['/workflows', w.id]" [queryParams]="onward()">the four parts</a>
                  <a [attr.data-test]="'wf-arena-' + w.id" [routerLink]="['/workflows', w.id, 'arena']" [queryParams]="onward()">arena</a>
                </div>
                <div class="a-muted wf-when">{{ w.createdAt }}</div>
              </li>
            }
          </ul>
        </div>
      }
    </div>
  `,
  styles: [ARENA_CSS, `.wf-when { font-size: 12px; } a { margin-right: 4px; }`],
})
export class WorkflowsPage implements OnInit {
  private readonly api = inject(ArenaApiService);
  private readonly router = inject(Router);
  private readonly activated = inject(ActivatedRoute);

  /**
   * Where "back" goes. `?from=` is the URL the person DEPARTED from — every arena-ward link carries
   * it — and the static link below is only the fallback for a pasted URL or a bookmark, which has
   * no history to honour. See `arena-nav.ts` for the four different hard-coded labels this replaces.
   */
  protected back() {
    return arenaBackTarget(this.activated.snapshot.queryParamMap.get('from'), {
      link: ['/initiatives'],
      label: 'the console',
    });
  }

  /** The `?from=` an onward arena link must carry, so the page it opens can come back here. */
  protected onward(): Record<string, string> {
    return arenaReturnParams(this.router.url);
  }
  protected readonly families = FAMILIES;
  protected readonly all = signal<Workflow[]>([]);
  protected readonly loading = signal(true);
  protected readonly loadError = signal<string | null>(null);

  /** Grouped by the champion key, champion first, then newest version first. */
  protected readonly groups = computed(() => {
    const byKey = new Map<string, { key: string; repo: string; phaseType: string; family: string; items: Workflow[] }>();
    for (const w of this.all()) {
      const key = `${w.repo}\u0000${w.phaseType}\u0000${w.family}`;
      const g = byKey.get(key) ?? { key, repo: w.repo, phaseType: w.phaseType, family: w.family, items: [] };
      g.items.push(w);
      byKey.set(key, g);
    }
    for (const g of byKey.values()) {
      g.items.sort((a, b) => (a.isChampion === b.isChampion ? b.version - a.version : a.isChampion ? -1 : 1));
    }
    return [...byKey.values()].sort((a, b) => a.key.localeCompare(b.key));
  });

  async ngOnInit(): Promise<void> {
    try {
      this.all.set(await firstValueFrom(this.api.workflows({})));
    } catch (e) {
      this.loadError.set(`Could not list workflows: ${String(e)}`);
    } finally {
      this.loading.set(false);
    }
  }
}
