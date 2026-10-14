import { Component, OnInit, computed, inject, signal } from '@angular/core';
import { ActivatedRoute, RouterLink } from '@angular/router';
import { catchError, firstValueFrom, of } from 'rxjs';
import {
  JohnnyApiService,
  type AgentPlanPhase,
} from '../../../../../ui/src/services/johnny-api.service';
import {
  ArenaApiService,
  type Workflow,
  type WorkflowPref,
  type WorkflowRun,
  type WorkflowSearchState,
} from './arena-api.service';
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
 *    to READ the preference back (`workflowPrefs`) and, only when there is none, offers an explicit
 *    "Compare" control to record one. It never records a row merely because a page was opened.
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
          @if (!resuming()) {
            <button type="button" class="a-btn" data-test="resume" (click)="resuming.set(true)">Lift the halt</button>
          } @else {
            <p class="a-note">Lifting the halt is its own decision and it is recorded with a reason. A promotion does not lift it — that is how a halt used to be escaped with no new evidence.</p>
            <textarea class="a-input" data-test="resume-reason" [value]="resumeReason()" (input)="resumeReason.set($any($event.target).value)"></textarea>
            @if (!resumeReasonValid().ok) { <p class="a-note" data-test="resume-hint">{{ resumeReasonValid().message }}</p> }
            <div class="a-row">
              <button type="button" class="a-btn" (click)="resuming.set(false)">Cancel</button>
              <button type="button" class="a-btn primary" data-test="resume-send" [disabled]="!resumeReasonValid().ok || resumeSending()" (click)="resume()">{{ resumeSending() ? 'Recording…' : 'Resume the search' }}</button>
            </div>
            @if (resumeError()) { <div class="a-block" data-test="resume-error">{{ resumeError() }}</div> }
          }
        </div>
      }

      <div class="a-card">
        <h2>score this run</h2>
        @if (runUnscored()) {
          <p class="a-note" data-test="no-executor-note">
            Nothing scores a run <b>automatically</b>. The coordinator does not trigger this — you
            do, here. Until it is scored the run's gate reads "never ran", which is not a pass and
            cannot be promoted.
          </p>
        }
        @if (hasGateToScore()) {
          @if (phases().length > 1) {
            <label class="a-note" for="scorephase">which phase's gate</label>
            <select id="scorephase" class="a-input" data-test="score-phase" [value]="scorePhaseId()" (change)="scorePhaseId.set($any($event.target).value)">
              @for (ph of phases(); track ph.phaseId) {
                <option [value]="ph.phaseId">{{ ph.phaseId }} — {{ ph.phaseTitle }}</option>
              }
            </select>
          }
          <div class="mono" data-test="score-commands">
            @for (c of scoreCommands(); track c) {<div>{{ c }}</div>}
          </div>
          <p class="a-note">
            The host runs this in the plan's workspace. It is a real test command and
            <b>can take minutes</b>; the exit code it returns is the whole score.
          </p>
          @if (scoreBlock(); as block) {
            <div class="a-block" data-test="score-block">{{ block }}</div>
          }
          @if (runUnscored()) {
            <button
              type="button"
              class="a-btn primary"
              data-test="score-run"
              [disabled]="scoring() || scoreBlock() !== null"
              (click)="scoreRun()"
            >{{ scoring() ? 'Running the gate…' : 'Run the gate on this run' }}</button>
            @if (scoring()) {
              <p class="a-note" data-test="score-running">Still running. No result has arrived, and none is shown until it does.</p>
            }
          } @else {
            @if (!rescoring()) {
              <button
                type="button"
                class="a-btn"
                data-test="rescore-run"
                [disabled]="scoring() || scoreBlock() !== null"
                (click)="rescoring.set(true)"
              >Run the gate again</button>
            } @else {
              <div class="a-warn" data-test="rescore-confirm">
                <b>This will REPLACE the recorded score for this run.</b> The run a sitting champion
                was promoted on cannot be re-scored at all — editing the evidence under a promotion
                that already happened is falsification, not correction.
                <div class="a-row">
                  <button type="button" class="a-btn" (click)="rescoring.set(false)">Keep the recorded score</button>
                  <button type="button" class="a-btn danger" data-test="rescore-do" [disabled]="scoring()" (click)="rescoreRun()">{{ scoring() ? 'Running the gate…' : 'Run it again and replace' }}</button>
                </div>
                @if (scoring()) {
                  <p class="a-note" data-test="score-running">Still running. No result has arrived, and none is shown until it does.</p>
                }
              </div>
            }
          }
        } @else {
          <p class="a-note" data-test="no-gate-to-score">
            No test command on this phase, at either level. The winner is decided by a command, not
            by an opinion, so there is nothing to run and nothing to compete for yet — and an exit
            code of 0 comes back from a phase with no gate exactly as it does from one that passed,
            which is why this run is left unscored rather than scored 0.
          </p>
        }
        @if (scoreError()) { <div class="a-block" data-test="score-error">{{ scoreError() }}</div> }
      </div>

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
  private readonly api = inject(ArenaApiService);
  /** The PLAN's gate, for the scoring step: `effectiveTestCommands` is the host's resolution of
   *  phase-then-plan, and a phase with no gate must not be offered a scoring control at all. */
  private readonly plans = inject(JohnnyApiService);
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
  protected readonly phases = signal<AgentPlanPhase[]>([]);
  protected readonly scorePhaseId = signal<string | null>(null);
  protected readonly scoring = signal(false);
  protected readonly scoreError = signal<string | null>(null);
  protected readonly rescoring = signal(false);
  protected readonly resuming = signal(false);
  protected readonly resumeSending = signal(false);
  protected readonly resumeReason = signal('');
  protected readonly resumeError = signal<string | null>(null);
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
    // A gate in flight is about to REPLACE this run's score. Promoting now would crown a champion
    // on a number that is being overwritten as the promotion is recorded — the same ordering this
    // screen calls falsification when it happens the other way round.
    if (this.scoring()) {
      return 'The gate is running on this run. Its score is about to change, so promotion is held until the result arrives — a champion crowned on a score that is being replaced is a champion crowned on nothing.';
    }
    // A halted search refuses a promotion as well as a run: a promotion used to be the way OUT of
    // a halt, which made the halt escapable with no new evidence. Lifting it is its own decision.
    const halt = this.haltBlock();
    if (halt) return halt;
    return promotionBlock({ run, pref: this.pref(), incumbentExists: this.champion() !== null });
  });
  /** True while a run exists but carries no score — which, today, is every run this UI creates. */
  protected readonly runUnscored = computed(() => {
    const run = this.challengerRun();
    return !!run && (run.gateExit === null || run.gateExit === undefined);
  });
  protected readonly resumeReasonValid = computed(() => rejectionReasonValid(this.resumeReason()));
  /** The phase whose gate scoring will run, and the commands it will actually run. */
  protected readonly scorePhase = computed(
    () => this.phases().find((p) => p.phaseId === this.scorePhaseId()) ?? null,
  );
  protected readonly scoreCommands = computed<string[]>(
    () => this.scorePhase()?.effectiveTestCommands ?? [],
  );
  /**
   * Why this run cannot be scored, or null when it can. Every clause reads a HOST answer:
   *   * `scoreable` / `notScoreableReason` are derived by the host from the run's fleet contention;
   *   * the halt is the host's and is what `runWorkflowRunGate` itself refuses on;
   *   * `effectiveTestCommands` is the host's phase-then-plan resolution — and an exit code of 0
   *     comes back both from a gate that passed and from a phase with no gate, so a run with no
   *     gate must never be scored rather than scored 0.
   */
  protected readonly scoreBlock = computed<string | null>(() => {
    const run = this.challengerRun();
    if (!run) return 'There is no run to score.';
    const halt = this.haltBlock();
    if (halt) return halt;
    if (run.scoreable === false) {
      return run.notScoreableReason ?? 'The host will not score this run.';
    }
    return null;
  });
  /** True when there is a gate to run at all. No gate ⇒ no control, and a sentence saying why. */
  protected readonly hasGateToScore = computed(() => this.scoreCommands().length > 0);
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
      // READ any existing head-to-head rather than recording one to discover it. The pair that
      // matters is this challenger's run against the champion's; anything else in the list is about
      // other runs and must not enable a promotion.
      // The plan behind this run, for its gate. `effectiveTestCommands` is the host's resolution;
      // nothing here re-implements the phase-then-plan fallback.
      const planId = this.challengerRun()?.planId;
      if (planId) {
        try {
          const run = await firstValueFrom(this.plans.getAgentPlan(planId));
          this.phases.set(run?.phases ?? []);
          this.scorePhaseId.set(
            run?.plan?.currentPhaseId ?? (run?.phases ?? [])[0]?.phaseId ?? null,
          );
        } catch {
          // A plan that cannot be read leaves no gate to offer, which the template states.
          this.phases.set([]);
        }
      }
      const prefs = await firstValueFrom(this.api.workflowPrefs(chal.id));
      const mine = this.challengerRun()?.id;
      const theirs = this.championRun()?.id;
      this.pref.set(
        prefs.find((pr) => pr.winnerRunId === mine && (theirs ? pr.loserRunId === theirs : true)) ?? null,
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
    // `promoteBlock()` already covers a gate in flight; this reads it rather than re-listing the
    // conditions, so the button, the confirm and the action can never disagree.
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

  /**
   * Run this run's gate and record the exit code. The host executes the plan's declared gate in the
   * plan's workspace, so this can take minutes — the template says so and claims nothing until the
   * result arrives.
   */
  protected async scoreRun(): Promise<void> {
    const run = this.challengerRun();
    if (!run || this.scoring() || this.scoreBlock() !== null || !this.hasGateToScore()) return;
    this.confirming.set(false);
    this.scoring.set(true);
    this.scoreError.set(null);
    // The refusal is handled IN THE STREAM rather than as a rejected promise: a promise rejection
    // raised inside a DOM event task is reported by zone.js/testing as an uncaught error even when
    // it is caught, which fails a run whose assertions all pass. Same behaviour, no phantom.
    const scored = await firstValueFrom(
      this.api.runWorkflowRunGate(run.id, this.scorePhaseId()).pipe(
        catchError((e: unknown) => {
          // The run keeps whatever score it had — for a fresh run that is none, "never ran".
          this.scoreError.set(`The host refused to score this run: ${String(e)}`);
          return of(null);
        }),
      ),
    );
    if (scored) this.challengerRun.set(scored);
    this.scoring.set(false);
  }

  /** Run the gate AGAIN and REPLACE the recorded result. Its own mutation and its own confirm:
   *  there is no flag to forget, and overwriting a recorded score is not an ordinary action. */
  protected async rescoreRun(): Promise<void> {
    const run = this.challengerRun();
    if (!run || this.scoring() || this.scoreBlock() !== null || !this.hasGateToScore()) return;
    // Close any open promotion confirm: the score it was about to act on is being replaced.
    this.confirming.set(false);
    this.scoring.set(true);
    this.scoreError.set(null);
    const scored = await firstValueFrom(
      this.api.runWorkflowRunRescore(run.id, this.scorePhaseId()).pipe(
        catchError((e: unknown) => {
          this.scoreError.set(`The host refused to re-score this run: ${String(e)}`);
          return of(null);
        }),
      ),
    );
    if (scored) {
      this.challengerRun.set(scored);
      this.rescoring.set(false);
    }
    this.scoring.set(false);
  }

  /** Lift a halt — its own confirmed decision, carrying a reason, never a side effect of
   *  promoting. Guarded by the same visible-reason rule a rejection is. */
  protected async resume(): Promise<void> {
    const w = this.challenger();
    if (!w || !this.resumeReasonValid().ok || this.resumeSending()) return;
    this.resumeSending.set(true);
    this.resumeError.set(null);
    try {
      this.searchState.set(
        await firstValueFrom(
          this.api.updateWorkflowSearchResumed(w.repo, w.phaseType, w.family, this.resumeReason(), true),
        ),
      );
      this.resuming.set(false);
      this.resumeReason.set('');
    } catch (e) {
      this.resumeError.set(`The host refused to resume the search: ${String(e)}`);
    } finally {
      this.resumeSending.set(false);
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
          // No seat budget and no contention map: the host measures both, and a replay that
          // declared its own would be measuring the client's opinion of the fleet.
          seats: null,
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
