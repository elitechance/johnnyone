#!/usr/bin/env node
// Verifies desktop (non-mobile) copy: mouse-drag selects terminal text, and
// Ctrl/Cmd-C copies the xterm selection to the clipboard via the copy handler.
const fs = require('node:fs');
const http = require('node:http');
const path = require('node:path');
const puppeteer = require('puppeteer');

const ROOT = path.resolve(__dirname, '..');
const DIST = path.join(ROOT, 'dist', 'web', 'browser');
const PORT = Number(process.env.JOHNNYONE_PROBE_PORT || 4326);
const TENANT_ID = '00000000-0000-0000-0000-000000000001';
const delay = (ms) => new Promise((r) => setTimeout(r, ms));

function frame() {
  return [
    'DESKTOP COPY PROBE',
    ...Array.from({ length: 20 }, (_, i) => `row ${String(i + 1).padStart(2, '0')} COPYME-${i + 1} the quick brown fox`),
    '$ ',
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
    const user = { id: 'deskcopy-user', tenantId, email: 'admin@johnnyone.local', displayName: 'Desk', roles: ['admin'], status: 'active' };
    localStorage.setItem('johnnyone_worker_url', 'http://127.0.0.1:7714');
    localStorage.setItem('johnnyone_access_token', 'deskcopy-token');
    localStorage.setItem('johnnyone_refresh_token', 'deskcopy-refresh');
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
          this.emit(d.sessionId, window.__frame(), 420);
        }
      }
      emit(sessionId, content, historyLines) {
        this.onmessage?.({ data: JSON.stringify({ type: 'terminal_screen', data: { sessionId, tmuxSessionName: 'jo', paneId: '%0', status: 'attached', cursor: Date.now(), content, cursorX: 2, cursorY: 22, historyLines, cols: 80, rows: 24 } }) });
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
    else if (body.includes('listDetectedCliTools')) data = { listDetectedCliTools: [{ provider: 'shell', command: 'bash', found: true, path: '/bin/bash' }] };
    else if (body.includes('getSetting')) data = { getSetting: '~/x' };
    else if (body.includes('listDesktopNodes')) data = { listDesktopNodes: [{ id: 'n', hostname: 'h', os: 'linux', arch: 'x64', version: 'p', status: 'online', capabilities: [], lastHeartbeatAt: new Date().toISOString(), createdAt: new Date().toISOString() }] };
    req.respond({ status: 200, contentType: 'application/json', headers: { 'Access-Control-Allow-Origin': '*' }, body: JSON.stringify({ data }) });
  });
}

