#!/usr/bin/env node
// Temporal blink probe: loads the built web app at a mobile viewport, simulates
// an active TUI that republishes a full (changing) capture every 2s like the
// desktop backend (MIN_TERMINAL_SCREEN_PUBLISH_MS), and samples the rendered
// xterm row text every ~16ms. A "blink" is any frame whose visible character
// count collapses far below the steady-state baseline between two republishes.
const fs = require('node:fs');
const http = require('node:http');
const path = require('node:path');
const puppeteer = require('puppeteer');

const ROOT = path.resolve(__dirname, '..');
const DIST = path.join(ROOT, 'dist', 'web', 'browser');
const PORT = Number(process.env.JOHNNYONE_PROBE_PORT || 4322);
const TENANT_ID = '00000000-0000-0000-0000-000000000001';

function delay(ms) {
  return new Promise((resolve) => setTimeout(resolve, ms));
}

function frame(tick) {
  // Production-like Grok/agent capture: tall (>28 lines, like the 120-line
  // scrollback the desktop now sends) and the body GROWS a line each tick —
  // exactly an agent streaming a response. The marker proves which tick is shown.
  const spin = ['⋅', '∘', '○', '●'][tick % 4];
  const grown = Array.from({ length: 40 + tick }, (_, i) => (
    `\x1b[38;5;245mline ${String(i + 1).padStart(3, '0')}\x1b[0m output token stream chunk`
  ));
  return [
    '\x1b[38;5;141mJohnnyOne live-update probe — agent streaming\x1b[0m',
    '\x1b[38;5;240m────────────────────────────────────────────\x1b[0m',
    ...grown,
    `\x1b[38;5;39mMARKER tick=${tick}\x1b[0m`,
    `\x1b[38;5;245m${spin} running — ${tick * 2}s elapsed\x1b[0m`,
    '❯ ',
  ].join('\n');
}

function startStaticServer() {
  const mime = {
    '.html': 'text/html', '.js': 'text/javascript', '.css': 'text/css',
    '.json': 'application/json', '.png': 'image/png', '.svg': 'image/svg+xml',
    '.woff2': 'font/woff2', '.ico': 'image/x-icon',
  };
  const server = http.createServer((req, res) => {
    const urlPath = decodeURIComponent((req.url || '/').split('?')[0]);
    const filePath = path.join(DIST, urlPath === '/' ? 'index.html' : urlPath);
    const resolved = path.normalize(filePath);
    if (!resolved.startsWith(DIST)) { res.writeHead(403).end(); return; }
    fs.readFile(resolved, (err, data) => {
      if (err) {
        fs.readFile(path.join(DIST, 'index.html'), (e2, d2) => {
          if (e2) { res.writeHead(404).end('Not found'); return; }
          res.writeHead(200, { 'Content-Type': 'text/html' }); res.end(d2);
        });
        return;
      }
      res.writeHead(200, { 'Content-Type': mime[path.extname(resolved)] || 'application/octet-stream' });
      res.end(data);
    });
  });
  return new Promise((resolve, reject) => {
    server.listen(PORT, '127.0.0.1', () => resolve(server));
    server.on('error', reject);
  });
}

