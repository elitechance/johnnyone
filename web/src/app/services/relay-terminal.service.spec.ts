import { describe, it, expect, vi, beforeEach, afterEach } from 'vitest';

// Ensure Angular JIT/compiler is available so the real @Injectable class with inject()
// fields can be imported without "PlatformLocation needs JIT" error. This allows
// a true value import of the real service (per T2/QA/Lead requirement) rather than
// type-only. We still use a prototype-backed instance for testability without full DI.
import '@angular/compiler';

// Import the REAL service (value, not type-only) and exercise its real methods.
import { RelayTerminalService } from './relay-terminal.service';

describe('RelayTerminalService Phase 02 (real service + faked WS capture)', () => {
  let OrigWebSocket: any;
  let capturedUrl: string;

  beforeEach(() => {
    capturedUrl = '';
    OrigWebSocket = (global as any).WebSocket;
    const FakeWS = function (this: any, url: string) {
      capturedUrl = url;
      this.url = url;
      this.readyState = 1;
      this.close = vi.fn();
      this.send = vi.fn();
      // fire onopen in microtask so ensure's promise resolves (assignment of handlers is post-new)
      queueMicrotask(() => {
        if (typeof this.onopen === 'function') {
          try { this.onopen({} as any); } catch {}
        }
      });
      return this;
    } as any;
    (global as any).WebSocket = FakeWS;
  });

  afterEach(() => {
    (global as any).WebSocket = OrigWebSocket;
    vi.useRealTimers();
  });

  function makeInstance() {
    const inst: any = Object.create(RelayTerminalService.prototype);
    inst.auth = {
      getAccessToken: vi.fn(() => 'access-abc123'),
      getRefreshToken: vi.fn(() => 'refresh-xyz'),
      refresh: vi.fn().mockResolvedValue(undefined),
      ensureFreshToken: vi.fn().mockResolvedValue(undefined),
      logout: vi.fn(),
      getTokenExpiresAt: vi.fn(() => null),
      getExpiresIn: vi.fn(() => 3600),
      isTokenNearExpiry: vi.fn(() => false),
    };
    inst.zone = { run: (fn: any) => fn() };
    inst['workerBaseUrl'] = () => 'http://localhost:8787';
    inst.visualSubscriptions = new Map();
    inst.streamSubscriptions = new Map();
    inst.pendingInput = new Map();
    inst.pendingTimers = new Map();
    inst.reconnectTimer = null;
    inst.socket = null;
    inst.terminalScreenCache = { remember: vi.fn(), get: vi.fn(), remove: vi.fn(), flush: vi.fn() };
    return inst;
  }

  it('imports the real RelayTerminalService', () => {
    expect(RelayTerminalService).toBeTruthy();
    expect(typeof RelayTerminalService).toBe('function');
  });

  it('real relayWsUrl() produces URL with token and no nodeId', () => {
    const inst = makeInstance();
    const url = (inst as any).relayWsUrl();
    expect(url).toMatch(/^wss?:\/\//);
    expect(url).toContain('clientType=mobile');
    expect(url).toContain('token=access-abc123');
    const u = new URL(url.replace(/^wss?:/, 'http:'));
    expect(u.searchParams.has('nodeId')).toBe(false);
    expect(u.searchParams.get('token')).toBe('access-abc123');
  });

  it('ensureConnected uses the REAL relayWsUrl output (faked WS captures token, no nodeId)', async () => {
    const inst = makeInstance();
    const ensureSpy = inst.auth.ensureFreshToken;
    await (inst as any).ensureConnected().catch(() => {});
    expect(capturedUrl).toBeTruthy();
    expect(capturedUrl).toContain('token=access-abc123');
    const u = new URL(capturedUrl.replace(/^wss?:/, 'http:'));
    expect(u.searchParams.has('nodeId')).toBe(false);
    expect(ensureSpy).toHaveBeenCalled();
  });

  it('display-only nodes: listDesktopNodes is not used for socket routing (relayWsUrl has no node)', () => {
    const inst = makeInstance();
    const url = (inst as any).relayWsUrl();
    // routing decision: server resolves; client never supplies node from list
    const u = new URL(url.replace(/^wss?:/, 'http:'));
    expect(u.searchParams.has('nodeId')).toBe(false);
    // no evidence of node id in constructed url from real method
  });

  it('expiry->refresh->reconnect carries NEW token on reconnect URL (real code path)', async () => {
    const inst = makeInstance();

    // First ensure: old token -> real url has old
    inst.auth.getAccessToken = vi.fn(() => 'access-abc123');
    await (inst as any).ensureConnected().catch(() => {});
    expect(capturedUrl).toContain('token=access-abc123');

    // Simulate the refresh + reconnect: reset socket so ensure proceeds, call refresh (real method stub), update token, call ensure again
    // (exercises real relayWsUrl after "refresh" with the new value)
    capturedUrl = '';
    (inst as any).socket = null;
    inst.auth.ensureFreshToken = vi.fn().mockImplementation(async () => {
      inst.auth.getAccessToken = vi.fn(() => 'access-NEW-999');
    });
    const ensureSpy = inst.auth.ensureFreshToken as any;
    await (inst as any).ensureConnected().catch(() => {});

    expect(capturedUrl).toContain('token=access-NEW-999');
    expect(ensureSpy).toHaveBeenCalled();
  });

  it('refresh-fail surfaces error (no silent dead, real ensure throws)', async () => {
    const inst = makeInstance();
    inst.auth.getAccessToken = vi.fn(() => null);
    inst.auth.getRefreshToken = vi.fn(() => 'r');
    inst.auth.ensureFreshToken = vi.fn().mockImplementation(async () => {
      inst.auth.logout();
      throw new Error('invalid refresh token');
    });

    let threw: any = null;
    try {
      await (inst as any).ensureConnected();
    } catch (e) {
      threw = e;
      console.error('refresh-fail-threw:', String(e));
    }
    expect(threw).toBeTruthy();
    expect(String(threw)).toMatch(/No valid authentication token|refresh|invalid refresh/i);
    expect(inst.auth.logout).toHaveBeenCalled();
  });

  it('auth-class ensureFreshToken reject: logout already called and WebSocket is not constructed', async () => {
    let constructCount = 0;
    (global as any).WebSocket = function (this: any, url: string) {
      constructCount += 1;
      this.url = url;
      this.readyState = 1;
      this.close = vi.fn();
      this.send = vi.fn();
      return this;
    };
    const inst = makeInstance();
    inst.auth.ensureFreshToken = vi.fn().mockImplementation(async () => {
      inst.auth.logout();
      throw new Error('invalid refresh token');
    });
    const before = constructCount;
    await expect((inst as any).ensureConnected()).rejects.toBeTruthy();
    expect(inst.auth.logout).toHaveBeenCalled();
    expect(constructCount).toBe(before);
  });

  it('transport-class ensureFreshToken reject does not call logout', async () => {
    const inst = makeInstance();
    inst.auth.ensureFreshToken = vi.fn().mockRejectedValue(new TypeError('network'));
    await expect((inst as any).ensureConnected()).rejects.toBeTruthy();
    expect(inst.auth.logout).not.toHaveBeenCalled();
  });

  it('scheduleReconnectForVisualSubscriptions calls ensureFreshToken, not auth.refresh', async () => {
    vi.useFakeTimers();
    const inst = makeInstance();
    inst.visualSubscriptions.set('sess-1', 1);
    inst.lastDisconnectWasAuthRejection = true;
    inst.authFailureCount = 1;
    inst.auth.ensureFreshToken = vi.fn().mockResolvedValue(undefined);
    inst.auth.refresh = vi.fn().mockResolvedValue(undefined);
    (inst as any).scheduleReconnectForVisualSubscriptions();
    await vi.advanceTimersByTimeAsync(2_000);
    expect(inst.auth.ensureFreshToken).toHaveBeenCalled();
    expect(inst.auth.refresh).not.toHaveBeenCalled();
  });
});