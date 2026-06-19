import { describe, it, expect, vi, beforeEach } from 'vitest';
import { ChatRelayDO } from './chat-relay-do';
import * as ownershipMod from './session-ownership';
import { getSocketAuth } from '../auth/socket-auth-context';

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

describe('ChatRelayDO gate (client→desktop forward path, Task 02)', () => {
  let state: FakeState;
  let env: any;
  let doInst: ChatRelayDO;

  beforeEach(() => {
    vi.restoreAllMocks();
    state = makeFakeState();
    env = makeEnv();
    doInst = new ChatRelayDO(state as any, env);
  });

  async function sendMessage(envelope: any, isDesktop = false) {
    const sent: any[] = [];
    const ws = {
      send: (m: string) => { sent.push(JSON.parse(m)); }
    } as any;
    (state as any).getTags = vi.fn(() => isDesktop ? ['desktop'] : ['mobile', 'tenant:t1', 'user:u1']);
    await (doInst as any).webSocketMessage(ws, JSON.stringify(envelope));
    return sent;
  }

  it('owned-passes calls broadcastTo for gated types', async () => {
    vi.spyOn(ownershipMod, 'verifySessionOwnership').mockResolvedValue(true as any);
    const bcast = vi.spyOn(doInst as any, 'broadcastTo');

    const gated = ['chat_request', 'terminal_command', 'terminal_visual_subscribe', 'terminal_visual_unsubscribe', 'terminal_resize', 'terminal_kill'];
    for (const t of gated) {
      bcast.mockClear();
      await sendMessage({ type: t, sessionId: 's-owned', requestId: 'r' });
      expect(bcast).toHaveBeenCalledWith('desktop', expect.objectContaining({ type: t }));
    }
  });

  it('foreign-rejected sends reject to origin only, no broadcast', async () => {
    vi.spyOn(ownershipMod, 'verifySessionOwnership').mockResolvedValue(false);
    const bcast = vi.spyOn(doInst as any, 'broadcastTo');
    const sent = await sendMessage({ type: 'terminal_command', sessionId: 'foreign', requestId: 'r' });
    expect(bcast).not.toHaveBeenCalled();
    expect(sent.length).toBe(1);
    expect(sent[0]).toEqual({ type: 'terminal_command_ack', error: 'forbidden_session', requestId: 'r', sessionId: 'foreign' });
  });

  it('no sessionId is denied', async () => {
    vi.spyOn(ownershipMod, 'verifySessionOwnership').mockResolvedValue(false);
    const bcast = vi.spyOn(doInst as any, 'broadcastTo');
    await sendMessage({ type: 'terminal_command', requestId: 'r' });
    expect(bcast).not.toHaveBeenCalled();
  });

  it('transport error fail-closed sends reject', async () => {
    vi.spyOn(ownershipMod, 'verifySessionOwnership').mockRejectedValue(new Error('Desktop RPC timed out'));
    const bcast = vi.spyOn(doInst as any, 'broadcastTo');
    const sent = await sendMessage({ type: 'terminal_command', sessionId: 's', requestId: 'r' });
    expect(bcast).not.toHaveBeenCalled();
    expect(sent[0].error).toBe('forbidden_session');
  });

  it('desktop->client path unaffected: no ownership call, broadcasts', async () => {
    const ownSpy = vi.spyOn(ownershipMod, 'verifySessionOwnership');
    const bcast = vi.spyOn(doInst as any, 'broadcastTo');
    const sent = await sendMessage({ type: 'terminal_screen', sessionId: 's', data: {} }, true /*desktop*/);
    expect(ownSpy).not.toHaveBeenCalled();
    expect(bcast).toHaveBeenCalledWith('mobile', expect.anything());
    expect(bcast).toHaveBeenCalledWith('api', expect.anything());
    expect(sent.length).toBe(0); // no reject on desktop path
  });

  it('forbidden_session reject is byte-identical for foreign vs unknown (modulo sid)', async () => {
    vi.spyOn(ownershipMod, 'verifySessionOwnership').mockResolvedValue(false);
    const sent1 = await sendMessage({ type: 'terminal_command', sessionId: 'existing-foreign', requestId: 'r1' });
    const sent2 = await sendMessage({ type: 'terminal_command', sessionId: 'unknown-xyz', requestId: 'r2' });
    const e1 = { ...sent1[0] }; delete e1.sessionId; delete e1.requestId;
    const e2 = { ...sent2[0] }; delete e2.sessionId; delete e2.requestId;
    expect(e1).toEqual(e2);
    expect(sent1[0].error).toBe('forbidden_session');
  });

  // WSS jk_/apiKey terminal scope guard tests (Task 06)
  // apiKey terminal scope tests using attachment mock to feed getSocketAuth
  it('apiKey with terminal:read allows subscribe but denies command with FORBIDDEN_SCOPE', async () => {
    vi.restoreAllMocks();
    vi.spyOn(ownershipMod, 'verifySessionOwnership').mockResolvedValue(true as any);
    const bcast = vi.spyOn(doInst as any, 'broadcastTo');

    const wsCmd = { send: vi.fn() } as any;
    (wsCmd as any).deserializeAttachment = vi.fn(() => ({
      tenantId: 't1', userId: 'u1', clientType: 'api', apiKey: { id: 'k', scopes: ['terminal:read'] }
    }));
    (state as any).getTags = vi.fn(() => ['api', 'tenant:t1', 'user:u1']);

    await (doInst as any).webSocketMessage(wsCmd, JSON.stringify({ type: 'terminal_command', sessionId: 's', requestId: 'rcmd' }));
    const calls = (wsCmd.send as any).mock.calls.map((c: any[]) => JSON.parse(c[0]));
    expect(calls.some((m: any) => m.error === 'FORBIDDEN_SCOPE' && m.scope === 'terminal:write')).toBe(true);

    const wsSub = { send: vi.fn() } as any;
    (wsSub as any).deserializeAttachment = vi.fn(() => ({
      tenantId: 't1', userId: 'u1', clientType: 'api', apiKey: { id: 'k', scopes: ['terminal:read'] }
    }));
    await (doInst as any).webSocketMessage(wsSub, JSON.stringify({ type: 'terminal_visual_subscribe', sessionId: 's', requestId: 'rsub' }));
    // no forbid sent for read (scope passed)
    const subCalls = (wsSub.send as any).mock.calls.map((c: any[]) => JSON.parse(c[0]));
    expect(subCalls.some((m: any) => m.error === 'FORBIDDEN_SCOPE')).toBe(false);
  });

  it('JWT sockets (no apiKey in authCtx) are unrestricted on terminal ops', async () => {
    vi.spyOn(ownershipMod, 'verifySessionOwnership').mockResolvedValue(true as any);
    const bcast = vi.spyOn(doInst as any, 'broadcastTo');
    const sent: any[] = [];
    const ws = { send: (m: string) => sent.push(JSON.parse(m)) } as any;
    (ws as any).deserializeAttachment = vi.fn(() => ({ tenantId: 't1', userId: 'u1', clientType: 'api' })); // no apiKey
    (state as any).getTags = vi.fn(() => ['api', 'tenant:t1', 'user:u1']);
    await (doInst as any).webSocketMessage(ws, JSON.stringify({ type: 'terminal_command', sessionId: 's', requestId: 'r' }));
    const hadForbid = sent.some((m: any) => m.error === 'FORBIDDEN_SCOPE');
    expect(hadForbid).toBe(false);
    expect(bcast).toHaveBeenCalled();
  });
});
