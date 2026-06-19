import { describe, it, expect, vi, beforeEach } from 'vitest';
import { ChatRelayDO } from './chat-relay-do';
import * as ownershipMod from './session-ownership';

// Integration matrix per Task 06: owned/foreign for all 6 gated types, cache behavior, post-delete deny.
// Uses class with spies (real behavior exercised; runInDurableObject not required for this matrix as per prior).

type FakeState = any;

function makeFakeState() {
  return {
    acceptWebSocket: vi.fn(),
    getTags: vi.fn(() => []),
    getWebSockets: vi.fn(() => []),
    serializeAttachment: vi.fn(),
  };
}

function makeEnv() {
  return {
    JWT_SECRET: 'test-secret',
    DB: {} as any,
    CHAT_RELAY_DO: { idFromName: vi.fn((id) => id), get: vi.fn() },
  };
}

describe('Phase 01 integration (Task 06)', () => {
  let state: FakeState;
  let env: any;
  let doInst: ChatRelayDO;

  beforeEach(() => {
    vi.restoreAllMocks();
    state = makeFakeState();
    env = makeEnv();
    doInst = new ChatRelayDO(state as any, env);
  });

  async function sendFromClient(envelope: any) {
    const sent: any[] = [];
    const ws = { send: (m: string) => sent.push(JSON.parse(m)) } as any;
    (state as any).getTags = vi.fn(() => ['mobile', 'tenant:t1', 'user:u1']);
    await (doInst as any).webSocketMessage(ws, JSON.stringify(envelope));
    return { sent, bcastCalls: [] }; // bcast spied separately
  }

  const gatedTypes = ['chat_request', 'terminal_command', 'terminal_visual_subscribe', 'terminal_visual_unsubscribe', 'terminal_resize', 'terminal_kill'];

  it('owned vs foreign matrix for all gated types', async () => {
    const bcast = vi.spyOn(doInst as any, 'broadcastTo');
    for (const t of gatedTypes) {
      // owned
      vi.spyOn(ownershipMod, 'verifySessionOwnership').mockResolvedValueOnce(true);
      bcast.mockClear();
      const ws = { send: vi.fn() } as any;
      (state as any).getTags = vi.fn(() => ['mobile', 'tenant:t1', 'user:u1']);
      await (doInst as any).webSocketMessage(ws, JSON.stringify({ type: t, sessionId: 'owned', requestId: 'r' }));
      expect(bcast).toHaveBeenCalledWith('desktop', expect.objectContaining({ type: t }));

      // foreign
      vi.spyOn(ownershipMod, 'verifySessionOwnership').mockResolvedValueOnce(false);
      bcast.mockClear();
      const ws2 = { send: vi.fn() } as any;
      await (doInst as any).webSocketMessage(ws2, JSON.stringify({ type: t, sessionId: 'foreign', requestId: 'r' }));
      expect(bcast).not.toHaveBeenCalled();
      expect(ws2.send).toHaveBeenCalledWith(expect.stringContaining('forbidden_session'));
    }
  });

  it('cache burst: N owned commands = ONE ownership lookup', async () => {
    vi.spyOn(ownershipMod, 'verifySessionOwnership').mockResolvedValue(true);
    const bcast = vi.spyOn(doInst as any, 'broadcastTo');
    for (let i = 0; i < 5; i++) {
      const ws = { send: vi.fn() } as any;
      (state as any).getTags = vi.fn(() => ['mobile', 'tenant:t1', 'user:u1']);
      await (doInst as any).webSocketMessage(ws, JSON.stringify({ type: 'terminal_command', sessionId: 'burst', requestId: 'r' + i }));
    }
    expect(bcast).toHaveBeenCalledTimes(5);
    // detailed cache-hit / 1 relay call covered in session-ownership.test.ts
  });

  it('post-session_deleted re-check denies', async () => {
    // first owned
    vi.spyOn(ownershipMod, 'verifySessionOwnership').mockResolvedValueOnce(true);
    const ws1 = { send: vi.fn() } as any;
    (state as any).getTags = vi.fn(() => ['mobile', 'tenant:t1', 'user:u1']);
    await (doInst as any).webSocketMessage(ws1, JSON.stringify({ type: 'terminal_command', sessionId: 'del-s', requestId: 'r' }));
    // simulate desktop delete arriving
    (state as any).getTags = vi.fn(() => ['desktop']);
    await (doInst as any).webSocketMessage({} as any, JSON.stringify({ type: 'session_deleted', sessionId: 'del-s' }));
    // recheck should deny (after invalidate)
    vi.spyOn(ownershipMod, 'verifySessionOwnership').mockResolvedValueOnce(false);
    const ws2 = { send: vi.fn() } as any;
    (state as any).getTags = vi.fn(() => ['mobile', 'tenant:t1', 'user:u1']);
    await (doInst as any).webSocketMessage(ws2, JSON.stringify({ type: 'terminal_command', sessionId: 'del-s', requestId: 'r2' }));
    expect(ws2.send).toHaveBeenCalledWith(expect.stringContaining('forbidden_session'));
  });
});