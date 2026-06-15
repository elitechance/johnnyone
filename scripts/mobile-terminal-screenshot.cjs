#!/usr/bin/env node
const fs = require('node:fs');
const http = require('node:http');
const path = require('node:path');
const puppeteer = require('puppeteer');

const ROOT = path.resolve(__dirname, '..');
const DIST = path.join(ROOT, 'dist', 'web', 'browser');
const PORT = Number(process.env.JOHNNYONE_SCREENSHOT_PORT || 4321);
const ARTIFACT_DIR = process.env.JOHNNYONE_SCREENSHOT_DIR
  || path.join(ROOT, 'e2e-artifacts', 'mobile-terminal-review');
const TENANT_ID = '00000000-0000-0000-0000-000000000001';

const VIEWPORTS = [
  { label: 'iphone-14', width: 390, height: 844, mobile: true },
  { label: 'iphone-14-pro-max', width: 430, height: 932, mobile: true },
  { label: 'desktop-1280', width: 1280, height: 800, mobile: false },
];

function delay(ms) {
  return new Promise((resolve) => setTimeout(resolve, ms));
}

function startStaticServer() {
  const mime = {
    '.html': 'text/html',
    '.js': 'text/javascript',
    '.css': 'text/css',
    '.json': 'application/json',
    '.png': 'image/png',
    '.svg': 'image/svg+xml',
    '.woff2': 'font/woff2',
    '.ico': 'image/x-icon',
  };

  const server = http.createServer((req, res) => {
    const urlPath = decodeURIComponent((req.url || '/').split('?')[0]);
    const filePath = path.join(DIST, urlPath === '/' ? 'index.html' : urlPath);
    const resolved = path.normalize(filePath);
    if (!resolved.startsWith(DIST)) {
      res.writeHead(403).end();
      return;
    }

    fs.readFile(resolved, (err, data) => {
      if (err) {
        fs.readFile(path.join(DIST, 'index.html'), (fallbackErr, fallbackData) => {
          if (fallbackErr) {
            res.writeHead(404).end('Not found');
            return;
          }
          res.writeHead(200, { 'Content-Type': 'text/html' });
          res.end(fallbackData);
        });
        return;
      }
      const ext = path.extname(resolved);
      res.writeHead(200, { 'Content-Type': mime[ext] || 'application/octet-stream' });
      res.end(data);
    });
  });

  return new Promise((resolve, reject) => {
    server.listen(PORT, '127.0.0.1', () => resolve(server));
    server.on('error', reject);
  });
}

function mockViewportChrome() {
  return [
    '\x1b[38;5;141mGrok Composer 2.5 Fast · always-approve\x1b[0m',
    '\x1b[38;5;240m╭────────────────────────────────────────────╮\x1b[0m',
    '\x1b[38;5;240m│\x1b[0m \x1b[38;5;245m▾ Tasks 3\x1b[0m',
    '\x1b[38;5;240m│\x1b[0m \x1b[38;5;245m⋅ running task\x1b[0m',
    '\x1b[38;5;240m│\x1b[0m',
    '\x1b[38;5;240m│ ❯\x1b[0m ',
    '█',
  ].join('\n');
}

function mockTerminalContent() {
  return [
    '\x1b[38;5;141mJohnnyOne mobile UI review — mock Grok session\x1b[0m',
    '\x1b[38;5;240m────────────────────────────────────────────\x1b[0m',
    '\x1b[48;5;52m\x1b[38;5;196mERROR: tool call failed — should not be a red bar\x1b[0m',
    '\x1b[38;5;196mValidation failed — fix before retry\x1b[0m',
    '\x1b[31mstandard red message\x1b[0m',
    '\x1b[91mbright red warning\x1b[0m',
    ...Array.from({ length: 22 }, (_, i) => (
      `\x1b[38;5;245m  [task ${i + 1}]\x1b[0m Implement scrolling + session tabs on mobile`
    )),
    '',
    '\x1b[38;5;141mHard-refresh should see full words like "mobile" and "another".\x1b[0m',
    '\x1b[38;5;39mDeployed\x1b[0m to production',
    '\x1b[48;5;236m\x1b[38;5;245m5. Remove gray bars from ANSI snapshot output\x1b[0m',
    'Verdict: PASS.',
    '❯ ',
  ].join('\n');
}

