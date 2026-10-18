import { Injectable, inject } from '@angular/core';
import { map, type Observable } from 'rxjs';
import { GraphQLClient } from '../../../../../ui/src/services/graphql-client';

/**
 * The workflow arena's client wire (desktop migration 026).
 *
 * It lives HERE, not on `JohnnyApiService`, and is `providedIn` nothing: only the arena's own lazy
 * components inject it, so these ten operations and their selection sets land in the lazy arena
 * chunk. The eager service is in the initial bundle, which is already ~1 MB over its budget, and
 * nine of these ten are unreachable from any eager screen. The two GATE operations
 * (`updateAgentPlanTestCommands`, `runAgentPlanTestGate`) deliberately stayed on the eager service:
 * the console's own Checks tab calls them.
 *
 * Every RULE behind these objects lives on the host — `budgets_are_comparable`, `promotion_block`,
 * `workflow_search_state`, and the host's own measurement of the seat budget and fleet contention.
 * This client reads the host's answers and never recomputes one.
 *
 * NUMERIC FIELDS ARE `Float`, NEVER `Int`. GraphQL `Int` is 32-bit, and a non-null `Int` that
 * cannot serialise propagates null to the response root — this project shipped that outage once,
 * when one session's token count passed 2^31-1 and emptied the terminal tab bar on every device.
 */

/** The SEARCHED ARTIFACT: the four prompt parts a phase-type is executed with. */
export interface Workflow {
  id: string;
  repo: string;
  phaseType: string;
  /** Model family. Part of a champion's IDENTITY, not its environment. */
  family: string;
  /** Monotonic WITHIN (repo, phaseType, family), from 1. */
  version: number;
  parentId?: string | null;
  role: string;
  instruction: string;
  contract: string;
  /** JSON string: steps, parallel groups, returns. Served as stored. */
  hop: string;
  fanOut: number;
  isChampion: boolean;
  createdAt: string;
}

/** One execution of one workflow. */
export interface WorkflowRun {
  id: string;
  workflowId: string;
  planId?: string | null;
  seed: number;
  /** THE SCORE. 0 = the gate passed. NULL = the gate never ran, which is NOT a pass. */
  gateExit?: number | null;
  /** Advisory and nullable; unbounded on the host, hence Float. */
  gateCollected?: number | null;
  /** JSON object, model → seats. MEASURED BY THE HOST at start; not a client argument. */
  seatsHeld: string;
  /** JSON object: fleet contention at start, measured by the host. */
  fleetBusy: string;
  startedAt: string;
  endedAt?: string | null;
  /** DERIVED by the host from `fleetBusy`: whether this run started on a quiet enough fleet to be
   *  scored at all. A run taken under load measured the fleet, not the workflow. */
  scoreable: boolean;
  /** Why not, when `scoreable` is false — the field that turns "these two are mysteriously never
   *  comparable" into something a human can read. */
  notScoreableReason?: string | null;
}

/** One outcome: this run beat that run. `comparable` is the host's, not an argument. */
export interface WorkflowPref {
  id: string;
  winnerRunId: string;
  loserRunId: string;
  /** False when the two runs held DIFFERENT seat budgets. Recorded either way; cannot promote. */
  comparable: boolean;
  reason?: string | null;
  createdAt: string;
}

/** The search's state for one (repo, phaseType, family) — the halt as DATA. */
export interface WorkflowSearchState {
  repo: string;
  phaseType: string;
  family: string;
  /** DISTINCT runs refused since the last resume. */
  consecutiveRejections: number;
  /** While true the host refuses to start a run AND to promote one. The only way out is an
   *  explicit, confirmed `updateWorkflowSearchResumed`. */
  halted: boolean;
  strikeLimit: number;
  /** Newest first — the one thing a halted search owes a human. */
  recentRejections: string[];
  championWorkflowId?: string | null;
}

@Injectable()
export class ArenaApiService {
  private readonly gql = inject(GraphQLClient);

  /**
   * The ONE `Workflow` selection set, hoisted for the same reason `sessionFields` is: the list, the
   * detail page and every mutation that returns a Workflow must ask for the same fields, or a
   * screen patched from one result silently loses a slot.
   */
  private readonly workflowFields = `
    id repo phaseType family version parentId
    role instruction contract hop fanOut isChampion createdAt
  `;
  /** The ONE `WorkflowRun` selection set. `gateCollected` is in it on purpose: a UI that never
   *  asked for the count cannot tell "not reported" from "the field was not requested". */
  private readonly workflowRunFields = `
    id workflowId planId seed gateExit gateCollected seatsHeld fleetBusy startedAt endedAt
    scoreable notScoreableReason
  `;
  private readonly workflowPrefFields = `
    id winnerRunId loserRunId comparable reason createdAt
  `;
  private readonly workflowSearchStateFields = `
    repo phaseType family consecutiveRejections halted strikeLimit recentRejections
    championWorkflowId
  `;

