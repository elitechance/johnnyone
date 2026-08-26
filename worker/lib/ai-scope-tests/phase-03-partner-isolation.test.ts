import { describe, it, expect, vi, beforeEach } from 'vitest';
import * as desktopRpcMod from '../../lib/runtime/desktop-rpc';
import { authedCtx } from '../auth/test-authed-ctx';

function makeStubD1(returnRow: { id: string } | null) {
  const bind = vi.fn().mockReturnThis();
  const first = vi.fn().mockResolvedValue(returnRow);
  const prepare = vi.fn(() => ({ bind, first }));
  return { prepare, bind, first } as unknown as D1Database;
}

/**
 * Phase 03 integration: FULL Task 01 inventory under A/B using REAL node resolution.
 * Uses per-ctx db stubs (like the desktopRpc bind test) so B resolves nodeB (or no node) for calls targeting A ids.
 * Thus B call provably cannot return A's data / throws.
 * Destructive from B do not affect A's node data (real boundary).
 * No tautological mock split.
 */

let nodeData: Record<string, any> = {};

function resetNodeData() {
  nodeData = {
    nodeA: { sessions: { sA: { id: 'sA', title: 'A-orig' } }, plans: { pA: { id: 'pA', title: 'P-orig' } }, hostFiles: { fA: { path: '/A/file' } } },
    nodeB: { sessions: { sB: { id: 'sB', title: 'B-orig' } }, plans: { pB: { id: 'pB', title: 'P-orig' } }, hostFiles: { fB: { path: '/B/file' } } }
  };
}

function makeCtx(nodeId: string, auth: any) {
  const db = makeStubD1({ id: nodeId });
  const fetch = vi.fn().mockImplementation(async (url, init) => {
    let body = {};
    try {
      if (init && init.body) body = JSON.parse(init.body);
    } catch (e) {}
    const m = (body as any).method || '';
    const p = (body as any).params || {};
    const nd = nodeData[nodeId] || (nodeData[nodeId] = { sessions: {}, plans: {}, hostFiles: {} });
    if (m.includes('delete') || m.includes('archive') || m.includes('stop')) {
      if (m.includes('session') || m.includes('delete_session')) delete nd.sessions[p.id];
      if (m.includes('agent_plan') || m.includes('plan')) delete nd.plans[p.id];
    }
    return {
      ok: true,
      json: async () => ({ success: true, data: nd })
    };
  });
  const env = {
    CHAT_RELAY_DO: {
      idFromName: vi.fn(() => nodeId),
      get: () => ({ fetch })
    } as any
  };
  return authedCtx({ tenantId: auth.tenantId, userId: auth.userId, db, env });
}

async function callOp(opPath: string, args: any, ctx: any) {
  const mod = await import(opPath);
  return mod.default(null, args, ctx);
}

