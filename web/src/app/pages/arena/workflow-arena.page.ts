import { Component, OnInit, computed, inject, signal } from '@angular/core';
import { ActivatedRoute, RouterLink } from '@angular/router';
import { firstValueFrom } from 'rxjs';
import {
  JohnnyApiService,
  type Workflow,
  type WorkflowPref,
  type WorkflowRun,
  type WorkflowSearchState,
} from '../../../../../ui/src/services/johnny-api.service';
import { ARENA_CSS } from './arena-ui.css';
import {
  changedSlots,
  gateCollectedLabel,
  gateExitLabel,
  latestRunOf,
  promotionBlock,
  promotionConfirmCopy,
  rejectionReasonValid,
  startRunBlock,
} from './arena-logic';

/**
 * `/workflows/:id/arena` — champion vs challenger (mock steps 5, 7 and 8).
 *
 * Three things this screen is careful about:
 *
 *  * THE SCORE IS THE GATE'S EXIT CODE. Timing is shown and greyed, and says why it never scores.
 *    There is no composite, no rating and no percentage anywhere on it.
 *  * COMPARABILITY IS THE HOST'S. `WorkflowPref.comparable` is computed by the host from the two
 *    runs' seat budgets and is not an argument, so this screen cannot assert it — it asks the host
 *    to record the head-to-head (the explicit "Compare" control) and then reads the verdict. There
 *    is no `workflowPrefs` QUERY in the schema, so a preference recorded in an earlier visit cannot
 *    be read back; recording it is the only way to hold one. That is a real gap, reported rather
 *    than worked around by re-deriving comparability here.
 *  * READING IS NOT SCORING. The documents live in the Plan tab, which this links to. No second
 *    document browser, no second markdown renderer, no second iframe sandbox.
 */