  /** All three filters are optional and independent; absent means "any". */
  workflows(filter: { repo?: string; phaseType?: string; family?: string }): Observable<Workflow[]> {
    return this.gql
      .query<{ workflows: Workflow[] }>(
        `query Workflows($repo: String, $phaseType: String, $family: String) {
          workflows(repo: $repo, phaseType: $phaseType, family: $family) {
            ${this.workflowFields}
          }
        }`,
        { repo: filter.repo, phaseType: filter.phaseType, family: filter.family }
      )
      .pipe(map((data) => data.workflows));
  }

  workflow(id: string): Observable<Workflow> {
    return this.gql
      .query<{ workflow: Workflow }>(
        `query Workflow($id: ID!) {
          workflow(id: $id) { ${this.workflowFields} }
        }`,
        { id }
      )
      .pipe(map((data) => data.workflow));
  }

  workflowRuns(workflowId: string): Observable<WorkflowRun[]> {
    return this.gql
      .query<{ workflowRuns: WorkflowRun[] }>(
        `query WorkflowRuns($workflowId: ID!) {
          workflowRuns(workflowId: $workflowId) { ${this.workflowRunFields} }
        }`,
        { workflowId }
      )
      .pipe(map((data) => data.workflowRuns));
  }

  /** Every preference involving this workflow's runs, newest first. Reading one back is how the
   *  promote step recovers a `prefId` after a reload — it used to require issuing a WRITE. */
  workflowPrefs(workflowId: string): Observable<WorkflowPref[]> {
    return this.gql
      .query<{ workflowPrefs: WorkflowPref[] }>(
        `query WorkflowPrefs($workflowId: ID!) {
          workflowPrefs(workflowId: $workflowId) { ${this.workflowPrefFields} }
        }`,
        { workflowId }
      )
      .pipe(map((data) => data.workflowPrefs));
  }

  /** The halt, as data. Read it before offering to start a run — the host refuses while halted. */
  workflowSearchState(repo: string, phaseType: string, family: string): Observable<WorkflowSearchState> {
    return this.gql
      .query<{ workflowSearchState: WorkflowSearchState }>(
        `query WorkflowSearchState($repo: String!, $phaseType: String!, $family: String!) {
          workflowSearchState(repo: $repo, phaseType: $phaseType, family: $family) {
            ${this.workflowSearchStateFields}
          }
        }`,
        { repo, phaseType, family }
      )
      .pipe(map((data) => data.workflowSearchState));
  }

  createWorkflowVersion(input: {
    /** The PLAN's workspace, not a repo name. The host derives the identity (`repoIdentity`): a
     *  client-supplied identity is a client-supplied champion key, and the basename of a path gave
     *  two unrelated checkouts called `johnnyone` one shared champion. */
    workspacePath: string;
    phaseType: string;
    family: string;
    parentId?: string | null;
    role: string;
    instruction: string;
    contract: string;
    hop: string;
    fanOut: number;
  }): Observable<Workflow> {
    return this.gql
      .mutate<{ createWorkflowVersion: Workflow }>(
        `mutation CreateWorkflowVersion($workspacePath: String!, $phaseType: String!, $family: String!, $parentId: String, $role: String!, $instruction: String!, $contract: String!, $hop: String!, $fanOut: Float!) {
          createWorkflowVersion(workspacePath: $workspacePath, phaseType: $phaseType, family: $family, parentId: $parentId, role: $role, instruction: $instruction, contract: $contract, hop: $hop, fanOut: $fanOut) {
            ${this.workflowFields}
          }
        }`,
        { ...input, parentId: input.parentId ?? null }
      )
      .pipe(map((data) => data.createWorkflowVersion));
  }

  /**
   * REFUSED by the host while the family's search is halted.
   *
   * There is deliberately no `seatsHeld` and no `fleetBusy` argument: the caller is a browser and
   * can measure neither. Sending a declared budget and an empty contention map reduced
   * `comparable` to "did two clients type the same numbers", which is the exact failure those two
   * columns exist to prevent. The host snapshots both. `seats` only OVERRIDES the workflow's own
   * `fanOut`; the family is never an argument — it is the workflow's.
   */
  startWorkflowRun(input: {
    workflowId: string;
    planId?: string | null;
    seed?: number;
    seats?: number | null;
  }): Observable<WorkflowRun> {
    return this.gql
      .mutate<{ startWorkflowRun: WorkflowRun }>(
        `mutation StartWorkflowRun($workflowId: ID!, $planId: String, $seed: Float, $seats: Float) {
          startWorkflowRun(workflowId: $workflowId, planId: $planId, seed: $seed, seats: $seats) {
            ${this.workflowRunFields}
          }
        }`,
        {
          workflowId: input.workflowId,
          planId: input.planId ?? null,
          seed: input.seed ?? 0,
          seats: input.seats ?? null,
        }
      )
      .pipe(map((data) => data.startWorkflowRun));
  }

