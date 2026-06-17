#!/usr/bin/env node
// Verifies the mobile terminal word-wraps (no mid-word breaks) for a Grok session.
// Emits one long colored logical line of distinct tokens; each token must appear
// intact on some rendered row (a char-wrap would split tokens across rows).
const fs = require('node:fs');
const http = require('node:http');
const path = require('node:path');
const puppeteer = require('puppeteer');

const ROOT = path.resolve(__dirname, '..');
const DIST = path.join(ROOT, 'dist', 'web', 'browser');
const PORT = Number(process.env.JOHNNYONE_PROBE_PORT || 4328);
const TENANT_ID = '00000000-0000-0000-0000-000000000001';
const delay = (ms) => new Promise((r) => setTimeout(r, ms));
const NTOK = 40;

function frame() {
  const words = Array.from({ length: 40 }, (_, i) => `tok${String(i).padStart(3, '0')}`);
  // One long colored logical line (exercises the Grok ANSI wrap path).
  return ['grok wrap probe', '\x1b[38;5;39m' + words.join(' ') + '\x1b[0m', '❯ '].join('\n');
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
    const user = { id: 'w-user', tenantId, email: 'admin@johnnyone.local', displayName: 'W', roles: ['admin'], status: 'active' };
    localStorage.setItem('johnnyone_worker_url', 'http://127.0.0.1:7714');
    localStorage.setItem('johnnyone_access_token', 'w-token');
    localStorage.setItem('johnnyone_refresh_token', 'w-refresh');
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
  let failed = false;
  try {
    const page = await browser.newPage();
    page.on('console', (m) => { if (m.type() === 'error') console.log('C.ERR>', m.text().slice(0, 160)); });
    page.on('pageerror', (e) => console.log('PAGEERR>', (e.message || '').slice(0, 160)));
    await page.setViewport({ width: 390, height: 844, isMobile: true, hasTouch: true, deviceScaleFactor: 2 });
    await page.evaluateOnNewDocument((src) => { window.__frame = new Function(`return (${src})();`); }, frame.toString());
    await primeAuthAndRelay(page);
    await mockGraphql(page, sessions);
    await page.goto(`http://127.0.0.1:${PORT}/terminal`, { waitUntil: 'networkidle2', timeout: 60000 });
    await page.waitForSelector('.terminal-pane johnny-terminal-screen .xterm-rows', { timeout: 30000 });
    await delay(2000);
    const allText = await page.evaluate(() => (document.querySelector('.terminal-pane johnny-terminal-screen .xterm-rows')?.textContent || '').slice(0, 120));
    console.log('rendered text head:', JSON.stringify(allText));

    const rows = await page.evaluate(() => Array.from(document.querySelectorAll('.terminal-pane johnny-terminal-screen .xterm-rows > div')).map((r) => (r.textContent || '').replace(/ /g, ' ').replace(/\s+$/,'')));
    const joined = rows.join('|');
    // A token is "split" if it does NOT appear intact anywhere (char-wrap would
    // break e.g. "tok012" into "tok01" + "2" across a row boundary).
    const split = [];
    for (let i = 0; i < NTOK; i++) {
      const tok = `tok${String(i).padStart(3, '0')}`;
      if (!rows.some((r) => r.includes(tok))) split.push(tok);
    }
    // How many rows the wrapped line used, and the max row text length (fill).
    const tokenRows = rows.filter((r) => /tok\d/.test(r));
    const maxLen = Math.max(0, ...tokenRows.map((r) => r.trim().length));
    console.log('wrapped rows:', tokenRows.length, 'maxRowLen:', maxLen);
    console.log('rows sample:', JSON.stringify(tokenRows.slice(0, 3)));
    if (split.length) { console.log(`❌ mid-word breaks: ${split.length} token(s) split across rows, e.g. ${split.slice(0, 6).join(', ')}`); failed = true; }
    else console.log('✅ word-wrap: every token stayed intact on a row (no mid-word breaks)');
    void joined;
    await page.close();
  } finally { await browser.close(); server.close(); }
  process.exitCode = failed ? 2 : 0;
}

main().catch((e) => { console.error(e); process.exit(1); });