@Component({
  selector: 'app-workflow-arena',
  standalone: true,
  imports: [RouterLink],
  template: `
    <div class="aw">
      <div class="a-head">
        <a class="a-back" [routerLink]="['/workflows', challenger()?.id]">‹ the four parts</a>
        <span class="eyebrow">{{ challenger()?.repo }} · {{ challenger()?.phaseType }} · {{ challenger()?.family }}</span>
        <h1>champion vs challenger</h1>
      </div>

      @if (loadError()) { <div class="a-block" data-test="load-error">{{ loadError() }}</div> }

      @if (haltBlock(); as block) {
        <div class="a-block" data-test="halt-block">
          <b>The search is paused.</b>
          <p class="a-note">{{ block }}</p>
          @if (searchState()?.recentRejections?.length) {
            <ul class="a-list">@for (r of searchState()!.recentRejections; track r) {<li>{{ r }}</li>}</ul>
          }
        </div>
      }

      <div class="a-cols">
        <div class="a-card">
          <h2>champion @if (champion()) { <span class="a-muted">v{{ champion()!.version }}</span> }</h2>
          @if (championRun(); as r) {
            <dl class="a-kv">
              <dt>gate</dt><dd [class.a-ok]="r.gateExit === 0" [class.a-bad]="r.gateExit !== 0" data-test="champion-exit">{{ exitLabel(r.gateExit) }}</dd>
              <dt>collected</dt><dd data-test="champion-collected">{{ collectedLabel(r.gateCollected) }}</dd>
              <dt>seats</dt><dd class="mono">{{ r.seatsHeld }}</dd>
              <dt class="a-muted">took</dt><dd class="a-muted" data-test="champion-took">{{ took(r) }}</dd>
            </dl>
          } @else {
            <p class="a-note" data-test="champion-no-run">No run recorded for the champion yet. That is an absence, not a pass.</p>
          }
        </div>

        <div class="a-card">
          <h2>challenger @if (challenger()) { <span class="a-muted">v{{ challenger()!.version }}</span> }</h2>
          @if (challengerRun(); as r) {
            <dl class="a-kv">
              <dt>gate</dt><dd [class.a-ok]="r.gateExit === 0" [class.a-bad]="r.gateExit !== 0" data-test="challenger-exit">{{ exitLabel(r.gateExit) }}</dd>
              <dt>collected</dt><dd data-test="challenger-collected">{{ collectedLabel(r.gateCollected) }}</dd>
              <dt>seats</dt><dd class="mono">{{ r.seatsHeld }}</dd>
              <dt class="a-muted">took</dt><dd class="a-muted" data-test="challenger-took">{{ took(r) }}</dd>
            </dl>
          } @else {
            <p class="a-note" data-test="challenger-no-run">No run recorded for this challenger yet.</p>
          }
        </div>
      </div>

      <p class="a-note">Time is shown and greyed: it depends on who else is on the box, so it never scores. The gate's exit code is the whole of it.</p>

      <div class="a-card">
        <h2>what the challenger changed</h2>
        @if (changes().length) {
          <ul class="a-list">
            @for (c of changes(); track c.slot) {
              <li data-test="changed-slot" [attr.data-slot]="c.slot">
                <b>{{ c.slot }}</b>
                <div class="a-muted was">was: {{ c.from }}</div>
                <div class="now">now: {{ c.to }}</div>
              </li>
            }
          </ul>
          <p class="a-note">Which of the four parts changed — not a text diff of a whole prompt.</p>
        } @else {
          <p class="a-note" data-test="no-changes">Nothing differs from the champion: this is a replay of the same prompt, not a challenger.</p>
        }
      </div>

      <div class="a-card">
        <h2>read the work</h2>
        <p class="a-note">Two prompts running the same phase produce two sets of documents. They are in the Plan tab, where every document already lives — reading them writes nothing here.</p>
        <a
          class="a-btn"
          data-test="read-the-work"
          [routerLink]="['/initiatives']"
          [queryParams]="{ initiativeId: challengerRun()?.planId || '', tab: 'plan' }"
        >Open the Plan tab</a>
      </div>

      <div class="a-card">
        <h2>replay</h2>
        <p class="a-note">One green run can be luck. Replays are serial and in the same family.</p>
        <button type="button" class="a-btn" data-test="replay" [disabled]="haltBlock() !== null || replaying()" (click)="replay()">{{ replaying() ? 'Starting…' : 'Run this challenger again' }}</button>
        @if (replayError()) { <div class="a-block" data-test="replay-error">{{ replayError() }}</div> }
      </div>

      <div class="a-card">
        <h2>your one decision</h2>
        @if (!pref()) {
          <p class="a-note" data-test="compare-note">Record the head-to-head first. Whether the two sides are comparable is the host's answer, computed from the two runs' seat budgets — this screen cannot decide it.</p>
          <button type="button" class="a-btn" data-test="compare" [disabled]="!bothRunsScored() || comparing()" (click)="compare()">{{ comparing() ? 'Recording…' : 'Record this comparison' }}</button>
        } @else {
          <dl class="a-kv">
            <dt>comparable</dt>
            <dd [class.a-ok]="pref()!.comparable" [class.a-bad]="!pref()!.comparable" data-test="pref-comparable">{{ pref()!.comparable ? 'yes — the seat budgets matched' : 'no — the seat budgets differed' }}</dd>
          </dl>
        }

        @if (promoteBlock(); as block) {
          <div class="a-block" data-test="promote-block">{{ block }}</div>
        }

        <div class="a-row">
          <button type="button" class="a-btn primary" data-test="promote" [disabled]="promoteBlock() !== null || promoting()" (click)="openConfirm()">Promote the challenger</button>
          <button type="button" class="a-btn danger" data-test="reject" (click)="rejecting.set(true)">Keep the champion</button>
        </div>

        @if (confirming()) {
          <div class="a-warn" data-test="promote-confirm">
            <b>{{ confirmCopy() }}</b>
            <p class="a-note">Promotion is the only step in the arena a human has to take, and it is not undone by a later run.</p>
            <div class="a-row">
              <button type="button" class="a-btn" (click)="confirming.set(false)">Keep the champion for now</button>
              <button type="button" class="a-btn primary" data-test="promote-do" [disabled]="promoting()" (click)="promote()">{{ promoting() ? 'Promoting…' : 'Yes, promote it' }}</button>
            </div>
          </div>
        }
        @if (promoteError()) { <div class="a-block" data-test="promote-error">{{ promoteError() }}</div> }

        @if (rejecting()) {
          <div class="a-card" data-test="reject-form">
            <h2>saying no</h2>
            <p class="a-note">A challenger that passed and was rejected anyway means the gate is missing something you care about. There is no bare no: write the reason, and it stays in the lineage so the same challenger is never proposed again.</p>
            <textarea class="a-input" data-test="reject-reason" [value]="reason()" (input)="reason.set($any($event.target).value)"></textarea>
            @if (!reasonValid().ok) { <p class="a-note" data-test="reject-hint">{{ reasonValid().message }}</p> }
            <div class="a-row">
              <button type="button" class="a-btn" (click)="rejecting.set(false)">Cancel</button>
              <button type="button" class="a-btn danger" data-test="reject-send" [disabled]="!reasonValid().ok || rejectSending()" (click)="reject()">{{ rejectSending() ? 'Recording…' : 'Record the rejection' }}</button>
            </div>
            @if (rejectError()) { <div class="a-block" data-test="reject-error">{{ rejectError() }}</div> }
          </div>
        }
      </div>
    </div>
  `,
  styles: [
    ARENA_CSS,
    `
      .was, .now { font-size: 12.5px; overflow-wrap: anywhere; }
      h2 { margin-top: 0; }
    `,
  ],
})
export class WorkflowArenaPage implements OnInit {
  private readonly api = inject(JohnnyApiService);
  private readonly route = inject(ActivatedRoute);