  /** Record a score someone else measured. `gateExit` is non-null: a missing score must be an
   *  error, not a 0 that reads as a pass. There is no `rescore` flag — replacing a result is
   *  `recordWorkflowRunRescore`, a separate name, because a flag is what a caller forgets. */
  recordWorkflowRunGate(runId: string, gateExit: number, gateCollected?: number | null): Observable<WorkflowRun> {
    return this.gql
      .mutate<{ recordWorkflowRunGate: WorkflowRun }>(
        `mutation RecordWorkflowRunGate($runId: ID!, $gateExit: Float!, $gateCollected: Float) {
          recordWorkflowRunGate(runId: $runId, gateExit: $gateExit, gateCollected: $gateCollected) {
            ${this.workflowRunFields}
          }
        }`,
        { runId, gateExit, gateCollected: gateCollected ?? null }
      )
      .pipe(map((data) => data.recordWorkflowRunGate));
  }

  /** Replace a recorded score with one measured elsewhere, deliberately. The sitting champion's
   *  promoting run is refused by the host either way — editing the evidence under a promotion that
   *  already happened is falsification, not correction. */
  recordWorkflowRunRescore(runId: string, gateExit: number, gateCollected?: number | null): Observable<WorkflowRun> {
    return this.gql
      .mutate<{ recordWorkflowRunRescore: WorkflowRun }>(
        `mutation RecordWorkflowRunRescore($runId: ID!, $gateExit: Float!, $gateCollected: Float) {
          recordWorkflowRunRescore(runId: $runId, gateExit: $gateExit, gateCollected: $gateCollected) {
            ${this.workflowRunFields}
          }
        }`,
        { runId, gateExit, gateCollected: gateCollected ?? null }
      )
      .pipe(map((data) => data.recordWorkflowRunRescore));
  }

  /**
   * EXECUTE this run's gate and record the result — the step that closes the arena's loop.
   *
   * The host runs the plan's effective gate in the plan's workspace, so this can take minutes. It
   * refuses a run with no plan, a halted family, and a plan that declares NO gate at either level —
   * that last one because an exit code of 0 comes back both from a gate that passed and from a
   * phase with no gate, and recording it would make an ungated run look promotable.
   */
  runWorkflowRunGate(runId: string, phaseId?: string | null): Observable<WorkflowRun> {
    return this.gql
      .mutate<{ runWorkflowRunGate: WorkflowRun }>(
        `mutation RunWorkflowRunGate($runId: ID!, $phaseId: String) {
          runWorkflowRunGate(runId: $runId, phaseId: $phaseId) {
            ${this.workflowRunFields}
          }
        }`,
        { runId, phaseId: phaseId ?? null }
      )
      .pipe(map((data) => data.runWorkflowRunGate));
  }

  /**
   * RUN one phase of an existing run on the desktop host — the arena's missing EXECUTE step, which
   * is what makes a created run actually executable from the console.
   *
   * `verify` and `binary` are relayed ONLY when the caller supplied a non-empty string after
   * trimming. They are deliberately NOT defaulted anywhere in the web code, and an omitted, empty
   * or whitespace-only value is ABSENT from the variables rather than `null`: the host owns the
   * resolution rule for which verify command and which binary a run executes with, and a null on
   * the wire reads as an explicit override of that rule rather than as "use my default".
   *
   * The answer is the updated run row, whose `gateExit` is the score — and `null` there means the
   * gate never ran, which is NOT a pass.
   */
  executeWorkflowRun(runId: string, verify?: string | null, binary?: string | null): Observable<WorkflowRun> {
    const variables: { runId: string; verify?: string; binary?: string } = { runId };
    if (typeof verify === 'string' && verify.trim() !== '') {
      variables.verify = verify;
    }
    if (typeof binary === 'string' && binary.trim() !== '') {
      variables.binary = binary;
    }
    return this.gql
      .mutate<{ executeWorkflowRun: WorkflowRun }>(
        `mutation ExecuteWorkflowRun($runId: ID!, $verify: String, $binary: String) {
          executeWorkflowRun(runId: $runId, verify: $verify, binary: $binary) {
            ${this.workflowRunFields}
          }
        }`,
        variables
      )
      .pipe(map((data) => data.executeWorkflowRun));
  }