async function primeAuthAndRelay(page) {
  await page.evaluateOnNewDocument((tenantId) => {
    const user = {
      id: 'blink-probe-user', tenantId, email: 'admin@johnnyone.local',
      displayName: 'Blink Probe', roles: ['admin'], status: 'active',
    };
    localStorage.setItem('johnnyone_worker_url', 'http://127.0.0.1:7714');
    localStorage.setItem('johnnyone_access_token', 'blink-probe-token');
    localStorage.setItem('johnnyone_refresh_token', 'blink-probe-refresh');
    localStorage.setItem('johnnyone_tenant_id', tenantId);
    localStorage.setItem('johnnyone_user_id', user.id);
    localStorage.setItem('johnnyone_auth_user', JSON.stringify(user));

    class MockRelayWebSocket {
      static OPEN = 1; static CONNECTING = 0;
      constructor() {
        window.WebSocket._lastInstance = this;
        this.readyState = MockRelayWebSocket.CONNECTING;
        this._tick = 0;
        setTimeout(() => { this.readyState = MockRelayWebSocket.OPEN; this.onopen?.({}); }, 0);
      }
      send(raw) {
        let envelope; try { envelope = JSON.parse(raw); } catch { return; }
        const data = envelope.data || {};
        if (envelope.type === 'terminal_command' && data.control === 'visual_subscribe' && data.sessionId) {
          this._sessionId = data.sessionId;
          this.emit(this._sessionId, window.__probeFrame(0), 420);
        }
        if (envelope.type === 'terminal_command' && data.control === 'visual_refresh' && data.sessionId) {
          this.emit(data.sessionId, window.__probeFrame(this._tick), 420);
        }
        if (envelope.type === 'terminal_command' && data.control === 'visual_history' && data.sessionId) {
          this.emit(data.sessionId, window.__probeFrame(this._tick), 420);
        }
      }
      emit(sessionId, content, historyLines) {
        this.onmessage?.({ data: JSON.stringify({
          type: 'terminal_screen',
          data: {
            sessionId, tmuxSessionName: 'jo-blink', paneId: '%0', status: 'attached',
            cursor: Date.now(), content, cursorX: 2, cursorY: 22,
            historyLines, cols: 80, rows: 24,
          },
        })});
      }
      republish() {
        if (!this._sessionId) return;
        this._tick += 1;
        this.emit(this._sessionId, window.__probeFrame(this._tick), 420);
      }
      close() {}
      addEventListener(t, h) { this[`on${t}`] = h; }
      removeEventListener(t) { this[`on${t}`] = null; }
    }
    window.WebSocket = MockRelayWebSocket;
  }, TENANT_ID);
}

async function mockGraphql(page, sessions) {
  await page.setRequestInterception(true);
  page.on('request', (request) => {
    if (!request.url().includes('/graphql')) { request.continue(); return; }
    if (request.method() === 'OPTIONS') {
      request.respond({ status: 204, headers: {
        'Access-Control-Allow-Origin': '*', 'Access-Control-Allow-Headers': '*',
        'Access-Control-Allow-Methods': 'POST, OPTIONS',
      }});
      return;
    }
    const body = request.postData() || '';
    let data = {};
    if (body.includes('listAiSessions')) data = { listAiSessions: sessions };
    else if (body.includes('getAiSession')) data = { getAiSession: sessions[0] };
    else if (body.includes('listAiMessages')) data = { listAiMessages: [] };
    else if (body.includes('listDetectedCliTools')) data = { listDetectedCliTools: [
      { provider: 'grok', command: 'grok', found: true, path: '/usr/bin/grok' },
    ]};
    else if (body.includes('getSetting')) data = { getSetting: '~/projects/demo' };
    else if (body.includes('listDesktopNodes')) data = { listDesktopNodes: [{
      id: 'mock-node', hostname: 'mock', os: 'linux', arch: 'x64', version: 'probe',
      status: 'online', capabilities: [], lastHeartbeatAt: new Date().toISOString(),
      createdAt: new Date().toISOString(),
    }]};
    request.respond({ status: 200, contentType: 'application/json',
      headers: { 'Access-Control-Allow-Origin': '*' }, body: JSON.stringify({ data }) });
  });
}