  protected readonly challenger = signal<Workflow | null>(null);
  protected readonly champion = signal<Workflow | null>(null);
  protected readonly challengerRun = signal<WorkflowRun | null>(null);
  protected readonly championRun = signal<WorkflowRun | null>(null);
  protected readonly searchState = signal<WorkflowSearchState | null>(null);
  protected readonly pref = signal<WorkflowPref | null>(null);
  protected readonly loadError = signal<string | null>(null);
  protected readonly comparing = signal(false);
  protected readonly confirming = signal(false);
  protected readonly promoting = signal(false);
  protected readonly promoteError = signal<string | null>(null);
  protected readonly rejecting = signal(false);
  protected readonly rejectSending = signal(false);
  protected readonly rejectError = signal<string | null>(null);
  protected readonly reason = signal('');
  protected readonly replaying = signal(false);
  protected readonly replayError = signal<string | null>(null);

  protected readonly haltBlock = computed(() => startRunBlock(this.searchState()));
  protected readonly reasonValid = computed(() => rejectionReasonValid(this.reason()));
  protected readonly changes = computed(() => {
    const champ = this.champion();
    const chal = this.challenger();
    return champ && chal ? changedSlots(champ, chal) : [];
  });
  protected readonly bothRunsScored = computed(() => {
    const a = this.challengerRun();
    const b = this.championRun();
    return !!a && a.gateExit !== null && a.gateExit !== undefined && !!b && b.gateExit !== null && b.gateExit !== undefined;
  });
  protected readonly promoteBlock = computed(() => {
    const run = this.challengerRun();
    if (!run) return 'This challenger has no run yet, so there is nothing to promote.';
    return promotionBlock({ run, pref: this.pref(), incumbentExists: this.champion() !== null });
  });
  protected readonly confirmCopy = computed(() => {
    const w = this.challenger();
    return w
      ? promotionConfirmCopy({ repo: w.repo, phaseType: w.phaseType, family: w.family, version: w.version })
      : '';
  });

  protected exitLabel(exit?: number | null): string {
    return gateExitLabel(exit);
  }
  protected collectedLabel(collected?: number | null): string {
    return gateCollectedLabel(collected);
  }
  /** Shown and greyed. Never a score. */
  protected took(run: WorkflowRun): string {
    if (!run.endedAt) return 'still running';
    const ms = Date.parse(run.endedAt) - Date.parse(run.startedAt);
    if (!Number.isFinite(ms) || ms < 0) return 'not recorded';
    const s = Math.round(ms / 1000);
    return `${Math.floor(s / 60)}m${String(s % 60).padStart(2, '0')}s`;
  }