function terminalPayload(sessionId) {
  const content = mockTerminalContent();

  return {
    sessionId,
    tmuxSessionName: 'jo-mobile-review',
    paneId: '%0',
    status: 'attached',
    cursor: Date.now(),
    content,
    cursorX: 2,
    cursorY: 22,
    historyLines: 420,
    cols: 80,
    rows: 24,
  };
}

function createSession(id, provider, title) {
  const now = new Date().toISOString();
  return {
    id,
    title,
    provider,
    model: provider === 'ollama' ? 'qwen3.5:2b' : '',
    workingDirectory: '/home/creepy/Documents/Workspace',
    status: 'active',
    totalInputTokens: 1200,
    totalOutputTokens: 4800,
    totalCostCents: 12,
    createdAt: now,
    updatedAt: now,
  };
}

async function primeAuthAndRelay(page) {
  const mockContent = mockTerminalContent();
  const mockChrome = mockViewportChrome();
  await page.evaluateOnNewDocument((tenantId, terminalContent, viewportChrome) => {
    const user = {
      id: 'mobile-screenshot-user',
      tenantId,
      email: 'admin@johnnyone.local',
      displayName: 'Mobile Screenshot',
      roles: ['admin'],
      status: 'active',
    };
    window.localStorage.setItem('johnnyone_worker_url', 'http://127.0.0.1:7714');
    window.localStorage.setItem('johnnyone_access_token', 'mobile-screenshot-token');
    window.localStorage.setItem('johnnyone_refresh_token', 'mobile-screenshot-refresh');
    window.localStorage.setItem('johnnyone_tenant_id', tenantId);
    window.localStorage.setItem('johnnyone_user_id', user.id);
    window.localStorage.setItem('johnnyone_auth_user', JSON.stringify(user));

    class MockRelayWebSocket {
      static OPEN = 1;
      static CONNECTING = 0;

      constructor() {
        window.WebSocket._lastInstance = this;
        this.readyState = MockRelayWebSocket.CONNECTING;
        setTimeout(() => {
          this.readyState = MockRelayWebSocket.OPEN;
          this.onopen?.({});
        }, 0);
      }

      send(raw) {
        let envelope;
        try {
          envelope = JSON.parse(raw);
        } catch {
          return;
        }
        const data = envelope.data || {};
        if (envelope.type === 'terminal_command' && data.control === 'visual_subscribe' && data.sessionId) {
          this.emitScreen(data.sessionId, viewportChrome, 24);
          setTimeout(() => this.emitScreen(data.sessionId, terminalContent, 420), 350);
        }
        if (envelope.type === 'terminal_command' && data.control === 'visual_refresh' && data.sessionId) {
          this.emitScreen(data.sessionId, terminalContent, 420);
        }
        if (envelope.type === 'terminal_command' && data.control === 'visual_history' && data.sessionId) {
          setTimeout(() => this.emitScreen(data.sessionId, terminalContent, 420), 120);
        }
      }

      emitScreen(sessionId, content, historyLines) {
        this.onmessage?.({
          data: JSON.stringify({
            type: 'terminal_screen',
            data: {
              sessionId,
              tmuxSessionName: 'jo-mobile-review',
              paneId: '%0',
              status: 'attached',
              cursor: Date.now(),
              content,
              cursorX: 2,
              cursorY: 22,
              historyLines,
              cols: 80,
              rows: 24,
            },
          }),
        });
      }

      startPeriodicChrome(sessionId, chrome, intervalMs = 2000) {
        if (this._chromeTimer) clearInterval(this._chromeTimer);
        this._chromeTimer = setInterval(() => {
          this.emitScreen(sessionId, chrome, 24);
        }, intervalMs);
      }

      stopPeriodicChrome() {
        if (this._chromeTimer) {
          clearInterval(this._chromeTimer);
          this._chromeTimer = null;
        }
      }

      close() {}
      addEventListener(type, handler) { this[`on${type}`] = handler; }
      removeEventListener(type) { this[`on${type}`] = null; }
    }

    window.WebSocket = MockRelayWebSocket;
  }, TENANT_ID, mockContent, mockChrome);
}

