#!/usr/bin/env node
// Measures whether the mobile terminal fills its host, and whether it recovers
// to full fill after a viewport height change (the on-screen-keyboard scenario).
const fs = require('node:fs');
const http = require('node:http');
const path = require('node:path');
const puppeteer = require('puppeteer');

const ROOT = path.resolve(__dirname, '..');
const DIST = path.join(ROOT, 'dist', 'web', 'browser');
const PORT = Number(process.env.JOHNNYONE_PROBE_PORT || 4327);
const TENANT_ID = '00000000-0000-0000-0000-000000000001';
const delay = (ms) => new Promise((r) => setTimeout(r, ms));

function frame() {
  return ['JohnnyOne height probe', ...Array.from({ length: 60 }, (_, i) => `line ${String(i + 1).padStart(3, '0')} output`), '❯ '].join('\n');
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
    const user = { id: 'h-user', tenantId, email: 'admin@johnnyone.local', displayName: 'H', roles: ['admin'], status: 'active' };
    localStorage.setItem('johnnyone_worker_url', 'http://127.0.0.1:7714');
    localStorage.setItem('johnnyone_access_token', 'h-token');
    localStorage.setItem('johnnyone_refresh_token', 'h-refresh');
    localStorage.setItem('johnnyone_tenant_id', tenantId);
    localStorage.setItem('johnnyone_user_id', user.id);
    localStorage.setItem('johnnyone_auth_user', JSON.stringify(user));
    class MockRelayWebSocket {
      static OPEN = 1; static CONNECTING = 0;
      constructor() { window.WebSocket._lastInstance = this; this.readyState = 0; setTimeout(() => { this.readyState = 1; this.onopen?.({}); }, 0); }
      send(raw) { let e; try { e = JSON.parse(raw); } catch { return; } const d = e.data || {};
        if (e.type === 'terminal_command' && (d.control === 'visual_subscribe' || d.control === 'visual_refresh' || d.control === 'visual_history') && d.sessionId) this.emit(d.sessionId, window.__frame(), 420); }
      emit(sessionId, content, historyLines) { this.onmessage?.({ data: JSON.stringify({ type: 'terminal_screen', data: { sessionId, tmuxSessionName: 'jo', paneId: '%0', status: 'attached', cursor: Date.now(), content, cursorX: 2, cursorY: 22, historyLines, cols: 80, rows: 24 } }) }); }
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
    else if (body.includes('listDetectedCliTools')) data = { listDetectedCliTools: [{ provider: 'shell', command: 'bash', found: true, path: '/bin/bash' }] };
    else if (body.includes('getSetting')) data = { getSetting: '~/x' };
    else if (body.includes('listDesktopNodes')) data = { listDesktopNodes: [{ id: 'n', hostname: 'h', os: 'linux', arch: 'x64', version: 'p', status: 'online', capabilities: [], lastHeartbeatAt: new Date().toISOString(), createdAt: new Date().toISOString() }] };
    req.respond({ status: 200, contentType: 'application/json', headers: { 'Access-Control-Allow-Origin': '*' }, body: JSON.stringify({ data }) });
  });
}

async function measure(page) {
  return page.evaluate(() => {
    const q = (s) => document.querySelector('.terminal-pane johnny-terminal-screen ' + s);
    const host = q('.terminal-host');
    const screen = q('.xterm-screen');
    const hostH = host ? Math.round(host.getBoundingClientRect().height) : -1;
    const screenH = screen ? Math.round(screen.getBoundingClientRect().height) : -1;
    return { hostH, screenH, fill: hostH > 0 ? +(screenH / hostH).toFixed(2) : 0 };
  });
}

async function main() {
  if (!fs.existsSync(DIST)) { console.error(`Missing build at ${DIST}. Run: npx nx build web`); process.exit(1); }
  const sessions = [{ id: 'aaaa1111-0000-0000-0000-000000000001', title: 'Shell', provider: 'shell', model: '', workingDirectory: '/x', status: 'active', totalInputTokens: 0, totalOutputTokens: 0, totalCostCents: 0, createdAt: new Date().toISOString(), updatedAt: new Date().toISOString() }];
  const server = await startStaticServer();
  const browser = await puppeteer.launch({ headless: true, args: ['--no-sandbox', '--disable-setuid-sandbox'] });
  let failed = false;
  try {
    const page = await browser.newPage();
    await page.setViewport({ width: 390, height: 844, isMobile: true, hasTouch: true, deviceScaleFactor: 2 });
    await page.evaluateOnNewDocument((src) => { window.__frame = new Function(`return (${src})();`); }, frame.toString());
    await primeAuthAndRelay(page);
    await mockGraphql(page, sessions);
    await page.goto(`http://127.0.0.1:${PORT}/terminal`, { waitUntil: 'networkidle2', timeout: 60000 });
    await page.waitForSelector('.terminal-pane johnny-terminal-screen .xterm-screen', { timeout: 30000 });
    await delay(1200);

    const initial = await measure(page);
    console.log('initial:', JSON.stringify(initial));

    // Simulate the on-screen keyboard: shrink viewport height, then restore.
    await page.setViewport({ width: 390, height: 520, isMobile: true, hasTouch: true, deviceScaleFactor: 2 });
    await delay(600);
    const shrunk = await measure(page);
    console.log('keyboard-up (520h):', JSON.stringify(shrunk));

    await page.setViewport({ width: 390, height: 844, isMobile: true, hasTouch: true, deviceScaleFactor: 2 });
    await delay(600);
    const restored = await measure(page);
    console.log('keyboard-down (844h):', JSON.stringify(restored));

    // The terminal content should fill most of its host in every state (>= 0.85).
    for (const [name, m] of [['initial', initial], ['shrunk', shrunk], ['restored', restored]]) {
      if (m.fill < 0.85) { console.log(`❌ ${name}: terminal fills only ${Math.round(m.fill*100)}% of host (host ${m.hostH}px, content ${m.screenH}px)`); failed = true; }
    }
    if (!failed) console.log('✅ terminal fills its host (>=85%) in all states incl. after viewport resize');
    await page.close();
  } finally { await browser.close(); server.close(); }
  process.exitCode = failed ? 2 : 0;
}

main().catch((e) => { console.error(e); process.exit(1); });
