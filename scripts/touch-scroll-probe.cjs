#!/usr/bin/env node
// Touch-scroll probe: loads the web app at a mobile viewport with a tall
// terminal capture, then simulates a real finger drag (CDP touch events) on the
// terminal and reports whether the rendered output actually scrolled.
const fs = require('node:fs');
const http = require('node:http');
const path = require('node:path');
const puppeteer = require('puppeteer');

const ROOT = path.resolve(__dirname, '..');
const DIST = path.join(ROOT, 'dist', 'web', 'browser');
const PORT = Number(process.env.JOHNNYONE_PROBE_PORT || 4324);
const TENANT_ID = '00000000-0000-0000-0000-000000000001';

const delay = (ms) => new Promise((r) => setTimeout(r, ms));

function tallFrame() {
  return [
    '\x1b[38;5;141mTouch scroll probe — tall capture\x1b[0m',
    ...Array.from({ length: 80 }, (_, i) => `\x1b[38;5;245mline ${String(i + 1).padStart(3, '0')}\x1b[0m the quick brown fox jumps`),
    '❯ ',
  ].join('\n');
}

function startStaticServer() {
  const mime = { '.html': 'text/html', '.js': 'text/javascript', '.css': 'text/css', '.json': 'application/json', '.png': 'image/png', '.svg': 'image/svg+xml', '.woff2': 'font/woff2', '.ico': 'image/x-icon' };
  const server = http.createServer((req, res) => {
    const urlPath = decodeURIComponent((req.url || '/').split('?')[0]);
    const resolved = path.normalize(path.join(DIST, urlPath === '/' ? 'index.html' : urlPath));
    if (!resolved.startsWith(DIST)) { res.writeHead(403).end(); return; }
    fs.readFile(resolved, (err, data) => {
      if (err) { fs.readFile(path.join(DIST, 'index.html'), (e2, d2) => { if (e2) { res.writeHead(404).end(); return; } res.writeHead(200, { 'Content-Type': 'text/html' }); res.end(d2); }); return; }
      res.writeHead(200, { 'Content-Type': mime[path.extname(resolved)] || 'application/octet-stream' }); res.end(data);
    });
  });
  return new Promise((resolve, reject) => { server.listen(PORT, '127.0.0.1', () => resolve(server)); server.on('error', reject); });
}

async function primeAuthAndRelay(page) {
  await page.evaluateOnNewDocument((tenantId) => {
    const user = { id: 'touch-probe-user', tenantId, email: 'admin@johnnyone.local', displayName: 'Touch', roles: ['admin'], status: 'active' };
    localStorage.setItem('johnnyone_worker_url', 'http://127.0.0.1:7714');
    localStorage.setItem('johnnyone_access_token', 'touch-token');
    localStorage.setItem('johnnyone_refresh_token', 'touch-refresh');
    localStorage.setItem('johnnyone_tenant_id', tenantId);
    localStorage.setItem('johnnyone_user_id', user.id);
    localStorage.setItem('johnnyone_auth_user', JSON.stringify(user));
    class MockRelayWebSocket {
      static OPEN = 1; static CONNECTING = 0;
      constructor() { window.WebSocket._lastInstance = this; this.readyState = 0; setTimeout(() => { this.readyState = 1; this.onopen?.({}); }, 0); }
      send(raw) {
        let e; try { e = JSON.parse(raw); } catch { return; }
        const d = e.data || {};
        if (e.type === 'terminal_command' && (d.control === 'visual_subscribe' || d.control === 'visual_refresh' || d.control === 'visual_history') && d.sessionId) {
          this._sid = d.sessionId; this.emit(d.sessionId, window.__frame(), 420);
        }
      }
      emit(sessionId, content, historyLines) {
        this.onmessage?.({ data: JSON.stringify({ type: 'terminal_screen', data: { sessionId, tmuxSessionName: 'jo-touch', paneId: '%0', status: 'attached', cursor: Date.now(), content, cursorX: 2, cursorY: 22, historyLines, cols: 80, rows: 24 } }) });
      }
      close() {} addEventListener(t, h) { this[`on${t}`] = h; } removeEventListener(t) { this[`on${t}`] = null; }
    }
    window.WebSocket = MockRelayWebSocket;
  }, TENANT_ID);
}

async function mockGraphql(page, sessions) {
  await page.setRequestInterception(true);
  page.on('request', (req) => {
    if (!req.url().includes('/graphql')) { req.continue(); return; }
    if (req.method() === 'OPTIONS') { req.respond({ status: 204, headers: { 'Access-Control-Allow-Origin': '*', 'Access-Control-Allow-Headers': '*', 'Access-Control-Allow-Methods': 'POST, OPTIONS' } }); return; }
    const body = req.postData() || ''; let data = {};
    if (body.includes('listAiSessions')) data = { listAiSessions: sessions };
    else if (body.includes('getAiSession')) data = { getAiSession: sessions[0] };
    else if (body.includes('listAiMessages')) data = { listAiMessages: [] };
    else if (body.includes('listDetectedCliTools')) data = { listDetectedCliTools: [{ provider: 'grok', command: 'grok', found: true, path: '/usr/bin/grok' }] };
    else if (body.includes('getSetting')) data = { getSetting: '~/x' };
    else if (body.includes('listDesktopNodes')) data = { listDesktopNodes: [{ id: 'n', hostname: 'h', os: 'linux', arch: 'x64', version: 'p', status: 'online', capabilities: [], lastHeartbeatAt: new Date().toISOString(), createdAt: new Date().toISOString() }] };
    req.respond({ status: 200, contentType: 'application/json', headers: { 'Access-Control-Allow-Origin': '*' }, body: JSON.stringify({ data }) });
  });
}