async function mockGraphql(page, sessions) {
  await page.setRequestInterception(true);
  page.on('request', (request) => {
    if (!request.url().includes('/graphql')) {
      request.continue();
      return;
    }
    if (request.method() === 'OPTIONS') {
      request.respond({
        status: 204,
        headers: {
          'Access-Control-Allow-Origin': '*',
          'Access-Control-Allow-Headers': '*',
          'Access-Control-Allow-Methods': 'POST, OPTIONS',
        },
      });
      return;
    }

    const body = request.postData() || '';
    const variables = JSON.parse(body || '{}').variables || {};
    let data = {};

    if (body.includes('listAiSessions')) {
      data = { listAiSessions: sessions };
    } else if (body.includes('getAiSession')) {
      data = { getAiSession: sessions.find((s) => s.id === variables.id) || sessions[0] };
    } else if (body.includes('listAiMessages')) {
      data = { listAiMessages: [] };
    } else if (body.includes('listDetectedCliTools')) {
      data = {
        listDetectedCliTools: [
          { provider: 'grok', command: 'grok', found: true, path: '/usr/bin/grok' },
          { provider: 'ollama', command: 'ollama', found: true, path: '/usr/bin/ollama' },
          { provider: 'shell', command: 'bash', found: true, path: '/bin/bash' },
        ],
      };
    } else if (body.includes('getSetting')) {
      data = { getSetting: '~/projects/demo' };
    } else if (body.includes('listDesktopNodes')) {
      data = {
        listDesktopNodes: [{
          id: 'mock-node',
          hostname: 'mock-host',
          os: 'linux',
          arch: 'x64',
          version: 'review',
          status: 'online',
          capabilities: [],
          lastHeartbeatAt: new Date().toISOString(),
          createdAt: new Date().toISOString(),
        }],
      };
    }

    request.respond({
      status: 200,
      contentType: 'application/json',
      headers: { 'Access-Control-Allow-Origin': '*' },
      body: JSON.stringify({ data }),
    });
  });
}

async function screenshot(page, name) {
  fs.mkdirSync(ARTIFACT_DIR, { recursive: true });
  const filePath = path.join(ARTIFACT_DIR, `${name}.png`);
  await page.screenshot({ path: filePath, fullPage: false });
  console.log(`saved ${filePath}`);
  return filePath;
}