  async ngOnInit(): Promise<void> {
    const id = this.route.snapshot.paramMap.get('id') ?? '';
    try {
      const chal = await firstValueFrom(this.api.workflow(id));
      this.challenger.set(chal);
      const family = await firstValueFrom(
        this.api.workflows({ repo: chal.repo, phaseType: chal.phaseType, family: chal.family }),
      );
      const champ = family.find((w) => w.isChampion && w.id !== chal.id) ?? null;
      this.champion.set(champ);
      this.challengerRun.set(latestRunOf(await firstValueFrom(this.api.workflowRuns(chal.id))));
      if (champ) {
        this.championRun.set(latestRunOf(await firstValueFrom(this.api.workflowRuns(champ.id))));
      }
      this.searchState.set(
        await firstValueFrom(this.api.workflowSearchState(chal.repo, chal.phaseType, chal.family)),
      );
    } catch (e) {
      this.loadError.set(`Could not read this arena: ${String(e)}`);
    }
  }

  protected async compare(): Promise<void> {
    const win = this.challengerRun();
    const lose = this.championRun();
    if (!win || !lose || this.comparing()) return;
    this.comparing.set(true);
    try {
      this.pref.set(await firstValueFrom(this.api.recordWorkflowPref(win.id, lose.id, null)));
    } catch (e) {
      this.loadError.set(`The comparison could not be recorded: ${String(e)}`);
    } finally {
      this.comparing.set(false);
    }
  }

  protected openConfirm(): void {
    // Guarded as well as disabled: the block is the rule, the attribute is only its shadow.
    if (this.promoteBlock() !== null) return;
    this.confirming.set(true);
  }

  protected async promote(): Promise<void> {
    if (this.promoteBlock() !== null || this.promoting()) return;
    const run = this.challengerRun();
    if (!run) return;
    this.promoting.set(true);
    this.promoteError.set(null);
    try {
      const promoted = await firstValueFrom(
        this.api.updateWorkflowChampion(run.id, this.pref()?.id ?? null, true),
      );
      this.champion.set(promoted);
      this.challenger.set(promoted);
      this.confirming.set(false);
    } catch (e) {
      this.promoteError.set(`The host refused the promotion: ${String(e)}`);
    } finally {
      this.promoting.set(false);
    }
  }

  protected async reject(): Promise<void> {
    const run = this.challengerRun();
    if (!this.reasonValid().ok || !run || this.rejectSending()) return;
    this.rejectSending.set(true);
    this.rejectError.set(null);
    try {
      // The host answers with the resulting search state, so the halt is data this screen already
      // holds — it is never recounted here.
      this.searchState.set(await firstValueFrom(this.api.recordWorkflowRejection(run.id, this.reason())));
      this.rejecting.set(false);
    } catch (e) {
      this.rejectError.set(`The rejection could not be recorded: ${String(e)}`);
    } finally {
      this.rejectSending.set(false);
    }
  }

  protected async replay(): Promise<void> {
    const w = this.challenger();
    const run = this.challengerRun();
    if (!w || this.haltBlock() !== null || this.replaying()) return;
    this.replaying.set(true);
    this.replayError.set(null);
    try {
      const next = await firstValueFrom(
        this.api.startWorkflowRun({
          workflowId: w.id,
          planId: run?.planId ?? null,
          seed: (run?.seed ?? 0) + 1,
          seatsHeld: run?.seatsHeld ?? JSON.stringify({ [w.family]: w.fanOut }),
          fleetBusy: JSON.stringify({}),
        }),
      );
      this.challengerRun.set(next);
    } catch (e) {
      this.replayError.set(`The host refused to start the run: ${String(e)}`);
    } finally {
      this.replaying.set(false);
    }
  }
}
