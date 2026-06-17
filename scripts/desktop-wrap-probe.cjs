#!/usr/bin/env node
// Desktop (non-mobile) Grok terminal: measure whether the wrap column accounts
// for the vertical scrollbar. Reports cols, viewport inner/outer width, the real
// scrollbar width, cell width, and the widest line that renders on a single row.
const fs = require('node:fs');
const http = require('node:http');
const path = require('node:path');
const puppeteer = require('puppeteer');

const ROOT = path.resolve(__dirname, '..');
const DIST = path.join(ROOT, 'dist', 'web', 'browser');
const PORT = Number(process.env.JOHNNYONE_PROBE_PORT || 4329);
const TENANT_ID = '00000000-0000-0000-0000-000000000001';
const delay = (ms) => new Promise((r) => setTimeout(r, ms));

// A frame with enough lines to force a vertical scrollbar, plus one very long
// hash line we can use to read the actual per-row fill, colored like Grok.
function frame() {
  const filler = Array.from({ length: 80 }, (_, i) => `line ${i}`);
  const longLine = '\x1b[38;5;39m' + '#'.repeat(400) + '\x1b[0m';
  return [longLine, ...filler, '❯ '].join('\n');
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
    const user = { id: 'd-user', tenantId, email: 'admin@johnnyone.local', displayName: 'D', roles: ['admin'], status: 'active' };
    localStorage.setItem('johnnyone_worker_url', 'http://127.0.0.1:7714');
    localStorage.setItem('johnnyone_access_token', 'd-token');
    localStorage.setItem('johnnyone_refresh_token', 'd-refresh');
    localStorage.setItem('johnnyone_tenant_id', tenantId);
    localStorage.setItem('johnnyone_user_id', user.id);
    localStorage.setItem('johnnyone_auth_user', JSON.stringify(user));
    class MockRelayWebSocket {
      static OPEN = 1; static CONNECTING = 0;
      constructor() { window.WebSocket._lastInstance = this; this.readyState = 0; setTimeout(() => { this.readyState = 1; this.onopen?.({}); }, 0); }
      send(raw) { let e; try { e = JSON.parse(raw); } catch { return; } const d = e.data || {};
        if (e.type === 'terminal_command' && (d.control === 'visual_subscribe' || d.control === 'visual_refresh' || d.control === 'visual_history') && d.sessionId) this.emit(d.sessionId, window.__frame(), 420); }
      emit(sessionId, content, historyLines) { this.onmessage?.({ data: JSON.stringify({ type: 'terminal_screen', data: { sessionId, tmuxSessionName: 'jo', paneId: '%0', status: 'attached', cursor: Date.now(), content, cursorX: 2, cursorY: 22, historyLines, cols: 200, rows: 50 } }) }); }
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
    page.on('console', (m) => { if (m.type() === 'error') console.log('C.ERR>', m.text().slice(0, 160)); });
    page.on('pageerror', (e) => console.log('PAGEERR>', (e.message || '').slice(0, 160)));
    // Desktop viewport, NOT mobile.
    await page.setViewport({ width: 1280, height: 900, isMobile: false, hasTouch: false, deviceScaleFactor: 1 });
    await page.evaluateOnNewDocument((src) => { window.__frame = new Function(`return (${src})();`); }, frame.toString());
    await primeAuthAndRelay(page);
    await mockGraphql(page, sessions);
    await page.goto(`http://127.0.0.1:${PORT}/terminal`, { waitUntil: 'networkidle2', timeout: 60000 });
    await page.waitForSelector('.terminal-pane johnny-terminal-screen .xterm-rows', { timeout: 30000 });
    await delay(2500);

    const m = await page.evaluate(() => {
      const root = document.querySelector('.terminal-pane johnny-terminal-screen');
      const xtermEl = root?.querySelector('.xterm');
      const viewport = root?.querySelector('.xterm-viewport');
      const screen = root?.querySelector('.xterm-screen');
      const rowsC = root?.querySelector('.xterm-rows');
      const firstRow = rowsC?.querySelector('div');
      // cell width: difference between two adjacent cell spans, else screen/cols.
      let cellWidth = 0;
      if (firstRow && firstRow.children.length >= 2) {
        cellWidth = firstRow.children[1].getBoundingClientRect().left - firstRow.children[0].getBoundingClientRect().left;
      }
      // The long hash line is the first logical row; read how many '#' fit on row 0.
      const fillOnRow0 = (firstRow?.textContent || '').replace(/[^#]/g, '').length;
      const vpClient = viewport ? viewport.clientWidth : -1;   // inner (excludes scrollbar)
      const vpOffset = viewport ? viewport.offsetWidth : -1;   // outer (includes scrollbar)
      const scrollbar = vpOffset - vpClient;
      const screenW = screen ? screen.getBoundingClientRect().width : -1;
      const hasVScroll = viewport ? viewport.scrollHeight > viewport.clientHeight + 1 : false;
      const fitInClient = cellWidth > 0 ? Math.floor(vpClient / cellWidth) : -1;
      const fitInOffset = cellWidth > 0 ? Math.floor(vpOffset / cellWidth) : -1;
      return { vpClient, vpOffset, scrollbar, screenW, cellWidth: +cellWidth.toFixed(2), fillOnRow0, fitInClient, fitInOffset, hasVScroll };
    });

    console.log(JSON.stringify(m, null, 2));
    console.log('--- interpretation ---');
    console.log(`scrollbar width (real): ${m.scrollbar}px  | FitAddon reserves 14px`);
    console.log(`cols actually used (chars on row 0): ${m.fillOnRow0}`);
    console.log(`max cols that fit in INNER width (excl scrollbar): ${m.fitInClient}`);
    console.log(`screen width ${m.screenW}px vs viewport inner ${m.vpClient}px  (screen > inner => row overflows under scrollbar)`);
    if (m.screenW > m.vpClient + 1) console.log('❌ rows are WIDER than the inner viewport — last column(s) sit under the scrollbar (wrap/overflow risk).');
    else if (m.fillOnRow0 < m.fitInClient - 1) console.log(`⚠️  under-fill: ${m.fitInClient - m.fillOnRow0} columns of usable width are left empty.`);
    else console.log('✅ rows fit within the inner viewport and use the available width.');
    await page.close();
  } finally { await browser.close(); server.close(); }
}

main().catch((e) => { console.error(e); process.exit(1); });
