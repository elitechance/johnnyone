// The arena's client wire. Drives the REAL services with a stub GraphQLClient and inspects the
// operation text and variables they send, which is the only place a missing field or a renamed
// argument shows up before a deploy. What it holds:
//   * the selection sets are HOISTED (the sessionFields / agentPlanRunFields precedent) — one copy,
//     so a field added for one screen cannot be missing on another;
//   * `gateCollected` and `effectiveTestCommands` are actually ASKED FOR. A UI that renders "not
//     reported" because it never requested the field is indistinguishable from a gate that reported
//     nothing, which is exactly the confusion the count exists to prevent;
//   * the ten ARENA-ONLY operations live on `ArenaApiService`, which only the lazy arena chunk
//     injects, so they cost nothing on an initial bundle that is already ~1 MB over budget. The two
//     gate operations stay on the eager service because the console's Checks tab reaches them.
import '@angular/compiler';
import { describe, it, expect, beforeEach } from 'vitest';
import { TestBed } from '@angular/core/testing';
import { of } from 'rxjs';
import '../../../test-setup';
import { JohnnyApiService } from '../../../../../ui/src/services/johnny-api.service';
import { GraphQLClient } from '../../../../../ui/src/services/graphql-client';
import { ArenaApiService } from './arena-api.service';

class StubClient {
  sent: { op: string; variables: Record<string, unknown> | undefined }[] = [];
  payload: unknown = {};
  query<T>(op: string, variables?: Record<string, unknown>) {
    this.sent.push({ op, variables });
    return of(this.payload as T);
  }
  queryPreferLocalHost<T>(workerOp: string, _localOp: string, variables?: Record<string, unknown>) {
    this.sent.push({ op: workerOp, variables });
    return of(this.payload as T);
  }
  mutate<T>(op: string, variables?: Record<string, unknown>) {
    this.sent.push({ op, variables });
    return of(this.payload as T);
  }
}

describe('JohnnyApiService — the gate, which the console itself reaches', () => {
  let api: JohnnyApiService;
  let gql: StubClient;

  beforeEach(() => {
    gql = new StubClient();
    TestBed.resetTestingModule();
    TestBed.configureTestingModule({
      providers: [JohnnyApiService, { provide: GraphQLClient, useValue: gql }],
    });
    api = TestBed.inject(JohnnyApiService);
  });

  const last = () => gql.sent[gql.sent.length - 1];

  it('asks for the phase gate the UI renders — effectiveTestCommands, on every plan read', () => {
    gql.payload = { getAgentPlan: { plan: {}, phases: [], tasks: [], events: [] } };
    api.getAgentPlan('p1').subscribe();
    expect(last().op).toContain('effectiveTestCommands');
    expect(last().op).toContain('testCommands');
  });

  it('runs the gate once and asks for BOTH numbers plus gateDeclared', () => {
    gql.payload = { runAgentPlanTestGate: { gateDeclared: true, exitCode: 0 } };
    api.runAgentPlanTestGate('p1', '04-leases').subscribe();
    const op = last().op;
    expect(op).toContain('runAgentPlanTestGate');
    expect(op).toContain('gateDeclared');
    expect(op).toContain('exitCode');
    expect(op).toContain('collected');
    expect(last().variables).toEqual({ id: 'p1', phaseId: '04-leases' });
  });

  it('carries NO arena operation — those belong to the lazy chunk', () => {
    const service = api as unknown as Record<string, unknown>;
    for (const name of [
      'workflows', 'workflow', 'workflowRuns', 'workflowPrefs', 'workflowSearchState',
      'createWorkflowVersion', 'startWorkflowRun', 'recordWorkflowRunGate', 'recordWorkflowPref',
      'updateWorkflowChampion', 'recordWorkflowRejection', 'updateWorkflowSearchResumed',
    ]) {
      expect(service[name], `${name} is still on the eager service`).toBeUndefined();
    }
  });
});