  /** Execute the gate AGAIN for a run that already has a score, replacing it. Its own field, so
   *  there is no flag to forget and no default to get wrong. */
  runWorkflowRunRescore(runId: string, phaseId?: string | null): Observable<WorkflowRun> {
    return this.gql
      .mutate<{ runWorkflowRunRescore: WorkflowRun }>(
        `mutation RunWorkflowRunRescore($runId: ID!, $phaseId: String) {
          runWorkflowRunRescore(runId: $runId, phaseId: $phaseId) {
            ${this.workflowRunFields}
          }
        }`,
        { runId, phaseId: phaseId ?? null }
      )
      .pipe(map((data) => data.runWorkflowRunRescore));
  }

  /** The repo identity the HOST derives for a workspace path: the git remote, normalised, else the
   *  absolute path. The one way a client learns the value to key its reads on without
   *  re-implementing the derivation — the client used to use the path's basename, which gave two
   *  unrelated checkouts named `johnnyone` a single shared champion. */
  repoIdentity(workspacePath: string): Observable<string> {
    return this.gql
      .query<{ repoIdentity: string }>(
        `query RepoIdentity($workspacePath: String!) {
          repoIdentity(workspacePath: $workspacePath)
        }`,
        { workspacePath }
      )
      .pipe(map((data) => data.repoIdentity));
  }

  /** No `comparable` argument exists, deliberately: the host computes it from the two runs.
   *  IDEMPOTENT per ordered pair — calling it again returns the existing preference. */
  recordWorkflowPref(winnerRunId: string, loserRunId: string, reason?: string | null): Observable<WorkflowPref> {
    return this.gql
      .mutate<{ recordWorkflowPref: WorkflowPref }>(
        `mutation RecordWorkflowPref($winnerRunId: ID!, $loserRunId: ID!, $reason: String) {
          recordWorkflowPref(winnerRunId: $winnerRunId, loserRunId: $loserRunId, reason: $reason) {
            ${this.workflowPrefFields}
          }
        }`,
        { winnerRunId, loserRunId, reason: reason ?? null }
      )
      .pipe(map((data) => data.recordWorkflowPref));
  }

  /** PROMOTE. `confirmed` is explicit and non-null because this is the arena's one irreversible
   *  step: a call that forgot its arguments must be a validation error, not a promotion. */
  updateWorkflowChampion(runId: string, prefId: string | null, confirmed: boolean): Observable<Workflow> {
    return this.gql
      .mutate<{ updateWorkflowChampion: Workflow }>(
        `mutation UpdateWorkflowChampion($runId: ID!, $prefId: String, $confirmed: Boolean!) {
          updateWorkflowChampion(runId: $runId, prefId: $prefId, confirmed: $confirmed) {
            ${this.workflowFields}
          }
        }`,
        { runId, prefId, confirmed }
      )
      .pipe(map((data) => data.updateWorkflowChampion));
  }

  /** REJECT, with a reason (the host also refuses a blank one). Returns the resulting search
   *  state, so the caller already holds the halt instead of having to ask again. */
  recordWorkflowRejection(runId: string, reason: string): Observable<WorkflowSearchState> {
    return this.gql
      .mutate<{ recordWorkflowRejection: WorkflowSearchState }>(
        `mutation RecordWorkflowRejection($runId: ID!, $reason: String!) {
          recordWorkflowRejection(runId: $runId, reason: $reason) {
            ${this.workflowSearchStateFields}
          }
        }`,
        { runId, reason }
      )
      .pipe(map((data) => data.recordWorkflowRejection));
  }

  /** RESUME: lift a halt. The only thing that resets the streak, and its own confirmed decision —
   *  a promotion used to clear a halt, which made the halt escapable with no new evidence. */
  updateWorkflowSearchResumed(
    repo: string,
    phaseType: string,
    family: string,
    reason: string,
    confirmed: boolean,
  ): Observable<WorkflowSearchState> {
    return this.gql
      .mutate<{ updateWorkflowSearchResumed: WorkflowSearchState }>(
        `mutation UpdateWorkflowSearchResumed($repo: String!, $phaseType: String!, $family: String!, $reason: String!, $confirmed: Boolean!) {
          updateWorkflowSearchResumed(repo: $repo, phaseType: $phaseType, family: $family, reason: $reason, confirmed: $confirmed) {
            ${this.workflowSearchStateFields}
          }
        }`,
        { repo, phaseType, family, reason, confirmed }
      )
      .pipe(map((data) => data.updateWorkflowSearchResumed));
  }
}
