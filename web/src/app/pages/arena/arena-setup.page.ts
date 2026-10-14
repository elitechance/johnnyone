import { Component, OnInit, computed, inject, signal } from '@angular/core';
import { ActivatedRoute, Router, RouterLink } from '@angular/router';
import { firstValueFrom } from 'rxjs';
import {
  JohnnyApiService,
  type AgentPlanPhase,
  type TestGateRun,
} from '../../../../../ui/src/services/johnny-api.service';
import {
  ArenaApiService,
  type Workflow,
  type WorkflowSearchState,
} from './arena-api.service';
import { GateStripComponent } from './gate-strip.component';
import { ARENA_CSS } from './arena-ui.css';
import {
  defaultPhaseType,
  gateAcceptance,
  hopIsJson,
  slotsComplete,
  startRunBlock,
} from './arena-logic';

/** The three families, with the seat caps MEASURED on 2026-10-12 and recorded in the mock. They are
 *  a recorded measurement, not a live reading: J1 has no lease pool, so nothing here is live. */
const FAMILIES = [
  { family: 'glm-5.3-flash', note: '262K · reasoning · executes', cap: 4 },
  { family: 'deepseek-v4.1-flash', note: '262K · proposes the next prompt', cap: 2 },
  { family: 'qwen3.8-flash-next', note: '131K · busiest model', cap: 4 },
];

/**
 * `/initiatives/:id/arena` — the three-step setup (mock steps 2, 3 and 4).
 *
 * Inline template and inline styles on purpose: a `templateUrl`/`styleUrl` component cannot be
 * compiled by TestBed under `web/vitest.config.ts` (no @analogjs plugin), and every behaviour on
 * this screen — the gate being refused, the halt refusing a run, a null count rendering as "not
 * reported" — is worth a failing-then-passing test.
 */
