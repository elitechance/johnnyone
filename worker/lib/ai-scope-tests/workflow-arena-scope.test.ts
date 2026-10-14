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
        {
          repo: 'r',
          phaseType: 'dev-phase',
          family: 'glm',
          role: 'ro',
          instruction: 'in',
          contract: 'co',
          hop: '{}',
          fanOut: 3,
        },
        {
          repo: 'r',
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
        { workflowId: 'w1', seatsHeld: '{"glm":3}', fleetBusy: '{}' },
        {
          workflowId: 'w1',
          planId: null,
          // An omitted seed is the first replay index, not "unset".
          seed: 0,
          seatsHeld: '{"glm":3}',
          fleetBusy: '{}',
        },
      ],
      [
        '../../resolvers/ai/record-workflow-run-gate',
        'record_workflow_run_gate',
        { runId: 'r1', gateExit: 0 },
        // The advisory count relays as NULL when omitted — never as 0, which would be the
        // different and much stronger claim "this gate collected nothing".
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
    const gate = fieldLine('recordWorkflowRunGate');
    expect(gate).toContain('gateExit: Int!');
    expect(gate).toContain('gateCollected: Int');
    expect(gate).not.toContain('gateCollected: Int!');

    // No `comparable` argument anywhere: the host computes it from the two runs' own seat budgets,
    // so a client cannot assert a comparability it does not have.
    const pref = fieldLine('recordWorkflowPref');
    expect(pref).toBeTruthy();
    expect(pref).not.toContain('comparable');

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
    const reads = [
      'list_workflows',
      'get_workflow',
      'list_workflow_runs',
      'get_workflow_search_state',
    ];
    const writes = [
      'create_workflow_version',
      'start_workflow_run',
      'record_workflow_run_gate',
      'record_workflow_pref',
      'update_workflow_champion',
      'record_workflow_rejection',
    ];
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