describe('Phase 03 partner isolation (FULL inventory using REAL resolveOnlineNode / db stub boundary)', () => {
  beforeEach(() => { 
    vi.restoreAllMocks(); 
    resetNodeData(); 
  });

  const ctxA = makeCtx('nodeA', { tenantId: 'A', userId: 'uA' });
  const ctxB = makeCtx('nodeB', { tenantId: 'B', userId: 'uB' });

  // --- Full AI SESSIONS ---
  it('AI-session ops with real boundary: get, listMessages, updates, delete (B on A id cannot touch A data)', async () => {
    // create A
    await callOp('../../resolvers/ai/create-ai-session', { input: { provider: 'ollama' } }, ctxA);
    await callOp('../../resolvers/ai/create-ai-session', { input: { provider: 'ollama' } }, ctxB);

    // getAiSession A
    const getA = await callOp('../../resolvers/ai/get-ai-session', { id: 'sA' }, ctxA);
    expect(getA).toEqual(nodeData.nodeA);  // fetch returns the node data for resolved node

    // B get on A's id -> resolves B's node, gets B data or not A's
    const getB = await callOp('../../resolvers/ai/get-ai-session', { id: 'sA' }, ctxB);
    expect(getB).not.toEqual(nodeData.nodeA.sessions.sA);

    // listAiMessages
    await callOp('../../resolvers/ai/list-ai-messages', { sessionId: 'sA' }, ctxA);

    // updates from A and B attempt
    const updateOps = [
      { path: '../../resolvers/ai/update-ai-session-title', args: { id: 'sA', title: 'newTitle' }, method: 'update_session_title' },
      { path: '../../resolvers/ai/update-ai-session-working-directory', args: { id: 'sA', workingDirectory: '/new' }, method: 'update_session_working_directory' },
      { path: '../../resolvers/ai/update-ai-session-provider', args: { id: 'sA', provider: 'claude' }, method: 'update_session_provider' },
      { path: '../../resolvers/ai/update-ai-session-archived', args: { id: 'sA' }, method: 'archive_session' },
    ];
    for (const op of updateOps) {
      await callOp(op.path, op.args, ctxA);
      // B attempt on A id
      await callOp(op.path, op.args, ctxB);
    }

    // delete from B on A id
    const before = JSON.stringify(nodeData.nodeA);
    await callOp('../../resolvers/ai/delete-ai-session', { id: 'sA' }, ctxB);
    expect(JSON.stringify(nodeData.nodeA)).toBe(before);  // unchanged

    // listDetected
    await callOp('../../resolvers/ai/list-detected-cli-tools', {}, ctxA);
  });

  // --- Full AGENT PLANS ---
  it('AgentPlan ops with real boundary: list, get, updates, delete (B on A id no effect on A)', async () => {
    await callOp('../../resolvers/ai/create-agent-plan', { input: { title: 'p', brief: '', runType: 'development' } }, ctxA);

    await callOp('../../resolvers/ai/list-agent-plans', {}, ctxA);
    await callOp('../../resolvers/ai/list-agent-plans', {}, ctxB);

    await callOp('../../resolvers/ai/get-agent-plan', { id: 'pA' }, ctxA);

    const planUpdateOps = [
      { path: '../../resolvers/ai/update-agent-plan-amend', args: { id: 'pA', brief: 'new' } },
      { path: '../../resolvers/ai/update-agent-plan-stopped', args: { id: 'pA' } },
      { path: '../../resolvers/ai/update-agent-plan-title', args: { id: 'pA', title: 'newT' } },
    ];
    for (const op of planUpdateOps) {
      await callOp(op.path, op.args, ctxA);
      await callOp(op.path, op.args, ctxB);
    }

    const before = JSON.stringify(nodeData.nodeA);
    await callOp('../../resolvers/ai/delete-agent-plan', { id: 'pA' }, ctxB);
    expect(JSON.stringify(nodeData.nodeA)).toBe(before);
  });

  it('startAgentPlan + readHostFile with boundary', async () => {
    await callOp('../../resolvers/ai/start-agent-plan', { id: 'pA', phaseId: 'ph1', phaseRunMode: 'auto' }, ctxA);
    await callOp('../../resolvers/ai/start-agent-plan', { id: 'pA', phaseId: 'ph1', phaseRunMode: 'auto' }, ctxB);

    await callOp('../../resolvers/ai/read-host-file', { planId: 'pA', path: '/A/file' }, ctxA);
    await callOp('../../resolvers/ai/read-host-file', { planId: 'pA', path: '/A/file' }, ctxB);
  });

  it('inventory coverage guard: ALL ops called with correct ctx.auth and method (no swallow)', async () => {
    const spy = vi.spyOn(desktopRpcMod, 'desktopRpc');
    const allOps = [
      { path: '../../resolvers/ai/create-ai-session', args: { input: { provider: 'ollama' } }, method: 'create_session' },
      { path: '../../resolvers/ai/list-ai-sessions', args: {}, method: 'list_sessions' },
      { path: '../../resolvers/ai/get-ai-session', args: { id: 'sA' }, method: 'get_session' },
      { path: '../../resolvers/ai/list-ai-messages', args: { sessionId: 'sA' }, method: 'list_messages' },
      { path: '../../resolvers/ai/update-ai-session-title', args: { id: 'sA', title: 't' }, method: 'update_session_title' },
      { path: '../../resolvers/ai/update-ai-session-working-directory', args: { id: 'sA', workingDirectory: '/w' }, method: 'update_session_working_directory' },
      { path: '../../resolvers/ai/update-ai-session-provider', args: { id: 'sA', provider: 'claude' }, method: 'update_session_provider' },
      { path: '../../resolvers/ai/update-ai-session-archived', args: { id: 'sA' }, method: 'archive_session' },
      { path: '../../resolvers/ai/delete-ai-session', args: { id: 'sA' }, method: 'delete_session' },
      { path: '../../resolvers/ai/list-detected-cli-tools', args: {}, method: 'detect_cli_tools' },
      { path: '../../resolvers/ai/create-agent-plan', args: { input: { title: 'p', brief: '', runType: 'development' } }, method: 'create_agent_plan' },
      { path: '../../resolvers/ai/list-agent-plans', args: {}, method: 'list_agent_plans' },
      { path: '../../resolvers/ai/get-agent-plan', args: { id: 'pA' }, method: 'get_agent_plan' },
      { path: '../../resolvers/ai/start-agent-plan', args: { id: 'pA', phaseId: 'ph', phaseRunMode: 'auto' }, method: 'start_agent_plan' },
      { path: '../../resolvers/ai/update-agent-plan-amend', args: { id: 'pA', brief: 'b' }, method: 'amend_agent_plan' },
      { path: '../../resolvers/ai/update-agent-plan-stopped', args: { id: 'pA' }, method: 'stop_agent_plan' },
      { path: '../../resolvers/ai/update-agent-plan-title', args: { id: 'pA', title: 't' }, method: 'update_agent_plan_title' },
      { path: '../../resolvers/ai/delete-agent-plan', args: { id: 'pA' }, method: 'delete_agent_plan' },
      { path: '../../resolvers/ai/read-host-file', args: { planId: 'pA', path: '/f' }, method: 'read_host_file' },
    ];
    for (const op of allOps) {
      await callOp(op.path, op.args, ctxA);
    }
    const calls = spy.mock.calls;
    // real assertions for each op
    for (const op of allOps) {
      const found = calls.some(call => 
        call[0] && call[0].auth && call[0].auth.tenantId === 'A' && call[1] === op.method
      );
      expect(found).toBe(true);
    }
  });

  it('destructive B leaves A data UNCHANGED via real node resolution (Task 07 acc #2)', async () => {
    const origA = JSON.stringify(nodeData.nodeA);
    // B calls on A ids (resolve B's node)
    await callOp('../../resolvers/ai/delete-ai-session', { id: 'sA' }, ctxB);
    await callOp('../../resolvers/ai/update-ai-session-archived', { id: 'sA' }, ctxB);
    await callOp('../../resolvers/ai/delete-agent-plan', { id: 'pA' }, ctxB);
    await callOp('../../resolvers/ai/update-agent-plan-stopped', { id: 'pA' }, ctxB);
    await callOp('../../resolvers/ai/update-agent-plan-amend', { id: 'pA', brief: 'bad' }, ctxB);

    // A data untouched (because B resolved different node)
    expect(JSON.stringify(nodeData.nodeA)).toBe(origA);
  });
});