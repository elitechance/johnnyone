import { describe, it, expect, vi, beforeEach } from 'vitest';
import * as desktopRpcMod from '../../lib/runtime/desktop-rpc';
import { isReadRpcCacheable, invalidatedReadsFor } from '../../lib/runtime/desktop-rpc-cache';
import { authedCtx } from '../auth/test-authed-ctx';

/**
 * The workflow arena's resolver seam (desktop migration 026).
 *
 * Mirrors agent-plan-write-scope.test.ts. Three things are pinned here and nowhere else:
 *
 *  1. every arena resolver relays the CALLER'S OWN ctx, so one tenant cannot drive another's
 *     arena — the champion decides what J1 executes for a whole repo;
 *  2. the exact host RPC method name and the exact param keys. Nothing else in the worker tree
 *     drives these, so a renamed key compiles, passes every other gate, and fails in production;
 *  3. the arguments that must NOT exist (`comparable`) and the ones that must be NON-NULL
 *     (`confirmed`, `gateExit`, `reason`), which is where the destructive-default bug lives.
 */
const mockCtx = authedCtx({ tenantId: 't1', userId: 'u1' });

const schemaSources = import.meta.glob('../../schema/johnnyone-ai.graphql', {
  query: '?raw',
  import: 'default',
  eager: true,
}) as Record<string, string>;
const schemaSource = Object.values(schemaSources)[0] ?? '';

/** A schema field's signature line, with the `#` doc comments above it skipped. */
function fieldLine(name: string): string | undefined {
  return schemaSource
    .split('\n')
    .map((line) => line.trim())
    .filter((line) => !line.startsWith('#'))
    .find((line) => line.startsWith(`${name}(`) || line.startsWith(`${name}:`));
}

