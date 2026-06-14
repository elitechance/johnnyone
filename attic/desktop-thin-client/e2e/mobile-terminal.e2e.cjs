const fs = require('node:fs');
const path = require('node:path');
const puppeteer = require('puppeteer');

const APP_URL = process.env.JOHNNYONE_E2E_URL || 'http://127.0.0.1:4200';
const ARTIFACT_DIR = process.env.JOHNNYONE_E2E_ARTIFACT_DIR
  || path.resolve(__dirname, 'artifacts/mobile-terminal');
const TENANT_ID = process.env.JOHNNYONE_E2E_TENANT_ID || '00000000-0000-0000-0000-000000000001';

function assert(condition, message) {
  if (!condition) throw new Error(message);
}

function delay(ms) {
  return new Promise((resolve) => setTimeout(resolve, ms));
}

async function screenshot(page, name) {
  fs.mkdirSync(ARTIFACT_DIR, { recursive: true });
  const filePath = path.join(ARTIFACT_DIR, `${name}.png`);
  await page.screenshot({ path: filePath, fullPage: true });
  console.log(`mobile-terminal.e2e: screenshot ${filePath}`);
}

async function primeAuthAndRelay(page) {
  await page.evaluateOnNewDocument((tenantId) => {
    const user = {
      id: 'mobile-terminal-e2e-user',
      tenantId,
      email: 'admin@johnnyone.local',
      displayName: 'Mobile Terminal E2E',
      roles: ['admin'],
      status: 'active',
    };
    window.localStorage.setItem('johnnyone_worker_url', 'http://127.0.0.1:7714');
    window.localStorage.setItem('johnnyone_access_token', 'mobile-terminal-e2e-token');
    window.localStorage.setItem('johnnyone_refresh_token', 'mobile-terminal-e2e-refresh-token');
    window.localStorage.setItem('johnnyone_tenant_id', tenantId);
    window.localStorage.setItem('johnnyone_user_id', user.id);
    window.localStorage.setItem('johnnyone_auth_user', JSON.stringify(user));

    window.__johnnyoneTerminalInputs = [];
    window.__johnnyoneTerminalScreens = {};

    class MockRelayWebSocket {
      static CONNECTING = 0;
      static OPEN = 1;
      static CLOSING = 2;
      static CLOSED = 3;

      constructor() {
        this.readyState = MockRelayWebSocket.CONNECTING;
        setTimeout(() => {
          this.readyState = MockRelayWebSocket.OPEN;
          this.onopen?.({ type: 'open' });
        }, 0);
      }

      send(raw) {
        const envelope = JSON.parse(raw);
        const data = envelope.data || {};
        if (envelope.type === 'terminal_command') {
          const sessionId = data.sessionId;
          if (data.data) {
            window.__johnnyoneTerminalInputs.push(data.data);
          }
          if (sessionId && (data.control || data.data)) {
            this.emitScreen(sessionId, data.data || '');
          }
          this.onmessage?.({
            data: JSON.stringify({
              type: 'terminal_command_ack',
              data: { requestId: data.requestId, sessionId, accepted: true },
            }),
          });
        }
      }

      close() {
        this.readyState = MockRelayWebSocket.CLOSED;
        this.onclose?.({ type: 'close' });
      }

      addEventListener(type, handler) {
        this[`on${type}`] = handler;
      }

      removeEventListener(type) {
        this[`on${type}`] = null;
      }

      emitScreen(sessionId, input) {
        const previous = window.__johnnyoneTerminalScreens[sessionId] || 'JohnnyOne mock terminal\n>>> ';
        const normalized = input
          ? `${previous}${input.replace(/\r/g, '\n')}\n>>> `
          : previous;
        window.__johnnyoneTerminalScreens[sessionId] = normalized;
        this.onmessage?.({
          data: JSON.stringify({
            type: 'terminal_screen',
            data: {
              sessionId,
              status: 'attached',
              cursor: Date.now(),
              content: normalized,
              cols: 80,
              rows: 24,
            },
          }),
        });
      }
    }

    window.WebSocket = MockRelayWebSocket;
  }, TENANT_ID);
}

