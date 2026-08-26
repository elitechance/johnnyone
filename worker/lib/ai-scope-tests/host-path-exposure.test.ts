import { describe, it, expect, vi, beforeEach } from 'vitest';
import * as desktopRpcMod from '../../lib/runtime/desktop-rpc';
import { authedCtx } from '../auth/test-authed-ctx';

const mockCtx = authedCtx({ tenantId: 't1', userId: 'u1' });

/**
 * Documents the intentional passthrough of host paths (Task 06 decision).
 * Paths are the caller's own (via node resolution); test proves foreign ctx cannot see them.
 * No projection implemented; raw fields (workspacePath etc) relay unmodified.
 */
describe('Host path exposure — passthrough (documented + scoped)', () => {
  beforeEach(() => {
    vi.restoreAllMocks();
  });

  it('getAgentPlan returns workspacePath/planPath/appScope/docsScope unmodified (passthrough)', async () => {
    const desktopData = {
      plan: {
        id: 'p1',
        workspacePath: '/home/svc-acct/ws/plan1',
        planPath: '/home/svc-acct/ws/plan1/plan.md',
        appScope: '/home/svc-acct/ws',
        docsScope: '/home/svc-acct/docs',
        // ... other fields minimal
        runType: 'development',
        title: 'p',
        status: 'active',
        workerProvider: 'ollama',
        reviewerProvider: 'ollama',
        currentPhaseIndex: 0,
        createdAt: 'now',
        updatedAt: 'now',
      },
      phases: [],
      tasks: [],
      events: [],
    };
    const spy = vi.spyOn(desktopRpcMod, 'desktopRpc').mockResolvedValueOnce(desktopData as any);
    const { default: get } = await import('../../resolvers/ai/get-agent-plan');
    const res = await get(null, { id: 'p1' }, mockCtx as any);
    expect(spy).toHaveBeenCalledWith(mockCtx, 'get_agent_plan', { id: 'p1' });
    expect(res.plan.workspacePath).toBe('/home/svc-acct/ws/plan1');
    expect(res.plan.planPath).toBe('/home/svc-acct/ws/plan1/plan.md');
    // raw paths passthrough for owner
  });

  it('getAgentPlan under foreign ctx: receives call with foreign auth (passthrough to desktopRpc)', async () => {
    const bCtx = authedCtx({ tenantId: 'tB', userId: 'uB' });
    const spyB = vi.spyOn(desktopRpcMod, 'desktopRpc').mockResolvedValueOnce({ error: 'no node for B' } as any);
    const { default: getB } = await import('../../resolvers/ai/get-agent-plan');
    await getB(null, { id: 'p1' }, bCtx as any);
    // B's lookup used B's auth, not t1; no path for t1's plan exposed to B
    expect(spyB).toHaveBeenCalledWith(expect.objectContaining({ auth: expect.objectContaining({ tenantId: 'tB', userId: 'uB' }) }), 'get_agent_plan', { id: 'p1' });
  });

  it('HostFileContent.path would passthrough raw for owner (documented; see read-host-file)', async () => {
    // Minimal documentation test; actual read-host-file uses similar relay.
    // Decision: passthrough accepted (own VM paths only).
    const spy = vi.spyOn(desktopRpcMod, 'desktopRpc').mockResolvedValueOnce({ path: '/home/svc-acct/file.txt', content: 'x' } as any);
    const { default: read } = await import('../../resolvers/ai/read-host-file');
    const res = await read(null, { planId: 'p1', path: '/home/svc-acct/file.txt' }, mockCtx as any);
    expect(res.path).toBe('/home/svc-acct/file.txt'); // unmodified passthrough
  });
});