@Component({
  selector: 'app-arena-setup',
  standalone: true,
  imports: [RouterLink, GateStripComponent],
  template: `
    <div class="aw">
      <div class="a-head">
        <a class="a-back" [routerLink]="['/initiatives']" [queryParams]="{ initiativeId: initiativeId(), tab: 'arena' }">‹ back to the console</a>
        <span class="eyebrow">Arena setup {{ stepNumber() }} of 3</span>
        <h1>{{ phase()?.phaseTitle || phaseId() || 'this phase' }}</h1>
      </div>

      <div class="a-steps">
        <span class="st" [class.on]="step() === 'gate'">1 · the test that decides</span>
        <span class="st" [class.on]="step() === 'seats'">2 · models and seats</span>
        <span class="st" [class.on]="step() === 'prompt'">3 · the prompt, in four parts</span>
      </div>

      @if (loadError()) { <div class="a-block" data-test="load-error">{{ loadError() }}</div> }

      @if (step() === 'gate') {
        <div class="a-card">
          <h2>The command that scores a run</h2>
          @if (commands().length) {
            <div class="mono" data-test="gate-commands">@for (c of commands(); track c) {<div>{{ c }}</div>}</div>
            <p class="a-note">It is run once, now, on untouched code, before the setup can continue. Both numbers — the exit code and the collected count — are stored on every run.</p>
          } @else {
            <p class="a-note" data-test="no-gate-note">This phase has no gate at either level. Set one below: the winner is decided by a command, not by an opinion.</p>
            <label class="a-note" for="newgate">one shell command per line, run from the plan's workspace</label>
            <textarea id="newgate" class="a-input mono" data-test="new-gate" [value]="newGate()" (input)="newGate.set($any($event.target).value)"></textarea>
            <div class="a-row">
              <button type="button" class="a-btn" data-test="save-gate" [disabled]="newGate().trim().length === 0 || saving()" (click)="saveGate()">{{ saving() ? 'Saving…' : 'Save this gate on the phase' }}</button>
            </div>
          }
        </div>

        <johnny-gate-strip [gate]="gate()" [pending]="gatePending()" (run)="runGate()" />

        @if (acceptance(); as a) {
          <div [class]="a.accepted ? 'a-card' : 'a-block'" data-test="gate-verdict">
            <b>{{ a.headline }}</b>
            @if (a.reason) { <p class="a-note">{{ a.reason }}</p> }
          </div>
        }

        <div class="a-row">
          <button type="button" class="a-btn primary" data-test="next-gate" [disabled]="!acceptance()?.accepted" (click)="go('seats')">Next</button>
        </div>
        <p class="a-note">No model, no reviewer and no lens ever scores a run. Only this command does.</p>
      }

      @if (step() === 'seats') {
        <div class="a-card">
          <h2>Family and width</h2>
          <p class="a-note">A prompt proven on one model family is not proven on another, so the family is part of what a winner means — one family per run.</p>
          @for (fam of families; track fam.family) {
            <label class="a-row fam">
              <input type="radio" name="family" [value]="fam.family" [checked]="family() === fam.family" (change)="family.set(fam.family)" [attr.data-test]="'family-' + fam.family" />
              <span><b>{{ fam.family }}</b> <span class="a-muted">{{ fam.note }}</span></span>
            </label>
          }
          <div class="a-warn" data-test="seats-unmeasured">
            Seats in use are <b>not measured</b>. J1 has no lease pool: the fleet API exposes no provider count, no queue depth and no active-user field, and going over a cap raises no error, so there is nothing to read. The caps below are a recorded measurement from 2026-10-12, not a live reading, and this screen deliberately draws no meter it cannot fill.
          </div>
          <dl class="a-kv">
            @for (fam of families; track fam.family) {
              <dt>{{ fam.family }}</dt><dd>cap {{ fam.cap }} · leases held: not reported</dd>
            }
          </dl>
          <label class="a-note" for="fanout">how wide the prompt may run (subagents in parallel)</label>
          <input id="fanout" class="a-input" type="number" min="1" data-test="fan-out" [value]="fanOut()" (input)="setFanOut($any($event.target).value)" />
          <p class="a-note">A prompt must be correct at width 1. Parallelism is an optimisation, never a requirement.</p>
        </div>
        <div class="a-row">
          <button type="button" class="a-btn" (click)="go('gate')">Back</button>
          <button type="button" class="a-btn primary" data-test="next-seats" (click)="go('prompt')">Next</button>
        </div>
      }

      @if (step() === 'prompt') {
        <div class="a-card">
          <h2>{{ champion() ? 'v' + champion()!.version + ' · champion' : 'no champion yet' }}</h2>
          <dl class="a-kv" data-test="champion-key">
            <dt>repo</dt><dd class="mono">{{ repo() || '…' }}</dd>
            <dt>phase type</dt><dd>{{ phaseType() }}</dd>
            <dt>family</dt><dd>{{ family() }}</dd>
          </dl>
          <p class="a-note">
            That repo identity is the host's, derived from the workspace — the git remote where
            there is one, the absolute path otherwise. It is what a champion is keyed on, so two
            checkouts of the same repo share one and two unrelated repos with the same folder name
            do not.
          </p>
          @if (champion()) {
            <p class="a-note" data-test="seeded-from">Seeded from v{{ champion()!.version }}, the prompt J1 runs for this phase today. Edit any part to propose a challenger.</p>
          } @else {
            <div class="a-warn" data-test="no-baseline">
              There is no champion for <b>{{ repo() }} · {{ phaseType() }} · {{ family() }}</b> yet, so there is nothing to beat and a "win" would mean nothing. Importing J1's current development prompt automatically is NOT built — no API exposes <span class="mono">DEFAULT_DEVELOPMENT_WORKER</span> — so the baseline has to be written here, and what you write becomes v1, the baseline, not a challenger.
            </div>
          }
        </div>

        @if (haltBlock(); as block) {
          <div class="a-block" data-test="halt-block">
            <b>The search is paused.</b>
            <p class="a-note">{{ block }}</p>
            @if (searchState()?.recentRejections?.length) {
              <ul class="a-list">@for (r of searchState()!.recentRejections; track r) {<li>{{ r }}</li>}</ul>
            }
          </div>
        }

        <div class="a-card">
          <label for="s-role">role</label>
          <textarea id="s-role" class="a-input" data-test="slot-role" [value]="role()" (input)="role.set($any($event.target).value)"></textarea>
          <label for="s-instruction">instruction</label>
          <textarea id="s-instruction" class="a-input" data-test="slot-instruction" [value]="instruction()" (input)="instruction.set($any($event.target).value)"></textarea>
          <label for="s-contract">contract</label>
          <textarea id="s-contract" class="a-input" data-test="slot-contract" [value]="contract()" (input)="contract.set($any($event.target).value)"></textarea>
          <label for="s-hop">hop <span class="a-muted">(json: steps, parallel groups, returns)</span></label>
          <textarea id="s-hop" class="a-input mono" data-test="slot-hop" [value]="hop()" (input)="hop.set($any($event.target).value)"></textarea>
          <p class="a-note">width {{ fanOut() }} — the seat cost of this prompt. Contract and hop are where detail migrates to as the search progresses.</p>
        </div>

        <div class="a-warn" data-test="no-executor-note">
          <b>This records a challenger and a run; nothing scores it automatically.</b> The run's
          gate reads "never ran" until someone runs it, and a run with no score cannot be promoted.
          Scoring is one button on the arena screen this takes you to — the coordinator does not
          trigger it, you do.
        </div>

        @if (blockers().length) {
          <div class="a-block" data-test="prompt-blockers">
            <ul class="a-list">@for (b of blockers(); track b) {<li>{{ b }}</li>}</ul>
          </div>
        }
        @if (startError()) { <div class="a-block" data-test="start-error">{{ startError() }}</div> }

        <div class="a-row">
          <button type="button" class="a-btn" (click)="go('seats')">Back</button>
          <button type="button" class="a-btn primary" data-test="start-search" [disabled]="blockers().length > 0 || starting()" (click)="startSearch()">
            {{ starting() ? 'Recording…' : champion() ? 'Record the challenger and its run' : 'Record v1 as the baseline' }}
          </button>
        </div>
      }
    </div>
  `,
  styles: [
    ARENA_CSS,
    `
      label { display: block; font-size: 12.5px; opacity: 0.75; margin: 10px 0 4px; }
      .fam { margin: 6px 0; }
      .fam input { width: 20px; height: 20px; }
    `,
  ],
})
export class ArenaSetupPage implements OnInit {
  private readonly api = inject(JohnnyApiService);
  /** The arena's own wire, provided by the ROUTE (see app.routes.ts), so it ships in the lazy
   *  arena chunk instead of the initial bundle. */
  private readonly arena = inject(ArenaApiService);
  private readonly route = inject(ActivatedRoute);
  private readonly router = inject(Router);