async function main() {
  if (!fs.existsSync(DIST)) { console.error(`Missing build at ${DIST}. Run: npx nx build web`); process.exit(1); }
  const sessions = [{ id: 'aaaa1111-0000-0000-0000-000000000001', title: 'Shell', provider: 'shell', model: '', workingDirectory: '/x', status: 'active', totalInputTokens: 0, totalOutputTokens: 0, totalCostCents: 0, createdAt: new Date().toISOString(), updatedAt: new Date().toISOString() }];
  const server = await startStaticServer();
  const browser = await puppeteer.launch({ headless: true, args: ['--no-sandbox', '--disable-setuid-sandbox'] });
  let failed = true;
  try {
    const ctx = browser.defaultBrowserContext();
    await ctx.overridePermissions(`http://127.0.0.1:${PORT}`, ['clipboard-read', 'clipboard-write']);
    const page = await browser.newPage();
    await page.setViewport({ width: 1280, height: 800, isMobile: false, hasTouch: false, deviceScaleFactor: 1 });
    await page.evaluateOnNewDocument((src) => { window.__frame = new Function(`return (${src})();`); }, frame.toString());
    await primeAuthAndRelay(page);
    await mockGraphql(page, sessions);
    await page.goto(`http://127.0.0.1:${PORT}/terminal`, { waitUntil: 'networkidle2', timeout: 60000 });
    await page.waitForSelector('.terminal-pane johnny-terminal-screen .xterm-rows', { timeout: 30000 });
    await page.waitForFunction(() => /COPYME-/.test(document.querySelector('.terminal-pane johnny-terminal-screen .xterm-rows')?.textContent || ''), { timeout: 15000 });
    await delay(600);
    await page.screenshot({ path: path.join(ROOT, 'e2e-artifacts', 'mobile-terminal-review', 'desktop-before-select.png') });
    const preText = await page.evaluate(() => {
      const rows = Array.from(document.querySelectorAll('.terminal-pane johnny-terminal-screen .xterm-rows > div'));
      return rows.slice(0, 10).map((r, i) => `${i}:${(r.textContent || '').trim().slice(0, 16)}`);
    });
    console.log('pre-select rows:', JSON.stringify(preText));
    const metrics = await page.evaluate(() => {
      const q = (s) => document.querySelector('.terminal-pane johnny-terminal-screen ' + s);
      const row = q('.xterm-rows > div');
      const xterm = q('.xterm');
      const rcs = row ? getComputedStyle(row) : null;
      const xcs = xterm ? getComputedStyle(xterm) : null;
      return {
        rowLineHeight: rcs?.lineHeight, rowFontSize: rcs?.fontSize, rowHeight: row ? row.getBoundingClientRect().height : -1,
        xtermLineHeight: xcs?.lineHeight, xtermFontSize: xcs?.fontSize,
      };
    });
    console.log('font metrics:', JSON.stringify(metrics));

    // Drag-select a couple of rows in the middle of the screen.
    const rect = await page.evaluate(() => {
      const rows = document.querySelectorAll('.terminal-pane johnny-terminal-screen .xterm-rows > div');
      const a = rows[3].getBoundingClientRect();
      const b = rows[5].getBoundingClientRect();
      return { x1: a.x + 6, y1: a.y + a.height / 2, x2: b.right - 6, y2: b.y + b.height / 2 };
    });
    await page.mouse.move(rect.x1, rect.y1);
    await page.mouse.down();
    await page.mouse.move((rect.x1 + rect.x2) / 2, (rect.y1 + rect.y2) / 2, { steps: 6 });
    await page.mouse.move(rect.x2, rect.y2, { steps: 6 });
    await page.mouse.up();
    await delay(150);
    await page.screenshot({ path: path.join(ROOT, 'e2e-artifacts', 'mobile-terminal-review', 'desktop-selection.png') });
    const selRects = await page.evaluate(() => {
      const sel = document.querySelector('.terminal-pane johnny-terminal-screen .xterm-selection');
      const rows = document.querySelectorAll('.terminal-pane johnny-terminal-screen .xterm-rows > div');
      const rowRect = rows[4]?.getBoundingClientRect();
      const divs = Array.from(sel?.children || []).map((c) => { const r = c.getBoundingClientRect(); return { x: Math.round(r.x), y: Math.round(r.y), w: Math.round(r.width), h: Math.round(r.height) }; });
      return { rowH: rowRect ? Math.round(rowRect.height) : -1, rowY: rowRect ? Math.round(rowRect.y) : -1, selDivs: divs };
    });
    console.log('selection geometry:', JSON.stringify(selRects));

    const hasSel = await page.evaluate(() => {
      const t = document.querySelector('.terminal-pane johnny-terminal-screen .xterm-selection');
      return !!t && t.children.length > 0;
    });

    // Ctrl+C → fires the copy event our handler intercepts.
    await page.keyboard.down('Control');
    await page.keyboard.press('KeyC');
    await page.keyboard.up('Control');
    await delay(200);

    const clip = await page.evaluate(async () => { try { return await navigator.clipboard.readText(); } catch (e) { return 'ERR:' + e.message; } });
    const ok = /COPYME-/.test(clip);
    console.log(`selection rendered: ${hasSel}`);
    console.log(`clipboard: "${clip.slice(0, 80).replace(/\n/g, '\\n')}"`);
    console.log(ok ? '✅ desktop Ctrl/Cmd-C copied the terminal selection' : '❌ desktop copy did NOT work');
    failed = !ok;
    await page.close();
  } finally { await browser.close(); server.close(); }
  process.exitCode = failed ? 2 : 0;
}

main().catch((e) => { console.error(e); process.exit(1); });