async function captureViewport(browser, viewport, sessions) {
  const page = await browser.newPage();
  await page.setViewport({
    width: viewport.width,
    height: viewport.height,
    isMobile: !!viewport.mobile,
    hasTouch: !!viewport.mobile,
    deviceScaleFactor: viewport.mobile ? 2 : 1,
  });

  await primeAuthAndRelay(page);
  await mockGraphql(page, sessions);

  await page.goto(`http://127.0.0.1:${PORT}/terminal`, { waitUntil: 'networkidle2', timeout: 60000 });
  await page.waitForSelector('.workspace-shell', { timeout: 30000 });
  if (viewport.mobile) {
    await page.waitForSelector('ion-segment.workspace-session-segment', { timeout: 15000 });
  } else {
    await page.waitForSelector('.workspace-tabs .workspace-tab', { timeout: 15000 });
  }
  await page.waitForSelector('.terminal-pane johnny-terminal-screen .xterm-rows', { timeout: 30000 });
  await delay(1200);
  if (viewport.mobile) {
    await page.waitForFunction(() => {
      const text = document.querySelector('.terminal-pane johnny-terminal-screen .xterm-rows')?.textContent || '';
      return /Hard-refresh should see full words/i.test(text) && /Validation failed/i.test(text);
    }, { timeout: 15000 });
  }
  await delay(300);

  const metrics = await page.evaluate(() => {
    const host = document.querySelector('.terminal-pane johnny-terminal-screen .terminal-host');
    const xterm = document.querySelector('.terminal-pane johnny-terminal-screen .xterm');
    const screen = document.querySelector('.terminal-pane johnny-terminal-screen .xterm-screen');
    const viewport = document.querySelector('.terminal-pane johnny-terminal-screen .xterm-viewport');
    const cwd = document.querySelector('.terminal-pane .pane-tool.pane-cwd')?.textContent?.trim() || '';
    const input = document.querySelector('.terminal-pane johnny-terminal-screen textarea[name="terminalInput"]');
    const inputRect = input?.getBoundingClientRect();
    const rows = document.querySelector('.terminal-pane johnny-terminal-screen .xterm-rows');
    const text = rows?.textContent || '';
    const deployedSpan = Array.from(rows?.querySelectorAll('span') || [])
      .find((span) => (span.textContent || '').includes('Deployed'));
    const deployedColor = deployedSpan ? getComputedStyle(deployedSpan).color : '';
    const errorSpan = Array.from(rows?.querySelectorAll('span') || [])
      .find((span) => (span.textContent || '').includes('ERROR: tool call failed'));
    const errorColor = errorSpan ? getComputedStyle(errorSpan).color : '';
    const errorBg = errorSpan ? getComputedStyle(errorSpan).backgroundColor : '';
    const validationSpan = Array.from(rows?.querySelectorAll('span') || [])
      .find((span) => (span.textContent || '').includes('Validation failed'));
    const validationColor = validationSpan ? getComputedStyle(validationSpan).color : '';
    const hasVisibleCsiJunk = /\[[0-9;]*m/.test(text);
    const taskRow = Array.from(rows?.children || []).find((row) => /\[task 1\]/i.test(row.textContent || ''));
    const taskText = taskRow?.textContent || '';
    const spans = taskRow ? Array.from(taskRow.querySelectorAll('span')) : [];
    const lastSpan = spans[spans.length - 1];
    const hostRect = host?.getBoundingClientRect();
    const viewportRect = viewport?.getBoundingClientRect();
    const screenRect = screen?.getBoundingClientRect();
    const lastRect = lastSpan?.getBoundingClientRect();
    const taskRowRect = taskRow?.getBoundingClientRect();
    const viewportRight = viewportRect?.right ?? 0;
    const rowOverflowsViewport = taskRowRect ? taskRowRect.right > viewportRight - 3 : null;
    const hostStyle = host ? getComputedStyle(host) : null;
    return {
      hostWidth: host?.clientWidth ?? 0,
      hostPadding: hostStyle?.padding,
      xtermWidth: xterm?.clientWidth ?? 0,
      viewportWidth: viewport?.clientWidth ?? 0,
      screenWidth: screen?.clientWidth ?? 0,
      screenWidthUtilization: viewport?.clientWidth
        ? (screen?.clientWidth ?? 0) / viewport.clientWidth
        : 0,
      screenWiderThanViewport: (screen?.clientWidth ?? 0) > (viewport?.clientWidth ?? 0) + 1,
      visibleTextSample: text.slice(0, 240),
      hasWrapArtifact: /\ble \[task/i.test(text),
      lineStartsCorrectly: /\[task 1\]/i.test(text) || /JohnnyOne mobile UI review/i.test(text),
      taskRowText: taskText.slice(0, 120),
      taskRowLen: taskText.length,
      taskShowsMobile: /tabs on mobile/i.test(text),
      showsFullAnother: /another/i.test(text),
      showsRemoveWord: /Remove gray bars/i.test(text),
      taskTruncated: /tabs on mo\b/i.test(text) && !/tabs on mobile/i.test(text),
      cwdLabel: cwd,
      cwdShortened: cwd.includes('~/Documents/Workspace') && !cwd.includes('/home/creepy'),
      lastChar: lastSpan?.textContent ?? '',
      lastSpanRight: lastRect?.right ?? 0,
      viewportRight: viewportRect?.right ?? 0,
      hostRight: hostRect?.right ?? 0,
      textClippedByViewport: lastRect && viewportRect ? lastRect.right > viewportRect.right - 1 : null,
      rowOverflowsViewport,
      taskRowRight: taskRowRect?.right ?? 0,
      inputVisible: !!input && (inputRect?.height ?? 0) > 0
        && (inputRect?.bottom ?? 0) <= window.innerHeight + 2
        && (inputRect?.top ?? 0) < window.innerHeight,
      inputRect: inputRect
        ? { top: inputRect.top, bottom: inputRect.bottom, height: inputRect.height }
        : null,
      windowHeight: window.innerHeight,
      deployedColor,
      errorColor,
      errorBg,
      validationColor,
      showsErrorLine: /ERROR: tool call failed/i.test(text),
      showsValidationLine: /Validation failed/i.test(text),
      hasVisibleCsiJunk,
      showsDeployedWord: /Deployed/i.test(text),
      showsLatestResponseLive: /Hard-refresh should see full words/i.test(text),
      historyPanelOpen: !!document.querySelector('.terminal-pane johnny-terminal-screen .history-viewer-open'),
    };
  });
  console.log(`${viewport.label} metrics:`, JSON.stringify(metrics));
  if (metrics.screenWiderThanViewport || metrics.textClippedByViewport || metrics.rowOverflowsViewport) {
    throw new Error(`${viewport.label}: terminal text is clipped (${JSON.stringify({
      screenWiderThanViewport: metrics.screenWiderThanViewport,
      textClippedByViewport: metrics.textClippedByViewport,
      screenWidth: metrics.screenWidth,
      viewportWidth: metrics.viewportWidth,
    })})`);
  }
  if (!metrics.inputVisible) {
    throw new Error(`${viewport.label}: composer input is not visible (${JSON.stringify({
      inputRect: metrics.inputRect,
      windowHeight: metrics.windowHeight,
    })})`);
  }
  if (viewport.mobile && !metrics.cwdShortened) {
    throw new Error(`${viewport.label}: pane cwd not shortened (${JSON.stringify({
      cwdLabel: metrics.cwdLabel,
    })})`);
  }
  if (viewport.mobile && metrics.hasVisibleCsiJunk) {
    throw new Error(`${viewport.label}: visible ANSI CSI junk in terminal text`);
  }
  if (
    viewport.mobile
    && metrics.showsDeployedWord
    && metrics.deployedColor === 'rgb(215, 225, 238)'
    && !metrics.showsLatestResponseLive
  ) {
    throw new Error(`${viewport.label}: "Deployed" rendered without ANSI color (${metrics.deployedColor})`);
  }
  if (viewport.mobile && metrics.screenWidthUtilization < 0.82) {
    throw new Error(`${viewport.label}: terminal underuses horizontal space (${JSON.stringify({
      screenWidth: metrics.screenWidth,
      viewportWidth: metrics.viewportWidth,
      screenWidthUtilization: metrics.screenWidthUtilization,
    })})`);
  }
  if (!metrics.taskShowsMobile || !metrics.showsFullAnother || !metrics.showsRemoveWord) {
    throw new Error(`${viewport.label}: terminal text still cropped (${JSON.stringify({
      taskShowsMobile: metrics.taskShowsMobile,
      showsFullAnother: metrics.showsFullAnother,
      showsRemoveWord: metrics.showsRemoveWord,
      taskRowText: metrics.taskRowText,
    })})`);
  }
  if (viewport.mobile && (!metrics.showsErrorLine || !metrics.showsValidationLine)) {
    throw new Error(`${viewport.label}: Grok error lines missing (${JSON.stringify({
      showsErrorLine: metrics.showsErrorLine,
      showsValidationLine: metrics.showsValidationLine,
    })})`);
  }
  if (viewport.mobile && metrics.errorBg && metrics.errorBg !== 'rgba(0, 0, 0, 0)' && metrics.errorBg !== 'rgb(5, 7, 11)') {
    throw new Error(`${viewport.label}: Grok error line still has red background (${metrics.errorBg})`);
  }
  if (viewport.mobile && metrics.errorColor === 'rgb(255, 0, 0)') {
    throw new Error(`${viewport.label}: Grok error still pure red (${metrics.errorColor})`);
  }
  if (viewport.mobile && metrics.validationColor === 'rgb(255, 0, 0)') {
    throw new Error(`${viewport.label}: Grok validation still pure red (${metrics.validationColor})`);
  }
  if (viewport.mobile && !metrics.showsLatestResponseLive) {
    throw new Error(`${viewport.label}: latest Grok response missing from live terminal (${JSON.stringify({
      showsLatestResponseLive: metrics.showsLatestResponseLive,
      historyPanelOpen: metrics.historyPanelOpen,
      visibleTextSample: metrics.visibleTextSample,
    })})`);
  }
  if (viewport.mobile && metrics.historyPanelOpen) {
    throw new Error(`${viewport.label}: history panel should stay closed for live-view test`);
  }

  await screenshot(page, `${viewport.label}-terminal-overview`);

  if (viewport.mobile) {
    const sessionId = sessions[0].id;
    const baselineText = await page.evaluate(() => (
      document.querySelector('.terminal-pane johnny-terminal-screen .xterm-rows')?.textContent || ''
    ));
    await page.evaluate((id, chrome) => {
      const ws = window.WebSocket;
      const relay = ws?._lastInstance;
      if (relay?.startPeriodicChrome) {
        relay.startPeriodicChrome(id, chrome, 400);
      }
    }, sessionId, mockViewportChrome());
    await delay(1800);
    const afterTicksText = await page.evaluate(() => (
      document.querySelector('.terminal-pane johnny-terminal-screen .xterm-rows')?.textContent || ''
    ));
    await page.evaluate(() => {
      const ws = window.WebSocket;
      const relay = ws?._lastInstance;
      relay?.stopPeriodicChrome?.();
    });
    if (!/Hard-refresh should see full words/i.test(afterTicksText)) {
      throw new Error(`${viewport.label}: periodic chrome updates wiped live response (${JSON.stringify({
        baselineHasResponse: /Hard-refresh should see full words/i.test(baselineText),
        afterTicksSample: afterTicksText.slice(0, 240),
      })})`);
    }
    if (baselineText.length > 0 && afterTicksText.length < baselineText.length * 0.6) {
      throw new Error(`${viewport.label}: periodic chrome updates collapsed terminal text (${JSON.stringify({
        baselineLen: baselineText.length,
        afterTicksLen: afterTicksText.length,
      })})`);
    }
  }

  // History panel + Refresh button were removed (reading-back is covered by
  // finger-scroll; mermaid/transcript plumbing retired). Only Live remains.

  if (viewport.mobile) {
    await page.evaluate(() => {
      const segment = document.querySelector('ion-segment.workspace-session-segment');
      if (segment) segment.scrollLeft = segment.scrollWidth;
    });
    await delay(300);
    await screenshot(page, `${viewport.label}-session-tabs-scrolled`);

    await page.click('ion-segment-button:nth-child(2)');
    await delay(700);
    await screenshot(page, `${viewport.label}-second-session`);
  }

  await page.close();
}

async function main() {
  if (!fs.existsSync(DIST)) {
    console.error(`Missing build at ${DIST}. Run: npx nx build web`);
    process.exit(1);
  }

  const sessions = [
    createSession('aaaa1111-0000-0000-0000-000000000001', 'grok', 'Grok dev'),
    createSession('bbbb2222-0000-0000-0000-000000000002', 'ollama', 'Ollama chat'),
    createSession('cccc3333-0000-0000-0000-000000000003', 'shell', 'Shell ops'),
    createSession('dddd4444-0000-0000-0000-000000000004', 'codex', 'Codex review'),
  ];

  const server = await startStaticServer();
  console.log(`Serving ${DIST} at http://127.0.0.1:${PORT}`);

  const browser = await puppeteer.launch({
    headless: true,
    args: ['--no-sandbox', '--disable-setuid-sandbox'],
  });

  try {
    for (const viewport of VIEWPORTS) {
      await captureViewport(browser, viewport, sessions);
    }
    console.log(`\nAll screenshots: ${ARTIFACT_DIR}`);
  } finally {
    await browser.close();
    server.close();
  }
}

main().catch((err) => {
  console.error(err);
  process.exit(1);
});