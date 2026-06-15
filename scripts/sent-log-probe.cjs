#!/usr/bin/env node
// Verifies (1) the "Your messages" sent-log captures a submitted message, and
// (2) long-press on the terminal copies the visible screen to the clipboard.
const fs = require('node:fs');
const http = require('node:http');
const path = require('node:path');
const puppeteer = require('puppeteer');

const ROOT = path.resolve(__dirname, '..');
const DIST = path.join(ROOT, 'dist', 'web', 'browser');
const PORT = Number(process.env.JOHNNYONE_PROBE_PORT || 4325);
const TENANT_ID = '00000000-0000-0000-0000-000000000001';
const delay = (ms) => new Promise((r) => setTimeout(r, ms));

function frame() {
  return [
    '\x1b[38;5;141mSent-log probe\x1b[0m',
    ...Array.from({ length: 40 }, (_, i) => `line ${String(i + 1).padStart(3, '0')} hello world`),
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
    const user = { id: 'sentlog-user', tenantId, email: 'admin@johnnyone.local', displayName: 'Sent', roles: ['admin'], status: 'active' };
    localStorage.setItem('johnnyone_worker_url', 'http://127.0.0.1:7714');
    localStorage.setItem('johnnyone_access_token', 'sentlog-token');
    localStorage.setItem('johnnyone_refresh_token', 'sentlog-refresh');
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
    const ctx = browser.defaultBrowserContext();
    await ctx.overridePermissions(`http://127.0.0.1:${PORT}`, ['clipboard-read', 'clipboard-write']);
    const page = await browser.newPage();
    page.on('console', (m) => { const t = m.text(); if (/copy|clip|toast|grab/i.test(t)) console.log('C>', t.slice(0, 120)); });
    await page.setViewport({ width: 390, height: 844, isMobile: true, hasTouch: true, deviceScaleFactor: 2 });
    await page.evaluateOnNewDocument((src) => { window.__frame = new Function(`return (${src})();`); }, frame.toString());
    await primeAuthAndRelay(page);
    await mockGraphql(page, sessions);
    await page.goto(`http://127.0.0.1:${PORT}/terminal`, { waitUntil: 'networkidle2', timeout: 60000 });
    await page.waitForSelector('.terminal-pane johnny-terminal-screen textarea[name="terminalInput"]', { timeout: 30000 });
    await delay(1000);

    // 1) Type a message and Send → expect it in the sent-log.
    const MSG = 'fix the login button please';
    await page.type('.terminal-pane johnny-terminal-screen textarea[name="terminalInput"]', MSG);
    await page.click('.terminal-pane johnny-terminal-screen .terminal-mobile-submit button[type="submit"]');
    await delay(400);
    const toggle = await page.$('.terminal-pane johnny-terminal-screen .terminal-sent-log-toggle');
    const toggleText = toggle ? await page.evaluate((el) => el.textContent.trim(), toggle) : '(none)';
    if (toggle) await toggle.click();
    await delay(200);
    const logText = await page.evaluate(() => (document.querySelector('.terminal-pane johnny-terminal-screen .terminal-sent-log-body')?.textContent || '').trim());
    const sentOk = logText.includes(MSG);
    console.log(`sent-log toggle: "${toggleText}"`);
    console.log(`sent-log body:   "${logText}"`);
    console.log(sentOk ? '✅ sent-log captured the submitted message' : '❌ sent-log did NOT capture the message');
    failed = failed || !sentOk;

    // 2) Long-press the terminal → expect visible text copied to clipboard.
    const box = await page.evaluate(() => {
      const el = document.querySelector('.terminal-pane johnny-terminal-screen .terminal-host');
      const r = el.getBoundingClientRect();
      return { x: Math.round(r.x + r.width / 2), y: Math.round(r.y + r.height / 2) };
    });
    const cdp = await page.createCDPSession();
    await cdp.send('Input.dispatchTouchEvent', { type: 'touchStart', touchPoints: [{ x: box.x, y: box.y }] });
    await delay(650); // exceed 450ms long-press, no movement
    await cdp.send('Input.dispatchTouchEvent', { type: 'touchEnd', touchPoints: [] });
    await delay(300);
    const toastText = await page.evaluate(() => (document.querySelector('.terminal-pane johnny-terminal-screen .terminal-copy-toast')?.textContent || '(no toast)').trim());
    console.log(`toast: "${toastText}"`);
    const clip = await page.evaluate(async () => { try { return await navigator.clipboard.readText(); } catch (e) { return 'ERR:' + e.message; } });
    const copyOk = /line 0\d\d hello world/.test(clip);
    console.log(`clipboard after long-press: "${clip.slice(0, 60).replace(/\n/g, '\\n')}..."`);
    console.log(copyOk ? '✅ long-press copied the screen text' : '❌ long-press did NOT copy screen text');
    failed = failed || !copyOk;

    await page.close();
  } finally { await browser.close(); server.close(); }
  process.exitCode = failed ? 2 : 0;
}

main().catch((e) => { console.error(e); process.exit(1); });