  protected readonly families = FAMILIES;
  protected readonly initiativeId = signal('');
  protected readonly phaseId = signal<string | null>(null);
  protected readonly step = signal<'gate' | 'seats' | 'prompt'>('gate');
  protected readonly phases = signal<AgentPlanPhase[]>([]);
  protected readonly workspacePath = signal('');
  protected readonly runType = signal('');
  protected readonly gate = signal<TestGateRun | null>(null);
  protected readonly gatePending = signal(false);
  protected readonly loadError = signal<string | null>(null);
  protected readonly startError = signal<string | null>(null);
  protected readonly saving = signal(false);
  protected readonly starting = signal(false);
  protected readonly newGate = signal('');
  protected readonly family = signal(FAMILIES[0].family);
  protected readonly fanOut = signal(1);
  protected readonly champion = signal<Workflow | null>(null);
  protected readonly searchState = signal<WorkflowSearchState | null>(null);
  protected readonly role = signal('');
  protected readonly instruction = signal('');
  protected readonly contract = signal('');
  protected readonly hop = signal('');

  protected readonly phase = computed(() =>
    this.phases().find((p) => p.phaseId === this.phaseId()) ?? null,
  );
  protected readonly commands = computed<string[]>(() => this.phase()?.effectiveTestCommands ?? []);
  /** The repo identity, as the HOST derives it (git remote, normalised, else the absolute path).
   *  Never computed here: the client used to use the path's basename, which gave two unrelated
   *  checkouts named `johnnyone` a single shared champion. */
  protected readonly repo = signal('');
  protected readonly phaseType = computed(() => defaultPhaseType(this.runType()));
  protected readonly acceptance = computed(() => {
    const g = this.gate();
    return g ? gateAcceptance({ gateDeclared: g.gateDeclared, exitCode: g.exitCode, collected: g.collected }) : null;
  });
  protected readonly haltBlock = computed(() => startRunBlock(this.searchState()));
  /** Everything standing between this screen and a run, as sentences. One list, one home. */
  protected readonly blockers = computed<string[]>(() => {
    const out: string[] = [];
    const halt = this.haltBlock();
    if (halt) out.push(halt);
    const slots = slotsComplete({
      role: this.role(),
      instruction: this.instruction(),
      contract: this.contract(),
      hop: this.hop(),
    });
    if (!slots.ok) {
      out.push(`All four parts must carry text — blank: ${slots.missing.join(', ')}. Without a seeded baseline there is nothing to beat, so a win would mean nothing.`);
    } else if (!hopIsJson(this.hop())) {
      out.push('The hop has to be JSON (an object or an array) — the host stores it as JSON and refuses anything else.');
    }
    return out;
  });

  protected stepNumber(): number {
    return this.step() === 'gate' ? 1 : this.step() === 'seats' ? 2 : 3;
  }

