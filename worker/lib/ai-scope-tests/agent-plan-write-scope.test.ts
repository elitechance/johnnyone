import { describe, it, expect, vi, beforeEach } from 'vitest';
import * as desktopRpcMod from '../../lib/runtime/desktop-rpc';
import { authedCtx } from '../auth/test-authed-ctx';

const mockCtx = authedCtx({ tenantId: 't1', userId: 'u1' });

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

  it('test gate: phaseId defaults to null (plan level) and absent commands clear the gate', async () => {
    // The host reads these exact keys. `phaseId: null` is what selects the PLAN's gate rather
    // than a phase's, and an absent `commands` must arrive as [] — the CLEAR signal — not as
    // undefined, which would drop the key and leave the gate untouched.
    const spy = vi.spyOn(desktopRpcMod, 'desktopRpc').mockResolvedValueOnce({} as any);
    const { default: setCommands } = await import('../../resolvers/ai/update-agent-plan-test-commands');
    await setCommands(null, { id: 'p1' }, mockCtx as any);
    expect(spy).toHaveBeenCalledWith(mockCtx, 'update_agent_plan_test_commands', {
      id: 'p1',
      phaseId: null,
      commands: [],
    });

    const runSpy = vi.spyOn(desktopRpcMod, 'desktopRpc').mockResolvedValueOnce({} as any);
    const { default: runGate } = await import('../../resolvers/ai/run-agent-plan-test-gate');
    await runGate(null, { id: 'p1' }, mockCtx as any);
    expect(runSpy).toHaveBeenCalledWith(mockCtx, 'run_agent_plan_test_gate', {
      id: 'p1',
      phaseId: null,
    });
  });
});