async function main() {
  if (!fs.existsSync(DIST)) { console.error(`Missing build at ${DIST}. Run: npx nx build web`); process.exit(1); }
  const sessions = [{ id: 'aaaa1111-0000-0000-0000-000000000001', title: 'Grok dev', provider: 'grok', model: '', workingDirectory: '/x', status: 'active', totalInputTokens: 0, totalOutputTokens: 0, totalCostCents: 0, createdAt: new Date().toISOString(), updatedAt: new Date().toISOString() }];
  const server = await startStaticServer();
  const browser = await puppeteer.launch({ headless: true, args: ['--no-sandbox', '--disable-setuid-sandbox'] });
  try {
    const page = await browser.newPage();
    await page.setViewport({ width: 390, height: 844, isMobile: true, hasTouch: true, deviceScaleFactor: 2 });
    await page.evaluateOnNewDocument((src) => { window.__frame = new Function(`return (${src})();`); }, tallFrame.toString());
    await primeAuthAndRelay(page);
    await mockGraphql(page, sessions);
    await page.goto(`http://127.0.0.1:${PORT}/terminal`, { waitUntil: 'networkidle2', timeout: 60000 });
    await page.waitForSelector('.terminal-pane johnny-terminal-screen .xterm-rows', { timeout: 30000 });
    await delay(1500);

    const before = await page.evaluate(() => {
      const q = (s) => document.querySelector('.terminal-pane johnny-terminal-screen ' + s);
      const vp = q('.xterm-viewport');
      const sa = q('.xterm-scroll-area');
      const screen = q('.xterm-screen');
      const cs = vp ? getComputedStyle(vp) : null;
      return {
        scrollTop: vp?.scrollTop ?? -1,
        scrollHeight: vp?.scrollHeight ?? -1,
        clientHeight: vp?.clientHeight ?? -1,
        vpRectH: vp ? Math.round(vp.getBoundingClientRect().height) : -1,
        vpOverflowY: cs?.overflowY,
        vpHeightStyle: cs?.height,
        scrollAreaH: sa ? Math.round(sa.getBoundingClientRect().height) : -1,
        screenH: screen ? Math.round(screen.getBoundingClientRect().height) : -1,
        topText: (q('.xterm-rows')?.textContent || '').slice(0, 40),
      };
    });

    // Find the terminal host center for the touch drag.
    const box = await page.evaluate(() => {
      const el = document.querySelector('.terminal-pane johnny-terminal-screen .terminal-host');
      const r = el.getBoundingClientRect();
      return { x: r.x + r.width / 2, y: r.y + r.height / 2, top: r.y, h: r.height };
    });

    const cdp = await page.createCDPSession();
    // Drag the finger DOWN: direct-manipulation should pull older output into
    // view (top line number decreases).
    const startY = Math.round(box.top + box.h * 0.2);
    const endY = Math.round(box.top + box.h * 0.8);
    const tp = (y) => [{ x: Math.round(box.x), y }];
    await cdp.send('Input.dispatchTouchEvent', { type: 'touchStart', touchPoints: tp(startY) });
    for (let i = 1; i <= 8; i++) {
      const y = Math.round(startY + ((endY - startY) * i) / 8);
      await cdp.send('Input.dispatchTouchEvent', { type: 'touchMove', touchPoints: tp(y) });
      await delay(20);
    }
    await cdp.send('Input.dispatchTouchEvent', { type: 'touchEnd', touchPoints: [] });
    await delay(700);

    const after = await page.evaluate(() => {
      const vp = document.querySelector('.terminal-pane johnny-terminal-screen .xterm-viewport');
      return { scrollTop: vp?.scrollTop ?? -1, topText: (document.querySelector('.terminal-pane johnny-terminal-screen .xterm-rows')?.textContent || '').slice(0, 40) };
    });

    console.log('BEFORE', JSON.stringify(before));
    console.log('AFTER ', JSON.stringify(after));
    const scrolled = Math.abs(after.scrollTop - before.scrollTop) > 4 || after.topText !== before.topText;
    console.log(scrolled ? '\n✅ touch drag SCROLLED the terminal' : '\n❌ touch drag did NOT scroll (scrollTop unchanged, same top line)');
    await page.close();
  } finally { await browser.close(); server.close(); }
}

main().catch((e) => { console.error(e); process.exit(1); });