  async ngOnInit(): Promise<void> {
    const snap = this.route.snapshot;
    this.initiativeId.set(snap.paramMap.get('id') ?? '');
    this.phaseId.set(snap.queryParamMap.get('phaseId'));
    const step = snap.queryParamMap.get('step');
    if (step === 'seats' || step === 'prompt' || step === 'gate') this.step.set(step);
    try {
      const run = await firstValueFrom(this.api.getAgentPlan(this.initiativeId()));
      this.phases.set(run.phases ?? []);
      this.workspacePath.set(run.plan?.workspacePath ?? '');
      this.runType.set(run.plan?.runType ?? '');
    } catch (e) {
      this.loadError.set(`Could not read this initiative: ${String(e)}`);
      return;
    }
    await this.loadFamilyState();
  }

  /** The champion to beat and the host's halt, for the chosen (repo, phaseType, family). */
  private async loadFamilyState(): Promise<void> {
    if (!this.repo() && this.workspacePath()) {
      try {
        this.repo.set(await firstValueFrom(this.arena.repoIdentity(this.workspacePath())));
      } catch (e) {
        this.loadError.set(`Could not read this workspace's repo identity: ${String(e)}`);
        return;
      }
    }
    const repo = this.repo();
    const phaseType = this.phaseType();
    const family = this.family();
    try {
      const all = await firstValueFrom(this.arena.workflows({ repo, phaseType, family }));
      const champ = all.find((w) => w.isChampion) ?? null;
      this.champion.set(champ);
      if (champ) {
        this.role.set(champ.role);
        this.instruction.set(champ.instruction);
        this.contract.set(champ.contract);
        this.hop.set(champ.hop);
        this.fanOut.set(champ.fanOut);
      }
    } catch (e) {
      this.loadError.set(`Could not read this family's workflows: ${String(e)}`);
    }
    try {
      this.searchState.set(await firstValueFrom(this.arena.workflowSearchState(repo, phaseType, family)));
    } catch (e) {
      // A state that cannot be read is itself a block (`startRunBlock(null)`), not an empty state.
      this.searchState.set(null);
      this.loadError.set(`Could not read the search state: ${String(e)}`);
    }
  }

  protected go(step: 'gate' | 'seats' | 'prompt'): void {
    this.step.set(step);
    if (step === 'prompt') void this.loadFamilyState();
  }

  protected setFanOut(value: string): void {
    const n = Number.parseInt(value, 10);
    this.fanOut.set(Number.isFinite(n) && n >= 1 ? n : 1);
  }

  protected async runGate(): Promise<void> {
    if (this.gatePending()) return;
    this.gatePending.set(true);
    try {
      this.gate.set(await firstValueFrom(this.api.runAgentPlanTestGate(this.initiativeId(), this.phaseId())));
    } catch (e) {
      this.loadError.set(`The gate could not be run: ${String(e)}`);
    } finally {
      this.gatePending.set(false);
    }
  }

  protected async saveGate(): Promise<void> {
    const commands = this.newGate().split('\n').map((c) => c.trim()).filter((c) => c.length > 0);
    if (commands.length === 0 || this.saving()) return;
    this.saving.set(true);
    try {
      const run = await firstValueFrom(
        this.api.updateAgentPlanTestCommands(this.initiativeId(), this.phaseId(), commands),
      );
      if (run?.phases?.length) this.phases.set(run.phases);
      this.gate.set(null);
    } catch (e) {
      this.loadError.set(`The gate could not be saved: ${String(e)}`);
    } finally {
      this.saving.set(false);
    }
  }

  protected async startSearch(): Promise<void> {
    // Guarded here as well as on the button: a disabled attribute is a hint, this is the rule.
    if (this.blockers().length > 0 || this.starting()) return;
    this.starting.set(true);
    this.startError.set(null);
    try {
      const wf = await firstValueFrom(
        this.arena.createWorkflowVersion({
          workspacePath: this.workspacePath(),
          phaseType: this.phaseType(),
          family: this.family(),
          parentId: this.champion()?.id ?? null,
          role: this.role(),
          instruction: this.instruction(),
          contract: this.contract(),
          hop: this.hop(),
          fanOut: this.fanOut(),
        }),
      );
      await firstValueFrom(
        this.arena.startWorkflowRun({
          workflowId: wf.id,
          planId: this.initiativeId(),
          seed: 0,
          // `seats` only OVERRIDES the workflow's own fanOut, and the workflow already carries it,
          // so this sends nothing. The seat budget and the fleet contention are MEASURED BY THE
          // HOST: a browser can measure neither, and the previous version sent a declared budget
          // plus an empty contention map, which reduced `comparable` to "did two clients type the
          // same numbers".
          seats: null,
        }),
      );
      void this.router.navigate(['/workflows', wf.id, 'arena']).catch(() => undefined);
    } catch (e) {
      this.startError.set(`The host refused to start this run: ${String(e)}`);
    } finally {
      this.starting.set(false);
    }
  }
}