describe('workflow arena resolver seam', () => {
  beforeEach(() => {
    vi.restoreAllMocks();
  });

  it('relays the caller own auth to the exact host RPC with the exact param keys', async () => {
    const cases: [string, string, Record<string, unknown>, Record<string, unknown>][] = [
      [
        '../../resolvers/ai/workflows',
        'list_workflows',
        { repo: 'johnnyone' },
        { repo: 'johnnyone', phaseType: null, family: null },
      ],
      ['../../resolvers/ai/workflow', 'get_workflow', { id: 'w1' }, { id: 'w1' }],
      [
        '../../resolvers/ai/workflow-runs',
        'list_workflow_runs',
        { workflowId: 'w1' },
        { workflowId: 'w1' },
      ],
      [
        '../../resolvers/ai/workflow-search-state',
        'get_workflow_search_state',
        { repo: 'r', phaseType: 'dev-phase', family: 'glm' },
        { repo: 'r', phaseType: 'dev-phase', family: 'glm' },
      ],
      [
        '../../resolvers/ai/create-workflow-version',
        'create_workflow_version',
        // `repo` is NOT an argument: the host derives the identity from the workspace path,
        // because a client-supplied identity is a client-supplied champion key.
        {
          workspacePath: '/w/johnnyone',
          phaseType: 'dev-phase',
          family: 'glm',
          role: 'ro',
          instruction: 'in',
          contract: 'co',
          hop: '{}',
          fanOut: 3,
        },
        {
          workspacePath: '/w/johnnyone',
          phaseType: 'dev-phase',
          family: 'glm',
          parentId: null,
          role: 'ro',
          instruction: 'in',
          contract: 'co',
          hop: '{}',
          fanOut: 3,
        },
      ],
      [
        '../../resolvers/ai/start-workflow-run',
        'start_workflow_run',
        // The payload deliberately still carries seatsHeld/fleetBusy — the parameters this
        // mutation used to take. They must NOT be relayed: the caller is a browser and cannot
        // measure either, so the host takes both snapshots. If they reappear below, the UI can
        // declare its own contention again and `comparable` goes back to "did two clients type the
        // same numbers".
        { workflowId: 'w1', seatsHeld: '{"attacker":999}', fleetBusy: '{"attacker":999}' } as any,
        {
          workflowId: 'w1',
          planId: null,
          // An omitted seed is the first replay index, not "unset".
          seed: 0,
          // An omitted seats override means "whatever the workflow declares".
          seats: null,
        },
      ],
      [
        '../../resolvers/ai/workflow-prefs',
        'list_workflow_prefs',
        { workflowId: 'w1' },
        { workflowId: 'w1' },
      ],
      [
        '../../resolvers/ai/repo-identity',
        'get_repo_identity',
        { workspacePath: '/w/johnnyone' },
        { workspacePath: '/w/johnnyone' },
      ],
      [
        '../../resolvers/ai/run-workflow-run-gate',
        'run_workflow_run_gate',
        { runId: 'r1' },
        { runId: 'r1', phaseId: null },
      ],
      [
        '../../resolvers/ai/record-workflow-run-rescore',
        'record_workflow_run_rescore',
        { runId: 'r1', gateExit: 1 },
        { runId: 'r1', gateExit: 1, gateCollected: null },
      ],
      [
        '../../resolvers/ai/run-workflow-run-rescore',
        'rescore_workflow_run_gate',
        { runId: 'r1' },
        { runId: 'r1', phaseId: null },
      ],
      [
        '../../resolvers/ai/record-workflow-run-gate',
        'record_workflow_run_gate',
        { runId: 'r1', gateExit: 0 },
        // The advisory count relays as NULL when omitted — never as 0, which would be the
        // different and much stronger claim "this gate collected nothing".
        // No `rescore` key at all: a second result goes through its own mutation, because the
        // separate NAME is the guard and a boolean on this field was the guard the host's own
        // rationale rejected.
        { runId: 'r1', gateExit: 0, gateCollected: null },
      ],
      [
        '../../resolvers/ai/record-workflow-pref',
        'record_workflow_pref',
        { winnerRunId: 'a', loserRunId: 'b' },
        { winnerRunId: 'a', loserRunId: 'b', reason: null },
      ],
      [
        '../../resolvers/ai/update-workflow-champion',
        'update_workflow_champion',
        { runId: 'r1', confirmed: true },
        { runId: 'r1', prefId: null, confirmed: true },
      ],
      [
        '../../resolvers/ai/record-workflow-rejection',
        'record_workflow_rejection',
        { runId: 'r1', reason: 'too slow' },
        { runId: 'r1', reason: 'too slow' },
      ],
      [
        '../../resolvers/ai/update-workflow-search-resumed',
        'update_workflow_search_resumed',
        { repo: 'r', phaseType: 'dev-phase', family: 'glm', reason: 'retrying', confirmed: true },
        {
          repo: 'r',
          phaseType: 'dev-phase',
          family: 'glm',
          reason: 'retrying',
          confirmed: true,
        },
      ],
    ];

    for (const [file, method, args, expected] of cases) {
      const spy = vi.spyOn(desktopRpcMod, 'desktopRpc').mockResolvedValueOnce({} as any);
      const mod = await import(file);
      await mod.default(null, args, mockCtx as any);
      expect(spy, `${method} must be the relayed method`).toHaveBeenCalledWith(
        mockCtx,
        method,
        expected,
      );
    }

    // Cross-tenant: B's call must arrive with B's identity. The champion governs a whole repo's
    // execution, so this is the one that matters most.
    const b = authedCtx({ tenantId: 'tB', userId: 'uB' });
    const spyB = vi.spyOn(desktopRpcMod, 'desktopRpc').mockResolvedValueOnce({} as any);
    const { default: promote } = await import('../../resolvers/ai/update-workflow-champion');
    await promote(null, { runId: 'r1', confirmed: true }, b as any);
    expect(spyB).toHaveBeenCalledWith(
      expect.objectContaining({
        auth: expect.objectContaining({ tenantId: 'tB', userId: 'uB' }),
      }),
      'update_workflow_champion',
      { runId: 'r1', prefId: null, confirmed: true },
    );
  });

  it('an omitted confirmed relays as false, so a forgetful call cannot promote', async () => {
    const spy = vi.spyOn(desktopRpcMod, 'desktopRpc').mockResolvedValueOnce({} as any);
    const { default: promote } = await import('../../resolvers/ai/update-workflow-champion');
    // The schema makes this a validation error; the resolver still fails SAFE if it ever gets here,
    // because the host's default is the destructive-free one and this relays it explicitly.
    await promote(null, { runId: 'r1' } as any, mockCtx as any);
    expect(spy).toHaveBeenCalledWith(mockCtx, 'update_workflow_champion', {
      runId: 'r1',
      prefId: null,
      confirmed: false,
    });
  });

  it('the schema pins the non-null arguments and omits comparable entirely', () => {
    expect(schemaSource, 'schema must be inlined').toContain('type Workflow {');

    // `confirmed` NON-NULL: promotion is irreversible, so an incomplete call must be a validation
    // error rather than a promotion — the same lesson as updateAgentPlanTestCommands' silent clear.
    const promote = fieldLine('updateWorkflowChampion');
    expect(promote, 'updateWorkflowChampion must exist').toBeTruthy();
    expect(promote).toContain('confirmed: Boolean!');
    expect(promote).toContain('prefId: String');

    // `gateExit` NON-NULL: it is the score, and a missing score must not arrive as a 0 that reads
    // as a pass. `gateCollected` NULLABLE: unparseable output is a normal result.
    // Float, not Int, on every numeric: a 32-bit `Int!` that cannot serialise nulls the whole
    // non-null chain up to the response root. `gateExit` stays NON-NULL (it is the score, and a
    // missing score must not arrive as a 0 that reads as a pass) and `gateCollected` stays NULLABLE
    // (unparseable output is a normal result).
    const gate = fieldLine('recordWorkflowRunGate');
    expect(gate).toContain('gateExit: Float!');
    expect(gate).toContain('gateCollected: Float');
    expect(gate).not.toContain('gateCollected: Float!');
    // A re-score is its OWN field, not an argument. The separate name is the guard.
    expect(gate, 'rescore must not be an argument').not.toContain('rescore');
    expect(fieldLine('recordWorkflowRunRescore')).toContain('gateExit: Float!');
    expect(fieldLine('runWorkflowRunRescore')).toContain('phaseId: String');

    // Lifting a halt is its own mutation, with both of its arguments non-null. A promotion must
    // never be the thing that clears a halt again.
    const resume = fieldLine('updateWorkflowSearchResumed');
    expect(resume, 'updateWorkflowSearchResumed must exist').toBeTruthy();
    expect(resume).toContain('reason: String!');
    expect(resume).toContain('confirmed: Boolean!');

    // No `comparable` argument anywhere: the host computes it from the two runs themselves, so a
    // client cannot assert a comparability it does not have.
    const pref = fieldLine('recordWorkflowPref');
    expect(pref).toBeTruthy();
    expect(pref).not.toContain('comparable');

    // And no seat/contention arguments on startWorkflowRun: those are the host's measurement.
    const start = fieldLine('startWorkflowRun');
    expect(start, 'startWorkflowRun must exist').toBeTruthy();
    expect(start, 'seatsHeld must not be a client argument').not.toContain('seatsHeld');
    expect(start, 'fleetBusy must not be a client argument').not.toContain('fleetBusy');
    expect(start, 'and the family is the workflow\'s, never the caller\'s').not.toContain('family');
    expect(start).toContain('seats: Float');

    // The read that makes a recorded preference discoverable without issuing a write.
    expect(fieldLine('workflowPrefs')).toContain('workflowId: ID!');

    // `repo` is gone from createWorkflowVersion: the host derives the identity, and this read is
    // how a client learns it.
    const create = fieldLine('createWorkflowVersion');
    expect(create, 'createWorkflowVersion must exist').toBeTruthy();
    expect(create).toContain('workspacePath: String!');
    expect(create, 'repo must not be a client argument').not.toContain('repo:');
    expect(fieldLine('repoIdentity')).toContain('workspacePath: String!');

    // The executor that closes the loop, with the same rescore argument as the recorder.
    const execute = fieldLine('runWorkflowRunGate');
    expect(execute, 'runWorkflowRunGate must exist').toBeTruthy();
    expect(execute, 'rescore must not be an argument here either').not.toContain('rescore');

    // Scoreability is served per run, so a client can say WHY a run cannot compete.
    expect(schemaSource).toContain('scoreable: Boolean!');
    expect(schemaSource).toContain('notScoreableReason: String');

    // A rejection's reason is non-null: three of them halt the search and the reasons are all a
    // halted search has to show.
    expect(fieldLine('recordWorkflowRejection')).toContain('reason: String!');

    // And the type still carries `comparable` as an output field — the point is that it is
    // computed, not that it is absent.
    expect(schemaSource).toContain('comparable: Boolean!');
  });

  /**
   * The INVALIDATED_READS question, answered as an invariant rather than by adding dead entries.
   *
   * `invalidateCachedRpc` only matters for reads that are in `CACHE_TTL_MS`. None of the arena's
   * reads are cached, so there is nothing for the arena's writes to invalidate and adding rows to
   * INVALIDATED_READS would be dead weight that reads as coverage.
   *
   * This test is the guard that keeps that true: the moment someone caches an arena read, it fails
   * and names the write that must then invalidate it. Promotion in particular must be readable
   * back immediately — a champion that is still the old one for 3 seconds after a human confirmed
   * the new one is exactly the "the button did nothing" failure the cache map was written for.
   */
  it('arena reads are uncached, so no write needs an INVALIDATED_READS entry', () => {
    const reads: string[] = [
      'list_workflows',
      'get_workflow',
      'list_workflow_runs',
      'get_workflow_search_state',
    ];
    const writes: string[] = [
      'create_workflow_version',
      'start_workflow_run',
      'record_workflow_run_gate',
      'record_workflow_pref',
      'update_workflow_champion',
      'record_workflow_rejection',
      'update_workflow_search_resumed',
    ];
    reads.push('list_workflow_prefs', 'get_repo_identity');
    writes.push('run_workflow_run_gate', 'rescore_workflow_run_gate', 'record_workflow_run_rescore');
    for (const read of reads) {
      expect(
        isReadRpcCacheable(read),
        `${read} is now cached: add it to INVALIDATED_READS under the writes that stale it`,
      ).toBe(false);
    }
    for (const write of writes) {
      expect(invalidatedReadsFor(write)).toEqual([]);
    }
    // Non-vacuous: the helpers do report a cached read and a mapped write for the plan surface.
    expect(isReadRpcCacheable('get_agent_plan')).toBe(true);
    expect(invalidatedReadsFor('update_agent_plan_test_commands')).toContain('get_agent_plan');
  });
});