async function main() {
  if (!fs.existsSync(DIST)) {
    console.error(`Missing build at ${DIST}. Run: npx nx build web`);
    process.exit(1);
  }
  const sessions = [{
    id: 'aaaa1111-0000-0000-0000-000000000001', title: 'Grok dev', provider: 'grok',
    model: '', workingDirectory: '/home/creepy/Documents/Workspace', status: 'active',
    totalInputTokens: 0, totalOutputTokens: 0, totalCostCents: 0,
    createdAt: new Date().toISOString(), updatedAt: new Date().toISOString(),
  }];

  const server = await startStaticServer();
  const browser = await puppeteer.launch({ headless: true, args: ['--no-sandbox', '--disable-setuid-sandbox'] });
  try {
    const page = await browser.newPage();
    page.on('pageerror', (e) => console.log('PAGEERROR:', e.message));
    page.on('console', (m) => { if (m.type() === 'error') console.log('CONSOLE.ERR:', m.text()); });
    await page.setViewport({ width: 390, height: 844, isMobile: true, hasTouch: true, deviceScaleFactor: 2 });
    await page.evaluateOnNewDocument((frameSrc) => {
      // eslint-disable-next-line no-new-func
      window.__probeFrame = new Function('tick', `return (${frameSrc})(tick);`);
    }, frame.toString());
    await primeAuthAndRelay(page);
    await mockGraphql(page, sessions);

    await page.goto(`http://127.0.0.1:${PORT}/terminal`, { waitUntil: 'networkidle2', timeout: 60000 });
    await page.waitForSelector('.terminal-pane johnny-terminal-screen .xterm-rows', { timeout: 30000 });
    await page.waitForFunction(() => /MARKER tick=/i.test(
      document.querySelector('.terminal-pane johnny-terminal-screen .xterm-rows')?.textContent || ''
    ), { timeout: 15000 });
    await delay(800);

    // Record rendered row text length on EVERY DOM mutation (catches sub-frame
    // blank states a fixed-interval poll would alias over), plus a 16ms poll.
    await page.evaluate(() => {
      window.__samples = [];
      const rows = document.querySelector('.terminal-pane johnny-terminal-screen .xterm-rows');
      const sample = (src) => {
        const text = (rows?.textContent || '').replace(/\s+/g, '');
        window.__samples.push({ t: performance.now(), len: text.length, src });
      };
      window.__mo = new MutationObserver(() => sample('mutation'));
      if (rows) window.__mo.observe(rows, { childList: true, subtree: true, characterData: true });
      window.__sampler = setInterval(() => sample('poll'), 16);
      sample('init');
    });

    // Drive 4 republishes at the real 2s cadence (active TUI changing each tick).
    for (let i = 0; i < 4; i++) {
      await delay(2000);
      await page.evaluate(() => window.WebSocket?._lastInstance?.republish());
    }
    await delay(800);

    const result = await page.evaluate(() => {
      clearInterval(window.__sampler);
      window.__mo?.disconnect();
      const s = window.__samples;
      const lens = s.map((x) => x.len);
      const sorted = [...lens].sort((a, b) => a - b);
      const median = sorted[Math.floor(sorted.length / 2)] || 0;
      const baseline = median;
      // Blink = a frame collapsing below 50% of baseline while not at the very start.
      const blinkThreshold = baseline * 0.5;
      const blinkFrames = s.filter((x, i) => i > 3 && x.len < blinkThreshold);
      return {
        sampleCount: s.length,
        mutationSamples: s.filter((x) => x.src === 'mutation').length,
        baselineLen: baseline,
        minLen: Math.min(...lens),
        maxLen: Math.max(...lens),
        blinkThreshold,
        blinkFrameCount: blinkFrames.length,
        blinkSamples: blinkFrames.slice(0, 12).map((x) => ({ t: Math.round(x.t), len: x.len, src: x.src })),
      };
    });

    console.log('blink probe result:', JSON.stringify(result, null, 2));
    if (result.blinkFrameCount > 0) {
      console.log(`\n❌ BLINK DETECTED: ${result.blinkFrameCount} frame(s) collapsed below ${Math.round(result.blinkThreshold)} chars (baseline ${result.baselineLen}).`);
      process.exitCode = 2;
    } else {
      console.log(`\n✅ No blink: min visible chars ${result.minLen} stayed at/above ${Math.round(result.blinkThreshold)} (baseline ${result.baselineLen}).`);
    }
    await page.close();
  } finally {
    await browser.close();
    server.close();
  }
}

main().catch((err) => { console.error(err); process.exit(1); });
