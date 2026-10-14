// The arena's client wire. Drives the REAL JohnnyApiService with a stub GraphQLClient and inspects
// the operation text and variables it sends, which is the only place a missing field or a renamed
// argument shows up before a deploy. Two things it exists to hold:
//   * the selection sets are HOISTED (the sessionFields / agentPlanRunFields precedent) — one copy,
//     so a field added for one screen cannot be missing on another;
//   * `gateCollected` and `effectiveTestCommands` are actually ASKED FOR. A UI that renders "not
//     reported" because it never requested the field is indistinguishable from a gate that reported
//     nothing, which is exactly the confusion the count exists to prevent.
import '@angular/compiler';
import { describe, it, expect, beforeEach } from 'vitest';
import { TestBed } from '@angular/core/testing';
import { of } from 'rxjs';
import '../../../test-setup';
import { JohnnyApiService } from '../../../../../ui/src/services/johnny-api.service';
import { GraphQLClient } from '../../../../../ui/src/services/graphql-client';

interface Sent {
  op: string;
  variables: Record<string, unknown> | undefined;
}

class StubClient {
  sent: Sent[] = [];
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

describe('JohnnyApiService — the arena wire', () => {
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

  it('lists workflows with the three independent filters', () => {
    gql.payload = { workflows: [] };
    api.workflows({ repo: 'johnnyone', phaseType: 'dev-phase' }).subscribe();
    expect(last().variables).toEqual({
      repo: 'johnnyone',
      phaseType: 'dev-phase',
      family: undefined,
    });
    expect(last().op).toContain('isChampion');
  });

  it('asks a run for gateExit AND the nullable advisory gateCollected', () => {
    gql.payload = { workflowRuns: [] };
    api.workflowRuns('w1').subscribe();
    expect(last().op).toContain('gateExit');
    expect(last().op).toContain('gateCollected');
    expect(last().op).toContain('seatsHeld');
  });

  it('reads the halt as data, with the reasons a halted search owes a human', () => {
    gql.payload = { workflowSearchState: {} };
    api.workflowSearchState('johnnyone', 'dev-phase', 'glm').subscribe();
    expect(last().op).toContain('halted');
    expect(last().op).toContain('consecutiveRejections');
    expect(last().op).toContain('recentRejections');
    expect(last().op).toContain('strikeLimit');
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
    expect(last().variables).toEqual({
      runId: 'r2',
      reason: 'the gate never touches the cashier path',
    });
    expect(last().op).toContain('halted');
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
