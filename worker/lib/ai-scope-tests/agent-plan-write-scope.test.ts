import { describe, it, expect, vi, beforeEach } from 'vitest';
import * as desktopRpcMod from '../../lib/runtime/desktop-rpc';
import { authedCtx } from '../auth/test-authed-ctx';

const mockCtx = authedCtx({ tenantId: 't1', userId: 'u1' });

// The schema, inlined at build time. `fs.readFileSync` does not work here — the vitest-pool-workers
// runtime is workerd, which has no filesystem — so this follows resolver-auth-audit.test.ts and
// pulls the raw text in through the bundler.
const schemaSources = import.meta.glob('../../schema/johnnyone-ai.graphql', {
  query: '?raw',
  import: 'default',
  eager: true,
}) as Record<string, string>;
const schemaSource = Object.values(schemaSources)[0] ?? '';

describe('AgentPlan WRITE scope (tenant isolation)', () => {
  beforeEach(() => {
    vi.restoreAllMocks();
  });

  it('create/start/amend/stopped/title/delete: ctx.auth passthrough (B call reaches with B auth)', async () => {
    const ops = [
      ['../../resolvers/ai/create-agent-plan', 'create_agent_plan', { input: { title: 'p', brief: 'b', runType: 'development' } }],
      ['../../resolvers/ai/start-agent-plan', 'start_agent_plan', { id: 'p1', phaseId: 'ph1', phaseRunMode: 'auto' }],
      ['../../resolvers/ai/update-agent-plan-amend', 'amend_agent_plan', { id: 'p1', brief: 'new' }],
      ['../../resolvers/ai/update-agent-plan-stopped', 'stop_agent_plan', { id: 'p1' }],
      ['../../resolvers/ai/update-agent-plan-title', 'update_agent_plan_title', { id: 'p1', title: 't' }],
      ['../../resolvers/ai/delete-agent-plan', 'delete_agent_plan', { id: 'p1' }],
      // The test gate (migrations 023 / 025). Both relay the caller's own auth, like every other
      // plan write — a tenant must not be able to set or run another tenant's gate.
      ['../../resolvers/ai/update-agent-plan-test-commands', 'update_agent_plan_test_commands', { id: 'p1', phaseId: '01', commands: ['npm test'] }],
      ['../../resolvers/ai/run-agent-plan-test-gate', 'run_agent_plan_test_gate', { id: 'p1', phaseId: '01' }],
    ] as const;

    for (const [file, method, args] of ops) {
      const spy = vi.spyOn(desktopRpcMod, 'desktopRpc').mockResolvedValueOnce({} as any);
      const mod = await import(file);
      await mod.default(null, args, mockCtx as any);
      expect(spy).toHaveBeenCalledWith(mockCtx, method, expect.anything());
    }

    // cross
    const b = authedCtx({ tenantId: 'tB', userId: 'uB' });
    const spyB = vi.spyOn(desktopRpcMod, 'desktopRpc').mockResolvedValueOnce({});
    const { default: delB } = await import('../../resolvers/ai/delete-agent-plan');
    await delB(null, { id: 'p1' }, b as any);
    expect(spyB).toHaveBeenCalledWith(expect.objectContaining({ auth: expect.objectContaining({ tenantId: 'tB', userId: 'uB' }) }), 'delete_agent_plan', { id: 'p1' });
  });

  it('test gate: phaseId defaults to the plan level, and clearing is never implicit', async () => {
    // The host reads these exact keys. `phaseId: null` selects the PLAN's gate rather than a
    // phase's, and `clear` defaults to FALSE.
    //
    // This test previously asserted the opposite — that an absent `commands` arrives as [] and
    // CLEARS the gate. That was the bug: `updateAgentPlanTestCommands(id: "x")`, a call that
    // forgot its optional arguments, silently wiped the Initiative-wide gate on a mutation whose
    // name sounds additive. `commands` is now non-null in the schema and the host rejects an empty
    // list unless `clear: true` is passed, so the destructive outcome is unreachable.
    const spy = vi.spyOn(desktopRpcMod, 'desktopRpc').mockResolvedValueOnce({} as any);
    const { default: setCommands } = await import('../../resolvers/ai/update-agent-plan-test-commands');
    await setCommands(null, { id: 'p1', commands: ['npm test'] }, mockCtx as any);
    expect(spy).toHaveBeenCalledWith(mockCtx, 'update_agent_plan_test_commands', {
      id: 'p1',
      phaseId: null,
      commands: ['npm test'],
      clear: false,
    });

    // Clearing has to be asked for, and the flag is relayed as given.
    const clearSpy = vi.spyOn(desktopRpcMod, 'desktopRpc').mockResolvedValueOnce({} as any);
    await setCommands(null, { id: 'p1', phaseId: '01', commands: [], clear: true }, mockCtx as any);
    expect(clearSpy).toHaveBeenCalledWith(mockCtx, 'update_agent_plan_test_commands', {
      id: 'p1',
      phaseId: '01',
      commands: [],
      clear: true,
    });

    // The schema is the other half of this guarantee: `commands` must be NON-NULL so that
    // omitting it is a validation error rather than a silent clear.
    const line = schemaSource
      .split('\n')
      .map((l) => l.trim())
      // Skip COMMENT lines: the doc comment above the field mentions the mutation by name
      // (explaining this very bug), and matching it instead of the field made this assertion
      // read the prose rather than the signature.
      .filter((l) => !l.startsWith('#'))
      .find((l) => l.includes('updateAgentPlanTestCommands('));
    expect(schemaSource, 'schema source must be inlined').toContain('type AgentPlan {');
    expect(line, 'mutation must exist in the schema').toBeTruthy();
    expect(line).toContain('commands: [String!]!');
    expect(line).toContain('clear: Boolean');

    const runSpy = vi.spyOn(desktopRpcMod, 'desktopRpc').mockResolvedValueOnce({} as any);
    const { default: runGate } = await import('../../resolvers/ai/run-agent-plan-test-gate');
    await runGate(null, { id: 'p1' }, mockCtx as any);
    expect(runSpy).toHaveBeenCalledWith(mockCtx, 'run_agent_plan_test_gate', {
      id: 'p1',
      phaseId: null,
    });
  });
});