describe('ArenaApiService — the arena wire, lazy', () => {
  let api: ArenaApiService;
  let gql: StubClient;

  beforeEach(() => {
    gql = new StubClient();
    TestBed.resetTestingModule();
    TestBed.configureTestingModule({
      providers: [ArenaApiService, { provide: GraphQLClient, useValue: gql }],
    });
    api = TestBed.inject(ArenaApiService);
  });

  const last = () => gql.sent[gql.sent.length - 1];

  it('lists workflows with the three independent filters', () => {
    gql.payload = { workflows: [] };
    api.workflows({ repo: 'johnnyone', phaseType: 'dev-phase' }).subscribe();
    expect(last().variables).toEqual({ repo: 'johnnyone', phaseType: 'dev-phase', family: undefined });
    expect(last().op).toContain('isChampion');
  });

  it('asks a run for gateExit AND the nullable advisory gateCollected', () => {
    gql.payload = { workflowRuns: [] };
    api.workflowRuns('w1').subscribe();
    expect(last().op).toContain('gateExit');
    expect(last().op).toContain('gateCollected');
    expect(last().op).toContain('seatsHeld');
    expect(last().op).toContain('scoreable');
    expect(last().op).toContain('notScoreableReason');
  });

  it('READS preferences back instead of recording one to discover it', () => {
    gql.payload = { workflowPrefs: [] };
    api.workflowPrefs('w2').subscribe();
    expect(last().op).toContain('workflowPrefs');
    expect(last().op).toContain('comparable');
    expect(last().variables).toEqual({ workflowId: 'w2' });
  });

  it('reads the halt as data, with the reasons a halted search owes a human', () => {
    gql.payload = { workflowSearchState: {} };
    api.workflowSearchState('johnnyone', 'dev-phase', 'glm').subscribe();
    expect(last().op).toContain('halted');
    expect(last().op).toContain('consecutiveRejections');
    expect(last().op).toContain('recentRejections');
    expect(last().op).toContain('strikeLimit');
  });

  it('starts a run WITHOUT inventing a seat budget or a contention map', () => {
    gql.payload = { startWorkflowRun: {} };
    api.startWorkflowRun({ workflowId: 'w2', planId: 'p1', seed: 1 }).subscribe();
    const sent = last();
    // The host measures both. A browser cannot, and sending `{}` made `comparable` mean
    // "did two clients type the same numbers".
    expect(JSON.stringify(sent.variables)).not.toContain('seatsHeld');
    expect(JSON.stringify(sent.variables)).not.toContain('fleetBusy');
    expect(sent.op).not.toContain('$seatsHeld');
    expect(sent.op).not.toContain('$fleetBusy');
    expect(sent.variables).toEqual({ workflowId: 'w2', planId: 'p1', seed: 1, seats: null });
  });

  it('declares every numeric argument as Float, never Int', () => {
    gql.payload = { createWorkflowVersion: {} };
    api.createWorkflowVersion({ workspacePath: '/w/r', phaseType: 'p', family: 'f', role: 'a', instruction: 'b', contract: 'c', hop: '{}', fanOut: 2 }).subscribe();
    expect(last().op).toContain('$fanOut: Float!');
    gql.payload = { startWorkflowRun: {} };
    api.startWorkflowRun({ workflowId: 'w2' }).subscribe();
    expect(last().op).toContain('$seed: Float');
    expect(last().op).toContain('$seats: Float');
    gql.payload = { recordWorkflowRunGate: {} };
    api.recordWorkflowRunGate('r2', 0, 23).subscribe();
    expect(last().op).toContain('$gateExit: Float!');
    expect(last().op).toContain('$gateCollected: Float');
    // No operation may declare an Int — Int is 32-bit and a non-null Int that cannot serialise
    // nulls the whole response, which this project has already shipped once.
    for (const sent of gql.sent) expect(sent.op).not.toMatch(/:\s*Int!?\b/);
  });

  // The `rescore: Boolean` argument is GONE from the API: a re-score is its own field, because a
  // flag on the recording mutation is exactly the thing a caller forgets or defaults wrong.
  it('records a score with no rescore flag anywhere on the wire', () => {
    gql.payload = { recordWorkflowRunGate: {} };
    api.recordWorkflowRunGate('r2', 1, null).subscribe();
    expect(last().variables).toEqual({ runId: 'r2', gateExit: 1, gateCollected: null });
    expect(last().op).not.toContain('rescore');
  });

  it('replaces a score only through the separately named mutation', () => {
    gql.payload = { recordWorkflowRunRescore: {} };
    api.recordWorkflowRunRescore('r2', 1, 23).subscribe();
    expect(last().op).toContain('recordWorkflowRunRescore');
    expect(last().variables).toEqual({ runId: 'r2', gateExit: 1, gateCollected: 23 });
  });

  // THE EXECUTOR. `runWorkflowRunGate` measures the score by running the plan's gate; the re-run is
  // a separate field for the same reason the re-score is.
  it('EXECUTES a run\u2019s gate, and asks for the run back with its score and scoreability', () => {
    gql.payload = { runWorkflowRunGate: {} };
    api.runWorkflowRunGate('r2', '04-leases').subscribe();
    expect(last().op).toContain('runWorkflowRunGate');
    expect(last().variables).toEqual({ runId: 'r2', phaseId: '04-leases' });
    for (const field of ['gateExit', 'gateCollected', 'scoreable', 'notScoreableReason']) {
      expect(last().op, `the executed run must come back with ${field}`).toContain(field);
    }
  });

  it('re-runs a gate only through the separately named mutation, never a flag', () => {
    gql.payload = { runWorkflowRunRescore: {} };
    api.runWorkflowRunRescore('r2', null).subscribe();
    expect(last().op).toContain('runWorkflowRunRescore');
    expect(last().variables).toEqual({ runId: 'r2', phaseId: null });
    for (const sent of gql.sent) {
      expect(sent.op, 'no arena operation may carry a rescore flag').not.toMatch(/\$rescore/);
    }
  });

  it('asks the HOST for the repo identity instead of deriving one from a path', () => {
    gql.payload = { repoIdentity: 'github.com/lokalcamp/johnnyone' };
    api.repoIdentity('/home/creepy/Documents/Workspace/personal/apps/johnnyone').subscribe();
    expect(last().op).toContain('repoIdentity');
    expect(last().variables).toEqual({
      workspacePath: '/home/creepy/Documents/Workspace/personal/apps/johnnyone',
    });
  });

  it('creates a version from the WORKSPACE PATH — repo is the host\u2019s to decide', () => {
    gql.payload = { createWorkflowVersion: {} };
    api.createWorkflowVersion({
      workspacePath: '/w/johnnyone', phaseType: 'dev-phase', family: 'glm',
      role: 'a', instruction: 'b', contract: 'c', hop: '{}', fanOut: 1,
    }).subscribe();
    expect(last().op).toContain('$workspacePath: String!');
    expect(last().op).not.toMatch(/\$repo\b/);
    expect(JSON.stringify(last().variables)).not.toMatch(/"repo"/);
  });

  it('promotes only with an explicit confirmed flag — the arena’s one irreversible step', () => {
    gql.payload = { updateWorkflowChampion: {} };
    api.updateWorkflowChampion('r2', 'p1', true).subscribe();
    expect(last().op).toContain('updateWorkflowChampion');
    expect(last().variables).toEqual({ runId: 'r2', prefId: 'p1', confirmed: true });
  });

  it('rejects with a reason and returns the resulting search state', () => {
    gql.payload = { recordWorkflowRejection: {} };
    api.recordWorkflowRejection('r2', 'the gate never touches the cashier path').subscribe();
    expect(last().variables).toEqual({ runId: 'r2', reason: 'the gate never touches the cashier path' });
    expect(last().op).toContain('halted');
  });

  it('resumes a halted search as its own confirmed decision, never as a side effect of promoting', () => {
    gql.payload = { updateWorkflowSearchResumed: {} };
    api.updateWorkflowSearchResumed('johnnyone', 'dev-phase', 'glm', 'added a coverage threshold', true).subscribe();
    expect(last().op).toContain('updateWorkflowSearchResumed');
    expect(last().variables).toEqual({
      repo: 'johnnyone', phaseType: 'dev-phase', family: 'glm',
      reason: 'added a coverage threshold', confirmed: true,
    });
  });

  it('never sends a `comparable` argument — the host computes it from the runs', () => {
    gql.payload = { recordWorkflowPref: {} };
    api.recordWorkflowPref('r2', 'r1', 'passed the gate').subscribe();
    expect(JSON.stringify(last().variables)).not.toContain('comparable');
    expect(last().op).not.toContain('$comparable');
  });

  // THE EXECUTE door. A run could be CREATED and then never RUN from the console, because nothing
  // in the web app called `executeWorkflowRun`. Same run-row selection as the gate operations, and
  // the same rule the worker follows: `verify`/`binary` are relayed only when actually supplied.
  it('EXECUTES a phase: sends the runId and asks for the run back with its score', () => {
    gql.payload = { executeWorkflowRun: {} };
    api.executeWorkflowRun('r7').subscribe();
    expect(last().op).toContain('executeWorkflowRun');
    expect(last().variables).toEqual({ runId: 'r7' });
    // gateExit is THE score, and null there means the gate never ran — not a pass.
    expect(last().op).toContain('gateExit');
  });

  it('EXECUTES with the same run selection the gate operations use, so no field is lost', () => {
    gql.payload = { executeWorkflowRun: {} };
    api.executeWorkflowRun('r7').subscribe();
    const execOp = last().op;
    for (const field of [
      'gateExit', 'gateCollected', 'seatsHeld', 'fleetBusy', 'startedAt', 'endedAt',
      'scoreable', 'notScoreableReason',
    ]) {
      expect(execOp, `the executed run must come back with ${field}`).toContain(field);
    }
  });

  it('EXECUTES a phase with a supplied non-empty verify and binary sent as variables', () => {
    gql.payload = { executeWorkflowRun: {} };
    api.executeWorkflowRun('r7', 'cargo test -p leases', 'kloo').subscribe();
    expect(last().op).toContain('$verify: String');
    expect(last().op).toContain('$binary: String');
    expect(last().variables).toEqual({ runId: 'r7', verify: 'cargo test -p leases', binary: 'kloo' });
  });

  // Absent, NOT null. A null on the wire reads as "the caller explicitly wants no verify command",
  // which is an override of the host's rule; the host owns that rule and must apply its default.
  it('EXECUTES with an omitted verify/binary ABSENT from the variables — the host defaults them', () => {
    gql.payload = { executeWorkflowRun: {} };
    api.executeWorkflowRun('r7').subscribe();
    expect(last().variables).toEqual({ runId: 'r7' });
    expect(JSON.stringify(last().variables)).not.toContain('verify');
    expect(JSON.stringify(last().variables)).not.toContain('binary');

    api.executeWorkflowRun('r7', null, null).subscribe();
    expect(last().variables).toEqual({ runId: 'r7' });

    api.executeWorkflowRun('r7', undefined, undefined).subscribe();
    expect(last().variables).toEqual({ runId: 'r7' });
  });

  it('EXECUTES with a blank or whitespace-only verify/binary ABSENT, not relayed as a command', () => {
    gql.payload = { executeWorkflowRun: {} };
    api.executeWorkflowRun('r7', '', '   ').subscribe();
    expect(last().variables).toEqual({ runId: 'r7' });
    api.executeWorkflowRun('r7', '  \t\n ', '').subscribe();
    expect(last().variables).toEqual({ runId: 'r7' });
    // A supplied one still rides along even when the other is blank.
    api.executeWorkflowRun('r7', '   ', 'npm run verify').subscribe();
    expect(last().variables).toEqual({ runId: 'r7', binary: 'npm run verify' });
  });

  it('EXECUTES with NO default command or binary anywhere on the web wire', () => {
    gql.payload = { executeWorkflowRun: {} };
    api.executeWorkflowRun('r7').subscribe();
    const sent = last();
    expect(JSON.stringify(sent.variables)).not.toMatch(/cargo|npm|npx|vitest|kloo|make/);
    expect(sent.op).not.toMatch(/default/i);
  });

  it('EXECUTES and maps the run row the mutation returned — the score, not a wrapper', () => {
    const row = {
      id: 'r7', workflowId: 'w3', planId: 'p9', seed: 4, gateExit: 0, gateCollected: 12,
      seatsHeld: '{"kloo":2}', fleetBusy: '{"running":1}', startedAt: '2026-01-01T00:00:00Z',
      endedAt: null, scoreable: true, notScoreableReason: null,
    };
    gql.payload = { executeWorkflowRun: row };
    let got: unknown;
    api.executeWorkflowRun('r7', 'true').subscribe((r) => (got = r));
    expect(got).toEqual(row);
    // A run whose gate never ran comes back with gateExit null — the caller must not read that 0.
    gql.payload = { executeWorkflowRun: { ...row, gateExit: null } };
    api.executeWorkflowRun('r7').subscribe((r) => (got = r));
    expect((got as WorkflowRun).gateExit).toBeNull();
  });

  it('hoists ONE workflow selection set: the detail read and the list ask for the same fields', () => {
    gql.payload = { workflows: [] };
    api.workflows({}).subscribe();
    const listOp = last().op;
    gql.payload = { workflow: {} };
    api.workflow('w1').subscribe();
    const oneOp = last().op;
    for (const field of ['role', 'instruction', 'contract', 'hop', 'fanOut', 'parentId', 'version']) {
      expect(listOp).toContain(field);
      expect(oneOp).toContain(field);
    }
  });
});
