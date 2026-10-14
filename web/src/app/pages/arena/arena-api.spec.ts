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
    api.createWorkflowVersion({ repo: 'r', phaseType: 'p', family: 'f', role: 'a', instruction: 'b', contract: 'c', hop: '{}', fanOut: 2 }).subscribe();
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

  it('asks for a re-score BY NAME, so a second result cannot overwrite the first by accident', () => {
    gql.payload = { recordWorkflowRunGate: {} };
    api.recordWorkflowRunGate('r2', 1, null).subscribe();
    expect(last().variables).toEqual({ runId: 'r2', gateExit: 1, gateCollected: null, rescore: false });
    api.recordWorkflowRunGate('r2', 1, null, true).subscribe();
    expect(last().variables!['rescore']).toBe(true);
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