function createSession(id, provider = 'ollama') {
  return {
    id,
    title: 'New Session',
    provider,
    model: provider === 'ollama' ? 'qwen3.5:2b' : '',
    workingDirectory: '/home/creepy/documents/workspace',
    status: 'active',
    totalInputTokens: 0,
    totalOutputTokens: 0,
    totalCostCents: 0,
    createdAt: new Date().toISOString(),
    updatedAt: new Date().toISOString(),
  };
}

async function mockGraphql(page) {
  const sessions = [];

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
    const parsed = JSON.parse(body || '{}');
    const variables = parsed.variables || {};
    let data = {};

    if (body.includes('listAiSessions')) {
      data = { listAiSessions: sessions };
    } else if (body.includes('createAiSession')) {
      const session = createSession(`mobile-session-${sessions.length + 1}`, variables.input?.provider || 'ollama');
      sessions.unshift(session);
      data = { createAiSession: session };
    } else if (body.includes('getAiSession')) {
      data = { getAiSession: sessions.find((session) => session.id === variables.id) || sessions[0] };
    } else if (body.includes('listAiMessages')) {
      data = { listAiMessages: [] };
    } else if (body.includes('updateAiSessionArchived')) {
      const index = sessions.findIndex((session) => session.id === variables.id);
      const archived = { ...(sessions[index] || createSession(variables.id)), status: 'archived' };
      if (index >= 0) sessions.splice(index, 1);
      data = { updateAiSessionArchived: archived };
    } else if (body.includes('listDetectedCliTools')) {
      data = { listDetectedCliTools: [{ provider: 'ollama', command: 'ollama', found: true, path: '/usr/bin/ollama' }] };
    } else if (body.includes('getSetting')) {
      data = { getSetting: '/home/creepy/documents/workspace' };
    } else if (body.includes('listDesktopNodes')) {
      data = {
        listDesktopNodes: [{
          id: 'mock-node',
          hostname: 'mock-host',
          os: 'linux',
          arch: 'x64',
          version: 'e2e',
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

async function waitForAttachedPane(page) {
  await page.waitForSelector('.terminal-pane.active johnny-terminal-screen .xterm-rows', { timeout: 30000 });
  await page.waitForFunction(
    () => (document.querySelector('.terminal-pane.active .pane-status')?.textContent || '')
      .toLowerCase()
      .includes('attached'),
    { timeout: 30000 },
  );
}

async function geometry(page) {
  return page.evaluate(() => {
    const pane = document.querySelector('.terminal-pane.active');
    const workspace = document.querySelector('.terminal-workspace');
    const host = document.querySelector('.terminal-pane.active johnny-terminal-screen .terminal-host');
    const input = document.querySelector('.terminal-pane.active johnny-terminal-screen .terminal-mobile-input');
    const textarea = document.querySelector('.terminal-pane.active johnny-terminal-screen textarea[name="terminalInput"]');
    const rect = (element) => {
      const box = element.getBoundingClientRect();
      return {
        left: box.left,
        right: box.right,
        top: box.top,
        bottom: box.bottom,
        width: box.width,
        height: box.height,
      };
    };
    return {
      viewportWidth: window.innerWidth,
      documentWidth: document.documentElement.scrollWidth,
      pane: pane ? rect(pane) : null,
      workspace: workspace ? rect(workspace) : null,
      host: host ? rect(host) : null,
      input: input ? rect(input) : null,
      textareaFocused: document.activeElement === textarea,
    };
  });
}

async function pasteIntoComposer(page, text) {
  await page.$eval(
    '.terminal-pane.active johnny-terminal-screen textarea[name="terminalInput"]',
    (element, value) => {
      const clipboardData = new DataTransfer();
      clipboardData.setData('text/plain', value);
      element.dispatchEvent(new ClipboardEvent('paste', {
        bubbles: true,
        cancelable: true,
        clipboardData,
      }));
    },
    text,
  );
}

async function run() {
  fs.mkdirSync(ARTIFACT_DIR, { recursive: true });
  const browser = await puppeteer.launch({
    headless: true,
    args: ['--no-sandbox', '--disable-setuid-sandbox'],
  });

  try {
    const page = await browser.newPage();
    await page.setViewport({ width: 390, height: 844, isMobile: true, hasTouch: true });
    await primeAuthAndRelay(page);
    await mockGraphql(page);
    page.on('pageerror', (error) => console.error(`[pageerror] ${error.message}`));

    await page.goto(`${APP_URL.replace(/\/$/, '')}/terminal`, { waitUntil: 'networkidle2', timeout: 60000 });
    await page.waitForSelector('.workspace-shell', { timeout: 30000 });

    await page.click('.workspace-actions .primary-action');
    await waitForAttachedPane(page);
    await screenshot(page, '01-01-390x844-terminal-active-pane');

    let metrics = await geometry(page);
    assert(metrics.documentWidth <= metrics.viewportWidth + 2, `mobile terminal has horizontal overflow: ${JSON.stringify(metrics)}`);
    assert(metrics.pane && metrics.workspace && Math.abs(metrics.pane.height - (metrics.workspace.height - 20)) <= 2, `active pane should fill padded workspace: ${JSON.stringify(metrics)}`);
    assert(metrics.host.bottom <= metrics.input.top + 2, `terminal output overlaps composer: ${JSON.stringify(metrics)}`);

    await page.click('.workspace-actions .primary-action');
    await waitForAttachedPane(page);
    await page.waitForFunction(
      () => document.querySelectorAll('ion-segment.workspace-session-segment ion-segment-button').length === 2,
      { timeout: 15000 },
    );
    const visiblePaneCount = await page.$$eval('.terminal-pane', (panes) => panes.length);
    assert(visiblePaneCount === 1, `mobile should render one visible pane, found ${visiblePaneCount}`);

    await page.click('ion-segment.workspace-session-segment ion-segment-button:nth-child(2)');
    await waitForAttachedPane(page);
    assert(await page.$$eval('.terminal-pane', (panes) => panes.length) === 1, 'switching tabs should still render one mobile pane');

    const inputSelector = '.terminal-pane.active johnny-terminal-screen textarea[name="terminalInput"]';
    await page.focus(inputSelector);
    await page.keyboard.type('mobile space test');
    await pasteIntoComposer(page, '\npasted line one\npasted line two');
    await delay(250);

    const inputBeforeSubmit = await page.$eval(inputSelector, (element) => element.value);
    assert(inputBeforeSubmit.includes('mobile space test'), 'typed spaces should remain visible in composer');
    assert(inputBeforeSubmit.includes('pasted line one\npasted line two'), 'multiline paste should remain visible in composer');
    assert(await page.evaluate(() => window.__johnnyoneTerminalInputs.length) === 0, 'composer should not send before submit');
    await screenshot(page, '01-02-390x844-terminal-composer-paste');

    await page.keyboard.press('Enter');
    await page.waitForFunction(() => window.__johnnyoneTerminalInputs.length === 1, { timeout: 5000 });
    const sent = await page.evaluate(() => window.__johnnyoneTerminalInputs[0]);
    assert(sent.includes('mobile space test'), `submitted input missing typed text: ${sent}`);
    assert(sent.includes('pasted line two'), `submitted input missing pasted text: ${sent}`);
    assert(sent.endsWith('\r'), `submitted input should end with carriage return: ${JSON.stringify(sent)}`);

    await page.focus(inputSelector);
    await page.click('.terminal-mobile-keys button:nth-child(3)');
    await page.waitForFunction(() => window.__johnnyoneTerminalInputs.includes('\u0003'), { timeout: 5000 });
    metrics = await geometry(page);
    assert(metrics.textareaFocused, 'control buttons should return focus to the composer');

    await page.setViewport({ width: 430, height: 932, isMobile: true, hasTouch: true });
    await delay(350);
    await screenshot(page, '01-01-430x932-terminal-active-pane');
    metrics = await geometry(page);
    assert(metrics.documentWidth <= metrics.viewportWidth + 2, `430px mobile terminal has horizontal overflow: ${JSON.stringify(metrics)}`);
    assert(metrics.host.bottom <= metrics.input.top + 2, `430px terminal output overlaps composer: ${JSON.stringify(metrics)}`);

    console.log('mobile-terminal.e2e: passed');
  } finally {
    await browser.close();
  }
}

run().catch((error) => {
  console.error(error);
  process.exitCode = 1;
});
