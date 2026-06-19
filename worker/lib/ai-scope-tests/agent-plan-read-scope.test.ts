import { describe, it, expect, vi, beforeEach } from 'vitest';
import * as desktopRpcMod from '../../lib/runtime/desktop-rpc';

const mockCtx = {
  db: {} as any,
  env: { CHAT_RELAY_DO: {} as any },
  auth: { tenantId: 't1', userId: 'u1' },
};

describe('AgentPlan READ scope (tenant isolation)', () => {
  beforeEach(() => {
    vi.restoreAllMocks();
  });

  it('listAgentPlans + getAgentPlan: ctx.auth passthrough (different auth reaches different effective node via call)', async () => {
    let spy = vi.spyOn(desktopRpcMod, 'desktopRpc').mockResolvedValueOnce([] as any);
    const { default: list } = await import('../../resolvers/ai/list-agent-plans');
    await list(null, { status: null, runType: null, onlyExisting: false }, mockCtx as any);
    expect(spy).toHaveBeenCalledWith(mockCtx, 'list_agent_plans', expect.anything());

    spy = vi.spyOn(desktopRpcMod, 'desktopRpc').mockResolvedValueOnce({ plan: {} } as any);
    const { default: get } = await import('../../resolvers/ai/get-agent-plan');
    await get(null, { id: 'p1' }, mockCtx as any);
    expect(spy).toHaveBeenCalledWith(mockCtx, 'get_agent_plan', { id: 'p1' });

    const b = { ...mockCtx, auth: { tenantId: 'tB', userId: 'uB' } };
    const spyB = vi.spyOn(desktopRpcMod, 'desktopRpc').mockResolvedValueOnce([] as any);
    const { default: listB } = await import('../../resolvers/ai/list-agent-plans');
    await listB(null, {}, b as any);
    expect(spyB).toHaveBeenCalledWith(expect.objectContaining({ auth: b.auth }), 'list_agent_plans', expect.anything());
  });
});